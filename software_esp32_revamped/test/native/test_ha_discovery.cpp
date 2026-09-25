// Home Assistant discovery: legacy KEEP entities (object ids, names and
// unique_ids of the legacy firmware), the 2.1 entities with their
// availability, DROP deletions, the classification of list lines and the
// discovery run over a fake port.
#include <stdio.h>
#include <string.h>

#include <initializer_list>
#include <map>
#include <string>
#include <vector>

#include "doctest.h"
#include "vdm/event_log.h"
#include "vdm/ha_discovery.h"
#include "vdm/json_writer.h"

using namespace vdm;

namespace {

const char kDevice[] =
    "\"device\":{\"identifiers\":\"VdMot\",\"name\":\"VdMot\",\"sw_version\":\"2.1.0-revamped\","
    "\"model\":\"VdMot Revamped\",\"manufacturer\":\"Lenti84/Surfgargano\","
    "\"configuration_url\":\"http://192.168.1.50/\"}}";
const char kEsp[] = "\"availability\":[{\"topic\":\"VdMot/status\"}],";
const char kEspStm[] =
    "\"availability\":[{\"topic\":\"VdMot/status\"},{\"topic\":\"VdMot/stm/status\"}],"
    "\"availability_mode\":\"all\",";
const char kTemplate[] = "\"value_template\":\"{{ value | replace(',', '.') | float(None) }}\",";

void base(DiscoveryContext& c) {
  c = DiscoveryContext{};
  copyString(c.topics.station, sizeof c.topics.station, "VdMot");
  copyString(c.ip, sizeof c.ip, "192.168.1.50");
  copyString(c.swVersion, sizeof c.swVersion, "2.1.0-revamped");
}

void valve(DiscoveryContext& c, uint8_t i, const char* seg, bool t1 = false, bool t2 = false,
           const char* name = "") {
  c.valves[i].active = true;
  copyString(c.valves[i].segment, sizeof c.valves[i].segment, seg);
  copyString(c.valves[i].name, sizeof c.valves[i].name, name);
  c.valves[i].hasTemp1 = t1;
  c.valves[i].hasTemp2 = t2;
}

void temp(DiscoveryContext& c, uint8_t i, const char* seg, const char* id, bool published = true,
          const char* topicSeg = nullptr) {
  c.temps[i].active = true;
  c.temps[i].published = published;
  copyString(c.temps[i].segment, sizeof c.temps[i].segment, seg);
  copyString(c.temps[i].topicSegment, sizeof c.temps[i].topicSegment, topicSeg ? topicSeg : seg);
  copyString(c.temps[i].id, sizeof c.temps[i].id, id);
}

void volt(DiscoveryContext& c, uint8_t i, const char* seg, const char* name, const char* unit,
          const char* id) {
  c.volts[i].active = true;
  copyString(c.volts[i].segment, sizeof c.volts[i].segment, seg);
  copyString(c.volts[i].topicSegment, sizeof c.volts[i].topicSegment, seg);
  copyString(c.volts[i].name, sizeof c.volts[i].name, name);
  copyString(c.volts[i].unit, sizeof c.volts[i].unit, unit);
  copyString(c.volts[i].id, sizeof c.volts[i].id, id);
}

struct Msg {
  std::string topic;
  std::string json;
};

std::vector<Msg> all(const DiscoveryContext& c) {
  DiscoveryIterator it(c);
  std::vector<Msg> out;
  DiscoveryMessage m;
  static char buf[4096];
  JsonWriter jw(buf, sizeof buf);
  while (it.next(m, jw)) {
    CHECK_FALSE(m.remove);
    CHECK(jw.complete());
    out.push_back({m.topic, buf});
  }
  CHECK(jw.ok());  // ended because the list is exhausted
  return out;
}

std::vector<std::string> topics(const DiscoveryContext& c) {
  DiscoveryIterator it(c);
  std::vector<std::string> out;
  DiscoveryMessage m;
  while (it.nextTopic(m)) out.push_back(m.topic);
  CHECK(m.topic[0] == '\0');
  return out;
}

const Msg* find(const std::vector<Msg>& v, const std::string& topic) {
  for (const Msg& m : v) {
    if (m.topic == topic) return &m;
  }
  return nullptr;
}

std::string json(const std::vector<Msg>& v, const std::string& topic) {
  const Msg* m = find(v, topic);
  REQUIRE(m != nullptr);
  return m->json;
}

bool has(const std::string& s, const std::string& part) { return s.find(part) != std::string::npos; }

std::vector<std::string> drops(const DiscoveryContext& c) {
  DropListIterator it(c);
  std::vector<std::string> out;
  DiscoveryMessage m;
  while (it.next(m)) {
    CHECK(m.remove);
    out.push_back(m.topic);
  }
  return out;
}

TopicClass cls(const DiscoveryContext& c, const std::string& t) {
  return classifyDiscoveryTopic(c, t.data(), t.size());
}

const char* const kValveKinds[] = {
    "text/VdMot/valves_state_",          "valve/VdMot/valves_target_",
    "sensor/VdMot/valves_actual_",       "sensor/VdMot/valves_temp1_",
    "sensor/VdMot/valves_temp2_",        "text/VdMot/valves_calibration_date_",
    "text/VdMot/valves_calibration_repetitions_", "text/VdMot/valves_diag_openCount_",
    "text/VdMot/valves_diag_closeCount_", "text/VdMot/valves_diag_deadZoneCount_",
    "text/VdMot/valves_diag_moves_",     "text/VdMot/valves_diag_meanCurrrent_",
    "sensor/VdMot/diag_earlyStops_",     "sensor/VdMot/diag_cmdRejected_",
    "sensor/VdMot/diag_lastStop_",       "sensor/VdMot/diag_calState_",
    "binary_sensor/VdMot/valves_problem_", "sensor/VdMot/valves_failsafe_",
    "sensor/VdMot/valves_sync_",         "button/VdMot/valves_calibrate_",
};

std::vector<std::string> valveTopics(const std::string& seg, bool t1, bool t2) {
  std::vector<std::string> v;
  for (int k = 0; k < 20; ++k) {
    if ((k == 3 && !t1) || (k == 4 && !t2)) continue;
    v.push_back(std::string("homeassistant/") + kValveKinds[k] + seg + "/config");
  }
  return v;
}

const char* const kTail[] = {
    "binary_sensor/VdMot/diag_esp_online",   "binary_sensor/VdMot/diag_stm_online",
    "binary_sensor/VdMot/diag_failsafe",     "sensor/VdMot/diag_stm_lease",
    "binary_sensor/VdMot/diag_stm_safeMode", "sensor/VdMot/diag_stm_link",
    "sensor/VdMot/diag_stm_proto",           "sensor/VdMot/diag_stm_version",
    "sensor/VdMot/diag_stm_started",         "sensor/VdMot/diag_stm_resets",
    "sensor/VdMot/diag_stm_rxOverflow",      "sensor/VdMot/diag_stm_parseErr",
    "binary_sensor/VdMot/diag_calibration_active", "sensor/VdMot/diag_calibration_next",
    "sensor/VdMot/diag_mqtt_eventsSuppressed", "sensor/VdMot/diag_mqtt_commandsRejected",
    "event/VdMot/events",                    "button/VdMot/cmd_calibrate_all",
    "button/VdMot/cmd_detect",               "button/VdMot/cmd_stop",
    "button/VdMot/cmd_stm_reset",            "button/VdMot/cmd_esp_restart",
    "button/VdMot/cmd_stm_safe_exit",
};

std::string tail(int k) { return std::string("homeassistant/") + kTail[k] + "/config"; }

}  // namespace

TEST_CASE("component names") {
  CHECK(std::string(haComponentName(HaComponent::Sensor)) == "sensor");
  CHECK(std::string(haComponentName(HaComponent::BinarySensor)) == "binary_sensor");
  CHECK(std::string(haComponentName(HaComponent::Text)) == "text");
  CHECK(std::string(haComponentName(HaComponent::Valve)) == "valve");
  CHECK(std::string(haComponentName(HaComponent::Number)) == "number");
  CHECK(std::string(haComponentName(HaComponent::Select)) == "select");
  CHECK(std::string(haComponentName(HaComponent::Switch)) == "switch");
  CHECK(std::string(haComponentName(HaComponent::Climate)) == "climate");
  CHECK(std::string(haComponentName(HaComponent::Button)) == "button");
  CHECK(std::string(haComponentName(HaComponent::Event)) == "event");
  CHECK(std::string(haComponentName(static_cast<HaComponent>(10))) == "");
}

TEST_CASE("discovery: golden order") {
  static DiscoveryContext c;
  base(c);
  valve(c, 0, "Bad_1", true, false, "Bad 1");
  valve(c, 2, "3", false, true);
  temp(c, 0, "1", "28-84-37-94-97-ff-03-23");
  volt(c, 1, "Batt", "Batt", "V", "26-00-00-00-00-00-00-01");
  std::vector<std::string> expected = {
      "homeassistant/text/VdMot/state/config", "homeassistant/text/VdMot/message/config",
      "homeassistant/text/VdMot/uptime/config", "homeassistant/text/VdMot/ip/config"};
  for (const std::string& t : valveTopics("Bad_1", true, false)) expected.push_back(t);
  for (const std::string& t : valveTopics("3", false, true)) expected.push_back(t);
  expected.push_back("homeassistant/sensor/VdMot/temps_1/config");
  expected.push_back("homeassistant/sensor/VdMot/volts_Batt/config");
  for (int k = 0; k < 23; ++k) {
    if (k != 19 && k != 22) expected.push_back(tail(k));  // stop / safe exit need protocol 3
  }
  const std::vector<Msg> v = all(c);
  REQUIRE(v.size() == expected.size());
  CHECK(v.size() == 65);
  for (size_t i = 0; i < v.size(); ++i) CHECK(v[i].topic == expected[i]);
  CHECK(topics(c) == expected);
  // Every current topic classifies as Current.
  for (const std::string& t : expected) CHECK(cls(c, t) == TopicClass::Current);
  // With protocol 3 the two buttons follow.
  c.stmV3 = true;
  const std::vector<std::string> t3 = topics(c);
  REQUIRE(t3.size() == 67);
  CHECK(t3[63] == tail(19));
  CHECK(t3[66] == tail(22));
}

TEST_CASE("discovery: KEEP entities carry no availability (K3-1, K4-1, W3-8)") {
  static DiscoveryContext c;
  base(c);
  valve(c, 0, "Bad_1", true, false, "Bad 1");
  temp(c, 0, "1", "28-84-37-94-97-ff-03-23");
  volt(c, 1, "Batt", "Batt", "V", "26-00-00-00-00-00-00-01");
  const std::vector<Msg> v = all(c);
  const std::string dev = kDevice;
  CHECK(json(v, "homeassistant/text/VdMot/state/config") ==
        "{\"name\":\"state\",\"unique_id\":\"VdMot.common.state\",\"state_topic\":\"VdMot/common/state/value\","
        "\"command_topic\":\"VdMot/common/state/set\",\"icon\":\"mdi:state-machine\"," + dev);
  CHECK(json(v, "homeassistant/text/VdMot/ip/config") ==
        "{\"name\":\"ip\",\"unique_id\":\"VdMot.common.ip\",\"state_topic\":\"VdMot/common/ip/value\","
        "\"command_topic\":\"VdMot/common/ip/set\",\"icon\":\"mdi:message\"," + dev);
  CHECK(json(v, "homeassistant/text/VdMot/valves_state_Bad_1/config") ==
        "{\"name\":\"valves.Bad_1.state\",\"unique_id\":\"VdMot.valves.Bad_1.state\","
        "\"state_topic\":\"VdMot/valves/Bad_1/state/value\",\"command_topic\":\"VdMot/valves/Bad_1/state/set\","
        "\"icon\":\"mdi:state-machine\"," + dev);
  CHECK(json(v, "homeassistant/valve/VdMot/valves_target_Bad_1/config") ==
        "{\"name\":\"valves.Bad_1.target\",\"unique_id\":\"VdMot.valves.Bad_1.target\","
        "\"state_topic\":\"VdMot/valves/Bad_1/target/value\",\"command_topic\":\"VdMot/valves/Bad_1/target/set\","
        "\"icon\":\"mdi:valve\",\"device_class\":\"water\",\"reports_position\":true,\"qos\":1," + dev);
  CHECK(json(v, "homeassistant/sensor/VdMot/valves_temp1_Bad_1/config") ==
        "{\"name\":\"valves.Bad_1.temp1\",\"unique_id\":\"VdMot.valves.Bad_1.temp1\","
        "\"state_topic\":\"VdMot/valves/Bad_1/temp1/value\"," + std::string(kTemplate) +
        "\"icon\":\"mdi:thermometer\",\"device_class\":\"temperature\",\"state_class\":\"measurement\","
        "\"unit_of_measurement\":\"\xC2\xB0" "C\",\"expire_after\":60," + dev);
  CHECK(json(v, "homeassistant/text/VdMot/valves_calibration_date_Bad_1/config") ==
        "{\"name\":\"valves.Bad_1.calibration.date\",\"unique_id\":\"VdMot.valves.Bad_1.calibration.date\","
        "\"state_topic\":\"VdMot/valves/Bad_1/calibration/date/value\","
        "\"command_topic\":\"VdMot/valves/Bad_1/calibration/date/set\",\"icon\":\"mdi:timelapse\"," + dev);
  const char* diag[] = {"openCount", "closeCount", "deadZoneCount", "moves", "meanCurrrent"};
  for (const char* d : diag) {
    const std::string x = d;
    CHECK(json(v, "homeassistant/text/VdMot/valves_diag_" + x + "_Bad_1/config") ==
          "{\"name\":\"valves.Bad_1.diag." + x + "\",\"unique_id\":\"VdMot.valves.Bad_1.diag." + x +
              "\",\"state_topic\":\"VdMot/valves/Bad_1/diag/" + x +
              "/value\",\"command_topic\":\"VdMot/valves/Bad_1/diag/" + x + "/set\",\"icon\":\"mdi:valve\"," + dev);
  }
  CHECK(json(v, "homeassistant/sensor/VdMot/temps_1/config") ==
        "{\"name\":\"temps.1\",\"unique_id\":\"VdMot.28-84-37-94-97-ff-03-23\","
        "\"state_topic\":\"VdMot/temps/1/value/value\"," + std::string(kTemplate) +
        "\"icon\":\"mdi:thermometer\",\"device_class\":\"temperature\",\"state_class\":\"measurement\","
        "\"unit_of_measurement\":\"\xC2\xB0" "C\",\"expire_after\":60," + dev);
  CHECK(json(v, "homeassistant/sensor/VdMot/volts_Batt/config") ==
        "{\"name\":\"volts.Batt\",\"unique_id\":\"VdMot.26-00-00-00-00-00-00-01\","
        "\"state_topic\":\"VdMot/sensors/Batt/value/value\"," + std::string(kTemplate) +
        "\"device_class\":\"voltage\",\"state_class\":\"measurement\",\"unit_of_measurement\":\"V\","
        "\"expire_after\":60," + dev);
  // KEEP entities: no availability at all.
  for (const Msg& m : v) {
    const bool keep = has(m.topic, "/text/") || has(m.topic, "valves_target_") ||
                      has(m.topic, "valves_temp") || has(m.topic, "temps_") ||
                      has(m.topic, "volts_") || has(m.topic, "diag_esp_online");
    CAPTURE(m.topic);
    CHECK(has(m.json, "availability") == !keep);
    CHECK(has(m.json, dev));
  }
  // expire_after = max(3 x interval, 60).
  for (auto p : {std::pair<uint16_t, int>{30, 90}, {3600, 10800}, {2, 60}, {20, 60}, {21, 63}}) {
    c.publishIntervalS = p.first;
    const std::vector<Msg> w = all(c);
    CHECK(has(json(w, "homeassistant/sensor/VdMot/valves_temp1_Bad_1/config"),
              "\"expire_after\":" + std::to_string(p.second) + ","));
    CHECK(has(json(w, "homeassistant/sensor/VdMot/volts_Batt/config"),
              "\"expire_after\":" + std::to_string(p.second) + ","));
  }
  // payload_stop only with protocol 3.
  c.stmV3 = true;
  CHECK(has(json(all(c), "homeassistant/valve/VdMot/valves_target_Bad_1/config"),
            "\"reports_position\":true,\"qos\":1,\"payload_stop\":\"STOP\",\"device\""));
}

TEST_CASE("discovery: new per-valve entities (E29-1, W15-1)") {
  static DiscoveryContext c;
  base(c);
  valve(c, 0, "Bad_1", false, false, "Bad 1");
  valve(c, 1, "2");
  const std::vector<Msg> v = all(c);
  const std::string dev = kDevice;
  CHECK(json(v, "homeassistant/sensor/VdMot/valves_actual_Bad_1/config") ==
        "{\"name\":\"Bad 1 position\",\"unique_id\":\"VdMot.valves.Bad_1.actual\","
        "\"state_topic\":\"VdMot/valves/Bad_1/actual/value\",\"icon\":\"mdi:valve\","
        "\"state_class\":\"measurement\",\"unit_of_measurement\":\"%\"," + std::string(kEspStm) + dev);
  CHECK(json(v, "homeassistant/sensor/VdMot/diag_earlyStops_Bad_1/config") ==
        "{\"name\":\"Bad 1 early stops\",\"unique_id\":\"VdMot.diag.Bad_1.earlyStops\","
        "\"state_topic\":\"VdMot/diag/valves/Bad_1/earlyStops\",\"icon\":\"mdi:alert-outline\","
        "\"state_class\":\"total_increasing\",\"entity_category\":\"diagnostic\"," + std::string(kEspStm) + dev);
  CHECK(json(v, "homeassistant/sensor/VdMot/diag_cmdRejected_2/config") ==
        "{\"name\":\"Valve 2 rejected commands\",\"unique_id\":\"VdMot.diag.2.cmdRejected\","
        "\"state_topic\":\"VdMot/diag/valves/2/cmdRejected\",\"icon\":\"mdi:alert-outline\","
        "\"state_class\":\"total_increasing\",\"entity_category\":\"diagnostic\"," + std::string(kEspStm) + dev);
  CHECK(json(v, "homeassistant/sensor/VdMot/diag_lastStop_Bad_1/config") ==
        "{\"name\":\"Bad 1 last stop\",\"unique_id\":\"VdMot.diag.Bad_1.lastStop\","
        "\"state_topic\":\"VdMot/diag/valves/Bad_1/lastMove\",\"value_template\":\"{{ value_json.stop }}\","
        "\"icon\":\"mdi:stop-circle-outline\",\"device_class\":\"enum\",\"options\":[\"none\",\"target\","
        "\"endstop\",\"early_endstop\",\"timeout\",\"undercurrent\",\"safety_overcurrent\",\"aborted\"],"
        "\"entity_category\":\"diagnostic\"," + std::string(kEspStm) + dev);
  CHECK(json(v, "homeassistant/sensor/VdMot/diag_calState_Bad_1/config") ==
        "{\"name\":\"Bad 1 calibration state\",\"unique_id\":\"VdMot.diag.Bad_1.calState\","
        "\"state_topic\":\"VdMot/diag/valves/Bad_1/calState\",\"icon\":\"mdi:progress-wrench\","
        "\"entity_category\":\"diagnostic\"," + std::string(kEspStm) + dev);
  CHECK(json(v, "homeassistant/binary_sensor/VdMot/valves_problem_Bad_1/config") ==
        "{\"name\":\"Bad 1 problem\",\"unique_id\":\"VdMot.valves.Bad_1.problem\","
        "\"state_topic\":\"VdMot/valves/Bad_1/problem/value\",\"device_class\":\"problem\","
        "\"payload_on\":\"1\",\"payload_off\":\"0\"," + std::string(kEsp) + dev);
  CHECK(json(v, "homeassistant/sensor/VdMot/valves_failsafe_Bad_1/config") ==
        "{\"name\":\"Bad 1 failsafe\",\"unique_id\":\"VdMot.valves.Bad_1.failsafe\","
        "\"state_topic\":\"VdMot/valves/Bad_1/failsafe/value\",\"icon\":\"mdi:shield-alert-outline\","
        "\"device_class\":\"enum\",\"options\":[\"off\",\"lease\",\"blocked\"]," + std::string(kEspStm) + dev);
  CHECK(json(v, "homeassistant/sensor/VdMot/valves_sync_Bad_1/config") ==
        "{\"name\":\"Bad 1 target delivery\",\"unique_id\":\"VdMot.valves.Bad_1.sync\","
        "\"state_topic\":\"VdMot/valves/Bad_1/sync/value\",\"device_class\":\"enum\",\"options\":[\"unknown\","
        "\"synced\",\"pending\",\"await_ack\",\"await_verify\",\"failed\"],\"entity_category\":\"diagnostic\"," +
        std::string(kEsp) + dev);
  CHECK(json(v, "homeassistant/button/VdMot/valves_calibrate_Bad_1/config") ==
        "{\"name\":\"Bad 1 calibrate\",\"unique_id\":\"VdMot.valves.Bad_1.calibrate\","
        "\"command_topic\":\"VdMot/cmd/valves/Bad_1/calibrate\",\"icon\":\"mdi:tune-vertical\","
        "\"entity_category\":\"config\"," + std::string(kEspStm) + dev);
  // Not separate: the suffix goes, the buttons stay.
  c.topics.separate = false;
  const std::vector<Msg> p = all(c);
  CHECK(has(json(p, "homeassistant/binary_sensor/VdMot/valves_problem_2/config"),
            "\"state_topic\":\"VdMot/valves/2/problem\","));
  CHECK(has(json(p, "homeassistant/valve/VdMot/valves_target_2/config"),
            "\"state_topic\":\"VdMot/valves/2/target\",\"command_topic\":\"VdMot/valves/2/target\","));
}

TEST_CASE("discovery: device entities (K3-2, E29-1, W15-2)") {
  static DiscoveryContext c;
  base(c);
  c.stmV3 = true;
  const std::vector<Msg> v = all(c);
  REQUIRE(v.size() == 4 + 23);
  const std::string dev = kDevice;
  const std::string e = kEsp;
  const std::string es = kEspStm;
  CHECK(json(v, tail(0)) ==
        "{\"name\":\"ESP online\",\"unique_id\":\"VdMot.diag.esp.online\",\"state_topic\":\"VdMot/status\","
        "\"device_class\":\"connectivity\",\"payload_on\":\"online\",\"payload_off\":\"offline\","
        "\"entity_category\":\"diagnostic\"," + dev);
  CHECK(json(v, tail(1)) ==
        "{\"name\":\"STM link\",\"unique_id\":\"VdMot.diag.stm.online\",\"state_topic\":\"VdMot/stm/status\","
        "\"device_class\":\"connectivity\",\"payload_on\":\"online\",\"payload_off\":\"offline\","
        "\"entity_category\":\"diagnostic\"," + e + dev);
  CHECK(json(v, tail(2)) ==
        "{\"name\":\"Failsafe active\",\"unique_id\":\"VdMot.diag.failsafe\",\"state_topic\":\"VdMot/failsafe\","
        "\"device_class\":\"problem\",\"payload_on\":\"1\",\"payload_off\":\"0\"," + es + dev);
  CHECK(json(v, tail(3)) ==
        "{\"name\":\"Lease\",\"unique_id\":\"VdMot.diag.stm.lease\",\"state_topic\":\"VdMot/diag/stm/lease\","
        "\"device_class\":\"enum\",\"options\":[\"off\",\"running\",\"expired\"],"
        "\"entity_category\":\"diagnostic\"," + es + dev);
  CHECK(json(v, tail(4)) ==
        "{\"name\":\"STM safe mode\",\"unique_id\":\"VdMot.diag.stm.safeMode\","
        "\"state_topic\":\"VdMot/diag/stm/safeMode\",\"device_class\":\"problem\",\"payload_on\":\"1\","
        "\"payload_off\":\"0\",\"entity_category\":\"diagnostic\"," + es + dev);
  CHECK(json(v, tail(5)) ==
        "{\"name\":\"STM link state\",\"unique_id\":\"VdMot.diag.stm.link\",\"state_topic\":\"VdMot/diag/stm/link\","
        "\"icon\":\"mdi:lan-connect\",\"device_class\":\"enum\",\"options\":[\"unknown\",\"up\",\"degraded\","
        "\"down\",\"booting\",\"suspended\"],\"entity_category\":\"diagnostic\"," + e + dev);
  CHECK(json(v, tail(6)) ==
        "{\"name\":\"STM protocol\",\"unique_id\":\"VdMot.diag.stm.proto\",\"state_topic\":\"VdMot/diag/stm/proto\","
        "\"entity_category\":\"diagnostic\"," + es + dev);
  CHECK(json(v, tail(7)) ==
        "{\"name\":\"STM firmware\",\"unique_id\":\"VdMot.diag.stm.version\","
        "\"state_topic\":\"VdMot/diag/stm/version\",\"icon\":\"mdi:chip\",\"entity_category\":\"diagnostic\"," +
        es + dev);
  CHECK(json(v, tail(8)) ==
        "{\"name\":\"STM started\",\"unique_id\":\"VdMot.diag.stm.started\","
        "\"state_topic\":\"VdMot/diag/stm/started\",\"device_class\":\"timestamp\","
        "\"entity_category\":\"diagnostic\"," + es + dev);
  const char* counters[3][3] = {{"resets", "STM resets", ""},
                                {"rxOverflow", "STM receive overflows", ""},
                                {"parseErr", "STM parse errors", ""}};
  for (int k = 0; k < 3; ++k) {
    const std::string key = counters[k][0];
    CHECK(json(v, tail(9 + k)) ==
          "{\"name\":\"" + std::string(counters[k][1]) + "\",\"unique_id\":\"VdMot.diag.stm." + key +
              "\",\"state_topic\":\"VdMot/diag/stm/" + key +
              "\",\"state_class\":\"total_increasing\",\"entity_category\":\"diagnostic\"," + es + dev);
  }
  CHECK(json(v, tail(12)) ==
        "{\"name\":\"Calibration running\",\"unique_id\":\"VdMot.diag.calibration.active\","
        "\"state_topic\":\"VdMot/diag/calibration/active\",\"device_class\":\"running\",\"payload_on\":\"1\","
        "\"payload_off\":\"0\",\"entity_category\":\"diagnostic\"," + es + dev);
  CHECK(json(v, tail(13)) ==
        "{\"name\":\"Next calibration\",\"unique_id\":\"VdMot.diag.calibration.next\","
        "\"state_topic\":\"VdMot/diag/calibration/next\",\"device_class\":\"timestamp\"," + e + dev);
  CHECK(json(v, tail(14)) ==
        "{\"name\":\"Suppressed events\",\"unique_id\":\"VdMot.diag.mqtt.eventsSuppressed\","
        "\"state_topic\":\"VdMot/diag/mqtt/eventsSuppressed\",\"state_class\":\"total_increasing\","
        "\"entity_category\":\"diagnostic\"," + e + dev);
  CHECK(json(v, tail(15)) ==
        "{\"name\":\"Rejected MQTT commands\",\"unique_id\":\"VdMot.diag.mqtt.commandsRejected\","
        "\"state_topic\":\"VdMot/diag/mqtt/commandsRejected\",\"state_class\":\"total_increasing\","
        "\"entity_category\":\"diagnostic\"," + e + dev);
  const char* names[128];
  const size_t n = eventMqttNames(names, 128);
  REQUIRE(n > 30);
  REQUIRE(n <= 128);
  std::string types;
  for (size_t i = 0; i < n; ++i) types += std::string(i ? "," : "") + "\"" + names[i] + "\"";
  CHECK(json(v, tail(16)) ==
        "{\"name\":\"Events\",\"unique_id\":\"VdMot.events\",\"state_topic\":\"VdMot/events\","
        "\"icon\":\"mdi:bell-alert-outline\",\"event_types\":[" + types + "]," + e + dev);
  const char* buttons[6][4] = {{"cmd_calibrate_all", "Calibrate all valves", "cmd.calibrate", "calibrate"},
                               {"cmd_detect", "Detect valves", "cmd.detect", "detect"},
                               {"cmd_stop", "Stop valves", "cmd.stop", "stop"},
                               {"cmd_stm_reset", "Reset STM", "cmd.stmReset", "stmReset"},
                               {"cmd_esp_restart", "Restart ESP", "cmd.restart", "restart"},
                               {"cmd_stm_safe_exit", "Leave STM safe mode", "cmd.stmSafeExit", "stmSafeExit"}};
  for (int k = 0; k < 6; ++k) {
    const bool restart = k == 3 || k == 4;
    CAPTURE(k);
    CHECK(json(v, tail(17 + k)) ==
          "{\"name\":\"" + std::string(buttons[k][1]) + "\",\"unique_id\":\"VdMot." + buttons[k][2] +
              "\",\"command_topic\":\"VdMot/cmd/" + buttons[k][3] + "\"," +
              (restart ? "\"device_class\":\"restart\"," : "") + "\"entity_category\":\"config\"," +
              (restart ? e : es) + dev);
  }
  // Gates: newDiag and events.
  c.newDiag = false;
  std::vector<std::string> t = topics(c);
  CHECK(t.size() == 4 + 3 + 1 + 6);
  c.events = false;
  t = topics(c);
  CHECK(t.size() == 4 + 3 + 6);
  CHECK(t[7] == tail(17));
}

TEST_CASE("discovery gates and counts") {
  static DiscoveryContext c;
  base(c);
  valve(c, 1, "2", true, true);
  CHECK(topics(c).size() == 4 + 20 + 21);
  c.publishUptime = false;
  CHECK(topics(c).size() == 3 + 20 + 21);
  c.publishDiag = false;
  CHECK(topics(c).size() == 3 + 15 + 21);
  c.newDiag = false;
  std::vector<std::string> t = topics(c);
  CHECK(t.size() == 3 + 11 + 8);
  // valves_actual stays without newDiag (regulator feedback).
  CHECK(t[5] == "homeassistant/sensor/VdMot/valves_actual_2/config");
  c.valves[1].hasTemp1 = false;
  CHECK(topics(c).size() == 3 + 10 + 8);
  c.valves[1].hasTemp2 = false;
  CHECK(topics(c).size() == 3 + 9 + 8);
  c.valves[1].active = false;
  CHECK(topics(c).size() == 3 + 8);
  c.plainText = false;
  c.publishAllTemps = false;
  CHECK(topics(c).size() == 3 + 8);

  // Sensors.
  base(c);
  c.newDiag = false;
  c.events = false;
  temp(c, 0, "1", "28-00-00-00-00-00-00-01");
  temp(c, 5, "Living", "28-00-00-00-00-00-00-02", false);  // not published
  temp(c, 6, "7", "");                                       // no id
  temp(c, 33, "34", "28-00-00-00-00-00-00-03");
  c.temps[33].active = false;
  temp(c, 32, "33", "28-00-00-00-00-00-00-04");
  temp(c, 20, "21", "x");  // any non-empty id counts
  volt(c, 0, "1", "", "mV", "26-00-00-00-00-00-00-05");
  volt(c, 1, "Cur", "Cur", "A", "26-00-00-00-00-00-00-06");
  volt(c, 2, "x", "x", "", "26-00-00-00-00-00-00-07");
  volt(c, 3, "My_V", "My V", "V", "26-00-00-00-00-00-00-08");
  volt(c, 4, "5", "", "V", "");  // no id
  volt(c, 7, "8", "", "V", "26-00-00-00-00-00-00-09");
  c.volts[7].active = false;
  std::vector<Msg> v = all(c);
  REQUIRE(v.size() == 4 + 3 + 4 + 7);
  CHECK(v[4].topic == "homeassistant/sensor/VdMot/temps_1/config");
  CHECK(v[5].topic == "homeassistant/sensor/VdMot/temps_21/config");
  CHECK(has(v[5].json, "\"unique_id\":\"VdMot.x\""));
  CHECK(v[6].topic == "homeassistant/sensor/VdMot/temps_33/config");
  CHECK(has(v[6].json, "\"unique_id\":\"VdMot.28-00-00-00-00-00-00-04\""));
  CHECK(v[7].topic == "homeassistant/sensor/VdMot/volts_1/config");
  CHECK(has(v[7].json, "\"name\":\"volts.\","));  // legacy: raw (empty) name
  CHECK(has(v[7].json, "\"device_class\":\"voltage\""));
  CHECK(has(v[7].json, "\"unit_of_measurement\":\"mV\""));
  CHECK_FALSE(has(v[8].json, "device_class"));
  CHECK(has(v[8].json, "\"unit_of_measurement\":\"A\""));
  CHECK_FALSE(has(v[9].json, "unit_of_measurement"));
  CHECK_FALSE(has(v[9].json, "device_class"));
  CHECK(v[10].topic == "homeassistant/sensor/VdMot/volts_My_V/config");
  CHECK(has(v[10].json, "\"name\":\"volts.My V\""));
  CHECK(has(v[10].json, "\"state_topic\":\"VdMot/sensors/My_V/value/value\""));
  copyString(c.volts[2].id, sizeof c.volts[2].id, "z");
  v = all(c);
  CHECK(has(v[9].json, "\"unique_id\":\"VdMot.z\""));
}

TEST_CASE("discovery: E22 published segment and topicKnown") {
  static DiscoveryContext c;
  base(c);
  temp(c, 0, "1", "28-00-00-00-00-00-00-01", true, "5");  // slot 1 at bus index 4
  std::vector<Msg> v = all(c);
  CHECK(has(json(v, "homeassistant/sensor/VdMot/temps_1/config"),
            "\"state_topic\":\"VdMot/temps/5/value/value\""));
  CHECK(cls(c, "homeassistant/sensor/VdMot/temps_1/config") == TopicClass::Current);
  c.temps[0].topicKnown = false;  // not on the bus
  v = all(c);
  CHECK(find(v, "homeassistant/sensor/VdMot/temps_1/config") == nullptr);
  CHECK(cls(c, "homeassistant/sensor/VdMot/temps_1/config") == TopicClass::KeptUnknown);
  volt(c, 2, "3", "", "V", "26-00-00-00-00-00-00-01");
  c.volts[2].topicKnown = false;
  CHECK(topics(c).size() == 4 + 21);
  CHECK(cls(c, "homeassistant/sensor/VdMot/volts_3/config") == TopicClass::KeptUnknown);
  c.temps[0].topicKnown = true;
  c.temps[0].topicSegment[0] = '\0';  // known but unusable: an error, skipped
  DiscoveryIterator it(c);
  DiscoveryMessage m;
  static char buf[4096];
  JsonWriter jw(buf, sizeof buf);
  for (int i = 0; i < 4; ++i) REQUIRE(it.next(m, jw));
  CHECK_FALSE(it.next(m, jw));
  CHECK_FALSE(jw.ok());
  CHECK(m.topic[0] == '\0');
}

TEST_CASE("discovery: HA-safe ids, raw names, root and prefix (E20-2, H5)") {
  static DiscoveryContext c;
  base(c);
  c.newDiag = false;
  c.events = false;
  copyString(c.topics.station, sizeof c.topics.station, "Dom 1");
  valve(c, 0, "\xc5\x81" "azienka", false, false, "\xc5\x81" "azienka");
  std::vector<Msg> v = all(c);
  const Msg* m = find(v, "homeassistant/text/Dom_1/valves_state_Lazienka/config");
  REQUIRE(m != nullptr);
  CHECK(has(m->json, "\"name\":\"valves.\xc5\x81" "azienka.state\",\"unique_id\":\"Dom_1.valves.\xc5\x81"
                     "azienka.state\""));
  CHECK(has(m->json, "\"state_topic\":\"Dom 1/valves/\xc5\x81" "azienka/state/value\""));
  CHECK(has(m->json, "\"identifiers\":\"Dom 1\",\"name\":\"Dom 1\""));
  CHECK(has(json(v, "homeassistant/sensor/Dom_1/valves_actual_Lazienka/config"),
            "\"name\":\"\xc5\x81" "azienka position\""));
  copyString(c.discoveryPrefix, sizeof c.discoveryPrefix, "ha");
  CHECK(topics(c)[4] == "ha/text/Dom_1/valves_state_Lazienka/config");
  c.discoveryPrefix[0] = '\0';  // empty: the legacy prefix
  CHECK(topics(c)[4] == "homeassistant/text/Dom_1/valves_state_Lazienka/config");
  for (const char* bad : {"a b", "/ha", "ha/", "a//b", "h+", "h#", "\xc3\xa4", "/"}) {
    CAPTURE(bad);
    copyString(c.discoveryPrefix, sizeof c.discoveryPrefix, bad);
    CHECK(topics(c)[0] == "homeassistant/text/Dom_1/state/config");
  }
  for (const char* ok : {"a/b-c_D/9", "Z", "-"}) {
    copyString(c.discoveryPrefix, sizeof c.discoveryPrefix, ok);
    CHECK(topics(c)[0] == std::string(ok) + "/text/Dom_1/state/config");
  }
  memset(c.discoveryPrefix, 'p', sizeof c.discoveryPrefix);  // unterminated: 32 chars
  CHECK(topics(c)[0] == std::string(32, 'p') + "/text/Dom_1/state/config");

  // Legacy override "Bad/WC": topics with the '/', unique_id raw, object id safe.
  base(c);
  c.newDiag = false;
  c.events = false;
  valve(c, 2, "Bad/WC");
  v = all(c);
  m = find(v, "homeassistant/text/VdMot/valves_state_Bad_WC/config");
  REQUIRE(m != nullptr);
  CHECK(has(m->json, "\"unique_id\":\"VdMot.valves.Bad/WC.state\",\"state_topic\":\"VdMot/valves/Bad/WC/state/value\""));
  CHECK(has(json(v, "homeassistant/valve/VdMot/valves_target_Bad_WC/config"),
            "\"command_topic\":\"VdMot/valves/Bad/WC/target/set\""));
  CHECK(has(json(v, "homeassistant/button/VdMot/valves_calibrate_Bad_WC/config"),
            "\"command_topic\":\"VdMot/cmd/valves/Bad/WC/calibrate\""));
  // Root VdMotFBH with the station VdMot: topics under the root, ids under the station.
  copyString(c.station, sizeof c.station, "VdMot");
  copyString(c.topics.station, sizeof c.topics.station, "VdMotFBH");
  v = all(c);
  m = find(v, "homeassistant/text/VdMot/valves_state_Bad_WC/config");
  REQUIRE(m != nullptr);
  CHECK(has(m->json, "\"unique_id\":\"VdMot.valves.Bad/WC.state\",\"state_topic\":\"VdMotFBH/valves/Bad/WC/state/value\""));
  CHECK(has(json(v, "homeassistant/binary_sensor/VdMot/diag_esp_online/config"),
            "\"state_topic\":\"VdMotFBH/status\""));
  CHECK(has(json(v, "homeassistant/binary_sensor/VdMot/diag_stm_online/config"),
            "\"availability\":[{\"topic\":\"VdMotFBH/status\"}]"));
  CHECK(has(m->json, "\"identifiers\":\"VdMot\""));
}

TEST_CASE("discovery: v20Topic, the 2.0.0 form of changed ids (E20-3)") {
  static DiscoveryContext c;
  base(c);
  copyString(c.topics.station, sizeof c.topics.station, "Dom 1");
  valve(c, 0, "\xc5\x81" "azienka");
  valve(c, 1, "Bad_1");
  copyString(c.discoveryPrefix, sizeof c.discoveryPrefix, "homeassistant");
  DiscoveryIterator it(c);
  DiscoveryMessage m;
  DiscoveryMessage old;
  CHECK_FALSE(it.v20Topic(old));  // nothing produced yet
  std::map<std::string, std::string> olds;
  while (it.nextTopic(m)) {
    if (it.v20Topic(old)) olds[m.topic] = old.topic;
  }
  CHECK(olds["homeassistant/text/Dom_1/valves_state_Lazienka/config"] ==
        "homeassistant/text/Dom 1/valves_state_\xc5\x81" "azienka/config");
  CHECK(olds["homeassistant/text/Dom_1/state/config"] == "homeassistant/text/Dom 1/state/config");
  CHECK(olds["homeassistant/binary_sensor/Dom_1/diag_esp_online/config"] ==
        "homeassistant/binary_sensor/Dom 1/diag_esp_online/config");
  // The same with a safe station and segment: nothing differs.
  base(c);
  valve(c, 1, "Bad_1");
  DiscoveryIterator same(c);
  int produced = 0;
  while (same.nextTopic(m)) {
    ++produced;
    CHECK_FALSE(same.v20Topic(old));
    CHECK(old.topic[0] == '\0');
  }
  CHECK(produced > 20);
  // Another prefix: the legacy prefix differs.
  copyString(c.discoveryPrefix, sizeof c.discoveryPrefix, "ha");
  DiscoveryIterator pre(c);
  REQUIRE(pre.nextTopic(m));
  CHECK(std::string(m.topic) == "ha/text/VdMot/state/config");
  REQUIRE(pre.v20Topic(old));
  CHECK(std::string(old.topic) == "homeassistant/text/VdMot/state/config");
}

TEST_CASE("discovery: device block with hw_version and variants (W15-4)") {
  static DiscoveryContext c;
  base(c);
  copyString(c.hwVersion, sizeof c.hwVersion, "C2");
  std::vector<Msg> v = all(c);
  CHECK(has(v[0].json, "\"sw_version\":\"2.1.0-revamped\",\"hw_version\":\"C2\",\"model\""));
  c.hwVersion[0] = '\0';
  v = all(c);
  CHECK_FALSE(has(v[0].json, "hw_version"));
  memset(c.hwVersion, 'C', sizeof c.hwVersion);  // unterminated: bounded
  v = all(c);
  CHECK(has(v[0].json, "\"hw_version\":\"CCC\""));
  c.hwVersion[0] = '\0';
  c.topics.pathAsRoot = true;
  v = all(c);
  CHECK(has(v[0].json, "\"state_topic\":\"/VdMot/common/state/value\""));
  CHECK(has(v[0].json, "\"command_topic\":\"/VdMot/common/state/set\""));
  CHECK(has(json(v, tail(1)), "\"availability\":[{\"topic\":\"/VdMot/status\"}]"));
  c.topics.pathAsRoot = false;
  c.ip[0] = '\0';
  copyString(c.swVersion, sizeof c.swVersion, "2.0\"x\\");
  v = all(c);
  CHECK_FALSE(has(v[0].json, "configuration_url"));
  CHECK(has(v[0].json, "\"sw_version\":\"2.0\\\"x\\\\\""));
  copyString(c.ip, sizeof c.ip, "1");
  v = all(c);
  CHECK(has(v[0].json, "\"configuration_url\":\"http://1/\""));
  memset(c.ip, '1', sizeof c.ip);
  memset(c.swVersion, 'v', sizeof c.swVersion);
  v = all(c);
  CHECK(has(v[0].json, "\"configuration_url\":\"http://" + std::string(15, '1') + "/\""));
  CHECK(has(v[0].json, "\"sw_version\":\"" + std::string(31, 'v') + "\""));
  // A station with a space: raw in names and device, '_' in unique_ids and node ids.
  base(c);
  copyString(c.topics.station, sizeof c.topics.station, "My Station");
  v = all(c);
  CHECK(v[0].topic == "homeassistant/text/My_Station/state/config");
  CHECK(has(v[0].json, "\"unique_id\":\"My_Station.common.state\""));
  CHECK(has(v[0].json, "\"state_topic\":\"My Station/common/state/value\""));
  CHECK(has(v[0].json, "\"identifiers\":\"My Station\",\"name\":\"My Station\""));
}

TEST_CASE("discovery refuses a missing or unsafe station") {
  static DiscoveryContext c;
  base(c);
  valve(c, 0, "1");
  for (const char* bad : {"", "a+b", "a/b", "x\xc3"}) {
    copyString(c.topics.station, sizeof c.topics.station, bad);
    DiscoveryIterator it(c);
    DiscoveryMessage m;
    static char buf[4096];
    JsonWriter jw(buf, sizeof buf);
    CHECK_FALSE(it.next(m, jw));
    CHECK(jw.ok());
    CHECK(m.topic[0] == '\0');
    DiscoveryIterator t(c);
    CHECK_FALSE(t.nextTopic(m));
    DropListIterator d(c);
    CHECK_FALSE(d.next(m));
    CHECK(cls(c, "homeassistant/text/x/state/config") == TopicClass::Stale);
  }
  memset(c.topics.station, 'x', sizeof c.topics.station);  // unterminated
  DiscoveryIterator it(c);
  DiscoveryMessage m;
  static char buf[4096];
  JsonWriter jw(buf, sizeof buf);
  CHECK_FALSE(it.next(m, jw));
  CHECK(jw.ok());  // off, not a failed entity
  // The longest valid station works; the explicit station wins over the root.
  memset(c.topics.station, 'x', kStationNameMax);
  c.topics.station[kStationNameMax] = '\0';
  DiscoveryIterator ok(c);
  REQUIRE(ok.next(m, jw));
  CHECK(std::string(m.topic) == "homeassistant/text/" + std::string(kStationNameMax, 'x') + "/state/config");
  copyString(c.station, sizeof c.station, "St");
  DiscoveryIterator st(c);
  REQUIRE(st.next(m, jw));
  CHECK(std::string(m.topic) == "homeassistant/text/St/state/config");
  copyString(c.station, sizeof c.station, "a+b");  // an unsafe station is not replaced by the root
  DiscoveryIterator bad(c);
  CHECK_FALSE(bad.next(m, jw));
  CHECK(jw.ok());
}

TEST_CASE("discovery skips an entity that cannot be built and continues") {
  static DiscoveryContext c;
  base(c);
  c.newDiag = false;
  c.publishDiag = false;
  c.publishUptime = false;
  c.events = false;
  valve(c, 0, "a//b");  // invalid segment
  valve(c, 1, "2");
  DiscoveryIterator it(c);
  DiscoveryMessage m;
  static char buf[4096];
  JsonWriter jw(buf, sizeof buf);
  for (int i = 0; i < 3; ++i) REQUIRE(it.next(m, jw));
  CHECK(it.position() == 4);
  // Valve 0: state, target, actual, calibration x2, problem, failsafe, sync, calibrate.
  for (int i = 0; i < 9; ++i) {
    CHECK_FALSE(it.next(m, jw));
    CHECK_FALSE(jw.ok());
    CHECK(m.topic[0] == '\0');
  }
  REQUIRE(it.next(m, jw));
  CHECK(jw.ok());
  CHECK(std::string(m.topic) == "homeassistant/text/VdMot/valves_state_2/config");
  int rest = 0;
  while (it.next(m, jw)) ++rest;
  CHECK(rest == 8 + 7);
  CHECK(jw.ok());
  CHECK(it.position() == 309);
  CHECK_FALSE(it.next(m, jw));
  // nextTopic passes unbuildable entities over.
  CHECK(topics(c).size() == 3 + 9 + 7);
  // Unterminated / oversize and quoted segments are rejected as well.
  for (const char* bad : {"a b", "a\"b", "a\\b", "+", "#"}) {
    copyString(c.valves[0].segment, sizeof c.valves[0].segment, bad);
    CHECK(topics(c).size() == 3 + 9 + 7);
  }
  memset(c.valves[0].segment, 'x', sizeof c.valves[0].segment);
  CHECK(topics(c).size() == 3 + 9 + 7);
  // A segment that buildHaId() cannot turn into an id (none survives).
  copyString(c.valves[0].segment, sizeof c.valves[0].segment, "");
  CHECK(topics(c).size() == 3 + 9 + 7);
  // A writer too small for a payload: failure with the topic, then the iterator moves on.
  copyString(c.valves[0].segment, sizeof c.valves[0].segment, "1");
  it.restart();
  CHECK(it.position() == 0);
  char small[200];
  JsonWriter sw(small, sizeof small);
  CHECK_FALSE(it.next(m, sw));
  CHECK_FALSE(sw.ok());
  CHECK(std::string(m.topic) == "homeassistant/text/VdMot/state/config");
  CHECK(it.position() == 1);
}

TEST_CASE("discovery with every entity enabled fits the MQTT buffer (W15-2)") {
  static DiscoveryContext c;
  base(c);
  copyString(c.topics.station, sizeof c.topics.station, "abcdefghijklmnopqrst");
  c.topics.pathAsRoot = true;
  c.stmV3 = true;
  copyString(c.hwVersion, sizeof c.hwVersion, "C99");
  copyString(c.discoveryPrefix, sizeof c.discoveryPrefix, "abcdefghijklmnopqrstuvwxyz012345");
  copyString(c.ip, sizeof c.ip, "255.255.255.255");
  copyString(c.swVersion, sizeof c.swVersion, "2.1.0-revamped-dev-0123456789ab");
  for (uint8_t i = 0; i < kValveCount; ++i) {
    char seg[kSegmentMax + 1];
    snprintf(seg, sizeof seg, "Valve_%04u", static_cast<unsigned>(i));
    valve(c, i, seg, true, true, "\xc5\x81\xc5\x81\xc5\x81\xc5\x81\xc5\x81");
  }
  for (uint8_t i = 0; i < kTempSlotCount; ++i) {
    char seg[kSegmentMax + 1];
    snprintf(seg, sizeof seg, "Temp__%04u", static_cast<unsigned>(i));
    temp(c, i, seg, "28-84-37-94-97-ff-03-23");
  }
  for (uint8_t i = 0; i < kVoltSlotCount; ++i) {
    char seg[kSegmentMax + 1];
    snprintf(seg, sizeof seg, "Volt__%04u", static_cast<unsigned>(i));
    volt(c, i, seg, "Volt  0000", "mV", "26-84-37-94-97-ff-03-23");
  }
  const std::vector<Msg> v = all(c);
  CHECK(v.size() == 309);
  size_t longest = 0;
  for (const Msg& m : v) {
    if (m.json.size() > longest) longest = m.json.size();
    CHECK(m.topic.size() <= kDiscoveryTopicMax);
    CHECK(cls(c, m.topic) == TopicClass::Current);
  }
  CHECK(longest <= kDiscoveryPayloadMax);
  CHECK(json(v, "abcdefghijklmnopqrstuvwxyz012345/event/abcdefghijklmnopqrst/events/config").size() == longest);
  MESSAGE("longest discovery payload: " << longest);
}

TEST_CASE("DROP list: legacy entities of both segment forms, once each") {
  static DiscoveryContext c;
  base(c);
  valve(c, 0, "Bad_1");
  c.valves[0].active = false;  // inactive valves are cleaned up too
  copyString(c.valves[2].segment, sizeof c.valves[2].segment, "3");    // unnamed
  copyString(c.valves[4].segment, sizeof c.valves[4].segment, "2");    // named like valve 2's number
  copyString(c.valves[5].segment, sizeof c.valves[5].segment, "a//b");  // invalid: index form only
  copyString(c.valves[6].segment, sizeof c.valves[6].segment, "a/b");  // override: index form only
  copyString(c.discoveryPrefix, sizeof c.discoveryPrefix, "ha");       // the literal legacy prefix
  const std::vector<std::string> d = drops(c);
  const char* kinds[7][2] = {
      {"climate", "climate_"},
      {"number", "valves_control_dynOffs_"},
      {"number", "valves_control_min_"},
      {"number", "valves_control_max_"},
      {"select", "valves_window_state_"},
      {"switch", "valves_window_state_"},
      {"number", "valves_window_target_"},
  };
  std::vector<std::string> expected = {"homeassistant/select/VdMot/heatControl/config",
                                       "homeassistant/number/VdMot/parkPosition/config"};
  auto add = [&](const std::string& seg) {
    for (auto& k : kinds) {
      expected.push_back(std::string("homeassistant/") + k[0] + "/VdMot/" + k[1] + seg + "/config");
    }
  };
  for (uint8_t i = 0; i < kValveCount; ++i) {
    const std::string number = std::to_string(i + 1);
    if (i == 0) add("Bad_1");
    if (i == 4) add("2");
    add(number);
  }
  REQUIRE(d.size() == expected.size());
  CHECK(d.size() == 2 + 7 * 14);
  for (size_t i = 0; i < d.size(); ++i) CHECK(d[i] == expected[i]);

  DropListIterator it(c);
  DiscoveryMessage m;
  REQUIRE(it.next(m));
  it.restart();
  REQUIRE(it.next(m));
  CHECK(std::string(m.topic) == expected[0]);
  CHECK(m.remove);
  // Station with a space stays raw in the topic.
  copyString(c.topics.station, sizeof c.topics.station, "My St");
  DropListIterator s(c);
  REQUIRE(s.next(m));
  CHECK(std::string(m.topic) == "homeassistant/select/My St/heatControl/config");
  // Worst case lengths fit.
  copyString(c.topics.station, sizeof c.topics.station, "abcdefghijklmnopqrst");
  for (uint8_t i = 0; i < kValveCount; ++i) {
    copyString(c.valves[i].segment, sizeof c.valves[i].segment, "0123456789");
  }
  const std::vector<std::string> worst = drops(c);
  CHECK(worst.size() == 2 + 7 * 24);
  for (const std::string& t : worst) CHECK(t.size() <= kDiscoveryTopicMax);
  // reset() binds another context.
  static DiscoveryContext o;
  base(o);
  copyString(o.topics.station, sizeof o.topics.station, "Other");
  it.reset(o);
  REQUIRE(it.next(m));
  CHECK(std::string(m.topic) == "homeassistant/select/Other/heatControl/config");
}

TEST_CASE("classify discovery list lines (W4-1)") {
  static DiscoveryContext c;
  base(c);
  valve(c, 0, "Bad_1", true);
  valve(c, 1, "2", false, false);
  c.valves[1].tempsKnown = false;
  valve(c, 4, "5");
  temp(c, 0, "1", "28-84-37-94-97-ff-03-23");
  for (const std::string& t : topics(c)) CHECK(cls(c, t) == TopicClass::Current);
  CHECK(cls(c, "homeassistant/text/VdMot/state/config\r\n") == TopicClass::Current);
  CHECK(cls(c, "homeassistant/text/VdMot/state/config \t") == TopicClass::Current);
  CHECK(cls(c, "homeassistant/sensor/VdMot/valves_temp1_2/config") == TopicClass::KeptUnknown);
  CHECK(cls(c, "homeassistant/sensor/VdMot/valves_temp2_2/config") == TopicClass::KeptUnknown);
  CHECK(cls(c, "homeassistant/sensor/VdMot/valves_temp2_Bad_1/config") == TopicClass::Stale);
  CHECK(cls(c, "homeassistant/text/VdMot/valves_state_5/config") == TopicClass::Current);
  c.valves[4].active = false;
  CHECK(cls(c, "homeassistant/text/VdMot/valves_state_5/config") == TopicClass::Stale);
  CHECK(cls(c, "homeassistant/text/OldSt/state/config") == TopicClass::Stale);
  CHECK(cls(c, "homeassistant/sensor/VdMot/diag_stm_uptime/config") == TopicClass::Stale);
  CHECK(cls(c, "homeassistant/climate/VdMot/climate_Bad_1/config") == TopicClass::Stale);
  CHECK(cls(c, "homeassistant/sensor/VdMot/state/config") == TopicClass::Stale);  // other component
  CHECK(cls(c, "homeassistant/text/Dom 1/valves_state_\xc5\x81" "azienka/config") == TopicClass::Stale);
  CHECK(cls(c, "ha/text/VdMot/state/config") == TopicClass::Foreign);  // other prefix
  for (const std::string& f : std::vector<std::string>{
           "homeassistant/+/x/config", "homeassistant/text/VdMot/#/config", "foo/bar",
           std::string(128, 'a'), "", "\r\n", "homeassistant/text/VdMot/state/confi",
           "homeassistant/text/VdMot/state/config/", " homeassistant/text/VdMot/state/config",
           "homeassistant/text/VdMot/config", "homeassistant/text/VdMot/a/b/config",
           "homeassistant//VdMot/state/config", "homeassistant/text//state/config",
           "homeassistant/text/VdMot//config", "homeassistant/text/Vd\x01Mot/state/config",
           "homeassistant/text/Vd\x7fMot/state/config", "homeassistantx/text/VdMot/state/config",
           "homeassistant", "homeassistant/", "homeassistant/text/VdMot/state/configx"}) {
    CAPTURE(f);
    CHECK(cls(c, f) == TopicClass::Foreign);
  }
  CHECK(classifyDiscoveryTopic(c, nullptr, 5) == TopicClass::Foreign);
  // A 127-char line is still a candidate.
  const std::string edge = "homeassistant/text/VdMot/" + std::string(127 - 25 - 7, 'x') + "/config";
  REQUIRE(edge.size() == 127);
  CHECK(cls(c, edge) == TopicClass::Stale);
  // len is authoritative.
  const char line[] = "homeassistant/text/VdMot/ip/configXYZ";
  CHECK(classifyDiscoveryTopic(c, line, strlen(line) - 3) == TopicClass::Current);
  // The configured prefix: its lines are candidates, the legacy ones stale.
  copyString(c.discoveryPrefix, sizeof c.discoveryPrefix, "ha");
  CHECK(cls(c, "ha/text/VdMot/state/config") == TopicClass::Current);
  CHECK(cls(c, "homeassistant/text/VdMot/state/config") == TopicClass::Stale);
  copyString(c.discoveryPrefix, sizeof c.discoveryPrefix, "a/b");
  CHECK(cls(c, "a/b/text/VdMot/state/config") == TopicClass::Current);
  CHECK(cls(c, "a/text/VdMot/state/config") == TopicClass::Foreign);
  // Gated off -> stale.
  copyString(c.discoveryPrefix, sizeof c.discoveryPrefix, "homeassistant");
  c.publishUptime = false;
  CHECK(cls(c, "homeassistant/text/VdMot/uptime/config") == TopicClass::Stale);
  c.newDiag = false;
  CHECK(cls(c, "homeassistant/sensor/VdMot/diag_stm_link/config") == TopicClass::Stale);
  // A valve whose sensors are known: its missing temps are stale.
  c.valves[1].tempsKnown = true;
  CHECK(cls(c, "homeassistant/sensor/VdMot/valves_temp1_2/config") == TopicClass::Stale);
}

TEST_CASE("classify fuzz: only exact current topics match" * doctest::test_suite("fuzz")) {
  static DiscoveryContext c;
  base(c);
  valve(c, 0, "Bad_1", true, true);
  valve(c, 5, "6");
  temp(c, 2, "3", "28-84-37-94-97-ff-03-23");
  volt(c, 1, "Bat", "Bat", "V", "26-00-00-00-00-00-00-01");
  const std::vector<std::string> cur = topics(c);
  REQUIRE(cur.size() > 20);
  uint32_t seed = 0x5EED0002u;
  auto rnd = [&seed]() {
    seed = seed * 1664525u + 1013904223u;
    return seed >> 8;
  };
  size_t hits = 0;
  for (int iter = 0; iter < 5000; ++iter) {
    std::string t = cur[rnd() % cur.size()];
    const uint32_t op = rnd() % 5;
    if (op == 0) {
      t[rnd() % t.size()] = static_cast<char>(rnd() & 0xFF);
    } else if (op == 1) {
      t.erase(rnd() % t.size(), 1);
    } else if (op == 2) {
      t.insert(t.begin() + static_cast<long>(rnd() % (t.size() + 1)), static_cast<char>(rnd() & 0xFF));
    } else if (op == 3) {
      t.assign(rnd() % 160, '\0');
      for (char& ch : t) ch = static_cast<char>(rnd() & 0xFF);
    }
    std::string trimmed = t;
    while (!trimmed.empty() && (trimmed.back() == '\r' || trimmed.back() == '\n' ||
                                trimmed.back() == ' ' || trimmed.back() == '\t')) {
      trimmed.pop_back();
    }
    bool expect = false;
    for (const std::string& x : cur) expect = expect || x == trimmed;
    const bool got = cls(c, t) == TopicClass::Current;
    REQUIRE(got == expect);
    if (got) ++hits;
  }
  CHECK(hits > 800);
}

TEST_CASE("discovery fuzz: random context bytes never overflow or emit bad JSON framing" *
          doctest::test_suite("fuzz")) {
  static DiscoveryContext c;
  uint32_t seed = 0x5EED0003u;
  auto rnd = [&seed]() {
    seed = seed * 1664525u + 1013904223u;
    return seed >> 8;
  };
  auto fill = [&rnd](char* f, size_t n) {
    const size_t len = rnd() % (n + 1);
    for (size_t k = 0; k < len && k < n; ++k) {
      f[k] = rnd() % 4 == 0 ? static_cast<char>(rnd() & 0xFF) : static_cast<char>(0x20 + rnd() % 95);
    }
    if (len < n) f[len] = '\0';
  };
  size_t messages = 0;
  for (int iter = 0; iter < 150; ++iter) {
    base(c);
    if (rnd() % 3 == 0) fill(c.topics.station, sizeof c.topics.station);
    if (rnd() % 3 == 0) fill(c.station, sizeof c.station);
    if (rnd() % 3 == 0) fill(c.discoveryPrefix, sizeof c.discoveryPrefix);
    if (rnd() % 3 == 0) fill(c.ip, sizeof c.ip);
    if (rnd() % 3 == 0) fill(c.swVersion, sizeof c.swVersion);
    if (rnd() % 3 == 0) fill(c.hwVersion, sizeof c.hwVersion);
    c.topics.pathAsRoot = rnd() % 2;
    c.publishDiag = rnd() % 2;
    c.newDiag = rnd() % 2;
    c.publishUptime = rnd() % 2;
    c.stmV3 = rnd() % 2;
    for (auto& v : c.valves) {
      v.active = rnd() % 2;
      v.hasTemp1 = rnd() % 2;
      v.hasTemp2 = rnd() % 2;
      v.tempsKnown = rnd() % 2;
      fill(v.name, sizeof v.name);
      if (rnd() % 2) fill(v.segment, sizeof v.segment);
      else snprintf(v.segment, sizeof v.segment, "%u", static_cast<unsigned>(rnd() % 12 + 1));
    }
    for (auto* arr : {c.temps, c.volts}) {
      const size_t n = arr == c.temps ? kTempSlotCount : kVoltSlotCount;
      for (size_t i = 0; i < n; ++i) {
        DiscoveryContext::Sensor& s = arr[i];
        s.active = rnd() % 2;
        s.published = rnd() % 2;
        s.topicKnown = rnd() % 2;
        fill(s.segment, sizeof s.segment);
        fill(s.topicSegment, sizeof s.topicSegment);
        fill(s.name, sizeof s.name);
        fill(s.id, sizeof s.id);
        fill(s.unit, sizeof s.unit);
      }
    }
    DiscoveryIterator it(c);
    DiscoveryMessage m;
    static char buf[4096];
    JsonWriter jw(buf, sizeof buf);
    for (int guard = 0; guard < 1000; ++guard) {
      const bool got = it.next(m, jw);
      REQUIRE(strlen(m.topic) <= kDiscoveryTopicMax);
      if (got) {
        ++messages;
        REQUIRE(jw.complete());
        const std::string j = buf;
        REQUIRE(j.size() <= kDiscoveryPayloadMax);
        REQUIRE(j.front() == '{');
        REQUIRE(j.back() == '}');
        REQUIRE(has(j, "\"unique_id\":\""));
        REQUIRE(has(m.topic, "/config"));
        bool clean = true;
        for (char ch : j) clean = clean && static_cast<unsigned char>(ch) >= 0x20;
        REQUIRE(clean);
        bool safe = true;  // node and object ids
        for (const char* p = m.topic; *p; ++p) safe = safe && *p != ' ' && *p != '+' && *p != '#';
        REQUIRE(safe);
      } else if (jw.ok()) {
        break;
      }
    }
    DropListIterator drop(c);
    for (int guard = 0; guard < 200 && drop.next(m); ++guard) {
      REQUIRE(m.remove);
      REQUIRE(strlen(m.topic) <= kDiscoveryTopicMax);
    }
    char line[kDiscoveryTopicMax + 1];
    fill(line, sizeof line - 1);
    line[sizeof line - 1] = '\0';
    (void)classifyDiscoveryTopic(c, line, strlen(line));
  }
  CHECK(messages > 500);
}

TEST_CASE("discovery: a volt unit of the full 8 characters is kept whole") {
  static DiscoveryContext c;
  base(c);
  volt(c, 0, "v1", "Pump", "kWh/m3ab", "26-11-22-33-44-55-66-29");
  REQUIRE(strlen(c.volts[0].unit) == 8);
  CHECK(has(json(all(c), "homeassistant/sensor/VdMot/volts_v1/config"),
            "\"unit_of_measurement\":\"kWh/m3ab\""));
  memset(c.volts[0].unit, 'u', sizeof c.volts[0].unit);
  CHECK(has(json(all(c), "homeassistant/sensor/VdMot/volts_v1/config"),
            "\"unit_of_measurement\":\"uuuuuuuu\","));
}

// ---------------------------------------------------------------- context

namespace {

OneWireId owid(uint8_t last) {
  OneWireId o;
  o.b[0] = 0x28;
  o.b[7] = last;
  return o;
}

}  // namespace

TEST_CASE("buildDiscoveryContext and discoveryInputKey") {
  static Config cfg;
  cfg = Config{};
  copyString(cfg.station, sizeof cfg.station, "VdMot");
  copyString(cfg.mqtt.rootTopic, sizeof cfg.mqtt.rootTopic, "VdMotFBH");
  copyString(cfg.mqtt.discoveryPrefix, sizeof cfg.mqtt.discoveryPrefix, "ha");
  cfg.mqtt.pathAsRoot = true;
  cfg.mqtt.separate = true;
  cfg.mqtt.plainText = false;
  cfg.mqtt.diag = false;
  cfg.mqtt.upTime = false;
  cfg.mqtt.allTemps = false;
  cfg.mqtt.newDiag = false;
  cfg.mqtt.events = false;
  cfg.mqtt.publishIntervalS = 33;
  cfg.valves[0].active = true;
  copyString(cfg.valves[0].name, sizeof cfg.valves[0].name, "Bad 1");
  copyString(cfg.valves[1].topic, sizeof cfg.valves[1].topic, "Bad/WC");
  cfg.temps[0].active = true;
  cfg.temps[0].id = owid(1);
  cfg.temps[1].active = true;
  cfg.temps[1].id = owid(2);
  copyString(cfg.temps[1].name, sizeof cfg.temps[1].name, "Wohn zi");
  cfg.volts[0].id = owid(3);  // inactive
  cfg.volts[1].id = owid(4);
  cfg.volts[1].active = true;
  copyString(cfg.volts[1].unit, sizeof cfg.volts[1].unit, "V");
  static ValveState valves[kValveCount];
  for (ValveState& v : valves) v = ValveState{};
  valves[0].known = true;
  valves[0].temp1 = 215;
  valves[1].temp2 = kTempReadError;
  TempReading temps[3];
  temps[2].id = owid(1);  // slot 1 at bus index 2
  VoltReading volts[2];
  volts[1].id = owid(4);
  DiscoveryInputs in;
  in.cfg = &cfg;
  in.valves = valves;
  in.temps = temps;
  in.tempCount = 3;
  in.volts = volts;
  in.voltCount = 2;
  in.sensorsSettled = true;
  in.stmProto = 3;
  in.stmHw = "C2";
  in.ip = 0x3201A8C0u;  // 192.168.1.50
  in.swVersion = "2.1.0-revamped";
  static DiscoveryContext c;
  memset(static_cast<void*>(&c), 0x5A, sizeof c);
  REQUIRE(buildDiscoveryContext(in, c));
  CHECK(std::string(c.topics.station) == "VdMotFBH");
  CHECK(c.topics.pathAsRoot);
  CHECK(c.topics.separate);
  CHECK(std::string(c.station) == "VdMot");
  CHECK(std::string(c.discoveryPrefix) == "ha");
  CHECK_FALSE(c.plainText);
  CHECK_FALSE(c.publishDiag);
  CHECK_FALSE(c.publishUptime);
  CHECK_FALSE(c.publishAllTemps);
  CHECK_FALSE(c.newDiag);
  CHECK_FALSE(c.events);
  CHECK(c.publishIntervalS == 33);
  CHECK(c.stmV3);
  CHECK(std::string(c.ip) == "192.168.1.50");
  CHECK(std::string(c.swVersion) == "2.1.0-revamped");
  CHECK(std::string(c.hwVersion) == "C2");
  CHECK(c.valves[0].active);
  CHECK(std::string(c.valves[0].segment) == "Bad_1");
  CHECK(std::string(c.valves[0].name) == "Bad 1");
  CHECK(c.valves[0].hasTemp1);
  CHECK_FALSE(c.valves[0].hasTemp2);
  CHECK(c.valves[0].tempsKnown);
  CHECK_FALSE(c.valves[1].active);
  CHECK(std::string(c.valves[1].segment) == "Bad/WC");
  CHECK_FALSE(c.valves[1].hasTemp1);
  CHECK(c.valves[1].hasTemp2);
  CHECK_FALSE(c.valves[1].tempsKnown);  // not known yet
  CHECK(std::string(c.valves[2].segment) == "3");
  CHECK(c.temps[0].active);
  CHECK(c.temps[0].published);
  CHECK(std::string(c.temps[0].segment) == "1");
  CHECK(std::string(c.temps[0].topicSegment) == "3");
  CHECK(c.temps[0].topicKnown);
  CHECK(std::string(c.temps[0].id) == "28-00-00-00-00-00-00-01");
  CHECK(std::string(c.temps[1].segment) == "Wohn_zi");
  CHECK(std::string(c.temps[1].topicSegment) == "Wohn_zi");
  CHECK(c.temps[1].topicKnown);
  CHECK(std::string(c.temps[1].name) == "Wohn zi");
  CHECK_FALSE(c.temps[2].active);
  CHECK_FALSE(c.temps[2].topicKnown);
  CHECK(c.temps[2].id[0] == '\0');
  CHECK_FALSE(c.volts[0].active);  // inactive: not announced
  CHECK(c.volts[0].id[0] != '\0');
  CHECK_FALSE(c.volts[0].topicKnown);
  CHECK(c.volts[1].active);
  CHECK(c.volts[1].published);
  CHECK(std::string(c.volts[1].topicSegment) == "2");
  CHECK(std::string(c.volts[1].unit) == "V");
  // allTemps off: a temp assigned to a valve is not published.
  valves[3].sensorSlot[0] = 2;
  buildDiscoveryContext(in, c);
  CHECK_FALSE(c.temps[1].published);
  CHECK(c.temps[0].published);
  // No ip, no valves: defaults.
  in.ip = 0;
  in.valves = nullptr;
  in.stmProto = 2;
  buildDiscoveryContext(in, c);
  CHECK(c.ip[0] == '\0');
  CHECK_FALSE(c.stmV3);
  CHECK_FALSE(c.valves[0].tempsKnown);
  CHECK_FALSE(c.valves[0].hasTemp1);
  // Without a config: false and an empty context.
  DiscoveryInputs none;
  memset(static_cast<void*>(&c), 0x5A, sizeof c);
  CHECK_FALSE(buildDiscoveryContext(none, c));
  CHECK(c.valves[0].active == false);
  CHECK(std::string(c.discoveryPrefix) == "homeassistant");
  CHECK(discoveryInputKey(none) == 0);

  // The key follows the snapshot-dependent parts only.
  in.valves = valves;
  in.stmProto = 3;
  in.ip = 0x3201A8C0u;
  const uint32_t k0 = discoveryInputKey(in);
  CHECK(k0 != 0);
  CHECK(discoveryInputKey(in) == k0);
  in.ip = 1;  // not in the key
  in.swVersion = "x";
  CHECK(discoveryInputKey(in) == k0);
  valves[5].temp1 = 200;
  const uint32_t k1 = discoveryInputKey(in);
  CHECK(k1 != k0);
  valves[5].temp2 = 200;
  const uint32_t k2 = discoveryInputKey(in);
  CHECK(k2 != k1);
  valves[5].known = true;
  const uint32_t k3 = discoveryInputKey(in);
  CHECK(k3 != k2);
  temps[2].id = owid(9);  // slot 1 leaves the bus
  const uint32_t k4 = discoveryInputKey(in);
  CHECK(k4 != k3);
  temps[1].id = owid(1);  // back at another index
  const uint32_t k5 = discoveryInputKey(in);
  CHECK(k5 != k4);
  volts[1].id = owid(8);
  const uint32_t k6 = discoveryInputKey(in);
  CHECK(k6 != k5);
  in.stmHw = "C1";
  const uint32_t k7 = discoveryInputKey(in);
  CHECK(k7 != k6);
  in.stmProto = 2;
  const uint32_t k8 = discoveryInputKey(in);
  CHECK(k8 != k7);
  valves[3].sensorSlot[0] = 0;  // temp 2 published again
  CHECK(discoveryInputKey(in) != k8);
}

// ---------------------------------------------------------------- list and run

namespace {

// In-memory LittleFS + broker for DiscoveryRun.
struct FakePort : DiscoveryPort {
  bool exists = true;
  std::string list;
  std::string tmp;
  bool writing = false;
  size_t readPos = 0;
  bool reading = false;
  long failPublishAt = -1;
  bool failBegin = false;
  long failWriteAt = -1;
  bool failCommit = false;
  std::vector<std::pair<std::string, std::string>> published;
  int opens = 0, reads = 0, closes = 0, begins = 0, writes = 0, commits = 0, aborts = 0;

  bool publish(const char* topic, const char* payload) override {
    if (static_cast<long>(published.size()) == failPublishAt) return false;
    published.emplace_back(topic, payload);
    return true;
  }
  bool listOpen() override {
    ++opens;
    if (!exists) return false;
    reading = true;
    readPos = 0;
    return true;
  }
  int listRead() override {
    ++reads;
    if (!reading || readPos >= list.size()) return -1;
    return static_cast<unsigned char>(list[readPos++]);
  }
  void listClose() override {
    ++closes;
    reading = false;
  }
  bool listBegin() override {
    ++begins;
    if (failBegin) return false;
    writing = true;
    tmp.clear();
    return true;
  }
  bool listWrite(const char* topic) override {
    if (writes++ == failWriteAt) return false;
    tmp += topic;
    tmp += '\n';
    return true;
  }
  bool listCommit() override {
    ++commits;
    writing = false;
    if (failCommit) return false;
    list = tmp;
    exists = true;
    return true;
  }
  void listAbort() override {
    ++aborts;
    writing = false;
    tmp.clear();
  }
  std::vector<std::string> deletes() const {
    std::vector<std::string> v;
    for (const auto& p : published) {
      if (p.second.empty()) v.push_back(p.first);
    }
    return v;
  }
  std::vector<std::string> configs() const {
    std::vector<std::string> v;
    for (const auto& p : published) {
      if (!p.second.empty()) v.push_back(p.first);
    }
    return v;
  }
};

DiscoveryRun::Phase runAll(DiscoveryRun& run, FakePort& port, int maxSteps = 5000) {
  static char buf[4096];
  JsonWriter jw(buf, sizeof buf);
  for (int i = 0; i < maxSteps && run.running(); ++i) run.step(port, jw);
  return run.phase();
}

std::string lines(const std::vector<std::string>& v) {
  std::string s;
  for (const std::string& t : v) s += t + "\n";
  return s;
}

void small(DiscoveryContext& c) {
  base(c);
  c.newDiag = false;
  c.events = false;
  c.publishDiag = false;
  c.publishUptime = false;
}

}  // namespace

TEST_CASE("readListLine: CR/LF, empty and overlong lines") {
  FakePort p;
  p.list = "a\r\n\nbb\n" + std::string(128, 'x') + "\ncc" + "\n" + std::string(127, 'y') + "\rlast";
  REQUIRE(p.listOpen());
  char out[kDiscoveryTopicMax + 1];
  REQUIRE(readListLine(p, out));
  CHECK(std::string(out) == "a");
  REQUIRE(readListLine(p, out));
  CHECK(std::string(out) == "bb");
  REQUIRE(readListLine(p, out));
  CHECK(std::string(out) == "cc");  // the 128-char line is dropped
  REQUIRE(readListLine(p, out));
  CHECK(std::string(out) == std::string(127, 'y'));
  REQUIRE(readListLine(p, out));
  CHECK(std::string(out) == "last");
  CHECK_FALSE(readListLine(p, out));
  CHECK(out[0] == '\0');
  FakePort e;
  e.list = "\n\r\n";
  REQUIRE(e.listOpen());
  CHECK_FALSE(readListLine(e, out));
  FakePort o;
  o.list = std::string(200, 'z');  // overlong at the end
  REQUIRE(o.listOpen());
  CHECK_FALSE(readListLine(o, out));
}

TEST_CASE("DiscoveryRun: prune before publish, rewrite the list (W4-2, W4-3)") {
  static DiscoveryContext c;
  small(c);
  valve(c, 0, "New");
  temp(c, 0, "B", "28-00-00-00-00-00-00-01");
  FakePort port;
  port.list = "homeassistant/text/VdMot/state/config\n"
              "homeassistant/text/VdMot/valves_state_Old/config\n"
              "homeassistant/sensor/VdMot/temps_A/config\n";
  DiscoveryRun run;
  CHECK(run.phase() == DiscoveryRun::Phase::Idle);
  DiscoveryPlan plan;
  plan.publish = true;
  run.start(c, plan);
  CHECK(run.phase() == DiscoveryRun::Phase::Prune);
  CHECK(run.running());
  REQUIRE(runAll(run, port) == DiscoveryRun::Phase::Done);
  const std::vector<std::string> cur = topics(c);
  std::vector<std::string> expected = {"homeassistant/text/VdMot/valves_state_Old/config",
                                       "homeassistant/sensor/VdMot/temps_A/config"};
  for (const std::string& t : cur) expected.push_back(t);
  std::vector<std::string> order;
  for (const auto& p : port.published) order.push_back(p.first);
  CHECK(order == expected);
  CHECK(port.deletes().size() == 2);
  CHECK(port.configs() == cur);
  CHECK(port.list == lines(cur));
  CHECK(run.stats().deletes == 2);
  CHECK(run.stats().configs == cur.size());
  CHECK(run.stats().skipped == 0);
  CHECK(run.stats().listWritten);
  CHECK(port.commits == 1);
  CHECK_FALSE(port.reading);
  // Second run: nothing to delete, no list write.
  FakePort again = port;
  again.published.clear();
  again.begins = again.commits = 0;
  DiscoveryRun second;
  second.start(c, plan);
  REQUIRE(runAll(second, again) == DiscoveryRun::Phase::Done);
  CHECK(again.deletes().empty());
  CHECK(again.begins == 0);
  CHECK_FALSE(second.stats().listWritten);
  CHECK(second.stats().configs == cur.size());
  CHECK(again.list == lines(cur));
}

TEST_CASE("DiscoveryRun: KeptUnknown lines are carried first and deleted once known (W4-4)") {
  static DiscoveryContext c;
  small(c);
  valve(c, 1, "2");
  c.valves[1].tempsKnown = false;
  const std::string kept = "homeassistant/sensor/VdMot/valves_temp1_2/config";
  FakePort port;
  port.list = kept + "\n";
  DiscoveryPlan plan;
  plan.publish = true;
  DiscoveryRun run;
  run.start(c, plan);
  REQUIRE(runAll(run, port) == DiscoveryRun::Phase::Done);
  CHECK(port.deletes().empty());
  const std::vector<std::string> cur = topics(c);
  CHECK(port.list == kept + "\n" + lines(cur));
  // The valve's sensors become known and there is none: the next run deletes it.
  c.valves[1].tempsKnown = true;
  port.published.clear();
  DiscoveryRun next;
  next.start(c, plan);
  REQUIRE(runAll(next, port) == DiscoveryRun::Phase::Done);
  CHECK(port.deletes() == std::vector<std::string>{kept});
  CHECK(port.list == lines(cur));
}

TEST_CASE("DiscoveryRun: Delete and DeleteAndPublish (W4-5)") {
  static DiscoveryContext c;
  small(c);
  valve(c, 0, "1");
  const std::vector<std::string> cur = topics(c);
  FakePort port;
  port.list = "homeassistant/text/VdMot/old/config\nfoo/bar\n" + lines(cur);
  DiscoveryPlan del;
  del.removeAll = true;
  del.prune = false;
  DiscoveryRun run;
  run.start(c, del);
  CHECK(run.phase() == DiscoveryRun::Phase::RemoveList);
  REQUIRE(runAll(run, port) == DiscoveryRun::Phase::Done);
  std::vector<std::string> expected = {"homeassistant/text/VdMot/old/config"};
  for (const std::string& t : cur) expected.push_back(t);  // list lines
  for (const std::string& t : cur) expected.push_back(t);  // current topics
  CHECK(port.deletes() == expected);
  CHECK(port.configs().empty());
  CHECK(port.list.empty());
  CHECK(port.commits == 1);
  CHECK(run.stats().listWritten);
  CHECK(run.stats().deletes == expected.size());
  // DeleteAndPublish: the same deletes, then every config, then the new list.
  port.list = "homeassistant/text/VdMot/old/config\n";
  port.published.clear();
  DiscoveryPlan both;
  both.removeAll = true;
  both.publish = true;
  DiscoveryRun r2;
  r2.start(c, both);
  REQUIRE(runAll(r2, port) == DiscoveryRun::Phase::Done);
  std::vector<std::string> dels = {"homeassistant/text/VdMot/old/config"};
  for (const std::string& t : cur) dels.push_back(t);
  CHECK(port.deletes() == dels);
  CHECK(port.configs() == cur);
  CHECK(port.list == lines(cur));
  // A publish failure in the middle: Aborted, the tmp list abandoned, no commit.
  FakePort f;
  f.list = lines(cur);
  f.failPublishAt = 2;
  DiscoveryRun r3;
  r3.start(c, del);
  REQUIRE(runAll(r3, f) == DiscoveryRun::Phase::Aborted);
  CHECK(f.published.size() == 2);
  CHECK(f.commits == 0);
  CHECK_FALSE(f.reading);
  CHECK_FALSE(r3.running());
  // A failure while the current topics are removed: the list stays as it was.
  FakePort w;
  w.list = "homeassistant/text/VdMot/x/config\n";
  w.failPublishAt = 2;
  DiscoveryRun r4;
  r4.start(c, both);
  REQUIRE(runAll(r4, w) == DiscoveryRun::Phase::Aborted);
  CHECK(w.commits == 0);
  CHECK(w.begins == 0);
  CHECK(w.list == "homeassistant/text/VdMot/x/config\n");
  // abort() of the glue (connection lost) while the tmp list is written.
  FakePort g;
  g.exists = false;
  DiscoveryPlan pub;
  pub.publish = true;
  DiscoveryRun r5;
  r5.start(c, pub);
  static char buf[4096];
  JsonWriter jw(buf, sizeof buf);
  while (r5.phase() != DiscoveryRun::Phase::WriteCurrent) REQUIRE(r5.step(g, jw) != DiscoveryRun::Phase::Done);
  r5.abort(g);
  CHECK(r5.phase() == DiscoveryRun::Phase::Aborted);
  CHECK(g.aborts == 1);
  CHECK(g.commits == 0);
  r5.abort(g);  // idempotent
  CHECK(g.aborts == 1);
  CHECK(r5.step(g, jw) == DiscoveryRun::Phase::Aborted);
}

TEST_CASE("DiscoveryRun: missing list, budget, failures (W4-6, W4-7, W4-10)") {
  static DiscoveryContext c;
  small(c);
  valve(c, 0, "1");
  valve(c, 1, "2");
  const std::vector<std::string> cur = topics(c);
  FakePort port;
  port.exists = false;
  DiscoveryPlan plan;
  plan.publish = true;
  DiscoveryRun run;
  run.start(c, plan);
  static char buf[4096];
  JsonWriter jw(buf, sizeof buf);
  int steps = 0;
  while (run.running()) {
    const size_t pubs = port.published.size();
    const int reads = port.reads;
    const int writes = port.writes;
    run.step(port, jw);
    ++steps;
    CHECK(port.published.size() - pubs <= 1);
    CHECK(writes + DiscoveryRun::kLinesPerStep >= port.writes);
    CHECK(reads <= port.reads);
    REQUIRE(steps < 1000);
  }
  CHECK(run.phase() == DiscoveryRun::Phase::Done);
  CHECK(port.deletes().empty());
  CHECK(port.list == lines(cur));
  // Reading is bounded too: 8 lines per step.
  FakePort big;
  std::string many;
  for (int i = 0; i < 40; ++i) many += "homeassistant/text/VdMot/state/config\n";
  big.list = many;
  DiscoveryPlan cleanup;  // no publish: Current lines are carried
  DiscoveryRun r;
  r.start(c, cleanup);
  CHECK(r.phase() == DiscoveryRun::Phase::Prune);
  r.step(big, jw);
  CHECK(big.reads == static_cast<int>(8 * strlen("homeassistant/text/VdMot/state/config\n")));
  REQUIRE(runAll(r, big) == DiscoveryRun::Phase::Done);
  CHECK(big.list == many);  // unchanged content: no rewrite
  CHECK(big.begins == 0);
  // An unsafe station: Done after one step, nothing touched.
  static DiscoveryContext u;
  small(u);
  copyString(u.topics.station, sizeof u.topics.station, "a+b");
  FakePort up;
  up.list = "homeassistant/text/x/state/config\n";
  DiscoveryRun ru;
  ru.start(u, plan);
  CHECK(ru.step(up, jw) == DiscoveryRun::Phase::Done);
  CHECK(up.published.empty());
  CHECK(up.opens == 0);
  CHECK(up.list == "homeassistant/text/x/state/config\n");
  // listBegin failing: every config published, the list not written.
  FakePort nb;
  nb.exists = false;
  nb.failBegin = true;
  DiscoveryRun rb;
  rb.start(c, plan);
  REQUIRE(runAll(rb, nb) == DiscoveryRun::Phase::Done);
  CHECK(nb.configs() == cur);
  CHECK_FALSE(rb.stats().listWritten);
  CHECK(nb.aborts == 1);
  CHECK(nb.commits == 0);
  // listWrite failing: abandoned, Done.
  FakePort nw;
  nw.exists = false;
  nw.failWriteAt = 3;
  DiscoveryRun rw;
  rw.start(c, plan);
  REQUIRE(runAll(rw, nw) == DiscoveryRun::Phase::Done);
  CHECK_FALSE(rw.stats().listWritten);
  CHECK(nw.aborts == 1);
  CHECK(nw.commits == 0);
  CHECK_FALSE(nw.exists);
  // listCommit failing.
  FakePort nc;
  nc.exists = false;
  nc.failCommit = true;
  DiscoveryRun rc;
  rc.start(c, plan);
  REQUIRE(runAll(rc, nc) == DiscoveryRun::Phase::Done);
  CHECK_FALSE(rc.stats().listWritten);
  CHECK(nc.aborts == 1);
  // A carried line failing to write.
  FakePort kw;
  kw.list = "homeassistant/text/VdMot/state/config\nhomeassistant/text/VdMot/x/config\n";
  kw.failWriteAt = 0;
  DiscoveryRun rk;
  rk.start(c, cleanup);
  REQUIRE(runAll(rk, kw) == DiscoveryRun::Phase::Done);
  CHECK(kw.aborts == 1);
  CHECK_FALSE(rk.stats().listWritten);
  // A ClearList whose listBegin fails.
  FakePort cb;
  cb.failBegin = true;
  cb.list = "";
  DiscoveryPlan del;
  del.removeAll = true;
  del.prune = false;
  DiscoveryRun rd;
  rd.start(c, del);
  REQUIRE(runAll(rd, cb) == DiscoveryRun::Phase::Done);
  CHECK(cb.aborts == 1);
  CHECK_FALSE(rd.stats().listWritten);
  // A run that was never started does nothing.
  DiscoveryRun idle;
  CHECK(idle.step(port, jw) == DiscoveryRun::Phase::Idle);
  CHECK_FALSE(idle.running());
  idle.abort(port);
  CHECK(idle.phase() == DiscoveryRun::Phase::Idle);
}

TEST_CASE("DiscoveryRun: first-run cleanup, migration, foreign and oversize entries") {
  static DiscoveryContext c;
  small(c);
  copyString(c.topics.station, sizeof c.topics.station, "Dom 1");
  valve(c, 0, "1");
  const std::vector<std::string> cur = topics(c);
  // Mode 1 cleanup: DROP list and prune, Current and KeptUnknown carried, no publish.
  FakePort port;
  port.list = "foo/bar\n" + cur[0] + "\nhomeassistant/text/Dom 1/state/config\n";
  DiscoveryPlan cleanup;
  cleanup.dropLegacy = true;
  DiscoveryRun run;
  run.start(c, cleanup);
  CHECK(run.phase() == DiscoveryRun::Phase::DropList);
  REQUIRE(runAll(run, port) == DiscoveryRun::Phase::Done);
  const std::vector<std::string> d = port.deletes();
  REQUIRE(d.size() == 2 + 7 * 12 + 1);
  CHECK(d[0] == "homeassistant/select/Dom 1/heatControl/config");
  CHECK(d.back() == "homeassistant/text/Dom 1/state/config");
  CHECK(port.configs().empty());
  CHECK(port.list == cur[0] + "\n");
  CHECK(run.stats().listWritten);
  // Migration: uptime and the 2.0.0 forms of changed ids, then the configs.
  FakePort mig;
  mig.exists = false;
  DiscoveryPlan plan;
  plan.publish = true;
  plan.retire20 = true;
  DiscoveryRun m;
  m.start(c, plan);
  CHECK(m.phase() == DiscoveryRun::Phase::Retired);
  REQUIRE(runAll(m, mig) == DiscoveryRun::Phase::Done);
  std::vector<std::string> dels = {"homeassistant/sensor/Dom 1/diag_stm_uptime/config"};
  for (const std::string& t : cur) {
    std::string old = t;
    old.replace(old.find("Dom_1"), 5, "Dom 1");
    dels.push_back(old);
  }
  CHECK(mig.deletes() == dels);
  CHECK(mig.configs() == cur);
  // retire20 without publish is ignored.
  FakePort np;
  DiscoveryPlan r20;
  r20.retire20 = true;
  DiscoveryRun rn;
  rn.start(c, r20);
  CHECK(rn.phase() == DiscoveryRun::Phase::Prune);
  // An entity whose payload does not fit: skipped, its topic still listed.
  static DiscoveryContext big;
  small(big);
  big.events = true;
  FakePort bp;
  bp.exists = false;
  DiscoveryRun rb;
  rb.start(big, plan);
  static char tiny[1200];
  JsonWriter jw(tiny, sizeof tiny);
  for (int i = 0; i < 2000 && rb.running(); ++i) rb.step(bp, jw);
  REQUIRE(rb.phase() == DiscoveryRun::Phase::Done);
  CHECK(rb.stats().skipped == 1);  // the event entity
  CHECK(has(bp.list, "homeassistant/event/VdMot/events/config\n"));
  CHECK(bp.configs().size() + 1 == topics(big).size());
}

// ---------------------------------------------------------------- edge cases

TEST_CASE("discovery: a one-char station and a one-char prefix") {
  static DiscoveryContext c;
  small(c);
  copyString(c.station, sizeof c.station, "X");
  copyString(c.discoveryPrefix, sizeof c.discoveryPrefix, "a");
  const std::vector<Msg> v = all(c);
  REQUIRE_FALSE(v.empty());
  CHECK(v[0].topic == "a/text/X/state/config");
  CHECK(has(v[0].json, "\"identifiers\":\"X\""));
  CHECK(has(v[0].json, "\"unique_id\":\"X.common.state\""));
}

TEST_CASE("discovery: expire_after is three publish intervals, at least 60 s") {
  static DiscoveryContext c;
  small(c);
  valve(c, 0, "1", true);
  for (const auto& p : std::vector<std::pair<uint16_t, std::string>>{{19, "60"}, {20, "60"}, {21, "63"}}) {
    c.publishIntervalS = p.first;
    CAPTURE(p.first);
    CHECK(has(json(all(c), "homeassistant/sensor/VdMot/valves_temp1_1/config"),
              "\"expire_after\":" + p.second + ","));
  }
}

TEST_CASE("discovery: a valve name of the full 10 chars is kept whole in entity names") {
  static DiscoveryContext c;
  small(c);
  valve(c, 0, "1", false, false, "ABCDEFGHIJ");
  CHECK(has(json(all(c), "homeassistant/sensor/VdMot/valves_actual_1/config"),
            "\"name\":\"ABCDEFGHIJ position\""));
}

TEST_CASE("discovery: the event entity lists fewer than 127 event types") {
  CHECK(eventMqttNames(nullptr, 0) < 127);
}

TEST_CASE("classify: the shape of a config topic") {
  static DiscoveryContext c;
  small(c);
  CHECK(classifyDiscoveryTopic(c, "a/b/c/config", 12) == TopicClass::Foreign);
  const std::vector<std::string> foreign = {
      "Xa/b/c/config", "abcdefghijklm/sensor/VdMot/x/config",
      "homeassistant/sensor/VdMot/x/confiG",
      "homeassistant/sensor/VdMot/" + std::string(110, 'x') + "/config"};
  for (const std::string& t : foreign) {
    CAPTURE(t);
    CHECK(classifyDiscoveryTopic(c, t.c_str(), t.size()) == TopicClass::Foreign);
  }
  const std::string oneChar = "homeassistant/s/VdMot/x/config";
  CHECK(classifyDiscoveryTopic(c, oneChar.c_str(), oneChar.size()) == TopicClass::Stale);
}

TEST_CASE("v20Topic: nothing for an unsafe station or a skipped last entity") {
  static DiscoveryContext c;
  small(c);
  copyString(c.station, sizeof c.station, "a/b");
  DiscoveryIterator bad(c);
  DiscoveryMessage m;
  CHECK_FALSE(bad.nextTopic(m));
  CHECK(bad.position() == 309);
  CHECK_FALSE(bad.v20Topic(m));
  small(c);
  copyString(c.station, sizeof c.station, "Dom 1");
  DiscoveryIterator it(c);
  while (it.nextTopic(m)) {
  }
  CHECK(it.position() == 309);  // the last entity (leave safe mode) needs STM v3
  CHECK_FALSE(it.v20Topic(m));
}

TEST_CASE("DiscoveryIterator::reset starts again at the first entity") {
  static DiscoveryContext c;
  small(c);
  DiscoveryIterator it(c);
  DiscoveryMessage first;
  DiscoveryMessage m;
  REQUIRE(it.nextTopic(first));
  REQUIRE(it.nextTopic(m));
  it.reset(c);
  CHECK(it.position() == 0);
  REQUIRE(it.nextTopic(m));
  CHECK(std::string(m.topic) == first.topic);
}

TEST_CASE("readListLine: a NUL byte does not end a line") {
  FakePort p;
  p.list = std::string("ab\0cd\n", 6);
  REQUIRE(p.listOpen());
  char out[kDiscoveryTopicMax + 1];
  REQUIRE(readListLine(p, out));
  CHECK(std::string(out) == "ab");
  CHECK_FALSE(readListLine(p, out));
}

TEST_CASE("discoveryInputKey: CRC32 over the snapshot parts in their order") {
  static Config cfg;
  cfg = Config{};
  copyString(cfg.station, sizeof cfg.station, "VdMot");
  cfg.temps[0].active = true;
  cfg.temps[0].id = owid(1);
  cfg.temps[3].id = owid(5);  // inactive with an id: not active
  cfg.volts[1].active = true;
  cfg.volts[1].id = owid(4);
  static ValveState valves[kValveCount];
  for (ValveState& v : valves) v = ValveState{};
  valves[0].known = true;
  valves[0].temp1 = 215;
  TempReading temps[1];
  temps[0].id = owid(1);
  VoltReading volts[2];
  volts[1].id = owid(4);
  DiscoveryInputs in;
  in.cfg = &cfg;
  in.valves = valves;
  in.temps = temps;
  in.tempCount = 1;
  in.volts = volts;
  in.voltCount = 2;
  in.sensorsSettled = true;
  in.stmProto = 3;
  in.stmHw = "C2";
  static DiscoveryContext c;
  REQUIRE(buildDiscoveryContext(in, c));
  CHECK_FALSE(c.temps[3].active);
  CHECK(std::string(c.volts[1].topicSegment) == "2");
  CHECK(c.volts[1].topicKnown);
  uint32_t crc = 0;
  for (const DiscoveryContext::Valve& v : c.valves) {
    const uint8_t b[3] = {v.hasTemp1, v.hasTemp2, v.tempsKnown};
    crc = crc32(b, sizeof b, crc);
  }
  for (const DiscoveryContext::Sensor* arr : {c.temps, c.volts}) {
    const uint8_t n = arr == c.temps ? kTempSlotCount : kVoltSlotCount;
    for (uint8_t i = 0; i < n; ++i) {
      const uint8_t b[2] = {arr[i].published, arr[i].topicKnown};
      crc = crc32(b, sizeof b, crc);
      crc = crc32(reinterpret_cast<const uint8_t*>(arr[i].topicSegment), sizeof arr[i].topicSegment, crc);
    }
  }
  crc = crc32(reinterpret_cast<const uint8_t*>(c.hwVersion), sizeof c.hwVersion, crc);
  const uint8_t v3 = 1;
  CHECK(discoveryInputKey(in) == crc32(&v3, 1, crc));
}

TEST_CASE("DiscoveryRun: a prune without publish writes a missing list") {
  static DiscoveryContext c;
  small(c);
  FakePort port;
  port.exists = false;
  DiscoveryPlan plan;  // prune only
  DiscoveryRun run;
  run.start(c, plan);
  REQUIRE(runAll(run, port) == DiscoveryRun::Phase::Done);
  CHECK(port.commits == 1);
  CHECK(run.stats().listWritten);
  CHECK(port.list.empty());
}

TEST_CASE("DiscoveryRun: the same topics in another order are written again") {
  static DiscoveryContext c;
  small(c);
  std::vector<std::string> cur = topics(c);
  std::vector<std::string> reversed(cur.rbegin(), cur.rend());
  FakePort port;
  port.list = lines(reversed);
  DiscoveryPlan plan;
  plan.publish = true;
  DiscoveryRun run;
  run.start(c, plan);
  REQUIRE(runAll(run, port) == DiscoveryRun::Phase::Done);
  CHECK(port.commits == 1);
  CHECK(port.list == lines(cur));
  CHECK(port.closes == port.opens);
}

TEST_CASE("DiscoveryRun: an unbuildable entity adds no list line") {
  static DiscoveryContext c;
  small(c);
  valve(c, 0, "a//b");
  FakePort port;
  port.list = lines(topics(c));
  DiscoveryPlan plan;
  plan.publish = true;
  DiscoveryRun run;
  run.start(c, plan);
  REQUIRE(runAll(run, port) == DiscoveryRun::Phase::Done);
  CHECK(run.stats().skipped == 9);
  CHECK(port.begins == 0);
  CHECK(port.commits == 0);
}

TEST_CASE("DiscoveryRun: eight list lines per step") {
  static DiscoveryContext c;
  small(c);
  FakePort port;
  for (int i = 0; i < 20; ++i) port.list += "x\n";
  DiscoveryPlan plan;
  plan.removeAll = true;
  plan.prune = false;
  DiscoveryRun run;
  run.start(c, plan);
  REQUIRE(run.phase() == DiscoveryRun::Phase::RemoveList);
  static char buf[4096];
  JsonWriter jw(buf, sizeof buf);
  run.step(port, jw);
  CHECK(port.readPos == 16);
  CHECK(run.phase() == DiscoveryRun::Phase::RemoveList);
  REQUIRE(runAll(run, port) == DiscoveryRun::Phase::Done);
  CHECK(port.closes == port.opens);
}

TEST_CASE("DiscoveryRun: eight current topics per list write step") {
  static DiscoveryContext c;
  base(c);
  FakePort port;
  port.exists = false;
  DiscoveryPlan plan;
  plan.publish = true;
  DiscoveryRun run;
  run.start(c, plan);
  static char buf[4096];
  JsonWriter jw(buf, sizeof buf);
  for (int i = 0; i < 5000 && run.phase() != DiscoveryRun::Phase::WriteCurrent; ++i) run.step(port, jw);
  REQUIRE(run.phase() == DiscoveryRun::Phase::WriteCurrent);
  REQUIRE(port.writes == 0);
  run.step(port, jw);
  CHECK(port.writes == 8);
  REQUIRE(runAll(run, port) == DiscoveryRun::Phase::Done);
  CHECK(port.list == lines(topics(c)));
  CHECK(port.commits == 1);
}

TEST_CASE("DiscoveryRun: delete and publish with prune closes and commits each list once") {
  static DiscoveryContext c;
  small(c);
  FakePort port;
  port.list = lines(topics(c));
  DiscoveryPlan plan;
  plan.removeAll = true;
  plan.publish = true;
  DiscoveryRun run;
  run.start(c, plan);
  REQUIRE(runAll(run, port) == DiscoveryRun::Phase::Done);
  CHECK(port.aborts == 0);
  CHECK(port.closes == port.opens);
  CHECK(port.commits == 2);  // the cleared list, then the new one
  CHECK(port.list == lines(topics(c)));
}
