// Tests of src/logger.cpp: buffer limits of the longest lines, host name length, statistics,
// the file handle and the flush backlog after a written cursor.
#include <IPAddress.h>
#include <LittleFS.h>

#include <stdint.h>
#include <string.h>

#include <string>
#include <vector>

#include <vdm/log_sink.h>

#include "glue_test.h"
#include "logger.h"

namespace {

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

void info(int n) {
  for (int i = 0; i < n; ++i) logger::log(vdm::EventCode::NetUp, vdm::kNoValve, i);
}

const char kLongText[] = "abcdefghijklmnopqrstuvw";  // kEventTextMax chars

// The event code whose message is the longest with the widest arguments and text.
vdm::EventCode longestCode() {
  vdm::EventCode best = vdm::EventCode::Boot;
  size_t bestLen = 0;
  for (uint16_t c = 0; c < 1000; ++c) {
    const vdm::EventCode code = static_cast<vdm::EventCode>(c);
    if (std::string(vdm::eventCodeName(code)) == "unknown") continue;
    const vdm::Event e =
        vdm::makeEvent(code, vdm::Severity::Warning, 11, INT32_MIN, INT32_MIN, kLongText);
    char msg[512];
    const size_t n = vdm::formatEventMessage(e, msg, sizeof msg);
    if (n > bestLen) {
      bestLen = n;
      best = code;
    }
  }
  return best;
}

}  // namespace

TEST_CASE("logger: the longest event is cut to the serial, file and syslog buffers") {
  glue::begin();
  mountFs();
  logger::begin();
  logger::configure(3, IPAddress(192, 168, 1, 9), 514, true, "VdMot");
  fakes::setWallClock(1790136000);
  const vdm::EventCode code = longestCode();
  logger::logSev(code, vdm::Severity::Warning, 11, INT32_MIN, INT32_MIN, kLongText);
  logger::service(true);
  logger::flush();
  const std::vector<vdm::Event> ev = all();
  REQUIRE(ev.size() == 1);
  const vdm::Event& e = ev[0];
  char line[160];
  vdm::formatEventLine(e, line, sizeof line);
  char full[512];
  vdm::formatEventLine(e, full, sizeof full);
  MESSAGE("longest line " << strlen(full));
  CHECK(strlen(full) > 160);  // the buffers below matter
  REQUIRE(fakes::serial(0).lines.size() == 1);
  CHECK(fakes::serial(0).lines[0] == line);
  CHECK(fakes::fs().read("/log/events.log") == std::string(line) + "\n");
  char msg[120];
  char fullMsg[512];
  CHECK(vdm::formatEventMessage(e, fullMsg, sizeof fullMsg) < sizeof msg - 1);  // never the limit
  vdm::formatEventMessage(e, msg, sizeof msg);
  char want[240];
  char host[64];
  vdm::buildHostname("VdMot", host, sizeof host);
  const size_t n = vdm::formatSyslog(e, msg, host, want, sizeof want);
  CHECK(n < sizeof want - 1);  // the packet buffer is never the limit
  REQUIRE(fakes::net().udpSent.size() == 1);
  CHECK(fakes::net().udpSent[0].data == want);
}

TEST_CASE("logger syslog: the host name is cut to the station name length") {
  glue::begin();
  logger::configure(1, IPAddress(192, 168, 1, 9), 514, true, "abcdefghijklmnopqrstuvwxyz");
  logger::logSev(vdm::EventCode::NetDown, vdm::Severity::Warning, vdm::kNoValve, 1);
  logger::service(true);
  REQUIRE(fakes::net().udpSent.size() == 1);
  const vdm::Event e = all().back();
  char msg[120];
  vdm::formatEventMessage(e, msg, sizeof msg);
  char want[240];
  vdm::formatSyslog(e, msg, "abcdefghijklmnopqrst", want, sizeof want);
  CHECK(fakes::net().udpSent[0].data == want);
}

TEST_CASE("logger file: the file is closed after the attempt; the backlog counts from the cursor") {
  glue::begin();
  mountFs();
  logger::begin();
  fakes::setMs(0);
  CHECK(logger::stats(5000).lastFlushAgeS == 0);  // never flushed
  info(300);
  logger::flush();
  CHECK(fakes::fs().openHandles == 0);
  CHECK(fakes::fs().writes == 300);
  CHECK(logger::stats(1000000).lastFlushAgeS == 1000);
  CHECK(logger::stats(999999).lastFlushAgeS == 999);
  info(1);
  logger::service(false);  // 1 Info waiting: not due
  CHECK(fakes::fs().writes == 300);
  CHECK(logger::stats(0).backlog == 1);
}
