#include <stdint.h>
#include <stdio.h>

#include <string>

#include "doctest.h"
#include "vdm/replies.h"

using vdm::StaticBufWriter;
using vdm::ValveDataReply;

namespace {

// The v1 implementation built the reply with itoa(int) + strcat.
std::string legacyValveData(const ValveDataReply& r) {
  std::string s = "gvlvd ";
  const long long fields[] = {static_cast<long long>(r.index), r.actualPosition, r.meanCurrent,
                              r.status,       r.temperature1,  r.temperature2,
                              r.movements,    r.openingCount,  r.closingCount,
                              r.deadzoneCount, r.calibRetries};
  for (long long f : fields) {
    char num[24];
    snprintf(num, sizeof(num), "%lld", f);
    s += num;
    s += ' ';
  }
  return s;
}

ValveDataReply typical() {
  ValveDataReply r{};
  r.index = 3;
  r.actualPosition = 42;
  r.meanCurrent = 17;
  r.status = 0x01 | 0x80;
  r.temperature1 = 215;
  r.temperature2 = -500;
  r.movements = 12;
  r.openingCount = 3567;
  r.closingCount = 3610;
  r.deadzoneCount = 43;
  r.calibRetries = 0;
  return r;
}

}  // namespace

TEST_CASE("formatValveData: v1 byte format") {
  StaticBufWriter<vdm::kValveDataReplyMaxLen + 1> w;
  const ValveDataReply r = typical();
  REQUIRE(vdm::formatValveData(w, "gvlvd", r));
  CHECK(std::string(w.c_str()) == "gvlvd 3 42 17 129 215 -500 12 3567 3610 43 0 ");
  CHECK(std::string(w.c_str()) == legacyValveData(r));
}

TEST_CASE("formatValveData: negative deadzone and sentinels print signed") {
  StaticBufWriter<vdm::kValveDataReplyMaxLen + 1> w;
  ValveDataReply r = typical();
  r.deadzoneCount = -150;
  r.temperature1 = -1270;
  r.temperature2 = 850;
  REQUIRE(vdm::formatValveData(w, "gvlvd", r));
  CHECK(std::string(w.c_str()) == legacyValveData(r));
  CHECK(std::string(w.c_str()).find(" -150 ") != std::string::npos);
}

TEST_CASE("formatValveData: worst case fits kValveDataReplyMaxLen") {
  ValveDataReply r{};
  r.index = UINT32_MAX;
  r.actualPosition = INT32_MIN;
  r.meanCurrent = INT32_MIN;
  r.status = INT32_MIN;
  r.temperature1 = INT32_MIN;
  r.temperature2 = INT32_MIN;
  r.movements = INT32_MIN;
  r.openingCount = INT32_MIN;
  r.closingCount = INT32_MIN;
  r.deadzoneCount = INT32_MIN;
  r.calibRetries = INT32_MIN;
  StaticBufWriter<vdm::kValveDataReplyMaxLen + 1> w;
  REQUIRE(vdm::formatValveData(w, "gvlvd", r));
  CHECK(w.length() <= vdm::kValveDataReplyMaxLen);
  CHECK(std::string(w.c_str()) == legacyValveData(r));

  // The old 60-byte buffer could not hold this reply (the v1 overflow).
  CHECK(w.length() + 1 > 60);
}

TEST_CASE("formatValveData: too small buffer writes nothing") {
  const ValveDataReply r = typical();
  const size_t needed = legacyValveData(r).size();
  for (size_t cap = 0; cap <= needed; ++cap) {
    char buf[128];
    buf[0] = 'q';
    vdm::BufWriter w(buf, cap);
    CAPTURE(cap);
    CHECK_FALSE(vdm::formatValveData(w, "gvlvd", r));
    CHECK(w.length() == 0);
    CHECK_FALSE(w.ok());
  }
  char buf[128];
  vdm::BufWriter exact(buf, needed + 1);
  CHECK(vdm::formatValveData(exact, "gvlvd", r));
  CHECK(exact.ok());
}

TEST_CASE("formatValveData: appends after existing content and rolls back on failure") {
  StaticBufWriter<20> w;
  REQUIRE(w.append("keep"));
  CHECK_FALSE(vdm::formatValveData(w, "gvlvd", typical()));
  CHECK(std::string(w.c_str()) == "keep");
  CHECK_FALSE(vdm::formatValveData(w, nullptr, typical()));
  CHECK(std::string(w.c_str()) == "keep");
}

TEST_CASE("formatStatusList: v1 byte format without trailing comma") {
  const uint8_t status[12] = {1, 2, 3, 4, 5, 6, 7, 8, 9, 1, 255, 0};
  StaticBufWriter<80> w;
  REQUIRE(vdm::formatStatusList(w, "gvlst", status, 12));
  CHECK(std::string(w.c_str()) == "gvlst 12 1,2,3,4,5,6,7,8,9,1,255,0 ");
}

TEST_CASE("formatStatusList: small and empty lists") {
  const uint8_t one[1] = {6};
  StaticBufWriter<32> w;
  REQUIRE(vdm::formatStatusList(w, "gvlst", one, 1));
  CHECK(std::string(w.c_str()) == "gvlst 1 6 ");
  w.clear();
  REQUIRE(vdm::formatStatusList(w, "gvlst", nullptr, 0));
  CHECK(std::string(w.c_str()) == "gvlst 0  ");
  w.clear();
  CHECK_FALSE(vdm::formatStatusList(w, "gvlst", nullptr, 3));
  CHECK(w.length() == 0);
}

TEST_CASE("formatStatusList: too small buffer writes nothing") {
  const uint8_t status[12] = {1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1};
  const std::string expected = "gvlst 12 1,1,1,1,1,1,1,1,1,1,1,1 ";
  for (size_t cap = 0; cap <= expected.size(); ++cap) {
    char buf[64];
    vdm::BufWriter w(buf, cap);
    CAPTURE(cap);
    CHECK_FALSE(vdm::formatStatusList(w, "gvlst", status, 12));
    CHECK(w.length() == 0);
  }
  char buf[64];
  vdm::BufWriter w(buf, expected.size() + 1);
  REQUIRE(vdm::formatStatusList(w, "gvlst", status, 12));
  CHECK(std::string(w.c_str()) == expected);
}
