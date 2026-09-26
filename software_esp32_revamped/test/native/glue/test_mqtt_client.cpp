// src/mqtt_client.cpp: session, LWT, subscriptions, inbound commands and their
// retained leftovers, the regulator state, published values, events and the
// discovery run with its list file.
#include <vdm/ha_discovery.h>
#include <vdm/mqtt_topics.h>
#include <vdm/version.h>

#include <string>
#include <vector>

#include "glue_test.h"
#include "mqtt_client.h"

namespace {

vdm::Config& useMqtt(vdm::MqttMode mode = vdm::MqttMode::Mqtt) {
  vdm::Config& c = sib::storage().active;
  c.mqtt.mode = mode;
  vdm::copyString(c.mqtt.host, sizeof c.mqtt.host, "broker.lan");
  c.mqtt.port = 1884;
  c.mqtt.keepAliveS = 30;
  vdm::copyString(c.station, sizeof c.station, "VdMot");
  c.valves[0].active = true;
  ++sib::storage().revision;
  sib::storage().haCleanupDone = true;  // no first-run cleanup unless a test wants it
  sib::storage().haLayout = 2;
  sib::net().up = true;
  fakes::fs().mounted = true;
  return c;
}

vdm::TopicContext topics(const vdm::Config& c) {
  vdm::TopicContext t;
  vdm::copyString(t.station, sizeof t.station, vdm::mqttRootTopic(c));
  t.pathAsRoot = c.mqtt.pathAsRoot;
  t.separate = c.mqtt.separate;
  return t;
}

std::string topicOf(const vdm::Config& c, vdm::Topic t, const char* segment = nullptr) {
  char buf[vdm::kTopicMax + 1];
  vdm::buildTopic(topics(c), t, segment, buf, sizeof buf);
  return buf;
}

// Runs the MQTT task for `passes` loop passes.
void runTask(long passes) {
  fakes::rtos().stopAfterYields(passes);
  CHECK_THROWS_AS(mqtt::task(nullptr), fakes::YieldLimit);
}

// Connected and the first full publish done.
void settle(long passes = 40) {
  mqtt::begin();
  runTask(passes);
  REQUIRE(fakes::mqtt().connected);
}

void deliver(const std::string& topic, const std::string& payload) {
  fakes::mqtt().inbox.push_back({topic, payload, false});
}

std::vector<std::string> payloads(const std::string& topic) {
  std::vector<std::string> v;
  for (const fakes::MqttMessage& m : fakes::mqtt().publishedTo(topic)) v.push_back(m.payload);
  return v;
}

std::string last(const std::string& topic) {
  const std::vector<std::string> v = payloads(topic);
  return v.empty() ? "<none>" : v.back();
}

// A new STM snapshot for the task.
vdm::StmSnapshot& snap() { return sib::app().snapshot; }
void publishSnap() { ++sib::app().snapshotRevision; }

vdm::StmSnapshot& linkUp() {
  vdm::StmSnapshot& s = snap();
  s.link = vdm::LinkState::Up;
  s.proto = 2;
  s.valves[0].known = true;
  publishSnap();
  return s;
}

std::vector<vdm::Event> rejected() { return sib::logger().withCode(vdm::EventCode::MqttCommandRejected); }

}  // namespace

TEST_CASE("mqtt begin: the PubSubClient buffer is 2304 bytes") {
  glue::begin();
  mqtt::begin();
  CHECK(fakes::mqtt().bufferSize == 2304);
  CHECK(fakes::mqtt().bufferSizeCalls == 1);
  CHECK(mqtt::status().haStatus == vdm::HaStatus::Unknown);  // power-on RTC contents
  CHECK(mqtt::regulatorState().ha == vdm::HaStatus::Unknown);
}

TEST_CASE("mqtt task: MQTT off stays disabled, never connects, the regulator reads Off") {
  glue::begin();
  mqtt::begin();
  runTask(2);
  CHECK(mqtt::status().state == vdm::MqttState::Disabled);
  CHECK(fakes::mqtt().connects == 0);
  CHECK(fakes::rtos().delays == std::vector<uint32_t>{500, 500});
  const vdm::RegulatorInput r = mqtt::regulatorState();
  CHECK(r.mode == vdm::MqttMode::Off);
  CHECK_FALSE(r.brokerConnected);
  CHECK(vdm::regulatorCause(r) == vdm::RegulatorCause::Alive);
}

TEST_CASE("mqtt task: connects with the MAC client id, LWT, online first, wildcard subscriptions") {
  glue::begin();
  const vdm::Config& c = useMqtt();
  mqtt::begin();
  runTask(1);
  const fakes::Mqtt& m = fakes::mqtt();
  CHECK(m.connects == 1);
  CHECK(m.host == "broker.lan");
  CHECK(m.port == 1884);
  CHECK(m.keepAlive == 30);
  CHECK(m.socketTimeout == 5);
  CHECK(m.clientId == "VdMot-123456");  // fake efuse MAC 24:0a:c4:12:34:56
  CHECK_FALSE(m.cleanSession);
  CHECK(m.userNull);
  CHECK(m.passNull);
  const std::string lwt = topicOf(c, vdm::Topic::Status);
  CHECK(m.willTopic == lwt);
  CHECK(m.willMessage == "offline");
  CHECK(m.willRetain);
  CHECK(m.willQos == 0);
  REQUIRE_FALSE(m.published.empty());
  CHECK(m.published[0].topic == lwt);
  CHECK(m.published[0].payload == "online");
  CHECK(m.published[0].retained);
  using S = std::vector<std::pair<std::string, uint8_t>>;
  CHECK(m.subscribed == S{{"VdMot/valves/+/target/set", 1}, {"VdMot/valves/+/target/set/set", 1},
                          {"VdMot/cmd/#", 0}});
  CHECK(mqtt::status().state == vdm::MqttState::Connected);
  CHECK(mqtt::status().reconnects == 1);
  CHECK(std::string(mqtt::status().clientId) == "VdMot-123456");
  CHECK(sib::logger().has(vdm::EventCode::MqttConnected));
  const vdm::RegulatorInput r = mqtt::regulatorState();
  CHECK(r.mode == vdm::MqttMode::Mqtt);
  CHECK(r.brokerConnected);
}

TEST_CASE("mqtt task: a configured client id of the longest allowed length is reported whole") {
  glue::begin();
  vdm::Config& c = useMqtt(vdm::MqttMode::Mqtt);
  const std::string id = "vdmot-" + std::string(vdm::kClientIdMax - 7, 'x') + "9";
  REQUIRE(id.size() == vdm::kClientIdMax);
  vdm::copyString(c.mqtt.clientId, sizeof c.mqtt.clientId, id.c_str());
  mqtt::begin();
  runTask(1);
  CHECK(fakes::mqtt().clientId == id);
  CHECK(mqtt::status().state == vdm::MqttState::Connected);
  CHECK(std::string(mqtt::status().clientId) == id);
}

TEST_CASE("mqtt task: a configured client id is used verbatim; HA mode subscriptions") {
  glue::begin();
  vdm::Config& c = useMqtt(vdm::MqttMode::MqttHa);
  vdm::copyString(c.mqtt.clientId, sizeof c.mqtt.clientId, "my.client-id_1");
  vdm::copyString(c.mqtt.discoveryPrefix, sizeof c.mqtt.discoveryPrefix, "ha");
  vdm::copyString(c.mqtt.rootTopic, sizeof c.mqtt.rootTopic, "VdMotFBH");
  vdm::copyString(c.valves[1].topic, sizeof c.valves[1].topic, "Bad/WC");
  mqtt::begin();
  runTask(1);
  const fakes::Mqtt& m = fakes::mqtt();
  CHECK(m.clientId == "my.client-id_1");
  CHECK(m.willTopic == "VdMotFBH/status");
  using S = std::vector<std::pair<std::string, uint8_t>>;
  CHECK(m.subscribed == S{{"VdMotFBH/valves/+/target/set", 1}, {"VdMotFBH/valves/+/target/set/set", 1},
                          {"VdMotFBH/cmd/#", 0}, {"VdMotFBH/valves/Bad/WC/target/set", 1},
                          {"VdMotFBH/valves/Bad/WC/target/set/set", 1}, {"homeassistant/status", 1},
                          {"ha/status", 1}});
}

TEST_CASE("mqtt task: credentials go out only as a pair") {
  glue::begin();
  vdm::Config& c = useMqtt();
  vdm::copyString(c.mqtt.user, sizeof c.mqtt.user, "u");
  mqtt::begin();
  runTask(1);
  CHECK(fakes::mqtt().userNull);
  CHECK(fakes::mqtt().passNull);
}

TEST_CASE("mqtt task: a refused connect is an error with the library state") {
  glue::begin();
  useMqtt();
  fakes::mqtt().connectResult = false;
  fakes::mqtt().failState = MQTT_CONNECT_UNAUTHORIZED;
  mqtt::begin();
  runTask(1);
  CHECK(mqtt::status().state == vdm::MqttState::Error);
  CHECK(mqtt::status().rc == MQTT_CONNECT_UNAUTHORIZED);
  CHECK(fakes::rtos().delays == std::vector<uint32_t>{100});
}

TEST_CASE("mqtt task: a broker that drops at once is retried at 2, 4, 8 s (W20-3)") {
  glue::begin();
  useMqtt();
  std::vector<uint64_t> connectedAt;
  int seen = 0;
  fakes::rtos().onDelay = [&](uint32_t) {
    fakes::Mqtt& m = fakes::mqtt();
    if (m.connects != seen) {
      seen = m.connects;
      connectedAt.push_back(fakes::nowMs());
    }
    if (m.connected) m.dropConnection();
  };
  mqtt::begin();
  runTask(150);  // 15 s of 100 ms passes
  REQUIRE(connectedAt.size() == 4);
  CHECK(connectedAt[1] - connectedAt[0] == 2100);
  CHECK(connectedAt[2] - connectedAt[1] == 4100);
  CHECK(connectedAt[3] - connectedAt[2] == 8100);
}

TEST_CASE("mqtt task: clean session only after a topic config change (W3-4)") {
  glue::begin();
  vdm::Config& c = useMqtt();
  settle(2);
  CHECK_FALSE(fakes::mqtt().cleanSession);
  c.failsafe.timeoutMin = 30;  // applies live: no reconnect
  c.valves[0].failsafePct = 20;
  ++sib::storage().revision;
  runTask(2);
  CHECK(fakes::mqtt().connects == 1);
  c.mqtt.separate = false;
  ++sib::storage().revision;
  runTask(2);
  CHECK(fakes::mqtt().connects == 2);
  CHECK(fakes::mqtt().cleanSession);
  using S = std::vector<std::pair<std::string, uint8_t>>;
  const S tail(fakes::mqtt().subscribed.end() - 3, fakes::mqtt().subscribed.end());
  CHECK(tail == S{{"VdMot/valves/+/target", 1}, {"VdMot/valves/+/target/set", 1}, {"VdMot/cmd/#", 0}});
  mqtt::requestReconnect();
  runTask(2);
  CHECK(fakes::mqtt().connects == 3);
  CHECK_FALSE(fakes::mqtt().cleanSession);
}

TEST_CASE("mqtt inbound: a target is submitted and its retained topic cleared (W3-5)") {
  glue::begin();
  useMqtt();
  settle();
  const long before = static_cast<long>(fakes::mqtt().published.size());
  deliver("VdMot/valves/1/target/set", "40");
  runTask(1);
  REQUIRE(sib::app().submitted.size() == 1);
  const app::Command& cmd = sib::app().submitted[0];
  CHECK(cmd.type == app::CommandType::SetTarget);
  CHECK(cmd.valve == 0);
  CHECK(cmd.pos == 40);
  CHECK(cmd.source == vdm::TargetSource::Mqtt);
  REQUIRE(static_cast<long>(fakes::mqtt().published.size()) > before);
  const fakes::MqttMessage clear = fakes::mqtt().published[before];
  CHECK(clear.topic == "VdMot/valves/1/target/set");
  CHECK(clear.payload == "");
  CHECK(clear.retained);
  CHECK(mqtt::regulatorState().commandSeq == 1);
  // The broker echoes the empty message: nothing happens.
  deliver("VdMot/valves/1/target/set", "");
  runTask(2);
  CHECK(sib::app().submitted.size() == 1);
  CHECK(rejected().empty());
  CHECK(mqtt::status().commandsRejected == 0);
  CHECK(mqtt::regulatorState().commandSeq == 1);
  CHECK(payloads("VdMot/valves/1/target/set").size() == 1);
}

TEST_CASE("mqtt inbound: fractions round half up, the number form of a named valve (W6-3)") {
  glue::begin();
  vdm::Config& c = useMqtt();
  vdm::copyString(c.valves[0].name, sizeof c.valves[0].name, "Bad");
  settle(2);
  deliver("VdMot/valves/1/target/set", "43,7");
  deliver("VdMot/valves/Bad/target/set/set", "43.49");
  runTask(2);
  REQUIRE(sib::app().submitted.size() == 2);
  CHECK(sib::app().submitted[0].pos == 44);
  CHECK(sib::app().submitted[1].pos == 43);
}

TEST_CASE("mqtt inbound: without separate the ESP's own target is not a command (W3-7)") {
  glue::begin();
  vdm::Config& c = useMqtt();
  c.mqtt.separate = false;
  vdm::StmSnapshot& s = linkUp();
  s.valves[0].desiredValid = true;
  s.valves[0].desired = 40;
  settle();
  REQUIRE(last("VdMot/valves/1/target") == "40");
  const size_t pubs = payloads("VdMot/valves/1/target").size();
  deliver("VdMot/valves/1/target", "40");
  runTask(2);
  CHECK(sib::app().submitted.empty());
  deliver("VdMot/valves/1/target", "41");
  runTask(2);
  REQUIRE(sib::app().submitted.size() == 1);
  CHECK(sib::app().submitted[0].pos == 41);
  CHECK(payloads("VdMot/valves/1/target").size() == pubs);  // no clear on the state form
  deliver("VdMot/valves/1/target/set", "42");
  runTask(2);
  REQUIRE(sib::app().submitted.size() == 2);
  CHECK(last("VdMot/valves/1/target/set") == "");  // the /set form is cleared
}

TEST_CASE("mqtt inbound: rejections are counted, logged once per 10 s (E28-3)") {
  glue::begin();
  useMqtt();
  settle(2);
  deliver("VdMot/valves/9/target/set", "50");
  runTask(1);
  std::vector<vdm::Event> ev = rejected();
  REQUIRE(ev.size() == 1);
  CHECK(ev[0].arg1 == 9);
  CHECK(ev[0].arg2 == 0);
  CHECK(std::string(ev[0].text) == "inactive");
  CHECK(ev[0].valve == vdm::kNoValve);
  CHECK(mqtt::status().commandsRejected == 1);
  CHECK(last("VdMot/valves/9/target/set") == "");  // cleared as well
  for (int i = 0; i < 99; ++i) deliver("VdMot/valves/9/target/set", "50");
  runTask(60);
  CHECK(rejected().size() == 1);
  CHECK(mqtt::status().commandsRejected == 100);
  CHECK(sib::app().submitted.empty());
  // Other reasons and valves are logged at once.
  deliver("VdMot/valves/1/target/set", "abc");
  deliver("VdMot/valves/Old/target/set", "5");
  deliver("VdMot/cmd/foo", "PRESS");
  runTask(3);
  ev = rejected();
  REQUIRE(ev.size() == 4);
  CHECK(ev[1].arg1 == 1);
  CHECK(ev[1].arg2 == static_cast<int32_t>(vdm::TargetPayload::NotNumber));
  CHECK(std::string(ev[1].text) == "payload");
  CHECK(ev[2].arg1 == 0);
  CHECK(std::string(ev[2].text) == "unknown valve");
  CHECK(std::string(ev[3].text) == "unknown command");
  CHECK(mqtt::status().commandsRejected == 103);
}

TEST_CASE("mqtt inbound: messages beyond the 4 inbound slots of one loop are rejected as queue full") {
  glue::begin();
  vdm::Config& c = useMqtt();
  for (int v = 0; v < 6; ++v) c.valves[v].active = true;
  settle(2);
  fakes::mqtt().burst = true;
  for (int v = 1; v <= 6; ++v) deliver("VdMot/valves/" + std::to_string(v) + "/target/set", "30");
  runTask(1);
  REQUIRE(sib::app().submitted.size() == 4);
  for (size_t i = 0; i < 4; ++i) CHECK(sib::app().submitted[i].valve == i);
  const std::vector<vdm::Event> ev = rejected();
  REQUIRE(ev.size() == 1);  // the second loss is counted, its log is rate-limited
  CHECK(std::string(ev[0].text) == "queue full");
  CHECK(ev[0].arg1 == 0);
  CHECK(mqtt::status().commandsRejected == 2);
  // The overflow is drained: the next loop starts clean.
  deliver("VdMot/valves/5/target/set", "30");
  runTask(1);
  CHECK(sib::app().submitted.size() == 5);
  CHECK(mqtt::status().commandsRejected == 2);
}

TEST_CASE("mqtt inbound: a button acts only after the broker echoed its clear (H13)") {
  glue::begin();
  useMqtt();
  settle(2);
  deliver("VdMot/cmd/restart", "PRESS");
  runTask(1);
  CHECK(last("VdMot/cmd/restart") == "");
  CHECK(sib::ota().restartRequests.empty());
  deliver("VdMot/cmd/restart", "");
  runTask(1);
  REQUIRE(sib::ota().restartRequests.size() == 1);
  CHECK(sib::ota().restartRequests[0].reason == 0);
  CHECK(sib::ota().restartRequests[0].delayMs == 1000);
  CHECK(mqtt::regulatorState().commandSeq == 1);
  CHECK(rejected().empty());
}

TEST_CASE("mqtt inbound: a button without the echo is rejected after 5 s") {
  glue::begin();
  useMqtt();
  settle(2);
  deliver("VdMot/cmd/restart", "PRESS");
  runTask(1);
  runTask(245);  // 4.9 s
  CHECK(rejected().empty());
  runTask(10);
  const std::vector<vdm::Event> ev = rejected();
  REQUIRE(ev.size() == 1);
  CHECK(std::string(ev[0].text) == "clear not confirmed");
  CHECK(sib::ota().restartRequests.empty());
  deliver("VdMot/cmd/restart", "");  // a late echo does nothing
  runTask(2);
  CHECK(sib::ota().restartRequests.empty());
}

TEST_CASE("mqtt inbound: a button whose clear publish fails is rejected (W3-6)") {
  glue::begin();
  useMqtt();
  settle(2);
  deliver("VdMot/cmd/restart", "PRESS");
  fakes::mqtt().failPublishAt = fakes::mqtt().publishCalls;
  runTask(1);
  CHECK(sib::ota().restartRequests.empty());
  const std::vector<vdm::Event> ev = rejected();
  REQUIRE(ev.size() == 1);
  CHECK(std::string(ev[0].text) == "clear not confirmed");
  CHECK(mqtt::status().publishFailures >= 1);
}

TEST_CASE("mqtt inbound: every cmd topic submits its command after the echo (W15-5)") {
  glue::begin();
  useMqtt();
  vdm::StmSnapshot& s = linkUp();
  s.proto = 3;
  settle(2);
  struct Row {
    const char* topic;
    app::CommandType type;
    uint8_t valve;
  };
  const Row rows[] = {
      {"VdMot/cmd/valves/1/calibrate", app::CommandType::Calibrate, 0},
      {"VdMot/cmd/calibrate", app::CommandType::Calibrate, vdm::kAllValves},
      {"VdMot/cmd/stmReset", app::CommandType::ResetStm, vdm::kNoValve},
      {"VdMot/cmd/detect", app::CommandType::Detect, vdm::kAllValves},
      {"VdMot/cmd/stop", app::CommandType::StopValve, vdm::kAllValves},
      {"VdMot/cmd/stmSafeExit", app::CommandType::LeaveSafeMode, vdm::kNoValve},
  };
  size_t n = 0;
  for (const Row& r : rows) {
    CAPTURE(r.topic);
    deliver(r.topic, "PRESS");
    runTask(1);
    CHECK(sib::app().submitted.size() == n);
    CHECK(last(r.topic) == "");
    deliver(r.topic, "");
    runTask(1);
    REQUIRE(sib::app().submitted.size() == n + 1);
    CHECK(sib::app().submitted[n].type == r.type);
    CHECK(sib::app().submitted[n].valve == r.valve);
    ++n;
  }
  CHECK(mqtt::regulatorState().commandSeq == 6);
  // STOP on a valve target (protocol 3).
  deliver("VdMot/valves/1/target/set", "STOP");
  runTask(1);
  REQUIRE(sib::app().submitted.size() == 7);
  CHECK(sib::app().submitted[6].type == app::CommandType::StopValve);
  CHECK(sib::app().submitted[6].valve == 0);
  // A full queue rejects a button.
  sib::app().submitResult = false;
  deliver("VdMot/cmd/detect", "PRESS");
  deliver("VdMot/cmd/detect", "");
  runTask(2);
  const std::vector<vdm::Event> ev = rejected();
  REQUIRE(ev.size() == 1);
  CHECK(std::string(ev[0].text) == "queue full");
}

TEST_CASE("mqtt inbound: stop and safe exit need protocol 3") {
  glue::begin();
  useMqtt();
  linkUp();  // protocol 2
  settle(2);
  deliver("VdMot/cmd/stop", "PRESS");
  deliver("VdMot/valves/1/target/set", "STOP");
  runTask(2);
  CHECK(sib::app().submitted.empty());
  const std::vector<vdm::Event> ev = rejected();
  REQUIRE(ev.size() == 2);
  CHECK(std::string(ev[0].text) == "unsupported");
  CHECK(ev[1].arg1 == 1);
}

TEST_CASE("mqtt inbound: a refused target is re-submitted, the newest wins (H14)") {
  glue::begin();
  useMqtt();
  settle(2);
  sib::app().submitResult = false;
  deliver("VdMot/valves/1/target/set", "10");
  deliver("VdMot/valves/1/target/set", "20");
  deliver("VdMot/valves/1/target/set", "30");
  runTask(3);
  CHECK(sib::app().submitted.empty());
  CHECK(rejected().empty());
  sib::app().submitResult = true;
  runTask(2);
  REQUIRE(sib::app().submitted.size() == 1);
  CHECK(sib::app().submitted[0].pos == 30);
  runTask(2);
  CHECK(sib::app().submitted.size() == 1);
}

TEST_CASE("mqtt HA status: offline/online, commands, discovery on the way back (K1-4)") {
  glue::begin();
  vdm::Config& c = useMqtt(vdm::MqttMode::MqttHa);
  c.mqtt.haDiscoveryOnConnect = false;
  settle(2);
  deliver("homeassistant/status", "offline");
  runTask(1);
  CHECK(mqtt::regulatorState().ha == vdm::HaStatus::Offline);
  CHECK(mqtt::status().haStatus == vdm::HaStatus::Offline);
  CHECK(vdm::regulatorCause(mqtt::regulatorState()) == vdm::RegulatorCause::HaOffline);
  CHECK(payloads("homeassistant/status").empty());  // never cleared
  // An accepted command brings it back without a discovery run.
  deliver("VdMot/valves/1/target/set", "10");
  runTask(1);
  CHECK(mqtt::regulatorState().ha == vdm::HaStatus::Online);
  CHECK(mqtt::regulatorState().commandSeq == 1);
  CHECK_FALSE(mqtt::status().discoveryRunning);
  // The status survives a software restart (RTC).
  deliver("homeassistant/status", "offline");
  runTask(1);
  mqtt::begin();
  CHECK(mqtt::regulatorState().ha == vdm::HaStatus::Offline);
  // online after offline runs discovery when enabled; a second online does not.
  c.mqtt.haDiscoveryOnConnect = true;
  ++sib::storage().revision;
  runTask(3);
  deliver("homeassistant/status", "online");
  runTask(1);
  CHECK(mqtt::regulatorState().ha == vdm::HaStatus::Online);
  CHECK(mqtt::status().discoveryRunning);
  runTask(400);
  CHECK_FALSE(mqtt::status().discoveryRunning);
  const size_t runs = sib::logger().withCode(vdm::EventCode::HaDiscoverySent).size();
  deliver("homeassistant/status", "online");
  runTask(2);
  CHECK_FALSE(mqtt::status().discoveryRunning);
  CHECK(sib::logger().withCode(vdm::EventCode::HaDiscoverySent).size() == runs);
  // Broker lost.
  fakes::mqtt().dropConnection();
  fakes::mqtt().connectResult = false;
  runTask(1);
  CHECK_FALSE(mqtt::regulatorState().brokerConnected);
  CHECK(vdm::regulatorCause(mqtt::regulatorState()) == vdm::RegulatorCause::BrokerDown);
  // MQTT switched off.
  c.mqtt.mode = vdm::MqttMode::Off;
  ++sib::storage().revision;
  runTask(1);
  CHECK(mqtt::regulatorState().mode == vdm::MqttMode::Off);
  CHECK(vdm::regulatorCause(mqtt::regulatorState()) == vdm::RegulatorCause::Alive);
}

TEST_CASE("mqtt HA status: ignored outside HA mode") {
  glue::begin();
  useMqtt(vdm::MqttMode::Mqtt);
  settle(2);
  deliver("homeassistant/status", "offline");
  runTask(1);
  CHECK(mqtt::regulatorState().ha == vdm::HaStatus::Unknown);
}

TEST_CASE("mqtt values: stm/status per link state, once per change (K3-4)") {
  glue::begin();
  useMqtt();
  vdm::StmSnapshot& s = linkUp();
  settle();
  CHECK(payloads("VdMot/stm/status") == std::vector<std::string>{"online"});
  CHECK(fakes::mqtt().publishedTo("VdMot/stm/status")[0].retained);
  s.link = vdm::LinkState::Degraded;
  publishSnap();
  runTask(3);
  CHECK(payloads("VdMot/stm/status").size() == 1);
  for (vdm::LinkState l : {vdm::LinkState::Down, vdm::LinkState::Unknown, vdm::LinkState::Booting,
                           vdm::LinkState::Suspended}) {
    s.link = l;
    publishSnap();
    runTask(3);
    CHECK(last("VdMot/stm/status") == "offline");
  }
  CHECK(payloads("VdMot/stm/status").size() == 2);
  s.link = vdm::LinkState::Up;
  publishSnap();
  runTask(3);
  CHECK(payloads("VdMot/stm/status").size() == 3);
  CHECK(last("VdMot/stm/status") == "online");
  // Every full publish repeats it.
  runTask(600);  // > publishIntervalS (10 s)
  CHECK(payloads("VdMot/stm/status").size() > 3);
}

TEST_CASE("mqtt values: failsafe, lease valve and common/state (K1-5)") {
  glue::begin();
  useMqtt();
  vdm::StmSnapshot& s = linkUp();
  s.lease.state = vdm::LeaseState::Expired;
  s.valves[0].fsOverride = true;
  s.valves[0].health = vdm::kHealthFailsafe;
  settle();
  CHECK(last("VdMot/failsafe") == "1");
  CHECK(fakes::mqtt().publishedTo("VdMot/failsafe").back().retained);
  CHECK(last("VdMot/valves/1/failsafe/value") == "lease");
  CHECK(last("VdMot/common/state/value") == "info");
  CHECK(last("VdMot/diag/stm/lease") == "expired");
  s.lease.state = vdm::LeaseState::Running;
  s.valves[0].fsOverride = false;
  s.valves[0].health = 0;
  publishSnap();
  runTask(300);  // > minDelayS
  CHECK(last("VdMot/failsafe") == "0");
  CHECK(last("VdMot/valves/1/failsafe/value") == "off");
  CHECK(last("VdMot/common/state/value") == "ok");
  CHECK(last("VdMot/diag/stm/lease") == "running");
}

TEST_CASE("mqtt values: a blocked valve at its failsafe position (K2-3)") {
  glue::begin();
  useMqtt();
  vdm::StmSnapshot& s = linkUp();
  s.proto = 3;
  s.valves[0].hasV3 = true;
  s.valves[0].stmFlags = vdm::kStmFlagFsBlocked;
  s.valves[0].fsPct = 50;
  s.valves[0].status = 9;
  s.valves[0].health = vdm::kHealthBlocked;
  s.haveStatus = true;
  s.status.v3 = true;
  s.status.safeMode = true;
  settle();
  CHECK(last("VdMot/valves/1/failsafe/value") == "blocked");
  CHECK(last("VdMot/valves/1/problem/value") == "1");
  CHECK(last("VdMot/common/state/value") == "error");
  CHECK(last("VdMot/diag/stm/safeMode") == "1");
}

TEST_CASE("mqtt values: requested, sync and the read-back target (W5-2)") {
  glue::begin();
  useMqtt();
  vdm::StmSnapshot& s = linkUp();
  vdm::ValveState& v = s.valves[0];
  v.stmTargetKnown = true;
  v.stmTarget = 30;
  v.desiredValid = true;
  v.desired = 30;
  v.sync = vdm::TargetSync::Synced;
  settle();
  CHECK(last("VdMot/valves/1/target/value") == "30");
  CHECK(last("VdMot/valves/1/requested/value") == "30");
  CHECK(last("VdMot/valves/1/sync/value") == "synced");
  CHECK(last("VdMot/valves/1/problem/value") == "0");
  const size_t targets = payloads("VdMot/valves/1/target/value").size();
  v.desired = 60;
  v.source = vdm::TargetSource::Mqtt;
  v.sync = vdm::TargetSync::AwaitVerify;
  publishSnap();
  runTask(300);  // > minDelayS
  CHECK(last("VdMot/valves/1/requested/value") == "60");
  CHECK(last("VdMot/valves/1/sync/value") == "await_verify");
  CHECK(last("VdMot/valves/1/target/value") == "30");
  v.stmTarget = 60;
  v.sync = vdm::TargetSync::Synced;
  publishSnap();
  runTask(300);  // > minDelayS
  CHECK(last("VdMot/valves/1/target/value") == "60");
  CHECK(last("VdMot/valves/1/sync/value") == "synced");
  CHECK(payloads("VdMot/valves/1/target/value").size() > targets);
  // Delivery failed: problem.
  v.sync = vdm::TargetSync::Failed;
  v.health = vdm::kHealthTargetUnconfirmed;
  publishSnap();
  runTask(300);  // > minDelayS
  CHECK(last("VdMot/valves/1/sync/value") == "failed");
  CHECK(last("VdMot/valves/1/problem/value") == "1");
  // Read-back pending: nothing published for target.
  const size_t n = payloads("VdMot/valves/1/target/value").size();
  v.stmTargetKnown = false;
  v.sync = vdm::TargetSync::Pending;
  publishSnap();
  runTask(300);  // > minDelayS
  CHECK(payloads("VdMot/valves/1/target/value").size() == n);
}

TEST_CASE("mqtt values: a failed temperature and link loss show as problems (K4-4, W5-3)") {
  glue::begin();
  useMqtt();
  vdm::StmSnapshot& s = linkUp();
  s.valves[0].temp1 = vdm::kTempReadError;
  s.valves[0].health = vdm::kHealthTempFailed;
  settle();
  CHECK(last("VdMot/valves/1/temp1/value") == "failed");
  CHECK(last("VdMot/valves/1/problem/value") == "1");
  s.valves[0].temp1 = 215;
  s.valves[0].health = 0;
  publishSnap();
  runTask(300);  // > minDelayS
  CHECK(last("VdMot/valves/1/temp1/value") == "21.5");
  CHECK(last("VdMot/valves/1/problem/value") == "0");
  s.link = vdm::LinkState::Down;
  s.valves[0].health = vdm::kHealthStale;
  publishSnap();
  runTask(300);  // > minDelayS
  CHECK(last("VdMot/stm/status") == "offline");
  CHECK(last("VdMot/valves/1/problem/value") == "1");
  CHECK(last("VdMot/common/state/value") == "error");
}

TEST_CASE("mqtt values: unnamed sensors use the bus index, inactive volts are published (E22, E23)") {
  glue::begin();
  vdm::Config& c = useMqtt();
  vdm::OneWireId t1;
  t1.b[0] = 0x28;
  t1.b[7] = 1;
  c.temps[0].active = true;
  c.temps[0].id = t1;
  vdm::OneWireId v1;
  v1.b[0] = 0x26;
  v1.b[7] = 2;
  c.volts[0].id = v1;  // inactive
  vdm::copyString(c.volts[0].unit, sizeof c.volts[0].unit, "V");
  vdm::StmSnapshot& s = linkUp();
  s.tempCount = 5;
  s.temps[4].id = t1;
  s.temps[4].seen = true;
  s.temps[4].raw = 200;
  s.voltCount = 3;
  s.volts[2].id = v1;
  s.volts[2].seen = true;
  s.volts[2].vad = 1200;
  settle();
  CHECK(last("VdMot/temps/5/id/value") == "28-00-00-00-00-00-00-01");
  CHECK(last("VdMot/temps/5/value/value") == "20.0");
  CHECK(payloads("VdMot/temps/1/id/value").empty());
  CHECK(last("VdMot/sensors/3/id/value") == "26-00-00-00-00-00-00-02");
  CHECK(last("VdMot/sensors/3/value/value") == "12.000");
  CHECK(last("VdMot/sensors/3/unit/value") == "V");
  // Off the bus: an unnamed sensor is not published at all.
  const size_t n = fakes::mqtt().published.size();
  s.tempCount = 0;
  s.voltCount = 0;
  publishSnap();
  runTask(700);
  for (size_t i = n; i < fakes::mqtt().published.size(); ++i) {
    CHECK(fakes::mqtt().published[i].topic.find("VdMot/temps/") == std::string::npos);
  }
}

TEST_CASE("mqtt diag: version, started, calibration next, counters (E29-3, W14)") {
  glue::begin();
  useMqtt();
  vdm::StmSnapshot& s = linkUp();
  s.version.valid = true;
  s.version.major = 2;
  s.version.minor = 1;
  vdm::copyString(s.version.suffix, sizeof s.version.suffix, "-revamped");
  vdm::copyString(s.version.hw, sizeof s.version.hw, "C2");
  s.haveStatus = true;
  s.status.uptimeS = 1000;
  sib::net().localTime.valid = true;
  sib::net().localTime.epoch = 1790072393;
  sib::app().calib.nextEpoch = 1790080000;
  settle(4);
  CHECK(last("VdMot/diag/stm/version") == "2.1.0-revamped_C2");
  CHECK(payloads("VdMot/diag/stm/started") == std::vector<std::string>{"2026-09-22T10:03:13+00:00"});
  CHECK(last("VdMot/diag/calibration/next") == "2026-09-22T12:26:40+00:00");
  CHECK(last("VdMot/diag/mqtt/commandsRejected") == "0");
  CHECK(last("VdMot/diag/mqtt/eventsSuppressed") == "0");
  // uptime +10 and epoch +10: nothing new.
  s.status.uptimeS = 1010;
  sib::net().localTime.epoch = 1790072403;
  publishSnap();
  runTask(3);
  CHECK(payloads("VdMot/diag/stm/started").size() == 1);
  // STM reboot: a new start time.
  s.status.uptimeS = 5;
  publishSnap();
  runTask(3);
  CHECK(last("VdMot/diag/stm/started") == "2026-09-22T10:19:58+00:00");
  sib::app().calib.nextEpoch = 0;
  runTask(2);
  CHECK(last("VdMot/diag/calibration/next") == "");
  // Counters at most every 10 s.
  deliver("VdMot/valves/9/target/set", "1");
  runTask(2);
  CHECK(last("VdMot/diag/mqtt/commandsRejected") == "0");
  runTask(500);
  CHECK(last("VdMot/diag/mqtt/commandsRejected") == "1");
}

TEST_CASE("mqtt events: 12 valves of one code become one message (W14-4)") {
  glue::begin();
  useMqtt();
  settle(2);
  for (uint8_t v = 0; v < vdm::kValveCount; ++v) {
    logger::log(vdm::EventCode::ValveStale, v, 60, 0, nullptr);
  }
  runTask(150);  // 3 s
  const std::vector<std::string> ev = payloads("VdMot/events");
  REQUIRE(ev.size() == 1);
  CHECK(ev[0].find("\"name\":\"valve_stale\",\"event_type\":\"valve_stale\",\"valve\":null,"
                   "\"valves\":[1,2,3,4,5,6,7,8,9,10,11,12]") != std::string::npos);
  CHECK_FALSE(fakes::mqtt().publishedTo("VdMot/events")[0].retained);
  // A single system event goes out at once.
  logger::log(vdm::EventCode::LinkDown, vdm::kNoValve, 3, 0, nullptr);
  runTask(5);  // a new task() call reads the log from its start again, 4 events per pass
  REQUIRE(payloads("VdMot/events").size() == 2);
  CHECK(payloads("VdMot/events")[1].find("\"code\":302") != std::string::npos);
}

TEST_CASE("mqtt discovery: the 2.0.0 migration runs once in HA mode (K3-3, K3-5)") {
  glue::begin();
  vdm::Config& c = useMqtt(vdm::MqttMode::MqttHa);
  c.mqtt.haDiscoveryOnConnect = false;
  sib::storage().haLayout = 0;
  mqtt::begin();
  runTask(500);
  CHECK(last("homeassistant/sensor/VdMot/diag_stm_uptime/config") == "");
  CHECK(sib::storage().haLayoutSets == std::vector<uint8_t>{2});
  CHECK(last("homeassistant/text/VdMot/state/config").find("\"unique_id\":\"VdMot.common.state\"") !=
        std::string::npos);
  REQUIRE(fakes::fs().exists("/HADiscovery.cfg"));
  CHECK(fakes::fs().read("/HADiscovery.cfg").find("homeassistant/text/VdMot/state/config\n") == 0);
  CHECK_FALSE(fakes::fs().exists("/HADiscovery.cfg.tmp"));
  const std::vector<vdm::Event> sent = sib::logger().withCode(vdm::EventCode::HaDiscoverySent);
  REQUIRE(sent.size() == 1);
  CHECK(sent[0].arg1 > 20);
  // The next connect: no run.
  const size_t before = fakes::mqtt().published.size();
  mqtt::requestReconnect();
  runTask(100);
  for (size_t i = before; i < fakes::mqtt().published.size(); ++i) {
    CHECK(fakes::mqtt().published[i].topic.find("homeassistant/") != 0);
  }
  // Delete (empty list) keeps the file.
  mqtt::requestDiscovery(mqtt::DiscoveryAction::Delete);
  runTask(300);
  REQUIRE(fakes::fs().exists("/HADiscovery.cfg"));
  CHECK(fakes::fs().read("/HADiscovery.cfg") == "");
  mqtt::requestDiscovery(mqtt::DiscoveryAction::DeleteAndPublish);
  runTask(500);
  REQUIRE(fakes::fs().exists("/HADiscovery.cfg"));
  CHECK(fakes::fs().read("/HADiscovery.cfg").find("homeassistant/text/VdMot/state/config\n") == 0);
}

TEST_CASE("mqtt discovery: a renamed valve loses its old config first (W4-8)") {
  glue::begin();
  vdm::Config& c = useMqtt(vdm::MqttMode::MqttHa);
  vdm::copyString(c.valves[0].name, sizeof c.valves[0].name, "Old");
  mqtt::begin();
  runTask(500);
  REQUIRE(fakes::fs().read("/HADiscovery.cfg").find("valves_state_Old") != std::string::npos);
  vdm::copyString(c.valves[0].name, sizeof c.valves[0].name, "New");
  ++sib::storage().revision;
  const size_t before = fakes::mqtt().published.size();
  runTask(500);
  long delOld = -1;
  long pubNew = -1;
  for (size_t i = before; i < fakes::mqtt().published.size(); ++i) {
    const fakes::MqttMessage& m = fakes::mqtt().published[i];
    if (m.topic == "homeassistant/text/VdMot/valves_state_Old/config" && m.payload.empty() && delOld < 0) {
      delOld = static_cast<long>(i);
    }
    if (m.topic == "homeassistant/text/VdMot/valves_state_New/config" && !m.payload.empty() && pubNew < 0) {
      pubNew = static_cast<long>(i);
    }
  }
  REQUIRE(delOld >= 0);
  REQUIRE(pubNew >= 0);
  CHECK(delOld < pubNew);
  const std::string list = fakes::fs().read("/HADiscovery.cfg");
  CHECK(list.find("valves_state_New") != std::string::npos);
  CHECK(list.find("valves_state_Old") == std::string::npos);
  CHECK(fakes::mqtt().cleanSession);
}

TEST_CASE("mqtt discovery: manual runs in mode 1, none automatic (E24-2)") {
  glue::begin();
  useMqtt(vdm::MqttMode::Mqtt);
  settle(2);
  mqtt::requestDiscovery(mqtt::DiscoveryAction::Publish);
  runTask(500);
  CHECK(last("homeassistant/text/VdMot/state/config").find("\"name\":\"state\"") != std::string::npos);
  CHECK(sib::logger().withCode(vdm::EventCode::HaDiscoverySent).size() == 1);
  mqtt::requestReconnect();
  const size_t before = fakes::mqtt().published.size();
  runTask(100);
  for (size_t i = before; i < fakes::mqtt().published.size(); ++i) {
    CHECK(fakes::mqtt().published[i].topic.find("homeassistant/") != 0);
  }
}

TEST_CASE("mqtt discovery: a failed publish aborts the run, the next connect starts again") {
  glue::begin();
  useMqtt(vdm::MqttMode::MqttHa);
  sib::storage().haCleanupDone = false;
  sib::storage().haLayout = 0;
  mqtt::begin();
  runTask(30);
  REQUIRE(mqtt::status().discoveryRunning);
  fakes::mqtt().failPublishAt = fakes::mqtt().publishCalls;
  runTask(1);
  CHECK_FALSE(mqtt::status().discoveryRunning);
  CHECK(sib::storage().haCleanupMarks == 0);
  CHECK(sib::storage().haLayoutSets.empty());
  CHECK(sib::logger().withCode(vdm::EventCode::HaDiscoverySent).empty());
  runTask(800);
  CHECK(sib::storage().haCleanupMarks == 1);
  CHECK(sib::storage().haLayoutSets == std::vector<uint8_t>{2});
  CHECK(sib::logger().withCode(vdm::EventCode::HaDiscoverySent).size() == 1);
}

TEST_CASE("mqtt task: the watchdog is reset after every publish (E12-4)") {
  glue::begin();
  useMqtt();
  linkUp();
  mqtt::begin();
  runTask(1);
  const long pubs = fakes::mqtt().publishCalls;
  const int resets = fakes::esp().wdtResets;
  runTask(1);
  const long n = fakes::mqtt().publishCalls - pubs;
  CHECK(n > 3);
  CHECK(fakes::esp().wdtResets - resets >= n + 1);
}

TEST_CASE("mqtt calibrationEnd: the end of the last calibration since boot") {
  glue::begin();
  useMqtt();
  vdm::StmSnapshot& s = linkUp();
  vdm::LocalTime t;
  CHECK_FALSE(mqtt::calibrationEnd(0, t));
  s.valves[0].calibrating = true;
  settle(2);
  sib::net().localTime.valid = true;
  sib::net().localTime.year = 2026;
  sib::net().localTime.epoch = 1790000000;
  s.valves[0].calibrating = false;
  publishSnap();
  runTask(2);
  REQUIRE(mqtt::calibrationEnd(0, t));
  CHECK(t.epoch == 1790000000);
  CHECK_FALSE(mqtt::calibrationEnd(1, t));
  CHECK_FALSE(mqtt::calibrationEnd(12, t));
}

TEST_CASE("mqtt task: a pending restart sends offline and disconnects cleanly") {
  glue::begin();
  const vdm::Config& c = useMqtt();
  mqtt::begin();
  runTask(1);
  REQUIRE(fakes::mqtt().connected);
  sib::ota().restartPending = true;
  runTask(2);
  const fakes::MqttMessage lastMsg = fakes::mqtt().published.back();
  CHECK(lastMsg.topic == topicOf(c, vdm::Topic::Status));
  CHECK(lastMsg.payload == "offline");
  CHECK(lastMsg.retained);
  CHECK(fakes::mqtt().disconnects == 1);
  CHECK(mqtt::status().state == vdm::MqttState::Disabled);
  CHECK(fakes::mqtt().connects == 1);
}

TEST_CASE("mqtt requests: reconnect and discovery are taken over by the task") {
  glue::begin();
  useMqtt();
  mqtt::begin();
  runTask(1);
  mqtt::requestReconnect();
  runTask(1);
  CHECK(fakes::mqtt().disconnects == 1);
  CHECK(fakes::mqtt().connects == 2);
}
