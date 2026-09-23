#include "mqtt_client.h"

#include <Arduino.h>
#include <PubSubClient.h>
#include <WiFiClient.h>
#include <esp_task_wdt.h>
#include <freertos/FreeRTOS.h>

#include <vdm/config.h>
#include <vdm/ha_discovery.h>
#include <vdm/health_monitor.h>
#include <vdm/json_writer.h>
#include <vdm/mqtt_topics.h>

#include "app.h"
#include "logger.h"
#include "net.h"
#include "storage.h"

namespace mqtt {

namespace {

WiFiClient gNet;
PubSubClient gClient(gNet);

// Task-owned copies (static: too large for the stack).
vdm::Config gCfg;
uint32_t gCfgRevision = 0;
app::StmSnapshot gSnap;
vdm::TopicContext gTopics;
char gSegments[vdm::kValveCount][vdm::kSegmentMax + 1];
char gHost[vdm::kHostMax + 1];  // PubSubClient keeps the pointer
char gLwtTopic[vdm::kTopicMax + 1];
vdm::PublishScheduler gScheduler;
vdm::EventRateLimiter gEventLimiter;
uint32_t gEventCursor = 0;
bool gFirstPublish = true;  // common/ip goes out only on the first full publish per connection

portMUX_TYPE gMux = portMUX_INITIALIZER_UNLOCKED;
Status gStatus;
volatile bool gReconnectRequested = false;
volatile bool gDiscoveryRequested = false;
volatile DiscoveryAction gDiscoveryAction = DiscoveryAction::Publish;

uint32_t gBackoffMs = kBackoffMinMs;
uint32_t gNextAttemptMs = 0;

void setState(vdm::MqttState s, int8_t rc) {
  portENTER_CRITICAL(&gMux);
  gStatus.state = s;
  gStatus.rc = rc;
  portEXIT_CRITICAL(&gMux);
}

void reloadConfig() {
  gCfgRevision = storage::configRevision();
  storage::getConfig(gCfg);
  vdm::copyString(gTopics.station, sizeof gTopics.station, gCfg.station);
  gTopics.pathAsRoot = gCfg.mqtt.pathAsRoot;
  gTopics.separate = gCfg.mqtt.separate;
  for (uint8_t i = 0; i < vdm::kValveCount; ++i) {
    vdm::buildSegment(gCfg.valves[i].name, i, gSegments[i], sizeof gSegments[i]);
  }
  vdm::PublishScheduler::Params p;
  p.onChange = gCfg.mqtt.onChange;
  p.publishIntervalMs = static_cast<uint32_t>(gCfg.mqtt.publishIntervalS) * 1000u;
  p.minDelayMs = static_cast<uint32_t>(gCfg.mqtt.minDelayS) * 1000u;
  gScheduler.configure(p);
  vdm::buildTopic(gTopics, vdm::Topic::Status, nullptr, gLwtTopic, sizeof gLwtTopic);
  // Topic, broker or credential changes take effect with a clean reconnect.
  if (gClient.connected()) gClient.disconnect();
}

bool publish(vdm::Topic t, const char* segment, const char* payload) {
  char topic[vdm::kTopicMax + 1];
  if (vdm::buildTopic(gTopics, t, segment, topic, sizeof topic) == 0) return false;
  const bool ok = gClient.publish(topic, payload, vdm::topicRetained(t, gCfg.mqtt.retained));
  if (!ok) {
    portENTER_CRITICAL(&gMux);
    ++gStatus.publishFailures;
    portEXIT_CRITICAL(&gMux);
  }
  return ok;
}

void onMessage(char* topic, uint8_t* payload, unsigned int len) {
  const int valve =
      vdm::parseTargetCommandTopic(gTopics, topic, strnlen(topic, vdm::kTopicMax + 1), gSegments);
  if (valve < 0) return;
  uint8_t pos = 0;
  const vdm::TargetPayload r =
      vdm::parseTargetPayload(reinterpret_cast<const char*>(payload), len, pos);
  if (r != vdm::TargetPayload::Ok || !gCfg.valves[valve].active) {
    portENTER_CRITICAL(&gMux);
    ++gStatus.commandsRejected;
    portEXIT_CRITICAL(&gMux);
    logger::log(vdm::EventCode::MqttCommandRejected, vdm::kNoValve, valve + 1,
                static_cast<int32_t>(r), r == vdm::TargetPayload::Ok ? "inactive" : "payload");
    return;
  }
  app::Command c;
  c.type = app::CommandType::SetTarget;
  c.valve = static_cast<uint8_t>(valve);
  c.pos = pos;
  c.source = vdm::TargetSource::Mqtt;
  if (!app::submit(c)) logger::log(vdm::EventCode::StmQueueFull, static_cast<uint8_t>(valve));
}

bool connect(uint32_t now) {
  if (gCfg.mqtt.host[0] == '\0') return false;
  vdm::copyString(gHost, sizeof gHost, gCfg.mqtt.host);
  gClient.setServer(gHost, gCfg.mqtt.port);
  gClient.setBufferSize(kBufferSize);
  gClient.setKeepAlive(gCfg.mqtt.keepAliveS);
  gClient.setSocketTimeout(kSocketTimeoutS);
  gClient.setCallback(onMessage);
  const bool auth = gCfg.mqtt.user[0] != '\0' && gCfg.mqtt.password[0] != '\0';
  setState(vdm::MqttState::Connecting, 0);
  const bool ok = gClient.connect(gCfg.station[0] ? gCfg.station : "VdMot",
                                  auth ? gCfg.mqtt.user : nullptr,
                                  auth ? gCfg.mqtt.password : nullptr, gLwtTopic, 0, true,
                                  "offline");
  if (!ok) {
    setState(vdm::MqttState::Error, static_cast<int8_t>(gClient.state()));
    return false;
  }
  gClient.publish(gLwtTopic, "online", true);
  char topic[vdm::kTopicMax + 1];
  for (uint8_t i = 0; i < vdm::kValveCount; ++i) {
    if (!gCfg.valves[i].active) continue;
    if (vdm::buildTargetCommandTopic(gTopics, gSegments[i], topic, sizeof topic) > 0) {
      gClient.subscribe(topic);
    }
  }
  if (gCfg.mqtt.mode == vdm::MqttMode::MqttHa) gClient.subscribe("homeassistant/status");
  gScheduler.onConnected(now);
  gFirstPublish = true;
  setState(vdm::MqttState::Connected, 0);
  portENTER_CRITICAL(&gMux);
  ++gStatus.reconnects;
  portEXIT_CRITICAL(&gMux);
  logger::log(vdm::EventCode::MqttConnected);
  if (gCfg.mqtt.mode == vdm::MqttMode::MqttHa && gCfg.mqtt.haDiscoveryOnConnect) {
    gDiscoveryAction = storage::haCleanupDone() ? DiscoveryAction::Publish
                                                : DiscoveryAction::DeleteAndPublish;
    gDiscoveryRequested = true;
  }
  return true;
}

// Publishing pass: legacy compat topics per PublishScheduler, new diag
// topics, rate-limited events. Bounded work per call.
void publishPass(uint32_t now) {
  app::readStmSnapshot(gSnap);
  if (gScheduler.takeFullPublish(now)) {
    char buf[40];
    if (gFirstPublish) {
      vdm::formatIpv4(net::info().ip, buf, sizeof buf);
      publish(vdm::Topic::CommonIp, nullptr, buf);
      gFirstPublish = false;
    }
    for (uint8_t i = 0; i < vdm::kValveCount; ++i) {
      if (!gCfg.valves[i].active) continue;
      const vdm::ValveState& v = gSnap.valves[i];
      if (v.desiredValid) {
        snprintf(buf, sizeof buf, "%u", static_cast<unsigned>(v.desired));
        publish(vdm::Topic::ValveTarget, gSegments[i], buf);
      }
      vdm::formatValveState(v.status, gCfg.mqtt.plainText, buf, sizeof buf);
      publish(vdm::Topic::ValveState, gSegments[i], buf);
      snprintf(buf, sizeof buf, "%u", static_cast<unsigned>(v.position));
      publish(vdm::Topic::ValveActual, gSegments[i], buf);
      gClient.loop();
    }
    gScheduler.markAllPublished(now);
  }
  if (gCfg.mqtt.events) {
    vdm::Event ev[4];
    uint32_t next = gEventCursor;
    const size_t n = logger::readSince(gEventCursor, ev, 4, next);
    gEventCursor = next;
    for (size_t i = 0; i < n; ++i) {
      if (!gEventLimiter.allow(ev[i], now)) continue;
      char json[320];
      vdm::JsonWriter jw(json, sizeof json);
      if (vdm::writeEventJson(jw, ev[i])) publish(vdm::Topic::Events, nullptr, json);
    }
  }
}

}  // namespace

void begin() { reloadConfig(); }

void task(void*) {
  esp_task_wdt_add(nullptr);
  gEventCursor = logger::lastSeq();
  for (;;) {
    esp_task_wdt_reset();
    const uint32_t now = app::nowMs();
    if (storage::configRevision() != gCfgRevision) reloadConfig();

    if (gCfg.mqtt.mode == vdm::MqttMode::Off || !net::isUp()) {
      if (gClient.connected()) gClient.disconnect();
      setState(gCfg.mqtt.mode == vdm::MqttMode::Off ? vdm::MqttState::Disabled
                                                     : vdm::MqttState::Connecting,
               0);
      vTaskDelay(pdMS_TO_TICKS(500));
      continue;
    }
    if (gReconnectRequested) {
      gReconnectRequested = false;
      gClient.disconnect();
    }
    if (!gClient.connected()) {
      if (gStatus.state == vdm::MqttState::Connected) {
        logger::log(vdm::EventCode::MqttDisconnected, vdm::kNoValve, gClient.state());
        setState(vdm::MqttState::Connecting, static_cast<int8_t>(gClient.state()));
      }
      if (vdm::timeReached(now, gNextAttemptMs)) {
        if (connect(now)) {
          gBackoffMs = kBackoffMinMs;
        } else {
          gNextAttemptMs = now + gBackoffMs;
          gBackoffMs = gBackoffMs >= kBackoffMaxMs / 2 ? kBackoffMaxMs : gBackoffMs * 2;
        }
      }
      vTaskDelay(pdMS_TO_TICKS(100));
      continue;
    }
    gClient.loop();
    publishPass(now);
    vTaskDelay(pdMS_TO_TICKS(20));
  }
}

Status status() {
  portENTER_CRITICAL(&gMux);
  const Status s = gStatus;
  portEXIT_CRITICAL(&gMux);
  return s;
}

void requestReconnect() { gReconnectRequested = true; }

void requestDiscovery(DiscoveryAction a) {
  gDiscoveryAction = a;
  gDiscoveryRequested = true;
}

}  // namespace mqtt
