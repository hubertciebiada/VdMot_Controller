#include <stdint.h>

#include <string>

#include "doctest.h"
#include "vdm/replies_v2.h"

using vdm::StaticBufWriter;

namespace {

vdm::ValveExtReply typical() {
  vdm::ValveExtReply r{};
  r.index = 3;
  r.status = 9;
  r.position = 0;
  r.target = 30;
  r.meanCurrent = 17;
  r.openingCount = 3567;
  r.closingCount = 3610;
  r.deadzoneCount = 43;
  r.calibRetries = 2;
  r.movements = 12;
  r.calState = vdm::kCalFlagLastFailed;
  r.earlyStops = 1;
  r.cmdRejected = 4;
  r.last.dir = 1;
  r.last.requestedCounts = 65535;
  r.last.countedCounts = 102;
  r.last.stopReason = 3;
  r.last.peakCurrent = 356;
  r.last.durationMs = 2140;
  return r;
}

}  // namespace

TEST_CASE("gvlvx: field order of the protocol table") {
  StaticBufWriter<vdm::kValveExtReplyMaxLen + 1> w;
  REQUIRE(vdm::formatValveExt(w, typical()));
  CHECK(std::string(w.c_str()) == "gvlvx 3 9 0 30 17 3567 3610 43 2 12 8 1 4 1 65535 102 3 356 2140");
}

TEST_CASE("gvlvx: worst case fits the declared maximum") {
  vdm::ValveExtReply r{};
  r.index = 255;
  r.status = 255;
  r.position = 255;
  r.target = 255;
  r.meanCurrent = 65535;
  r.openingCount = 0xFFFFFFFFu;
  r.closingCount = 0xFFFFFFFFu;
  r.deadzoneCount = INT32_MIN;
  r.calibRetries = 255;
  r.movements = 0xFFFFFFFFu;
  r.calState = 255;
  r.earlyStops = 65535;
  r.cmdRejected = 65535;
  r.last.dir = 255;
  r.last.requestedCounts = 65535;
  r.last.countedCounts = 65535;
  r.last.stopReason = 255;
  r.last.peakCurrent = 65535;
  r.last.durationMs = 0xFFFFFFFFu;
  StaticBufWriter<vdm::kValveExtReplyMaxLen + 1> w;
  REQUIRE(vdm::formatValveExt(w, r));
  CHECK(w.length() <= vdm::kValveExtReplyMaxLen);
  CHECK(std::string(w.c_str()).find("-2147483648") != std::string::npos);
}

TEST_CASE("formatters write nothing when the buffer is too small") {
  StaticBufWriter<40> w;
  w.append("x");
  CHECK_FALSE(vdm::formatValveExt(w, typical()));
  CHECK(std::string(w.c_str()) == "x");

  vdm::ProfileRecorder p;
  p.reset();
  for (uint32_t c = 0; c < 32; ++c) p.add(c * 1000, 600);
  CHECK_FALSE(vdm::formatProfile(w, 1, p));
  CHECK(std::string(w.c_str()) == "x");

  StaticBufWriter<8> tiny;
  CHECK_FALSE(vdm::formatStat(tiny, vdm::StatReply{1, 2, 3, 4, 5, 6}));
  CHECK_FALSE(vdm::formatEscalation(tiny, vdm::EscalationConfig{1, 25, 50}));
  CHECK_FALSE(vdm::formatMotorLimits(tiny));
  CHECK_FALSE(vdm::formatIndexedResult(tiny, "svmov", 11, 2));
  CHECK(std::string(tiny.c_str()).empty());
}

TEST_CASE("gprof: pairs of count and current") {
  vdm::ProfileRecorder p;
  p.reset();
  p.add(0, 0);
  p.add(120, -187);
  p.finish(4012, 402);
  StaticBufWriter<vdm::kProfileReplyMaxLen + 1> w;
  REQUIRE(vdm::formatProfile(w, 7, p));
  CHECK(std::string(w.c_str()) == "gprof 7 3 0:0 120:187 4012:402");

  vdm::ProfileRecorder empty;
  empty.reset();
  w.clear();
  REQUIRE(vdm::formatProfile(w, 0, empty));
  CHECK(std::string(w.c_str()) == "gprof 0 0");
}

TEST_CASE("gprof: 32 samples of maximum width fit") {
  vdm::ProfileRecorder p;
  p.reset();
  for (uint32_t c = 0; c < 32; ++c) p.add(65504 + c, 65535);
  REQUIRE(p.size() == 32);
  StaticBufWriter<vdm::kProfileReplyMaxLen + 1> w;
  REQUIRE(vdm::formatProfile(w, 11, p));
  CHECK(w.length() <= vdm::kProfileReplyMaxLen);
}

TEST_CASE("gstat, gcalx, gmotx, gproto") {
  StaticBufWriter<vdm::kStatReplyMaxLen + 1> w;
  REQUIRE(vdm::formatStat(w, vdm::StatReply{86400, 3, 4, 17, 2, 1}));
  CHECK(std::string(w.c_str()) == "gstat 86400 3 4 17 2 1");

  StaticBufWriter<32> c;
  REQUIRE(vdm::formatEscalation(c, vdm::EscalationConfig{1, 25, 50}));
  CHECK(std::string(c.c_str()) == "gcalx 1 25 50");

  StaticBufWriter<vdm::kMotorLimitsReplyMaxLen + 1> m;
  REQUIRE(vdm::formatMotorLimits(m));
  CHECK(std::string(m.c_str()) == "gmotx 10 40 10 40 0 100 0 60000 0 2");

  StaticBufWriter<16> g;
  REQUIRE(vdm::formatProtocolVersion(g));
  CHECK(std::string(g.c_str()) == "gproto 2");
}

TEST_CASE("ok / err replies") {
  StaticBufWriter<32> w;
  REQUIRE(vdm::formatResult(w, "scalx", true));
  CHECK(std::string(w.c_str()) == "scalx ok");
  w.clear();
  REQUIRE(vdm::formatResult(w, "smotc", false));
  CHECK(std::string(w.c_str()) == "smotc err");
  w.clear();
  REQUIRE(vdm::formatIndexedResult(w, "svmov", 4, 0));
  CHECK(std::string(w.c_str()) == "svmov 4 ok");
  w.clear();
  REQUIRE(vdm::formatIndexedResult(w, "svmov", 4, 2));
  CHECK(std::string(w.c_str()) == "svmov 4 err 2");
  w.clear();
  REQUIRE(vdm::formatIndexedResult(w, "svmov", -1, 1));
  CHECK(std::string(w.c_str()) == "svmov -1 err 1");
  w.clear();
  CHECK_FALSE(vdm::formatResult(w, nullptr, true));
  CHECK(std::string(w.c_str()).empty());
}

TEST_CASE("composeCalState: state in bits 0..1, flags above") {
  CHECK(vdm::composeCalState(false, false, false, false) == 0);
  CHECK(vdm::composeCalState(false, true, false, false) == 1);
  CHECK(vdm::composeCalState(true, false, false, false) == 2);
  CHECK(vdm::composeCalState(true, true, false, false) == 2);
  CHECK(vdm::composeCalState(false, false, true, false) == 4);
  CHECK(vdm::composeCalState(false, false, false, true) == 8);
  CHECK(vdm::composeCalState(true, true, true, true) == 14);
  CHECK((vdm::composeCalState(false, true, true, true) & vdm::kCalStateMask) == vdm::kCalStateRequested);
}

TEST_CASE("encodeValveStatus: gvlvd encoding, bit 7 while calibrating") {
  CHECK(vdm::encodeValveStatus(1, false) == 1);
  CHECK(vdm::encodeValveStatus(8, true) == 0x88);
  CHECK(vdm::encodeValveStatus(9, true) == 0x89);
  CHECK(vdm::encodeValveStatus(0, true) == 0x80);
  // same result as the gvlvd handler, which ORs the bit into the raw status
  for (unsigned s = 0; s <= 0xFF; ++s) {
    CHECK(vdm::encodeValveStatus(static_cast<uint8_t>(s), false) == s);
    CHECK(vdm::encodeValveStatus(static_cast<uint8_t>(s), true) == (s | 0x80u));
  }
}
