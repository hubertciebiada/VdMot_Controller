// Log sinks: when the RAM log is written to the file (flash wear), which
// events go to the file and to syslog, rotation steps, gap lines and the
// syslog packet. Hardware-free.
#pragma once

#include <stddef.h>
#include <stdint.h>

#include "vdm/event_log.h"

namespace vdm {

struct LogFlushParams {
  uint32_t periodMs = 300000;          // every 5 min
  uint32_t urgentGapMs = 10000;        // Warning+ waits at most this long after the last
                                       // attempt; also the back-off after a failed attempt
  uint32_t backlogHigh = 256;          // half the RAM ring (logger::kEventCapacity 512)
  uint32_t failureReportMs = 3600000;  // LogWriteFailed at most hourly
};

// The RAM ring is the buffer: the file cursor lags behind it and the
// backlog is written only when due() says so.
class LogFlushPolicy {
 public:
  explicit LogFlushPolicy(const LogFlushParams& p = LogFlushParams()) : p_(p) {}
  void begin(uint32_t nowMs);  // boot: the periodic timer starts here
  // backlog = events after the file cursor (all severities); urgent = a
  // Warning+ event is among them; requested = logger::requestFlush() or a
  // forced flush.
  //  backlog 0 -> false; last attempt failed and < urgentGapMs ago -> false;
  //  requested or backlog >= backlogHigh -> true; urgent and >= urgentGapMs
  //  since the last attempt -> true; >= periodMs since the last attempt -> true.
  bool due(uint32_t backlog, bool urgent, bool requested, uint32_t nowMs) const;
  // An attempt ended (ok: every line it tried was written). Returns true when
  // a LogWriteFailed event is due (first failure, then at most every
  // failureReportMs).
  bool onAttempt(bool ok, uint32_t nowMs);
  uint32_t attempts() const { return attempts_; }
  uint32_t failures() const { return failures_; }
  bool attempted() const { return attempts_ != 0; }
  uint32_t lastAttemptMs() const { return lastMs_; }

 private:
  LogFlushParams p_;
  uint32_t lastMs_ = 0;  // the last attempt, begin() before the first
  bool lastFailed_ = false;
  uint32_t attempts_ = 0;
  uint32_t failures_ = 0;
  bool reported_ = false;
  uint32_t reportMs_ = 0;
};

// Info and above go to the file; Debug stays in RAM, serial and syslog.
bool fileWantsSeverity(Severity s);
// Syslog level 1 Warning+, 2 Info+, 3 everything, else nothing.
bool syslogWants(uint8_t level, Severity s);

enum class LogFileStep : uint8_t { Append, Rotate, Defer };
//  size + len <= maxBytes -> Append; !rotationBlocked -> Rotate;
//  size + len <= maxBytes + slackBytes -> Append (a reader holds a file); else Defer.
LogFileStep logFileStep(size_t fileSize, size_t lineLen, bool rotationBlocked, size_t maxBytes,
                        size_t slackBytes);

struct LogGap {
  uint32_t from = 0;
  uint32_t to = 0;
};
// Events between the file cursor and the oldest event still in the ring.
// firstSeq 0 (empty ring) or firstSeq <= cursor + 1 -> false.
bool detectLogGap(uint32_t cursor, uint32_t firstSeq, LogGap& out);
// "#<from>-<to> gap: <n> events not written"; returns chars written
// (NUL-terminated, truncated).
size_t formatLogGapLine(const LogGap& g, char* out, size_t cap);

// RFC 5424 packet: "<PRI>1 TIMESTAMP HOSTNAME vdmot - <code name> - <msg>",
// facility local0, TIMESTAMP "-" before the clock is set, HOSTNAME "-" when
// empty. Returns chars written (NUL-terminated, truncated), 0 for cap 0.
size_t formatSyslog(const Event& e, const char* msg, const char* host, char* out, size_t cap);

}  // namespace vdm
