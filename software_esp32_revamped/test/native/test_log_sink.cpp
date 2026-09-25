// Log sinks: flush policy, severity filters, rotation steps, gap lines, syslog packets.
#include <string.h>

#include <string>

#include "doctest.h"
#include "vdm/log_sink.h"

using namespace vdm;

TEST_CASE("LogFlushPolicy: triggers after begin at 1000") {
  LogFlushPolicy p;
  p.begin(1000);
  CHECK_FALSE(p.attempted());
  CHECK(p.lastAttemptMs() == 1000);
  CHECK_FALSE(p.due(0, true, true, 1000));
  CHECK_FALSE(p.due(0, true, true, 4000000000u));
  CHECK(p.due(1, false, true, 1000));
  CHECK(p.due(256, false, false, 1000));
  CHECK_FALSE(p.due(255, false, false, 1000));
  CHECK_FALSE(p.due(1, true, false, 1000 + 9999));
  CHECK(p.due(1, true, false, 1000 + 10000));
  CHECK_FALSE(p.due(255, false, false, 1000 + 299999));
  CHECK(p.due(1, false, false, 1000 + 300000));
}

TEST_CASE("LogFlushPolicy: attempts, failures and the back-off after a failure") {
  LogFlushPolicy p;
  p.begin(0);
  const uint32_t t = 500000;
  CHECK(p.onAttempt(false, t));  // the first failure is reported
  CHECK(p.attempted());
  CHECK(p.attempts() == 1);
  CHECK(p.failures() == 1);
  CHECK(p.lastAttemptMs() == t);
  CHECK_FALSE(p.due(300, false, true, t + 9999));
  CHECK_FALSE(p.due(300, true, true, t + 9999));
  CHECK(p.due(300, false, true, t + 10000));
  CHECK_FALSE(p.onAttempt(false, t + 3599999));
  CHECK(p.failures() == 2);
  CHECK(p.onAttempt(false, t + 3600000));
  CHECK_FALSE(p.onAttempt(true, t + 3600001));
  CHECK(p.attempts() == 4);
  CHECK(p.failures() == 3);
  // After a success no back-off: a request is due at once.
  CHECK(p.due(1, false, true, t + 3600001));
  // The report interval counts from the last report, not from the last failure.
  CHECK_FALSE(p.onAttempt(false, t + 3600002));
  CHECK(p.onAttempt(false, t + 7200000));
}

TEST_CASE("LogFlushPolicy: an ok attempt restarts the periodic timer") {
  LogFlushPolicy p;
  p.begin(0);
  CHECK_FALSE(p.onAttempt(true, 100000));
  CHECK_FALSE(p.due(1, false, false, 399999));
  CHECK(p.due(1, false, false, 400000));
  CHECK_FALSE(p.due(1, true, false, 109999));
  CHECK(p.due(1, true, false, 110000));
}

TEST_CASE("LogFlushPolicy: begin near the 32-bit wrap and custom parameters") {
  LogFlushPolicy p;
  const uint32_t s = 0xFFFFF000u;
  p.begin(s);
  CHECK_FALSE(p.due(1, true, false, s + 9999u));
  CHECK(p.due(1, true, false, s + 10000u));
  CHECK_FALSE(p.due(1, false, false, s + 299999u));
  CHECK(p.due(1, false, false, s + 300000u));
  LogFlushParams prm;
  prm.periodMs = 50;
  prm.urgentGapMs = 5;
  prm.backlogHigh = 3;
  prm.failureReportMs = 20;
  LogFlushPolicy c(prm);
  c.begin(0);
  CHECK_FALSE(c.due(2, false, false, 49));
  CHECK(c.due(2, false, false, 50));
  CHECK(c.due(3, false, false, 1));
  CHECK_FALSE(c.due(2, true, false, 4));
  CHECK(c.due(2, true, false, 5));
  CHECK(c.onAttempt(false, 100));
  CHECK_FALSE(c.due(3, false, true, 104));
  CHECK(c.due(3, false, true, 105));
  CHECK_FALSE(c.onAttempt(false, 119));
  CHECK(c.onAttempt(false, 120));
}

TEST_CASE("LogFlushPolicy: reference day (one Info per 10 s, one Warning per hour at :30)" *
          doctest::test_suite("slow")) {
  LogFlushPolicy p;
  p.begin(0);
  uint32_t backlog = 0;
  bool urgent = false;
  for (uint32_t t = 0; t < 24u * 3600u * 1000u; t += 100) {
    if (t % 10000 == 0) ++backlog;
    if (t % 3600000 == 1800000) {
      ++backlog;
      urgent = true;
    }
    if (p.due(backlog, urgent, false, t)) {
      p.onAttempt(true, t);
      backlog = 0;
      urgent = false;
    }
  }
  CHECK(p.attempts() >= 280);
  CHECK(p.attempts() <= 312);
  CHECK(p.failures() == 0);
}

TEST_CASE("fileWantsSeverity and syslogWants") {
  CHECK_FALSE(fileWantsSeverity(Severity::Debug));
  CHECK(fileWantsSeverity(Severity::Info));
  CHECK(fileWantsSeverity(Severity::Warning));
  CHECK(fileWantsSeverity(Severity::Error));
  CHECK(fileWantsSeverity(Severity::Critical));
  const Severity all[] = {Severity::Debug, Severity::Info, Severity::Warning, Severity::Error,
                          Severity::Critical};
  for (Severity s : all) {
    const int v = static_cast<int>(s);
    CHECK_FALSE(syslogWants(0, s));
    CHECK(syslogWants(1, s) == (v >= 2));
    CHECK(syslogWants(2, s) == (v >= 1));
    CHECK(syslogWants(3, s));
    CHECK_FALSE(syslogWants(4, s));
  }
}

TEST_CASE("logFileStep with max 65536 and slack 8192") {
  CHECK(logFileStep(65526, 10, false, 65536, 8192) == LogFileStep::Append);
  CHECK(logFileStep(65526, 11, false, 65536, 8192) == LogFileStep::Rotate);
  CHECK(logFileStep(65526, 11, true, 65536, 8192) == LogFileStep::Append);
  CHECK(logFileStep(73718, 10, true, 65536, 8192) == LogFileStep::Append);
  CHECK(logFileStep(73718, 11, true, 65536, 8192) == LogFileStep::Defer);
  CHECK(logFileStep(0, 0, true, 0, 0) == LogFileStep::Append);
}

TEST_CASE("detectLogGap") {
  LogGap g;
  CHECK_FALSE(detectLogGap(10, 11, g));
  CHECK_FALSE(detectLogGap(10, 5, g));
  CHECK_FALSE(detectLogGap(10, 0, g));
  CHECK_FALSE(detectLogGap(0, 1, g));
  CHECK(g.from == 0);
  CHECK(g.to == 0);
  CHECK(detectLogGap(10, 12, g));
  CHECK(g.from == 11);
  CHECK(g.to == 11);
  CHECK(detectLogGap(10, 15, g));
  CHECK(g.from == 11);
  CHECK(g.to == 14);
  CHECK(detectLogGap(0, 600, g));
  CHECK(g.from == 1);
  CHECK(g.to == 599);
}

TEST_CASE("formatLogGapLine") {
  char out[64];
  LogGap g;
  g.from = 11;
  g.to = 14;
  CHECK(formatLogGapLine(g, out, sizeof out) == 32);
  CHECK(std::string(out) == "#11-14 gap: 4 events not written");
  g.from = 1;
  g.to = 4294967294u;
  formatLogGapLine(g, out, sizeof out);
  CHECK(std::string(out) == "#1-4294967294 gap: 4294967294 events not written");
  g.from = 11;
  g.to = 14;
  CHECK(formatLogGapLine(g, out, 1) == 0);
  CHECK(std::string(out).empty());
  CHECK(formatLogGapLine(g, out, 6) == 5);
  CHECK(std::string(out) == "#11-1");
  out[0] = 'q';
  CHECK(formatLogGapLine(g, out, 0) == 0);
  CHECK(out[0] == 'q');
  CHECK(formatLogGapLine(g, nullptr, 10) == 0);
}

TEST_CASE("formatSyslog") {
  Event e = makeEvent(EventCode::NetDown, Severity::Warning, kNoValve, 1, 0, nullptr);
  char out[200];
  const std::string pre =
      "<" + std::to_string(128 + syslogSeverity(Severity::Warning)) + ">1 - host vdmot - ";
  size_t n = formatSyslog(e, "msg text", "host", out, sizeof out);
  const std::string want = pre + eventCodeName(EventCode::NetDown) + " - msg text";
  CHECK(std::string(out) == want);
  CHECK(n == want.size());
  e.epoch = 1790136000;  // 2026-09-23T04:00:00Z
  e.severity = Severity::Debug;
  formatSyslog(e, "m", "", out, sizeof out);
  CHECK(std::string(out) == "<" + std::to_string(128 + syslogSeverity(Severity::Debug)) +
                                ">1 2026-09-23T04:00:00Z - vdmot - " +
                                eventCodeName(EventCode::NetDown) + " - m");
  formatSyslog(e, nullptr, nullptr, out, sizeof out);
  CHECK(std::string(out) == "<" + std::to_string(128 + syslogSeverity(Severity::Debug)) +
                                ">1 2026-09-23T04:00:00Z - vdmot - " +
                                eventCodeName(EventCode::NetDown) + " - ");
  CHECK(formatSyslog(e, "m", "h", out, 5) == 4);
  CHECK(std::string(out) == "<135");
  out[0] = 'q';
  CHECK(formatSyslog(e, "m", "h", out, 0) == 0);
  CHECK(out[0] == 'q');
  CHECK(formatSyslog(e, "m", "h", nullptr, 10) == 0);
}
