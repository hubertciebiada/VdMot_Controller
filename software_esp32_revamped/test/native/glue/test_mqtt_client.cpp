// Smoke tests of src/mqtt_client.cpp: session, LWT, subscriptions, inbound commands, restart.
#include <vdm/mqtt_topics.h>

#include <string>

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
  sib::net().up = true;
  return c;
}

vdm::TopicContext topics(const vdm::Config& c) {
  vdm::TopicContext t;
  vdm::copyString(t.station, sizeof t.station, c.station);
  t.pathAsRoot = c.mqtt.pathAsRoot;
  t.separate = c.mqtt.separate;
  return t;
}

std::string topicOf(const vdm::Config& c, vdm::Topic t, const char* segment = nullptr) {
  char buf[vdm::kTopicMax + 1];
  vdm::buildTopic(topics(c), t, segment, buf, sizeof buf);
  return buf;
}

std::string commandTopic(const vdm::Config& c, uint8_t valve) {
  char seg[vdm::kSegmentMax + 1];
  vdm::buildSegment(c.valves[valve].name, valve, seg, sizeof seg);
  char buf[vdm::kTopicMax + 1];
  vdm::buildTargetCommandTopic(topics(c), seg, buf, sizeof buf);
  return buf;
}

// Runs the MQTT task for `passes` loop passes.
void runTask(long passes) {
  fakes::rtos().stopAfterYields(passes);
  CHECK_THROWS_AS(mqtt::task(nullptr), fakes::YieldLimit);
}

}  // namespace

TEST_CASE("mqtt begin: the PubSubClient buffer is 1792 bytes") {
  glue::begin();
  mqtt::begin();
  CHECK(fakes::mqtt().bufferSize == 1792);
  CHECK(fakes::mqtt().bufferSizeCalls == 1);
}

TEST_CASE("mqtt task: MQTT off stays disabled and never connects") {
  glue::begin();
  mqtt::begin();
  runTask(2);
  CHECK(mqtt::status().state == vdm::MqttState::Disabled);
  CHECK(fakes::mqtt().connects == 0);
  CHECK(fakes::rtos().delays == std::vector<uint32_t>{500, 500});
}

TEST_CASE("mqtt task: connects with LWT, announces online, subscribes the active valves") {
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
  CHECK(m.clientId == "VdMot");
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
  const std::string cmd = commandTopic(c, 0);
  REQUIRE(m.subscribed.size() == 2);
  CHECK(m.subscribed[0].first == cmd);
  CHECK(m.subscribed[1].first == cmd + "/set");
  CHECK(mqtt::status().state == vdm::MqttState::Connected);
  CHECK(mqtt::status().reconnects == 1);
  CHECK(sib::logger().has(vdm::EventCode::MqttConnected));
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

TEST_CASE("mqtt inbound: a target command of an active valve is submitted") {
  glue::begin();
  const vdm::Config& c = useMqtt();
  mqtt::begin();
  fakes::mqtt().inbox.push_back({commandTopic(c, 0), "55", false});
  runTask(2);
  REQUIRE(sib::app().submitted.size() == 1);
  const app::Command& cmd = sib::app().submitted[0];
  CHECK(cmd.type == app::CommandType::SetTarget);
  CHECK(cmd.valve == 0);
  CHECK(cmd.pos == 55);
  CHECK(cmd.source == vdm::TargetSource::Mqtt);
}

TEST_CASE("mqtt inbound: an inactive valve and a bad payload are rejected") {
  glue::begin();
  const vdm::Config& c = useMqtt();
  mqtt::begin();
  fakes::mqtt().inbox.push_back({commandTopic(c, 1), "10", false});
  fakes::mqtt().inbox.push_back({commandTopic(c, 0), "abc", false});
  runTask(3);
  CHECK(sib::app().submitted.empty());
  const std::vector<vdm::Event> ev = sib::logger().withCode(vdm::EventCode::MqttCommandRejected);
  REQUIRE(ev.size() == 2);
  CHECK(ev[0].arg1 == 2);
  CHECK(std::string(ev[0].text) == "inactive");
  CHECK(ev[1].arg1 == 1);
  CHECK(std::string(ev[1].text) == "payload");
  CHECK(mqtt::status().commandsRejected == 2);
}

TEST_CASE("mqtt task: a pending restart sends offline and disconnects cleanly") {
  glue::begin();
  const vdm::Config& c = useMqtt();
  mqtt::begin();
  runTask(1);
  REQUIRE(fakes::mqtt().connected);
  sib::ota().restartPending = true;
  runTask(2);
  const fakes::MqttMessage last = fakes::mqtt().published.back();
  CHECK(last.topic == topicOf(c, vdm::Topic::Status));
  CHECK(last.payload == "offline");
  CHECK(last.retained);
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
