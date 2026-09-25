#include "vdm/log_sink.h"

#include <stdio.h>

#include "vdm/common.h"

namespace vdm {

void LogFlushPolicy::begin(uint32_t nowMs) {
  lastMs_ = nowMs;
  lastFailed_ = false;
}

bool LogFlushPolicy::due(uint32_t backlog, bool urgent, bool requested, uint32_t nowMs) const {
  if (backlog == 0) return false;
  const uint32_t since = elapsedMs(nowMs, lastMs_);
  if (lastFailed_ && since < p_.urgentGapMs) return false;
  if (requested || backlog >= p_.backlogHigh) return true;
  if (urgent && since >= p_.urgentGapMs) return true;
  return since >= p_.periodMs;
}

bool LogFlushPolicy::onAttempt(bool ok, uint32_t nowMs) {
  ++attempts_;
  lastMs_ = nowMs;
  lastFailed_ = !ok;
  if (ok) return false;
  ++failures_;
  if (reported_ && elapsedMs(nowMs, reportMs_) < p_.failureReportMs) return false;
  reported_ = true;
  reportMs_ = nowMs;
  return true;
}

bool fileWantsSeverity(Severity s) { return s >= Severity::Info; }

bool syslogWants(uint8_t level, Severity s) {
  switch (level) {
    case 1: return s >= Severity::Warning;
    case 2: return s >= Severity::Info;
    case 3: return true;
    default: return false;
  }
}

LogFileStep logFileStep(size_t fileSize, size_t lineLen, bool rotationBlocked, size_t maxBytes,
                        size_t slackBytes) {
  const size_t after = fileSize + lineLen;
  if (after <= maxBytes) return LogFileStep::Append;
  if (!rotationBlocked) return LogFileStep::Rotate;
  return after <= maxBytes + slackBytes ? LogFileStep::Append : LogFileStep::Defer;
}

bool detectLogGap(uint32_t cursor, uint32_t firstSeq, LogGap& out) {
  if (firstSeq == 0 || firstSeq <= cursor + 1) return false;
  out.from = cursor + 1;
  out.to = firstSeq - 1;
  return true;
}

namespace {

// snprintf result -> chars in `out` (the formats here never fail).
size_t finish(int n, size_t cap) {
  const size_t len = static_cast<size_t>(n);
  return len < cap ? len : cap - 1;
}

}  // namespace

size_t formatLogGapLine(const LogGap& g, char* out, size_t cap) {
  if (out == nullptr || cap == 0) return 0;
  const int n = snprintf(out, cap, "#%lu-%lu gap: %lu events not written",
                         static_cast<unsigned long>(g.from), static_cast<unsigned long>(g.to),
                         static_cast<unsigned long>(g.to - g.from + 1));
  return finish(n, cap);
}

size_t formatSyslog(const Event& e, const char* msg, const char* host, char* out, size_t cap) {
  if (out == nullptr || cap == 0) return 0;
  char ts[26] = "-";
  if (e.epoch != 0) {
    formatUtcTimestamp(e.epoch, ts, sizeof ts);
    ts[19] = 'Z';  // "...T08:13:40+00:00" -> "...T08:13:40Z"
    ts[20] = '\0';
  }
  const unsigned pri = 16u * 8u + syslogSeverity(e.severity);
  const int n = snprintf(out, cap, "<%u>1 %s %s vdmot - %s - %s", pri, ts,
                         host != nullptr && host[0] != '\0' ? host : "-", eventCodeName(e.code),
                         msg != nullptr ? msg : "");
  return finish(n, cap);
}

}  // namespace vdm
