// Smoke tests of src/logger.cpp: RAM log, serial mirror, file sink with rotation, syslog.
#include <IPAddress.h>
#include <LittleFS.h>

#include <algorithm>
#include <string>

#include "glue_test.h"
#include "logger.h"

namespace {

std::string lineOf(const vdm::Event& e) {
  char line[160];
  vdm::formatEventLine(e, line, sizeof line);
  return line;
}

std::vector<vdm::Event> all() {
  std::vector<vdm::Event> out(logger::kEventCapacity);
  uint32_t next = 0;
  out.resize(logger::readSince(0, out.data(), out.size(), next));
  return out;
}

void mountFs() {
  REQUIRE(LittleFS.begin(false));
  fakes::fs().mkdirs("/log");
}

}  // namespace

TEST_CASE("logger log: seq from 1, uptime and wall clock filled, the line goes to Serial") {
  glue::begin();
  logger::begin();
  fakes::setMs(65000);
  fakes::setWallClock(1790136000);
  CHECK(logger::log(vdm::EventCode::NetUp, vdm::kNoValve, 1, 0, "10.0.0.2") == 1);
  CHECK(logger::logSev(vdm::EventCode::Boot, vdm::Severity::Warning, 3, -1, 2) == 2);
  const std::vector<vdm::Event> ev = all();
  REQUIRE(ev.size() == 2);
  CHECK(ev[0].uptimeS == 65);
  CHECK(ev[0].epoch == 1790136000);
  CHECK(ev[0].severity == vdm::eventDefaultSeverity(vdm::EventCode::NetUp));
  CHECK(ev[1].severity == vdm::Severity::Warning);
  CHECK(ev[1].valve == 3);
  CHECK(logger::lastSeq() == 2);
  CHECK(fakes::serial(0).lines == std::vector<std::string>{lineOf(ev[0]), lineOf(ev[1])});
}

TEST_CASE("logger log: works before begin, the epoch is 0 before 2020") {
  glue::begin();
  fakes::setWallClock(1577836799);  // 2019-12-31T23:59:59Z
  logger::log(vdm::EventCode::Boot);
  fakes::setWallClock(1577836800);
  logger::log(vdm::EventCode::Boot);
  const std::vector<vdm::Event> ev = all();
  REQUIRE(ev.size() == 2);
  CHECK(ev[0].epoch == 0);
  CHECK(ev[1].epoch == 1577836800);
}

TEST_CASE("logger read: filter, first and last seq") {
  glue::begin();
  for (int i = 0; i < 5; ++i) logger::log(vdm::EventCode::Boot, static_cast<uint8_t>(i));
  vdm::EventFilter f;
  f.sinceSeq = 2;
  vdm::Event out[10];
  uint32_t next = 0, first = 0, last = 0, dropped = 0;
  CHECK(logger::read(f, out, 10, next, first, last, dropped) == 3);
  CHECK(out[0].seq == 3);
  CHECK(first == 1);
  CHECK(last == 5);
  CHECK(dropped == 0);
  CHECK(next == 5);
}

TEST_CASE("logger service: pending events are appended to /log/events.log") {
  glue::begin();
  mountFs();
  logger::log(vdm::EventCode::Boot);
  logger::log(vdm::EventCode::NetUp, vdm::kNoValve, 1);
  logger::service(false);
  const std::vector<vdm::Event> ev = all();
  CHECK(fakes::fs().read("/log/events.log") == lineOf(ev[0]) + "\n" + lineOf(ev[1]) + "\n");
  logger::service(false);  // nothing new
  CHECK(fakes::fs().writeOpens == 1);
}

TEST_CASE("logger service: at most 32 events per call") {
  glue::begin();
  mountFs();
  for (int i = 0; i < 40; ++i) logger::log(vdm::EventCode::Boot);
  logger::service(false);
  const std::string file = fakes::fs().read("/log/events.log");
  CHECK(std::count(file.begin(), file.end(), '\n') == 32);
  logger::flush();
  const std::string all40 = fakes::fs().read("/log/events.log");
  CHECK(std::count(all40.begin(), all40.end(), '\n') == 40);
}

TEST_CASE("logger service: no file without persistence or without a file system") {
  glue::begin();
  mountFs();
  logger::configure(0, 0, 514, false, "VdMot");
  logger::log(vdm::EventCode::Boot);
  logger::service(false);
  CHECK_FALSE(fakes::fs().exists("/log/events.log"));
  CHECK_FALSE(logger::stats(0).persist);
  logger::configure(0, 0, 514, true, "VdMot");
  sib::storage().fsReady = false;
  logger::log(vdm::EventCode::Boot);
  logger::service(false);
  CHECK_FALSE(fakes::fs().exists("/log/events.log"));
  CHECK(logger::stats(0).persist);
}

TEST_CASE("logger service: the file rotates to events.1.log at 64 KiB") {
  glue::begin();
  mountFs();
  logger::log(vdm::EventCode::Boot);
  const std::string line = lineOf(all()[0]) + "\n";
  fakes::fs().put("/log/events.log", std::string(64 * 1024 - line.size() + 1, 'o'));
  fakes::fs().put("/log/events.1.log", "old");
  logger::service(false);
  CHECK(fakes::fs().read("/log/events.1.log").size() == 64 * 1024 - line.size() + 1);
  CHECK(fakes::fs().read("/log/events.log") == line);
}

TEST_CASE("logger service: a failed rotation leaves the file below its limit") {
  glue::begin();
  mountFs();
  logger::log(vdm::EventCode::Boot);
  const std::string full(64 * 1024 - 1, 'o');
  fakes::fs().put("/log/events.log", full);
  fakes::fs().fail("rename", "/log/events.log");
  logger::service(false);
  CHECK(fakes::fs().read("/log/events.log") == full);
}

TEST_CASE("logger service: syslog level 1 sends warnings as RFC 5424 to the server") {
  glue::begin();
  logger::configure(1, IPAddress(192, 168, 1, 9), 1514, true, "Heating Floor");
  logger::log(vdm::EventCode::Boot);  // Info: not sent at level 1
  logger::logSev(vdm::EventCode::NetDown, vdm::Severity::Warning, vdm::kNoValve, 1);
  logger::service(false);
  CHECK(fakes::net().udpSent.empty());  // network down
  logger::logSev(vdm::EventCode::NetDown, vdm::Severity::Warning, vdm::kNoValve, 2);
  logger::service(true);
  REQUIRE(fakes::net().udpSent.size() == 1);
  const fakes::UdpPacket& p = fakes::net().udpSent[0];
  CHECK(p.ip == static_cast<uint32_t>(IPAddress(192, 168, 1, 9)));
  CHECK(p.port == 1514);
  char host[64];
  vdm::buildHostname("Heating Floor", host, sizeof host);
  const std::string prefix =
      "<" + std::to_string(16 * 8 + vdm::syslogSeverity(vdm::Severity::Warning)) + ">1 - " + host +
      " vdmot - " + vdm::eventCodeName(vdm::EventCode::NetDown) + " - ";
  CHECK(p.data.compare(0, prefix.size(), prefix) == 0);
}
