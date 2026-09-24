// Home Assistant discovery: legacy KEEP entities (object ids, names,
// unique_ids from specs/03 §7), new diagnostic entities, DROP deletions and
// the stale-topic check.
#include <stdio.h>
#include <string.h>

#include <initializer_list>
#include <string>
#include <vector>

#include "doctest.h"
#include "vdm/ha_discovery.h"
#include "vdm/json_writer.h"

using namespace vdm;

namespace {

const char kDevice[] =
    "\"availability_topic\":\"VdMot/status\",\"payload_available\":\"online\","
    "\"payload_not_available\":\"offline\",\"device\":{\"identifiers\":\"VdMot\",\"name\":\"VdMot\","
    "\"sw_version\":\"2.0.0-revamped\",\"hw_version\":\"2.0\",\"model\":\"VdMot Revamped\","
    "\"manufacturer\":\"Lenti84/Surfgargano\",\"configuration_url\":\"http://192.168.1.50/\"}}";

void base(DiscoveryContext& c) {
  copyString(c.topics.station, sizeof c.topics.station, "VdMot");
  copyString(c.ip, sizeof c.ip, "192.168.1.50");
  copyString(c.swVersion, sizeof c.swVersion, "2.0.0-revamped");
}

void valve(DiscoveryContext& c, uint8_t i, const char* seg, bool t1 = false, bool t2 = false) {
  c.valves[i].active = true;
  copyString(c.valves[i].segment, sizeof c.valves[i].segment, seg);
  c.valves[i].hasTemp1 = t1;
  c.valves[i].hasTemp2 = t2;
}

void temp(DiscoveryContext& c, uint8_t i, const char* seg, const char* id, bool published = true) {
  c.temps[i].active = true;
  c.temps[i].published = published;
  copyString(c.temps[i].segment, sizeof c.temps[i].segment, seg);
  copyString(c.temps[i].id, sizeof c.temps[i].id, id);
}

void volt(DiscoveryContext& c, uint8_t i, const char* seg, const char* name, const char* unit,
          const char* id) {
  c.volts[i].active = true;
  copyString(c.volts[i].segment, sizeof c.volts[i].segment, seg);
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
  char buf[kDiscoveryPayloadMax + 1];
  JsonWriter jw(buf, sizeof buf);
  while (it.next(m, jw)) {
    CHECK_FALSE(m.remove);
    CHECK(jw.complete());
    out.push_back({m.topic, buf});
  }
  CHECK(jw.ok());  // ended because the list is exhausted
  return out;
}

const Msg* find(const std::vector<Msg>& v, const std::string& topic) {
  for (const Msg& m : v) {
    if (m.topic == topic) return &m;
  }
  return nullptr;
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

bool current(const DiscoveryContext& c, const std::string& t) {
  return discoveryTopicIsCurrent(c, t.data(), t.size());
}

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
  CHECK(std::string(haComponentName(static_cast<HaComponent>(8))) == "");
}

TEST_CASE("discovery: golden payloads and fixed order") {
  static DiscoveryContext c;
  c = DiscoveryContext{};
  base(c);
  valve(c, 0, "Bad_1", true);
  valve(c, 2, "3", false, true);
  temp(c, 0, "1", "28-84-37-94-97-ff-03-23");
  volt(c, 1, "Batt", "Batt", "V", "26-00-00-00-00-00-00-01");
  const std::vector<Msg> v = all(c);

  const std::vector<std::string> expected = {
      "homeassistant/text/VdMot/state/config",
      "homeassistant/text/VdMot/message/config",
      "homeassistant/text/VdMot/uptime/config",
      "homeassistant/text/VdMot/ip/config",
      "homeassistant/text/VdMot/valves_state_Bad_1/config",
      "homeassistant/valve/VdMot/valves_target_Bad_1/config",
      "homeassistant/sensor/VdMot/valves_actual_Bad_1/config",
      "homeassistant/sensor/VdMot/valves_temp1_Bad_1/config",
      "homeassistant/text/VdMot/valves_calibration_date_Bad_1/config",
      "homeassistant/text/VdMot/valves_calibration_repetitions_Bad_1/config",
      "homeassistant/text/VdMot/valves_diag_openCount_Bad_1/config",
      "homeassistant/text/VdMot/valves_diag_closeCount_Bad_1/config",
      "homeassistant/text/VdMot/valves_diag_deadZoneCount_Bad_1/config",
      "homeassistant/text/VdMot/valves_diag_moves_Bad_1/config",
      "homeassistant/text/VdMot/valves_diag_meanCurrrent_Bad_1/config",
      "homeassistant/sensor/VdMot/diag_earlyStops_Bad_1/config",
      "homeassistant/sensor/VdMot/diag_cmdRejected_Bad_1/config",
      "homeassistant/sensor/VdMot/diag_lastStop_Bad_1/config",
      "homeassistant/text/VdMot/valves_state_3/config",
      "homeassistant/valve/VdMot/valves_target_3/config",
      "homeassistant/sensor/VdMot/valves_actual_3/config",
      "homeassistant/sensor/VdMot/valves_temp2_3/config",
      "homeassistant/text/VdMot/valves_calibration_date_3/config",
      "homeassistant/text/VdMot/valves_calibration_repetitions_3/config",
      "homeassistant/text/VdMot/valves_diag_openCount_3/config",
      "homeassistant/text/VdMot/valves_diag_closeCount_3/config",
      "homeassistant/text/VdMot/valves_diag_deadZoneCount_3/config",
      "homeassistant/text/VdMot/valves_diag_moves_3/config",
      "homeassistant/text/VdMot/valves_diag_meanCurrrent_3/config",
      "homeassistant/sensor/VdMot/diag_earlyStops_3/config",
      "homeassistant/sensor/VdMot/diag_cmdRejected_3/config",
      "homeassistant/sensor/VdMot/diag_lastStop_3/config",
      "homeassistant/sensor/VdMot/temps_1/config",
      "homeassistant/sensor/VdMot/volts_Batt/config",
      "homeassistant/sensor/VdMot/diag_stm_uptime/config",
      "homeassistant/binary_sensor/VdMot/diag_calibration_active/config",
      "homeassistant/sensor/VdMot/diag_stm_link/config",
  };
  REQUIRE(v.size() == expected.size());
  for (size_t i = 0; i < v.size(); ++i) CHECK(v[i].topic == expected[i]);

  const std::string dev = kDevice;
  CHECK(v[0].json ==
        "{\"name\":\"state\",\"unique_id\":\"VdMot.common.state\",\"state_topic\":\"VdMot/common/state/value\","
        "\"command_topic\":\"VdMot/common/state/set\",\"icon\":\"mdi:state-machine\"," + dev);
  CHECK(v[1].json ==
        "{\"name\":\"message\",\"unique_id\":\"VdMot.common.message\",\"state_topic\":\"VdMot/common/message/value\","
        "\"command_topic\":\"VdMot/common/message/set\",\"icon\":\"mdi:message\"," + dev);
  CHECK(v[2].json ==
        "{\"name\":\"uptime\",\"unique_id\":\"VdMot.common.uptime\",\"state_topic\":\"VdMot/common/uptime/value\","
        "\"command_topic\":\"VdMot/common/uptime/set\",\"icon\":\"mdi:timelapse\"," + dev);
  CHECK(v[3].json ==
        "{\"name\":\"ip\",\"unique_id\":\"VdMot.common.ip\",\"state_topic\":\"VdMot/common/ip/value\","
        "\"command_topic\":\"VdMot/common/ip/set\",\"icon\":\"mdi:message\"," + dev);
  CHECK(v[4].json ==
        "{\"name\":\"valves.Bad_1.state\",\"unique_id\":\"VdMot.valves.Bad_1.state\","
        "\"state_topic\":\"VdMot/valves/Bad_1/state/value\",\"command_topic\":\"VdMot/valves/Bad_1/state/set\","
        "\"icon\":\"mdi:state-machine\"," + dev);
  CHECK(v[5].json ==
        "{\"name\":\"valves.Bad_1.target\",\"unique_id\":\"VdMot.valves.Bad_1.target\","
        "\"state_topic\":\"VdMot/valves/Bad_1/target/value\",\"command_topic\":\"VdMot/valves/Bad_1/target/set\","
        "\"icon\":\"mdi:valve\",\"device_class\":\"water\",\"reports_position\":true," + dev);
  CHECK(v[6].json ==
        "{\"name\":\"valves.Bad_1.actual\",\"unique_id\":\"VdMot.valves.Bad_1.actual\","
        "\"state_topic\":\"VdMot/valves/Bad_1/actual/value\",\"icon\":\"mdi:valve\","
        "\"state_class\":\"measurement\",\"unit_of_measurement\":\"%\"," + dev);
  CHECK(v[7].json ==
        "{\"name\":\"valves.Bad_1.temp1\",\"unique_id\":\"VdMot.valves.Bad_1.temp1\","
        "\"state_topic\":\"VdMot/valves/Bad_1/temp1/value\",\"icon\":\"mdi:thermometer\","
        "\"device_class\":\"temperature\",\"state_class\":\"measurement\","
        "\"unit_of_measurement\":\"\xC2\xB0" "C\"," + dev);
  CHECK(v[8].json ==
        "{\"name\":\"valves.Bad_1.calibration.date\",\"unique_id\":\"VdMot.valves.Bad_1.calibration.date\","
        "\"state_topic\":\"VdMot/valves/Bad_1/calibration/date/value\","
        "\"command_topic\":\"VdMot/valves/Bad_1/calibration/date/set\",\"icon\":\"mdi:timelapse\"," + dev);
  CHECK(v[9].json ==
        "{\"name\":\"valves.Bad_1.calibration.repetitions\",\"unique_id\":\"VdMot.valves.Bad_1.calibration.repetitions\","
        "\"state_topic\":\"VdMot/valves/Bad_1/calibration/repetitions/value\","
        "\"command_topic\":\"VdMot/valves/Bad_1/calibration/repetitions/set\",\"icon\":\"mdi:valve\"," + dev);
  const char* diag[] = {"openCount", "closeCount", "deadZoneCount", "moves", "meanCurrrent"};
  for (int i = 0; i < 5; ++i) {
    const std::string x = diag[i];
    CHECK(v[10 + i].json ==
          "{\"name\":\"valves.Bad_1.diag." + x + "\",\"unique_id\":\"VdMot.valves.Bad_1.diag." + x +
              "\",\"state_topic\":\"VdMot/valves/Bad_1/diag/" + x +
              "/value\",\"command_topic\":\"VdMot/valves/Bad_1/diag/" + x + "/set\",\"icon\":\"mdi:valve\"," + dev);
  }
  CHECK(v[15].json ==
        "{\"name\":\"diag.Bad_1.earlyStops\",\"unique_id\":\"VdMot.diag.Bad_1.earlyStops\","
        "\"state_topic\":\"VdMot/diag/valves/Bad_1/earlyStops\",\"icon\":\"mdi:alert-outline\","
        "\"state_class\":\"total_increasing\",\"entity_category\":\"diagnostic\"," + dev);
  CHECK(v[16].json ==
        "{\"name\":\"diag.Bad_1.cmdRejected\",\"unique_id\":\"VdMot.diag.Bad_1.cmdRejected\","
        "\"state_topic\":\"VdMot/diag/valves/Bad_1/cmdRejected\",\"icon\":\"mdi:alert-outline\","
        "\"state_class\":\"total_increasing\",\"entity_category\":\"diagnostic\"," + dev);
  CHECK(v[17].json ==
        "{\"name\":\"diag.Bad_1.lastStop\",\"unique_id\":\"VdMot.diag.Bad_1.lastStop\","
        "\"state_topic\":\"VdMot/diag/valves/Bad_1/lastMove\",\"value_template\":\"{{ value_json.stop }}\","
        "\"icon\":\"mdi:stop-circle-outline\",\"entity_category\":\"diagnostic\"," + dev);
  CHECK(v[21].json ==
        "{\"name\":\"valves.3.temp2\",\"unique_id\":\"VdMot.valves.3.temp2\","
        "\"state_topic\":\"VdMot/valves/3/temp2/value\",\"icon\":\"mdi:thermometer\","
        "\"device_class\":\"temperature\",\"state_class\":\"measurement\","
        "\"unit_of_measurement\":\"\xC2\xB0" "C\"," + dev);
  CHECK(v[32].json ==
        "{\"name\":\"temps.1\",\"unique_id\":\"VdMot.28-84-37-94-97-ff-03-23\","
        "\"state_topic\":\"VdMot/temps/1/value/value\",\"icon\":\"mdi:thermometer\","
        "\"device_class\":\"temperature\",\"state_class\":\"measurement\","
        "\"unit_of_measurement\":\"\xC2\xB0" "C\"," + dev);
  CHECK(v[33].json ==
        "{\"name\":\"volts.Batt\",\"unique_id\":\"VdMot.26-00-00-00-00-00-00-01\","
        "\"state_topic\":\"VdMot/sensors/Batt/value/value\",\"device_class\":\"voltage\","
        "\"state_class\":\"measurement\",\"unit_of_measurement\":\"V\"," + dev);
  CHECK(v[34].json ==
        "{\"name\":\"diag.stm.uptime\",\"unique_id\":\"VdMot.diag.stm.uptime\","
        "\"state_topic\":\"VdMot/diag/stm/uptime\",\"device_class\":\"duration\","
        "\"state_class\":\"measurement\",\"unit_of_measurement\":\"s\",\"entity_category\":\"diagnostic\"," + dev);
  CHECK(v[35].json ==
        "{\"name\":\"diag.calibration.active\",\"unique_id\":\"VdMot.diag.calibration.active\","
        "\"state_topic\":\"VdMot/diag/calibration/active\",\"device_class\":\"running\","
        "\"payload_on\":\"1\",\"payload_off\":\"0\",\"entity_category\":\"diagnostic\"," + dev);
  CHECK(v[36].json ==
        "{\"name\":\"diag.stm.link\",\"unique_id\":\"VdMot.diag.stm.link\","
        "\"state_topic\":\"VdMot/diag/stm/link\",\"icon\":\"mdi:lan-connect\","
        "\"entity_category\":\"diagnostic\"," + dev);
  // Every entity names the availability topic and belongs to one device.
  for (const Msg& m : v) CHECK(has(m.json, dev));
}

TEST_CASE("discovery gates") {
  static DiscoveryContext c;
  c = DiscoveryContext{};
  base(c);
  valve(c, 1, "2", true, true);
  CHECK(all(c).size() == 4 + 15 + 3);
  c.publishUptime = false;
  std::vector<Msg> v = all(c);
  CHECK(v.size() == 3 + 15 + 3);
  CHECK(find(v, "homeassistant/text/VdMot/uptime/config") == nullptr);
  c.publishDiag = false;
  v = all(c);
  CHECK(v.size() == 3 + 10 + 3);
  CHECK(find(v, "homeassistant/text/VdMot/valves_diag_moves_2/config") == nullptr);
  CHECK(find(v, "homeassistant/sensor/VdMot/diag_earlyStops_2/config") != nullptr);
  c.newDiag = false;
  v = all(c);
  CHECK(v.size() == 3 + 6);
  CHECK(find(v, "homeassistant/sensor/VdMot/valves_actual_2/config") == nullptr);
  CHECK(find(v, "homeassistant/sensor/VdMot/diag_lastStop_2/config") == nullptr);
  CHECK(find(v, "homeassistant/sensor/VdMot/diag_stm_link/config") == nullptr);
  c.valves[1].hasTemp1 = false;
  v = all(c);
  CHECK(find(v, "homeassistant/sensor/VdMot/valves_temp1_2/config") == nullptr);
  CHECK(find(v, "homeassistant/sensor/VdMot/valves_temp2_2/config") != nullptr);
  c.valves[1].hasTemp2 = false;
  CHECK(all(c).size() == 3 + 4);
  c.valves[1].active = false;
  CHECK(all(c).size() == 3);
  // plainText and publishAllTemps do not change the entity set by themselves.
  c.plainText = false;
  c.publishAllTemps = false;
  CHECK(all(c).size() == 3);

  // Sensors.
  c = DiscoveryContext{};
  base(c);
  c.newDiag = false;
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
  v = all(c);
  REQUIRE(v.size() == 4 + 3 + 4);
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
  // A one-character volt id counts as well.
  copyString(c.volts[2].id, sizeof c.volts[2].id, "z");
  v = all(c);
  CHECK(has(v[9].json, "\"unique_id\":\"VdMot.z\""));
}

TEST_CASE("discovery topic variants: station, root, separate, ip, escaping") {
  static DiscoveryContext c;
  c = DiscoveryContext{};
  base(c);
  c.newDiag = false;
  c.publishDiag = false;
  copyString(c.topics.station, sizeof c.topics.station, "My Station");
  valve(c, 0, "1");
  std::vector<Msg> v = all(c);
  REQUIRE(v.size() == 8);
  CHECK(v[0].topic == "homeassistant/text/My Station/state/config");
  CHECK(has(v[0].json, "\"unique_id\":\"My_Station.common.state\""));
  CHECK(has(v[0].json, "\"state_topic\":\"My Station/common/state/value\""));
  CHECK(has(v[0].json, "\"identifiers\":\"My Station\",\"name\":\"My Station\""));
  CHECK(has(v[0].json, "\"availability_topic\":\"My Station/status\""));

  copyString(c.topics.station, sizeof c.topics.station, "VdMot");
  c.topics.pathAsRoot = true;
  v = all(c);
  CHECK(v[0].topic == "homeassistant/text/VdMot/state/config");
  CHECK(has(v[0].json, "\"state_topic\":\"/VdMot/common/state/value\""));
  CHECK(has(v[0].json, "\"command_topic\":\"/VdMot/common/state/set\""));
  CHECK(has(v[0].json, "\"availability_topic\":\"/VdMot/status\""));
  CHECK(has(v[5].json, "\"command_topic\":\"/VdMot/valves/1/target/set\""));

  c.topics.pathAsRoot = false;
  c.topics.separate = false;
  v = all(c);
  CHECK(has(v[0].json, "\"state_topic\":\"VdMot/common/state\",\"command_topic\":\"VdMot/common/state/set\""));
  CHECK(has(v[5].json, "\"state_topic\":\"VdMot/valves/1/target\",\"command_topic\":\"VdMot/valves/1/target\""));

  c.topics.separate = true;
  c.ip[0] = '\0';
  copyString(c.swVersion, sizeof c.swVersion, "2.0\"x\\");
  v = all(c);
  CHECK_FALSE(has(v[0].json, "configuration_url"));
  CHECK(has(v[0].json, "\"sw_version\":\"2.0\\\"x\\\\\""));
  // Single-character values are values.
  copyString(c.ip, sizeof c.ip, "1");
  v = all(c);
  CHECK(has(v[0].json, "\"configuration_url\":\"http://1/\""));
  // Unterminated ip / version arrays are read bounded.
  memset(c.ip, '1', sizeof c.ip);
  memset(c.swVersion, 'v', sizeof c.swVersion);
  v = all(c);
  CHECK(has(v[0].json, "\"configuration_url\":\"http://" + std::string(15, '1') + "/\""));
  CHECK(has(v[0].json, "\"sw_version\":\"" + std::string(31, 'v') + "\""));
}

TEST_CASE("discovery refuses a missing or unsafe station") {
  static DiscoveryContext c;
  c = DiscoveryContext{};
  base(c);
  valve(c, 0, "1");
  for (const char* bad : {"", "a+b", "a/b", "x\xc3"}) {
    copyString(c.topics.station, sizeof c.topics.station, bad);
    DiscoveryIterator it(c);
    DiscoveryMessage m;
    char buf[kDiscoveryPayloadMax + 1];
    JsonWriter jw(buf, sizeof buf);
    CHECK_FALSE(it.next(m, jw));
    CHECK(jw.ok());
    CHECK(m.topic[0] == '\0');
    DropListIterator d(c);
    CHECK_FALSE(d.next(m));
    CHECK_FALSE(current(c, "homeassistant/text/" + std::string(bad) + "/state/config"));
  }
  memset(c.topics.station, 'x', sizeof c.topics.station);  // unterminated
  DiscoveryIterator it(c);
  DiscoveryMessage m;
  char buf[kDiscoveryPayloadMax + 1];
  JsonWriter jw(buf, sizeof buf);
  CHECK_FALSE(it.next(m, jw));
  CHECK(jw.ok());  // off, not a failed entity
  DropListIterator d(c);
  CHECK_FALSE(d.next(m));
  // The longest valid station works.
  memset(c.topics.station, 'x', kStationNameMax);
  c.topics.station[kStationNameMax] = '\0';
  DiscoveryIterator ok(c);
  REQUIRE(ok.next(m, jw));
  CHECK(std::string(m.topic) == "homeassistant/text/" + std::string(kStationNameMax, 'x') + "/state/config");
}

TEST_CASE("discovery skips an entity that cannot be built and continues") {
  static DiscoveryContext c;
  c = DiscoveryContext{};
  base(c);
  c.newDiag = false;
  c.publishDiag = false;
  c.publishUptime = false;
  valve(c, 0, "a/b");  // invalid segment
  valve(c, 1, "2");
  DiscoveryIterator it(c);
  DiscoveryMessage m;
  char buf[kDiscoveryPayloadMax + 1];
  JsonWriter jw(buf, sizeof buf);
  for (int i = 0; i < 3; ++i) REQUIRE(it.next(m, jw));
  CHECK(it.position() == 4);
  // Valve 0: four entities, each reported as a failure.
  for (int i = 0; i < 4; ++i) {
    CHECK_FALSE(it.next(m, jw));
    CHECK_FALSE(jw.ok());
  }
  REQUIRE(it.next(m, jw));
  CHECK(jw.ok());
  CHECK(std::string(m.topic) == "homeassistant/text/VdMot/valves_state_2/config");
  int rest = 0;
  while (it.next(m, jw)) ++rest;
  CHECK(rest == 3);
  CHECK(jw.ok());
  CHECK(it.position() == 229);
  CHECK_FALSE(it.next(m, jw));

  // Unterminated / oversize segments are rejected as well.
  memset(c.valves[0].segment, 'x', sizeof c.valves[0].segment);
  c.valves[1].active = false;
  it.restart();
  CHECK(it.position() == 0);
  for (int i = 0; i < 3; ++i) REQUIRE(it.next(m, jw));
  CHECK_FALSE(it.next(m, jw));
  CHECK_FALSE(jw.ok());
  copyString(c.valves[0].segment, sizeof c.valves[0].segment, "");
  it.restart();
  for (int i = 0; i < 3; ++i) REQUIRE(it.next(m, jw));
  CHECK_FALSE(it.next(m, jw));
  CHECK_FALSE(jw.ok());

  // A writer too small for a payload: failure, then the iterator moves on.
  copyString(c.valves[0].segment, sizeof c.valves[0].segment, "1");
  it.restart();
  char small[200];
  JsonWriter sw(small, sizeof small);
  CHECK_FALSE(it.next(m, sw));
  CHECK_FALSE(sw.ok());
  CHECK(it.position() == 1);
  // A writer larger than the MQTT buffer: payloads above kDiscoveryPayloadMax fail.
  static char big[4096];
  JsonWriter bw(big, sizeof big);
  it.restart();
  REQUIRE(it.next(m, bw));
  CHECK(bw.length() <= kDiscoveryPayloadMax);
}

TEST_CASE("discovery with every entity enabled fits the MQTT buffer") {
  static DiscoveryContext c;
  c = DiscoveryContext{};
  base(c);
  copyString(c.topics.station, sizeof c.topics.station, "abcdefghijklmnopqrst");
  c.topics.pathAsRoot = true;
  copyString(c.ip, sizeof c.ip, "255.255.255.255");
  copyString(c.swVersion, sizeof c.swVersion, "2.0.0-revamped-dev-0123456789ab");
  for (uint8_t i = 0; i < kValveCount; ++i) {
    char seg[kSegmentMax + 1];
    snprintf(seg, sizeof seg, "Valve_%04u", static_cast<unsigned>(i));
    valve(c, i, seg, true, true);
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
  CHECK(v.size() == 229);
  size_t longest = 0;
  for (const Msg& m : v) {
    if (m.json.size() > longest) longest = m.json.size();
    CHECK(m.topic.size() <= kDiscoveryTopicMax);
    CHECK(current(c, m.topic));
  }
  CHECK(longest <= kDiscoveryPayloadMax);
  CHECK(longest > 600);  // sanity: long names really used
}

TEST_CASE("DROP list: legacy entities of both segment forms, once each") {
  static DiscoveryContext c;
  c = DiscoveryContext{};
  base(c);
  valve(c, 0, "Bad_1");
  c.valves[0].active = false;  // inactive valves are cleaned up too
  copyString(c.valves[2].segment, sizeof c.valves[2].segment, "3");  // unnamed
  copyString(c.valves[4].segment, sizeof c.valves[4].segment, "2");  // named like valve 2's number
  copyString(c.valves[5].segment, sizeof c.valves[5].segment, "a/b");  // invalid: index form only
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
}

TEST_CASE("stale-topic check for legacy /HADiscovery.cfg lines") {
  static DiscoveryContext c;
  c = DiscoveryContext{};
  base(c);
  valve(c, 0, "Bad_1", true);
  temp(c, 0, "1", "28-84-37-94-97-ff-03-23");
  CHECK(current(c, "homeassistant/text/VdMot/state/config"));
  CHECK(current(c, "homeassistant/text/VdMot/state/config\n"));
  CHECK(current(c, "homeassistant/text/VdMot/state/config\r\n"));
  CHECK(current(c, "homeassistant/text/VdMot/state/config \t"));
  CHECK(current(c, "homeassistant/valve/VdMot/valves_target_Bad_1/config"));
  CHECK(current(c, "homeassistant/sensor/VdMot/valves_temp1_Bad_1/config"));
  CHECK(current(c, "homeassistant/sensor/VdMot/temps_1/config"));
  CHECK(current(c, "homeassistant/sensor/VdMot/diag_stm_link/config"));
  CHECK_FALSE(current(c, "homeassistant/sensor/VdMot/valves_temp2_Bad_1/config"));
  CHECK_FALSE(current(c, "homeassistant/climate/VdMot/climate_Bad_1/config"));
  CHECK_FALSE(current(c, "homeassistant/select/VdMot/heatControl/config"));
  CHECK_FALSE(current(c, "homeassistant/text/VdMot/valves_state_2/config"));   // inactive
  CHECK_FALSE(current(c, "homeassistant/text/Old/state/config"));              // renamed station
  CHECK_FALSE(current(c, "homeassistant/sensor/VdMot/state/config"));          // other component
  CHECK_FALSE(current(c, "homeassistant/text/VdMot/state/confi"));
  CHECK_FALSE(current(c, "homeassistant/text/VdMot/state/config/"));
  CHECK_FALSE(current(c, " homeassistant/text/VdMot/state/config"));
  CHECK_FALSE(current(c, ""));
  CHECK_FALSE(current(c, "\r\n"));
  CHECK_FALSE(discoveryTopicIsCurrent(c, nullptr, 5));
  CHECK_FALSE(current(c, std::string(200, 'a')));
  c.publishUptime = false;
  CHECK_FALSE(current(c, "homeassistant/text/VdMot/uptime/config"));
  c.newDiag = false;
  CHECK_FALSE(current(c, "homeassistant/sensor/VdMot/diag_stm_link/config"));
  // len is authoritative.
  const char line[] = "homeassistant/text/VdMot/ip/configXYZ";
  CHECK(discoveryTopicIsCurrent(c, line, strlen(line) - 3));
}

TEST_CASE("stale-topic check keeps valve temp configs while the sensors are unknown") {
  static DiscoveryContext c;
  c = DiscoveryContext{};
  base(c);
  valve(c, 0, "Bad_1", false);  // no temps reported (yet)
  valve(c, 1, "Flur", false);
  c.valves[1].active = false;
  const std::string t1 = "homeassistant/sensor/VdMot/valves_temp1_Bad_1/config";
  const std::string t2 = "homeassistant/sensor/VdMot/valves_temp2_Bad_1/config";
  CHECK_FALSE(current(c, t1));  // known: no sensor -> stale
  CHECK_FALSE(current(c, t2));
  c.valves[0].tempsKnown = false;  // STM data not settled: never deleted on a guess
  c.valves[1].tempsKnown = false;
  CHECK(current(c, t1));
  CHECK(current(c, t2));
  CHECK_FALSE(current(c, "homeassistant/sensor/VdMot/valves_temp1_Flur/config"));  // inactive
  CHECK(current(c, "homeassistant/valve/VdMot/valves_target_Bad_1/config"));
  // ... but nothing is published for them.
  DiscoveryIterator it(c);
  DiscoveryMessage m;
  static char buf[kDiscoveryPayloadMax + 1];
  size_t n = 0;
  for (;;) {
    JsonWriter jw(buf, sizeof buf);
    if (!it.next(m, jw)) break;
    ++n;
    CHECK(std::string(m.topic) != t1);
    CHECK(std::string(m.topic) != t2);
  }
  CHECK(n > 0);
}

TEST_CASE("stale-topic check fuzz: only exact current topics match") {
  static DiscoveryContext c;
  c = DiscoveryContext{};
  base(c);
  valve(c, 0, "Bad_1", true, true);
  valve(c, 5, "6");
  temp(c, 2, "3", "28-84-37-94-97-ff-03-23");
  volt(c, 1, "Bat", "Bat", "V", "26-00-00-00-00-00-00-01");
  const std::vector<Msg> cur = all(c);
  REQUIRE(cur.size() > 20);
  uint32_t seed = 0x5EED0002u;
  auto rnd = [&seed]() {
    seed = seed * 1664525u + 1013904223u;
    return seed >> 8;
  };
  size_t hits = 0;
  for (int iter = 0; iter < 20000; ++iter) {
    std::string t = cur[rnd() % cur.size()].topic;
    const uint32_t op = rnd() % 5;
    if (op == 0) {
      t[rnd() % t.size()] = static_cast<char>(rnd() & 0xFF);  // may or may not change it
    } else if (op == 1) {
      t.erase(rnd() % t.size(), 1);
    } else if (op == 2) {
      t.insert(t.begin() + static_cast<long>(rnd() % (t.size() + 1)), static_cast<char>(rnd() & 0xFF));
    } else if (op == 3) {
      t.assign(rnd() % 160, '\0');
      for (char& ch : t) ch = static_cast<char>(rnd() & 0xFF);
    }
    // op 4: unchanged.
    std::string trimmed = t;
    while (!trimmed.empty() && (trimmed.back() == '\r' || trimmed.back() == '\n' ||
                                trimmed.back() == ' ' || trimmed.back() == '\t')) {
      trimmed.pop_back();
    }
    const bool expect = find(cur, trimmed) != nullptr;
    const bool got = current(c, t);
    REQUIRE(got == expect);
    if (got) ++hits;
  }
  CHECK(hits > 3000);
}

TEST_CASE("discovery fuzz: random context bytes never overflow or emit bad JSON framing") {
  static DiscoveryContext c;
  uint32_t seed = 0x5EED0003u;
  auto rnd = [&seed]() {
    seed = seed * 1664525u + 1013904223u;
    return seed >> 8;
  };
  // Printable-heavy random bytes, sometimes filling the field without a NUL.
  auto fill = [&rnd](char* f, size_t n) {
    const size_t len = rnd() % (n + 1);
    for (size_t k = 0; k < len && k < n; ++k) {
      f[k] = rnd() % 4 == 0 ? static_cast<char>(rnd() & 0xFF) : static_cast<char>(0x20 + rnd() % 95);
    }
    if (len < n) f[len] = '\0';
  };
  size_t messages = 0;
  for (int iter = 0; iter < 300; ++iter) {
    c = DiscoveryContext{};
    base(c);
    if (rnd() % 3 == 0) fill(c.topics.station, sizeof c.topics.station);
    if (rnd() % 3 == 0) fill(c.ip, sizeof c.ip);
    if (rnd() % 3 == 0) fill(c.swVersion, sizeof c.swVersion);
    c.topics.pathAsRoot = rnd() % 2;
    c.publishDiag = rnd() % 2;
    c.newDiag = rnd() % 2;
    c.publishUptime = rnd() % 2;
    for (auto& v : c.valves) {
      v.active = rnd() % 2;
      v.hasTemp1 = rnd() % 2;
      v.hasTemp2 = rnd() % 2;
      if (rnd() % 2) fill(v.segment, sizeof v.segment);
      else snprintf(v.segment, sizeof v.segment, "%u", static_cast<unsigned>(rnd() % 12 + 1));
    }
    for (auto* arr : {c.temps, c.volts}) {
      const size_t n = arr == c.temps ? kTempSlotCount : kVoltSlotCount;
      for (size_t i = 0; i < n; ++i) {
        DiscoveryContext::Sensor& s = arr[i];
        s.active = rnd() % 2;
        s.published = rnd() % 2;
        fill(s.segment, sizeof s.segment);
        fill(s.name, sizeof s.name);
        fill(s.id, sizeof s.id);
        fill(s.unit, sizeof s.unit);
      }
    }
    DiscoveryIterator it(c);
    DiscoveryMessage m;
    char buf[kDiscoveryPayloadMax + 1];
    JsonWriter jw(buf, sizeof buf);
    // An unbuildable entity stops next() (jw not ok); the caller moves on.
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
        REQUIRE(std::string(m.topic).rfind("homeassistant/", 0) == 0);
        REQUIRE(has(m.topic, "/config"));
        // Control bytes never appear raw in a JSON payload.
        bool clean = true;
        for (char ch : j) clean = clean && static_cast<unsigned char>(ch) >= 0x20;
        REQUIRE(clean);
      } else if (jw.ok()) {
        break;  // exhausted
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
    (void)discoveryTopicIsCurrent(c, line, strlen(line));
  }
  CHECK(messages > 1000);
}

TEST_CASE("discovery: a volt unit of the full 8 characters is kept whole") {
  static DiscoveryContext c;
  c = DiscoveryContext{};
  base(c);
  volt(c, 0, "v1", "Pump", "kWh/m3ab", "26-11-22-33-44-55-66-29");
  REQUIRE(strlen(c.volts[0].unit) == 8);
  const std::vector<Msg> v = all(c);
  const Msg* m = find(v, "homeassistant/sensor/VdMot/volts_v1/config");
  REQUIRE(m != nullptr);
  CHECK(has(m->json, "\"unit_of_measurement\":\"kWh/m3ab\""));
  // Unterminated unit field: cut to 8 characters.
  memset(c.volts[0].unit, 'u', sizeof c.volts[0].unit);
  const std::vector<Msg> w = all(c);
  m = find(w, "homeassistant/sensor/VdMot/volts_v1/config");
  REQUIRE(m != nullptr);
  CHECK(has(m->json, "\"unit_of_measurement\":\"uuuuuuuu\","));
}
