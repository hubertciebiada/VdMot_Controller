// stm_codec: command table, request builders (golden lines), reply parser
// (every v1 and v2 reply, malformed input, fuzz), reply matching, helpers.
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#include <initializer_list>
#include <string>
#include <utility>

#include "doctest.h"
#include "support/stm_golden.h"
#include "vdm/line_assembler.h"
#include "vdm/stm_codec.h"

using namespace vdm;

namespace {

const char* const kId1 = "28-84-37-94-97-ff-03-23";  // valid CRC
const char* const kId2 = "28-aa-bb-cc-dd-ee-01-67";  // valid CRC
const char* const kId3 = "26-11-22-33-44-55-66-29";  // valid CRC (DS2438 family)

OneWireId id(const char* s) {
  OneWireId v;
  REQUIRE(parseOneWireId(s, strlen(s), v));
  return v;
}

ParseStatus parse(const std::string& s, Reply& r) { return parseReply(s.data(), s.size(), r); }

std::string text(const RequestLine& r) { return std::string(r.text, r.len); }

// Fills a Reply with non-default garbage so resets are observable.
void dirty(Reply& r) {
  r.cmd = Cmd::Gstat;
  r.gvlonError = true;
  r.valveData.valve = 7;
  r.valveData.moves = 99;
  r.status.uptimeS = 1234;
  r.proto = 9;
  r.version.valid = true;
  r.build = 77;
  r.profile.count = 3;
}

bool isEmptyReply(const Reply& r) {
  return r.cmd == Cmd::None && !r.gvlonError && r.valveData.valve == 0 && r.valveData.moves == 0 &&
         r.status.uptimeS == 0 && r.proto == 0 && !r.version.valid && r.build == 0 &&
         r.profile.count == 0;
}

std::string idList(const char* one, size_t n, bool trailingComma) {
  std::string s;
  for (size_t i = 0; i < n; ++i) {
    if (i) s += ',';
    s += one;
  }
  if (trailingComma) s += ',';
  return s;
}

}  // namespace

// ================================================================ commands

TEST_CASE("codec: command names round trip and are exact") {
  CHECK(kCmdCount == 41);
  CHECK(std::string(cmdName(Cmd::None)) == "");
  CHECK(std::string(cmdName(static_cast<Cmd>(kCmdCount))) == "");
  CHECK(std::string(cmdName(static_cast<Cmd>(200))) == "");
  const char* expected[] = {"",      "stgtp", "gtgtp", "gvlvd", "gvlst", "gonec",  "goned", "gvlon",
                            "gowvc", "gowvd", "stons", "stvls", "masns", "staop",  "staln", "stdet",
                            "stlnm", "gtlnm", "smotc", "gmotc", "gvers", "ghwin",  "eepst", "reset",
                            "gproto", "gvlvx", "gprof", "svmov", "scalx", "gcalx", "gstat", "gvlvy",
                            "gstax", "slhbt", "slcfg", "sfspo", "glcfg", "sstop",  "gtlnt", "ssafe",
                            "stlnt"};
  static_assert(sizeof expected / sizeof *expected == kCmdCount, "one name per command");
  for (uint8_t i = 0; i < kCmdCount; ++i) {
    const Cmd c = static_cast<Cmd>(i);
    CHECK(std::string(cmdName(c)) == expected[i]);
    if (i > 0) CHECK(cmdFromName(expected[i], strlen(expected[i])) == c);
  }
  CHECK(cmdFromName("", 0) == Cmd::None);
  CHECK(cmdFromName(nullptr, 5) == Cmd::None);
  CHECK(cmdFromName("gvlv", 4) == Cmd::None);
  CHECK(cmdFromName("gvlvdx", 6) == Cmd::None);
  CHECK(cmdFromName("GVLVD", 5) == Cmd::None);
  CHECK(cmdFromName("gprot", 5) == Cmd::None);
  CHECK(cmdFromName("gvlvd 1", 5) == Cmd::Gvlvd);  // exactly len bytes
  CHECK(cmdFromName("stsnx", 5) == Cmd::None);
}

TEST_CASE("codec: v2 and idempotent classification") {
  for (uint8_t i = 0; i < kCmdCount; ++i) {
    const Cmd c = static_cast<Cmd>(i);
    CAPTURE(cmdName(c));
    uint8_t proto = 1;
    if (c == Cmd::None) proto = 0;
    if (i >= static_cast<uint8_t>(Cmd::Gproto)) proto = 2;
    if (i >= static_cast<uint8_t>(Cmd::Gvlvy)) proto = 3;
    if (c == Cmd::Stlnt) proto = 1;
    CHECK(cmdMinProtocol(c) == proto);
    CHECK(cmdIsV2(c) == (proto >= 2));
  }
  CHECK(cmdMinProtocol(Cmd::Stgtp) == 1);
  CHECK(cmdMinProtocol(Cmd::Reset) == 1);
  CHECK(cmdMinProtocol(Cmd::Gproto) == 2);
  CHECK(cmdMinProtocol(Cmd::Gstat) == 2);
  CHECK(cmdMinProtocol(Cmd::Gvlvy) == 3);
  CHECK(cmdMinProtocol(Cmd::Ssafe) == 3);
  CHECK(cmdMinProtocol(Cmd::Stlnt) == 1);
  CHECK_FALSE(cmdIsV2(Cmd::Stlnt));
  CHECK(cmdMinProtocol(static_cast<Cmd>(kCmdCount)) == 0);
  CHECK(cmdMinProtocol(static_cast<Cmd>(255)) == 0);
  CHECK_FALSE(cmdIsV2(static_cast<Cmd>(kCmdCount)));
  CHECK_FALSE(cmdIsV2(static_cast<Cmd>(255)));

  const Cmd notIdempotent[] = {Cmd::None,  Cmd::Stons, Cmd::Masns, Cmd::Staop,
                               Cmd::Staln, Cmd::Stdet, Cmd::Reset, Cmd::Svmov};
  for (uint8_t i = 0; i < kCmdCount; ++i) {
    const Cmd c = static_cast<Cmd>(i);
    bool expect = true;
    for (Cmd n : notIdempotent) expect = expect && n != c;
    CAPTURE(cmdName(c));
    CHECK(cmdIsIdempotent(c) == expect);
  }
  CHECK_FALSE(cmdIsIdempotent(static_cast<Cmd>(kCmdCount)));
}

// ================================================================ validators

TEST_CASE("codec: motorCharsValid boundaries") {
  MotorChars m;
  CHECK(motorCharsValid(m));
  m.lowFactor = 9;
  CHECK_FALSE(motorCharsValid(m));
  m.lowFactor = 10;
  CHECK(motorCharsValid(m));
  m.lowFactor = 40;
  CHECK(motorCharsValid(m));
  m.lowFactor = 41;
  CHECK_FALSE(motorCharsValid(m));
  m = MotorChars{};
  m.highFactor = 9;
  CHECK_FALSE(motorCharsValid(m));
  m.highFactor = 10;
  CHECK(motorCharsValid(m));
  m.highFactor = 40;
  CHECK(motorCharsValid(m));
  m.highFactor = 41;
  CHECK_FALSE(motorCharsValid(m));
  m = MotorChars{};
  m.startOnPower = 0;
  CHECK(motorCharsValid(m));
  m.startOnPower = 100;
  CHECK(motorCharsValid(m));
  m.startOnPower = 101;
  CHECK_FALSE(motorCharsValid(m));
  m = MotorChars{};
  m.minCounts = 0;
  CHECK(motorCharsValid(m));
  m.minCounts = 60000;
  CHECK(motorCharsValid(m));
  m.minCounts = 60001;
  CHECK_FALSE(motorCharsValid(m));
  m = MotorChars{};
  m.maxCalibRetries = 0;
  CHECK(motorCharsValid(m));
  m.maxCalibRetries = 2;
  CHECK(motorCharsValid(m));
  m.maxCalibRetries = 3;
  CHECK_FALSE(motorCharsValid(m));
}

TEST_CASE("codec: breakawayValid and learnMovementsValid boundaries") {
  Breakaway b;
  CHECK(breakawayValid(b));
  b.enable = true;
  b.stepPct = 100;
  CHECK(breakawayValid(b));
  b.stepPct = 101;
  CHECK_FALSE(breakawayValid(b));
  b.stepPct = 0;
  b.maxmA = 19;
  CHECK_FALSE(breakawayValid(b));
  b.maxmA = 20;
  CHECK(breakawayValid(b));
  b.maxmA = 60;
  CHECK(breakawayValid(b));
  b.maxmA = 61;
  CHECK_FALSE(breakawayValid(b));

  CHECK(learnMovementsValid(0));
  CHECK_FALSE(learnMovementsValid(1));
  CHECK_FALSE(learnMovementsValid(49));
  CHECK(learnMovementsValid(50));
  CHECK(learnMovementsValid(2000));
  CHECK(learnMovementsValid(65534));
  CHECK_FALSE(learnMovementsValid(65535));
  CHECK_FALSE(learnMovementsValid(65536));
  CHECK_FALSE(learnMovementsValid(0xFFFFFFFFu));
}

// ================================================================ builders

namespace {

void checkLine(bool ok, const RequestLine& r, const char* expected, Cmd cmd, uint8_t valve,
               uint16_t arg) {
  CAPTURE(expected);
  REQUIRE(ok);
  CHECK(text(r) == expected);
  CHECK(r.len == strlen(expected));
  CHECK(r.text[r.len] == '\0');
  CHECK(r.cmd == cmd);
  CHECK(r.valve == valve);
  CHECK(r.arg == arg);
}

void checkRejected(bool ok, const RequestLine& r) {
  CHECK_FALSE(ok);
  CHECK(r.len == 0);
  CHECK(r.cmd == Cmd::None);
  CHECK(r.valve == kNoValve);
  CHECK(r.arg == 0);
  CHECK(r.text[0] == '\0');
  CHECK_FALSE(r.probe);
  CHECK(isZero(r.expect));
}

RequestLine garbage() {
  RequestLine r;
  memset(r.text, 'x', sizeof r.text - 1);
  r.len = 20;
  r.cmd = Cmd::Gstat;
  r.valve = 3;
  r.arg = 9;
  r.probe = true;
  r.expect.b[0] = 0x28;
  return r;
}

}  // namespace

TEST_CASE("codec: builders without arguments (golden lines)") {
  RequestLine r;
  checkLine(buildValveStates(r), r, "gvlst \r\n", Cmd::Gvlst, kNoValve, 0);
  checkLine(buildTempCount(r), r, "gonec \r\n", Cmd::Gonec, kNoValve, 0);
  checkLine(buildTempList(r), r, "gonec 255 \r\n", Cmd::Gonec, kNoValve, 255);
  checkLine(buildVoltCount(r), r, "gowvc \r\n", Cmd::Gowvc, kNoValve, 0);
  checkLine(buildVoltList(r), r, "gowvc 255 \r\n", Cmd::Gowvc, kNoValve, 255);
  checkLine(buildScanOneWire(r), r, "stons \r\n", Cmd::Stons, kNoValve, 0);
  checkLine(buildMatchSensors(r), r, "masns \r\n", Cmd::Masns, kNoValve, 0);
  checkLine(buildDetect(r), r, "stdet 255 \r\n", Cmd::Stdet, kAllValves, 0);
  checkLine(buildGetLearnMovements(r), r, "gtlnm \r\n", Cmd::Gtlnm, kNoValve, 0);
  checkLine(buildGetMotorChars(r), r, "gmotc \r\n", Cmd::Gmotc, kNoValve, 0);
  checkLine(buildGetVersion(r), r, "gvers \r\n", Cmd::Gvers, kNoValve, 0);
  checkLine(buildGetHwId(r), r, "ghwin \r\n", Cmd::Ghwin, kNoValve, 0);
  checkLine(buildEepromState(r), r, "eepst \r\n", Cmd::Eepst, kNoValve, 0);
  checkLine(buildSoftReset(r), r, "reset \r\n", Cmd::Reset, kNoValve, 0);
  checkLine(buildGetProto(r), r, "gproto \r\n", Cmd::Gproto, kNoValve, 0);
  checkLine(buildGetBreakaway(r), r, "gcalx \r\n", Cmd::Gcalx, kNoValve, 0);
  checkLine(buildGetStatus(r), r, "gstat \r\n", Cmd::Gstat, kNoValve, 0);
  checkLine(buildGetStatusV3(r), r, "gstax \r\n", Cmd::Gstax, kNoValve, 0);
  checkLine(buildGetLeaseConfig(r), r, "glcfg \r\n", Cmd::Glcfg, kNoValve, 0);
  checkLine(buildGetLearnTime(r), r, "gtlnt \r\n", Cmd::Gtlnt, kNoValve, 0);
  checkLine(buildLeaveSafeMode(r), r, "ssafe 0 \r\n", Cmd::Ssafe, kNoValve, 0);
}

TEST_CASE("codec: buildHeartbeat and buildSetLeaseTimeout") {
  RequestLine r = garbage();
  checkLine(buildHeartbeat(true, r), r, "slhbt 1 \r\n", Cmd::Slhbt, kNoValve, 1);
  checkLine(buildHeartbeat(false, r), r, "slhbt 0 \r\n", Cmd::Slhbt, kNoValve, 0);
  checkLine(buildSetLeaseTimeout(60, r), r, "slcfg 60 \r\n", Cmd::Slcfg, kNoValve, 60);
  checkLine(buildSetLeaseTimeout(0, r), r, "slcfg 0 \r\n", Cmd::Slcfg, kNoValve, 0);
  checkLine(buildSetLeaseTimeout(5, r), r, "slcfg 5 \r\n", Cmd::Slcfg, kNoValve, 5);
  checkLine(buildSetLeaseTimeout(1440, r), r, "slcfg 1440 \r\n", Cmd::Slcfg, kNoValve, 1440);
  for (uint32_t bad : {1u, 4u, 1441u, 65541u, 0xFFFFFFFFu}) {
    CAPTURE(bad);
    r = garbage();
    checkRejected(buildSetLeaseTimeout(bad, r), r);
  }
}

TEST_CASE("codec: buildSetFailsafe") {
  RequestLine r = garbage();
  checkLine(buildSetFailsafe(2, 50, r), r, "sfspo 2 50 \r\n", Cmd::Sfspo, 2, 50);
  checkLine(buildSetFailsafe(0, 0, r), r, "sfspo 0 0 \r\n", Cmd::Sfspo, 0, 0);
  checkLine(buildSetFailsafe(11, 100, r), r, "sfspo 11 100 \r\n", Cmd::Sfspo, 11, 100);
  checkLine(buildSetFailsafe(7, 255, r), r, "sfspo 7 255 \r\n", Cmd::Sfspo, 7, 255);
  checkLine(buildSetFailsafe(kAllValves, 40, r), r, "sfspo 255 40 \r\n", Cmd::Sfspo, kAllValves,
            40);
  const uint8_t badValves[] = {12, 254, kNoValve};
  for (uint8_t v : badValves) {
    CAPTURE(v);
    r = garbage();
    checkRejected(buildSetFailsafe(v, 50, r), r);
  }
  const uint8_t badPct[] = {101, 200, 254};
  for (uint8_t p : badPct) {
    CAPTURE(p);
    r = garbage();
    checkRejected(buildSetFailsafe(3, p, r), r);
  }
}

TEST_CASE("codec: buildSetLearnTime") {
  RequestLine r = garbage();
  checkLine(buildSetLearnTime(0, r), r, "stlnt 0 \r\n", Cmd::Stlnt, kNoValve, 0);
  checkLine(buildSetLearnTime(604800, r), r, "stlnt 604800 \r\n", Cmd::Stlnt, kNoValve, 0);
  checkLine(buildSetLearnTime(0xFFFFFFFFu, r), r, "stlnt 4294967295 \r\n", Cmd::Stlnt, kNoValve,
            0);
}

TEST_CASE("codec: buildSetTarget") {
  RequestLine r = garbage();
  checkLine(buildSetTarget(3, 50, r), r, "stgtp 3 50 \r\n", Cmd::Stgtp, 3, 50);
  checkLine(buildSetTarget(0, 0, r), r, "stgtp 0 0 \r\n", Cmd::Stgtp, 0, 0);
  checkLine(buildSetTarget(11, 100, r), r, "stgtp 11 100 \r\n", Cmd::Stgtp, 11, 100);
  r = garbage();
  checkRejected(buildSetTarget(12, 50, r), r);
  r = garbage();
  checkRejected(buildSetTarget(0, 101, r), r);
  r = garbage();
  checkRejected(buildSetTarget(255, 0, r), r);
}

TEST_CASE("codec: single-valve builders") {
  struct Case {
    bool (*fn)(uint8_t, RequestLine&);
    const char* name;
    Cmd cmd;
  } cases[] = {
      {buildGetTarget, "gtgtp", Cmd::Gtgtp},
      {buildValveData, "gvlvd", Cmd::Gvlvd},
      {buildValveEx, "gvlvx", Cmd::Gvlvx},
      {buildProfile, "gprof", Cmd::Gprof},
      {buildValveExV3, "gvlvy", Cmd::Gvlvy},
  };
  for (const Case& c : cases) {
    CAPTURE(c.name);
    RequestLine r;
    for (uint8_t v = 0; v < kValveCount; ++v) {
      const std::string expect = std::string(c.name) + " " + std::to_string(v) + " \r\n";
      checkLine(c.fn(v, r), r, expect.c_str(), c.cmd, v, 0);
    }
    r = garbage();
    checkRejected(c.fn(12, r), r);
    r = garbage();
    checkRejected(c.fn(kAllValves, r), r);
    r = garbage();
    checkRejected(c.fn(kNoValve, r), r);
  }
}

TEST_CASE("codec: valve-or-all builders") {
  struct Case {
    bool (*fn)(uint8_t, RequestLine&);
    const char* name;
    Cmd cmd;
  } cases[] = {
      {buildValveSensors, "gvlon", Cmd::Gvlon},
      {buildAssembly, "staop", Cmd::Staop},
      {buildCalibrate, "staln", Cmd::Staln},
      {buildStop, "sstop", Cmd::Sstop},
  };
  for (const Case& c : cases) {
    CAPTURE(c.name);
    RequestLine r;
    checkLine(c.fn(0, r), r, (std::string(c.name) + " 0 \r\n").c_str(), c.cmd, 0, 0);
    checkLine(c.fn(11, r), r, (std::string(c.name) + " 11 \r\n").c_str(), c.cmd, 11, 0);
    checkLine(c.fn(kAllValves, r), r, (std::string(c.name) + " 255 \r\n").c_str(), c.cmd,
              kAllValves, 0);
    r = garbage();
    checkRejected(c.fn(12, r), r);
    r = garbage();
    checkRejected(c.fn(254, r), r);
  }
}

TEST_CASE("codec: bus index builders") {
  RequestLine r;
  checkLine(buildTempData(0, r), r, "goned 0 \r\n", Cmd::Goned, kNoValve, 0);
  checkLine(buildTempData(33, r), r, "goned 33 \r\n", Cmd::Goned, kNoValve, 33);
  r = garbage();
  checkRejected(buildTempData(34, r), r);
  checkLine(buildVoltData(0, r), r, "gowvd 0 \r\n", Cmd::Gowvd, kNoValve, 0);
  checkLine(buildVoltData(7, r), r, "gowvd 7 \r\n", Cmd::Gowvd, kNoValve, 7);
  r = garbage();
  checkRejected(buildVoltData(8, r), r);
}

TEST_CASE("codec: buildSetValveSensors") {
  RequestLine r;
  const OneWireId zero;
  checkLine(buildSetValveSensors(11, id(kId1), id(kId2), r), r,
            "stvls 11 28-84-37-94-97-ff-03-23 28-aa-bb-cc-dd-ee-01-67 \r\n", Cmd::Stvls, 11, 0);
  CHECK(r.len == 59);  // longest real request (spec 01 §2.4)
  checkLine(buildSetValveSensors(0, zero, zero, r), r,
            "stvls 0 00-00-00-00-00-00-00-00 00-00-00-00-00-00-00-00 \r\n", Cmd::Stvls, 0, 0);
  checkLine(buildSetValveSensors(5, zero, id(kId3), r), r,
            "stvls 5 00-00-00-00-00-00-00-00 26-11-22-33-44-55-66-29 \r\n", Cmd::Stvls, 5, 0);
  // Upper-case input is emitted lower case.
  checkLine(buildSetValveSensors(1, id("28-AA-BB-CC-DD-EE-01-67"), zero, r), r,
            "stvls 1 28-aa-bb-cc-dd-ee-01-67 00-00-00-00-00-00-00-00 \r\n", Cmd::Stvls, 1, 0);

  OneWireId bad = id(kId1);
  bad.b[7] ^= 1;
  r = garbage();
  checkRejected(buildSetValveSensors(0, bad, zero, r), r);
  r = garbage();
  checkRejected(buildSetValveSensors(0, zero, bad, r), r);
  r = garbage();
  checkRejected(buildSetValveSensors(12, zero, zero, r), r);
  r = garbage();
  checkRejected(buildSetValveSensors(kAllValves, id(kId1), id(kId2), r), r);
}

TEST_CASE("codec: buildSetLearnMovements") {
  RequestLine r;
  checkLine(buildSetLearnMovements(0, r), r, "stlnm 0 \r\n", Cmd::Stlnm, kNoValve, 0);
  checkLine(buildSetLearnMovements(50, r), r, "stlnm 50 \r\n", Cmd::Stlnm, kNoValve, 50);
  checkLine(buildSetLearnMovements(65534, r), r, "stlnm 65534 \r\n", Cmd::Stlnm, kNoValve, 65534);
  r = garbage();
  checkRejected(buildSetLearnMovements(49, r), r);
  r = garbage();
  checkRejected(buildSetLearnMovements(65535, r), r);
  r = garbage();
  checkRejected(buildSetLearnMovements(4000000000u, r), r);
}

TEST_CASE("codec: buildSetMotorChars") {
  RequestLine r;
  MotorChars m;
  checkLine(buildSetMotorChars(m, r), r, "smotc 17 17 30 3000 2 \r\n", Cmd::Smotc, kNoValve, 17);
  m.lowFactor = 10;
  m.highFactor = 40;
  m.startOnPower = 100;
  m.minCounts = 60000;
  m.maxCalibRetries = 0;
  m.fieldCount = 3;  // parse-only field: always 5 args sent
  checkLine(buildSetMotorChars(m, r), r, "smotc 10 40 100 60000 0 \r\n", Cmd::Smotc, kNoValve, 10);
  m.lowFactor = 5;  // the v1 STM would take it, but loses it on reboot
  r = garbage();
  checkRejected(buildSetMotorChars(m, r), r);
}

TEST_CASE("codec: buildServiceMove") {
  RequestLine r;
  checkLine(buildServiceMove(3, MoveDir::Close, 500, 30, r), r, "svmov 3 1 500 30 \r\n",
            Cmd::Svmov, 3, 1);
  checkLine(buildServiceMove(0, MoveDir::Open, 1, 5, r), r, "svmov 0 0 1 5 \r\n", Cmd::Svmov, 0,
            0);
  checkLine(buildServiceMove(11, MoveDir::Open, 10000, 60, r), r, "svmov 11 0 10000 60 \r\n",
            Cmd::Svmov, 11, 0);
  r = garbage();
  checkRejected(buildServiceMove(12, MoveDir::Open, 1, 5, r), r);
  r = garbage();
  checkRejected(buildServiceMove(0, MoveDir::Open, 0, 5, r), r);
  r = garbage();
  checkRejected(buildServiceMove(0, MoveDir::Open, 10001, 5, r), r);
  r = garbage();
  checkRejected(buildServiceMove(0, MoveDir::Open, 1, 4, r), r);
  r = garbage();
  checkRejected(buildServiceMove(0, MoveDir::Open, 1, 61, r), r);
  r = garbage();
  checkRejected(buildServiceMove(0, static_cast<MoveDir>(2), 1, 5, r), r);
}

TEST_CASE("codec: buildSetBreakaway") {
  RequestLine r;
  Breakaway b;
  b.enable = true;
  b.stepPct = 25;
  b.maxmA = 45;
  checkLine(buildSetBreakaway(b, r), r, "scalx 1 25 45 \r\n", Cmd::Scalx, kNoValve, 1);
  b.enable = false;
  b.stepPct = 0;
  b.maxmA = 20;
  checkLine(buildSetBreakaway(b, r), r, "scalx 0 0 20 \r\n", Cmd::Scalx, kNoValve, 0);
  b.maxmA = 61;
  r = garbage();
  checkRejected(buildSetBreakaway(b, r), r);
}

TEST_CASE("codec: every built request honours the wire rules") {
  // Build one of each and check: ends with " \r\n", single spaces, <= 63.
  RequestLine lines[48];
  size_t n = 0;
  MotorChars m;
  Breakaway b;
  REQUIRE(buildSetTarget(11, 100, lines[n++]));
  REQUIRE(buildGetTarget(11, lines[n++]));
  REQUIRE(buildValveData(11, lines[n++]));
  REQUIRE(buildValveStates(lines[n++]));
  REQUIRE(buildTempCount(lines[n++]));
  REQUIRE(buildTempList(lines[n++]));
  REQUIRE(buildTempData(33, lines[n++]));
  REQUIRE(buildValveSensors(255, lines[n++]));
  REQUIRE(buildVoltCount(lines[n++]));
  REQUIRE(buildVoltList(lines[n++]));
  REQUIRE(buildVoltData(7, lines[n++]));
  REQUIRE(buildScanOneWire(lines[n++]));
  REQUIRE(buildSetValveSensors(11, id(kId1), id(kId2), lines[n++]));
  REQUIRE(buildMatchSensors(lines[n++]));
  REQUIRE(buildAssembly(255, lines[n++]));
  REQUIRE(buildCalibrate(255, lines[n++]));
  REQUIRE(buildDetect(lines[n++]));
  REQUIRE(buildSetLearnMovements(65534, lines[n++]));
  REQUIRE(buildGetLearnMovements(lines[n++]));
  REQUIRE(buildSetMotorChars(m, lines[n++]));
  REQUIRE(buildGetMotorChars(lines[n++]));
  REQUIRE(buildGetVersion(lines[n++]));
  REQUIRE(buildGetHwId(lines[n++]));
  REQUIRE(buildEepromState(lines[n++]));
  REQUIRE(buildSoftReset(lines[n++]));
  REQUIRE(buildGetProto(lines[n++]));
  REQUIRE(buildValveEx(11, lines[n++]));
  REQUIRE(buildProfile(11, lines[n++]));
  REQUIRE(buildServiceMove(11, MoveDir::Close, 10000, 60, lines[n++]));
  REQUIRE(buildSetBreakaway(b, lines[n++]));
  REQUIRE(buildGetBreakaway(lines[n++]));
  REQUIRE(buildGetStatus(lines[n++]));
  REQUIRE(buildValveExV3(11, lines[n++]));
  REQUIRE(buildGetStatusV3(lines[n++]));
  REQUIRE(buildHeartbeat(true, lines[n++]));
  REQUIRE(buildSetLeaseTimeout(1440, lines[n++]));
  REQUIRE(buildSetFailsafe(255, 255, lines[n++]));
  REQUIRE(buildGetLeaseConfig(lines[n++]));
  REQUIRE(buildStop(255, lines[n++]));
  REQUIRE(buildGetLearnTime(lines[n++]));
  REQUIRE(buildLeaveSafeMode(lines[n++]));
  REQUIRE(buildSetLearnTime(0xFFFFFFFFu, lines[n++]));
  CHECK(n == 42);
  for (size_t i = 0; i < n; ++i) {
    const std::string t = text(lines[i]);
    CAPTURE(t);
    // Only gproto may stay unanswered (a v1 STM does not know it); no
    // builder sets an expected sensor id.
    CHECK(lines[i].probe == (lines[i].cmd == Cmd::Gproto));
    CHECK(isZero(lines[i].expect));
    CHECK(t.size() <= kRequestMaxLen);
    CHECK(t.size() >= 8);
    CHECK(t.substr(t.size() - 3) == " \r\n");
    CHECK(t.find("  ") == std::string::npos);
    CHECK(t.find(' ') != std::string::npos);  // never a line without a space
    CHECK((t.find('-') == std::string::npos) == (lines[i].cmd != Cmd::Stvls));  // no negatives
    CHECK(t.substr(0, strlen(cmdName(lines[i].cmd))) == cmdName(lines[i].cmd));
    CHECK(t[strlen(cmdName(lines[i].cmd))] == ' ');
  }
}

// ================================================================ replies: basics

TEST_CASE("codec: parse status names") {
  CHECK(std::string(parseStatusName(ParseStatus::Ok)) == "ok");
  CHECK(std::string(parseStatusName(ParseStatus::Empty)) == "empty");
  CHECK(std::string(parseStatusName(ParseStatus::UnknownCommand)) == "unknown_command");
  CHECK(std::string(parseStatusName(ParseStatus::BadArgCount)) == "bad_arg_count");
  CHECK(std::string(parseStatusName(ParseStatus::BadNumber)) == "bad_number");
  CHECK(std::string(parseStatusName(ParseStatus::OutOfRange)) == "out_of_range");
  CHECK(std::string(parseStatusName(ParseStatus::BadOneWireId)) == "bad_onewire_id");
  CHECK(std::string(parseStatusName(ParseStatus::BadFormat)) == "bad_format");
  CHECK(std::string(parseStatusName(ParseStatus::TooLong)) == "too_long");
  CHECK(std::string(parseStatusName(static_cast<ParseStatus>(99))) == "unknown");
}

TEST_CASE("codec: stop reason names") {
  CHECK(std::string(stopReasonName(StopReason::None)) == "none");
  CHECK(std::string(stopReasonName(StopReason::Target)) == "target");
  CHECK(std::string(stopReasonName(StopReason::EndStop)) == "endstop");
  CHECK(std::string(stopReasonName(StopReason::EarlyEndStop)) == "early_endstop");
  CHECK(std::string(stopReasonName(StopReason::Timeout)) == "timeout");
  CHECK(std::string(stopReasonName(StopReason::UnderCurrent)) == "undercurrent");
  CHECK(std::string(stopReasonName(StopReason::SafetyOverCurrent)) == "safety_overcurrent");
  CHECK(std::string(stopReasonName(StopReason::Aborted)) == "aborted");
  CHECK(std::string(stopReasonName(static_cast<StopReason>(8))) == "unknown");
}

TEST_CASE("codec: empty, blank, null and too long lines") {
  Reply r;
  dirty(r);
  CHECK(parseReply(nullptr, 5, r) == ParseStatus::Empty);
  CHECK(isEmptyReply(r));
  dirty(r);
  CHECK(parse("", r) == ParseStatus::Empty);
  CHECK(isEmptyReply(r));
  CHECK(parse("     ", r) == ParseStatus::Empty);
  // Only ' ' separates tokens; a TAB is part of a token.
  CHECK(parse("\t", r) == ParseStatus::UnknownCommand);

  std::string atLimit = "stons" + std::string(kStmMaxLineLen - 5, ' ');
  CHECK(atLimit.size() == kStmMaxLineLen);
  CHECK(parse(atLimit, r) == ParseStatus::Ok);
  CHECK(r.cmd == Cmd::Stons);
  dirty(r);
  CHECK(parse(atLimit + " ", r) == ParseStatus::TooLong);
  CHECK(isEmptyReply(r));

  // 40 tokens are fine for the tokenizer (then the payload check decides).
  std::string t40 = "gprof";
  for (int i = 0; i < 39; ++i) t40 += " 1";
  CHECK(parse(t40, r) == ParseStatus::BadArgCount);
  CHECK(parse(t40 + " 1", r) == ParseStatus::TooLong);
}

TEST_CASE("codec: unknown commands") {
  Reply r;
  const char* unknown[] = {"stsnx", "gvlvz", "gactp", "ESPalive", "gvlvdd 1", "gvl", "123456",
                           "GVLVD 1", "gprot 2", ",", "gvlvd,1"};
  for (const char* s : unknown) {
    CAPTURE(s);
    dirty(r);
    CHECK(parse(s, r) == ParseStatus::UnknownCommand);
    CHECK(isEmptyReply(r));
  }
}

TEST_CASE("codec: whitespace tolerance") {
  Reply r;
  CHECK(parse("   gtgtp    3     50     ", r) == ParseStatus::Ok);
  CHECK(r.cmd == Cmd::Gtgtp);
  CHECK(r.target.valve == 3);
  CHECK(r.target.target == 50);
  CHECK(parse("gtgtp 3\t50", r) == ParseStatus::BadArgCount);
  CHECK(parse("gtgtp 3 50\t", r) == ParseStatus::BadNumber);
  // parseReply reads exactly len bytes.
  const char buf[] = "gtgtp 3 50 garbage";
  CHECK(parseReply(buf, 10, r) == ParseStatus::Ok);
  CHECK(r.target.target == 50);
}

// ================================================================ replies: acks

TEST_CASE("codec: ack replies (golden)") {
  struct Case {
    const char* line;
    Cmd cmd;
  } cases[] = {
      {"stgtp", Cmd::Stgtp}, {"stons", Cmd::Stons}, {"staln", Cmd::Staln}, {"stlnm", Cmd::Stlnm},
      {"smotc", Cmd::Smotc}, {"staop ", Cmd::Staop}, {"stdet ", Cmd::Stdet},
      {"masns ", Cmd::Masns}, {"reset ", Cmd::Reset},
  };
  for (const Case& c : cases) {
    CAPTURE(c.line);
    Reply r;
    dirty(r);
    REQUIRE(parse(c.line, r) == ParseStatus::Ok);
    CHECK(r.cmd == c.cmd);
    CHECK_FALSE(r.ack.error);
    CHECK(r.ack.valve == kNoValve);
    CHECK_FALSE(r.gvlonError);
    CHECK(r.valveData.moves == 0);  // rest reset
    // An ack takes no argument (smotc: only "err").
    dirty(r);
    CHECK(parse(std::string(c.line) + " 1", r) ==
          (c.cmd == Cmd::Smotc ? ParseStatus::BadFormat : ParseStatus::BadArgCount));
    CHECK(isEmptyReply(r));
  }
}

TEST_CASE("codec: smotc and scalx ok/err forms") {
  Reply r;
  REQUIRE(parse("smotc err", r) == ParseStatus::Ok);
  CHECK(r.cmd == Cmd::Smotc);
  CHECK(r.ack.error);
  CHECK(parse("smotc ok", r) == ParseStatus::BadFormat);
  CHECK(parse("smotc err 1", r) == ParseStatus::BadArgCount);
  CHECK(parse("smotc ERR", r) == ParseStatus::BadFormat);

  REQUIRE(parse("scalx ok", r) == ParseStatus::Ok);
  CHECK(r.cmd == Cmd::Scalx);
  CHECK_FALSE(r.ack.error);
  REQUIRE(parse("scalx err ", r) == ParseStatus::Ok);
  CHECK(r.ack.error);
  CHECK(parse("scalx", r) == ParseStatus::BadArgCount);
  CHECK(parse("scalx ok ok", r) == ParseStatus::BadArgCount);
  CHECK(parse("scalx fine", r) == ParseStatus::BadFormat);
  CHECK(parse("scalx o", r) == ParseStatus::BadFormat);
  CHECK(parse("scalx oks", r) == ParseStatus::BadFormat);
}

TEST_CASE("codec: stvls ack carries the valve") {
  Reply r;
  REQUIRE(parse("stvls 3", r) == ParseStatus::Ok);  // v1: no trailing space
  CHECK(r.cmd == Cmd::Stvls);
  CHECK(r.ack.valve == 3);
  CHECK_FALSE(r.ack.error);
  REQUIRE(parse("stvls 0", r) == ParseStatus::Ok);
  CHECK(r.ack.valve == 0);
  REQUIRE(parse("stvls 11 ", r) == ParseStatus::Ok);
  CHECK(r.ack.valve == 11);
  CHECK(parse("stvls 12", r) == ParseStatus::OutOfRange);
  CHECK(parse("stvls", r) == ParseStatus::BadArgCount);
  CHECK(parse("stvls 1 2", r) == ParseStatus::BadArgCount);
  CHECK(parse("stvls x", r) == ParseStatus::BadNumber);
}

// ================================================================ replies: v1 data

TEST_CASE("codec: gtgtp") {
  Reply r;
  REQUIRE(parse("gtgtp 3 50 ", r) == ParseStatus::Ok);
  CHECK(r.cmd == Cmd::Gtgtp);
  CHECK(r.target.valve == 3);
  CHECK(r.target.target == 50);
  REQUIRE(parse("gtgtp 11 100", r) == ParseStatus::Ok);
  CHECK(r.target.valve == 11);
  CHECK(r.target.target == 100);
  REQUIRE(parse("gtgtp 0 0", r) == ParseStatus::Ok);
  CHECK(r.target.target == 0);
  CHECK(parse("gtgtp 12 50", r) == ParseStatus::OutOfRange);
  CHECK(parse("gtgtp 3 101", r) == ParseStatus::OutOfRange);
  CHECK(parse("gtgtp 3", r) == ParseStatus::BadArgCount);
  CHECK(parse("gtgtp 3 50 1", r) == ParseStatus::BadArgCount);
  CHECK(parse("gtgtp -1 50", r) == ParseStatus::BadNumber);
}

TEST_CASE("codec: gvlvd golden and calibrating bit") {
  Reply r;
  dirty(r);
  REQUIRE(parse("gvlvd 3 42 18 1 215 -500 57 3120 3350 230 0 ", r) == ParseStatus::Ok);
  CHECK(r.cmd == Cmd::Gvlvd);
  const ValveData& d = r.valveData;
  CHECK(d.valve == 3);
  CHECK(d.position == 42);
  CHECK(d.meanCurrent == 18);
  CHECK(d.status == 1);
  CHECK_FALSE(d.calibrating);
  CHECK(d.temp1 == 215);
  CHECK(d.temp2 == -500);
  CHECK(d.moves == 57);
  CHECK(d.openCount == 3120);
  CHECK(d.closeCount == 3350);
  CHECK(d.deadZone == 230);
  CHECK(d.calibRetries == 0);
  CHECK_FALSE(r.gvlonError);  // other members reset
  CHECK(r.status.uptimeS == 0);

  REQUIRE(parse("gvlvd 0 0 20 131 -1270 850 0 0 0 0 2", r) == ParseStatus::Ok);
  CHECK(r.valveData.status == 3);
  CHECK(r.valveData.calibrating);
  CHECK(r.valveData.temp1 == -1270);
  CHECK(r.valveData.temp2 == 850);
  CHECK(r.valveData.calibRetries == 2);

  REQUIRE(parse("gvlvd 11 100 65535 255 32767 -32768 4294967295 4294967295 4294967295 -2147483648 255",
                r) == ParseStatus::Ok);
  CHECK(r.valveData.valve == 11);
  CHECK(r.valveData.position == 100);
  CHECK(r.valveData.meanCurrent == 65535);
  CHECK(r.valveData.status == 127);
  CHECK(r.valveData.calibrating);
  CHECK(r.valveData.temp1 == 32767);
  CHECK(r.valveData.temp2 == -32768);
  CHECK(r.valveData.moves == 4294967295u);
  CHECK(r.valveData.openCount == 4294967295u);
  CHECK(r.valveData.closeCount == 4294967295u);
  CHECK(r.valveData.deadZone == INT32_MIN);
  CHECK(r.valveData.calibRetries == 255);

  REQUIRE(parse("gvlvd 1 0 0 128 0 0 0 0 0 2147483647 0", r) == ParseStatus::Ok);
  CHECK(r.valveData.status == 0);
  CHECK(r.valveData.calibrating);
  CHECK(r.valveData.deadZone == 2147483647);
  REQUIRE(parse("gvlvd 1 0 0 127 0 0 0 0 0 -230 0", r) == ParseStatus::Ok);
  CHECK(r.valveData.status == 127);
  CHECK_FALSE(r.valveData.calibrating);
  CHECK(r.valveData.deadZone == -230);
}

TEST_CASE("codec: gvlvd rejects every bad field") {
  const char* base[] = {"3", "42", "18", "1", "215", "-500", "57", "3120", "3350", "230", "0"};
  // Field index -> out-of-range value, bad-number value.
  const char* oor[] = {"12",    "101",         "65536",       "256",        "32768", "-32769",
                       "4294967296", "4294967296", "4294967296", "2147483648", "256"};
  const char* bad[] = {"-1", "-1", "-1", "-1", "2a", "+5", "-1", "-1", "-1", "--1", "-1"};
  for (int f = 0; f < 11; ++f) {
    CAPTURE(f);
    for (int k = 0; k < 2; ++k) {
      std::string line = "gvlvd";
      for (int i = 0; i < 11; ++i) {
        line += ' ';
        line += i == f ? (k == 0 ? oor[i] : bad[i]) : base[i];
      }
      Reply r;
      dirty(r);
      const ParseStatus st = parse(line, r);
      CAPTURE(line);
      CHECK(st == (k == 0 ? ParseStatus::OutOfRange : ParseStatus::BadNumber));
      CHECK(isEmptyReply(r));
    }
  }
  Reply r;
  CHECK(parse("gvlvd 3 42 18 1 215 -500 57 3120 3350 230", r) == ParseStatus::BadArgCount);
  CHECK(parse("gvlvd 3 42 18 1 215 -500 57 3120 3350 230 0 0", r) == ParseStatus::BadArgCount);
  CHECK(parse("gvlvd", r) == ParseStatus::BadArgCount);
  CHECK(parse("gvlvd 3 42 18 1 215 -500 57 3120 3350 -2147483649 0", r) == ParseStatus::OutOfRange);
  CHECK(parse("gvlvd 3 42 18 1 215 -500 57 3120 3350 - 0", r) == ParseStatus::BadNumber);
  CHECK(parse("gvlvd 3 42 18 1 215 -500 57 12345678901 3350 0 0", r) == ParseStatus::BadNumber);
}

TEST_CASE("codec: gvlst with and without the v1 trailing comma") {
  Reply r;
  dirty(r);
  REQUIRE(parse("gvlst 12 8,6,6,8,6,6,6,6,6,6,6,6, ", r) == ParseStatus::Ok);
  CHECK(r.cmd == Cmd::Gvlst);
  const uint8_t expect[] = {8, 6, 6, 8, 6, 6, 6, 6, 6, 6, 6, 6};
  for (int i = 0; i < 12; ++i) CHECK(r.valveStates.status[i] == expect[i]);
  CHECK(r.valveData.moves == 0);

  REQUIRE(parse("gvlst 12 0,1,2,3,4,5,6,7,8,9,255,137 ", r) == ParseStatus::Ok);
  for (int i = 0; i < 10; ++i) CHECK(r.valveStates.status[i] == i);
  CHECK(r.valveStates.status[10] == 255);
  CHECK(r.valveStates.status[11] == 137);

  CHECK(parse("gvlst 12 1,1,1,1,1,1,1,1,1,1,1", r) == ParseStatus::BadFormat);      // 11
  CHECK(parse("gvlst 12 1,1,1,1,1,1,1,1,1,1,1,1,1", r) == ParseStatus::BadFormat);  // 13
  CHECK(parse("gvlst 12 1,1,1,1,1,1,1,1,1,1,1,1,,", r) == ParseStatus::BadFormat);
  CHECK(parse("gvlst 12 1,,1,1,1,1,1,1,1,1,1,1,1", r) == ParseStatus::BadFormat);
  CHECK(parse("gvlst 12 ,1,1,1,1,1,1,1,1,1,1,1,1", r) == ParseStatus::BadFormat);
  CHECK(parse("gvlst 12 ,", r) == ParseStatus::BadFormat);
  CHECK(parse("gvlst 12 1,1,1,1,1,1,1,1,1,1,1,256", r) == ParseStatus::OutOfRange);
  CHECK(parse("gvlst 12 1,1,1,1,1,1,1,1,1,1,1,x", r) == ParseStatus::BadNumber);
  CHECK(parse("gvlst 11 1,1,1,1,1,1,1,1,1,1,1", r) == ParseStatus::OutOfRange);
  CHECK(parse("gvlst 13 1,1,1,1,1,1,1,1,1,1,1,1,1", r) == ParseStatus::OutOfRange);
  CHECK(parse("gvlst 12", r) == ParseStatus::BadArgCount);
  CHECK(parse("gvlst 12 1,1,1,1,1,1 1,1,1,1,1,1", r) == ParseStatus::BadArgCount);
  CHECK(parse("gvlst x 1,1,1,1,1,1,1,1,1,1,1,1", r) == ParseStatus::BadNumber);
}

TEST_CASE("codec: gonec / gowvc count and list forms") {
  Reply r;
  REQUIRE(parse("gonec 0 ", r) == ParseStatus::Ok);
  CHECK(r.cmd == Cmd::Gonec);
  CHECK(r.oneWireList.count == 0);
  CHECK_FALSE(r.oneWireList.hasList);
  REQUIRE(parse("gonec 34 ", r) == ParseStatus::Ok);
  CHECK(r.oneWireList.count == 34);
  CHECK_FALSE(r.oneWireList.hasList);
  CHECK(parse("gonec 35", r) == ParseStatus::OutOfRange);
  REQUIRE(parse("gowvc 8", r) == ParseStatus::Ok);
  CHECK(r.cmd == Cmd::Gowvc);
  CHECK(r.oneWireList.count == 8);
  CHECK(parse("gowvc 9", r) == ParseStatus::OutOfRange);

  const std::string three = std::string(kId1) + "," + kId2 + "," + kId3;
  REQUIRE(parse("gonec 3 " + three + " ", r) == ParseStatus::Ok);
  CHECK(r.oneWireList.count == 3);
  CHECK(r.oneWireList.hasList);
  CHECK(r.oneWireList.ids[0] == id(kId1));
  CHECK(r.oneWireList.ids[1] == id(kId2));
  CHECK(r.oneWireList.ids[2] == id(kId3));
  CHECK(isZero(r.oneWireList.ids[3]));
  REQUIRE(parse("gowvc 3 " + three + ",", r) == ParseStatus::Ok);  // trailing comma
  CHECK(r.oneWireList.ids[2] == id(kId3));

  // Full lists at the limits; the 34-sensor line is the longest reply.
  const std::string line34 = "gonec 34 " + idList(kId1, 34, false) + " ";
  CHECK(line34.size() == 9 + 34 * 23 + 33 + 1);
  REQUIRE(parse(line34, r) == ParseStatus::Ok);
  CHECK(r.oneWireList.count == 34);
  CHECK(r.oneWireList.ids[33] == id(kId1));
  REQUIRE(parse("gowvc 8 " + idList(kId3, 8, true), r) == ParseStatus::Ok);
  CHECK(r.oneWireList.count == 8);

  CHECK(parse("gonec 35 " + idList(kId1, 35, false), r) == ParseStatus::OutOfRange);
  CHECK(parse("gonec 3 " + std::string(kId1) + "," + kId2, r) == ParseStatus::BadFormat);  // short
  CHECK(parse("gonec 1 " + std::string(kId1) + "," + kId2, r) == ParseStatus::BadFormat);  // long
  CHECK(parse("gonec 0 " + std::string(kId1), r) == ParseStatus::BadFormat);
  CHECK(parse("gonec 2 " + std::string(kId1) + ",," + kId2, r) == ParseStatus::BadFormat);
  CHECK(parse("gonec 1 28-84-37-94-97-ff-03-2", r) == ParseStatus::BadOneWireId);
  CHECK(parse("gonec 1 28-84-37-94-97-ff-03-2g", r) == ParseStatus::BadOneWireId);
  CHECK(parse("gonec 1 28:84-37-94-97-ff-03-23", r) == ParseStatus::BadOneWireId);
  CHECK(parse("gonec 2 " + std::string(kId1) + " " + kId2, r) == ParseStatus::BadArgCount);
  CHECK(parse("gonec", r) == ParseStatus::BadArgCount);
  CHECK(parse("gonec -1", r) == ParseStatus::BadNumber);
}

TEST_CASE("codec: goned / gowvd data and error forms") {
  Reply r;
  dirty(r);
  REQUIRE(parse("goned 28-84-37-94-97-ff-03-23 215 ", r) == ParseStatus::Ok);
  CHECK(r.cmd == Cmd::Goned);
  CHECK(r.tempData.valid);
  CHECK(r.tempData.id == id(kId1));
  CHECK(r.tempData.value == 215);
  CHECK_FALSE(r.gvlonError);
  REQUIRE(parse("goned 00-00-00-00-00-00-00-00 -500", r) == ParseStatus::Ok);  // stale index
  CHECK(r.tempData.valid);
  CHECK(isZero(r.tempData.id));
  CHECK(r.tempData.value == -500);
  REQUIRE(parse("goned 28-84-37-94-97-FF-03-23 -32768", r) == ParseStatus::Ok);
  CHECK(r.tempData.value == -32768);
  CHECK(parse("goned 28-84-37-94-97-ff-03-23 32768", r) == ParseStatus::OutOfRange);
  CHECK(parse("goned 28-84-37-94-97-ff-03-23 -32769", r) == ParseStatus::OutOfRange);
  CHECK(parse("goned 28-84-37-94-97-ff-03-23 21.5", r) == ParseStatus::BadNumber);
  CHECK(parse("goned 28-84-37-94-97-ff-03 215", r) == ParseStatus::BadOneWireId);

  REQUIRE(parse("goned 0 ", r) == ParseStatus::Ok);
  CHECK(r.cmd == Cmd::Goned);
  CHECK_FALSE(r.tempData.valid);
  CHECK(r.tempData.value == kTempUnassigned);
  CHECK(parse("goned 00", r) == ParseStatus::BadFormat);
  CHECK(parse("goned 1", r) == ParseStatus::BadFormat);
  CHECK(parse("goned", r) == ParseStatus::BadArgCount);
  CHECK(parse("goned 1 2 3", r) == ParseStatus::BadArgCount);

  REQUIRE(parse("gowvd 26-11-22-33-44-55-66-29 1234 ", r) == ParseStatus::Ok);
  CHECK(r.cmd == Cmd::Gowvd);
  CHECK(r.voltData.valid);
  CHECK(r.voltData.id == id(kId3));
  CHECK(r.voltData.vad == 1234);
  REQUIRE(parse("gowvd 26-11-22-33-44-55-66-29 -1000", r) == ParseStatus::Ok);
  CHECK(r.voltData.vad == kVadFailed);
  REQUIRE(parse("gowvd 26-11-22-33-44-55-66-29 -2147483648", r) == ParseStatus::Ok);
  CHECK(r.voltData.vad == INT32_MIN);
  REQUIRE(parse("gowvd 26-11-22-33-44-55-66-29 2147483647", r) == ParseStatus::Ok);
  CHECK(r.voltData.vad == INT32_MAX);
  CHECK(parse("gowvd 26-11-22-33-44-55-66-29 2147483648", r) == ParseStatus::OutOfRange);
  REQUIRE(parse("gowvd 0", r) == ParseStatus::Ok);
  CHECK_FALSE(r.voltData.valid);
  CHECK(r.voltData.vad == kVadFailed);
  // "error" is only the gvlon quirk on the goned prefix.
  CHECK(parse("gowvd error", r) == ParseStatus::BadFormat);
}

TEST_CASE("codec: v1 gvlon error arrives with the goned prefix") {
  Reply r;
  dirty(r);
  REQUIRE(parse("goned error ", r) == ParseStatus::Ok);
  CHECK(r.cmd == Cmd::Gvlon);
  CHECK(r.gvlonError);
  CHECK_FALSE(r.tempData.valid);
  CHECK(r.valveData.moves == 0);
  CHECK(parse("goned error 1", r) == ParseStatus::BadOneWireId);
  CHECK(parse("goned errors", r) == ParseStatus::BadFormat);
  CHECK(parse("gvlon error", r) == ParseStatus::BadArgCount);
}

TEST_CASE("codec: gvlon single and list") {
  Reply r;
  REQUIRE(parse(std::string("gvlon 3 ") + kId1 + " " + kId2 + " ", r) == ParseStatus::Ok);
  CHECK(r.cmd == Cmd::Gvlon);
  CHECK_FALSE(r.valveSensors.isList);
  CHECK_FALSE(r.gvlonError);
  CHECK(r.valveSensors.valve == 3);
  CHECK(r.valveSensors.ids[3][0] == id(kId1));
  CHECK(r.valveSensors.ids[3][1] == id(kId2));
  CHECK(isZero(r.valveSensors.ids[0][0]));
  REQUIRE(parse("gvlon 11 00-00-00-00-00-00-00-00 " + std::string(kId2), r) == ParseStatus::Ok);
  CHECK(r.valveSensors.valve == 11);
  CHECK(isZero(r.valveSensors.ids[11][0]));
  CHECK(r.valveSensors.ids[11][1] == id(kId2));
  CHECK(parse("gvlon 12 " + std::string(kId1) + " " + kId2, r) == ParseStatus::OutOfRange);
  CHECK(parse("gvlon 3 x " + std::string(kId2), r) == ParseStatus::BadOneWireId);
  CHECK(parse("gvlon 3 " + std::string(kId2) + " x", r) == ParseStatus::BadOneWireId);
  CHECK(parse("gvlon 3", r) == ParseStatus::BadArgCount);
  CHECK(parse("gvlon 3 a b c", r) == ParseStatus::BadArgCount);

  // List: 24 ids in valve pairs; garbage ids (v1 OOB read) are passed verbatim.
  std::string list;
  for (int v = 0; v < 12; ++v) {
    list += v % 2 ? kId1 : kId2;
    list += ',';
    list += v == 5 ? "de-ad-be-ef-01-02-03-04" : "00-00-00-00-00-00-00-00";
    if (v < 11) list += ',';
  }
  REQUIRE(parse("gvlon 12 " + list + " ", r) == ParseStatus::Ok);
  CHECK(r.valveSensors.isList);
  for (int v = 0; v < 12; ++v) {
    CHECK(r.valveSensors.ids[v][0] == id(v % 2 ? kId1 : kId2));
  }
  CHECK(r.valveSensors.ids[5][1] == id("de-ad-be-ef-01-02-03-04"));
  CHECK(isZero(r.valveSensors.ids[4][1]));
  REQUIRE(parse("gvlon 12 " + list + ",", r) == ParseStatus::Ok);
  CHECK(parse("gvlon 12 " + list + "," + kId1, r) == ParseStatus::BadFormat);  // 25 ids
  CHECK(parse("gvlon 12 " + idList(kId1, 23, false), r) == ParseStatus::BadFormat);
  CHECK(parse("gvlon 11 " + idList(kId1, 22, false), r) == ParseStatus::OutOfRange);
  CHECK(parse("gvlon 12 " + idList(kId1, 23, false) + ",bad", r) == ParseStatus::BadOneWireId);
}

TEST_CASE("codec: gtlnm, ghwin, eepst") {
  Reply r;
  REQUIRE(parse("gtlnm 2000 ", r) == ParseStatus::Ok);
  CHECK(r.cmd == Cmd::Gtlnm);
  CHECK(r.learnMovements == 2000);
  REQUIRE(parse("gtlnm 0", r) == ParseStatus::Ok);
  CHECK(r.learnMovements == 0);
  REQUIRE(parse("gtlnm 65535", r) == ParseStatus::Ok);
  CHECK(r.learnMovements == 65535);
  CHECK(parse("gtlnm 65536", r) == ParseStatus::OutOfRange);
  CHECK(parse("gtlnm", r) == ParseStatus::BadArgCount);
  CHECK(parse("gtlnm 1 2", r) == ParseStatus::BadArgCount);

  REQUIRE(parse("ghwin 1073 ", r) == ParseStatus::Ok);
  CHECK(r.cmd == Cmd::Ghwin);
  CHECK(r.hwId == 0x431);
  CHECK(std::string(stmChipName(r.hwId)) == "STM32F411xx");
  REQUIRE(parse("ghwin 1059", r) == ParseStatus::Ok);
  CHECK(r.hwId == 0x423);
  REQUIRE(parse("ghwin 4095", r) == ParseStatus::Ok);
  CHECK(r.hwId == 4095);
  REQUIRE(parse("ghwin 0", r) == ParseStatus::Ok);
  CHECK(r.hwId == 0);
  CHECK(parse("ghwin 4096", r) == ParseStatus::OutOfRange);
  CHECK(parse("ghwin 0x431", r) == ParseStatus::BadNumber);

  REQUIRE(parse("eepst 1 ", r) == ParseStatus::Ok);
  CHECK(r.cmd == Cmd::Eepst);
  CHECK(r.eepromIdle);
  REQUIRE(parse("eepst 0 ", r) == ParseStatus::Ok);
  CHECK_FALSE(r.eepromIdle);
  CHECK(parse("eepst 2", r) == ParseStatus::OutOfRange);
  CHECK(parse("eepst", r) == ParseStatus::BadArgCount);
}

TEST_CASE("codec: gmotc with 3..5 fields") {
  Reply r;
  REQUIRE(parse("gmotc 17 17 50 3000 0 ", r) == ParseStatus::Ok);
  CHECK(r.cmd == Cmd::Gmotc);
  CHECK(r.motorChars.lowFactor == 17);
  CHECK(r.motorChars.highFactor == 17);
  CHECK(r.motorChars.startOnPower == 50);
  CHECK(r.motorChars.minCounts == 3000);
  CHECK(r.motorChars.maxCalibRetries == 0);
  CHECK(r.motorChars.fieldCount == 5);
  REQUIRE(parse("gmotc 5 50 255 65535 255", r) == ParseStatus::Ok);  // v1 accepts, report verbatim
  CHECK(r.motorChars.lowFactor == 5);
  CHECK(r.motorChars.highFactor == 50);
  CHECK(r.motorChars.startOnPower == 255);
  CHECK(r.motorChars.minCounts == 65535);
  CHECK(r.motorChars.maxCalibRetries == 255);
  REQUIRE(parse("gmotc 12 13 40", r) == ParseStatus::Ok);
  CHECK(r.motorChars.fieldCount == 3);
  CHECK(r.motorChars.lowFactor == 12);
  CHECK(r.motorChars.highFactor == 13);
  CHECK(r.motorChars.startOnPower == 40);
  CHECK(r.motorChars.minCounts == 3000);  // defaults for missing fields
  CHECK(r.motorChars.maxCalibRetries == 2);
  REQUIRE(parse("gmotc 12 13 40 100", r) == ParseStatus::Ok);
  CHECK(r.motorChars.fieldCount == 4);
  CHECK(r.motorChars.minCounts == 100);
  CHECK(r.motorChars.maxCalibRetries == 2);
  CHECK(parse("gmotc 12 13", r) == ParseStatus::BadArgCount);
  CHECK(parse("gmotc 1 2 3 4 5 6", r) == ParseStatus::BadArgCount);
  CHECK(parse("gmotc 256 13 40", r) == ParseStatus::OutOfRange);
  CHECK(parse("gmotc 12 256 40", r) == ParseStatus::OutOfRange);
  CHECK(parse("gmotc 12 13 256", r) == ParseStatus::OutOfRange);
  CHECK(parse("gmotc 12 13 40 65536", r) == ParseStatus::OutOfRange);
  CHECK(parse("gmotc 12 13 40 1 256", r) == ParseStatus::OutOfRange);
}

TEST_CASE("codec: gvers with suffix, hw and build") {
  Reply r;
  dirty(r);
  REQUIRE(parse("gvers 1.4.9_C1 1 ", r) == ParseStatus::Ok);
  CHECK(r.cmd == Cmd::Gvers);
  CHECK(r.version.valid);
  CHECK(r.version.major == 1);
  CHECK(r.version.minor == 4);
  CHECK(r.version.patch == 9);
  CHECK(std::string(r.version.suffix) == "");
  CHECK(std::string(r.version.hw) == "C1");
  CHECK(r.build == 1);

  REQUIRE(parse("gvers 1.4.9_Dev_C2 1712345678 ", r) == ParseStatus::Ok);
  CHECK(std::string(r.version.suffix) == "_Dev");
  CHECK(std::string(r.version.hw) == "C2");
  CHECK(r.build == 1712345678u);
  CHECK_FALSE(isRevamped(r.version));

  REQUIRE(parse("gvers 2.0.0-revamped_C2 1", r) == ParseStatus::Ok);
  CHECK(r.version.major == 2);
  CHECK(std::string(r.version.suffix) == "-revamped");
  CHECK(std::string(r.version.hw) == "C2");
  CHECK(isRevamped(r.version));

  REQUIRE(parse("gvers 2.0.0-revamped-dev_C2 4294967295", r) == ParseStatus::Ok);
  CHECK(std::string(r.version.suffix) == "-revamped-dev");
  CHECK(r.build == 4294967295u);

  dirty(r);
  REQUIRE(parse("gvers 1.4.9_Dev", r) == ParseStatus::Ok);  // no build, no hw
  CHECK(r.build == 0);
  CHECK(std::string(r.version.suffix) == "_Dev");
  CHECK(std::string(r.version.hw) == "");

  Version min;
  REQUIRE(parseVersion("1.4.0", 5, min));
  REQUIRE(parse("gvers 1.4.12+hc2 1", r) == ParseStatus::Ok);
  CHECK(compareVersion(r.version, min) > 0);
  REQUIRE(parse("gvers 1.3.9_C2 1", r) == ParseStatus::Ok);
  CHECK(compareVersion(r.version, min) < 0);

  dirty(r);
  CHECK(parse("gvers 1.4 1", r) == ParseStatus::BadFormat);
  CHECK(isEmptyReply(r));
  CHECK(parse("gvers v1.4.9 1", r) == ParseStatus::BadFormat);
  CHECK(parse("gvers 1.4.9 x", r) == ParseStatus::BadNumber);
  CHECK(parse("gvers 1.4.9 4294967296", r) == ParseStatus::OutOfRange);
  CHECK(parse("gvers", r) == ParseStatus::BadArgCount);
  CHECK(parse("gvers 1.4.9 1 2", r) == ParseStatus::BadArgCount);
}

// ================================================================ replies: v2

TEST_CASE("codec: gproto") {
  Reply r;
  REQUIRE(parse("gproto 2", r) == ParseStatus::Ok);
  CHECK(r.cmd == Cmd::Gproto);
  CHECK(r.proto == 2);
  REQUIRE(parse("gproto 1 ", r) == ParseStatus::Ok);
  CHECK(r.proto == 1);
  REQUIRE(parse("gproto 255", r) == ParseStatus::Ok);
  CHECK(r.proto == 255);
  CHECK(parse("gproto 0", r) == ParseStatus::OutOfRange);
  CHECK(parse("gproto 256", r) == ParseStatus::OutOfRange);
  CHECK(parse("gproto", r) == ParseStatus::BadArgCount);
}

namespace {
const char* const kGvlvxFields[19] = {"4",  "130", "42",   "60",  "21", "3120", "3350",
                                      "-230", "1", "57",   "2",   "7",  "3",    "1",
                                      "3000", "1450", "3", "412", "8123"};
std::string gvlvxLine(int replace = -1, const char* value = nullptr) {
  std::string s = "gvlvx";
  for (int i = 0; i < 19; ++i) {
    s += ' ';
    s += i == replace ? value : kGvlvxFields[i];
  }
  return s;
}
}  // namespace

TEST_CASE("codec: gvlvx golden") {
  Reply r;
  dirty(r);
  REQUIRE(parse(gvlvxLine() + " ", r) == ParseStatus::Ok);
  CHECK(r.cmd == Cmd::Gvlvx);
  const ValveEx& x = r.valveEx;
  CHECK(x.valve == 4);
  CHECK(x.status == 2);
  CHECK(x.calibrating);
  CHECK(x.position == 42);
  CHECK(x.target == 60);
  CHECK(x.meanCurrent == 21);
  CHECK(x.openCount == 3120);
  CHECK(x.closeCount == 3350);
  CHECK(x.deadZone == -230);
  CHECK(x.calibRetries == 1);
  CHECK(x.moves == 57);
  CHECK(x.calState == 2);
  CHECK(x.calFlags == 0);
  CHECK(x.earlyStops == 7);
  CHECK(x.cmdRejected == 3);
  CHECK(x.lastMove.dir == MoveDir::Close);
  CHECK(x.lastMove.requestedCounts == 3000);
  CHECK(x.lastMove.countedCounts == 1450);
  CHECK(x.lastMove.stop == StopReason::EarlyEndStop);
  CHECK(x.lastMove.peakCurrent == 412);
  CHECK(x.lastMove.durationMs == 8123);
  CHECK(r.valveData.moves == 0);

  REQUIRE(parse(gvlvxLine(1, "9"), r) == ParseStatus::Ok);
  CHECK(r.valveEx.status == 9);
  CHECK(r.valveEx.calibrating);  // calState 2: running, bit 7 does not matter
  REQUIRE(parse(gvlvxLine(13, "0"), r) == ParseStatus::Ok);
  CHECK(r.valveEx.lastMove.dir == MoveDir::Open);
  REQUIRE(parse(gvlvxLine(16, "7"), r) == ParseStatus::Ok);
  CHECK(r.valveEx.lastMove.stop == StopReason::Aborted);
  REQUIRE(parse(gvlvxLine(16, "0"), r) == ParseStatus::Ok);
  CHECK(r.valveEx.lastMove.stop == StopReason::None);
  REQUIRE(parse(gvlvxLine(10, "0"), r) == ParseStatus::Ok);
  CHECK(r.valveEx.calState == 0);
}

TEST_CASE("codec: gvlvx calState is a bit field; calibrating follows it") {
  // Examples of software_stm32/PROTOCOL_V2.md (revamped STM 2.x).
  Reply r;
  REQUIRE(parse("gvlvx 3 9 0 30 17 3567 3610 43 2 12 8 1 4 1 65535 102 3 356 2140", r) ==
          ParseStatus::Ok);
  CHECK(r.valveEx.valve == 3);
  CHECK(r.valveEx.status == 9);
  CHECK(r.valveEx.calState == kCalStateIdle);
  CHECK(r.valveEx.calFlags == kCalFlagLastFailed);
  CHECK_FALSE(r.valveEx.calibrating);
  CHECK(r.valveEx.earlyStops == 1);

  struct Case {
    const char* status;
    const char* cal;
    uint8_t state;
    uint8_t flags;
    bool calibrating;
  };
  const Case cases[] = {
      {"1", "0", 0, 0, false},
      // requested by the time trigger / a valve found at start-up: bit 7 clear
      {"8", "1", 1, 0, false},
      // requested by staln or the movement trigger: bit 7 set
      {"129", "1", 1, 0, true},
      // running without bit 7 (time trigger, first target change)
      {"1", "2", 2, 0, true},
      {"1", "6", 2, kCalFlagEarlyStop, true},
      {"1", "10", 2, kCalFlagLastFailed, true},
      {"129", "14", 2, kCalFlagEarlyStop | kCalFlagLastFailed, true},
      {"1", "4", 0, kCalFlagEarlyStop, false},
      {"9", "12", 0, kCalFlagEarlyStop | kCalFlagLastFailed, false},
      {"1", "13", 1, kCalFlagEarlyStop | kCalFlagLastFailed, false},
      {"1", "15", 3, kCalFlagEarlyStop | kCalFlagLastFailed, false},
  };
  for (const Case& c : cases) {
    CAPTURE(c.status);
    CAPTURE(c.cal);
    std::string line = gvlvxLine(10, c.cal);
    line.replace(line.find(" 130 "), 5, std::string(" ") + c.status + " ");
    REQUIRE(parse(line, r) == ParseStatus::Ok);
    CHECK(r.valveEx.status == (atoi(c.status) & 0x7F));
    CHECK(r.valveEx.calState == c.state);
    CHECK(r.valveEx.calFlags == c.flags);
    CHECK(r.valveEx.calibrating == c.calibrating);
  }
  CHECK(parse(gvlvxLine(10, "16"), r) == ParseStatus::OutOfRange);
}

TEST_CASE("codec: gvlvx field ranges") {
  // Max accepted and first rejected value per field.
  const char* maxOk[19] = {"11",   "255",        "100",        "100",        "65535",
                           "4294967295", "4294967295", "2147483647", "255", "4294967295",
                           "15",   "4294967295", "4294967295", "1",          "4294967295",
                           "4294967295", "7", "65535", "4294967295"};
  const char* tooBig[19] = {"12",   "256",        "101",        "101",        "65536",
                            "4294967296", "4294967296", "2147483648", "256", "4294967296",
                            "16",   "4294967296", "4294967296", "2",          "4294967296",
                            "4294967296", "8", "65536", "4294967296"};
  for (int f = 0; f < 19; ++f) {
    CAPTURE(f);
    Reply r;
    CHECK(parse(gvlvxLine(f, maxOk[f]), r) == ParseStatus::Ok);
    dirty(r);
    CHECK(parse(gvlvxLine(f, tooBig[f]), r) == ParseStatus::OutOfRange);
    CHECK(isEmptyReply(r));
    CHECK(parse(gvlvxLine(f, f == 7 ? "1-" : "-1"), r) == ParseStatus::BadNumber);
  }
  Reply r;
  CHECK(parse(gvlvxLine(7, "-2147483648"), r) == ParseStatus::Ok);
  CHECK(r.valveEx.deadZone == INT32_MIN);
  CHECK(parse(gvlvxLine(7, "-2147483649"), r) == ParseStatus::OutOfRange);
  CHECK(parse(gvlvxLine() + " 0", r) == ParseStatus::BadArgCount);
  std::string eighteen = gvlvxLine();
  eighteen.resize(eighteen.rfind(' '));
  CHECK(parse(eighteen, r) == ParseStatus::BadArgCount);
}

TEST_CASE("codec: gprof") {
  Reply r;
  dirty(r);
  REQUIRE(parse("gprof 3 3 0:150 1500:212 3000:98 ", r) == ParseStatus::Ok);
  CHECK(r.cmd == Cmd::Gprof);
  CHECK(r.profile.valve == 3);
  CHECK(r.profile.count == 3);
  CHECK(r.profile.samples[0].count == 0);
  CHECK(r.profile.samples[0].current == 150);
  CHECK(r.profile.samples[1].count == 1500);
  CHECK(r.profile.samples[1].current == 212);
  CHECK(r.profile.samples[2].count == 3000);
  CHECK(r.profile.samples[2].current == 98);
  CHECK(r.profile.samples[3].count == 0);

  REQUIRE(parse("gprof 11 0", r) == ParseStatus::Ok);
  CHECK(r.profile.valve == 11);
  CHECK(r.profile.count == 0);

  std::string full = "gprof 0 32";
  for (int i = 0; i < 32; ++i) full += " " + std::to_string(i * 100) + ":" + std::to_string(i);
  REQUIRE(parse(full, r) == ParseStatus::Ok);
  CHECK(r.profile.count == 32);
  CHECK(r.profile.samples[31].count == 3100);
  CHECK(r.profile.samples[31].current == 31);
  REQUIRE(parse("gprof 0 1 4294967295:65535", r) == ParseStatus::Ok);
  CHECK(r.profile.samples[0].count == 4294967295u);
  CHECK(r.profile.samples[0].current == 65535);

  dirty(r);
  CHECK(parse("gprof 0 33" + std::string(33 * 4, ' '), r) == ParseStatus::OutOfRange);
  CHECK(isEmptyReply(r));
  CHECK(parse("gprof 12 0", r) == ParseStatus::OutOfRange);
  CHECK(parse("gprof 0 2 1:1", r) == ParseStatus::BadArgCount);
  CHECK(parse("gprof 0 1 1:1 2:2", r) == ParseStatus::BadArgCount);
  CHECK(parse("gprof 0", r) == ParseStatus::BadArgCount);
  CHECK(parse("gprof 0 1 11", r) == ParseStatus::BadFormat);
  CHECK(parse("gprof 0 1 :1", r) == ParseStatus::BadNumber);
  CHECK(parse("gprof 0 1 1:", r) == ParseStatus::BadNumber);
  CHECK(parse("gprof 0 1 1:2:3", r) == ParseStatus::BadNumber);
  CHECK(parse("gprof 0 1 1:65536", r) == ParseStatus::OutOfRange);
  CHECK(parse("gprof 0 1 4294967296:1", r) == ParseStatus::OutOfRange);
  CHECK(parse("gprof 0 1 -1:1", r) == ParseStatus::BadNumber);
  CHECK(parse("gprof 0 x", r) == ParseStatus::BadNumber);
}

TEST_CASE("codec: svmov") {
  Reply r;
  REQUIRE(parse("svmov 3 ok", r) == ParseStatus::Ok);
  CHECK(r.cmd == Cmd::Svmov);
  CHECK(r.serviceMove.index == 3);
  CHECK(r.serviceMove.ok);
  CHECK(r.serviceMove.errorCode == 0);
  REQUIRE(parse("svmov 11 err 2 ", r) == ParseStatus::Ok);
  CHECK(r.serviceMove.index == 11);
  CHECK_FALSE(r.serviceMove.ok);
  CHECK(r.serviceMove.errorCode == 2);
  REQUIRE(parse("svmov 0 err 65535", r) == ParseStatus::Ok);
  CHECK(r.serviceMove.errorCode == 65535);
  CHECK(parse("svmov 0 err 65536", r) == ParseStatus::OutOfRange);
  CHECK(parse("svmov 0 err", r) == ParseStatus::BadFormat);
  CHECK(parse("svmov 0 ok 1", r) == ParseStatus::BadFormat);
  CHECK(parse("svmov 0 fine", r) == ParseStatus::BadFormat);
  CHECK(parse("svmov 12 ok", r) == ParseStatus::OutOfRange);
  CHECK(parse("svmov 0", r) == ParseStatus::BadArgCount);
  CHECK(parse("svmov 0 err 1 2", r) == ParseStatus::BadArgCount);
  CHECK(parse("svmov x ok", r) == ParseStatus::BadNumber);
}

TEST_CASE("codec: gcalx") {
  Reply r;
  REQUIRE(parse("gcalx 1 10 40", r) == ParseStatus::Ok);
  CHECK(r.cmd == Cmd::Gcalx);
  CHECK(r.breakaway.enable);
  CHECK(r.breakaway.stepPct == 10);
  CHECK(r.breakaway.maxmA == 40);
  REQUIRE(parse("gcalx 0 0 20 ", r) == ParseStatus::Ok);
  CHECK_FALSE(r.breakaway.enable);
  CHECK(r.breakaway.stepPct == 0);
  CHECK(r.breakaway.maxmA == 20);
  REQUIRE(parse("gcalx 0 100 60", r) == ParseStatus::Ok);
  CHECK(r.breakaway.stepPct == 100);
  CHECK(r.breakaway.maxmA == 60);
  CHECK(parse("gcalx 2 0 20", r) == ParseStatus::OutOfRange);
  CHECK(parse("gcalx 0 101 20", r) == ParseStatus::OutOfRange);
  CHECK(parse("gcalx 0 0 19", r) == ParseStatus::OutOfRange);
  CHECK(parse("gcalx 0 0 61", r) == ParseStatus::OutOfRange);
  CHECK(parse("gcalx 0 0", r) == ParseStatus::BadArgCount);
  CHECK(parse("gcalx 0 0 20 1", r) == ParseStatus::BadArgCount);
}

TEST_CASE("codec: gstat") {
  Reply r;
  REQUIRE(parse("gstat 3600 2 4 17 5 1 ", r) == ParseStatus::Ok);
  CHECK(r.cmd == Cmd::Gstat);
  CHECK(r.status.uptimeS == 3600);
  CHECK(r.status.resets == 2);
  CHECK(r.status.bootReason == 4);
  CHECK(r.status.rxOverflow == 17);
  CHECK(r.status.parseErrors == 5);
  CHECK(r.status.eepState == 1);
  REQUIRE(parse("gstat 4294967295 4294967295 4294967295 4294967295 4294967295 255", r) ==
          ParseStatus::Ok);
  CHECK(r.status.uptimeS == 4294967295u);
  CHECK(r.status.eepState == 255);
  CHECK(parse("gstat 1 2 3 4 5 256", r) == ParseStatus::OutOfRange);
  CHECK(parse("gstat 4294967296 2 3 4 5 6", r) == ParseStatus::OutOfRange);
  CHECK(parse("gstat 1 2 3 4 5", r) == ParseStatus::BadArgCount);
  CHECK(parse("gstat 1 2 3 4 5 6 7", r) == ParseStatus::BadArgCount);
  REQUIRE(parse("gstat 3600 2 4 17 5 1", r) == ParseStatus::Ok);
  CHECK_FALSE(r.status.v3);
  CHECK(r.status.lease == LeaseState::Off);
  CHECK(r.status.sysFlags == 0);
}

// ================================================================ replies: protocol 3

namespace {

const char* const kGvlvyGolden =
    "gvlvy 3 9 50 30 17 3567 3610 43 2 0 8 1 4 0 1750 1750 1 262 6120 66 4 50 50 3540 0";
const char* const kGstaxGolden = "gstax 86400 3 2 0 2 0 1 3540 1 60 0 0 0 0 0 0 0 0 0 12 2 3600 0";

// The gvlvy golden with field `field` (1-based, after the command) replaced.
std::string gvlvyWith(size_t field, const std::string& value) {
  std::string line = kGvlvyGolden;
  size_t pos = 0;
  for (size_t i = 0; i < field; ++i) pos = line.find(' ', pos) + 1;
  const size_t end = line.find(' ', pos);
  return line.substr(0, pos) + value + (end == std::string::npos ? "" : line.substr(end));
}

// A valid "gstax" line: field i = i, except lease (7), leaseClient (9) and
// safeMode (12) = 0, then the given replacements.
std::string gstaxLine(std::initializer_list<std::pair<size_t, const char*>> repl,
                      size_t count = 23) {
  std::string line = "gstax";
  for (size_t i = 1; i <= count; ++i) {
    std::string v = (i == 7 || i == 9 || i == 12) ? "0" : std::to_string(i);
    for (const auto& p : repl) {
      if (p.first == i) v = p.second;
    }
    line += " " + v;
  }
  return line;
}

}  // namespace

TEST_CASE("codec: gvlvy golden") {
  Reply r;
  dirty(r);
  REQUIRE(parse(kGvlvyGolden, r) == ParseStatus::Ok);
  CHECK(r.cmd == Cmd::Gvlvy);
  const ValveEx& x = r.valveEx;
  CHECK(x.v3);
  CHECK(x.valve == 3);
  CHECK(x.status == 9);
  CHECK_FALSE(x.calibrating);
  CHECK(x.position == 50);
  CHECK(x.target == 30);
  CHECK(x.meanCurrent == 17);
  CHECK(x.openCount == 3567);
  CHECK(x.closeCount == 3610);
  CHECK(x.deadZone == 43);
  CHECK(x.calibRetries == 2);
  CHECK(x.moves == 0);
  CHECK(x.calState == kCalStateIdle);
  CHECK(x.calFlags == kCalFlagLastFailed);
  CHECK(x.earlyStops == 1);
  CHECK(x.cmdRejected == 4);
  CHECK(x.lastMove.dir == MoveDir::Open);
  CHECK(x.lastMove.requestedCounts == 1750);
  CHECK(x.lastMove.countedCounts == 1750);
  CHECK(x.lastMove.stop == StopReason::Target);
  CHECK(x.lastMove.peakCurrent == 262);
  CHECK(x.lastMove.durationMs == 6120);
  CHECK(x.flags == 66);
  CHECK(x.flags == (kStmFlagFsBlocked | kStmFlagRetry));
  CHECK(x.fault == static_cast<uint8_t>(ValveFault::StrokesTooShort));
  CHECK(x.fsPct == 50);
  CHECK(x.drive == 50);
  CHECK(x.retryS == 3540);
  CHECK(x.retries == 0);

  // Every v3 field at its upper limit, distinct values.
  REQUIRE(parse("gvlvy 11 2 1 2 3 4 5 -6 7 8 2 9 10 1 11 12 7 13 14 65535 255 255 100 4294967295 "
                "255 ",
                r) == ParseStatus::Ok);
  CHECK(r.valveEx.flags == 65535);
  CHECK(r.valveEx.fault == 255);
  CHECK(r.valveEx.fsPct == kFailsafeHold);
  CHECK(r.valveEx.drive == 100);
  CHECK(r.valveEx.retryS == 4294967295u);
  CHECK(r.valveEx.retries == 255);
  CHECK(r.valveEx.calibrating);  // calState running
  REQUIRE(parse(gvlvyWith(22, "0"), r) == ParseStatus::Ok);
  CHECK(r.valveEx.fsPct == 0);
  REQUIRE(parse(gvlvyWith(22, "100"), r) == ParseStatus::Ok);
  CHECK(r.valveEx.fsPct == 100);
  REQUIRE(parse(gvlvyWith(23, "0"), r) == ParseStatus::Ok);
  CHECK(r.valveEx.drive == 0);

  // Field count: at least 25; numeric extras are ignored.
  const std::string golden = kGvlvyGolden;
  CHECK(parse(golden.substr(0, golden.rfind(' ')), r) == ParseStatus::BadArgCount);
  CHECK(isEmptyReply(r));
  REQUIRE(parse(golden + " 7", r) == ParseStatus::Ok);
  CHECK(r.valveEx.retryS == 3540);
  CHECK(r.valveEx.retries == 0);
  REQUIRE(parse(golden + " 0 4294967295", r) == ParseStatus::Ok);
  CHECK(parse(golden + " x", r) == ParseStatus::BadNumber);
  CHECK(parse(golden + " 7 -1", r) == ParseStatus::BadNumber);
  CHECK(parse(golden + " 4294967296", r) == ParseStatus::OutOfRange);

  // Ranges of the v3 fields.
  CHECK(parse(gvlvyWith(20, "65536"), r) == ParseStatus::OutOfRange);
  CHECK(parse(gvlvyWith(21, "256"), r) == ParseStatus::OutOfRange);
  CHECK(parse(gvlvyWith(22, "101"), r) == ParseStatus::OutOfRange);
  CHECK(parse(gvlvyWith(22, "254"), r) == ParseStatus::OutOfRange);
  CHECK(parse(gvlvyWith(22, "256"), r) == ParseStatus::OutOfRange);
  REQUIRE(parse(gvlvyWith(22, "255"), r) == ParseStatus::Ok);
  CHECK(r.valveEx.fsPct == 255);
  CHECK(parse(gvlvyWith(23, "101"), r) == ParseStatus::OutOfRange);
  CHECK(parse(gvlvyWith(24, "4294967296"), r) == ParseStatus::OutOfRange);
  CHECK(parse(gvlvyWith(25, "256"), r) == ParseStatus::OutOfRange);
  CHECK(parse(gvlvyWith(20, "x"), r) == ParseStatus::BadNumber);
  CHECK(parse(gvlvyWith(22, "-1"), r) == ParseStatus::BadNumber);
  // The 19 gvlvx fields keep their rules.
  CHECK(parse(gvlvyWith(1, "12"), r) == ParseStatus::OutOfRange);
  CHECK(parse(gvlvyWith(3, "101"), r) == ParseStatus::OutOfRange);
  CHECK(parse(gvlvyWith(11, "16"), r) == ParseStatus::OutOfRange);
  CHECK(parse(gvlvyWith(19, "x"), r) == ParseStatus::BadNumber);
}

TEST_CASE("codec: gvlvx stays at exactly 19 fields without v3 values") {
  Reply r;
  const std::string gvlvx = "gvlvx 4 130 42 60 21 3120 3350 -230 1 57 2 7 3 1 3000 1450 3 412 8123";
  REQUIRE(parse(gvlvx, r) == ParseStatus::Ok);
  CHECK_FALSE(r.valveEx.v3);
  CHECK(r.valveEx.flags == 0);
  CHECK(r.valveEx.fault == 0);
  CHECK(r.valveEx.fsPct == kFailsafeHold);
  CHECK(r.valveEx.drive == 0);
  CHECK(r.valveEx.retryS == 0);
  CHECK(r.valveEx.retries == 0);
  CHECK(parse(gvlvx + " 1", r) == ParseStatus::BadArgCount);
  CHECK(parse(std::string(kGvlvyGolden).replace(0, 5, "gvlvx"), r) == ParseStatus::BadArgCount);
}

TEST_CASE("codec: gstax golden with 23 values") {
  Reply r;
  dirty(r);
  REQUIRE(parse(kGstaxGolden, r) == ParseStatus::Ok);
  CHECK(r.cmd == Cmd::Gstax);
  const StmStatus& s = r.status;
  CHECK(s.v3);
  CHECK(s.uptimeS == 86400);
  CHECK(s.resets == 3);
  CHECK(s.bootReason == 2);
  CHECK(s.rxOverflow == 0);
  CHECK(s.parseErrors == 2);
  CHECK(s.eepState == 0);
  CHECK(s.lease == LeaseState::Running);
  CHECK(s.leaseRemainS == 3540);
  CHECK(s.leaseClient);
  CHECK(s.leaseTimeoutMin == 60);
  CHECK(s.failsafeMask == 0);
  CHECK_FALSE(s.safeMode);
  CHECK(s.wdgResets == 0);
  CHECK(s.uartOre == 0);
  CHECK(s.uartFe == 0);
  CHECK(s.uartNe == 0);
  CHECK(s.rxDropped == 0);
  CHECK(s.cfgFlags == 0);
  CHECK(s.cfgEvents == 0);
  CHECK(s.eepWrites == 12);
  CHECK(s.tempAgeS == 2);
  CHECK(s.owScanAgeS == 3600);
  CHECK(s.sysFlags == 0);

  // Distinct values in every field (catches swapped fields).
  REQUIRE(parse(gstaxLine({{6, "6"}, {7, "2"}, {9, "1"}, {10, "1440"}, {11, "4095"}, {12, "1"},
                           {18, "255"}, {23, "255"}}),
                r) == ParseStatus::Ok);
  CHECK(r.status.uptimeS == 1);
  CHECK(r.status.resets == 2);
  CHECK(r.status.bootReason == 3);
  CHECK(r.status.rxOverflow == 4);
  CHECK(r.status.parseErrors == 5);
  CHECK(r.status.eepState == 6);
  CHECK(r.status.lease == LeaseState::Expired);
  CHECK(r.status.leaseRemainS == 8);
  CHECK(r.status.leaseClient);
  CHECK(r.status.leaseTimeoutMin == 1440);
  CHECK(r.status.failsafeMask == 4095);
  CHECK(r.status.safeMode);
  CHECK(r.status.wdgResets == 13);
  CHECK(r.status.uartOre == 14);
  CHECK(r.status.uartFe == 15);
  CHECK(r.status.uartNe == 16);
  CHECK(r.status.rxDropped == 17);
  CHECK(r.status.cfgFlags == 255);
  CHECK(r.status.cfgEvents == 19);
  CHECK(r.status.eepWrites == 20);
  CHECK(r.status.tempAgeS == 21);
  CHECK(r.status.owScanAgeS == 22);
  CHECK(r.status.sysFlags == 255);
  REQUIRE(parse(gstaxLine({}), r) == ParseStatus::Ok);
  CHECK(r.status.lease == LeaseState::Off);
  CHECK_FALSE(r.status.leaseClient);
  CHECK_FALSE(r.status.safeMode);
  CHECK(r.status.wdgResets == 13);
  REQUIRE(parse(gstaxLine({{13, "255"}, {10, "5"}, {11, "0"}}), r) == ParseStatus::Ok);
  CHECK(r.status.wdgResets == 255);
  CHECK(r.status.leaseTimeoutMin == 5);
  CHECK(r.status.failsafeMask == 0);
  REQUIRE(parse(gstaxLine({{7, "1"}}), r) == ParseStatus::Ok);
  CHECK(r.status.lease == LeaseState::Running);

  // Field ranges (every other field valid).
  CHECK(parse(gstaxLine({{7, "3"}}), r) == ParseStatus::OutOfRange);
  CHECK(parse(gstaxLine({{9, "2"}}), r) == ParseStatus::OutOfRange);
  CHECK(parse(gstaxLine({{10, "1441"}}), r) == ParseStatus::OutOfRange);
  CHECK(parse(gstaxLine({{11, "4096"}}), r) == ParseStatus::OutOfRange);
  CHECK(parse(gstaxLine({{12, "2"}}), r) == ParseStatus::OutOfRange);
  CHECK(parse(gstaxLine({{13, "256"}}), r) == ParseStatus::OutOfRange);
  CHECK(parse(gstaxLine({{18, "256"}}), r) == ParseStatus::OutOfRange);
  CHECK(parse(gstaxLine({{23, "256"}}), r) == ParseStatus::OutOfRange);
  CHECK(parse(gstaxLine({{8, "4294967296"}}), r) == ParseStatus::OutOfRange);
  CHECK(parse(gstaxLine({{14, "4294967296"}}), r) == ParseStatus::OutOfRange);
  CHECK(parse(gstaxLine({{17, "4294967296"}}), r) == ParseStatus::OutOfRange);
  CHECK(parse(gstaxLine({{19, "4294967296"}}), r) == ParseStatus::OutOfRange);
  CHECK(parse(gstaxLine({{22, "4294967296"}}), r) == ParseStatus::OutOfRange);
  CHECK(parse(gstaxLine({{22, "x"}}), r) == ParseStatus::BadNumber);
  CHECK(parse(gstaxLine({{6, "256"}}), r) == ParseStatus::OutOfRange);  // gstat field 6
  CHECK(isEmptyReply(r));

  // Field count: at least 23; numeric extras are ignored.
  const std::string golden = kGstaxGolden;
  CHECK(parse(golden.substr(0, golden.rfind(' ')), r) == ParseStatus::BadArgCount);  // 22
  CHECK(parse("gstax 86400 3 2 0 2 0 1 3540 1 60 0 0 0 0 0 0 0 0 0 12 2", r) ==
        ParseStatus::BadArgCount);  // 21
  REQUIRE(parse(golden + " 99", r) == ParseStatus::Ok);
  CHECK(r.status.sysFlags == 0);
  CHECK(parse(golden + " x", r) == ParseStatus::BadNumber);
  CHECK(parse(golden + " 4294967296", r) == ParseStatus::OutOfRange);
  // gstat stays at exactly 6.
  CHECK(parse("gstat 86400 3 2 0 2 0 1", r) == ParseStatus::BadArgCount);
}

TEST_CASE("codec: glcfg") {
  Reply r;
  dirty(r);
  REQUIRE(parse("glcfg 1440 0 1 2 3 4 5 6 7 8 9 100 255", r) == ParseStatus::Ok);
  CHECK(r.cmd == Cmd::Glcfg);
  CHECK(r.leaseConfig.timeoutMin == 1440);
  for (uint8_t i = 0; i < 10; ++i) CHECK(r.leaseConfig.failsafePct[i] == i);
  CHECK(r.leaseConfig.failsafePct[10] == 100);
  CHECK(r.leaseConfig.failsafePct[11] == kFailsafeHold);
  REQUIRE(parse("glcfg 0 50 50 50 50 50 50 50 50 50 50 50 50 ", r) == ParseStatus::Ok);
  CHECK(r.leaseConfig.timeoutMin == 0);
  CHECK(r.leaseConfig.failsafePct[0] == 50);
  REQUIRE(parse("glcfg 60 50 50 50 50 50 50 50 50 50 50 50 50 1 2", r) == ParseStatus::Ok);
  CHECK(r.leaseConfig.timeoutMin == 60);
  CHECK(parse("glcfg 60 50 50 50 50 50 50 50 50 50 50 50", r) == ParseStatus::BadArgCount);
  CHECK(parse("glcfg", r) == ParseStatus::BadArgCount);
  CHECK(parse("glcfg 1441 50 50 50 50 50 50 50 50 50 50 50 50", r) == ParseStatus::OutOfRange);
  CHECK(parse("glcfg 60 101 50 50 50 50 50 50 50 50 50 50 50", r) == ParseStatus::OutOfRange);
  CHECK(parse("glcfg 60 50 50 50 50 50 50 50 50 50 50 50 254", r) == ParseStatus::OutOfRange);
  CHECK(parse("glcfg 60 50 50 50 50 50 50 50 50 50 50 50 256", r) == ParseStatus::OutOfRange);
  CHECK(parse("glcfg 60 50 50 50 50 50 50 50 50 50 50 50 x", r) == ParseStatus::BadNumber);
  CHECK(parse("glcfg x 50 50 50 50 50 50 50 50 50 50 50 50", r) == ParseStatus::BadNumber);
  CHECK(parse("glcfg 60 50 50 50 50 50 50 50 50 50 50 50 50 x", r) == ParseStatus::BadNumber);
  CHECK(isEmptyReply(r));
}

TEST_CASE("codec: slhbt ok and error forms") {
  Reply r;
  dirty(r);
  REQUIRE(parse("slhbt 1 3540", r) == ParseStatus::Ok);
  CHECK(r.cmd == Cmd::Slhbt);
  CHECK(r.heartbeat.lease == LeaseState::Running);
  CHECK(r.heartbeat.remainS == 3540);
  CHECK_FALSE(r.ack.error);
  REQUIRE(parse("slhbt 2 0 ", r) == ParseStatus::Ok);
  CHECK(r.heartbeat.lease == LeaseState::Expired);
  REQUIRE(parse("slhbt 0 4294967295", r) == ParseStatus::Ok);
  CHECK(r.heartbeat.lease == LeaseState::Off);
  CHECK(r.heartbeat.remainS == 4294967295u);
  REQUIRE(parse("slhbt 1 5 7", r) == ParseStatus::Ok);  // extra field
  CHECK(r.heartbeat.remainS == 5);
  REQUIRE(parse("slhbt err", r) == ParseStatus::Ok);
  CHECK(r.cmd == Cmd::Slhbt);
  CHECK(r.ack.error);
  CHECK(r.heartbeat.lease == LeaseState::Off);
  CHECK(parse("slhbt ok", r) == ParseStatus::BadFormat);
  CHECK(parse("slhbt 1", r) == ParseStatus::BadFormat);
  CHECK(parse("slhbt", r) == ParseStatus::BadArgCount);
  CHECK(parse("slhbt 3 0", r) == ParseStatus::OutOfRange);
  CHECK(parse("slhbt 1 4294967296", r) == ParseStatus::OutOfRange);
  CHECK(parse("slhbt 1 5 x", r) == ParseStatus::BadNumber);
  CHECK(parse("slhbt err 1", r) == ParseStatus::BadNumber);
}

TEST_CASE("codec: slcfg and ssafe ok/err forms") {
  for (const char* cmd : {"slcfg", "ssafe"}) {
    CAPTURE(cmd);
    const std::string c = cmd;
    Reply r;
    REQUIRE(parse(c + " ok", r) == ParseStatus::Ok);
    CHECK(r.cmd == cmdFromName(cmd, 5));
    CHECK_FALSE(r.ack.error);
    REQUIRE(parse(c + " err ", r) == ParseStatus::Ok);
    CHECK(r.ack.error);
    CHECK(parse(c, r) == ParseStatus::BadArgCount);
    CHECK(parse(c + " ok 1", r) == ParseStatus::BadArgCount);
    CHECK(parse(c + " fine", r) == ParseStatus::BadFormat);
  }
}

TEST_CASE("codec: sfspo and sstop indexed results") {
  for (const char* cmd : {"sfspo", "sstop"}) {
    CAPTURE(cmd);
    const std::string c = cmd;
    const bool isStop = c == "sstop";
    Reply r;
    auto res = [&]() -> const IndexedResult& { return isStop ? r.stop : r.failsafe; };
    REQUIRE(parse(c + " 3 ok", r) == ParseStatus::Ok);
    CHECK(r.cmd == cmdFromName(cmd, 5));
    CHECK(res().index == 3);
    CHECK(res().ok);
    CHECK(res().errorCode == 0);
    REQUIRE(parse(c + " 0 ok", r) == ParseStatus::Ok);
    CHECK(res().index == 0);
    REQUIRE(parse(c + " 11 ok", r) == ParseStatus::Ok);
    CHECK(res().index == 11);
    REQUIRE(parse(c + " 255 ok ", r) == ParseStatus::Ok);
    CHECK(res().index == 255);
    CHECK(res().ok);
    REQUIRE(parse(c + " -1 err 1", r) == ParseStatus::Ok);
    CHECK(res().index == -1);
    CHECK_FALSE(res().ok);
    CHECK(res().errorCode == 1);
    REQUIRE(parse(c + " 7 err 65535", r) == ParseStatus::Ok);
    CHECK(res().index == 7);
    CHECK(res().errorCode == 65535);
    CHECK(parse(c + " -1 ok", r) == ParseStatus::BadFormat);
    CHECK(parse(c + " 12 ok", r) == ParseStatus::OutOfRange);
    CHECK(parse(c + " 254 ok", r) == ParseStatus::OutOfRange);
    CHECK(parse(c + " 256 ok", r) == ParseStatus::OutOfRange);
    CHECK(parse(c + " -2 err 1", r) == ParseStatus::OutOfRange);
    CHECK(parse(c + " 3 err 65536", r) == ParseStatus::OutOfRange);
    CHECK(parse(c + " 3 err", r) == ParseStatus::BadFormat);
    CHECK(parse(c + " 3 ok 1", r) == ParseStatus::BadFormat);
    CHECK(parse(c + " 3", r) == ParseStatus::BadArgCount);
    CHECK(parse(c + " 3 err 1 2", r) == ParseStatus::BadArgCount);
    CHECK(parse(c + " x ok", r) == ParseStatus::BadNumber);
    CHECK(isEmptyReply(r));
  }
}

TEST_CASE("codec: svmov signed index") {
  Reply r;
  REQUIRE(parse("svmov -1 err 1", r) == ParseStatus::Ok);
  CHECK(r.cmd == Cmd::Svmov);
  CHECK(r.serviceMove.index == -1);
  CHECK_FALSE(r.serviceMove.ok);
  CHECK(r.serviceMove.errorCode == 1);
  REQUIRE(parse("svmov 3 ok", r) == ParseStatus::Ok);
  CHECK(r.serviceMove.index == 3);
  REQUIRE(parse("svmov 0 ok", r) == ParseStatus::Ok);
  CHECK(r.serviceMove.index == 0);
  CHECK(r.serviceMove.ok);
  CHECK(parse("svmov -2 err 1", r) == ParseStatus::OutOfRange);
  CHECK(parse("svmov 12 ok", r) == ParseStatus::OutOfRange);
  CHECK(parse("svmov 255 ok", r) == ParseStatus::OutOfRange);
  CHECK(parse("svmov -1 ok", r) == ParseStatus::BadFormat);
  CHECK(parse("sfspo 255 ok", r) == ParseStatus::Ok);
  CHECK(r.failsafe.index == 255);
  CHECK(parse("sfspo 12 ok", r) == ParseStatus::OutOfRange);
}

TEST_CASE("codec: gtlnt and stlnt") {
  Reply r;
  dirty(r);
  REQUIRE(parse("gtlnt 604800", r) == ParseStatus::Ok);
  CHECK(r.cmd == Cmd::Gtlnt);
  CHECK(r.learnTime == 604800);
  REQUIRE(parse("gtlnt 0 ", r) == ParseStatus::Ok);
  CHECK(r.learnTime == 0);
  REQUIRE(parse("gtlnt 4294967295", r) == ParseStatus::Ok);
  CHECK(r.learnTime == 4294967295u);
  CHECK(parse("gtlnt 4294967296", r) == ParseStatus::OutOfRange);
  CHECK(parse("gtlnt", r) == ParseStatus::BadArgCount);
  CHECK(parse("gtlnt 1 2", r) == ParseStatus::BadArgCount);
  REQUIRE(parse("stlnt", r) == ParseStatus::Ok);
  CHECK(r.cmd == Cmd::Stlnt);
  CHECK_FALSE(r.ack.error);
  REQUIRE(parse("stlnt ", r) == ParseStatus::Ok);
  CHECK(parse("stlnt 5", r) == ParseStatus::BadArgCount);
}

TEST_CASE("codec: v3 flag, fault and configuration flag names") {
  const uint16_t flags[] = {kStmFlagFsLease,    kStmFlagFsBlocked,   kStmFlagUncalibrated,
                            kStmFlagNeedsRef,   kStmFlagRecal,       kStmFlagCalRestored,
                            kStmFlagRetry,      kStmFlagEarlyPending, kStmFlagAssembly,
                            kStmFlagSvcHold};
  const char* flagNames[] = {"fsLease", "fsBlocked",    "uncalibrated", "needsRef", "recal",
                             "calRestored", "retry",    "earlyPending", "assembly", "svcHold"};
  for (uint8_t b = 0; b < 10; ++b) {
    CAPTURE(b);
    CHECK(flags[b] == (1u << b));
    CHECK(std::string(stmFlagName(b)) == flagNames[b]);
  }
  for (uint8_t b : {10, 11, 15, 16, 255}) CHECK(std::string(stmFlagName(b)) == "");

  const char* faults[] = {"none", "move_timeout", "stroke_timeout", "short", "strokes_too_short",
                          "inrush_trip"};
  for (uint8_t f = 0; f < 6; ++f) CHECK(std::string(valveFaultName(f)) == faults[f]);
  CHECK(static_cast<uint8_t>(ValveFault::InrushTrip) == 5);
  CHECK(static_cast<uint8_t>(ValveFault::StrokesTooShort) == 4);
  for (uint8_t f : {6, 7, 255}) CHECK(std::string(valveFaultName(f)) == "unknown");

  const uint8_t cfg[] = {kStmCfgLayoutCrc,    kStmCfgShadowMissing, kStmCfgSettingsCorrupt,
                         kStmCfgSafetyCorrupt, kStmCfgSensorSlot,   kStmCfgCalib,
                         kStmCfgUnverified,   kStmCfgReadFailed};
  const char* cfgNames[] = {"layoutCrc",  "shadowMissing", "settingsCorrupt", "safetyCorrupt",
                            "sensorSlot", "calib",         "unverified",      "readFailed"};
  for (uint8_t b = 0; b < 8; ++b) {
    CAPTURE(b);
    CHECK(cfg[b] == (1u << b));
    CHECK(std::string(stmCfgFlagName(b)) == cfgNames[b]);
  }
  for (uint8_t b : {8, 9, 255}) CHECK(std::string(stmCfgFlagName(b)) == "");
  CHECK(kStmSysProtectSuspended == 0x01);
}

// ================================================================ matching

namespace {
Reply parsed(const std::string& s) {
  Reply r;
  REQUIRE(parse(s, r) == ParseStatus::Ok);
  return r;
}
}  // namespace

TEST_CASE("codec: replyMatches by command and valve") {
  RequestLine q;
  REQUIRE(buildValveData(3, q));
  CHECK(replyMatches(q, parsed("gvlvd 3 42 18 1 215 -500 57 3120 3350 230 0")));
  CHECK_FALSE(replyMatches(q, parsed("gvlvd 4 42 18 1 215 -500 57 3120 3350 230 0")));
  CHECK_FALSE(replyMatches(q, parsed("gtgtp 3 50")));

  REQUIRE(buildValveEx(4, q));
  CHECK(replyMatches(q, parsed(gvlvxLine())));
  REQUIRE(buildValveEx(5, q));
  CHECK_FALSE(replyMatches(q, parsed(gvlvxLine())));

  REQUIRE(buildGetTarget(3, q));
  CHECK(replyMatches(q, parsed("gtgtp 3 50")));
  CHECK_FALSE(replyMatches(q, parsed("gtgtp 2 50")));

  REQUIRE(buildProfile(3, q));
  CHECK(replyMatches(q, parsed("gprof 3 0")));
  CHECK_FALSE(replyMatches(q, parsed("gprof 2 0")));

  REQUIRE(buildServiceMove(3, MoveDir::Open, 10, 10, q));
  CHECK(replyMatches(q, parsed("svmov 3 ok")));
  CHECK(replyMatches(q, parsed("svmov 3 err 1")));
  CHECK_FALSE(replyMatches(q, parsed("svmov 2 ok")));

  REQUIRE(buildSetValveSensors(7, OneWireId{}, OneWireId{}, q));
  CHECK(replyMatches(q, parsed("stvls 7")));
  CHECK_FALSE(replyMatches(q, parsed("stvls 6")));

  REQUIRE(buildSetTarget(3, 50, q));
  CHECK(replyMatches(q, parsed("stgtp")));
  CHECK_FALSE(replyMatches(q, parsed("staln")));

  REQUIRE(buildCalibrate(kAllValves, q));
  CHECK(replyMatches(q, parsed("staln")));
  REQUIRE(buildSetMotorChars(MotorChars{}, q));
  CHECK(replyMatches(q, parsed("smotc")));
  CHECK(replyMatches(q, parsed("smotc err")));
  REQUIRE(buildGetProto(q));
  CHECK(replyMatches(q, parsed("gproto 2")));
  CHECK_FALSE(replyMatches(q, parsed("gstat 1 2 3 4 5 6")));
}

TEST_CASE("codec: replyMatches for gvlon forms and error") {
  RequestLine one, all;
  REQUIRE(buildValveSensors(3, one));
  REQUIRE(buildValveSensors(kAllValves, all));
  const Reply single = parsed(std::string("gvlon 3 ") + kId1 + " " + kId2);
  const Reply other = parsed(std::string("gvlon 4 ") + kId1 + " " + kId2);
  const Reply list = parsed("gvlon 12 " + idList(kId1, 24, true));
  const Reply err = parsed("goned error");
  CHECK(replyMatches(one, single));
  CHECK_FALSE(replyMatches(one, other));
  CHECK_FALSE(replyMatches(one, list));
  CHECK(replyMatches(one, err));
  CHECK(replyMatches(all, list));
  CHECK_FALSE(replyMatches(all, single));
  CHECK(replyMatches(all, err));
  RequestLine goned;
  REQUIRE(buildTempData(0, goned));
  CHECK_FALSE(replyMatches(goned, err));  // the error form belongs to gvlon
}

TEST_CASE("codec: replyMatches for sensor count/list/data") {
  RequestLine count, list, vcount, vlist, data, vdata;
  REQUIRE(buildTempCount(count));
  REQUIRE(buildTempList(list));
  REQUIRE(buildVoltCount(vcount));
  REQUIRE(buildVoltList(vlist));
  REQUIRE(buildTempData(5, data));
  REQUIRE(buildVoltData(5, vdata));
  const Reply c0 = parsed("gonec 0");
  const Reply c3 = parsed("gonec 3");
  const Reply l1 = parsed(std::string("gonec 1 ") + kId1);
  CHECK(replyMatches(count, c0));
  CHECK(replyMatches(count, c3));
  CHECK_FALSE(replyMatches(count, l1));
  CHECK(replyMatches(list, c0));  // empty list looks like the count reply
  CHECK_FALSE(replyMatches(list, c3));
  CHECK(replyMatches(list, l1));
  CHECK_FALSE(replyMatches(vlist, l1));
  CHECK(replyMatches(vlist, parsed(std::string("gowvc 1 ") + kId3)));
  CHECK(replyMatches(vlist, parsed("gowvc 0")));
  CHECK_FALSE(replyMatches(vlist, parsed("gowvc 2")));
  CHECK(replyMatches(vcount, parsed("gowvc 2")));
  CHECK(replyMatches(data, parsed(std::string("goned ") + kId1 + " 215")));
  CHECK(replyMatches(data, parsed("goned 0")));
  CHECK_FALSE(replyMatches(data, parsed("gowvd 0")));
  CHECK(replyMatches(vdata, parsed("gowvd 0")));
  CHECK(replyMatches(vdata, parsed(std::string("gowvd ") + kId3 + " 12")));
}

TEST_CASE("codec: replyMatches with an expected sensor id") {
  RequestLine data, vdata;
  REQUIRE(buildTempData(4, data));
  REQUIRE(buildVoltData(4, vdata));
  const Reply x = parsed(std::string("goned ") + kId1 + " 215");
  const Reply y = parsed(std::string("goned ") + kId2 + " 215");
  const Reply vx = parsed(std::string("gowvd ") + kId3 + " 12");
  const Reply vy = parsed(std::string("gowvd ") + kId1 + " 12");
  // Expect zero (unknown id at that bus index): any reading matches.
  CHECK(replyMatches(data, x));
  CHECK(replyMatches(data, y));
  CHECK(replyMatches(vdata, vx));
  CHECK(replyMatches(vdata, vy));
  data.expect = id(kId1);
  vdata.expect = id(kId3);
  CHECK(replyMatches(data, x));
  CHECK_FALSE(replyMatches(data, y));       // a late reply for another bus index
  CHECK(replyMatches(data, parsed("goned 0")));  // the invalid form: completes as rejected
  CHECK(replyMatches(vdata, vx));
  CHECK_FALSE(replyMatches(vdata, vy));
  CHECK(replyMatches(vdata, parsed("gowvd 0")));
  CHECK_FALSE(replyMatches(data, vx));      // other command
}

TEST_CASE("codec: replyMatches for v3 and indexed replies") {
  RequestLine q;
  REQUIRE(buildValveExV3(3, q));
  CHECK(replyMatches(q, parsed(kGvlvyGolden)));
  REQUIRE(buildValveExV3(4, q));
  CHECK_FALSE(replyMatches(q, parsed(kGvlvyGolden)));
  REQUIRE(buildValveEx(3, q));
  CHECK_FALSE(replyMatches(q, parsed(kGvlvyGolden)));  // gvlvx request, gvlvy reply

  REQUIRE(buildServiceMove(3, MoveDir::Open, 10, 10, q));
  CHECK(replyMatches(q, parsed("svmov -1 err 1")));  // the STM could not read the index
  CHECK_FALSE(replyMatches(q, parsed("svmov 4 ok")));
  CHECK_FALSE(replyMatches(q, parsed("svmov 0 ok")));

  REQUIRE(buildSetFailsafe(2, 50, q));
  CHECK(replyMatches(q, parsed("sfspo 2 ok")));
  CHECK(replyMatches(q, parsed("sfspo 2 err 1")));
  CHECK(replyMatches(q, parsed("sfspo -1 err 1")));
  CHECK_FALSE(replyMatches(q, parsed("sfspo 3 ok")));
  CHECK_FALSE(replyMatches(q, parsed("sfspo 0 ok")));
  CHECK_FALSE(replyMatches(q, parsed("sfspo 255 ok")));
  CHECK_FALSE(replyMatches(q, parsed("sstop 2 ok")));
  REQUIRE(buildSetFailsafe(kAllValves, 50, q));
  CHECK(replyMatches(q, parsed("sfspo 255 ok")));
  CHECK_FALSE(replyMatches(q, parsed("sfspo 2 ok")));

  REQUIRE(buildStop(5, q));
  CHECK(replyMatches(q, parsed("sstop 5 ok")));
  CHECK(replyMatches(q, parsed("sstop -1 err 1")));
  CHECK_FALSE(replyMatches(q, parsed("sstop 6 ok")));
  CHECK_FALSE(replyMatches(q, parsed("sstop 0 ok")));
  REQUIRE(buildStop(kAllValves, q));
  CHECK(replyMatches(q, parsed("sstop 255 ok")));

  REQUIRE(buildGetStatusV3(q));
  CHECK(replyMatches(q, parsed(kGstaxGolden)));
  CHECK_FALSE(replyMatches(q, parsed("gstat 1 2 3 4 5 6")));
  REQUIRE(buildHeartbeat(true, q));
  CHECK(replyMatches(q, parsed("slhbt 1 3540")));
  CHECK(replyMatches(q, parsed("slhbt err")));
  REQUIRE(buildGetLeaseConfig(q));
  CHECK(replyMatches(q, parsed("glcfg 60 50 50 50 50 50 50 50 50 50 50 50 50")));
  REQUIRE(buildSetLeaseTimeout(60, q));
  CHECK(replyMatches(q, parsed("slcfg ok")));
  REQUIRE(buildGetLearnTime(q));
  CHECK(replyMatches(q, parsed("gtlnt 0")));
  REQUIRE(buildLeaveSafeMode(q));
  CHECK(replyMatches(q, parsed("ssafe err")));
  REQUIRE(buildSetLearnTime(0, q));
  CHECK(replyMatches(q, parsed("stlnt")));
}

TEST_CASE("codec: replyMatches rejects empty requests and replies") {
  RequestLine none;
  Reply empty;
  CHECK_FALSE(replyMatches(none, empty));
  RequestLine q;
  REQUIRE(buildGetStatus(q));
  CHECK_FALSE(replyMatches(q, empty));
  RequestLine noLen = q;
  noLen.len = 0;
  CHECK_FALSE(replyMatches(noLen, parsed("gstat 1 2 3 4 5 6")));
  CHECK(replyMatches(q, parsed("gstat 1 2 3 4 5 6")));
}

TEST_CASE("codec: every request built has a matching golden reply") {
  RequestLine q[32];
  const char* replies[32];
  size_t n = 0;
  auto add = [&](bool ok, const char* rep) {
    REQUIRE(ok);
    replies[n++] = rep;
  };
  add(buildSetTarget(1, 2, q[n]), "stgtp");
  add(buildScanOneWire(q[n]), "stons");
  add(buildMatchSensors(q[n]), "masns ");
  add(buildAssembly(255, q[n]), "staop ");
  add(buildDetect(q[n]), "stdet ");
  add(buildSetLearnMovements(2000, q[n]), "stlnm");
  add(buildGetLearnMovements(q[n]), "gtlnm 2000 ");
  add(buildGetMotorChars(q[n]), "gmotc 17 17 50 3000 0 ");
  add(buildGetVersion(q[n]), "gvers 1.4.9_C2 1 ");
  add(buildGetHwId(q[n]), "ghwin 1073 ");
  add(buildEepromState(q[n]), "eepst 1 ");
  add(buildSoftReset(q[n]), "reset ");
  add(buildValveStates(q[n]), "gvlst 12 8,6,6,8,6,6,6,6,6,6,6,6, ");
  add(buildGetBreakaway(q[n]), "gcalx 1 10 40");
  Breakaway b;
  add(buildSetBreakaway(b, q[n]), "scalx ok");
  add(buildValveExV3(3, q[n]), kGvlvyGolden);
  add(buildGetStatusV3(q[n]), kGstaxGolden);
  add(buildHeartbeat(false, q[n]), "slhbt 2 0");
  add(buildSetLeaseTimeout(60, q[n]), "slcfg ok");
  add(buildSetFailsafe(3, 50, q[n]), "sfspo 3 ok");
  add(buildGetLeaseConfig(q[n]), "glcfg 60 50 50 50 50 50 50 50 50 50 50 50 50");
  add(buildStop(255, q[n]), "sstop 255 ok");
  add(buildGetLearnTime(q[n]), "gtlnt 604800");
  add(buildLeaveSafeMode(q[n]), "ssafe ok");
  add(buildSetLearnTime(0, q[n]), "stlnt");
  for (size_t i = 0; i < n; ++i) {
    CAPTURE(replies[i]);
    CHECK(replyMatches(q[i], parsed(replies[i])));
  }
}

// ================================================================ helpers

TEST_CASE("codec: resolveTempSlot") {
  OneWireId slots[kTempSlotCount];
  slots[4] = id(kId1);
  slots[33] = id(kId2);
  CHECK(resolveTempSlot(id(kId1), slots, kTempSlotCount) == 5);
  CHECK(resolveTempSlot(id(kId2), slots, kTempSlotCount) == 34);
  CHECK(resolveTempSlot(id(kId2), slots, 33) == 0);  // beyond slotCount
  CHECK(resolveTempSlot(id(kId3), slots, kTempSlotCount) == 0);
  CHECK(resolveTempSlot(OneWireId{}, slots, kTempSlotCount) == 0);  // zero never matches empty
  OneWireId badCrc = id(kId1);
  badCrc.b[7] ^= 0x80;
  slots[0] = badCrc;
  CHECK(resolveTempSlot(badCrc, slots, kTempSlotCount) == 0);
  CHECK(resolveTempSlot(id(kId1), nullptr, kTempSlotCount) == 0);
  CHECK(resolveTempSlot(id(kId1), slots, 0) == 0);
  slots[1] = id(kId1);  // duplicate: first slot wins
  CHECK(resolveTempSlot(id(kId1), slots, kTempSlotCount) == 2);
}

TEST_CASE("codec: stmChipName") {
  CHECK(std::string(stmChipName(0x413)) == "STM32F40xx/41xx");
  CHECK(std::string(stmChipName(0x423)) == "STM32F401xB/C");
  CHECK(std::string(stmChipName(0x431)) == "STM32F411xx");
  CHECK(std::string(stmChipName(0x433)) == "STM32F401xD/E");
  CHECK(std::string(stmChipName(0)) == "Unknown Chip");
  CHECK(std::string(stmChipName(0x432)) == "Unknown Chip");
  CHECK(std::string(stmChipName(0xFFFF)) == "Unknown Chip");
}

// ================================================================ fuzz

namespace {

struct Rng {
  uint32_t s;
  uint32_t next() {
    s ^= s << 13;
    s ^= s >> 17;
    s ^= s << 5;
    return s;
  }
};

bool knownStatus(ParseStatus st) { return static_cast<uint8_t>(st) <= 8; }

void checkInvariants(const std::string& line, ParseStatus st, const Reply& r) {
  CAPTURE(line);
  REQUIRE(knownStatus(st));
  if (st != ParseStatus::Ok) {
    REQUIRE(isEmptyReply(r));
    return;
  }
  REQUIRE(r.cmd != Cmd::None);
  REQUIRE(r.valveData.valve < kValveCount);
  REQUIRE(r.valveData.position <= 100);
  REQUIRE(r.valveData.status <= 0x7F);
  REQUIRE(r.valveEx.valve < kValveCount);
  REQUIRE(r.valveEx.position <= 100);
  REQUIRE(r.valveEx.target <= 100);
  REQUIRE(r.valveEx.calState <= kCalStateMask);
  REQUIRE((r.valveEx.calFlags & ~kCalFlagMask) == 0);
  REQUIRE(static_cast<uint8_t>(r.valveEx.lastMove.stop) <= 7);
  REQUIRE(static_cast<uint8_t>(r.valveEx.lastMove.dir) <= 1);
  REQUIRE(r.target.valve < kValveCount);
  REQUIRE(r.target.target <= 100);
  REQUIRE(r.oneWireList.count <= kTempSlotCount);
  REQUIRE(r.profile.count <= kProfileMaxSamples);
  REQUIRE(r.profile.valve < kValveCount);
  REQUIRE(r.serviceMove.index >= -1);
  REQUIRE(r.serviceMove.index < kValveCount);
  REQUIRE(!(r.serviceMove.ok && r.serviceMove.index < 0));
  for (const IndexedResult* x : {&r.failsafe, &r.stop}) {
    REQUIRE((x->index >= -1 && (x->index < kValveCount || x->index == kAllValves)));
    REQUIRE(!(x->ok && x->index < 0));
  }
  REQUIRE(failsafePctValid(r.valveEx.fsPct));
  REQUIRE(r.valveEx.drive <= 100);
  REQUIRE(static_cast<uint8_t>(r.status.lease) <= 2);
  REQUIRE(r.status.leaseTimeoutMin <= 1440);
  REQUIRE(r.status.failsafeMask <= 0x0FFF);
  REQUIRE(static_cast<uint8_t>(r.heartbeat.lease) <= 2);
  REQUIRE(r.leaseConfig.timeoutMin <= 1440);
  for (uint8_t pct : r.leaseConfig.failsafePct) REQUIRE(failsafePctValid(pct));
  if (r.valveEx.v3) REQUIRE(r.cmd == Cmd::Gvlvy);
  if (r.status.v3) REQUIRE(r.cmd == Cmd::Gstax);
  REQUIRE(r.valveSensors.valve < kValveCount);
  REQUIRE(r.hwId <= 0xFFF);
  REQUIRE(r.breakaway.stepPct <= 100);
  REQUIRE(r.breakaway.maxmA >= 20);
  REQUIRE(r.breakaway.maxmA <= 60);
  REQUIRE((r.ack.valve < kValveCount || r.ack.valve == kNoValve));
  if (r.cmd == Cmd::Gowvc) REQUIRE(r.oneWireList.count <= kVoltSlotCount);
  if (r.gvlonError) REQUIRE(r.cmd == Cmd::Gvlon);
}

using vdm_test::kStmGolden;
using vdm_test::kStmGoldenCount;

}  // namespace

TEST_CASE("codec: fuzz with random bytes (fixed seed)") {
  Rng rng{0xC0FFEEu};
  Reply r;
  for (int iter = 0; iter < 20000; ++iter) {
    const size_t len = rng.next() % 96;
    std::string line;
    for (size_t i = 0; i < len; ++i) {
      const uint32_t k = rng.next() % 10;
      // Bias towards the protocol alphabet so deeper paths are reached.
      static const char alpha[] = "0123456789 ,:-abcdefglnoprstvwx";
      line.push_back(k < 7 ? alpha[rng.next() % (sizeof alpha - 1)]
                           : static_cast<char>(rng.next() & 0xFF));
    }
    if (iter % 3 == 0) line = std::string(kStmGolden[rng.next() % kStmGoldenCount]).substr(0, 5) + line;
    dirty(r);
    const ParseStatus st = parse(line, r);
    checkInvariants(line, st, r);
  }
}

TEST_CASE("codec: fuzz by mutating golden replies (fixed seed)") {
  Rng rng{12345u};
  Reply r;
  size_t ok = 0;
  size_t rejected = 0;
  for (const char* g : kStmGolden) {
    CAPTURE(g);
    REQUIRE(parse(g, r) == ParseStatus::Ok);
  }
  for (int iter = 0; iter < 40000; ++iter) {
    std::string line = kStmGolden[rng.next() % kStmGoldenCount];
    const int edits = 1 + static_cast<int>(rng.next() % 3);
    for (int e = 0; e < edits && !line.empty(); ++e) {
      const size_t pos = rng.next() % line.size();
      static const char alpha[] = "0123456789 ,:-aefx";
      const char c = alpha[rng.next() % (sizeof alpha - 1)];
      switch (rng.next() % 4) {
        case 0: line[pos] = c; break;
        case 1: line.insert(line.begin() + static_cast<long>(pos), c); break;
        case 2: line.erase(pos, 1); break;
        default: line.insert(pos, line.substr(pos, rng.next() % 8)); break;
      }
    }
    dirty(r);
    const ParseStatus st = parse(line, r);
    checkInvariants(line, st, r);
    (st == ParseStatus::Ok ? ok : rejected)++;
  }
  CHECK(ok > 1000);
  CHECK(rejected > 1000);
}

// ================================================================ lower bounds

TEST_CASE("codec: every numeric field accepts its minimum") {
  Reply r;
  REQUIRE(parse("gvlvd 0 0 0 0 0 0 0 0 0 0 0", r) == ParseStatus::Ok);
  CHECK(r.valveData.status == 0);
  CHECK_FALSE(r.valveData.calibrating);
  CHECK(r.valveData.temp1 == 0);
  REQUIRE(parse("gvlvx 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0", r) == ParseStatus::Ok);
  CHECK(r.valveEx.lastMove.stop == StopReason::None);
  REQUIRE(parse("gstat 0 0 0 0 0 0", r) == ParseStatus::Ok);
  CHECK(r.status.uptimeS == 0);
  REQUIRE(parse("gmotc 0 0 0 0 0", r) == ParseStatus::Ok);
  CHECK(r.motorChars.lowFactor == 0);
  CHECK(r.motorChars.minCounts == 0);
  REQUIRE(parse("gcalx 0 0 20", r) == ParseStatus::Ok);
  REQUIRE(parse("gvers 1.4.9 0", r) == ParseStatus::Ok);
  CHECK(r.build == 0);
  REQUIRE(parse("svmov 0 err 0", r) == ParseStatus::Ok);
  CHECK(r.serviceMove.errorCode == 0);
  CHECK_FALSE(r.serviceMove.ok);
  REQUIRE(parse("gprof 0 1 0:0", r) == ParseStatus::Ok);
  CHECK(r.profile.count == 1);
  REQUIRE(parse(std::string("gvlon 0 ") + kId1 + " " + kId2, r) == ParseStatus::Ok);
  CHECK(r.valveSensors.valve == 0);
  CHECK(r.valveSensors.ids[0][0] == id(kId1));
  CHECK(r.valveSensors.ids[0][1] == id(kId2));
  REQUIRE(parse("gvlst 12 0,0,0,0,0,0,0,0,0,0,0,0", r) == ParseStatus::Ok);
  REQUIRE(parse("stvls 0", r) == ParseStatus::Ok);
  REQUIRE(parse("gtlnm 0", r) == ParseStatus::Ok);
}

TEST_CASE("codec: signed fields") {
  Reply r;
  REQUIRE(parse("goned 28-84-37-94-97-ff-03-23 -5", r) == ParseStatus::Ok);
  CHECK(r.tempData.value == -5);
  REQUIRE(parse("goned 28-84-37-94-97-ff-03-23 -0", r) == ParseStatus::Ok);
  CHECK(r.tempData.value == 0);
  CHECK(parse("goned 28-84-37-94-97-ff-03-23 -", r) == ParseStatus::BadNumber);
  CHECK(parse("goned 28-84-37-94-97-ff-03-23 -5x", r) == ParseStatus::BadNumber);
  CHECK(parse("goned 28-84-37-94-97-ff-03-23 -12x", r) == ParseStatus::BadNumber);
  CHECK(parse("goned 28-84-37-94-97-ff-03-23 5-", r) == ParseStatus::BadNumber);
  CHECK(parse("gowvd 26-11-22-33-44-55-66-29 -12345678901", r) == ParseStatus::BadNumber);
  CHECK(parse("gowvd 26-11-22-33-44-55-66-29 -9999999999", r) == ParseStatus::OutOfRange);
}

TEST_CASE("codec: list elements beyond the count are rejected") {
  Reply r;
  CHECK(parse(std::string("gonec 1 ") + kId1 + "," + kId2 + ",", r) == ParseStatus::BadFormat);
  CHECK(parse("gvlst 12 1,1,1,1,1,1,1,1,1,1,1,1,1,", r) == ParseStatus::BadFormat);
  CHECK(parse("gonec 1 ,", r) == ParseStatus::BadFormat);
  CHECK(parse("gonec 1 ,,", r) == ParseStatus::BadFormat);
}

TEST_CASE("codec: resolveTempSlot matches the first slot") {
  OneWireId slots[3];
  slots[0] = id(kId1);
  CHECK(resolveTempSlot(id(kId1), slots, 3) == 1);
  CHECK(resolveTempSlot(id(kId1), slots, 1) == 1);
}
