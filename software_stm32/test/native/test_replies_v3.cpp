#include <stdint.h>

#include <string>

#include "doctest.h"
#include "vdm/lease.h"
#include "vdm/replies_v3.h"
#include "vdm/valve_codes.h"

using vdm::StaticBufWriter;

// the replies fit the reply buffer of the dispatcher (the gprof reply is the longest)
static_assert(vdm::kValveExtV3ReplyMaxLen <= vdm::kProfileReplyMaxLen, "gvlvy fits the reply buffer");
static_assert(vdm::kStatV3ReplyMaxLen <= vdm::kProfileReplyMaxLen, "gstax fits the reply buffer");

// codes of the protocol table
static_assert(vdm::kStIdle == 1 && vdm::kStOpening == 2 && vdm::kStClosing == 3 && vdm::kStFailed == 4 &&
                  vdm::kStUnknown == 5 && vdm::kStOpenCircuit == 6 && vdm::kStFullOpen == 7 &&
                  vdm::kStPresent == 8 && vdm::kStBlocked == 9,
              "valve status codes");
static_assert(vdm::kVlvFlagFsLease == 1 && vdm::kVlvFlagFsBlocked == 2 && vdm::kVlvFlagUncalibrated == 4 &&
                  vdm::kVlvFlagNeedsRef == 8 && vdm::kVlvFlagRecal == 16 && vdm::kVlvFlagCalRestored == 32 &&
                  vdm::kVlvFlagRetry == 64 && vdm::kVlvFlagEarlyPending == 128 && vdm::kVlvFlagAssembly == 256 &&
                  vdm::kVlvFlagSvcHold == 512,
              "gvlvy flag bits");
static_assert(static_cast<uint8_t>(vdm::ValveFault::None) == 0 &&
                  static_cast<uint8_t>(vdm::ValveFault::MoveTimeout) == 1 &&
                  static_cast<uint8_t>(vdm::ValveFault::StrokeTimeout) == 2 &&
                  static_cast<uint8_t>(vdm::ValveFault::Short) == 3 &&
                  static_cast<uint8_t>(vdm::ValveFault::StrokesTooShort) == 4 &&
                  static_cast<uint8_t>(vdm::ValveFault::InrushTrip) == 5,
              "gvlvy fault codes");
static_assert(vdm::kSysFlagProtectSuspended == 1, "gstax sysFlags");

namespace {

// valve 3 blocked at its failsafe position 50 % (the example of the protocol table)
vdm::ValveExtV3Reply blockedValve() {
  vdm::ValveExtV3Reply r{};
  r.base.index = 3;
  r.base.status = vdm::kStBlocked;
  r.base.position = 50;
  r.base.target = 30;
  r.base.meanCurrent = 17;
  r.base.openingCount = 3567;
  r.base.closingCount = 3610;
  r.base.deadzoneCount = 43;
  r.base.calibRetries = 2;
  r.base.movements = 0;
  r.base.calState = vdm::kCalFlagLastFailed;
  r.base.earlyStops = 1;
  r.base.cmdRejected = 4;
  r.base.last.dir = 0;
  r.base.last.requestedCounts = 1750;
  r.base.last.countedCounts = 1750;
  r.base.last.stopReason = 1;
  r.base.last.peakCurrent = 262;
  r.base.last.durationMs = 6120;
  r.flags = vdm::kVlvFlagFsBlocked | vdm::kVlvFlagRetry;
  r.fault = static_cast<uint8_t>(vdm::ValveFault::StrokesTooShort);
  r.failsafePct = 50;
  r.drive = 50;
  r.retryS = 3540;
  r.retries = 0;
  return r;
}

vdm::StatV3Reply leaseRunning() {
  vdm::StatV3Reply r{};
  r.base = vdm::StatReply{86400, 3, 2, 0, 2, 0};
  r.lease = static_cast<uint8_t>(vdm::LeaseState::Running);
  r.leaseRemainS = 3540;
  r.leaseClient = 1;
  r.leaseTimeoutMin = 60;
  r.eepWrites = 12;
  r.tempAgeS = 2;
  r.owScanAgeS = 3600;
  return r;
}

// every value different, so any change of the field order shows
vdm::StatV3Reply distinctStat() {
  vdm::StatV3Reply r{};
  r.base = vdm::StatReply{101, 102, 3, 104, 105, 1};
  r.lease = 2;
  r.leaseRemainS = 108;
  r.leaseClient = 1;
  r.leaseTimeoutMin = 1440;
  r.failsafeMask = 4095;
  r.safeMode = 1;
  r.wdgResets = 13;
  r.uartOre = 114;
  r.uartFe = 115;
  r.uartNe = 116;
  r.rxDropped = 117;
  r.cfgFlags = 118;
  r.cfgEvents = 119;
  r.eepWrites = 120;
  r.tempAgeS = 121;
  r.owScanAgeS = 122;
  r.sysFlags = 123;
  return r;
}

std::string afterCommand(const char* line) { return std::string(line).substr(5); }

}  // namespace

TEST_CASE("gvlvy: the blocked valve of the protocol table") {
  StaticBufWriter<vdm::kValveExtV3ReplyMaxLen + 1> w;
  REQUIRE(vdm::formatValveExtV3(w, blockedValve()));
  CHECK(std::string(w.c_str()) == "gvlvy 3 9 50 30 17 3567 3610 43 2 0 8 1 4 0 1750 1750 1 262 6120 66 4 50 50 3540 0");
}

TEST_CASE("gvlvy: values 1..19 are those of gvlvx") {
  vdm::ValveExtV3Reply r = blockedValve();
  r.flags = 0x0203;
  r.fault = 5;
  r.failsafePct = 255;
  r.drive = 30;
  r.retryS = 86400;
  r.retries = 255;
  StaticBufWriter<vdm::kValveExtV3ReplyMaxLen + 1> v3;
  StaticBufWriter<vdm::kValveExtReplyMaxLen + 1> v2;
  REQUIRE(vdm::formatValveExtV3(v3, r));
  REQUIRE(vdm::formatValveExt(v2, r.base));
  CHECK(afterCommand(v3.c_str()) == afterCommand(v2.c_str()) + " 515 5 255 30 86400 255");
}

TEST_CASE("gvlvy: worst case fits the declared maximum") {
  vdm::ValveExtV3Reply r{};
  r.base.index = 255;
  r.base.status = 255;
  r.base.position = 255;
  r.base.target = 255;
  r.base.meanCurrent = 65535;
  r.base.openingCount = UINT32_MAX;
  r.base.closingCount = UINT32_MAX;
  r.base.deadzoneCount = INT32_MIN;
  r.base.calibRetries = 255;
  r.base.movements = UINT32_MAX;
  r.base.calState = 255;
  r.base.earlyStops = 65535;
  r.base.cmdRejected = 65535;
  r.base.last.dir = 255;
  r.base.last.requestedCounts = 65535;
  r.base.last.countedCounts = 65535;
  r.base.last.stopReason = 255;
  r.base.last.peakCurrent = 65535;
  r.base.last.durationMs = UINT32_MAX;
  r.flags = 65535;
  r.fault = 255;
  r.failsafePct = 255;
  r.drive = 255;
  r.retryS = UINT32_MAX;
  r.retries = 255;
  StaticBufWriter<vdm::kValveExtV3ReplyMaxLen + 1> w;
  REQUIRE(vdm::formatValveExtV3(w, r));
  CHECK(w.length() <= vdm::kValveExtV3ReplyMaxLen);
}

TEST_CASE("gstax: running lease (the example of the protocol table)") {
  StaticBufWriter<vdm::kStatV3ReplyMaxLen + 1> w;
  REQUIRE(vdm::formatStatV3(w, leaseRunning()));
  CHECK(std::string(w.c_str()) == "gstax 86400 3 2 0 2 0 1 3540 1 60 0 0 0 0 0 0 0 0 0 12 2 3600 0");
}

TEST_CASE("gstax: 23 values in the order of the protocol table") {
  StaticBufWriter<vdm::kStatV3ReplyMaxLen + 1> w;
  REQUIRE(vdm::formatStatV3(w, distinctStat()));
  CHECK(std::string(w.c_str()) ==
        "gstax 101 102 3 104 105 1 2 108 1 1440 4095 1 13 114 115 116 117 118 119 120 121 122 123");
}

TEST_CASE("gstax: values 1..6 are those of gstat") {
  const vdm::StatV3Reply r = distinctStat();
  StaticBufWriter<vdm::kStatV3ReplyMaxLen + 1> v3;
  StaticBufWriter<vdm::kStatReplyMaxLen + 1> v2;
  REQUIRE(vdm::formatStatV3(v3, r));
  REQUIRE(vdm::formatStat(v2, r.base));
  CHECK(afterCommand(v3.c_str()).find(afterCommand(v2.c_str()) + " ") == 0);
}

TEST_CASE("gstax: worst case fits the declared maximum") {
  vdm::StatV3Reply r{};
  r.base = vdm::StatReply{UINT32_MAX, UINT32_MAX, 255, UINT32_MAX, UINT32_MAX, 255};
  r.lease = 255;
  r.leaseRemainS = UINT32_MAX;
  r.leaseClient = 255;
  r.leaseTimeoutMin = 65535;
  r.failsafeMask = 65535;
  r.safeMode = 255;
  r.wdgResets = 255;
  r.uartOre = UINT32_MAX;
  r.uartFe = UINT32_MAX;
  r.uartNe = UINT32_MAX;
  r.rxDropped = UINT32_MAX;
  r.cfgFlags = 255;
  r.cfgEvents = UINT32_MAX;
  r.eepWrites = UINT32_MAX;
  r.tempAgeS = UINT32_MAX;
  r.owScanAgeS = UINT32_MAX;
  r.sysFlags = 255;
  StaticBufWriter<vdm::kStatV3ReplyMaxLen + 1> w;
  REQUIRE(vdm::formatStatV3(w, r));
  CHECK(w.length() <= vdm::kStatV3ReplyMaxLen);
}

TEST_CASE("slhbt: lease state and remaining seconds") {
  StaticBufWriter<32> w;
  REQUIRE(vdm::formatHeartbeat(w, 1, 3540));
  CHECK(std::string(w.c_str()) == "slhbt 1 3540");
  w.clear();
  REQUIRE(vdm::formatHeartbeat(w, 0, 0));
  CHECK(std::string(w.c_str()) == "slhbt 0 0");
  w.clear();
  REQUIRE(vdm::formatHeartbeat(w, 2, 0));
  CHECK(std::string(w.c_str()) == "slhbt 2 0");
}

TEST_CASE("glcfg: timeout and the 12 failsafe positions") {
  StaticBufWriter<80> w;
  const uint8_t fs[12] = {50, 50, 50, 50, 50, 50, 50, 50, 50, 50, 50, 255};
  REQUIRE(vdm::formatLeaseConfig(w, 60, fs));
  CHECK(std::string(w.c_str()) == "glcfg 60 50 50 50 50 50 50 50 50 50 50 50 255");
  w.clear();
  const uint8_t distinct[12] = {0, 1, 2, 3, 40, 50, 60, 70, 80, 99, 100, 255};
  REQUIRE(vdm::formatLeaseConfig(w, 0, distinct));
  CHECK(std::string(w.c_str()) == "glcfg 0 0 1 2 3 40 50 60 70 80 99 100 255");
  w.clear();
  REQUIRE(vdm::formatLeaseConfig(w, 1440, fs));
  CHECK(std::string(w.c_str()).find("glcfg 1440 50 ") == 0);
}

TEST_CASE("gtlnt: stored learn time") {
  StaticBufWriter<32> w;
  REQUIRE(vdm::formatLearnTime(w, 604800));
  CHECK(std::string(w.c_str()) == "gtlnt 604800");
  w.clear();
  REQUIRE(vdm::formatLearnTime(w, 0));
  CHECK(std::string(w.c_str()) == "gtlnt 0");
  w.clear();
  REQUIRE(vdm::formatLearnTime(w, UINT32_MAX));
  CHECK(std::string(w.c_str()) == "gtlnt 4294967295");
}

TEST_CASE("protocol 3 ok and err replies") {
  StaticBufWriter<32> w;
  const auto reply = [&w](bool formatted) {
    std::string s = formatted ? std::string(w.c_str()) : std::string("<none>");
    w.clear();
    return s;
  };
  CHECK(reply(vdm::formatResult(w, "slhbt", false)) == "slhbt err");
  CHECK(reply(vdm::formatResult(w, "slcfg", true)) == "slcfg ok");
  CHECK(reply(vdm::formatResult(w, "slcfg", false)) == "slcfg err");
  CHECK(reply(vdm::formatIndexedResult(w, "sfspo", 3, 0)) == "sfspo 3 ok");
  CHECK(reply(vdm::formatIndexedResult(w, "sfspo", 255, 0)) == "sfspo 255 ok");
  CHECK(reply(vdm::formatIndexedResult(w, "sfspo", 3, 1)) == "sfspo 3 err 1");
  CHECK(reply(vdm::formatIndexedResult(w, "sfspo", -1, 1)) == "sfspo -1 err 1");
  CHECK(reply(vdm::formatIndexedResult(w, "sstop", 2, 0)) == "sstop 2 ok");
  CHECK(reply(vdm::formatIndexedResult(w, "sstop", 255, 0)) == "sstop 255 ok");
  CHECK(reply(vdm::formatIndexedResult(w, "sstop", -1, 1)) == "sstop -1 err 1");
  CHECK(reply(vdm::formatResult(w, "ssafe", true)) == "ssafe ok");
  CHECK(reply(vdm::formatResult(w, "ssafe", false)) == "ssafe err");
}

TEST_CASE("protocol 3 formatters write nothing when the buffer is too small") {
  const uint8_t fs[12] = {50, 50, 50, 50, 50, 50, 50, 50, 50, 50, 50, 255};

  // room for the values of gvlvx, not for the rest of gvlvy
  StaticBufWriter<70> w;
  w.append("x");
  StaticBufWriter<70> probe;
  probe.append("x");
  REQUIRE(vdm::formatValveExt(probe, blockedValve().base));
  CHECK_FALSE(vdm::formatValveExtV3(w, blockedValve()));
  CHECK(std::string(w.c_str()) == "x");
  // room for the values of gstat, not for the rest of gstax
  probe.clear();
  probe.append("x");
  REQUIRE(vdm::formatStat(probe, distinctStat().base));
  CHECK_FALSE(vdm::formatStatV3(w, distinctStat()));
  CHECK(std::string(w.c_str()) == "x");

  StaticBufWriter<12> tiny;
  tiny.append("x");
  CHECK_FALSE(vdm::formatHeartbeat(tiny, 1, 3540000));
  CHECK_FALSE(vdm::formatLeaseConfig(tiny, 60, fs));
  CHECK_FALSE(vdm::formatLearnTime(tiny, 604800));
  CHECK(std::string(tiny.c_str()) == "x");

  // one character short of the whole reply
  StaticBufWriter<sizeof("glcfg 60 50 50 50 50 50 50 50 50 50 50 50 255") - 1> almost;
  CHECK_FALSE(vdm::formatLeaseConfig(almost, 60, fs));
  CHECK(std::string(almost.c_str()).empty());
  StaticBufWriter<sizeof("gtlnt 604800") - 1> almostTime;
  CHECK_FALSE(vdm::formatLearnTime(almostTime, 604800));
  CHECK(std::string(almostTime.c_str()).empty());
  StaticBufWriter<sizeof("slhbt 1 3540") - 1> almostBeat;
  CHECK_FALSE(vdm::formatHeartbeat(almostBeat, 1, 3540));
  CHECK(std::string(almostBeat.c_str()).empty());
}
