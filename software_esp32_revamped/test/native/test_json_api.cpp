// json_api: golden documents for every builder (empty and fully populated
// snapshots), null handling, overflow, and the API router (every route,
// methods, auth flags, malformed paths, fuzz).
#include <stdint.h>
#include <string.h>

#include <random>
#include <string>
#include <vector>

#include "doctest.h"
#include "vdm/json_api.h"

using namespace vdm;

namespace {

char gBuf[16384];

template <typename Fn>
std::string build(Fn fn) {
  JsonWriter jw(gBuf, sizeof gBuf);
  REQUIRE(fn(jw));
  REQUIRE(jw.complete());
  return std::string(gBuf, jw.length());
}

// Every builder must fail (not crash, not overrun) on every short buffer.
template <typename Fn>
void checkOverflow(Fn fn, size_t fullLen) {
  std::vector<char> buf(fullLen + 16, '#');
  for (size_t cap = 0; cap <= fullLen; cap += (cap < 64 ? 1 : 37)) {
    CAPTURE(cap);
    std::fill(buf.begin(), buf.end(), '#');
    JsonWriter jw(buf.data(), cap);
    CHECK_FALSE(fn(jw));
    for (size_t i = cap; i < buf.size(); ++i) REQUIRE(buf[i] == '#');
  }
  JsonWriter jw(buf.data(), fullLen + 1);
  CHECK(fn(jw));
}

std::string q(const char* s) { return std::string("\"") + s + "\""; }

OneWireId oid(const char* s) {
  OneWireId v;
  REQUIRE(parseOneWireId(s, strlen(s), v));
  return v;
}

}  // namespace

TEST_CASE("api: state names") {
  CHECK(std::string(netStateName(NetState::Down)) == "down");
  CHECK(std::string(netStateName(NetState::Ethernet)) == "ethernet");
  CHECK(std::string(netStateName(NetState::Wifi)) == "wifi");
  CHECK(std::string(netStateName(static_cast<NetState>(9))) == "down");
  CHECK(std::string(mqttStateName(MqttState::Disabled)) == "disabled");
  CHECK(std::string(mqttStateName(MqttState::Connecting)) == "connecting");
  CHECK(std::string(mqttStateName(MqttState::Connected)) == "connected");
  CHECK(std::string(mqttStateName(MqttState::Error)) == "error");
  CHECK(std::string(mqttStateName(static_cast<MqttState>(9))) == "error");
}

// ---------------------------------------------------------------- status

TEST_CASE("api: status of an empty snapshot") {
  StatusSnapshot s;
  const std::string j = build([&](JsonWriter& jw) { return writeStatusJson(jw, s); });
  const std::string expected =
      "{\"esp\":{\"version\":\"\",\"build\":null,\"uptime\":0,\"resetReason\":\"unknown\","
      "\"boots\":0,\"heap\":{\"free\":0,\"min\":0,\"largest\":0},\"flash\":{\"used\":0,\"size\":0}},"
      "\"time\":{\"valid\":false,\"epoch\":null,\"local\":null,\"lastSync\":null},"
      "\"net\":{\"state\":\"down\",\"ip\":\"0.0.0.0\",\"mask\":\"0.0.0.0\",\"gw\":\"0.0.0.0\","
      "\"dns\":\"0.0.0.0\",\"mac\":\"\",\"rssi\":null,\"hostname\":\"\"},"
      "\"mqtt\":{\"state\":\"disabled\",\"rc\":0,\"reconnects\":0,\"publishFailures\":0},"
      "\"stm\":{\"link\":" + q(linkStateName(LinkState::Unknown)) +
      ",\"proto\":null,\"version\":null,\"build\":null,\"hwId\":null,\"chip\":null,"
      "\"compatible\":true,\"minVersion\":\"1.4.0\",\"stats\":{\"sent\":0,\"answered\":0,"
      "\"timeouts\":0,\"failedRequests\":0,\"strayLines\":0,\"parseErrors\":0,\"queueFull\":0,"
      "\"evictions\":0,\"policyResets\":0,\"userResets\":0,\"consecutiveTimeouts\":0,"
      "\"lastReplyMs\":0},\"status\":null,\"espRx\":{\"overflow\":0,\"malformed\":0}},"
      "\"calibration\":{\"active\":false,\"lastScheduled\":null,\"nextSlot\":null},"
      "\"auth\":false,\"lastEventSeq\":0}";
  CHECK(j == expected);
}

TEST_CASE("api: status of a populated snapshot") {
  StatusSnapshot s;
  s.espVersion = "2.0.0-revamped";
  s.buildEpoch = 1790000000;
  s.uptimeS = 3600;
  s.resetReason = 6;
  s.bootCount = 17;
  s.freeHeap = 150000;
  s.minFreeHeap = 90000;
  s.largestFreeBlock = 110000;
  s.sketchSize = 987654;
  s.sketchSpace = 1310720;
  s.timeValid = true;
  s.epoch = 1790000123;
  s.local.valid = true;
  s.local.year = 2026;
  s.local.month = 9;
  s.local.mday = 3;
  s.local.hour = 4;
  s.local.minute = 5;
  s.local.second = 6;
  s.lastSyncEpoch = 1790000000;
  s.net = NetState::Wifi;
  s.ip = 0x3201A8C0;
  s.mask = 0x00FFFFFF;
  s.gateway = 0x0101A8C0;
  s.dns = 0x08080808;
  strcpy(s.mac, "AA:BB:CC:DD:EE:FF");
  s.wifiRssi = -67;
  strcpy(s.hostname, "VdMot \"OG\"");
  s.mqtt = MqttState::Connected;
  s.mqttRc = -2;
  s.mqttReconnects = 3;
  s.mqttPublishFailures = 4;
  s.link = LinkState::Up;
  s.linkStats.sent = 1;
  s.linkStats.answered = 2;
  s.linkStats.timeouts = 3;
  s.linkStats.failedRequests = 4;
  s.linkStats.strayLines = 5;
  s.linkStats.parseErrors = 6;
  s.linkStats.queueFull = 7;
  s.linkStats.evictions = 8;
  s.linkStats.policyResets = 9;
  s.linkStats.userResets = 10;
  s.linkStats.consecutiveTimeouts = 11;
  s.linkStats.lastReplyMs = 12;
  s.stmProto = 2;
  REQUIRE(parseVersion("2.0.0-revamped_C2", 17, s.stmVersion));
  s.stmBuild = 20260901;
  s.stmHwId = 0x431;
  s.stmCompatible = false;
  s.haveStmStatus = true;
  s.stmStatus.uptimeS = 100;
  s.stmStatus.resets = 2;
  s.stmStatus.bootReason = 3;
  s.stmStatus.rxOverflow = 4;
  s.stmStatus.parseErrors = 5;
  s.stmStatus.eepState = 1;
  s.espLineOverflows = 13;
  s.espLineMalformed = 14;
  s.calibrationActive = true;
  s.lastScheduledCalibEpoch = 1789990000;
  s.nextCalibSlot = 20260927;
  s.authEnabled = true;
  s.lastEventSeq = 999;
  const std::string j = build([&](JsonWriter& jw) { return writeStatusJson(jw, s); });
  const std::string expected =
      "{\"esp\":{\"version\":\"2.0.0-revamped\",\"build\":1790000000,\"uptime\":3600,"
      "\"resetReason\":\"task_wdt\",\"boots\":17,\"heap\":{\"free\":150000,\"min\":90000,"
      "\"largest\":110000},\"flash\":{\"used\":987654,\"size\":1310720}},"
      "\"time\":{\"valid\":true,\"epoch\":1790000123,\"local\":\"2026-09-03T04:05:06\","
      "\"lastSync\":1790000000},"
      "\"net\":{\"state\":\"wifi\",\"ip\":\"192.168.1.50\",\"mask\":\"255.255.255.0\","
      "\"gw\":\"192.168.1.1\",\"dns\":\"8.8.8.8\",\"mac\":\"AA:BB:CC:DD:EE:FF\",\"rssi\":-67,"
      "\"hostname\":\"VdMot \\\"OG\\\"\"},"
      "\"mqtt\":{\"state\":\"connected\",\"rc\":-2,\"reconnects\":3,\"publishFailures\":4},"
      "\"stm\":{\"link\":" + q(linkStateName(LinkState::Up)) +
      ",\"proto\":2,\"version\":\"2.0.0-revamped_C2\",\"build\":20260901,\"hwId\":\"0x431\","
      "\"chip\":" + q(stmChipName(0x431)) +
      ",\"compatible\":false,\"minVersion\":\"1.4.0\",\"stats\":{\"sent\":1,\"answered\":2,"
      "\"timeouts\":3,\"failedRequests\":4,\"strayLines\":5,\"parseErrors\":6,\"queueFull\":7,"
      "\"evictions\":8,\"policyResets\":9,\"userResets\":10,\"consecutiveTimeouts\":11,"
      "\"lastReplyMs\":12},\"status\":{\"uptime\":100,\"resets\":2,\"bootReason\":3,"
      "\"rxOverflow\":4,\"parseErr\":5,\"eepState\":1},\"espRx\":{\"overflow\":13,"
      "\"malformed\":14}},"
      "\"calibration\":{\"active\":true,\"lastScheduled\":1789990000,\"nextSlot\":20260927},"
      "\"auth\":true,\"lastEventSeq\":999}";
  CHECK(j == expected);
  checkOverflow([&](JsonWriter& jw) { return writeStatusJson(jw, s); }, j.size());
}

TEST_CASE("api: status field rules") {
  StatusSnapshot s;
  // Reset reason names, out-of-range -> unknown.
  const char* names[] = {"unknown", "poweron", "ext",      "sw",       "panic", "int_wdt",
                         "task_wdt", "wdt",    "deepsleep", "brownout", "sdio"};
  for (uint8_t r = 0; r < 11; ++r) {
    s.resetReason = r;
    const std::string j = build([&](JsonWriter& jw) { return writeStatusJson(jw, s); });
    CHECK(j.find(std::string("\"resetReason\":\"") + names[r] + "\"") != std::string::npos);
  }
  s.resetReason = 11;
  std::string j = build([&](JsonWriter& jw) { return writeStatusJson(jw, s); });
  CHECK(j.find("\"resetReason\":\"unknown\"") != std::string::npos);
  s.resetReason = 255;
  j = build([&](JsonWriter& jw) { return writeStatusJson(jw, s); });
  CHECK(j.find("\"resetReason\":\"unknown\"") != std::string::npos);

  // rssi only on WiFi.
  s.wifiRssi = -50;
  s.net = NetState::Ethernet;
  j = build([&](JsonWriter& jw) { return writeStatusJson(jw, s); });
  CHECK(j.find("\"state\":\"ethernet\"") != std::string::npos);
  CHECK(j.find("\"rssi\":null") != std::string::npos);

  // Local time only with valid time on both flags.
  s.timeValid = true;
  s.epoch = 5;
  j = build([&](JsonWriter& jw) { return writeStatusJson(jw, s); });
  CHECK(j.find("\"epoch\":5,\"local\":null") != std::string::npos);
  s.timeValid = false;
  s.local.valid = true;
  j = build([&](JsonWriter& jw) { return writeStatusJson(jw, s); });
  CHECK(j.find("\"epoch\":null,\"local\":null") != std::string::npos);

  s.lastScheduledCalibEpoch = 1;
  j = build([&](JsonWriter& jw) { return writeStatusJson(jw, s); });
  CHECK(j.find("\"lastScheduled\":1,") != std::string::npos);
  // Negative lastScheduled is "none"; hw id is zero-padded to 3 digits.
  s.lastScheduledCalibEpoch = -5;
  s.stmHwId = 0x23;
  s.espVersion = nullptr;
  j = build([&](JsonWriter& jw) { return writeStatusJson(jw, s); });
  CHECK(j.find("\"lastScheduled\":null") != std::string::npos);
  CHECK(j.find("\"hwId\":\"0x023\"") != std::string::npos);
  CHECK(j.find("\"version\":null,\"build\":null,\"uptime\"") != std::string::npos);

  // Unterminated fixed-size strings are cut at the array size.
  memset(s.mac, 'M', sizeof s.mac);
  memset(s.hostname, 'H', sizeof s.hostname);
  j = build([&](JsonWriter& jw) { return writeStatusJson(jw, s); });
  CHECK(j.find("\"mac\":\"" + std::string(sizeof s.mac - 1, 'M') + "\"") != std::string::npos);
  CHECK(j.find("\"hostname\":\"" + std::string(sizeof s.hostname - 1, 'H') + "\"") !=
        std::string::npos);
}

// ---------------------------------------------------------------- valves

TEST_CASE("api: valves document") {
  ValveState st;
  st.known = true;
  st.lastSeenMs = 1000;
  st.status = 2;
  st.calibrating = true;
  st.position = 55;
  st.meanCurrent = 12;
  st.desiredValid = true;
  st.desired = 60;
  st.source = TargetSource::Web;
  st.stmTargetKnown = true;
  st.stmTarget = 50;
  st.sync = TargetSync::Pending;
  st.moves = 100;
  st.openCount = 20;
  st.closeCount = 21;
  st.deadZone = -3;
  st.calibRetries = 1;
  st.health = kHealthBlocked | kHealthStale | kHealthTempFailed;
  st.hasExtended = true;
  st.calState = 2;
  st.calFlags = kCalFlagLastFailed;
  st.earlyStops = 3;
  st.cmdRejected = 4;
  st.lastMove.dir = MoveDir::Close;
  st.lastMove.requestedCounts = 3000;
  st.lastMove.countedCounts = 1500;
  st.lastMove.stop = StopReason::EarlyEndStop;
  st.lastMove.peakCurrent = 123;
  st.lastMove.durationMs = 4567;
  st.moveSeq = 9;
  ValveConfig cfg;
  strcpy(cfg.name, "Bad");
  cfg.active = true;

  ValveView views[3];
  views[0].state = &st;
  views[0].config = &cfg;
  views[0].sensorSlot[0] = 3;
  views[0].sensorName[0] = "Flur";
  views[0].sensorValid[0] = true;
  views[0].sensorTenths[0] = 215;
  views[0].sensorSlot[1] = 7;
  views[0].sensorName[1] = nullptr;
  views[0].sensorTenths[1] = 999;  // not valid -> null
  // views[1]: null state and config
  ValveState st2;
  st2.known = true;
  st2.lastSeenMs = 0xFFFFF000u;  // wrap: 6500 - 0xFFFFF000 = 10596 ms
  st2.status = 9;
  st2.health = 0x1FF;
  st2.lastMove.dir = MoveDir::Open;
  st2.hasExtended = true;
  views[2].state = &st2;

  const std::string j = build([&](JsonWriter& jw) { return writeValvesJson(jw, views, 3, 6500); });
  const std::string expected =
      "{\"valves\":[{\"idx\":1,\"name\":\"Bad\",\"active\":true,\"known\":true,\"state\":2,"
      "\"stateKey\":" + q(valveStatusKey(2)) +
      ",\"calibrating\":true,\"pos\":55,\"target\":60,\"targetSource\":" +
      q(targetSourceName(TargetSource::Web)) + ",\"sync\":" + q(targetSyncName(TargetSync::Pending)) +
      ",\"stmTarget\":50,\"meanCur\":12,\"moves\":100,\"oc\":20,\"cc\":21,\"dc\":-3,\"cr\":1,"
      "\"health\":[\"blocked\",\"stale\",\"tempFailed\"],\"age\":5,"
      "\"sensors\":[{\"slot\":3,\"name\":\"Flur\",\"temp\":21.5},{\"slot\":7,\"name\":\"\","
      "\"temp\":null}],\"ext\":{\"calState\":2,\"calEarlyStop\":false,"
      "\"calLastFailed\":true,\"earlyStops\":3,\"cmdRejected\":4,"
      "\"lastMove\":{\"dir\":\"close\",\"req\":3000,\"cnt\":1500,\"stop\":" +
      q(stopReasonName(StopReason::EarlyEndStop)) +
      ",\"peak\":12.3,\"ms\":4567},\"moveSeq\":9}},"
      "{\"idx\":2,\"name\":\"\",\"active\":false,\"known\":false,\"state\":0,\"stateKey\":" +
      q(valveStatusKey(0)) + ",\"calibrating\":false,\"pos\":0,\"target\":null,\"targetSource\":" +
      q(targetSourceName(TargetSource::None)) + ",\"sync\":" +
      q(targetSyncName(TargetSync::Unknown)) +
      ",\"stmTarget\":null,\"meanCur\":0,\"moves\":0,\"oc\":0,\"cc\":0,\"dc\":0,\"cr\":0,"
      "\"health\":[],\"age\":null,\"sensors\":[],\"ext\":null},"
      "{\"idx\":3,\"name\":\"\",\"active\":false,\"known\":true,\"state\":9,\"stateKey\":" +
      q(valveStatusKey(9)) + ",\"calibrating\":false,\"pos\":0,\"target\":null,\"targetSource\":" +
      q(targetSourceName(TargetSource::None)) + ",\"sync\":" +
      q(targetSyncName(TargetSync::Unknown)) +
      ",\"stmTarget\":null,\"meanCur\":0,\"moves\":0,\"oc\":0,\"cc\":0,\"dc\":0,\"cr\":0,"
      "\"health\":[\"blocked\",\"failed\",\"noValve\",\"calibRetries\",\"earlyStop\","
      "\"cmdRejected\",\"stale\",\"targetUnconfirmed\",\"tempFailed\"],\"age\":10,\"sensors\":[],"
      "\"ext\":{\"calState\":0,\"calEarlyStop\":false,\"calLastFailed\":false,"
      "\"earlyStops\":0,\"cmdRejected\":0,\"lastMove\":{\"dir\":\"open\","
      "\"req\":0,\"cnt\":0,\"stop\":" + q(stopReasonName(StopReason::None)) +
      ",\"peak\":0.0,\"ms\":0},\"moveSeq\":0}}]}";
  CHECK(j == expected);
  checkOverflow([&](JsonWriter& jw) { return writeValvesJson(jw, views, 3, 6500); }, j.size());

  // Names: at most 10 chars, also when the array is not terminated.
  ValveConfig full;
  memcpy(full.name, "0123456789", 10);
  ValveView fv;
  fv.config = &full;
  CHECK(build([&](JsonWriter& jw) { return writeValvesJson(jw, &fv, 1, 0); })
            .find("\"name\":\"0123456789\",") != std::string::npos);
  memset(full.name, 'n', sizeof full.name);
  CHECK(build([&](JsonWriter& jw) { return writeValvesJson(jw, &fv, 1, 0); })
            .find("\"name\":\"nnnnnnnnnn\",") != std::string::npos);
  // Unknown health bits above the table are ignored.
  st2.health = 0xFE00;
  const std::string k = build([&](JsonWriter& jw) { return writeValvesJson(jw, &views[2], 1, 0); });
  CHECK(k.find("\"health\":[]") != std::string::npos);
  // No views.
  CHECK(build([&](JsonWriter& jw) { return writeValvesJson(jw, nullptr, 12, 0); }) ==
        "{\"valves\":[]}");
  CHECK(build([&](JsonWriter& jw) { return writeValvesJson(jw, views, 0, 0); }) ==
        "{\"valves\":[]}");
}

TEST_CASE("api: twelve full valves fit a response slot") {
  static ValveState st[kValveCount];
  static ValveConfig cfg[kValveCount];
  ValveView views[kValveCount];
  for (uint8_t i = 0; i < kValveCount; ++i) {
    st[i].known = true;
    st[i].status = 1;
    st[i].desiredValid = true;
    st[i].stmTargetKnown = true;
    st[i].moves = 4000000000u;
    st[i].openCount = 4000000000u;
    st[i].closeCount = 4000000000u;
    st[i].deadZone = INT32_MIN;
    st[i].health = 0x1FF;
    st[i].hasExtended = true;
    st[i].earlyStops = 4000000000u;
    st[i].cmdRejected = 4000000000u;
    st[i].lastMove.requestedCounts = 4000000000u;
    st[i].lastMove.countedCounts = 4000000000u;
    st[i].lastMove.durationMs = 4000000000u;
    st[i].lastMove.peakCurrent = 65535;
    st[i].moveSeq = 4000000000u;
    memset(cfg[i].name, '"', kItemNameMax);  // worst-case escaping
    views[i].state = &st[i];
    views[i].config = &cfg[i];
    views[i].sensorSlot[0] = 34;
    views[i].sensorSlot[1] = 33;
    views[i].sensorName[0] = "\"\"\"\"\"\"\"\"\"\"";
    views[i].sensorName[1] = "\"\"\"\"\"\"\"\"\"\"";
    views[i].sensorValid[0] = true;
    views[i].sensorValid[1] = true;
    views[i].sensorTenths[0] = -1270;
    views[i].sensorTenths[1] = -1270;
  }
  static char slot[12 * 1024];
  JsonWriter jw(slot, sizeof slot);
  CHECK(writeValvesJson(jw, views, kValveCount, 0));
  CHECK(jw.complete());
  MESSAGE("worst-case /api/valves: ", jw.length(), " bytes");
}

// ---------------------------------------------------------------- profile, sensors, events

TEST_CASE("api: profile document") {
  Profile p;
  p.valve = 2;
  p.count = 2;
  p.samples[0].count = 100;
  p.samples[0].current = 50;
  p.samples[1].count = 4000000000u;
  p.samples[1].current = 65535;
  const std::string j = build([&](JsonWriter& jw) { return writeProfileJson(jw, p); });
  CHECK(j == "{\"valve\":3,\"count\":2,\"samples\":[[100,50],[4000000000,65535]]}");
  checkOverflow([&](JsonWriter& jw) { return writeProfileJson(jw, p); }, j.size());

  Profile empty;
  CHECK(build([&](JsonWriter& jw) { return writeProfileJson(jw, empty); }) ==
        "{\"valve\":1,\"count\":0,\"samples\":[]}");
  Profile over;
  over.count = 200;  // clamped to the 32 stored samples
  for (uint8_t i = 0; i < kProfileMaxSamples; ++i) over.samples[i].count = i;
  const std::string o = build([&](JsonWriter& jw) { return writeProfileJson(jw, over); });
  CHECK(o.find("\"count\":32,") != std::string::npos);
  CHECK(o.find("[31,0]]}") != std::string::npos);
  over.count = 32;
  CHECK(build([&](JsonWriter& jw) { return writeProfileJson(jw, over); }) == o);
  over.count = 31;
  CHECK(build([&](JsonWriter& jw) { return writeProfileJson(jw, over); }).find("[30,0]]}") !=
        std::string::npos);
}

TEST_CASE("api: sensors document") {
  SensorView t[2];
  t[0].slot = 1;
  t[0].name = "Flur";
  t[0].active = true;
  t[0].onBus = true;
  t[0].id = oid("28-84-37-94-97-ff-03-23");
  t[0].valid = true;
  t[0].raw = 210;
  t[0].value = 215;
  t[0].ageS = 3;
  t[0].valve = 0;
  t[1].name = nullptr;
  t[1].raw = -1270;
  t[1].value = 77;
  t[1].valve = 11;
  SensorView v[2];
  v[0].slot = 2;
  v[0].name = "Akku";
  v[0].active = true;
  v[0].id = oid("26-11-22-33-44-55-66-29");
  v[0].valid = true;
  v[0].raw = 1234;
  v[0].value = -12345;
  v[0].unit = "V";
  v[0].ageS = 9;
  v[1].unit = nullptr;
  v[1].valve = 3;  // ignored for volts
  const std::string j =
      build([&](JsonWriter& jw) { return writeSensorsJson(jw, t, 2, v, 2); });
  const std::string expected =
      "{\"temps\":[{\"slot\":1,\"name\":\"Flur\",\"id\":\"28-84-37-94-97-ff-03-23\","
      "\"active\":true,\"onBus\":true,\"temp\":21.5,\"raw\":210,\"age\":3,\"valve\":1},"
      "{\"slot\":null,\"name\":\"\",\"id\":\"\",\"active\":false,\"onBus\":false,\"temp\":null,"
      "\"raw\":-1270,\"age\":0,\"valve\":12}],"
      "\"volts\":[{\"slot\":2,\"name\":\"Akku\",\"id\":\"26-11-22-33-44-55-66-29\","
      "\"active\":true,\"onBus\":false,\"value\":-12.345,\"unit\":\"V\",\"raw\":1234,\"age\":9},"
      "{\"slot\":null,\"name\":\"\",\"id\":\"\",\"active\":false,\"onBus\":false,\"value\":null,"
      "\"unit\":\"\",\"raw\":0,\"age\":0}]}";
  CHECK(j == expected);
  checkOverflow([&](JsonWriter& jw) { return writeSensorsJson(jw, t, 2, v, 2); }, j.size());

  t[1].valve = 12;  // out of range -> null
  CHECK(build([&](JsonWriter& jw) { return writeSensorsJson(jw, &t[1], 1, nullptr, 0); })
            .find("\"valve\":null}") != std::string::npos);
  t[1].valve = kNoValve;
  CHECK(build([&](JsonWriter& jw) { return writeSensorsJson(jw, &t[1], 1, nullptr, 0); })
            .find("\"valve\":null}") != std::string::npos);
  CHECK(build([&](JsonWriter& jw) { return writeSensorsJson(jw, nullptr, 5, nullptr, 5); }) ==
        "{\"temps\":[],\"volts\":[]}");
}

TEST_CASE("api: events document") {
  Event e[2];
  e[0] = makeEvent(EventCode::EarlyStop, Severity::Warning, 2, 2, 3, "x");
  e[0].seq = 41;
  e[0].epoch = 1790000000;
  e[0].uptimeS = 77;
  e[1] = makeEvent(EventCode::LinkDown, Severity::Error, kNoValve, 5, 0, "");
  e[1].seq = 42;
  std::string ev0, ev1;
  {
    char b[512];
    JsonWriter jw(b, sizeof b);
    REQUIRE(writeEventJson(jw, e[0]));
    ev0 = jw.c_str();
    JsonWriter jw1(b, sizeof b);
    REQUIRE(writeEventJson(jw1, e[1]));
    ev1 = jw1.c_str();
  }
  const std::string j =
      build([&](JsonWriter& jw) { return writeEventsJson(jw, e, 2, 10, 42, 42, 9); });
  CHECK(j == "{\"first\":10,\"last\":42,\"next\":42,\"dropped\":9,\"events\":[" + ev0 + "," +
                 ev1 + "]}");
  checkOverflow([&](JsonWriter& jw) { return writeEventsJson(jw, e, 2, 10, 42, 42, 9); },
                j.size());
  CHECK(build([&](JsonWriter& jw) { return writeEventsJson(jw, nullptr, 3, 0, 0, 0, 0); }) ==
        "{\"first\":0,\"last\":0,\"next\":0,\"dropped\":0,\"events\":[]}");
  CHECK(build([&](JsonWriter& jw) {
          return writeEventsJson(jw, e, 0, 4294967295u, 1, 2, 3);
        }) == "{\"first\":4294967295,\"last\":1,\"next\":2,\"dropped\":3,\"events\":[]}");
}

// ---------------------------------------------------------------- flash, motor, error

TEST_CASE("api: flash status document") {
  FlashStatus idle;
  const std::string j = build([&](JsonWriter& jw) { return writeFlashStatusJson(jw, idle, nullptr); });
  const std::string expected =
      "{\"phase\":" + q(flashPhaseName(FlashPhase::Idle)) +
      ",\"status\":" + std::to_string(legacyFlashStatus(FlashPhase::Idle)) +
      ",\"percent\":0,\"bytesDone\":0,\"bytesTotal\":0,\"chipId\":null,\"chipName\":null,"
      "\"bootloaderVersion\":null,\"attempt\":0,\"error\":null,\"startedMs\":0,\"finishedMs\":0,"
      "\"image\":null,\"appVersion\":null}";
  CHECK(j == expected);

  FlashStatus s;
  s.phase = FlashPhase::Failed;
  s.error = FlashError::Nack;
  s.errorPhase = FlashPhase::Writing;
  s.errorAddress = 0x08000100;
  s.percent = 42;
  s.bytesDone = 2048;
  s.bytesTotal = 65536;
  s.chipPid = 0x431;
  s.bootloaderVersion = 0x31;
  s.attempt = 2;
  s.startedMs = 1000;
  s.finishedMs = 4294967295u;
  s.image.size = 65533;
  s.image.crc = 0xDEADBEEF;
  strcpy(s.image.version, "2.0.0-revamped_C2");
  REQUIRE(parseVersion("1.4.9_Dev_C2", 12, s.appVersion));
  const std::string f =
      build([&](JsonWriter& jw) { return writeFlashStatusJson(jw, s, "fw \"1\".bin"); });
  const std::string fe =
      "{\"phase\":" + q(flashPhaseName(FlashPhase::Failed)) +
      ",\"status\":" + std::to_string(legacyFlashStatus(FlashPhase::Failed)) +
      ",\"percent\":42,\"bytesDone\":2048,\"bytesTotal\":65536,\"chipId\":\"0x431\","
      "\"chipName\":" + q(stmChipName(0x431)) +
      ",\"bootloaderVersion\":\"3.1\",\"attempt\":2,\"error\":{\"code\":" +
      q(flashErrorName(FlashError::Nack)) + ",\"phase\":" + q(flashPhaseName(FlashPhase::Writing)) +
      ",\"addr\":\"0x08000100\"},\"startedMs\":1000,\"finishedMs\":4294967295,"
      "\"image\":{\"name\":\"fw \\\"1\\\".bin\",\"size\":65533,\"crc32\":\"0xdeadbeef\","
      "\"version\":\"2.0.0-revamped_C2\"},\"appVersion\":\"1.4.9_Dev_C2\"}";
  CHECK(f == fe);
  checkOverflow([&](JsonWriter& jw) { return writeFlashStatusJson(jw, s, "fw \"1\".bin"); },
                f.size());

  // Image without a name, name without an image, unterminated version.
  FlashStatus a;
  a.image.size = 1;
  a.image.crc = 0x1;
  std::string k = build([&](JsonWriter& jw) { return writeFlashStatusJson(jw, a, nullptr); });
  CHECK(k.find("\"image\":{\"name\":null,\"size\":1,\"crc32\":\"0x00000001\",\"version\":null}") !=
        std::string::npos);
  FlashStatus b;
  b.bootloaderVersion = 0x10;
  k = build([&](JsonWriter& jw) { return writeFlashStatusJson(jw, b, "a.bin"); });
  CHECK(k.find("\"image\":{\"name\":\"a.bin\",\"size\":0,\"crc32\":\"0x00000000\","
               "\"version\":null}") != std::string::npos);
  CHECK(k.find("\"bootloaderVersion\":\"1.0\"") != std::string::npos);
  memset(b.image.version, 'v', sizeof b.image.version);
  k = build([&](JsonWriter& jw) { return writeFlashStatusJson(jw, b, "a.bin"); });
  CHECK(k.find("\"version\":\"" + std::string(sizeof b.image.version - 1, 'v') + "\"") !=
        std::string::npos);
}

TEST_CASE("api: motor document") {
  MotorChars m;
  const std::string j =
      build([&](JsonWriter& jw) { return writeMotorJson(jw, m, 0, nullptr, false); });
  CHECK(j ==
        "{\"motor\":{\"lowC\":17,\"highC\":17,\"startOnPower\":30,\"noOfMinCount\":3000,"
        "\"maxCalReps\":2},\"learnMovements\":0,\"breakaway\":null,\"known\":false}");
  m.lowFactor = 10;
  m.highFactor = 40;
  m.startOnPower = 100;
  m.minCounts = 60000;
  m.maxCalibRetries = 0;
  Breakaway b;
  b.enable = true;
  b.stepPct = 25;
  b.maxmA = 45;
  const std::string k = build([&](JsonWriter& jw) { return writeMotorJson(jw, m, 65534, &b, true); });
  CHECK(k ==
        "{\"motor\":{\"lowC\":10,\"highC\":40,\"startOnPower\":100,\"noOfMinCount\":60000,"
        "\"maxCalReps\":0},\"learnMovements\":65534,\"breakaway\":{\"enable\":true,"
        "\"stepPct\":25,\"maxmA\":45},\"known\":true}");
  checkOverflow([&](JsonWriter& jw) { return writeMotorJson(jw, m, 65534, &b, true); }, k.size());
}

TEST_CASE("api: error document") {
  CHECK(build([](JsonWriter& jw) { return writeErrorJson(jw, "invalid", "mqtt.port"); }) ==
        "{\"error\":\"invalid\",\"detail\":\"mqtt.port\"}");
  CHECK(build([](JsonWriter& jw) { return writeErrorJson(jw, "x", nullptr); }) ==
        "{\"error\":\"x\",\"detail\":null}");
  checkOverflow([](JsonWriter& jw) { return writeErrorJson(jw, "a\"b", "c"); },
                strlen("{\"error\":\"a\\\"b\",\"detail\":\"c\"}"));
  // Not a root document -> not complete -> false.
  char buf[64];
  JsonWriter jw(buf, sizeof buf);
  jw.beginArray();
  CHECK_FALSE(writeErrorJson(jw, "a", "b"));
}

// ---------------------------------------------------------------- routing

namespace {

RouteMatch route(HttpMethod m, const std::string& path, bool protectRead = false) {
  return matchApiRoute(m, path.data(), path.size(), protectRead);
}

}  // namespace

TEST_CASE("api: every route, method and auth flag") {
  struct Case {
    HttpMethod m;
    const char* path;
    ApiRoute r;
    bool readOnly;  // public unless protectRead
  };
  const HttpMethod G = HttpMethod::Get, P = HttpMethod::Post, D = HttpMethod::Delete;
  const Case cases[] = {
      {G, "/api/status", ApiRoute::Status, true},
      {G, "/api/valves", ApiRoute::Valves, true},
      {P, "/api/valves/1/target", ApiRoute::ValveTarget, false},
      {P, "/api/valves/1/calibrate", ApiRoute::ValveCalibrate, false},
      {P, "/api/valves/1/assembly", ApiRoute::ValveAssembly, false},
      {P, "/api/valves/1/service-move", ApiRoute::ValveServiceMove, false},
      {P, "/api/valves/1/sensors", ApiRoute::ValveSensors, false},
      {G, "/api/valves/1/profile", ApiRoute::ValveProfile, true},
      {P, "/api/valves/1/profile", ApiRoute::ValveProfileRefresh, false},
      {P, "/api/valves/calibrate", ApiRoute::CalibrateAll, false},
      {P, "/api/valves/assembly", ApiRoute::AssemblyAll, false},
      {P, "/api/valves/detect", ApiRoute::Detect, false},
      {G, "/api/sensors", ApiRoute::Sensors, true},
      {P, "/api/sensors/scan", ApiRoute::SensorsScan, false},
      {G, "/api/events", ApiRoute::Events, true},
      {G, "/api/config", ApiRoute::ConfigGet, false},
      {P, "/api/config", ApiRoute::ConfigPatch, false},
      {G, "/api/config/export", ApiRoute::ConfigExport, false},
      {G, "/api/stm/motor", ApiRoute::Motor, true},
      {P, "/api/stm/motor", ApiRoute::MotorSet, false},
      {P, "/api/stm/reset", ApiRoute::StmReset, false},
      {G, "/api/stm/images", ApiRoute::StmImages, false},
      {P, "/api/stm/images", ApiRoute::StmImageUpload, false},
      {D, "/api/stm/images/fw.bin", ApiRoute::StmImageDelete, false},
      {P, "/api/stm/flash", ApiRoute::StmFlash, false},
      {G, "/api/stm/flash", ApiRoute::StmFlashStatus, true},
      {P, "/api/stm/flash/abort", ApiRoute::StmFlashAbort, false},
      {P, "/api/ota/esp", ApiRoute::EspOta, false},
      {P, "/api/system/reboot", ApiRoute::Reboot, false},
      {P, "/api/system/factory-reset", ApiRoute::FactoryReset, false},
      {P, "/api/mqtt/reconnect", ApiRoute::MqttReconnect, false},
      {P, "/api/mqtt/discovery", ApiRoute::MqttDiscovery, false},
      {G, "/api/log", ApiRoute::LogDownload, false},
  };
  for (const Case& k : cases) {
    CAPTURE(k.path);
    const RouteMatch a = route(k.m, k.path, false);
    CHECK(a.route == k.r);
    CHECK(a.needsAuth == !k.readOnly);
    const RouteMatch b = route(k.m, k.path, true);
    CHECK(b.route == k.r);
    CHECK(b.needsAuth);
    // Every other method on a known path -> 405 (unless another entry uses it).
    for (HttpMethod other : {G, P, D, HttpMethod::Other}) {
      if (other == k.m) continue;
      bool alsoRouted = false;
      for (const Case& c2 : cases) alsoRouted |= (c2.m == other && strcmp(c2.path, k.path) == 0);
      if (alsoRouted) continue;
      const RouteMatch x = route(other, k.path);
      CHECK(x.route == ApiRoute::MethodNotAllowed);
      CHECK(x.needsAuth);
    }
  }
}

TEST_CASE("api: route parameters") {
  for (int n = 1; n <= 12; ++n) {
    const RouteMatch m = route(HttpMethod::Post, "/api/valves/" + std::to_string(n) + "/target");
    CHECK(m.route == ApiRoute::ValveTarget);
    CHECK(m.valve == n - 1);
  }
  CHECK(route(HttpMethod::Get, "/api/status").valve == kNoValve);
  const char* badValves[] = {"0", "13", "01", "1a", "", "-1", "+1", "100", "99999999999"};
  for (const char* v : badValves) {
    CAPTURE(v);
    CHECK(route(HttpMethod::Post, std::string("/api/valves/") + v + "/target").route ==
          ApiRoute::NotFound);
  }
  RouteMatch m = route(HttpMethod::Delete, "/api/stm/images/STM32F411_C2-rev.1_x.bin");
  CHECK(m.route == ApiRoute::StmImageDelete);
  CHECK(std::string(m.name) == "STM32F411_C2-rev.1_x.bin");
  const std::string n31(31, 'n');
  m = route(HttpMethod::Delete, "/api/stm/images/" + n31);
  CHECK(m.route == ApiRoute::StmImageDelete);
  CHECK(std::string(m.name) == n31);
  CHECK(m.name[31] == '\0');
  m = route(HttpMethod::Delete, "/api/stm/images/azAZ09._-");
  CHECK(m.route == ApiRoute::StmImageDelete);
  CHECK(std::string(m.name) == "azAZ09._-");
  const char* badNames[] = {".hidden", "a b", "a/b", "a%20", "", "\xc3\xa4", "%ab", "a`",
                            "a{",      "a@",  "a[",  "a:",   "a\x7f", "_:"};
  for (const char* b : badNames) {
    CAPTURE(b);
    CHECK(route(HttpMethod::Delete, std::string("/api/stm/images/") + b).route ==
          ApiRoute::NotFound);
  }
  CHECK(route(HttpMethod::Delete, "/api/stm/images/" + n31 + "n").route == ApiRoute::NotFound);
  CHECK(route(HttpMethod::Get, "/api/stm/images/a.bin").route == ApiRoute::MethodNotAllowed);
  CHECK(route(HttpMethod::Get, "/api/stm/images/a.bin").name[0] == '\0');
  CHECK(route(HttpMethod::Delete, "/api/stm/images").route == ApiRoute::MethodNotAllowed);
}

TEST_CASE("api: malformed and unknown paths") {
  const char* notFound[] = {
      "",
      "/",
      "/api",
      "/api/",
      "api/status",
      "/API/status",
      "/api/status/",
      "/api//status",
      "/api/status?x=1",
      "/api/statuss",
      "/api/statu",
      "/api/valves/",
      "/api/valves/1",
      "/api/valves/1/",
      "/api/valves/1/target/x",
      "/api/valves//target",
      "/api/valves/target",
      "/api/stm",
      "/api/stm/images/a.bin/x",
      "/api/stm/flash/abort/",
      "/apix/status",
      "/api/unknown",
      "//api/status",
      "/api/config/export/x",
  };
  for (const char* p : notFound) {
    CAPTURE(p);
    for (HttpMethod m : {HttpMethod::Get, HttpMethod::Post, HttpMethod::Delete}) {
      const RouteMatch r = route(m, p);
      CHECK(r.route == ApiRoute::NotFound);
      CHECK(r.needsAuth);
      CHECK(r.valve == kNoValve);
    }
  }
  CHECK(matchApiRoute(HttpMethod::Get, nullptr, 11, false).route == ApiRoute::NotFound);
  const std::string withNul("/api/status\0", 12);
  CHECK(route(HttpMethod::Get, withNul).route == ApiRoute::NotFound);
  const std::string nulInside("/api/sta\0us", 11);
  CHECK(route(HttpMethod::Get, nulInside).route == ApiRoute::NotFound);
  // Only `len` bytes are considered.
  CHECK(matchApiRoute(HttpMethod::Get, "/api/statusXYZ", 11, false).route == ApiRoute::Status);
  CHECK(matchApiRoute(HttpMethod::Get, "/api/status", 10, false).route == ApiRoute::NotFound);
}

TEST_CASE("api: router fuzz") {
  std::mt19937 rng(31337);
  const char* pieces[] = {"/", "api", "valves", "1", "12", "13", "0", "stm", "images", "flash",
                          "abort", "a.bin", ".x", "target", "profile", "config", "export", "",
                          "%", "\x01", "status", "log"};
  for (int iter = 0; iter < 20000; ++iter) {
    std::string p = rng() % 4 ? "/api/" : "";
    const int n = static_cast<int>(rng() % 6);
    for (int i = 0; i < n; ++i) p += pieces[rng() % (sizeof pieces / sizeof pieces[0])];
    const HttpMethod m = static_cast<HttpMethod>(rng() % 4);
    const RouteMatch r = matchApiRoute(m, p.data(), p.size(), rng() % 2 != 0);
    CHECK(strlen(r.name) < sizeof r.name);
    if (r.route == ApiRoute::NotFound || r.route == ApiRoute::MethodNotAllowed) {
      CHECK(r.needsAuth);
    }
    CHECK((r.valve == kNoValve || r.valve < kValveCount));
  }
}

TEST_CASE("status JSON: all-ones addresses are written in full") {
  StatusSnapshot s;
  s.ip = 0xFFFFFFFFu;
  s.mask = 0xFFFFFFFFu;
  s.gateway = 0xFEFFFFFFu;
  s.dns = 0xFFFFFFFEu;
  static char buf[4096];
  JsonWriter jw(buf, sizeof buf);
  REQUIRE(writeStatusJson(jw, s));
  const std::string j(buf, jw.length());
  CHECK(j.find("\"ip\":\"255.255.255.255\",\"mask\":\"255.255.255.255\","
               "\"gw\":\"255.255.255.254\",\"dns\":\"254.255.255.255\"") != std::string::npos);
}
