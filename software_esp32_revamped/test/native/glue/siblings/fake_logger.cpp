// Sibling fake of src/logger.cpp (see siblings.h): events are recorded with their seq and kept in
// a real vdm::EventLog, so read(), readSince() and lastSeq() answer like the logger.
#include <stdarg.h>
#include <stdio.h>

#include <vdm/event_log.h>

#include "fakes/fakes.h"
#include "logger.h"
#include "sib_internal.h"
#include "siblings.h"

namespace logger {

void begin() {
  ++sib::logger().begins;
  fakes::note("logger.begin");
}

uint32_t log(const vdm::Event& e) {
  vdm::Event copy = e;
  copy.seq = sib::eventLog().append(copy);
  sib::logger().events.push_back(copy);
  fakes::note(std::string("logger.log ") + vdm::eventCodeName(copy.code));
  return copy.seq;
}

uint32_t log(vdm::EventCode code, uint8_t valve, int32_t arg1, int32_t arg2, const char* text) {
  return log(vdm::makeEvent(code, vdm::eventDefaultSeverity(code), valve, arg1, arg2, text));
}

uint32_t logSev(vdm::EventCode code, vdm::Severity sev, uint8_t valve, int32_t arg1,
                int32_t arg2, const char* text) {
  return log(vdm::makeEvent(code, sev, valve, arg1, arg2, text));
}

size_t read(const vdm::EventFilter& f, vdm::Event* out, size_t maxOut, uint32_t& nextSince,
            uint32_t& firstSeq, uint32_t& lastSeq, uint32_t& dropped) {
  const vdm::EventLog& l = sib::eventLog();
  firstSeq = l.firstSeq();
  lastSeq = l.lastSeq();
  dropped = l.dropped();
  return l.read(f, out, maxOut, nextSince);
}

uint32_t lastSeq() { return sib::eventLog().lastSeq(); }

size_t readSince(uint32_t sinceSeq, vdm::Event* out, size_t maxOut, uint32_t& nextSince) {
  vdm::EventFilter f;
  f.sinceSeq = sinceSeq;
  return sib::eventLog().read(f, out, maxOut, nextSince);
}

void configure(uint8_t syslogLevel, uint32_t syslogServer, uint16_t syslogPort, bool persist,
               const char* hostname) {
  sib::logger().configures.push_back(
      {syslogLevel, syslogServer, syslogPort, persist, hostname != nullptr ? hostname : ""});
  fakes::note("logger.configure");
}

void service(bool netUp) {
  sib::logger().services.push_back(netUp);
  fakes::note(std::string("logger.service ") + (netUp ? "1" : "0"));
}

void flush() {
  ++sib::logger().flushes;
  fakes::note("logger.flush");
}

void requestFlush() { ++sib::logger().flushRequests; }

vdm::LogHealthInfo stats(uint32_t) { return sib::logger().stats; }

void debug(const char* fmt, ...) {
  char buf[160];
  va_list ap;
  va_start(ap, fmt);
  vsnprintf(buf, sizeof buf, fmt, ap);
  va_end(ap);
  sib::logger().debugLines.push_back(buf);
}

}  // namespace logger
