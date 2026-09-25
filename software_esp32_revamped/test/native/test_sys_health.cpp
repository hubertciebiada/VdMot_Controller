// System health: stack threshold, ResourceMonitor, httpStatusOk, reset reasons.
#include <string.h>

#include <string>

#include "doctest.h"
#include "vdm/sys_health.h"

using namespace vdm;

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
