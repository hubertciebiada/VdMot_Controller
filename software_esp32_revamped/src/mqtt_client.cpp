#include "mqtt_client.h"

#include <Arduino.h>
#include <LittleFS.h>
#include <PubSubClient.h>
#include <WiFiClient.h>
#include <esp_task_wdt.h>
#include <freertos/FreeRTOS.h>
#include <string.h>

#include <vdm/config.h>
#include <vdm/event_limiter.h>
#include <vdm/ha_discovery.h>
#include <vdm/json_writer.h>
#include <vdm/mqtt_topics.h>
#include <vdm/mqtt_values.h>

#include "app.h"
#include "boot_alloc.h"
#include "logger.h"
#include "net.h"
#include "ota.h"
#include "storage.h"

namespace mqtt {

namespace {

// PublishScheduler slots (DESIGN.md "MQTT topics").
constexpr uint8_t kSlotCommon = 0;
constexpr uint8_t kSlotValve0 = 1;
constexpr uint8_t kSlotTemp0 = kSlotValve0 + vdm::kValveCount;  // 13
constexpr uint8_t kSlotVolt0 = kSlotTemp0 + vdm::kTempSlotCount;  // 47
constexpr uint8_t kSlotStm = kSlotVolt0 + vdm::kVoltSlotCount;    // 55
constexpr uint8_t kSlotCount = kSlotStm + 1;
static_assert(kSlotCount <= vdm::PublishScheduler::kSlots, "scheduler slots");

constexpr uint8_t kMaxSlotsPerPass = 2;       // on-change items per loop pass
constexpr uint8_t kMaxDiagPerPass = 4;        // diag messages per loop pass
constexpr size_t kEventsPerPass = 4;
constexpr uint32_t kSensorStaleMs = 60000;
constexpr const char* kHaStatusTopic = "homeassistant/status";
constexpr const char* kLegacyDiscoveryFile = "/HADiscovery.cfg";
constexpr const char* kLegacyDiscoveryDone = "/HADiscovery.cfg.done";

// Compat fields of a valve that trigger an on-change publish.
constexpr uint32_t kValveCompatMask = vdm::kChangeStatus | vdm::kChangePosition |
                                      vdm::kChangeTarget | vdm::kChangeMeanCurrent |
                                      vdm::kChangeTemp1 | vdm::kChangeTemp2 |
                                      vdm::kChangeCounters | vdm::kChangeCalibRetries |
                                      vdm::kChangeSensors | vdm::kChangeKnown;

WiFiClient gNet;
PubSubClient gClient(gNet);

// Task-owned copies (static: too large for the stack).
vdm::Config& gCfg = bootAlloc<vdm::Config>();
vdm::Config& gNextCfg = bootAlloc<vdm::Config>();  // reloadConfig() scratch
uint32_t gCfgRevision = 0;
app::StmSnapshot& gSnap = bootAlloc<app::StmSnapshot>();
uint32_t gSnapRevision = UINT32_MAX;
vdm::TopicContext gTopics;
char gSegments[vdm::kValveCount][vdm::kSegmentMax + 1];
char gHost[vdm::kHostMax + 1];  // PubSubClient keeps the pointer
char gClientId[vdm::kStationNameMax + 1];
char gLwtTopic[vdm::kTopicMax + 1];
vdm::PublishScheduler gScheduler;
vdm::EventRateLimiter gEventLimiter;
uint32_t gEventCursor = 0;

// Last published compat values (on-change detection).
using ValveStates = ObjArray<vdm::ValveState, vdm::kValveCount>;
ValveStates& gPubValve = bootAlloc<ValveStates>();
bool gPubValveValid[vdm::kValveCount];
struct SensorPub {
  bool valid = false;  // an entry was published since connect
  bool ok = false;
  int32_t value = 0;
};
SensorPub gPubTemp[vdm::kTempSlotCount];
SensorPub gPubVolt[vdm::kVoltSlotCount];
uint8_t gPubState = 0xFF;
uint32_t gPubUptime = UINT32_MAX;
char gMessage[120] = "";        // message of the latest Warning+ event
bool gMessageChanged = true;
bool gFirstPublish = true;      // common/ip only on the first full publish per connection
bool gFullRunning = false;
uint8_t gFullCursor = 0;
uint8_t gChangeCursor = 0;

// Calibration end per valve (legacy calibration/date), observed from the
// snapshots regardless of the connection state.
bool gCalibrating[vdm::kValveCount];
bool gCalibEnded[vdm::kValveCount];
vdm::LocalTime gCalibEnd[vdm::kValveCount];
bool gCalibDateDirty[vdm::kValveCount];
bool gObservedOnce = false;

// New diag topics: last published values (valid after connect).
struct DiagPub {
  bool valid = false;
  uint32_t moveSeq = 0;
  uint32_t earlyStops = 0;
  uint32_t cmdRejected = 0;
  uint8_t calState = 0;
  uint32_t profileCrc = 0;
};
DiagPub gPubDiag[vdm::kValveCount];
struct StmPub {
  bool valid = false;
  uint8_t proto = 0;
  vdm::LinkState link = vdm::LinkState::Unknown;
  uint32_t resets = 0, rxOverflow = 0, parseErr = 0;
  bool statusValid = false;
  int8_t calibActive = -1;
} gPubStm;

// Discovery.
enum class DiscPhase : uint8_t { Idle, DropList, LegacyFile, RemoveCurrent, PublishCurrent };
vdm::DiscoveryContext& gDiscCtx = bootAlloc<vdm::DiscoveryContext>();
vdm::DiscoveryIterator gDiscIter(gDiscCtx);
vdm::DropListIterator gDropIter(gDiscCtx);
DiscPhase gDiscPhase = DiscPhase::Idle;
bool gDiscPublishAfter = false;  // RemoveCurrent is followed by PublishCurrent
DiscPhase gDiscAfterCleanup = DiscPhase::Idle;  // phase after the first-run cleanup
uint32_t gDiscTempMask = 0;  // valves with temp1/temp2 when discovery last ran
uint16_t gDiscConfigs = 0, gDiscDeletes = 0, gDiscSkipped = 0;
fs::File gLegacyFile;
using PayloadBuf = ObjArray<char, vdm::kDiscoveryPayloadMax + 1>;
PayloadBuf& gDiscPayload = bootAlloc<PayloadBuf>();  // also the profile payload

portMUX_TYPE gMux = portMUX_INITIALIZER_UNLOCKED;
Status gStatus;
volatile bool gReconnectRequested = false;
volatile bool gDiscoveryRequested = false;
volatile DiscoveryAction gDiscoveryAction = DiscoveryAction::Publish;
bool gOfflineSent = false;
bool gConnected = false;
// First-run HA cleanup waiting for the STM data (see cleanupReady()).
bool gCleanupDeferred = false;

vdm::Backoff gBackoff(kBackoffMinMs, kBackoffMaxMs);

void setState(vdm::MqttState s, int8_t rc) {
  portENTER_CRITICAL(&gMux);
  gStatus.state = s;
  gStatus.rc = rc;
  portEXIT_CRITICAL(&gMux);
}

void count(uint32_t Status::*field) {
  portENTER_CRITICAL(&gMux);
  ++(gStatus.*field);
  portEXIT_CRITICAL(&gMux);
}

void setDiscoveryRunning(bool on) {
  portENTER_CRITICAL(&gMux);
  gStatus.discoveryRunning = on;
  portEXIT_CRITICAL(&gMux);
}

// Config parts that shape topics, subscriptions or discovery. A change of
// any of them needs a clean reconnect; other config changes do not.
bool topicConfigChanged(const vdm::Config& a, const vdm::Config& b) {
  return strcmp(a.station, b.station) != 0 || memcmp(&a.mqtt, &b.mqtt, sizeof a.mqtt) != 0 ||
         memcmp(a.valves, b.valves, sizeof a.valves) != 0 ||
         memcmp(a.temps, b.temps, sizeof a.temps) != 0 ||
         memcmp(a.volts, b.volts, sizeof a.volts) != 0;
}

void reloadConfig() {
  vdm::Config& next = gNextCfg;
  gCfgRevision = storage::configRevision();
  storage::getConfig(next);
  const bool reconnect = topicConfigChanged(next, gCfg);
  gCfg = next;
  vdm::copyString(gTopics.station, sizeof gTopics.station, gCfg.station);
  gTopics.pathAsRoot = gCfg.mqtt.pathAsRoot;
  gTopics.separate = gCfg.mqtt.separate;
  for (uint8_t i = 0; i < vdm::kValveCount; ++i) {
    vdm::buildSegment(gCfg.valves[i].name, i, gSegments[i], sizeof gSegments[i]);
  }
  vdm::copyString(gClientId, sizeof gClientId, gCfg.station[0] ? gCfg.station : "VdMot");
  vdm::PublishScheduler::Params p;
  p.onChange = gCfg.mqtt.onChange;
  p.publishIntervalMs = static_cast<uint32_t>(gCfg.mqtt.publishIntervalS) * 1000u;
  p.minDelayMs = static_cast<uint32_t>(gCfg.mqtt.minDelayS) * 1000u;
  gScheduler.configure(p);
  vdm::buildTopic(gTopics, vdm::Topic::Status, nullptr, gLwtTopic, sizeof gLwtTopic);
  if (reconnect && gClient.connected()) {
    gClient.disconnect();  // clean: the broker does not send the LWT
  }
}

// PubSubClient needs 5 (fixed header) + 2 (topic length) bytes besides the
// topic and payload. Every message built here fits, so a publish only fails
// on a dead connection or a socket write that stalled.
static_assert(5 + 2 + vdm::kTopicMax + vdm::kDiscoveryPayloadMax <= kBufferSize,
              "largest MQTT message must fit the PubSubClient buffer");

bool publishRaw(const char* topic, const char* payload, bool retained) {
  if (gClient.publish(topic, payload, retained)) return true;
  count(&Status::publishFailures);
  // A failed publish on a live connection is a socket write that WiFiClient
  // already retried for up to 10 s (broker gone without FIN/RST). Every
  // further publish of this pass would stall as long, past the task
  // watchdog: drop the connection and let the reconnect back-off take over.
  if (gClient.connected()) gNet.stop();
  return false;
}

bool publish(vdm::Topic t, const char* segment, const char* payload) {
  char topic[vdm::kTopicMax + 1];
  if (vdm::buildTopic(gTopics, t, segment, topic, sizeof topic) == 0) return false;
  return publishRaw(topic, payload, vdm::topicRetained(t, gCfg.mqtt.retained));
}

bool publishUint(vdm::Topic t, const char* segment, uint32_t v) {
  char buf[12];
  snprintf(buf, sizeof buf, "%lu", static_cast<unsigned long>(v));
  return publish(t, segment, buf);
}

bool publishCounter(vdm::Topic t, const char* segment, uint32_t v) {
  char buf[12];
  vdm::formatLegacyCounter(v, buf, sizeof buf);
  return publish(t, segment, buf);
}

// ---------------------------------------------------------------- inbound

void onMessage(char* topic, uint8_t* payload, unsigned int len) {
  const size_t topicLen = strnlen(topic, vdm::kTopicMax + 1);
  if (topicLen == strlen(kHaStatusTopic) && strcmp(topic, kHaStatusTopic) == 0) {
    if (len == 6 && memcmp(payload, "online", 6) == 0 && gCfg.mqtt.mode == vdm::MqttMode::MqttHa &&
        gCfg.mqtt.haDiscoveryOnConnect) {
      requestDiscovery(DiscoveryAction::Publish);
    }
    return;
  }
  const int valve = vdm::parseTargetCommandTopic(gTopics, topic, topicLen, gSegments);
  if (valve < 0) return;
  uint8_t pos = 0;
  const vdm::TargetPayload r =
      vdm::parseTargetPayload(reinterpret_cast<const char*>(payload), len, pos);
  const char* reason = nullptr;
  if (r != vdm::TargetPayload::Ok) {
    reason = "payload";
  } else if (!gCfg.valves[valve].active) {
    reason = "inactive";
  } else {
    app::Command c;
    c.type = app::CommandType::SetTarget;
    c.valve = static_cast<uint8_t>(valve);
    c.pos = pos;
    c.source = vdm::TargetSource::Mqtt;
    if (app::submit(c)) return;
    reason = "queue full";
  }
  count(&Status::commandsRejected);
  logger::log(vdm::EventCode::MqttCommandRejected, vdm::kNoValve, valve + 1,
              static_cast<int32_t>(r), reason);
}

// ---------------------------------------------------------------- snapshot

// Reads a new snapshot when there is one and tracks calibration ends
// (needed for calibration/date even while disconnected).
void observe() {
  const uint32_t rev = app::stmSnapshotRevision();
  if (rev == gSnapRevision) return;
  gSnapRevision = rev;
  app::readStmSnapshot(gSnap);
  for (uint8_t i = 0; i < vdm::kValveCount; ++i) {
    const bool cal = gSnap.valves[i].calibrating;
    if (gObservedOnce && gCalibrating[i] && !cal) {
      gCalibEnd[i] = net::localTime();
      gCalibEnded[i] = true;
      gCalibDateDirty[i] = true;
    }
    gCalibrating[i] = cal;
  }
  gObservedOnce = true;
}

uint16_t activeMask() {
  uint16_t m = 0;
  for (uint8_t i = 0; i < vdm::kValveCount; ++i) {
    if (gCfg.valves[i].active) m |= static_cast<uint16_t>(1u << i);
  }
  return m;
}

uint8_t currentSystemState() {
  return vdm::systemState(gSnap.link, gSnap.valves, vdm::kValveCount, activeMask());
}

// Temperature of a config slot (1-based) with its offset; false = "failed".
bool slotTemp(uint8_t slot1, int32_t& tenths) {
  if (slot1 == 0 || slot1 > vdm::kTempSlotCount) return false;
  const vdm::TempSlotConfig& s = gCfg.temps[slot1 - 1];
  if (vdm::isZero(s.id)) return false;
  for (uint8_t b = 0; b < vdm::kTempSlotCount; ++b) {
    const vdm::TempReading& r = gSnap.temps[b];
    if (!r.seen || r.id != s.id) continue;
    if (!vdm::tempRawValid(r.raw) || vdm::elapsedMs(app::nowMs(), r.lastSeenMs) > kSensorStaleMs) {
      return false;
    }
    tenths = static_cast<int32_t>(r.raw) + s.offset;
    return true;
  }
  return false;
}

// Volt value of a config slot (0-based) in the configured unit.
bool slotVolt(uint8_t i, double& value) {
  const vdm::VoltSlotConfig& s = gCfg.volts[i];
  if (vdm::isZero(s.id)) return false;
  for (uint8_t b = 0; b < vdm::kVoltSlotCount; ++b) {
    const vdm::VoltReading& r = gSnap.volts[b];
    if (!r.seen || r.id != s.id) continue;
    if (!vdm::vadValid(r.vad) || vdm::elapsedMs(app::nowMs(), r.lastSeenMs) > kSensorStaleMs) {
      return false;
    }
    value = (static_cast<double>(r.vad) / 100.0 + s.offset) * s.factor;
    return true;
  }
  return false;
}

bool tempAssignedToValve(uint8_t slot1) {
  for (uint8_t v = 0; v < vdm::kValveCount; ++v) {
    const vdm::ValveState& s = gSnap.valves[v];
    if (s.sensorSlot[0] == slot1 || s.sensorSlot[1] == slot1) return true;
  }
  return false;
}

bool tempPublished(uint8_t i) {
  const vdm::TempSlotConfig& s = gCfg.temps[i];
  return s.active && !vdm::isZero(s.id) &&
         (gCfg.mqtt.allTemps || !tempAssignedToValve(static_cast<uint8_t>(i + 1)));
}

bool voltPublished(uint8_t i) { return gCfg.volts[i].active && !vdm::isZero(gCfg.volts[i].id); }

// ---------------------------------------------------------------- compat slots

void publishCommon(bool full) {
  char buf[vdm::kTopicMax + 1];
  if (full && gFirstPublish) {
    vdm::formatIpv4(net::info().ip, buf, sizeof buf);
    if (publish(vdm::Topic::CommonIp, nullptr, buf)) gFirstPublish = false;
  }
  const uint8_t st = currentSystemState();
  vdm::formatSystemState(st, gCfg.mqtt.plainText, buf, sizeof buf);
  if (publish(vdm::Topic::CommonState, nullptr, buf)) gPubState = st;
  if (gCfg.mqtt.upTime) {
    const uint32_t up = app::uptimeS();
    vdm::formatUptime(up, buf, sizeof buf);
    if (publish(vdm::Topic::CommonUptime, nullptr, buf)) gPubUptime = up;
  }
  if (publish(vdm::Topic::CommonMessage, nullptr, gMessage)) gMessageChanged = false;
}

void publishValveTemp(vdm::Topic t, const char* seg, int16_t raw, uint8_t slot1) {
  if (raw == vdm::kTempUnassigned) return;
  int32_t tenths = raw;
  if (slot1 >= 1 && slot1 <= vdm::kTempSlotCount) tenths += gCfg.temps[slot1 - 1].offset;
  char buf[16];
  vdm::formatTemp(tenths, vdm::tempRawValid(raw), gCfg.mqtt.germanDecimal, buf, sizeof buf);
  publish(t, seg, buf);
}

void publishValve(uint8_t i) {
  const vdm::ValveState& v = gSnap.valves[i];
  const char* seg = gSegments[i];
  gPubValve[i] = v;
  gPubValveValid[i] = true;
  if (!gCfg.valves[i].active || !v.known) return;
  char buf[48];
  if (v.desiredValid) publishUint(vdm::Topic::ValveTarget, seg, v.desired);
  vdm::formatValveState(v.status, gCfg.mqtt.plainText, buf, sizeof buf);
  publish(vdm::Topic::ValveState, seg, buf);
  publishUint(vdm::Topic::ValveActual, seg, v.position);
  if (gCalibEnded[i]) {
    vdm::formatCalibDate(gCalibEnd[i], buf, sizeof buf);
    if (publish(vdm::Topic::ValveCalibDate, seg, buf)) gCalibDateDirty[i] = false;
  }
  publishUint(vdm::Topic::ValveCalibRepetitions, seg, v.calibRetries);
  if (gCfg.mqtt.diag) {
    publishUint(vdm::Topic::ValveMeanCurrent, seg, v.meanCurrent);
    publishCounter(vdm::Topic::ValveOpenCount, seg, v.openCount);
    publishCounter(vdm::Topic::ValveCloseCount, seg, v.closeCount);
    snprintf(buf, sizeof buf, "%ld", static_cast<long>(v.deadZone));
    publish(vdm::Topic::ValveDeadZoneCount, seg, buf);
    publishCounter(vdm::Topic::ValveMoves, seg, v.moves);
  }
  publishValveTemp(vdm::Topic::ValveTemp1, seg, v.temp1, v.sensorSlot[0]);
  publishValveTemp(vdm::Topic::ValveTemp2, seg, v.temp2, v.sensorSlot[1]);
}

void publishTemp(uint8_t i) {
  SensorPub& p = gPubTemp[i];
  int32_t tenths = 0;
  p.ok = slotTemp(static_cast<uint8_t>(i + 1), tenths);
  p.value = tenths;
  p.valid = true;
  if (!tempPublished(i)) return;
  char seg[vdm::kSegmentMax + 1];
  char buf[vdm::kOneWireIdTextLen + 1];
  vdm::buildSegment(gCfg.temps[i].name, i, seg, sizeof seg);
  vdm::formatOneWireId(gCfg.temps[i].id, buf, sizeof buf);
  publish(vdm::Topic::TempId, seg, buf);
  vdm::formatTemp(tenths, p.ok, gCfg.mqtt.germanDecimal, buf, sizeof buf);
  publish(vdm::Topic::TempValue, seg, buf);
}

void publishVolt(uint8_t i) {
  SensorPub& p = gPubVolt[i];
  double value = 0;
  p.ok = slotVolt(i, value);
  p.value = p.ok ? static_cast<int32_t>(value * 1000.0) : 0;
  p.valid = true;
  if (!voltPublished(i)) return;
  char seg[vdm::kSegmentMax + 1];
  char buf[vdm::kOneWireIdTextLen + 1];
  vdm::buildSegment(gCfg.volts[i].name, i, seg, sizeof seg);
  vdm::formatOneWireId(gCfg.volts[i].id, buf, sizeof buf);
  publish(vdm::Topic::VoltId, seg, buf);
  vdm::formatVolt(value, p.ok, gCfg.mqtt.germanDecimal, buf, sizeof buf);
  publish(vdm::Topic::VoltValue, seg, buf);
  publish(vdm::Topic::VoltUnit, seg, gCfg.volts[i].unit);
}

void publishStmFull() {
  if (!gCfg.mqtt.newDiag || gSnap.proto < 2 || !gSnap.haveStatus) return;
  publishUint(vdm::Topic::DiagStmUptime, nullptr, gSnap.status.uptimeS);
}

void publishSlot(uint8_t slot, bool full) {
  if (slot == kSlotCommon) {
    publishCommon(full);
  } else if (slot < kSlotTemp0) {
    publishValve(slot - kSlotValve0);
  } else if (slot < kSlotVolt0) {
    publishTemp(slot - kSlotTemp0);
  } else if (slot < kSlotStm) {
    publishVolt(slot - kSlotVolt0);
  } else {
    publishStmFull();
  }
}

bool slotChanged(uint8_t slot) {
  if (slot == kSlotCommon) {
    // The uptime text changes every second; minDelayS spaces it further.
    return (gCfg.mqtt.upTime && app::uptimeS() != gPubUptime) || gMessageChanged ||
           currentSystemState() != gPubState;
  }
  if (slot < kSlotTemp0) {
    const uint8_t i = slot - kSlotValve0;
    return !gPubValveValid[i] || gCalibDateDirty[i] ||
           (vdm::diffValve(gPubValve[i], gSnap.valves[i]) & kValveCompatMask) != 0;
  }
  if (slot < kSlotVolt0) {
    const uint8_t i = slot - kSlotTemp0;
    int32_t tenths = 0;
    const bool ok = slotTemp(static_cast<uint8_t>(i + 1), tenths);
    const SensorPub& p = gPubTemp[i];
    return !p.valid || p.ok != ok || (ok && p.value != tenths);
  }
  if (slot < kSlotStm) {
    const uint8_t i = slot - kSlotVolt0;
    double value = 0;
    const bool ok = slotVolt(i, value);
    const SensorPub& p = gPubVolt[i];
    return !p.valid || p.ok != ok || (ok && p.value != static_cast<int32_t>(value * 1000.0));
  }
  return false;  // STM uptime goes out with the full publish only
}

// Full publish, spread over passes: one valve (or a few sensors) per pass
// with loop() in between, so inbound commands are never starved.
void serviceFullPublish(uint32_t now) {
  if (gScheduler.takeFullPublish(now)) {
    gFullRunning = true;
    gFullCursor = 0;
  }
  if (!gFullRunning) return;
  uint8_t budget = 4;
  while (budget > 0 && gFullCursor < kSlotCount) {
    const uint8_t slot = gFullCursor++;
    publishSlot(slot, true);
    budget = (slot >= kSlotValve0 && slot < kSlotTemp0) ? 0 : budget - 1;
  }
  if (gFullCursor >= kSlotCount) {
    gFullRunning = false;
    gScheduler.markAllPublished(now);
  }
}

void serviceOnChange(uint32_t now) {
  if (!gCfg.mqtt.onChange || gFullRunning) return;
  uint8_t sent = 0;
  for (uint8_t n = 0; n < kSlotCount && sent < kMaxSlotsPerPass; ++n) {
    const uint8_t slot = gChangeCursor;
    gChangeCursor = static_cast<uint8_t>((gChangeCursor + 1) % kSlotCount);
    if (gScheduler.takeItem(slot, slotChanged(slot), now)) {
      publishSlot(slot, false);
      ++sent;
    }
  }
}

// ---------------------------------------------------------------- new diag

void publishLastMove(uint8_t i, const vdm::ValveState& v) {
  char json[160];
  vdm::JsonWriter jw(json, sizeof json);
  jw.beginObject();
  jw.kv("dir", v.lastMove.dir == vdm::MoveDir::Open ? "open" : "close");
  jw.kv("req", v.lastMove.requestedCounts);
  jw.kv("cnt", v.lastMove.countedCounts);
  jw.kv("stop", vdm::stopReasonName(v.lastMove.stop));
  jw.kv("peak", static_cast<uint32_t>(v.lastMove.peakCurrent));
  jw.kv("ms", v.lastMove.durationMs);
  jw.endObject();
  if (jw.complete()) publish(vdm::Topic::DiagValveLastMove, gSegments[i], json);
}

uint32_t profileCrc(const vdm::Profile& p) {
  return vdm::crc32(reinterpret_cast<const uint8_t*>(&p), sizeof p);
}

// Publishes changed diag values; bounded messages per pass.
void serviceDiag() {
  if (!gCfg.mqtt.newDiag) return;
  uint8_t budget = kMaxDiagPerPass;
  // STM level.
  StmPub& s = gPubStm;
  if (!s.valid || s.proto != gSnap.proto) {
    if (gSnap.proto != 0 && publishUint(vdm::Topic::DiagStmProto, nullptr, gSnap.proto)) {
      s.proto = gSnap.proto;
    }
    --budget;
  }
  if (!s.valid || s.link != gSnap.link) {
    if (publish(vdm::Topic::DiagStmLink, nullptr, vdm::linkStateName(gSnap.link))) {
      s.link = gSnap.link;
    }
    --budget;
  }
  s.valid = true;
  if (gSnap.proto >= 2 && gSnap.haveStatus) {
    const vdm::StmStatus& st = gSnap.status;
    if (!s.statusValid || s.resets != st.resets)
      publishUint(vdm::Topic::DiagStmResets, nullptr, st.resets);
    if (!s.statusValid || s.rxOverflow != st.rxOverflow) {
      publishUint(vdm::Topic::DiagStmRxOverflow, nullptr, st.rxOverflow);
    }
    if (!s.statusValid || s.parseErr != st.parseErrors) {
      publishUint(vdm::Topic::DiagStmParseErr, nullptr, st.parseErrors);
    }
    s.resets = st.resets;
    s.rxOverflow = st.rxOverflow;
    s.parseErr = st.parseErrors;
    s.statusValid = true;
  }
  bool calibActive = false;
  for (const vdm::ValveState& v : gSnap.valves) calibActive = calibActive || v.calibrating;
  if (s.calibActive != static_cast<int8_t>(calibActive)) {
    if (publish(vdm::Topic::DiagCalibrationActive, nullptr, calibActive ? "1" : "0")) {
      s.calibActive = static_cast<int8_t>(calibActive);
    }
  }
  // Per valve (v2 only).
  for (uint8_t i = 0; i < vdm::kValveCount && budget > 0; ++i) {
    const vdm::ValveState& v = gSnap.valves[i];
    if (!gCfg.valves[i].active || !v.hasExtended) continue;
    DiagPub& d = gPubDiag[i];
    const char* seg = gSegments[i];
    if (!d.valid || d.moveSeq != v.moveSeq) {
      if (v.moveSeq != 0) publishLastMove(i, v);
      d.moveSeq = v.moveSeq;
      --budget;
    }
    if (!d.valid || d.earlyStops != v.earlyStops) {
      publishUint(vdm::Topic::DiagValveEarlyStops, seg, v.earlyStops);
      d.earlyStops = v.earlyStops;
    }
    if (!d.valid || d.cmdRejected != v.cmdRejected) {
      publishUint(vdm::Topic::DiagValveCmdRejected, seg, v.cmdRejected);
      d.cmdRejected = v.cmdRejected;
    }
    if (!d.valid || d.calState != v.calState) {
      publishUint(vdm::Topic::DiagValveCalState, seg, v.calState);
      d.calState = v.calState;
    }
    const vdm::Profile& p = gSnap.profiles[i];
    const uint32_t crc = p.count > 0 ? profileCrc(p) : 0;
    if (crc != d.profileCrc && budget > 0) {
      // Not retained: only new profiles go out (not the one known at connect).
      if (d.valid && p.count > 0) {
        // Shares the discovery buffer: both run in this task, one at a time.
        vdm::JsonWriter jw(gDiscPayload.data(), sizeof gDiscPayload.items);
        if (vdm::writeProfileJson(jw, p) && jw.complete()) {
          publish(vdm::Topic::DiagValveProfile, seg, jw.c_str());
        }
        --budget;
      }
      d.profileCrc = crc;
    }
    d.valid = true;
    gClient.loop();
  }
}

// ---------------------------------------------------------------- events

void serviceEvents(uint32_t now) {
  vdm::Event ev[kEventsPerPass];
  uint32_t next = gEventCursor;
  const size_t n = logger::readSince(gEventCursor, ev, kEventsPerPass, next);
  gEventCursor = next;
  for (size_t i = 0; i < n; ++i) {
    if (ev[i].severity >= vdm::Severity::Warning) {
      vdm::formatEventMessage(ev[i], gMessage, sizeof gMessage);
      gMessageChanged = true;
    }
    if (!gConnected || !gCfg.mqtt.events) continue;
    if (!gEventLimiter.allow(ev[i], now)) continue;
    char json[320];
    vdm::JsonWriter jw(json, sizeof json);
    if (vdm::writeEventJson(jw, ev[i]) && jw.complete()) {
      publish(vdm::Topic::Events, nullptr, json);
    }
  }
  portENTER_CRITICAL(&gMux);
  gStatus.eventsSuppressed = gEventLimiter.suppressed();
  portEXIT_CRITICAL(&gMux);
}

// ---------------------------------------------------------------- discovery

// Bit 2i / 2i+1: valve i reports sensor 1 / 2 (temp entities exist).
uint32_t valveTempMask() {
  uint32_t m = 0;
  for (uint8_t i = 0; i < vdm::kValveCount; ++i) {
    if (gSnap.valves[i].temp1 != vdm::kTempUnassigned) m |= 1u << (2 * i);
    if (gSnap.valves[i].temp2 != vdm::kTempUnassigned) m |= 1u << (2 * i + 1);
  }
  return m;
}

void buildDiscoveryContext() {
  vdm::DiscoveryContext& c = gDiscCtx;
  c = vdm::DiscoveryContext{};
  c.topics = gTopics;
  c.plainText = gCfg.mqtt.plainText;
  c.publishDiag = gCfg.mqtt.diag;
  c.publishUptime = gCfg.mqtt.upTime;
  c.publishAllTemps = gCfg.mqtt.allTemps;
  c.newDiag = gCfg.mqtt.newDiag;
  vdm::formatIpv4(net::info().ip, c.ip, sizeof c.ip);
  vdm::copyString(c.swVersion, sizeof c.swVersion, vdm::firmwareVersion());
  gDiscTempMask = valveTempMask();
  for (uint8_t i = 0; i < vdm::kValveCount; ++i) {
    c.valves[i].active = gCfg.valves[i].active;
    vdm::copyString(c.valves[i].segment, sizeof c.valves[i].segment, gSegments[i]);
    c.valves[i].hasTemp1 = gSnap.valves[i].temp1 != vdm::kTempUnassigned;
    c.valves[i].hasTemp2 = gSnap.valves[i].temp2 != vdm::kTempUnassigned;
    c.valves[i].tempsKnown = gSnap.sensorsSettled && gSnap.valves[i].known;
  }
  for (uint8_t i = 0; i < vdm::kTempSlotCount; ++i) {
    const vdm::TempSlotConfig& s = gCfg.temps[i];
    vdm::DiscoveryContext::Sensor& d = c.temps[i];
    d.active = s.active && !vdm::isZero(s.id);
    d.published = tempPublished(i);
    vdm::buildSegment(s.name, i, d.segment, sizeof d.segment);
    vdm::copyString(d.name, sizeof d.name, s.name);
    if (!vdm::isZero(s.id)) vdm::formatOneWireId(s.id, d.id, sizeof d.id);
  }
  for (uint8_t i = 0; i < vdm::kVoltSlotCount; ++i) {
    const vdm::VoltSlotConfig& s = gCfg.volts[i];
    vdm::DiscoveryContext::Sensor& d = c.volts[i];
    d.active = voltPublished(i);
    d.published = d.active;
    vdm::buildSegment(s.name, i, d.segment, sizeof d.segment);
    vdm::copyString(d.name, sizeof d.name, s.name);
    if (!vdm::isZero(s.id)) vdm::formatOneWireId(s.id, d.id, sizeof d.id);
    vdm::copyString(d.unit, sizeof d.unit, s.unit);
  }
}

// The first-run cleanup judges every legacy config against the current
// entity set, and the valve temp entities depend on STM data that arrives
// seconds after MQTT connects. It waits for the STM re-sync to settle, at
// most kCleanupMaxWaitS after boot; valves whose sensors are still unknown
// then keep their temp configs (ha_discovery tempsKnown).
constexpr uint32_t kCleanupMaxWaitS = 600;

bool cleanupReady() { return gSnap.sensorsSettled || app::uptimeS() >= kCleanupMaxWaitS; }

// `cleanupOnly`: only the first-run deletion of legacy DROP/stale entities
// (haDiscoveryOnConnect off, or MQTT without HA: the legacy firmware let
// both modes send discovery, R2 wants the DROP configs gone once); the
// current entities are left alone.
void startDiscovery(DiscoveryAction a, bool cleanupOnly = false) {
  if (gCfg.mqtt.mode == vdm::MqttMode::Off) return;
  if (gCfg.mqtt.mode != vdm::MqttMode::MqttHa && !cleanupOnly) return;
  bool cleanup = !storage::haCleanupDone();
  if (cleanup && !cleanupReady()) {
    gCleanupDeferred = true;
    cleanup = false;
  }
  if (cleanupOnly && !cleanup) return;
  if (gLegacyFile) gLegacyFile.close();
  buildDiscoveryContext();
  gDiscIter.restart();
  gDropIter.restart();
  gDiscConfigs = gDiscDeletes = gDiscSkipped = 0;
  gDiscPublishAfter = a == DiscoveryAction::DeleteAndPublish;
  const DiscPhase first = cleanupOnly                          ? DiscPhase::Idle
                          : a == DiscoveryAction::Publish      ? DiscPhase::PublishCurrent
                                                               : DiscPhase::RemoveCurrent;
  gDiscAfterCleanup = first;
  gDiscPhase = cleanup ? DiscPhase::DropList : first;
  setDiscoveryRunning(true);
}

void finishDiscovery() {
  if (gLegacyFile) gLegacyFile.close();
  gDiscPhase = DiscPhase::Idle;
  setDiscoveryRunning(false);
  logger::log(vdm::EventCode::HaDiscoverySent, vdm::kNoValve, gDiscConfigs, gDiscDeletes,
              gDiscSkipped ? "skipped entities" : nullptr);
}

bool deleteTopic(const char* topic) {
  const bool ok = publishRaw(topic, "", true);
  if (ok) ++gDiscDeletes;
  return ok;
}

// Reads the next line of the legacy discovery list (bounded); false at EOF.
bool nextLegacyLine(char* out, size_t cap) {
  size_t n = 0;
  bool any = false;
  bool overlong = false;
  while (gLegacyFile.available() > 0) {
    const int c = gLegacyFile.read();
    if (c < 0) break;
    any = true;
    if (c == '\n' || c == '\r') {
      if (n == 0 && !overlong) continue;  // empty line / CRLF
      break;
    }
    if (n + 1 < cap) {
      out[n++] = static_cast<char>(c);
    } else {
      overlong = true;
    }
  }
  out[n] = '\0';
  if (overlong) out[0] = '\0';  // never publish a truncated topic
  return any;
}

bool legacyLineUsable(const char* t) {
  if (strncmp(t, "homeassistant/", 14) != 0) return false;
  for (const char* p = t; *p; ++p) {
    if (*p == '+' || *p == '#' || static_cast<unsigned char>(*p) < 0x20 || *p == 0x7F) return false;
  }
  return true;
}

// One discovery message per call.
void serviceDiscovery() {
  switch (gDiscPhase) {
    case DiscPhase::Idle:
      return;
    case DiscPhase::DropList: {
      vdm::DiscoveryMessage msg;
      if (gDropIter.next(msg)) {
        deleteTopic(msg.topic);
        return;
      }
      gDiscPhase = DiscPhase::LegacyFile;
      gLegacyFile =
          storage::fsReady() ? LittleFS.open(kLegacyDiscoveryFile, FILE_READ) : fs::File();
      return;
    }
    case DiscPhase::LegacyFile: {
      char line[vdm::kDiscoveryTopicMax + 1];
      if (gLegacyFile && nextLegacyLine(line, sizeof line)) {
        const size_t len = strlen(line);
        if (len > 0 && legacyLineUsable(line) &&
            !vdm::discoveryTopicIsCurrent(gDiscCtx, line, len)) {
          deleteTopic(line);
        }
        return;
      }
      if (gLegacyFile) {
        gLegacyFile.close();
        LittleFS.remove(kLegacyDiscoveryDone);
        LittleFS.rename(kLegacyDiscoveryFile, kLegacyDiscoveryDone);
      }
      storage::setHaCleanupDone();
      if (gDiscAfterCleanup == DiscPhase::Idle) {
        finishDiscovery();
      } else {
        gDiscPhase = gDiscAfterCleanup;
      }
      return;
    }
    case DiscPhase::RemoveCurrent:
    case DiscPhase::PublishCurrent: {
      vdm::DiscoveryMessage msg;
      vdm::JsonWriter jw(gDiscPayload.data(), sizeof gDiscPayload.items);
      if (gDiscIter.next(msg, jw)) {
        if (gDiscPhase == DiscPhase::RemoveCurrent) {
          deleteTopic(msg.topic);
        } else if (publishRaw(msg.topic, jw.c_str(), true)) {
          ++gDiscConfigs;
        }
        return;
      }
      if (!jw.ok()) {  // entity did not fit: skipped, iteration continues
        ++gDiscSkipped;
        return;
      }
      if (gDiscPhase == DiscPhase::RemoveCurrent && gDiscPublishAfter) {
        gDiscIter.restart();
        gDiscPhase = DiscPhase::PublishCurrent;
        return;
      }
      finishDiscovery();
      return;
    }
  }
}

// ---------------------------------------------------------------- connection

void resetPublishedState() {
  for (bool& b : gPubValveValid) b = false;
  for (SensorPub& p : gPubTemp) p = SensorPub{};
  for (SensorPub& p : gPubVolt) p = SensorPub{};
  for (DiagPub& d : gPubDiag) d = DiagPub{};
  gPubStm = StmPub{};
  gPubState = 0xFF;
  gPubUptime = UINT32_MAX;
  gMessageChanged = true;
  gFirstPublish = true;
  gFullRunning = false;
  gFullCursor = 0;
}

bool subscribeValve(uint8_t i) {
  char topic[vdm::kTopicMax + 1];
  const size_t n = vdm::buildTargetCommandTopic(gTopics, gSegments[i], topic, sizeof topic);
  if (n == 0 || !gClient.subscribe(topic)) return false;
  // Also "<cmd>/set" (ioBroker "/set/set" when separate, "/set" otherwise).
  if (n + 4 > vdm::kTopicMax) return true;
  memcpy(topic + n, "/set", 5);
  return gClient.subscribe(topic);
}

bool connect(uint32_t now) {
  if (gCfg.mqtt.host[0] == '\0' || gLwtTopic[0] == '\0') {
    setState(vdm::MqttState::Error, 0);
    return false;
  }
  vdm::copyString(gHost, sizeof gHost, gCfg.mqtt.host);
  gClient.setServer(gHost, gCfg.mqtt.port);
  gClient.setKeepAlive(gCfg.mqtt.keepAliveS);
  gClient.setSocketTimeout(kSocketTimeoutS);
  gClient.setCallback(onMessage);
  const bool auth = gCfg.mqtt.user[0] != '\0' && gCfg.mqtt.password[0] != '\0';
  setState(vdm::MqttState::Connecting, 0);
  esp_task_wdt_reset();  // connect may take up to 3 s TCP + 5 s CONNACK
  const bool ok = gClient.connect(gClientId, auth ? gCfg.mqtt.user : nullptr,
                                  auth ? gCfg.mqtt.password : nullptr, gLwtTopic, 0, true,
                                  "offline");
  esp_task_wdt_reset();
  if (!ok) {
    setState(vdm::MqttState::Error, static_cast<int8_t>(gClient.state()));
    return false;
  }
  resetPublishedState();
  publishRaw(gLwtTopic, "online", true);
  for (uint8_t i = 0; i < vdm::kValveCount; ++i) {
    if (gCfg.valves[i].active) subscribeValve(i);
  }
  if (gCfg.mqtt.mode == vdm::MqttMode::MqttHa) gClient.subscribe(kHaStatusTopic);
  gScheduler.onConnected(now);
  gConnected = true;
  gOfflineSent = false;
  setState(vdm::MqttState::Connected, 0);
  count(&Status::reconnects);
  logger::log(vdm::EventCode::MqttConnected);
  startDiscovery(DiscoveryAction::Publish,
                 gCfg.mqtt.mode != vdm::MqttMode::MqttHa || !gCfg.mqtt.haDiscoveryOnConnect);
  return true;
}

void onDisconnected() {
  if (!gConnected) return;
  gConnected = false;
  if (gDiscPhase != DiscPhase::Idle) {
    if (gLegacyFile) gLegacyFile.close();
    gDiscPhase = DiscPhase::Idle;
    setDiscoveryRunning(false);
  }
  logger::log(vdm::EventCode::MqttDisconnected, vdm::kNoValve, gClient.state());
  setState(vdm::MqttState::Connecting, static_cast<int8_t>(gClient.state()));
}

void disconnectClean() {
  if (gClient.connected()) {
    publishRaw(gLwtTopic, "offline", true);
    gClient.disconnect();
  }
  onDisconnected();
}

}  // namespace

void begin() {
  gClient.setBufferSize(kBufferSize);  // allocated once, never resized
  reloadConfig();
}

void task(void*) {
  esp_task_wdt_add(nullptr);
  gEventCursor = 0;  // events since boot feed common/message and <main>events
  for (;;) {
    esp_task_wdt_reset();
    const uint32_t now = app::nowMs();
    if (storage::configRevision() != gCfgRevision) reloadConfig();
    observe();

    if (ota::restartPending()) {
      if (!gOfflineSent) {
        gOfflineSent = true;
        disconnectClean();
        setState(vdm::MqttState::Disabled, 0);
      }
      vTaskDelay(pdMS_TO_TICKS(100));
      continue;
    }
    if (gCfg.mqtt.mode == vdm::MqttMode::Off || !net::isUp()) {
      disconnectClean();
      setState(gCfg.mqtt.mode == vdm::MqttMode::Off ? vdm::MqttState::Disabled
                                                     : vdm::MqttState::Connecting,
               0);
      serviceEvents(now);  // keep the message tracking and cursor current
      vTaskDelay(pdMS_TO_TICKS(500));
      continue;
    }
    if (gReconnectRequested) {
      gReconnectRequested = false;
      disconnectClean();
      gBackoff.reset();
    }
    if (!gClient.connected()) {
      onDisconnected();
      if (gBackoff.due(now)) {
        if (connect(now)) {
          gBackoff.reset();
        } else {
          gBackoff.onFailure(app::nowMs());
        }
      }
      serviceEvents(now);
      vTaskDelay(pdMS_TO_TICKS(100));
      continue;
    }
    if (gDiscoveryRequested) {
      gDiscoveryRequested = false;
      startDiscovery(gDiscoveryAction);
    } else if (gDiscPhase == DiscPhase::Idle && gCleanupDeferred && cleanupReady()) {
      gCleanupDeferred = false;
      startDiscovery(DiscoveryAction::Publish, true);
    } else if (gDiscPhase == DiscPhase::Idle && gCfg.mqtt.mode == vdm::MqttMode::MqttHa &&
               gCfg.mqtt.haDiscoveryOnConnect && valveTempMask() != gDiscTempMask) {
      // A valve sensor appeared/disappeared after the last run (the STM
      // reports assignments only after its first 1-Wire read).
      startDiscovery(DiscoveryAction::Publish);
    }
    gClient.loop();
    serviceEvents(now);
    serviceFullPublish(now);
    gClient.loop();
    serviceOnChange(now);
    serviceDiag();
    serviceDiscovery();
    vTaskDelay(pdMS_TO_TICKS(kDiscoveryPaceMs));
  }
}

Status status() {
  portENTER_CRITICAL(&gMux);
  const Status s = gStatus;
  portEXIT_CRITICAL(&gMux);
  return s;
}

// No regulator watch yet: mode Off reads as a live regulator, so no
// failsafe starts because of it.
vdm::RegulatorInput regulatorState() { return vdm::RegulatorInput{}; }

bool calibrationEnd(uint8_t, vdm::LocalTime&) { return false; }

void requestReconnect() { gReconnectRequested = true; }

void requestDiscovery(DiscoveryAction a) {
  gDiscoveryAction = a;
  gDiscoveryRequested = true;
}

}  // namespace mqtt
