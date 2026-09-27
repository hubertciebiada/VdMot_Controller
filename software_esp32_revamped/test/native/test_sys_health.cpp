// System health: stack threshold, ResourceMonitor, HeapGuard, httpStatusOk, reset reasons.
#include <string.h>

#include <string>

#include "doctest.h"
#include "vdm/sys_health.h"

using namespace vdm;

namespace {

// One sample per second from `fromMs` to `toMs` (both included) with the same
// figures; the uptime of the first sample that asks for the restart, 0 if none.
uint32_t firstRestart(HeapGuard& g, uint32_t freeHeap, bool blocked, uint32_t fromMs,
                      uint32_t toMs) {
  for (uint32_t t = fromMs; t <= toMs; t += 1000) {
    if (g.onSample(freeHeap, blocked, t)) return t;
  }
  return 0;
}

}  // namespace

TEST_CASE("stackLowThreshold") {
  CHECK(stackLowThreshold(0) == 512);
  CHECK(stackLowThreshold(4096) == 512);
  CHECK(stackLowThreshold(4103) == 512);
  CHECK(stackLowThreshold(4104) == 513);
  CHECK(stackLowThreshold(6144) == 768);
  CHECK(stackLowThreshold(16384) == 2048);
}

TEST_CASE("ResourceMonitor onHeap: LowHeap below 30 KiB, again after an hour") {
  ResourceMonitor m;
  CHECK(m.minLargestBlock() == 0);
  Event ev[2];
  CHECK(m.onHeap(30720, 20000, 90000, 0, ev, 2) == 0);
  CHECK(m.minLargestBlock() == 90000);
  REQUIRE(m.onHeap(30719, 20000, 90000, 10000, ev, 2) == 1);
  CHECK(ev[0].code == EventCode::LowHeap);
  CHECK(ev[0].severity == eventDefaultSeverity(EventCode::LowHeap));
  CHECK(ev[0].valve == kNoValve);
  CHECK(ev[0].arg1 == 30719);
  CHECK(ev[0].arg2 == 20000);
  CHECK(m.onHeap(100, 50, 90000, 10000 + 3599999, ev, 2) == 0);
  REQUIRE(m.onHeap(100, 50, 90000, 10000 + 3600000, ev, 2) == 1);
  CHECK(ev[0].arg1 == 100);
  CHECK(ev[0].arg2 == 50);
  CHECK(m.onHeap(100, 50, 90000, 10000 + 3600001, ev, 2) == 0);
}

TEST_CASE("ResourceMonitor onHeap: HeapFragmented below 8 KiB largest block") {
  ResourceMonitor m;
  Event ev[2];
  CHECK(m.onHeap(100000, 90000, 8192, 0, ev, 2) == 0);
  REQUIRE(m.onHeap(100000, 90000, 8191, 1000, ev, 2) == 1);
  CHECK(ev[0].code == EventCode::HeapFragmented);
  CHECK(ev[0].severity == eventDefaultSeverity(EventCode::HeapFragmented));
  CHECK(ev[0].arg1 == 8191);
  CHECK(ev[0].arg2 == 100000);
  CHECK(m.onHeap(100000, 90000, 100, 1000 + 3599999, ev, 2) == 0);
  CHECK(m.onHeap(100000, 90000, 100, 1000 + 3600000, ev, 2) == 1);
  CHECK(m.minLargestBlock() == 100);
  CHECK(m.onHeap(100000, 90000, 5000, 5000000, ev, 2) == 0);
  CHECK(m.minLargestBlock() == 100);
}

TEST_CASE("ResourceMonitor onHeap: both at once, maxOut and null out") {
  ResourceMonitor m;
  Event ev[2];
  REQUIRE(m.onHeap(29000, 28000, 4000, 0, ev, 2) == 2);
  CHECK(ev[0].code == EventCode::LowHeap);
  CHECK(ev[1].code == EventCode::HeapFragmented);
  CHECK(ev[1].arg1 == 4000);
  CHECK(ev[1].arg2 == 29000);
  ResourceMonitor one;
  REQUIRE(one.onHeap(29000, 28000, 4000, 0, ev, 1) == 1);
  CHECK(ev[0].code == EventCode::LowHeap);
  // The fragmentation alarm was not written, so it is still due.
  REQUIRE(one.onHeap(29000, 28000, 4000, 10000, ev, 1) == 1);
  CHECK(ev[0].code == EventCode::HeapFragmented);
  ResourceMonitor none;
  CHECK(none.onHeap(29000, 28000, 4000, 0, nullptr, 2) == 0);
  CHECK(none.minLargestBlock() == 4000);
  CHECK(none.onHeap(29000, 28000, 4000, 0, ev, 0) == 0);
  CHECK(none.onHeap(29000, 28000, 4000, 0, ev, 2) == 2);
}

TEST_CASE("ResourceMonitor onStack") {
  ResourceMonitor m;
  Event ev[1];
  CHECK(m.onStack(0, "stm", 6144, 768, ev, 1) == 0);
  REQUIRE(m.onStack(0, "stm", 6144, 767, ev, 1) == 1);
  CHECK(ev[0].code == EventCode::StackLow);
  CHECK(ev[0].severity == eventDefaultSeverity(EventCode::StackLow));
  CHECK(ev[0].arg1 == 767);
  CHECK(ev[0].arg2 == 6144);
  CHECK(std::string(ev[0].text) == "stm");
  CHECK(m.onStack(0, "stm", 6144, 10, ev, 1) == 0);  // once per task per boot
  REQUIRE(m.onStack(7, "app", 8192, 100, ev, 1) == 1);
  CHECK(std::string(ev[0].text) == "app");
  CHECK(m.onStack(7, "app", 8192, 100, ev, 1) == 0);
  REQUIRE(m.onStack(1, "mqtt", 8192, 1023, ev, 1) == 1);
  CHECK(m.onStack(8, "x", 8192, 0, ev, 1) == 0);
  CHECK(m.onStack(255, "x", 8192, 0, ev, 1) == 0);
  CHECK(m.onStack(2, "x", 8192, 0, nullptr, 1) == 0);
  CHECK(m.onStack(2, "x", 8192, 0, ev, 0) == 0);
  // Not reported by the calls above: still due.
  REQUIRE(m.onStack(2, "abcdefghijklmnopqrstuvwxyz0123", 8192, 0, ev, 1) == 1);
  CHECK(std::string(ev[0].text) == "abcdefghijklmnopqrstuvw");
  CHECK(strlen(ev[0].text) == kEventTextMax);
}

TEST_CASE("HeapGuard: below 12 KiB on every sample for 60 s asks once") {
  HeapGuard g;
  CHECK(firstRestart(g, 12287, false, 600000, 659000) == 0);
  CHECK(g.onSample(12287, false, 660000));
  // Still low: the same low period never asks again.
  CHECK(firstRestart(g, 0, false, 661000, 900000) == 0);
}

TEST_CASE("HeapGuard: 12 KiB free is not low") {
  HeapGuard g;
  CHECK(firstRestart(g, 12288, false, 600000, 700000) == 0);
  CHECK(firstRestart(g, 12287, false, 701000, 800000) == 761000);
}

TEST_CASE("HeapGuard: exactly 60 s after the first low sample, whatever the sample spacing") {
  HeapGuard g;
  CHECK_FALSE(g.onSample(100, false, 700000));
  CHECK_FALSE(g.onSample(100, false, 759999));
  CHECK(g.onSample(100, false, 760000));
}

TEST_CASE("HeapGuard: one sample at or above the threshold starts the 60 s again") {
  HeapGuard g;
  CHECK(firstRestart(g, 5000, false, 600000, 659000) == 0);
  CHECK_FALSE(g.onSample(12288, false, 659500));
  CHECK(firstRestart(g, 5000, false, 660000, 719000) == 0);
  CHECK(g.onSample(5000, false, 720000));
}

TEST_CASE("HeapGuard: not armed during the first 10 minutes") {
  HeapGuard g;
  // Low from boot: nothing before 10 min, and the 60 s start there.
  CHECK(firstRestart(g, 1000, false, 0, 599000) == 0);
  CHECK_FALSE(g.onSample(1000, false, 599999));
  CHECK(firstRestart(g, 1000, false, 600000, 659000) == 0);
  CHECK(g.onSample(1000, false, 660000));
}

TEST_CASE("HeapGuard: stays armed when millis() wraps") {
  HeapGuard g;
  CHECK_FALSE(g.onSample(50000, false, 600000));
  CHECK_FALSE(g.onSample(1000, false, 0xFFFFFFFFu - 29999));  // low from 2^32 - 30 s
  CHECK_FALSE(g.onSample(1000, false, 29999));
  CHECK(g.onSample(1000, false, 30000));
}

TEST_CASE("HeapGuard: blocked by an ESP upload or an STM flash, the 60 s start again after it") {
  HeapGuard g;
  CHECK(firstRestart(g, 1000, false, 600000, 630000) == 0);
  CHECK(firstRestart(g, 1000, true, 631000, 700000) == 0);
  CHECK(firstRestart(g, 1000, false, 701000, 760000) == 0);
  CHECK(g.onSample(1000, false, 761000));
  // Blocked exactly when it would ask.
  HeapGuard h;
  CHECK(firstRestart(h, 1000, false, 600000, 659000) == 0);
  CHECK_FALSE(h.onSample(1000, true, 660000));
  CHECK(firstRestart(h, 1000, false, 661000, 800000) == 721000);
}

TEST_CASE("HeapGuard: a new low period after a recovery or a block asks again") {
  HeapGuard g;
  CHECK(firstRestart(g, 1000, false, 600000, 700000) == 660000);
  CHECK_FALSE(g.onSample(20000, false, 701000));
  CHECK(firstRestart(g, 1000, false, 702000, 800000) == 762000);
  // The restart it asked for is pending (blocked), then it did not happen.
  CHECK_FALSE(g.onSample(1000, true, 801000));
  CHECK(firstRestart(g, 1000, false, 802000, 900000) == 862000);
}

TEST_CASE("httpStatusOk") {
  auto ok = [](const char* s) { return httpStatusOk(s, strlen(s)); };
  CHECK(ok("HTTP/1.1 200 OK\r\n"));
  CHECK(httpStatusOk("HTTP/1.0 200", 12));
  CHECK(ok("HTTP/1.1 200\r"));
  CHECK(ok("HTTP/1.0 200 "));
  CHECK_FALSE(ok("HTTP/1.1 2000"));
  CHECK_FALSE(ok("HTTP/1.1 503 Service"));
  CHECK_FALSE(ok("HTTP/1.1 20"));
  CHECK_FALSE(ok("http/1.1 200"));
  CHECK_FALSE(ok("HTTP/1.2 200"));
  CHECK_FALSE(ok("HTTP/2.1 200"));
  CHECK_FALSE(ok("HTTP/1.1 201"));
  CHECK_FALSE(ok("HTTP/1.1_200"));
  CHECK_FALSE(ok("HTTP/1.1 200\n"));
  CHECK_FALSE(httpStatusOk("HTTP/1.1 200 OK", 11));
  CHECK_FALSE(httpStatusOk(nullptr, 12));
}

TEST_CASE("isAbnormalReset") {
  for (int r = -1; r < 12; ++r) {
    INFO(r);
    CHECK(isAbnormalReset(r) == (r == 4 || r == 5 || r == 6 || r == 7 || r == 9));
  }
}
