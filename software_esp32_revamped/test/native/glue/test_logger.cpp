// Tests of src/logger.cpp: RAM log, serial mirror, file sink (flush policy, rotation, gap lines,
// write failures), syslog, statistics.
#include <IPAddress.h>
#include <LittleFS.h>

#include <algorithm>
#include <string>

#include <vdm/log_sink.h>

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

// The file lines of the events in the ring with seq in [from, to] (Info and above).
std::string linesOf(uint32_t from, uint32_t to) {
  std::string s;
  for (const vdm::Event& e : all()) {
    if (e.seq >= from && e.seq <= to && e.severity != vdm::Severity::Debug) s += lineOf(e) + "\n";
  }
  return s;
}

void mountFs() {
  REQUIRE(LittleFS.begin(false));
  fakes::fs().mkdirs("/log");
}

std::string logFile() { return fakes::fs().read("/log/events.log"); }

size_t countLines(const std::string& s) { return std::count(s.begin(), s.end(), '\n'); }

void info(int n) {
  for (int i = 0; i < n; ++i) logger::log(vdm::EventCode::NetUp, vdm::kNoValve, i);
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
  CHECK(fakes::serial(0).lines[0].compare(0, 3, "#1 ") == 0);
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

TEST_CASE("logger file: Info events wait in RAM for the 5 min flush, then one append") {
  glue::begin();
  mountFs();
  logger::begin();
  CHECK_FALSE(logger::stats(0).flushed);
  for (int i = 0; i < 20; ++i) {
    fakes::setMs(static_cast<uint64_t>(i) * 10000);
    info(1);
    logger::service(false);
  }
  fakes::setMs(299999);
  logger::service(false);
  CHECK(fakes::fs().opens == 0);
  CHECK_FALSE(fakes::fs().exists("/log/events.log"));
  vdm::LogHealthInfo s = logger::stats(299999);
  CHECK(s.persist);
  CHECK(s.backlog == 20);
  CHECK(s.flushes == 0);
  fakes::setMs(300000);
  logger::service(false);
  CHECK(fakes::fs().writeOpens == 1);
  CHECK(fakes::fs().writes == 20);
  CHECK(logFile() == linesOf(1, 20));
  s = logger::stats(305999);
  CHECK(s.backlog == 0);
  CHECK(s.flushes == 1);
  CHECK(s.flushed);
  CHECK(s.lastFlushAgeS == 5);
  CHECK(s.failures == 0);
  CHECK(s.lost == 0);
  logger::service(false);  // nothing new
  CHECK(fakes::fs().writeOpens == 1);
}

TEST_CASE("logger file: a Warning 3 s after an attempt is written 10 s after that attempt") {
  glue::begin();
  mountFs();
  logger::begin();
  fakes::setMs(1000);
  info(1);
  logger::requestFlush();
  logger::service(false);
  REQUIRE(countLines(logFile()) == 1);
  fakes::setMs(4000);
  logger::logSev(vdm::EventCode::NetDown, vdm::Severity::Warning, vdm::kNoValve, 1);
  fakes::setMs(10999);
  logger::service(false);
  CHECK(countLines(logFile()) == 1);
  fakes::setMs(11000);
  logger::service(false);
  CHECK(logFile() == linesOf(1, 2));
}

TEST_CASE("logger file: a Warning 10 s after the last attempt is written in the next pass") {
  glue::begin();
  mountFs();
  logger::begin();
  fakes::setMs(9999);
  logger::logSev(vdm::EventCode::NetDown, vdm::Severity::Warning, vdm::kNoValve, 1);
  logger::service(false);
  CHECK_FALSE(fakes::fs().exists("/log/events.log"));
  fakes::setMs(10000);
  logger::service(false);
  CHECK(logFile() == linesOf(1, 1));
}

TEST_CASE("logger file: 256 waiting events are written at once, 255 are not") {
  glue::begin();
  mountFs();
  logger::begin();
  info(255);
  logger::service(false);
  CHECK(fakes::fs().opens == 0);
  info(1);
  logger::service(false);
  CHECK(countLines(logFile()) == 256);
  CHECK(logFile() == linesOf(1, 256));
}

TEST_CASE("logger file: requestFlush writes the backlog at the next pass") {
  glue::begin();
  mountFs();
  logger::begin();
  info(2);
  logger::service(false);
  CHECK(fakes::fs().opens == 0);
  logger::requestFlush();
  CHECK(fakes::fs().opens == 0);
  logger::service(false);
  CHECK(logFile() == linesOf(1, 2));
  info(1);
  logger::service(false);  // the request was used up
  CHECK(countLines(logFile()) == 2);
}

TEST_CASE("logger file: Debug events stay out of the file but reach serial and syslog") {
  glue::begin();
  mountFs();
  logger::begin();
  logger::configure(3, IPAddress(192, 168, 1, 9), 514, true, "VdMot");
  logger::logSev(vdm::EventCode::TimeSynced, vdm::Severity::Debug, vdm::kNoValve, 5);
  logger::requestFlush();
  logger::service(true);
  CHECK(fakes::fs().opens == 0);  // a Debug-only backlog opens nothing
  CHECK(fakes::serial(0).lines.size() == 1);
  REQUIRE(fakes::net().udpSent.size() == 1);
  CHECK(fakes::net().udpSent[0].data.find(vdm::eventCodeName(vdm::EventCode::TimeSynced)) !=
        std::string::npos);
  CHECK(logger::stats(0).backlog == 0);
  info(1);
  logger::requestFlush();
  logger::service(true);
  CHECK(logFile() == linesOf(2, 2));
}

TEST_CASE("logger file: nothing without persistence or without a file system") {
  glue::begin();
  mountFs();
  logger::begin();
  logger::configure(0, 0, 514, false, "VdMot");
  info(1);
  logger::flush();
  logger::requestFlush();
  logger::service(false);
  CHECK_FALSE(fakes::fs().exists("/log/events.log"));
  CHECK_FALSE(logger::stats(0).persist);
  CHECK(logger::stats(0).backlog == 0);
  logger::configure(0, 0, 514, true, "VdMot");
  sib::storage().fsReady = false;
  info(1);
  logger::flush();
  CHECK_FALSE(fakes::fs().exists("/log/events.log"));
  CHECK(logger::stats(0).persist);
  CHECK(logger::stats(0).backlog == 0);
  CHECK(logger::stats(0).flushes == 0);
  // The events of that time are not written later.
  sib::storage().fsReady = true;
  info(1);
  logger::flush();
  CHECK(logFile() == linesOf(3, 3));
}

TEST_CASE("logger flush: writes the whole backlog now, nothing when there is none") {
  glue::begin();
  mountFs();
  logger::begin();
  logger::flush();
  CHECK(logger::stats(0).flushes == 0);
  info(300);
  logger::flush();
  CHECK(countLines(logFile()) == 300);
  CHECK(logger::stats(0).flushes == 1);
}

TEST_CASE("logger file: the file rotates to events.1.log at 64 KiB") {
  glue::begin();
  mountFs();
  logger::begin();
  info(1);
  const std::string line = lineOf(all()[0]) + "\n";
  const std::string old(64 * 1024 - line.size() + 1, 'o');
  fakes::fs().put("/log/events.log", old);
  fakes::fs().put("/log/events.1.log", "old");
  logger::flush();
  CHECK(fakes::fs().read("/log/events.1.log") == old);
  CHECK(logFile() == line);
}

TEST_CASE("logger file: a line that just fits is appended without a rotation") {
  glue::begin();
  mountFs();
  logger::begin();
  info(1);
  const std::string line = lineOf(all()[0]) + "\n";
  const std::string old(64 * 1024 - line.size(), 'o');
  fakes::fs().put("/log/events.log", old);
  logger::flush();
  CHECK(logFile() == old + line);
  CHECK(fakes::fs().renames == 0);
}

TEST_CASE("logger file: a blocked rotation grows the file up to 72 KiB, then stops") {
  glue::begin();
  mountFs();
  logger::begin();
  const std::string old(64 * 1024 - 10, 'o');
  fakes::fs().put("/log/events.log", old);
  fakes::fs().fail("rename", "/log/events.log", 1000);
  info(200);
  logger::flush();
  const std::string grown = logFile();
  CHECK(grown.size() > 64 * 1024);
  CHECK(grown.size() <= 72 * 1024);
  CHECK(grown.compare(0, old.size(), old) == 0);
  const size_t written = countLines(grown.substr(old.size()));
  CHECK(written > 100);
  CHECK(written < 200);
  CHECK(grown.substr(old.size()) == linesOf(1, static_cast<uint32_t>(written)));
  CHECK_FALSE(fakes::fs().exists("/log/events.1.log"));
  const vdm::Event report = all().back();
  CHECK(report.code == vdm::EventCode::LogWriteFailed);
  CHECK(report.arg1 == 4);
  CHECK(report.arg2 == 0);
  CHECK(logger::stats(0).failures == 1);
  CHECK(logger::stats(0).backlog == 201 - written);
  // The reader is gone: the next attempt rotates and writes the rest.
  fakes::fs().failures.clear();
  logger::flush();
  CHECK(fakes::fs().read("/log/events.1.log") == grown);
  CHECK(logFile() == linesOf(static_cast<uint32_t>(written) + 1, 201));
  CHECK(logger::stats(0).backlog == 0);
}

TEST_CASE("logger file: an open failure keeps the backlog, reports once and retries after 10 s") {
  glue::begin();
  mountFs();
  logger::begin();
  fakes::setMs(1000);
  info(2);
  fakes::fs().fail("open", "/log/events.log", 2);
  logger::requestFlush();
  logger::service(false);
  std::vector<vdm::Event> ev = all();
  REQUIRE(ev.size() == 3);
  CHECK(ev[2].code == vdm::EventCode::LogWriteFailed);
  CHECK(ev[2].arg1 == 1);
  CHECK(ev[2].severity == vdm::Severity::Warning);
  logger::requestFlush();
  fakes::setMs(10999);
  logger::service(false);
  CHECK(fakes::fs().opens == 1);  // back-off
  fakes::setMs(11000);
  logger::service(false);  // fails again, not reported again within the hour
  CHECK(fakes::fs().opens == 2);
  CHECK(all().size() == 3);
  CHECK(logger::stats(11000).failures == 2);
  fakes::setMs(21000);
  logger::service(false);  // the Warning is due after the back-off
  CHECK(logFile() == linesOf(1, 3));
  CHECK(logger::stats(21000).failures == 2);
  CHECK(logger::stats(21000).flushes == 3);
}

TEST_CASE("logger file: events lost in the ring become one gap line") {
  glue::begin();
  mountFs();
  logger::begin();
  fakes::setMs(1000);
  fakes::fs().fail("open", "/log/events.log", 1000);
  info(300);
  logger::service(false);  // due (256 waiting), the open fails
  REQUIRE(all().back().code == vdm::EventCode::LogWriteFailed);
  info(299);  // 600 events: the ring (512) lost the oldest 88
  fakes::fs().failures.clear();
  fakes::setMs(11000);
  logger::service(false);
  const uint32_t f = all().front().seq;
  REQUIRE(f == 89);
  const std::string gap = "#1-" + std::to_string(f - 1) + " gap: " + std::to_string(f - 1) +
                          " events not written\n";
  CHECK(logFile() == gap + linesOf(f, 600));
  CHECK(logger::stats(11000).lost == f - 1);
  CHECK(logger::stats(11000).backlog == 0);
}

TEST_CASE("logger file: events the ring drops while the file is written become a gap line") {
  glue::begin();
  mountFs();
  logger::begin();
  info(300);  // 256 waiting: due
  const std::vector<vdm::Event> before = all();
  std::string first4;
  for (size_t i = 0; i < 4; ++i) first4 += lineOf(before[i]) + "\n";
  bool burst = false;
  fakes::fs().onWrite = [&burst](const std::string& path) {
    if (path != "/log/events.log" || burst) return;
    burst = true;
    info(600);  // another task meanwhile: the ring (512) drops seq 1..388
  };
  logger::service(false);
  fakes::fs().onWrite = nullptr;
  REQUIRE(burst);
  REQUIRE(all().front().seq == 389);
  // lines 1..4 were read before the drop, the next batch starts at 389
  const std::string head = first4 + "#5-388 gap: 384 events not written\n" + linesOf(389, 392);
  CHECK(logFile() == head);
  CHECK(logger::stats(0).lost == 384);
  logger::flush();
  CHECK(logFile() == head + linesOf(393, 900));
  CHECK(logger::stats(0).lost == 384);
}

TEST_CASE("logger file: a short write keeps the cursor at the last complete line") {
  glue::begin();
  mountFs();
  logger::begin();
  info(8);
  std::vector<vdm::Event> ev = all();
  size_t four = 0;
  for (int i = 0; i < 4; ++i) four += lineOf(ev[static_cast<size_t>(i)]).size() + 1;
  // Lines 1-4 fill the file's block exactly and there is no free block for line 5.
  const std::string old(4096 - four, 'o');
  fakes::fs().put("/log/events.log", old);
  fakes::fs().totalBytes = fakes::fs().usedBytes();
  logger::flush();
  CHECK(logFile() == old + linesOf(1, 4));
  CHECK(logger::stats(0).backlog == 5);  // lines 5-8 and the LogWriteFailed report
  CHECK(all().back().code == vdm::EventCode::LogWriteFailed);
  CHECK(all().back().arg1 == 2);
  fakes::fs().totalBytes = 0x170000;
  logger::flush();
  CHECK(logFile() == old + linesOf(1, 9));
}

TEST_CASE("logger syslog: level 1 sends warnings as RFC 5424 to the server") {
  glue::begin();
  logger::configure(1, IPAddress(192, 168, 1, 9), 1514, true, "Heating Floor");
  logger::log(vdm::EventCode::Boot);  // Info: not sent at level 1
  logger::logSev(vdm::EventCode::NetDown, vdm::Severity::Warning, vdm::kNoValve, 1);
  logger::service(false);
  CHECK(fakes::net().udpSent.empty());  // network down: not sent, not kept
  logger::logSev(vdm::EventCode::NetDown, vdm::Severity::Warning, vdm::kNoValve, 2);
  logger::service(true);
  REQUIRE(fakes::net().udpSent.size() == 1);
  const fakes::UdpPacket& p = fakes::net().udpSent[0];
  CHECK(p.ip == static_cast<uint32_t>(IPAddress(192, 168, 1, 9)));
  CHECK(p.port == 1514);
  char host[64];
  vdm::buildHostname("Heating Floor", host, sizeof host);
  const vdm::Event e = all().back();
  char msg[120];
  vdm::formatEventMessage(e, msg, sizeof msg);
  char want[240];
  vdm::formatSyslog(e, msg, host, want, sizeof want);
  CHECK(p.data == want);
}

TEST_CASE("logger syslog: at most 32 events per pass, no server or port -> nothing") {
  glue::begin();
  logger::configure(3, IPAddress(192, 168, 1, 9), 514, true, "VdMot");
  info(40);
  logger::service(true);
  CHECK(fakes::net().udpSent.size() == 32);
  logger::service(true);
  CHECK(fakes::net().udpSent.size() == 40);
  logger::configure(3, 0, 514, true, "VdMot");
  info(1);
  logger::service(true);
  logger::configure(3, IPAddress(192, 168, 1, 9), 0, true, "VdMot");
  info(1);
  logger::service(true);
  logger::configure(0, IPAddress(192, 168, 1, 9), 514, true, "VdMot");
  info(1);
  logger::service(true);
  CHECK(fakes::net().udpSent.size() == 40);
}
