// Tests of src/web_server.cpp, documents: status, valves, sensors, events, profile, motor, flash
// status, the image list and health, each compared with the document the core writes for the
// views the glue is expected to build.
#include <string.h>
#include <time.h>

#include <string>
#include <vector>

#include <vdm/json_api.h>
#include <vdm/json_writer.h>

#include "glue_test.h"
#include "web_server.h"

namespace {

using fakes::http::Exchange;
using fakes::http::Request;
using fakes::http::Response;

std::string errorBody(const std::string& code, const std::string& detail) {
  return "{\"error\":\"" + code + "\",\"detail\":\"" + detail + "\"}";
}

void start() { web::begin(); }

Response get(const std::string& url) { return fakes::http::perform(fakes::http::get(url)); }

// Writes a document with `write` into a buffer large enough for any of them.
template <typename F>
std::string doc(F write) {
  std::vector<char> buf(64 * 1024);
  vdm::JsonWriter jw(buf.data(), buf.size());
  REQUIRE(write(jw));
  REQUIRE(jw.complete());
  return std::string(jw.c_str(), jw.length());
}

vdm::OneWireId wid(uint8_t family, uint8_t n) {
  vdm::OneWireId id;
  id.b[0] = family;
  id.b[1] = n;
  id.b[7] = n;
  return id;
}

vdm::SensorView view(uint8_t slot, const char* name, bool active, const vdm::OneWireId& id) {
  vdm::SensorView v;
  v.slot = slot;
  v.name = name;
  v.active = active;
  v.id = id;
  return v;
}

vdm::SensorView& seen(vdm::SensorView& v, int32_t raw, int32_t value, bool valid, uint32_t ageS) {
  v.onBus = true;
  v.raw = raw;
  v.value = value;
  v.valid = valid;
  v.ageS = ageS;
  return v;
}

void temp(uint8_t b, const vdm::OneWireId& id, int16_t raw, bool isSeen, uint32_t lastSeenMs) {
  vdm::TempReading& r = sib::app().snapshot.temps[b];
  r.id = id;
  r.raw = raw;
  r.seen = isSeen;
  r.lastSeenMs = lastSeenMs;
}

void volt(uint8_t b, const vdm::OneWireId& id, int32_t vad, bool isSeen, uint32_t lastSeenMs) {
  vdm::VoltReading& r = sib::app().snapshot.volts[b];
  r.id = id;
  r.vad = vad;
  r.seen = isSeen;
  r.lastSeenMs = lastSeenMs;
}

std::string sensorsDoc(const std::vector<vdm::SensorView>& t,
                       const std::vector<vdm::SensorView>& v) {
  return doc([&](vdm::JsonWriter& jw) {
    return vdm::writeSensorsJson(jw, t.data(), static_cast<uint8_t>(t.size()), v.data(),
                                 static_cast<uint8_t>(v.size()));
  });
}

constexpr uint32_t kNow = 200000;

}  // namespace

// ---------------------------------------------------------------- sensors

TEST_CASE("web sensors: temperature slots, bus sensors without a slot, staleness and age") {
  glue::begin();
  fakes::setMs(kNow);
  vdm::TempSlotConfig* c = sib::storage().active.temps;
  vdm::copyString(c[0].name, sizeof c[0].name, "A");  // a name only
  c[1].active = true;                                  // active only
  c[2].id = wid(0x28, 3);                              // an id only
  vdm::copyString(c[4].name, sizeof c[4].name, "Hall");
  c[4].active = true;
  c[4].id = wid(0x28, 5);
  c[4].offset = -3;
  vdm::copyString(c[5].name, sizeof c[5].name, "Unseen");
  c[5].id = wid(0x28, 6);
  vdm::copyString(c[6].name, sizeof c[6].name, "Stale");
  c[6].id = wid(0x28, 7);
  vdm::copyString(c[7].name, sizeof c[7].name, "Sec");
  c[7].id = wid(0x28, 8);
  c[7].offset = 2;
  vdm::StmSnapshot& s = sib::app().snapshot;
  temp(0, wid(0x28, 101), 205, true, kNow - 1000);
  temp(1, wid(0x28, 3), 215, true, kNow - 60000);
  temp(2, wid(0x28, 5), 850, true, kNow);  // not a temperature
  temp(3, wid(0x28, 6), 200, false, kNow);
  temp(4, wid(0x28, 7), 100, true, kNow - 60001);
  temp(5, vdm::OneWireId{}, 190, true, kNow);  // an index without an id
  temp(6, wid(0x28, 8), 100, true, kNow - 999);
  temp(7, wid(0x28, 102), 300, true, kNow - 999);
  temp(8, wid(0x28, 103), 250, false, kNow);
  temp(9, wid(0x28, 104), 850, true, kNow);
  temp(10, wid(0x28, 105), 260, true, kNow - 60001);
  temp(11, wid(0x28, 106), 270, true, kNow - 60000);
  s.tempCount = 12;
  s.valves[0].sensorSlot[1] = 3;
  s.valves[4].sensorSlot[0] = 5;
  start();

  std::vector<vdm::SensorView> t;
  t.push_back(view(1, "A", false, vdm::OneWireId{}));
  t.push_back(view(2, "", true, vdm::OneWireId{}));
  t.push_back(view(3, "", false, wid(0x28, 3)));
  seen(t.back(), 215, 215, true, 60).valve = 0;
  t.push_back(view(5, "Hall", true, wid(0x28, 5)));
  seen(t.back(), 850, 847, false, 0).valve = 4;
  t.push_back(view(6, "Unseen", false, wid(0x28, 6)));
  seen(t.back(), 200, 200, false, 0);
  t.push_back(view(7, "Stale", false, wid(0x28, 7)));
  seen(t.back(), 100, 100, false, 60);
  t.push_back(view(8, "Sec", false, wid(0x28, 8)));
  seen(t.back(), 100, 102, true, 0);
  t.push_back(view(0, "", false, wid(0x28, 101)));
  seen(t.back(), 205, 205, true, 1);
  t.push_back(view(0, "", false, wid(0x28, 102)));
  seen(t.back(), 300, 300, true, 0);
  t.push_back(view(0, "", false, wid(0x28, 103)));
  seen(t.back(), 250, 250, false, 0);
  t.push_back(view(0, "", false, wid(0x28, 104)));
  seen(t.back(), 850, 850, false, 0);
  t.push_back(view(0, "", false, wid(0x28, 105)));
  seen(t.back(), 260, 260, false, 60);
  t.push_back(view(0, "", false, wid(0x28, 106)));
  seen(t.back(), 270, 270, true, 60);
  const Response r = get("/api/sensors");
  CHECK(r.code == 200);
  CHECK(r.contentType == "application/json");
  CHECK(r.body == sensorsDoc(t, {}));
}

TEST_CASE("web sensors: voltage slots, conversion, clamping and bus sensors without a slot") {
  glue::begin();
  fakes::setMs(kNow);
  vdm::VoltSlotConfig* c = sib::storage().active.volts;
  vdm::copyString(c[0].name, sizeof c[0].name, "V");  // a name only
  c[1].active = true;                                  // active only
  c[2].id = wid(0x26, 3);                              // an id only
  c[2].offset = 0.5f;
  c[2].factor = 2.0f;
  vdm::copyString(c[3].name, sizeof c[3].name, "Big");
  vdm::copyString(c[3].unit, sizeof c[3].unit, "V");
  c[3].active = true;
  c[3].id = wid(0x26, 4);
  c[3].offset = 1000.0f;
  c[3].factor = 1000.0f;
  vdm::copyString(c[4].name, sizeof c[4].name, "Neg");
  c[4].id = wid(0x26, 5);
  c[4].offset = 1000.0f;
  c[4].factor = -1000.0f;
  vdm::copyString(c[5].name, sizeof c[5].name, "Off");
  c[5].id = wid(0x26, 6);
  vdm::copyString(c[6].name, sizeof c[6].name, "Old");
  c[6].id = wid(0x26, 7);
  vdm::copyString(c[7].name, sizeof c[7].name, "Fail");
  c[7].id = wid(0x26, 9);
  vdm::StmSnapshot& s = sib::app().snapshot;
  volt(0, wid(0x26, 101), 700, true, kNow - 1000);
  volt(1, wid(0x26, 3), 1200, true, kNow - 60000);
  volt(2, wid(0x26, 4), 150000, true, kNow);
  volt(3, wid(0x26, 5), 150000, true, kNow);
  volt(4, wid(0x26, 6), 500, false, kNow);
  volt(5, wid(0x26, 7), 500, true, kNow - 60001);
  volt(6, wid(0x26, 9), vdm::kVadFailed, true, kNow - 999);
  volt(7, wid(0x26, 102), 800, true, kNow - 999);
  s.voltCount = 8;
  start();

  std::vector<vdm::SensorView> v;
  v.push_back(view(1, "V", false, vdm::OneWireId{}));
  v.push_back(view(2, "", true, vdm::OneWireId{}));
  v.push_back(view(3, "", false, wid(0x26, 3)));
  seen(v.back(), 1200, 25000, true, 60);
  v.push_back(view(4, "Big", true, wid(0x26, 4)));
  seen(v.back(), 150000, 2000000000, true, 0).unit = "V";
  v.push_back(view(5, "Neg", false, wid(0x26, 5)));
  seen(v.back(), 150000, -2000000000, true, 0);
  v.push_back(view(6, "Off", false, wid(0x26, 6)));
  seen(v.back(), 500, 5000, false, 0);
  v.push_back(view(7, "Old", false, wid(0x26, 7)));
  seen(v.back(), 500, 5000, false, 60);
  v.push_back(view(8, "Fail", false, wid(0x26, 9)));
  seen(v.back(), vdm::kVadFailed, -10000, false, 0);
  v.push_back(view(0, "", false, wid(0x26, 101)));
  seen(v.back(), 700, 0, false, 1);
  v.push_back(view(0, "", false, wid(0x26, 102)));
  seen(v.back(), 800, 0, false, 0);
  Response r = get("/api/sensors");
  CHECK(r.code == 200);
  CHECK(r.body == sensorsDoc({}, v));
  // an unseen bus sensor without a slot has no age
  s.volts[7].seen = false;
  v.back().ageS = 0;
  s.volts[0].seen = false;
  v[8].ageS = 0;
  r = get("/api/sensors");
  CHECK(r.body == sensorsDoc({}, v));
}

TEST_CASE("web sensors: readings past the bus count are ignored") {
  glue::begin();
  fakes::setMs(kNow);
  vdm::Config& c = sib::storage().active;
  vdm::copyString(c.temps[0].name, sizeof c.temps[0].name, "T");
  c.temps[0].id = wid(0x28, 1);
  vdm::copyString(c.volts[0].name, sizeof c.volts[0].name, "U");
  c.volts[0].id = wid(0x26, 1);
  vdm::StmSnapshot& s = sib::app().snapshot;
  temp(0, wid(0x28, 50), 200, true, kNow);
  temp(1, wid(0x28, 1), 210, true, kNow);  // left over from a longer list
  s.tempCount = 1;
  volt(0, wid(0x26, 50), 300, true, kNow);
  volt(1, wid(0x26, 1), 310, true, kNow);
  s.voltCount = 1;
  start();
  std::vector<vdm::SensorView> t{view(1, "T", false, wid(0x28, 1)),
                                 view(0, "", false, wid(0x28, 50))};
  seen(t[1], 200, 200, true, 0);
  std::vector<vdm::SensorView> v{view(1, "U", false, wid(0x26, 1)),
                                 view(0, "", false, wid(0x26, 50))};
  seen(v[1], 300, 0, false, 0);
  CHECK(get("/api/sensors").body == sensorsDoc(t, v));
  // left-over readings of sensors without a slot are not listed either
  temp(1, wid(0x28, 51), 220, true, kNow);
  volt(1, wid(0x26, 51), 320, true, kNow);
  CHECK(get("/api/sensors").body == sensorsDoc(t, v));
}

TEST_CASE("web sensors: a document larger than a response slot answers 500, slot freed") {
  glue::begin();
  fakes::setMs(kNow);
  vdm::Config& c = sib::storage().active;
  std::vector<vdm::SensorView> t, v;
  // every slot and every bus position in use, the names escaped to six times their length
  for (uint8_t i = 0; i < vdm::kTempSlotCount; ++i) {
    memset(c.temps[i].name, 1, vdm::kItemNameMax);
    c.temps[i].active = true;
    c.temps[i].id = wid(0x28, static_cast<uint8_t>(i + 1));
    temp(i, wid(0x28, static_cast<uint8_t>(i + 101)), -125, true, kNow - 1000000);
  }
  for (uint8_t i = 0; i < vdm::kVoltSlotCount; ++i) {
    vdm::copyString(c.volts[i].name, sizeof c.volts[i].name,
                    ("Volt" + std::to_string(100000 + i)).c_str());
    c.volts[i].id = wid(0x26, static_cast<uint8_t>(i + 1));
    volt(i, wid(0x26, static_cast<uint8_t>(i + 101)), -999, true, kNow - 1000000);
  }
  sib::app().snapshot.tempCount = vdm::kTempSlotCount;
  sib::app().snapshot.voltCount = vdm::kVoltSlotCount;
  start();
  for (uint8_t i = 0; i < vdm::kTempSlotCount; ++i) {
    t.push_back(view(static_cast<uint8_t>(i + 1), c.temps[i].name, true, c.temps[i].id));
  }
  for (uint8_t i = 0; i < vdm::kTempSlotCount; ++i) {
    t.push_back(view(0, "", false, wid(0x28, static_cast<uint8_t>(i + 101))));
    seen(t.back(), -125, -125, false, 1000);
  }
  for (uint8_t i = 0; i < vdm::kVoltSlotCount; ++i) {
    v.push_back(view(static_cast<uint8_t>(i + 1), c.volts[i].name, false, c.volts[i].id));
  }
  for (uint8_t i = 0; i < vdm::kVoltSlotCount; ++i) {
    v.push_back(view(0, "", false, wid(0x26, static_cast<uint8_t>(i + 101))));
    seen(v.back(), -999, 0, false, 1000);
  }
  REQUIRE(sensorsDoc(t, v).size() >= web::kResponseSlotSize);
  const Response r = get("/api/sensors");
  CHECK(r.code == 500);
  CHECK(r.body == errorBody("internal", "document too large"));
  // the slot is free again
  Exchange a(fakes::http::get("/api/status"));
  Exchange b(fakes::http::get("/api/status"));
  CHECK(a.finish().code == 200);
  CHECK(b.finish().code == 200);
}

// ---------------------------------------------------------------- valves

TEST_CASE("web valves: the sensors of every valve with name, offset and validity") {
  glue::begin();
  fakes::setMs(kNow);
  vdm::Config& c = sib::storage().active;
  vdm::copyString(c.temps[0].name, sizeof c.temps[0].name, "One");
  c.temps[0].offset = 1;
  vdm::copyString(c.temps[2].name, sizeof c.temps[2].name, "Three");
  c.temps[2].offset = -2;
  vdm::copyString(c.temps[33].name, sizeof c.temps[33].name, "Last");
  vdm::StmSnapshot& s = sib::app().snapshot;
  s.valves[1].sensorSlot[0] = 1;
  s.valves[1].temp1 = 215;
  s.valves[1].health = 0x0303;  // flags next to the two sensor positions
  s.valves[2].sensorSlot[1] = 34;
  s.valves[2].temp2 = 190;
  s.valves[3].sensorSlot[0] = 35;  // no such slot
  s.valves[3].temp1 = 100;
  s.valves[4].sensorSlot[0] = 3;
  s.valves[4].temp1 = vdm::kTempUnassigned;
  s.valves[4].sensorSlot[1] = 1;
  s.valves[4].temp2 = -45;
  start();
  vdm::ValveView views[vdm::kValveCount];
  for (uint8_t i = 0; i < vdm::kValveCount; ++i) {
    views[i].state = &s.valves[i];
    views[i].config = &c.valves[i];
  }
  views[1].sensorSlot[0] = 1;
  views[1].sensorName[0] = "One";
  views[1].sensorValid[0] = true;
  views[1].sensorTenths[0] = 216;
  views[2].sensorSlot[1] = 34;
  views[2].sensorName[1] = "Last";
  views[2].sensorValid[1] = true;
  views[2].sensorTenths[1] = 190;
  views[4].sensorSlot[0] = 3;
  views[4].sensorName[0] = "Three";
  views[4].sensorValid[0] = false;
  views[4].sensorTenths[0] = vdm::kTempUnassigned - 2;
  views[4].sensorSlot[1] = 1;
  views[4].sensorName[1] = "One";
  views[4].sensorValid[1] = true;
  views[4].sensorTenths[1] = -44;
  const Response r = get("/api/valves");
  CHECK(r.code == 200);
  CHECK(r.body == doc([&](vdm::JsonWriter& jw) {
          return vdm::writeValvesJson(jw, views, vdm::kValveCount, kNow);
        }));
}

// ---------------------------------------------------------------- status

TEST_CASE("web status: flash size, calibration activity and the next slot at epoch 1") {
  glue::begin();
  sib::app().snapshot.valves[5].calibrating = true;
  sib::app().calib.nextEpoch = 1;
  start();
  Response r = get("/api/status");
  CHECK(r.code == 200);
  CHECK(r.body.find("\"flash\":{\"used\":1091984,\"size\":1310720}") != std::string::npos);
  CHECK(r.body.find("\"calibration\":{\"active\":true,") != std::string::npos);
  const time_t t = 1;
  struct tm tm;
  REQUIRE(localtime_r(&t, &tm) != nullptr);
  char want[64];
  snprintf(want, sizeof want, "\"next\":\"%04d-%02d-%02dT%02d:%02d:%02d\"}", tm.tm_year + 1900,
           tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min, tm.tm_sec);
  CHECK(r.body.find(want) != std::string::npos);
  fakes::ota().hasRunning = false;
  sib::app().snapshot.valves[5].calibrating = false;
  r = get("/api/status");
  CHECK(r.body.find("\"flash\":{\"used\":1091984,\"size\":0}") != std::string::npos);
  CHECK(r.body.find("\"calibration\":{\"active\":false,") != std::string::npos);
}

// ---------------------------------------------------------------- events

namespace {

void logEvents(int n, uint8_t valve, vdm::Severity sev, const char* text = nullptr) {
  for (int i = 0; i < n; ++i) {
    logger::logSev(vdm::EventCode::ConfigSaved, sev, valve, i, 0, text);
  }
}

size_t eventCount(const std::string& body) {
  size_t n = 0;
  for (size_t at = body.find("{\"seq\":"); at != std::string::npos;
       at = body.find("{\"seq\":", at + 1)) {
    ++n;
  }
  return n;
}

}  // namespace

TEST_CASE("web events: default and maximum count, parameters and their errors") {
  glue::begin();
  logEvents(30, 0, vdm::Severity::Info);
  logEvents(20, 1, vdm::Severity::Error);
  logEvents(10, vdm::kNoValve, vdm::Severity::Warning);
  start();
  Response r = get("/api/events");
  CHECK(r.code == 200);
  CHECK(eventCount(r.body) == 50);
  CHECK(r.body.rfind("{\"first\":1,\"last\":60,\"next\":50,\"dropped\":0,\"events\":[{\"seq\":1,", 0) ==
        0);
  CHECK(eventCount(get("/api/events?limit=50").body) == 50);
  r = get("/api/events?limit=1");
  CHECK(r.code == 200);
  CHECK(eventCount(r.body) == 1);
  CHECK(r.body.find("\"next\":1,") != std::string::npos);
  r = get("/api/events?since=55");
  CHECK(eventCount(r.body) == 5);
  CHECK(r.body.find("\"events\":[{\"seq\":56,") != std::string::npos);
  // valve 2 is index 1: its events only (the system events belong to no valve)
  r = get("/api/events?valve=2");
  CHECK(r.code == 200);
  CHECK(eventCount(r.body) == 20);
  CHECK(r.body.find("\"events\":[{\"seq\":31,") != std::string::npos);
  r = get("/api/events?minSeverity=error");
  CHECK(eventCount(r.body) == 20);
  CHECK(r.body.find("\"events\":[{\"seq\":31,") != std::string::npos);
  const std::string params = errorBody("bad_request", "since/limit/valve");
  for (const char* bad : {"/api/events?since=x", "/api/events?limit=51", "/api/events?limit=0",
                          "/api/events?valve=13", "/api/events?since=x&limit=5&valve=1"}) {
    CAPTURE(bad);
    r = get(bad);
    CHECK(r.code == 400);
    CHECK(r.body == params);
  }
  r = get("/api/events?valve=0");
  CHECK(r.code == 400);
  CHECK(r.body == errorBody("bad_request", "valve"));
  r = get("/api/events?minSeverity=loud");
  CHECK(r.code == 400);
  CHECK(r.body == errorBody("bad_request", "minSeverity"));
}

TEST_CASE("web events: busy slots answer 503") {
  glue::begin();
  logEvents(3, 0, vdm::Severity::Info);
  start();
  Exchange a(fakes::http::get("/api/status"));
  Exchange b(fakes::http::get("/api/status"));
  Response r = get("/api/events");
  CHECK(r.code == 503);
  CHECK(r.body == errorBody("busy", "response buffers in use"));
  a.finish();
  r = get("/api/events");
  CHECK(r.code == 200);
  CHECK(eventCount(r.body) == 3);
  b.finish();
}

TEST_CASE("web events: a count that does not fit is halved until it does") {
  glue::begin();
  // events whose text is escaped six times its length
  const std::string text(vdm::kEventTextMax, '\x01');
  logEvents(50, 0, vdm::Severity::Info, text.c_str());
  start();
  // how many of them fit a response slot
  vdm::Event ev[50];
  vdm::EventFilter f;
  uint32_t next = 0, first = 0, last = 0, dropped = 0;
  REQUIRE(logger::read(f, ev, 50, next, first, last, dropped) == 50);
  size_t fit = 0;
  for (size_t n = 1; n <= 50; ++n) {
    std::vector<char> buf(web::kResponseSlotSize);
    vdm::JsonWriter jw(buf.data(), buf.size());
    if (vdm::writeEventsJson(jw, ev, n, first, last, ev[n - 1].seq, dropped) && jw.complete()) {
      fit = n;
    }
  }
  REQUIRE(fit >= 25);
  REQUIRE(fit < 40);  // 50 and 40 do not fit, 25 and 20 do
  const Response r = get("/api/events");
  CHECK(r.code == 200);
  CHECK(eventCount(r.body) == 25);
  CHECK(r.body.find("\"next\":25,") != std::string::npos);
  const Response forty = get("/api/events?limit=40");
  CHECK(forty.code == 200);
  CHECK(eventCount(forty.body) == 20);
}

// ---------------------------------------------------------------- STM documents

TEST_CASE("web: a valve profile, 404 without one") {
  glue::begin();
  vdm::Profile& p = sib::app().snapshot.profiles[1];
  p.valve = 1;
  p.count = 2;
  p.samples[0].count = 10;
  p.samples[0].current = 120;
  p.samples[1].count = 90;
  p.samples[1].current = 180;
  start();
  Response r = get("/api/valves/2/profile");
  CHECK(r.code == 200);
  CHECK(r.contentType == "application/json");
  CHECK(r.body == doc([&](vdm::JsonWriter& jw) { return vdm::writeProfileJson(jw, p); }));
  r = get("/api/valves/1/profile");
  CHECK(r.code == 404);
  CHECK(r.body == errorBody("not_found", "no profile"));
}

TEST_CASE("web: the motor parameters with and without breakaway") {
  glue::begin();
  vdm::StmSnapshot& s = sib::app().snapshot;
  s.haveMotor = true;
  s.motor.lowFactor = 22;
  s.motor.minCounts = 4000;
  s.learnMovements = 120;
  s.haveBreakaway = true;
  s.breakaway.enable = true;
  s.breakaway.stepPct = 15;
  s.breakaway.maxmA = 40;
  start();
  Response r = get("/api/stm/motor");
  CHECK(r.code == 200);
  CHECK(r.body == doc([&](vdm::JsonWriter& jw) {
          return vdm::writeMotorJson(jw, s.motor, 120, &s.breakaway, true);
        }));
  s.haveBreakaway = false;
  s.haveMotor = false;
  r = get("/api/stm/motor");
  CHECK(r.code == 200);
  CHECK(r.body == doc([&](vdm::JsonWriter& jw) {
          return vdm::writeMotorJson(jw, s.motor, 120, nullptr, false);
        }));
}

TEST_CASE("web: the flash status names its image") {
  glue::begin();
  vdm::StmSnapshot& s = sib::app().snapshot;
  s.flash.phase = vdm::FlashPhase::Writing;
  s.flash.percent = 40;
  vdm::copyString(s.flashImage, sizeof s.flashImage, "a");
  start();
  Response r = get("/api/stm/flash");
  CHECK(r.code == 200);
  CHECK(r.body == doc([&](vdm::JsonWriter& jw) {
          return vdm::writeFlashStatusJson(jw, s.flash, "a", false);
        }));
  s.flashImage[0] = '\0';
  r = get("/api/stm/flash");
  CHECK(r.code == 200);
  CHECK(r.body == doc([&](vdm::JsonWriter& jw) {
          return vdm::writeFlashStatusJson(jw, s.flash, nullptr, false);
        }));
}

TEST_CASE("web: the image list shows crc, version, check and board only when known") {
  glue::begin();
  storage::ImageEntry a;
  vdm::copyString(a.name, sizeof a.name, "fw");
  a.size = 1234;
  a.scanned = true;
  a.crc = 0xabcd;
  vdm::copyString(a.version, sizeof a.version, "1");
  a.check = vdm::FlashError::None;
  vdm::copyString(a.hwTag, sizeof a.hwTag, "7");
  storage::ImageEntry b;
  vdm::copyString(b.name, sizeof b.name, "old");
  b.size = 5;
  b.crc = 5;
  vdm::copyString(b.version, sizeof b.version, "9.9.9");
  storage::ImageEntry c;
  vdm::copyString(c.name, sizeof c.name, "raw");
  c.size = 7;
  c.scanned = true;
  sib::storage().images = {a, b, c};
  start();
  const std::string check = vdm::flashErrorName(vdm::FlashError::None);
  const Response r = get("/api/stm/images");
  CHECK(r.code == 200);
  CHECK(r.body == "[{\"name\":\"fw\",\"size\":1234,\"crc32\":\"0x0000abcd\",\"version\":\"1\","
                  "\"check\":\"" + check + "\",\"hw\":\"7\"},"
                  "{\"name\":\"old\",\"size\":5,\"crc32\":null,\"version\":null,\"check\":null,"
                  "\"hw\":null},"
                  "{\"name\":\"raw\",\"size\":7,\"crc32\":\"0x00000000\",\"version\":null,"
                  "\"check\":\"" + check + "\",\"hw\":null}]");
}

// ---------------------------------------------------------------- health

TEST_CASE("web health: the document may use its whole 1024-byte buffer") {
  glue::begin();
  start();
  // the longest version whose document fits 1023 characters and the terminator
  std::string version;
  size_t n = 0;
  for (size_t len = 1; len < 1100; ++len) {
    version.assign(len, 'v');
    sib::app().health.version = version.c_str();
    vdm::HealthSnapshot h;
    app::readHealth(h);
    char buf[1024];
    vdm::JsonWriter jw(buf, sizeof buf);
    if (vdm::writeHealthJson(jw, h)) n = len;
  }
  REQUIRE(n > 0);
  version.assign(n, 'v');
  sib::app().health.version = version.c_str();
  Response r = get("/api/health");
  CHECK(r.code == 200);
  CHECK(r.body.size() == 1023);
  CHECK(r.body.find("\"version\":\"" + version + "\"") != std::string::npos);
  version.assign(n + 1, 'v');
  sib::app().health.version = version.c_str();
  r = get("/api/health");
  CHECK(r.code == 500);
  CHECK(r.body == errorBody("internal", "health"));
}

// ---------------------------------------------------------------- files

TEST_CASE("web files: at most 48 entries, more are reported as truncated") {
  glue::begin();
  for (int i = 0; i < 49; ++i) {
    vdm::FileEntry e{};
    vdm::copyString(e.path, sizeof e.path, ("/f" + std::to_string(i) + ".bin").c_str());
    e.size = 10;
    sib::storage().files.push_back(e);
  }
  start();
  Response r = get("/api/files");
  CHECK(r.code == 200);
  CHECK(r.body.find("\"truncated\":true,") != std::string::npos);
  CHECK(r.body.find("{\"path\":\"/f47.bin\",") != std::string::npos);
  CHECK(r.body.find("{\"path\":\"/f48.bin\",") == std::string::npos);
  sib::storage().files.pop_back();
  r = get("/api/files");
  CHECK(r.body.find("\"truncated\":false,") != std::string::npos);
  CHECK(r.body.find("{\"path\":\"/f47.bin\",") != std::string::npos);
}
