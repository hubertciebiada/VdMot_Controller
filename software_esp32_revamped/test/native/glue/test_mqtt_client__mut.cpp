// src/mqtt_client.cpp, pass by pass: the full publish layout, the on-change windows and their
// order, the diag budget, values at their boundaries (offsets, buffer sizes, stale sensors,
// paced counters), events, inbound edge cases, connection back-off and the discovery plans.
#include <stddef.h>
#include <string.h>

#include <functional>
#include <string>
#include <vector>

#include <vdm/event_log.h>
#include <vdm/ha_discovery.h>
#include <vdm/json_writer.h>
#include <vdm/mqtt_topics.h>

#include "glue_test.h"
#include "logger.h"
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
  sib::storage().haCleanupDone = true;
  sib::storage().haLayout = 2;
  sib::net().up = true;
  fakes::fs().mounted = true;
  return c;
}

void runTask(long passes) {
  fakes::rtos().stopAfterYields(passes);
  CHECK_THROWS_AS(mqtt::task(nullptr), fakes::YieldLimit);
}

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

size_t count(const std::string& topic) { return payloads(topic).size(); }

size_t countPrefix(const std::string& prefix, size_t from = 0) {
  size_t n = 0;
  const std::vector<fakes::MqttMessage>& p = fakes::mqtt().published;
  for (size_t i = from; i < p.size(); ++i) n += p[i].topic.compare(0, prefix.size(), prefix) == 0;
  return n;
}

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

std::vector<vdm::Event> rejected() {
  return sib::logger().withCode(vdm::EventCode::MqttCommandRejected);
}

vdm::OneWireId oneWire(uint8_t family, uint8_t last) {
  vdm::OneWireId id;
  id.b[0] = family;
  id.b[7] = last;
  return id;
}

// Pass tracking: the clock of every task pass and the published messages at its end. `each`
// runs after every pass with the clock of the next one.
struct Track {
  std::vector<uint64_t> at;
  std::vector<size_t> end;
  std::function<void(size_t passes, uint64_t now)> each;
};
Track& track() {
  static Track t;
  return t;
}

void startTracking(std::function<void(size_t passes, uint64_t now)> each = nullptr) {
  track() = Track{};
  track().each = std::move(each);
  fakes::rtos().onDelay = [](uint32_t ticks) {
    Track& t = track();
    t.at.push_back(fakes::nowMs() - ticks);
    t.end.push_back(fakes::mqtt().published.size());
    if (t.each) t.each(t.at.size(), fakes::nowMs());
  };
}

size_t passOf(size_t message) {
  const Track& t = track();
  for (size_t k = 0; k < t.end.size(); ++k) {
    if (t.end[k] > message) return k;
  }
  return SIZE_MAX;
}

std::vector<size_t> passesOf(const std::string& topic) {
  std::vector<size_t> v;
  const std::vector<fakes::MqttMessage>& p = fakes::mqtt().published;
  for (size_t i = 0; i < p.size(); ++i) {
    if (p[i].topic == topic) v.push_back(passOf(i));
  }
  return v;
}

std::vector<uint64_t> timesOf(const std::string& topic) {
  std::vector<uint64_t> v;
  for (size_t k : passesOf(topic)) v.push_back(k < track().at.size() ? track().at[k] : UINT64_MAX);
  return v;
}

// Publish times of `topic` in the open interval (from, to).
std::vector<uint64_t> timesBetween(const std::string& topic, uint64_t from, uint64_t to) {
  std::vector<uint64_t> v;
  for (uint64_t t : timesOf(topic)) {
    if (t > from && t < to) v.push_back(t);
  }
  return v;
}

size_t indexOf(const std::string& topic) {
  const std::vector<fakes::MqttMessage>& p = fakes::mqtt().published;
  for (size_t i = 0; i < p.size(); ++i) {
    if (p[i].topic == topic) return i;
  }
  return SIZE_MAX;
}

const std::string kState = "VdMot/common/state/value";
const std::string kUptime = "VdMot/common/uptime/value";
const std::string kMessage = "VdMot/common/message/value";
const std::string kIp = "VdMot/common/ip/value";

}  // namespace

// ---------------------------------------------------------------- full publish

TEST_CASE("mqtt full publish: common and a valve, one valve per pass, four other slots per pass") {
  glue::begin();
  useMqtt();
  vdm::StmSnapshot& s = linkUp();
  s.haveStatus = true;
  s.status.uptimeS = 77;
  startTracking();
  mqtt::begin();
  runTask(40);
  // Pass 0 connects; the full publish takes passes 1 (common + valve 1), 2..12 (valves 2..12),
  // 13..23 (34 temp, 8 volt, STM and system slots, four per pass).
  CHECK(passesOf(kIp) == std::vector<size_t>{1});
  CHECK(passesOf(kUptime) == std::vector<size_t>{1});
  CHECK(passesOf("VdMot/valves/1/state/value") == std::vector<size_t>{1});
  CHECK(passesOf("VdMot/diag/stm/uptime") == std::vector<size_t>{23});
  CHECK(payloads("VdMot/diag/stm/uptime") == std::vector<std::string>{"77"});
  CHECK(passesOf("VdMot/stm/status") == std::vector<size_t>{23});
  CHECK(passesOf("VdMot/failsafe") == std::vector<size_t>{23});
  CHECK(indexOf("VdMot/diag/stm/uptime") < indexOf("VdMot/stm/status"));
}

TEST_CASE("mqtt full publish: no uptime topic without upTime") {
  glue::begin();
  vdm::Config& c = useMqtt();
  c.mqtt.upTime = false;
  settle();
  CHECK(count(kUptime) == 0);
  CHECK(count(kState) == 1);
}

TEST_CASE("mqtt full publish: the STM uptime needs newDiag, protocol 2 and a status") {
  glue::begin();
  useMqtt();
  vdm::StmSnapshot& s = linkUp();
  s.proto = 1;
  s.haveStatus = true;
  settle();
  CHECK(count("VdMot/diag/stm/uptime") == 0);
  CHECK(count("VdMot/diag/stm/resets") == 0);
  CHECK(count("VdMot/diag/stm/started") == 0);
}

TEST_CASE("mqtt full publish: no STM uptime without a status") {
  glue::begin();
  useMqtt();
  linkUp();
  settle();
  CHECK(count("VdMot/diag/stm/uptime") == 0);
  CHECK(count("VdMot/diag/stm/resets") == 0);
}

TEST_CASE("mqtt full publish: no STM uptime without newDiag") {
  glue::begin();
  vdm::Config& c = useMqtt();
  c.mqtt.newDiag = false;
  vdm::StmSnapshot& s = linkUp();
  s.haveStatus = true;
  settle();
  CHECK(count("VdMot/diag/stm/uptime") == 0);
  CHECK(count(kState) == 1);
}

TEST_CASE("mqtt full publish: common/ip once per connection") {
  glue::begin();
  useMqtt();
  sib::app().uptimeS = 1000;
  mqtt::begin();
  runTask(1 + 1245);  // three full publishes
  CHECK(count(kIp) == 1);
  CHECK(count(kState) == 3);
  mqtt::requestReconnect();
  runTask(40);
  CHECK(count(kIp) == 2);
}

// ---------------------------------------------------------------- on change

namespace {

// Valve 1 with a calibration end, temp slot 1 and volt slot 1 on the bus, STM with a status,
// a fixed ESP uptime; the broker session starts with tracking at t0.
uint64_t steadySetup(vdm::Config& c, vdm::StmSnapshot*& out) {
  c.temps[0].active = true;
  c.temps[0].id = oneWire(0x28, 1);
  c.volts[0].id = oneWire(0x26, 2);
  vdm::copyString(c.volts[0].unit, sizeof c.volts[0].unit, "V");
  vdm::StmSnapshot& s = linkUp();
  s.haveStatus = true;
  s.status.uptimeS = 500;
  s.tempCount = 1;
  s.temps[0].id = c.temps[0].id;
  s.temps[0].raw = 215;
  s.temps[0].seen = true;
  s.temps[0].lastSeenMs = static_cast<uint32_t>(fakes::nowMs());
  s.voltCount = 1;
  s.volts[0].id = c.volts[0].id;
  s.volts[0].vad = 1234;
  s.volts[0].seen = true;
  s.volts[0].lastSeenMs = static_cast<uint32_t>(fakes::nowMs());
  sib::app().uptimeS = 1000;
  vdm::LocalTime& lt = sib::net().localTime;
  lt.valid = true;
  lt.year = 2026;
  lt.month = 9;
  lt.mday = 21;
  lt.wday = 1;
  lt.hour = 14;
  lt.epoch = 1790000000;
  // A calibration ended before the broker session.
  sib::net().up = false;
  s.valves[0].calibrating = true;
  publishSnap();
  mqtt::begin();
  runTask(1);
  s.valves[0].calibrating = false;
  publishSnap();
  runTask(1);
  sib::net().up = true;
  out = &s;
  return fakes::nowMs();
}

}  // namespace

TEST_CASE("mqtt on change: steady values go out with the full publishes only") {
  glue::begin();
  vdm::Config& c = useMqtt();
  vdm::StmSnapshot* s = nullptr;
  steadySetup(c, s);
  s->lease.state = vdm::LeaseState::Expired;
  publishSnap();
  startTracking();
  runTask(1 + 1245);  // full publishes at +0.1, +10.1 and +20.1 s
  for (const char* t :
       {"VdMot/common/state/value", "VdMot/common/uptime/value", "VdMot/common/message/value",
        "VdMot/valves/1/state/value", "VdMot/valves/1/calibration/date/value",
        "VdMot/temps/1/value/value", "VdMot/sensors/1/value/value", "VdMot/diag/stm/uptime",
        "VdMot/stm/status", "VdMot/failsafe"}) {
    CAPTURE(t);
    CHECK(count(t) == 3);
  }
  CHECK(last("VdMot/failsafe") == "1");
  CHECK(last("VdMot/valves/1/calibration/date/value") == "Monday, September 21.2026 14:00:00");
}

TEST_CASE("mqtt on change: each change goes out after minDelayS, before the next full publish") {
  glue::begin();
  vdm::Config& c = useMqtt();
  c.valves[1].active = true;
  vdm::StmSnapshot* sp = nullptr;
  const uint64_t t0 = steadySetup(c, sp);
  vdm::StmSnapshot& s = *sp;
  s.valves[1].known = true;
  publishSnap();
  // Full publishes start at t0 + 100 + 10000 n and end 440 ms later.
  auto fullEnd = [t0](int n) { return t0 + 540 + 10000u * static_cast<uint64_t>(n); };
  startTracking([&](size_t, uint64_t now) {
    if (now == fullEnd(0) + 500) {  // cycle 0: message, temp value, volt value, calibration
      logger::log(vdm::EventCode::LowHeap, vdm::kNoValve, 1, 2);
      s.temps[0].raw = 225;
      s.volts[0].vad = 1300;
      s.valves[0].calibrating = true;
      publishSnap();
    } else if (now == fullEnd(0) + 1000) {
      s.valves[0].calibrating = false;
      publishSnap();
    } else if (now == fullEnd(1) + 500) {  // cycle 1: state, temp and volt failed
      s.valves[1].health = vdm::kHealthStale;
      s.temps[0].raw = vdm::kTempReadError;
      s.volts[0].vad = vdm::kVadFailed;
      publishSnap();
    } else if (now == fullEnd(2) + 500) {  // cycle 2: ESP uptime
      sib::app().uptimeS = 1001;
    }
  });
  runTask(1 + 1795);  // to t0 + 36 s
  const std::vector<uint64_t> ends = timesOf("VdMot/stm/status");
  REQUIRE(ends.size() == 4);
  for (int n = 0; n < 4; ++n) CHECK(ends[static_cast<size_t>(n)] == fullEnd(n));
  // Two slots per pass: common and a valve first, temp and volt 20 ms later.
  struct Expect {
    int cycle;
    const char* topic;
    uint64_t after;
    const char* payload;
  };
  const Expect rows[] = {
      {0, "VdMot/common/message/value", 5000, "low heap (free 1, min 2)"},
      {0, "VdMot/valves/1/calibration/date/value", 5000, nullptr},
      {0, "VdMot/temps/1/value/value", 5020, "22.5"},
      {0, "VdMot/sensors/1/value/value", 5020, "13.000"},
      {1, "VdMot/common/state/value", 5000, "info"},
      {1, "VdMot/temps/1/value/value", 5020, "failed"},
      {1, "VdMot/sensors/1/value/value", 5020, "failed"},
      {2, "VdMot/common/uptime/value", 5000, "0d 0:16:41"},
  };
  for (const Expect& e : rows) {
    CAPTURE(e.cycle);
    CAPTURE(e.topic);
    const uint64_t from = fullEnd(e.cycle);
    const std::vector<uint64_t> t = timesBetween(e.topic, from, from + 9560);
    REQUIRE(t.size() == 1);
    CHECK(t[0] == from + e.after);
    if (e.payload != nullptr) {
      const std::vector<std::string> p = payloads(e.topic);
      const std::vector<uint64_t> all = timesOf(e.topic);
      for (size_t i = 0; i < all.size(); ++i) {
        if (all[i] == t[0]) CHECK(p[i] == e.payload);
      }
    }
  }
  // Unchanged slots stay quiet in the same windows.
  CHECK(timesBetween("VdMot/common/state/value", fullEnd(0), fullEnd(0) + 9560).size() == 1);
  CHECK(timesBetween("VdMot/temps/1/value/value", fullEnd(2), fullEnd(2) + 9560).empty());
  CHECK(timesBetween("VdMot/valves/2/state/value", fullEnd(0), fullEnd(0) + 9560).empty());
}

TEST_CASE("mqtt on change: two slots per pass, in slot order from common") {
  glue::begin();
  vdm::Config& c = useMqtt();
  c.valves[1].active = true;
  c.valves[2].active = true;
  vdm::StmSnapshot& s = linkUp();
  s.valves[1].known = true;
  s.valves[2].known = true;
  sib::app().uptimeS = 1000;
  publishSnap();
  const uint64_t t0 = fakes::nowMs();
  startTracking([&](size_t, uint64_t now) {
    if (now != t0 + 1040) return;
    logger::log(vdm::EventCode::LowHeap, vdm::kNoValve, 1, 2);
    for (int v = 0; v < 3; ++v) s.valves[v].position = static_cast<uint8_t>(10 + v);
    publishSnap();
  });
  mqtt::begin();
  runTask(1 + 300);
  const std::vector<size_t> st = passesOf(kState);
  REQUIRE(st.size() == 2);
  const size_t p = st[1];
  CHECK(track().at[p] == t0 + 5540);
  CHECK(passesOf("VdMot/valves/1/actual/value") == std::vector<size_t>{1, p});
  CHECK(passesOf("VdMot/valves/2/actual/value") == std::vector<size_t>{2, p + 1});
  CHECK(passesOf("VdMot/valves/3/actual/value") == std::vector<size_t>{3, p + 1});
}

TEST_CASE("mqtt on change: the system state follows the STM safe mode of a v3 status only") {
  glue::begin();
  useMqtt();
  vdm::StmSnapshot& s = linkUp();
  s.status.v3 = true;
  s.status.safeMode = true;  // without a status
  settle();
  CHECK(last(kState) == "ok");
  s.haveStatus = true;
  s.status.v3 = false;  // not a v3 status
  publishSnap();
  runTask(300);
  CHECK(last(kState) == "ok");
  s.status.v3 = true;
  publishSnap();
  runTask(300);
  CHECK(last(kState) == "error");
}

// ---------------------------------------------------------------- values

TEST_CASE("mqtt values: valve temperatures with the offset of their 1-based slot") {
  glue::begin();
  vdm::Config& c = useMqtt();
  c.valves[1].active = true;
  c.temps[0].offset = 3;
  c.temps[1].offset = 5;
  c.temps[2].offset = 13;
  c.temps[32].offset = 7;
  c.temps[33].offset = 11;
  // Non-zero bytes next to the slot array.
  vdm::copyString(c.valves[11].name, sizeof c.valves[11].name, "ZZZZZZZZZZ");
  vdm::copyString(c.volts[0].name, sizeof c.volts[0].name, "YYYYYYYYYY");
  c.volts[0].offset = 1.1f;
  vdm::StmSnapshot& s = linkUp();
  s.valves[0].temp1 = 200;
  s.valves[0].sensorSlot[0] = 1;
  s.valves[0].temp2 = 200;
  s.valves[0].sensorSlot[1] = 34;
  s.valves[1].known = true;
  s.valves[1].temp1 = 200;
  s.valves[1].sensorSlot[0] = 0;
  s.valves[1].temp2 = 200;
  s.valves[1].sensorSlot[1] = 35;
  publishSnap();
  settle();
  CHECK(last("VdMot/valves/1/temp1/value") == "20.3");
  CHECK(last("VdMot/valves/1/temp2/value") == "21.1");
  CHECK(last("VdMot/valves/2/temp1/value") == "20.0");
  CHECK(last("VdMot/valves/2/temp2/value") == "20.0");
}

TEST_CASE("mqtt values: valve state, diag counters, no calibration date, inactive valves") {
  glue::begin();
  vdm::Config& c = useMqtt();
  (void)c;
  vdm::StmSnapshot& s = linkUp();
  vdm::ValveState& v = s.valves[0];
  v.status = 1;
  v.meanCurrent = 12;
  v.openCount = 0x80000000u;
  v.closeCount = 7;
  v.moves = 3;
  v.deadZone = -4;
  s.valves[1].known = true;  // inactive
  publishSnap();
  settle();
  CHECK(last("VdMot/valves/1/state/value") == "idle");
  CHECK(last("VdMot/valves/1/diag/meanCurrrent/value") == "12");
  CHECK(last("VdMot/valves/1/diag/openCount/value") == "-2147483648");
  CHECK(last("VdMot/valves/1/diag/closeCount/value") == "7");
  CHECK(last("VdMot/valves/1/diag/deadZoneCount/value") == "-4");
  CHECK(count("VdMot/valves/1/calibration/date/value") == 0);
  CHECK(countPrefix("VdMot/valves/2/") == 0);
}

TEST_CASE("mqtt values: without separate a valve without a published target has no echo") {
  glue::begin();
  vdm::Config& c = useMqtt();
  c.mqtt.separate = false;
  linkUp();
  settle();
  REQUIRE(count("VdMot/valves/1/target") == 0);
  deliver("VdMot/valves/1/target", "0");
  runTask(2);
  REQUIRE(sib::app().submitted.size() == 1);
  CHECK(sib::app().submitted[0].pos == 0);
}

TEST_CASE("mqtt values: sensor names of 10 characters are topic segments") {
  glue::begin();
  vdm::Config& c = useMqtt();
  vdm::copyString(c.temps[0].name, sizeof c.temps[0].name, "ABCDEFGHIJ");
  c.temps[0].active = true;
  c.temps[0].id = oneWire(0x28, 1);
  vdm::copyString(c.volts[0].name, sizeof c.volts[0].name, "KLMNOPQRST");
  c.volts[0].id = oneWire(0x26, 2);
  vdm::StmSnapshot& s = linkUp();
  s.tempCount = 1;
  s.temps[0].id = c.temps[0].id;
  s.temps[0].raw = 215;
  s.temps[0].seen = true;
  s.voltCount = 1;
  s.volts[0].id = c.volts[0].id;
  s.volts[0].vad = 1234;
  s.volts[0].seen = true;
  publishSnap();
  settle();
  CHECK(last("VdMot/temps/ABCDEFGHIJ/value/value") == "21.5");
  CHECK(last("VdMot/sensors/KLMNOPQRST/value/value") == "12.340");
}

namespace {

// Temp slot 1 on the bus, last seen `age` ms before every pass.
std::vector<std::string> tempSeenAgo(uint32_t age) {
  vdm::Config& c = useMqtt();
  c.temps[0].active = true;
  c.temps[0].id = oneWire(0x28, 1);
  vdm::StmSnapshot& s = linkUp();
  s.tempCount = 1;
  s.temps[0].id = c.temps[0].id;
  s.temps[0].raw = 215;
  s.temps[0].seen = true;
  s.temps[0].lastSeenMs = static_cast<uint32_t>(fakes::nowMs()) - age;
  publishSnap();
  startTracking([&s, age](size_t, uint64_t now) {
    s.temps[0].lastSeenMs = static_cast<uint32_t>(now) - age;
    publishSnap();
  });
  mqtt::begin();
  runTask(1 + 300);
  return payloads("VdMot/temps/1/value/value");
}

}  // namespace

TEST_CASE("mqtt values: a sensor seen 60 s ago is current") {
  glue::begin();
  CHECK(tempSeenAgo(60000) == std::vector<std::string>{"21.5"});
}

TEST_CASE("mqtt values: a sensor seen 60.001 s ago is stale") {
  glue::begin();
  CHECK(tempSeenAgo(60001) == std::vector<std::string>{"failed"});
}

// ---------------------------------------------------------------- diag

TEST_CASE("mqtt diag: STM counters, link, protocol, lease, safe mode once per change") {
  glue::begin();
  useMqtt();
  vdm::StmSnapshot& s = linkUp();
  s.haveStatus = true;
  s.status.resets = 1;
  s.status.rxOverflow = 2;
  s.status.parseErrors = 0;
  publishSnap();
  settle(60);
  using V = std::vector<std::string>;
  CHECK(payloads("VdMot/diag/stm/proto") == V{"2"});
  CHECK(payloads("VdMot/diag/stm/link") == V{"up"});
  CHECK(payloads("VdMot/diag/stm/resets") == V{"1"});
  CHECK(payloads("VdMot/diag/stm/rxOverflow") == V{"2"});
  CHECK(payloads("VdMot/diag/stm/parseErr") == V{"0"});
  CHECK(payloads("VdMot/diag/calibration/active") == V{"0"});
  CHECK(payloads("VdMot/diag/stm/lease") == V{"off"});
  CHECK(payloads("VdMot/diag/calibration/next") == V{""});
  CHECK(payloads("VdMot/diag/stm/safeMode").empty());  // not a v3 status
  s.link = vdm::LinkState::Degraded;
  s.proto = 3;
  s.status.resets = 4;
  s.valves[0].calibrating = true;
  s.lease.state = vdm::LeaseState::Running;
  publishSnap();
  sib::app().calib.nextEpoch = 1;
  runTask(20);
  CHECK(payloads("VdMot/diag/stm/proto") == V{"2", "3"});
  CHECK(payloads("VdMot/diag/stm/link") == V{"up", "degraded"});
  CHECK(payloads("VdMot/diag/stm/resets") == V{"1", "4"});
  CHECK(payloads("VdMot/diag/stm/rxOverflow") == V{"2"});
  CHECK(payloads("VdMot/diag/stm/parseErr") == V{"0"});
  CHECK(payloads("VdMot/diag/calibration/active") == V{"0", "1"});
  CHECK(payloads("VdMot/diag/stm/lease") == V{"off", "running"});
  CHECK(payloads("VdMot/diag/calibration/next") == V{"", "1970-01-01T00:00:01+00:00"});
  s.status.rxOverflow = 5;
  s.status.parseErrors = 6;
  s.valves[0].calibrating = false;
  s.status.v3 = true;
  publishSnap();
  runTask(20);
  CHECK(payloads("VdMot/diag/stm/resets") == V{"1", "4"});
  CHECK(payloads("VdMot/diag/stm/rxOverflow") == V{"2", "5"});
  CHECK(payloads("VdMot/diag/stm/parseErr") == V{"0", "6"});
  CHECK(payloads("VdMot/diag/calibration/active") == V{"0", "1", "0"});
  CHECK(payloads("VdMot/diag/stm/safeMode") == V{"0"});
  s.status.safeMode = true;
  publishSnap();
  runTask(20);
  CHECK(payloads("VdMot/diag/stm/safeMode") == V{"0", "1"});
  CHECK(payloads("VdMot/diag/stm/proto") == V{"2", "3"});
  CHECK(payloads("VdMot/diag/stm/link") == V{"up", "degraded"});
  CHECK(payloads("VdMot/diag/stm/lease") == V{"off", "running"});
}

TEST_CASE("mqtt diag: STM version up to 31 characters, published once") {
  glue::begin();
  useMqtt();
  vdm::StmSnapshot& s = linkUp();
  s.version.valid = true;
  s.version.major = 2;
  s.version.minor = 1;
  vdm::copyString(s.version.suffix, sizeof s.version.suffix, "-abcdefghijklmnopqrstuv");  // 23
  vdm::copyString(s.version.hw, sizeof s.version.hw, "C2");
  publishSnap();
  settle();
  runTask(20);
  CHECK(payloads("VdMot/diag/stm/version") ==
        std::vector<std::string>{"2.1.0-abcdefghijklmnopqrstuv_C2"});
  // 32 characters do not fit: the last version stays.
  vdm::copyString(s.version.suffix, sizeof s.version.suffix, "-abcdefghijklmnopqrstuvw");
  publishSnap();
  runTask(20);
  CHECK(payloads("VdMot/diag/stm/version").size() == 1);
}

TEST_CASE("mqtt diag: STM start time moves by more than 60 s, needs protocol 2, status, time") {
  glue::begin();
  useMqtt();
  vdm::StmSnapshot& s = linkUp();
  s.haveStatus = true;
  s.status.uptimeS = 1000;
  vdm::LocalTime& t = sib::net().localTime;
  t.valid = true;
  t.epoch = 1790001000;  // started 1790000000
  publishSnap();
  settle(4);
  const std::string started = "VdMot/diag/stm/started";
  REQUIRE(payloads(started) == std::vector<std::string>{"2026-09-21T14:13:20+00:00"});
  t.epoch += 60;  // moved by 60 s
  runTask(3);
  CHECK(count(started) == 1);
  t.epoch += 1;  // 61 s
  runTask(3);
  CHECK(count(started) == 2);
  CHECK(last(started) == "2026-09-21T14:14:21+00:00");
  s.status.uptimeS = 1030;  // back by 30 s
  publishSnap();
  runTask(3);
  CHECK(count(started) == 2);
  t.epoch += 5000;  // far off, but:
  s.proto = 1;       // protocol 1
  publishSnap();
  runTask(3);
  CHECK(count(started) == 2);
  s.proto = 2;
  s.haveStatus = false;  // no status
  publishSnap();
  runTask(3);
  CHECK(count(started) == 2);
  s.haveStatus = true;
  t.valid = false;  // no valid time
  publishSnap();
  runTask(3);
  CHECK(count(started) == 2);
  t.valid = true;
  t.epoch = s.status.uptimeS;  // the STM started at epoch 0
  runTask(3);
  CHECK(count(started) == 2);
  // A new session: a start time close to the epoch is published as well.
  t.epoch = 100;
  s.status.uptimeS = 50;
  publishSnap();
  mqtt::requestReconnect();
  runTask(40);
  CHECK(last(started) == "1970-01-01T00:00:50+00:00");
}

TEST_CASE("mqtt diag: paced counters go out 10 s after their last publish") {
  glue::begin();
  useMqtt();
  const std::string topic = "VdMot/diag/mqtt/commandsRejected";
  const uint64_t t0 = fakes::nowMs();
  const uint64_t first = t0 + 100;
  startTracking([&](size_t, uint64_t now) {
    if (now == first + 1000 || now == first + 11000) deliver("VdMot/valves/9/target/set", "50");
    if (now == first + 12000) fakes::advanceMs(19);  // the passes move off the 20 ms grid
  });
  mqtt::begin();
  runTask(1 + 1050);
  CHECK(payloads(topic) == std::vector<std::string>{"0", "1", "2"});
  CHECK(timesOf(topic) == std::vector<uint64_t>{first, first + 10000, first + 20019});
}

TEST_CASE("mqtt diag: valve last move, counters, inactive and unextended valves") {
  glue::begin();
  vdm::Config& c = useMqtt();
  for (int v = 0; v < 4; ++v) c.valves[v].active = true;
  vdm::StmSnapshot& s = linkUp();  // protocol 2, no status
  for (int v = 0; v < 5; ++v) {
    s.valves[v].known = true;
    s.valves[v].hasExtended = v != 3;
  }
  vdm::ValveState& v0 = s.valves[0];
  v0.moveSeq = 1;
  v0.lastMove.dir = vdm::MoveDir::Close;
  v0.lastMove.requestedCounts = 100;
  v0.lastMove.countedCounts = 90;
  v0.lastMove.stop = vdm::StopReason::EndStop;
  v0.lastMove.peakCurrent = 55;
  v0.lastMove.durationMs = 1234;
  v0.earlyStops = 2;
  v0.cmdRejected = 3;
  v0.calState = 4;
  publishSnap();
  settle();
  runTask(10);
  using V = std::vector<std::string>;
  const std::string stop = vdm::stopReasonName(vdm::StopReason::EndStop);
  CHECK(payloads("VdMot/diag/valves/1/lastMove") ==
        V{"{\"dir\":\"close\",\"req\":100,\"cnt\":90,\"stop\":\"" + stop +
          "\",\"peak\":55,\"ms\":1234}"});
  CHECK(payloads("VdMot/diag/valves/1/earlyStops") == V{"2"});
  CHECK(payloads("VdMot/diag/valves/1/cmdRejected") == V{"3"});
  CHECK(payloads("VdMot/diag/valves/1/calState") == V{"4"});
  CHECK(count("VdMot/diag/valves/2/lastMove") == 0);  // moveSeq 0
  CHECK(payloads("VdMot/diag/valves/2/earlyStops") == V{"0"});
  CHECK(payloads("VdMot/diag/valves/3/calState") == V{"0"});
  CHECK(countPrefix("VdMot/diag/valves/4/") == 0);  // no extended data
  CHECK(countPrefix("VdMot/diag/valves/5/") == 0);  // inactive
  CHECK(count("VdMot/diag/stm/resets") == 0);       // no status
  v0.moveSeq = 2;
  v0.lastMove.dir = vdm::MoveDir::Open;
  v0.earlyStops = 5;
  s.valves[1].cmdRejected = 7;
  s.valves[1].calState = 1;
  s.valves[2].earlyStops = 9;
  publishSnap();
  runTask(10);
  const V moves = payloads("VdMot/diag/valves/1/lastMove");
  REQUIRE(moves.size() == 2);
  CHECK(moves[1].find("{\"dir\":\"open\",") == 0);
  CHECK(payloads("VdMot/diag/valves/1/earlyStops") == V{"2", "5"});
  CHECK(payloads("VdMot/diag/valves/1/cmdRejected") == V{"3"});
  CHECK(payloads("VdMot/diag/valves/1/calState") == V{"4"});
  CHECK(payloads("VdMot/diag/valves/2/cmdRejected") == V{"0", "7"});
  CHECK(payloads("VdMot/diag/valves/2/calState") == V{"0", "1"});
  CHECK(payloads("VdMot/diag/valves/2/earlyStops") == V{"0"});
  CHECK(payloads("VdMot/diag/valves/3/earlyStops") == V{"0", "9"});
}

namespace {

void setProfile(vdm::Profile& p, uint8_t valve, uint8_t n, uint32_t seed) {
  p = vdm::Profile{};
  p.valve = valve;
  p.count = n;
  for (uint8_t i = 0; i < n; ++i) {
    p.samples[i].count = seed + i;
    p.samples[i].current = static_cast<uint16_t>(100 + i);
  }
}

}  // namespace

TEST_CASE("mqtt diag: four valve messages per pass, profiles only when new") {
  glue::begin();
  vdm::Config& c = useMqtt();
  for (int v = 0; v < 10; ++v) c.valves[v].active = true;
  vdm::StmSnapshot& s = snap();  // protocol 0, link unknown
  for (int v = 0; v < 10; ++v) {
    s.valves[v].known = true;
    s.valves[v].hasExtended = true;
    s.valves[v].moveSeq = static_cast<uint32_t>(v + 1);
  }
  publishSnap();
  size_t phaseA = 0;
  size_t phaseB = 0;
  size_t phaseC = 0;
  startTracking([&](size_t passes, uint64_t) {
    if (passes == 30) {  // four new profiles, one of a single sample
      setProfile(s.profiles[0], 0, 3, 10);
      setProfile(s.profiles[1], 1, 3, 20);
      setProfile(s.profiles[2], 2, 3, 30);
      setProfile(s.profiles[3], 3, 1, 40);
      publishSnap();
      phaseA = passes;
    } else if (passes == 40) {  // valve 1 and 3 move and have new profiles, valve 2 a profile
      ++s.valves[0].moveSeq;
      setProfile(s.profiles[0], 0, 3, 11);
      setProfile(s.profiles[1], 1, 3, 21);
      ++s.valves[2].moveSeq;
      setProfile(s.profiles[2], 2, 3, 31);
      publishSnap();
      phaseB = passes;
    } else if (passes == 50) {  // a profile cleared
      setProfile(s.profiles[0], 0, 0, 0);
      publishSnap();
      phaseC = passes;
    }
  });
  mqtt::begin();
  runTask(60);
  CHECK(count("VdMot/diag/stm/proto") == 0);
  CHECK(payloads("VdMot/diag/stm/link") == std::vector<std::string>{"unknown"});
  // Connect pass 0; pass 1: protocol and link cost two of four, then two valves per pass... four.
  using P = std::vector<size_t>;
  const size_t lm[10] = {1, 1, 2, 2, 2, 2, 3, 3, 3, 3};
  for (int v = 0; v < 10; ++v) {
    CAPTURE(v);
    const std::vector<size_t> p = passesOf("VdMot/diag/valves/" + std::to_string(v + 1) + "/lastMove");
    REQUIRE_FALSE(p.empty());
    CHECK(p[0] == lm[v]);
  }
  REQUIRE(phaseA > 0);
  REQUIRE(phaseB > 0);
  REQUIRE(phaseC > 0);
  CHECK(passesOf("VdMot/diag/valves/1/profile") == P{phaseA, phaseB});
  CHECK(passesOf("VdMot/diag/valves/2/profile") == P{phaseA, phaseB});
  CHECK(passesOf("VdMot/diag/valves/3/profile") == P{phaseA, phaseB + 1});
  CHECK(passesOf("VdMot/diag/valves/4/profile") == P{phaseA});
  CHECK(passesOf("VdMot/diag/valves/1/lastMove") == P{1, phaseB});
  CHECK(passesOf("VdMot/diag/valves/3/lastMove") == P{2, phaseB});
  const std::vector<fakes::MqttMessage> p4 = fakes::mqtt().publishedTo("VdMot/diag/valves/4/profile");
  REQUIRE(p4.size() == 1);
  CHECK(p4[0].payload == "{\"valve\":4,\"count\":1,\"samples\":[[40,100]]}");
  CHECK_FALSE(p4[0].retained);
}

TEST_CASE("mqtt diag: the valve loop ends after valve 12, whatever follows the valves") {
  glue::begin();
  vdm::Config& c = useMqtt();
  c.valves[11].active = true;
  // The bytes after the 12 valve configs and states read like an active, extended valve.
  vdm::copyString(c.temps[0].name, sizeof c.temps[0].name, "ABCDEFGHI\x01");
  c.temps[0].active = true;
  REQUIRE(reinterpret_cast<const uint8_t*>(c.valves + vdm::kValveCount)[offsetof(vdm::ValveConfig, active)] == 1);
  vdm::StmSnapshot& s = linkUp();
  s.valves[11].known = true;
  s.valves[11].hasExtended = true;
  REQUIRE(reinterpret_cast<const uint8_t*>(s.valves + vdm::kValveCount) ==
          reinterpret_cast<const uint8_t*>(s.temps));
  REQUIRE(offsetof(vdm::ValveState, hasExtended) < sizeof s.temps);
  memset(static_cast<void*>(s.temps), 1, sizeof s.temps);  // tempCount 0: no reading is used
  publishSnap();
  settle();
  CHECK(payloads("VdMot/diag/valves/12/earlyStops") == std::vector<std::string>{"0"});
}

// ---------------------------------------------------------------- events

TEST_CASE("mqtt events: from the first logged event, four per pass") {
  glue::begin();
  useMqtt();
  logger::log(vdm::EventCode::LowHeap, vdm::kNoValve, 1, 2);  // seq 1, before the session
  startTracking([](size_t passes, uint64_t) {
    if (passes != 5) return;
    logger::log(vdm::EventCode::CalibTimeMissing, vdm::kNoValve, 1);
    logger::log(vdm::EventCode::HeapFragmented, vdm::kNoValve, 1, 2);
    logger::log(vdm::EventCode::ConfigRepaired, vdm::kNoValve, 0, 3);
    logger::log(vdm::EventCode::FactoryResetSkipped);
    logger::log(vdm::EventCode::ValveStale, 12, 60);  // not one of the 12 valves: not joined
  });
  mqtt::begin();
  runTask(20);
  CHECK(payloads(kMessage).front() == "low heap (free 1, min 2)");
  CHECK(passesOf("VdMot/events") == std::vector<size_t>{0, 5, 5, 5, 5, 6});
  CHECK(last("VdMot/events").find("\"name\":\"valve_stale\"") != std::string::npos);
}

TEST_CASE("mqtt events: only Warning and above set common/message") {
  glue::begin();
  useMqtt();
  settle();
  logger::log(vdm::EventCode::CalibOk, 0, 3000, 3100);  // Info
  runTask(300);
  CHECK(last(kMessage) == "");
  logger::log(vdm::EventCode::ValveStale, 0, 60);  // Warning
  runTask(300);
  CHECK(last(kMessage) == "valve 1: no data for 60 s");
}

TEST_CASE("mqtt events: nothing on <main>events without the events option") {
  glue::begin();
  vdm::Config& c = useMqtt();
  c.mqtt.events = false;
  settle();
  logger::log(vdm::EventCode::ValveStale, 0, 60);
  logger::log(vdm::EventCode::LinkDown, vdm::kNoValve, 3);
  runTask(200);
  CHECK(count("VdMot/events") == 0);
}

TEST_CASE("mqtt events: events that never reach MQTT are not joined or counted") {
  glue::begin();
  useMqtt();
  startTracking([](size_t passes, uint64_t) {
    if (passes != 5) return;
    logger::log(vdm::EventCode::ValveStateChanged, 0, 1, 2);  // Debug
    logger::log(vdm::EventCode::ValveStateChanged, 0, 1, 2);
  });
  mqtt::begin();
  runTask(200);
  CHECK(mqtt::status().eventsSuppressed == 0);
  CHECK(payloads("VdMot/diag/mqtt/eventsSuppressed") == std::vector<std::string>{"0"});
}

TEST_CASE("mqtt events: a duplicate valve event counts as suppressed") {
  glue::begin();
  useMqtt();
  startTracking([](size_t passes, uint64_t) {
    if (passes != 5) return;
    logger::log(vdm::EventCode::ValveStale, 0, 60);
    logger::log(vdm::EventCode::ValveStale, 0, 60);
  });
  mqtt::begin();
  runTask(1 + 600);
  CHECK(mqtt::status().eventsSuppressed == 1);
  CHECK(payloads("VdMot/diag/mqtt/eventsSuppressed") == std::vector<std::string>{"0", "1"});
}

TEST_CASE("mqtt events: a fifth valve code flushes the oldest joined event at once") {
  glue::begin();
  useMqtt();
  startTracking([](size_t passes, uint64_t) {
    if (passes != 5) return;
    logger::log(vdm::EventCode::ValveBlocked, 0, 3);
    logger::log(vdm::EventCode::ValveNoValve, 0);
    logger::log(vdm::EventCode::CalibRetry, 0, 1);
    logger::log(vdm::EventCode::EarlyStop, 0, 4);
    logger::log(vdm::EventCode::CmdRejected, 0, 5);
  });
  mqtt::begin();
  runTask(200);
  const std::vector<std::string> ev = payloads("VdMot/events");
  const std::vector<size_t> at = passesOf("VdMot/events");
  REQUIRE(ev.size() == 5);
  CHECK(at[0] == 6);
  CHECK(ev[0].find("\"name\":\"valve_blocked\"") != std::string::npos);
  CHECK(at[1] > 6);
}

TEST_CASE("mqtt events: an event JSON of 511 characters goes out, 512 do not fit") {
  glue::begin();
  useMqtt();
  // Events whose <main>events JSON is exactly 511 and 512 characters long.
  auto jsonOf = [](const vdm::Event& e) {
    static char buf[2048];
    vdm::JsonWriter jw(buf, sizeof buf);
    REQUIRE(vdm::writeMqttEventJson(jw, e, 0));
    return std::string(buf);
  };
  auto build = [&](uint8_t valve, uint32_t seq, size_t want, vdm::Event& out) {
    for (uint32_t up : {0u, 10u, 100u, 1000u, 10000u, 100000u, 1000000u, 10000000u,
                        100000000u, 1000000000u}) {
      for (size_t k = 0; k <= vdm::kEventTextMax; ++k) {
        for (size_t m = 0; k + m <= vdm::kEventTextMax; ++m) {
          const std::string text = std::string(k, '\x01') + std::string(m, 'a');
          vdm::Event e = vdm::makeEvent(vdm::EventCode::StmEepromWaitTimeout,
                                        vdm::Severity::Warning, valve, INT32_MIN, 1, text.c_str());
          e.seq = seq;
          e.uptimeS = up;
          e.epoch = UINT32_MAX;
          if (jsonOf(e).size() == want) {
            out = e;
            return true;
          }
        }
      }
    }
    return false;
  };
  vdm::Event fits;
  vdm::Event tooLong;
  std::string fitsJson;
  startTracking([&](size_t passes, uint64_t) {
    if (passes != 5) return;
    const uint32_t seq = logger::lastSeq() + 1;
    REQUIRE(build(vdm::kNoValve, seq, 511, fits));
    REQUIRE(build(vdm::kAllValves, seq + 1, 512, tooLong));
    fitsJson = jsonOf(fits);
    CHECK(logger::log(fits) == seq);
    CHECK(logger::log(tooLong) == seq + 1);
  });
  mqtt::begin();
  runTask(20);
  CHECK(payloads("VdMot/events") == std::vector<std::string>{fitsJson});
}

// ---------------------------------------------------------------- inbound

TEST_CASE("mqtt inbound: payloads are cut at 33 bytes") {
  glue::begin();
  useMqtt();
  settle(2);
  deliver("VdMot/valves/1/target/set", std::string(32, ' ') + "x");  // 33 bytes: not blank
  runTask(2);
  CHECK(mqtt::status().commandsRejected == 1);
  deliver("VdMot/valves/1/target/set", std::string(33, ' ') + "x");  // blank after the cut
  runTask(2);
  CHECK(mqtt::status().commandsRejected == 1);
  CHECK(sib::app().submitted.empty());
}

TEST_CASE("mqtt inbound: topics up to 127 characters are read, longer ones dropped") {
  glue::begin();
  useMqtt();
  settle(2);
  const std::string base = "VdMot/cmd/";
  deliver(base + std::string(127 - base.size(), 'x'), "PRESS");
  runTask(2);
  REQUIRE(rejected().size() == 1);
  CHECK(std::string(rejected()[0].text) == "unknown command");
  CHECK(mqtt::status().commandsRejected == 1);
  deliver(base + std::string(128 - base.size(), 'x'), "PRESS");
  runTask(2);
  CHECK(mqtt::status().commandsRejected == 1);
}

TEST_CASE("mqtt inbound: an inbound overflow is rejected without a detail") {
  glue::begin();
  vdm::Config& c = useMqtt();
  for (int v = 0; v < 6; ++v) c.valves[v].active = true;
  settle(2);
  fakes::mqtt().burst = true;
  for (int v = 1; v <= 5; ++v) deliver("VdMot/valves/" + std::to_string(v) + "/target/set", "30");
  runTask(1);
  const std::vector<vdm::Event> ev = rejected();
  REQUIRE(ev.size() == 1);
  CHECK(std::string(ev[0].text) == "queue full");
  CHECK(ev[0].arg2 == 0);
}

TEST_CASE("mqtt inbound: an unconfirmed button is rejected without a detail") {
  glue::begin();
  useMqtt();
  settle(2);
  deliver("VdMot/cmd/restart", "PRESS");
  runTask(300);
  const std::vector<vdm::Event> ev = rejected();
  REQUIRE(ev.size() == 1);
  CHECK(std::string(ev[0].text) == "clear not confirmed");
  CHECK(ev[0].arg2 == 0);
}

TEST_CASE("mqtt inbound: a button whose clear failed is rejected without a detail") {
  glue::begin();
  useMqtt();
  settle(2);
  deliver("VdMot/cmd/restart", "PRESS");
  fakes::mqtt().failPublishAt = fakes::mqtt().publishCalls;
  runTask(1);
  const std::vector<vdm::Event> ev = rejected();
  REQUIRE(ev.size() == 1);
  CHECK(std::string(ev[0].text) == "clear not confirmed");
  CHECK(ev[0].arg2 == 0);
}

TEST_CASE("mqtt inbound: a confirmed button the queue refuses is rejected without a detail") {
  glue::begin();
  useMqtt();
  settle(2);
  sib::app().submitResult = false;
  deliver("VdMot/cmd/detect", "PRESS");
  deliver("VdMot/cmd/detect", "");
  runTask(2);
  const std::vector<vdm::Event> ev = rejected();
  REQUIRE(ev.size() == 1);
  CHECK(std::string(ev[0].text) == "queue full");
  CHECK(ev[0].arg2 == 0);
}

TEST_CASE("mqtt inbound: a fifth held button is rejected without a detail") {
  glue::begin();
  useMqtt();
  settle(2);
  for (const char* t : {"restart", "stmReset", "detect", "calibrate", "valves/1/calibrate"}) {
    deliver(std::string("VdMot/cmd/") + t, "PRESS");
  }
  runTask(5);
  const std::vector<vdm::Event> ev = rejected();
  REQUIRE(ev.size() == 1);
  CHECK(std::string(ev[0].text) == "queue full");
  CHECK(ev[0].arg1 == 1);
  CHECK(ev[0].arg2 == 0);
}

TEST_CASE("mqtt inbound: a message of the last session is not handled again after a reconnect") {
  glue::begin();
  useMqtt();
  settle(2);
  deliver("VdMot/valves/1/target/set", "40");
  runTask(2);
  REQUIRE(sib::app().submitted.size() == 1);
  mqtt::requestReconnect();
  runTask(5);
  CHECK(fakes::mqtt().connects == 2);
  CHECK(sib::app().submitted.size() == 1);
}

TEST_CASE("mqtt inbound: refused targets are re-submitted valve after valve from valve 1") {
  glue::begin();
  vdm::Config& c = useMqtt();
  c.valves[1].active = true;
  c.valves[2].active = true;
  settle(2);
  sib::app().submitResult = false;
  fakes::mqtt().burst = true;
  deliver("VdMot/valves/1/target/set", "10");
  deliver("VdMot/valves/2/target/set", "20");
  deliver("VdMot/valves/3/target/set", "30");
  runTask(1);  // all latched; each drain of the pass re-submits the next one (valves 1, 2)
  REQUIRE(sib::app().submitted.empty());
  sib::app().submitResult = true;
  runTask(2);
  REQUIRE(sib::app().submitted.size() == 3);
  CHECK(sib::app().submitted[0].valve == 2);
  CHECK(sib::app().submitted[1].valve == 0);
  CHECK(sib::app().submitted[2].valve == 1);
}

// ---------------------------------------------------------------- connection

TEST_CASE("mqtt connection: a refused connect is retried after the back-off only") {
  glue::begin();
  useMqtt();
  fakes::mqtt().connectResult = false;
  mqtt::begin();
  runTask(10);
  CHECK(fakes::mqtt().connects == 1);
  runTask(15);
  CHECK(fakes::mqtt().connects == 2);
}

TEST_CASE("mqtt connection: without a host no connect, an error with rc 0, then the back-off") {
  glue::begin();
  vdm::Config& c = useMqtt();
  c.mqtt.host[0] = '\0';
  mqtt::begin();
  runTask(3);
  CHECK(fakes::mqtt().connects == 0);
  CHECK(mqtt::status().state == vdm::MqttState::Error);
  CHECK(mqtt::status().rc == 0);
  vdm::copyString(c.mqtt.host, sizeof c.mqtt.host, "b");  // one character
  ++sib::storage().revision;
  runTask(3);
  CHECK(fakes::mqtt().connects == 0);  // the failed attempt waits 2 s
  runTask(20);
  CHECK(fakes::mqtt().connects == 1);
  CHECK(fakes::mqtt().host == "b");
  CHECK(mqtt::status().state == vdm::MqttState::Connected);
  CHECK(mqtt::status().rc == 0);
}

TEST_CASE("mqtt connection: a host and a client id of 64 characters") {
  glue::begin();
  vdm::Config& c = useMqtt();
  const std::string host(64, 'h');
  const std::string id(64, 'i');
  vdm::copyString(c.mqtt.host, sizeof c.mqtt.host, host.c_str());
  vdm::copyString(c.mqtt.clientId, sizeof c.mqtt.clientId, id.c_str());
  mqtt::begin();
  runTask(1);
  CHECK(fakes::mqtt().host == host);
  CHECK(fakes::mqtt().clientId == id);
}

TEST_CASE("mqtt connection: a client id of one character") {
  glue::begin();
  vdm::Config& c = useMqtt();
  vdm::copyString(c.mqtt.clientId, sizeof c.mqtt.clientId, "x");
  mqtt::begin();
  runTask(1);
  CHECK(fakes::mqtt().clientId == "x");
}

TEST_CASE("mqtt connection: user and password of one character each") {
  glue::begin();
  vdm::Config& c = useMqtt();
  vdm::copyString(c.mqtt.user, sizeof c.mqtt.user, "u");
  vdm::copyString(c.mqtt.password, sizeof c.mqtt.password, "p");
  mqtt::begin();
  runTask(1);
  CHECK_FALSE(fakes::mqtt().userNull);
  CHECK_FALSE(fakes::mqtt().passNull);
  CHECK(fakes::mqtt().user == "u");
  CHECK(fakes::mqtt().pass == "p");
}

TEST_CASE("mqtt connection: a restart after a refused connect disables with rc 0") {
  glue::begin();
  useMqtt();
  fakes::mqtt().connectResult = false;
  fakes::mqtt().failState = MQTT_CONNECT_UNAUTHORIZED;
  mqtt::begin();
  runTask(1);
  REQUIRE(mqtt::status().state == vdm::MqttState::Error);
  sib::ota().restartPending = true;
  runTask(2);
  CHECK(mqtt::status().state == vdm::MqttState::Disabled);
  CHECK(mqtt::status().rc == 0);
  CHECK(fakes::rtos().delays.back() == 100);
}

TEST_CASE("mqtt connection: a restart of a connected session disables with rc 0") {
  glue::begin();
  useMqtt();
  settle(2);
  fakes::mqtt().state = MQTT_CONNECT_UNAUTHORIZED;  // what state() answers after the disconnect
  sib::ota().restartPending = true;
  runTask(2);
  CHECK(mqtt::status().state == vdm::MqttState::Disabled);
  CHECK(mqtt::status().rc == 0);
}

TEST_CASE("mqtt connection: a network down never connects, MQTT off reads rc 0") {
  glue::begin();
  vdm::Config& c = useMqtt();
  sib::net().up = false;
  mqtt::begin();
  runTask(3);
  CHECK(fakes::mqtt().connects == 0);
  CHECK(mqtt::status().state == vdm::MqttState::Connecting);
  CHECK(mqtt::status().rc == 0);
  c.mqtt.mode = vdm::MqttMode::Off;
  ++sib::storage().revision;
  runTask(2);
  CHECK(mqtt::status().state == vdm::MqttState::Disabled);
  CHECK(mqtt::status().rc == 0);
}

TEST_CASE("mqtt connection: after 60 s of a stable session a drop reconnects at once") {
  glue::begin();
  useMqtt();
  settle(2);
  runTask(3050);  // > 60 s
  fakes::mqtt().dropConnection();
  runTask(1);
  CHECK(fakes::mqtt().connects == 2);
}

// ---------------------------------------------------------------- discovery

TEST_CASE("mqtt discovery: none in mode MQTT after the first-run cleanup") {
  glue::begin();
  useMqtt(vdm::MqttMode::Mqtt);
  settle(300);
  CHECK(countPrefix("homeassistant/") == 0);
  CHECK(sib::logger().withCode(vdm::EventCode::HaDiscoverySent).empty());
}

TEST_CASE("mqtt discovery: the first-run cleanup in mode MQTT deletes only") {
  glue::begin();
  useMqtt(vdm::MqttMode::Mqtt);
  sib::storage().haCleanupDone = false;
  settle(400);
  CHECK(sib::storage().haCleanupMarks == 1);
  CHECK(countPrefix("homeassistant/") > 0);
  for (const fakes::MqttMessage& m : fakes::mqtt().published) {
    if (m.topic.compare(0, 14, "homeassistant/") == 0) CHECK(m.payload.empty());
  }
  CHECK(sib::storage().haLayoutSets.empty());
}

TEST_CASE("mqtt discovery: a manual run prunes stale entries, retained configs, no 2.0 cleanup") {
  glue::begin();
  useMqtt(vdm::MqttMode::Mqtt);
  const std::string stale = "homeassistant/text/VdMot/valves_state_Old/config";
  fakes::fs().put("/HADiscovery.cfg", stale + "\n");
  settle(2);
  mqtt::requestDiscovery(mqtt::DiscoveryAction::Publish);
  runTask(500);
  CHECK(payloads(stale) == std::vector<std::string>{""});
  CHECK(count("homeassistant/sensor/VdMot/diag_stm_uptime/config") == 0);
  const std::vector<fakes::MqttMessage> st = fakes::mqtt().publishedTo("homeassistant/text/VdMot/state/config");
  REQUIRE(st.size() == 1);
  CHECK(st[0].retained);
  CHECK(fakes::fs().read("/HADiscovery.cfg").find(stale) == std::string::npos);
}

TEST_CASE("mqtt discovery: layout 1 runs the 2.0 cleanup on connect in HA mode") {
  glue::begin();
  vdm::Config& c = useMqtt(vdm::MqttMode::MqttHa);
  c.mqtt.haDiscoveryOnConnect = false;
  sib::storage().haLayout = 1;
  settle(500);
  CHECK(payloads("homeassistant/sensor/VdMot/diag_stm_uptime/config") == std::vector<std::string>{""});
  CHECK(sib::storage().haLayoutSets == std::vector<uint8_t>{2});
}

TEST_CASE("mqtt discovery: changed inputs run it again after the current run") {
  glue::begin();
  useMqtt(vdm::MqttMode::MqttHa);
  vdm::StmSnapshot& s = linkUp();
  startTracking([&](size_t passes, uint64_t) {
    if (passes == 10) {  // during the first run
      vdm::copyString(s.version.hw, sizeof s.version.hw, "C2");
      publishSnap();
    }
  });
  mqtt::begin();
  runTask(1000);
  auto runs = [] { return sib::logger().withCode(vdm::EventCode::HaDiscoverySent).size(); };
  CHECK(runs() == 2);
  CHECK_FALSE(mqtt::status().discoveryRunning);
  publishSnap();  // a new snapshot with the same inputs
  runTask(500);
  CHECK(runs() == 2);
  vdm::copyString(s.version.hw, sizeof s.version.hw, "C3");
  publishSnap();
  runTask(500);
  CHECK(runs() == 3);
}

TEST_CASE("mqtt discovery: a dropped session ends the run") {
  glue::begin();
  useMqtt(vdm::MqttMode::Mqtt);
  settle(2);
  mqtt::requestDiscovery(mqtt::DiscoveryAction::Publish);
  runTask(10);
  REQUIRE(mqtt::status().discoveryRunning);
  fakes::mqtt().dropConnection();
  runTask(1);
  CHECK_FALSE(mqtt::status().discoveryRunning);
  const size_t before = fakes::mqtt().published.size();
  runTask(600);
  CHECK(fakes::mqtt().connects == 2);
  CHECK(countPrefix("homeassistant/", before) == 0);
  CHECK(sib::logger().withCode(vdm::EventCode::HaDiscoverySent).empty());
}

TEST_CASE("mqtt discovery: without a ready file system the list is neither read nor written") {
  glue::begin();
  useMqtt(vdm::MqttMode::Mqtt);
  const std::string stale = "homeassistant/text/VdMot/valves_state_Old/config";
  fakes::fs().put("/HADiscovery.cfg", stale + "\n");
  sib::storage().fsReady = false;
  settle(2);
  mqtt::requestDiscovery(mqtt::DiscoveryAction::Publish);
  runTask(500);
  REQUIRE(sib::logger().withCode(vdm::EventCode::HaDiscoverySent).size() == 1);
  CHECK(count(stale) == 0);
  CHECK(fakes::fs().read("/HADiscovery.cfg") == stale + "\n");
  CHECK_FALSE(fakes::fs().exists("/HADiscovery.cfg.tmp"));
}

TEST_CASE("mqtt discovery: a list that cannot be committed leaves no temporary file") {
  glue::begin();
  useMqtt(vdm::MqttMode::Mqtt);
  fakes::fs().fail("rename");
  settle(2);
  mqtt::requestDiscovery(mqtt::DiscoveryAction::Publish);
  runTask(500);
  REQUIRE(sib::logger().withCode(vdm::EventCode::HaDiscoverySent).size() == 1);
  CHECK_FALSE(fakes::fs().exists("/HADiscovery.cfg"));
  CHECK_FALSE(fakes::fs().exists("/HADiscovery.cfg.tmp"));
}

TEST_CASE("mqtt discovery: a run replacing an unfinished one drops its half-written list") {
  glue::begin();
  vdm::Config& c = useMqtt(vdm::MqttMode::Mqtt);
  c.temps[0].active = true;
  c.temps[0].id = oneWire(0x28, 1);
  vdm::StmSnapshot& s = linkUp();
  settle(2);
  mqtt::requestDiscovery(mqtt::DiscoveryAction::Publish);
  runTask(500);
  const std::string before = fakes::fs().read("/HADiscovery.cfg");
  REQUIRE_FALSE(before.empty());
  // The sensor shows up on the bus: a new entity, the list is rewritten ...
  s.tempCount = 5;
  s.temps[4].id = c.temps[0].id;
  s.temps[4].seen = true;
  s.temps[4].raw = 200;
  publishSnap();
  bool replaced = false;
  startTracking([&](size_t, uint64_t) {
    if (replaced || !fakes::fs().exists("/HADiscovery.cfg.tmp")) return;
    // ... and while it is written the sensor is gone again: a run for the old entities.
    s.tempCount = 0;
    publishSnap();
    mqtt::requestDiscovery(mqtt::DiscoveryAction::Publish);
    replaced = true;
  });
  mqtt::requestDiscovery(mqtt::DiscoveryAction::Publish);
  runTask(500);
  REQUIRE(replaced);
  CHECK(sib::logger().withCode(vdm::EventCode::HaDiscoverySent).size() == 2);
  CHECK(fakes::fs().read("/HADiscovery.cfg") == before);
  CHECK_FALSE(fakes::fs().exists("/HADiscovery.cfg.tmp"));
}
