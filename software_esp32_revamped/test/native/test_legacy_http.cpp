// legacy_http: the legacy route table (aliases, 405, 410 with replacements)
// and golden documents of /valves, /temps and /volts.
#include <stdint.h>
#include <string.h>

#include <string>
#include <vector>

#include "doctest.h"
#include "vdm/legacy_http.h"

using namespace vdm;

namespace {

char gBuf[8192];

template <typename Fn>
std::string build(Fn fn) {
  JsonWriter jw(gBuf, sizeof gBuf);
  REQUIRE(fn(jw));
  REQUIRE(jw.complete());
  return std::string(gBuf, jw.length());
}

template <typename Fn>
void checkOverflow(Fn fn, size_t fullLen) {
  std::vector<char> buf(fullLen + 16, '#');
  for (size_t cap = 0; cap <= fullLen; ++cap) {
    CAPTURE(cap);
    std::fill(buf.begin(), buf.end(), '#');
    JsonWriter jw(buf.data(), cap);
    CHECK_FALSE((fn(jw) && jw.complete()));
    for (size_t i = cap; i < buf.size(); ++i) REQUIRE(buf[i] == '#');
  }
}

OneWireId oid(const char* s) {
  OneWireId v;
  REQUIRE(parseOneWireId(s, strlen(s), v));
  return v;
}

LegacyMatch match(HttpMethod m, const char* p, bool protectRead = false) {
  return matchLegacyRoute(m, p, strlen(p), protectRead);
}

const HttpMethod kMethods[] = {HttpMethod::Get, HttpMethod::Post, HttpMethod::Delete,
                               HttpMethod::Other};

}  // namespace

TEST_CASE("legacy routes: aliases per method and auth") {
  struct Row {
    const char* path;
    LegacyRoute route;
    HttpMethod method;
  };
  const Row rows[] = {{"/valves", LegacyRoute::Valves, HttpMethod::Get},
                      {"/temps", LegacyRoute::Temps, HttpMethod::Get},
                      {"/volts", LegacyRoute::Volts, HttpMethod::Get},
                      {"/setvalve", LegacyRoute::SetValve, HttpMethod::Post}};
  for (const Row& r : rows) {
    for (HttpMethod m : kMethods) {
      for (bool protectRead : {false, true}) {
        CAPTURE(r.path);
        CAPTURE(static_cast<int>(m));
        CAPTURE(protectRead);
        const LegacyMatch lm = match(m, r.path, protectRead);
        CHECK(std::string(lm.replacement) == "");
        if (m == r.method) {
          CHECK(lm.route == r.route);
          CHECK(lm.needsAuth == (r.route == LegacyRoute::SetValve || protectRead));
        } else {
          CHECK(lm.route == LegacyRoute::MethodNotAllowed);
          CHECK_FALSE(lm.needsAuth);
        }
      }
    }
  }
}

TEST_CASE("legacy routes: the 410 table") {
  struct Row {
    const char* path;
    const char* replacement;
  };
  const Row rows[] = {
      {"/netinfo", "/api/status"},
      {"/sysinfo", "/api/status"},
      {"/sysdyninfo", "/api/status"},
      {"/update/identity", "/api/status"},
      {"/netconfig", "/api/config"},
      {"/protconfig", "/api/config"},
      {"/valvesconfig", "/api/config"},
      {"/tempsconfig", "/api/config"},
      {"/voltsconfig", "/api/config"},
      {"/sysconfig", "/api/config"},
      {"/sysLogCfg", "/api/config"},
      {"/motorconfig", "/api/stm/motor"},
      {"/tempsensorsid", "/api/sensors"},
      {"/voltsensorsid", "/api/sensors"},
      {"/fsdir", "/api/files"},
      {"/fupload", "/api/stm/images"},
      {"/stmupdate", "/#maintenance"},
      {"/stmupdstatus", "/api/stm/flash"},
      {"/stmdoupdate", "/api/stm/flash"},
      {"/update", "/api/ota/esp"},
      {"/cmd",
       "/api/system/reboot, /api/valves/calibrate, /api/valves/assembly, /api/valves/detect, "
       "/api/sensors/scan, /api/mqtt/reconnect, /api/mqtt/discovery"},
      {"/valvesctrlconfig", "removed: PI control"},
      {"/msgconfig", "removed: messenger"},
      {"/testPO", "removed: messenger"},
      {"/testEmail", "removed: messenger"},
      {"/ssidinfo", "removed: WiFi scan"},
      {"/auth", "removed: HTTP Basic auth is used"},
  };
  for (const Row& r : rows) {
    for (HttpMethod m : kMethods) {
      for (bool protectRead : {false, true}) {
        CAPTURE(r.path);
        const LegacyMatch lm = match(m, r.path, protectRead);
        CHECK(lm.route == LegacyRoute::Gone);
        CHECK(std::string(lm.replacement) == r.replacement);
        CHECK_FALSE(lm.needsAuth);
      }
    }
  }
}

TEST_CASE("legacy routes: everything else is None") {
  const char* const paths[] = {"/valves/", "/Valves", "/valve", "/valvess", "/", "",
                               "/api/valves", "/netinfo/", "/NETINFO", "/update/", "/cm",
                               "/setvalve/1", "/temps?x=1"};
  for (const char* p : paths) {
    CAPTURE(p);
    const LegacyMatch lm = match(HttpMethod::Get, p);
    CHECK(lm.route == LegacyRoute::None);
    CHECK_FALSE(lm.needsAuth);
    CHECK(std::string(lm.replacement) == "");
  }
  CHECK(matchLegacyRoute(HttpMethod::Get, nullptr, 7, true).route == LegacyRoute::None);
  // only `len` bytes count
  CHECK(matchLegacyRoute(HttpMethod::Get, "/valvesX", 7, false).route == LegacyRoute::Valves);
  CHECK(matchLegacyRoute(HttpMethod::Get, "/valves", 6, false).route == LegacyRoute::None);
}

TEST_CASE("legacy /valves document") {
  ValveState st[5];
  ValveConfig cfg[5];
  ValveView v[5];
  for (int i = 0; i < 5; ++i) {
    v[i].state = &st[i];
    v[i].config = &cfg[i];
  }
  copyString(cfg[0].name, sizeof cfg[0].name, "Bad");
  st[0].status = 1;
  st[0].position = 40;
  st[0].meanCurrent = 12;
  st[0].desiredValid = true;
  st[0].desired = 55;
  st[0].stmTargetKnown = true;
  st[0].stmTarget = 50;
  st[0].moves = 7;
  st[0].openCount = 300;
  st[0].closeCount = 310;
  st[0].deadZone = 4;
  st[0].calibRetries = 2;
  v[0].sensorSlot[0] = 3;
  v[0].sensorName[0] = "Floor";
  v[0].sensorValid[0] = true;
  v[0].sensorTenths[0] = 215;
  v[0].sensorSlot[1] = 4;
  v[0].sensorName[1] = "Wall";
  v[0].sensorValid[1] = false;
  st[1].status = 6;  // no valve: skipped
  st[2].status = 0;  // no data: skipped
  copyString(cfg[3].name, sizeof cfg[3].name, "WC");
  st[3].status = 2;
  st[3].position = 10;
  st[3].stmTargetKnown = true;
  st[3].stmTarget = 30;
  st[3].calibrating = true;
  v[3].sensorSlot[1] = 9;
  v[3].sensorName[1] = nullptr;
  v[3].sensorValid[1] = true;
  v[3].sensorTenths[1] = -5;
  st[4].status = 9;
  st[4].position = 77;
  const std::string j = build([&](JsonWriter& jw) { return writeLegacyValvesJson(jw, v, 5); });
  const std::string expected =
      "{\"valves\":["
      "{\"idx\":1,\"name\":\"Bad\",\"state\":1,\"pos\":40,\"meanCur\":12,\"targetPos\":55,"
      "\"link\":0,\"moves\":7,\"oc\":300,\"cc\":310,\"dc\":4,\"cr\":2,"
      "\"tIdxName1\":\"Floor\",\"temp1\":21.5,\"tIdxName2\":\"Wall\",\"temp2\":\"failed\","
      "\"controlActive\":0},"
      "{\"idx\":4,\"name\":\"WC\",\"state\":2,\"pos\":10,\"meanCur\":0,\"targetPos\":30,"
      "\"link\":0,\"moves\":0,\"oc\":0,\"cc\":0,\"dc\":0,\"cr\":0,"
      "\"tIdxName2\":\"\",\"temp2\":-0.5,\"controlActive\":0,\"calibration\":1},"
      "{\"idx\":5,\"name\":\"\",\"state\":9,\"pos\":77,\"meanCur\":0,\"targetPos\":77,"
      "\"link\":0,\"moves\":0,\"oc\":0,\"cc\":0,\"dc\":0,\"cr\":0,\"controlActive\":0}]}";
  CHECK(j == expected);
  checkOverflow([&](JsonWriter& jw) { return writeLegacyValvesJson(jw, v, 5); }, j.size());
  CHECK(build([&](JsonWriter& jw) { return writeLegacyValvesJson(jw, nullptr, 5); }) ==
        "{\"valves\":[]}");
  CHECK(build([&](JsonWriter& jw) { return writeLegacyValvesJson(jw, v, 0); }) ==
        "{\"valves\":[]}");
  // a view without state or config is skipped
  v[0].state = nullptr;
  v[4].config = nullptr;
  const std::string k = build([&](JsonWriter& jw) { return writeLegacyValvesJson(jw, v, 5); });
  CHECK(k.find("\"idx\":1,") == std::string::npos);
  CHECK(k.find("\"idx\":5,") == std::string::npos);
  CHECK(k.find("\"idx\":4,") != std::string::npos);
}

TEST_CASE("legacy /temps document") {
  SensorView t[6];
  t[0].slot = 1;
  t[0].name = "Floor";
  t[0].active = true;
  t[0].id = oid("28-84-37-94-97-ff-03-23");
  t[0].valid = true;
  t[0].value = 215;
  t[0].valve = 2;  // used by a valve
  t[1].slot = 2;
  t[1].name = "Outside";
  t[1].active = true;
  t[1].id = oid("28-11-22-33-44-55-66-8c");
  t[1].valid = false;  // stale
  t[1].value = 100;
  t[2].slot = 3;  // inactive
  t[2].id = oid("28-11-22-33-44-55-66-8c");
  t[3].slot = 4;  // no id
  t[3].active = true;
  t[4].slot = 0;  // bus sensor without a slot
  t[4].active = true;
  t[4].id = oid("28-84-37-94-97-ff-03-23");
  t[5].slot = 6;
  t[5].name = nullptr;
  t[5].active = true;
  t[5].id = oid("28-84-37-94-97-ff-03-23");
  t[5].valid = true;
  t[5].value = -12;
  t[5].valve = kNoValve;
  const std::string j =
      build([&](JsonWriter& jw) { return writeLegacyTempsJson(jw, t, 6, false); });
  const std::string expected =
      "[{\"id\":\"28-11-22-33-44-55-66-8c\",\"name\":\"Outside\",\"temp\":\"failed\"},"
      "{\"id\":\"28-84-37-94-97-ff-03-23\",\"name\":\"\",\"temp\":-1.2}]";
  CHECK(j == expected);
  const std::string all =
      build([&](JsonWriter& jw) { return writeLegacyTempsJson(jw, t, 6, true); });
  CHECK(all ==
        "[{\"id\":\"28-84-37-94-97-ff-03-23\",\"name\":\"Floor\",\"temp\":21.5}," +
            expected.substr(1));
  checkOverflow([&](JsonWriter& jw) { return writeLegacyTempsJson(jw, t, 6, true); }, all.size());
  t[0].valve = kValveCount;  // out of range counts as unused
  CHECK(build([&](JsonWriter& jw) { return writeLegacyTempsJson(jw, t, 1, false); }) ==
        "[{\"id\":\"28-84-37-94-97-ff-03-23\",\"name\":\"Floor\",\"temp\":21.5}]");
  CHECK(build([&](JsonWriter& jw) { return writeLegacyTempsJson(jw, nullptr, 3, true); }) == "[]");
}

TEST_CASE("legacy /volts document") {
  SensorView v[4];
  v[0].slot = 1;
  v[0].name = "Supply";
  v[0].active = true;
  v[0].id = oid("26-11-22-33-44-55-66-29");
  v[0].valid = true;
  v[0].raw = 1208;
  v[0].value = 12080;  // (1208 / 100 + 0) x 1, in milli-units
  v[0].unit = "V";
  v[1].slot = 2;
  v[1].name = nullptr;
  v[1].active = true;
  v[1].id = oid("26-11-22-33-44-55-66-29");
  v[1].valid = false;
  v[1].unit = nullptr;
  v[2].slot = 3;  // inactive
  v[2].id = oid("26-11-22-33-44-55-66-29");
  v[3].slot = 0;  // unconfigured
  v[3].active = true;
  v[3].id = oid("26-11-22-33-44-55-66-29");
  const std::string j = build([&](JsonWriter& jw) { return writeLegacyVoltsJson(jw, v, 4); });
  CHECK(j ==
        "[{\"id\":\"26-11-22-33-44-55-66-29\",\"name\":\"Supply\",\"unit\":\"V\","
        "\"value\":12.080},"
        "{\"id\":\"26-11-22-33-44-55-66-29\",\"name\":\"\",\"unit\":\"\",\"value\":\"failed\"}]");
  checkOverflow([&](JsonWriter& jw) { return writeLegacyVoltsJson(jw, v, 4); }, j.size());
  CHECK(build([&](JsonWriter& jw) { return writeLegacyVoltsJson(jw, nullptr, 3); }) == "[]");
  v[0].active = false;
  v[1].id = OneWireId{};
  CHECK(build([&](JsonWriter& jw) { return writeLegacyVoltsJson(jw, v, 4); }) == "[]");
}
