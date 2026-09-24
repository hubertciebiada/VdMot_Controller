#include <stdint.h>

#include <initializer_list>

#include "doctest.h"
#include "vdm/motor_params.h"

using vdm::MotorParams;

namespace {

MotorParams valid() { return MotorParams{17, 23, 40, 3000, 1}; }

bool same(const MotorParams& a, const MotorParams& b) {
  return a.lowFac == b.lowFac && a.highFac == b.highFac && a.startOnPower == b.startOnPower &&
         a.minCounts == b.minCounts && a.maxRetries == b.maxRetries;
}

}  // namespace

TEST_CASE("ParamRange::contains is inclusive") {
  const vdm::ParamRange r{5, 50, 17};
  CHECK_FALSE(r.contains(4));
  CHECK(r.contains(5));
  CHECK(r.contains(50));
  CHECK_FALSE(r.contains(51));
  CHECK_FALSE(r.contains(0xFFFFFFFFu));
}

TEST_CASE("range table matches the web page and the 1.x start-up defaults") {
  CHECK(vdm::kLowFacRange.min == 5);
  CHECK(vdm::kLowFacRange.max == 50);
  CHECK(vdm::kHighFacRange.min == 5);
  CHECK(vdm::kHighFacRange.max == 50);
  CHECK(vdm::kStartOnPowerRange.max == 100);
  CHECK(vdm::kMinCountsRange.max == 60000);
  CHECK(vdm::kMaxRetriesRange.max == 2);
  const MotorParams d = vdm::kMotorParamsDefault;
  CHECK(same(d, MotorParams{17, 17, 30, 3000, 2}));
  CHECK(vdm::motorParamsValid(d));
}

TEST_CASE("motorParamsValid checks every field at both ends") {
  CHECK(vdm::motorParamsValid(valid()));
  CHECK(vdm::motorParamsValid(MotorParams{5, 5, 0, 0, 0}));
  CHECK(vdm::motorParamsValid(MotorParams{50, 50, 100, 60000, 2}));

  MotorParams p = valid();
  p.lowFac = 4;
  CHECK_FALSE(vdm::motorParamsValid(p));
  p = valid();
  p.lowFac = 51;
  CHECK_FALSE(vdm::motorParamsValid(p));
  p = valid();
  p.highFac = 4;
  CHECK_FALSE(vdm::motorParamsValid(p));
  p = valid();
  p.highFac = 51;
  CHECK_FALSE(vdm::motorParamsValid(p));
  p = valid();
  p.startOnPower = 101;
  CHECK_FALSE(vdm::motorParamsValid(p));
  p = valid();
  p.minCounts = 60001;
  CHECK_FALSE(vdm::motorParamsValid(p));
  p = valid();
  p.maxRetries = 3;
  CHECK_FALSE(vdm::motorParamsValid(p));
}

TEST_CASE("sanitizeMotorParams replaces only out-of-range fields") {
  CHECK(same(vdm::sanitizeMotorParams(valid()), valid()));

  // erased EEPROM
  const MotorParams erased{0xFF, 0xFF, 0xFF, 0xFFFF, 0xFF};
  CHECK(same(vdm::sanitizeMotorParams(erased), vdm::kMotorParamsDefault));

  // 1.x accepted 5..9 and 41..50 with smotc but dropped them at the next start
  CHECK(vdm::sanitizeMotorParams(MotorParams{5, 50, 30, 3000, 2}).lowFac == 5);
  CHECK(vdm::sanitizeMotorParams(MotorParams{5, 50, 30, 3000, 2}).highFac == 50);

  const MotorParams mixed{4, 30, 101, 60000, 3};
  const MotorParams s = vdm::sanitizeMotorParams(mixed);
  CHECK(s.lowFac == 17);
  CHECK(s.highFac == 30);
  CHECK(s.startOnPower == 30);
  CHECK(s.minCounts == 60000);
  CHECK(s.maxRetries == 2);

  CHECK(vdm::sanitizeMotorParams(MotorParams{17, 51, 0, 60001, 0}).highFac == 17);
  CHECK(vdm::sanitizeMotorParams(MotorParams{17, 51, 0, 60001, 0}).startOnPower == 0);
  CHECK(vdm::sanitizeMotorParams(MotorParams{17, 51, 0, 60001, 0}).minCounts == 3000);
  CHECK(vdm::sanitizeMotorParams(MotorParams{17, 51, 0, 60001, 0}).maxRetries == 0);
}

TEST_CASE("applyMotorParamsRequest: three mandatory values") {
  MotorParams p = valid();
  const uint32_t v[5] = {10, 20, 55, 0, 0};
  REQUIRE(vdm::applyMotorParamsRequest(p, 3, v));
  CHECK(same(p, MotorParams{10, 20, 55, 3000, 1}));
}

TEST_CASE("applyMotorParamsRequest: optional minCounts and retries") {
  MotorParams p = valid();
  const uint32_t v4[5] = {10, 20, 55, 100, 99};
  REQUIRE(vdm::applyMotorParamsRequest(p, 4, v4));
  CHECK(same(p, MotorParams{10, 20, 55, 100, 1}));

  const uint32_t v5[5] = {5, 50, 100, 60000, 2};
  REQUIRE(vdm::applyMotorParamsRequest(p, 5, v5));
  CHECK(same(p, MotorParams{5, 50, 100, 60000, 2}));

  const uint32_t v0[5] = {5, 5, 0, 0, 0};
  REQUIRE(vdm::applyMotorParamsRequest(p, 5, v0));
  CHECK(same(p, MotorParams{5, 5, 0, 0, 0}));
}

TEST_CASE("applyMotorParamsRequest: any out-of-range value rejects the whole request") {
  const uint32_t cases[][5] = {
      {4, 20, 50, 3000, 2},  {51, 20, 50, 3000, 2}, {17, 4, 50, 3000, 2},
      {17, 51, 50, 3000, 2}, {17, 20, 101, 3000, 2}, {17, 20, 50, 60001, 2},
      {17, 20, 50, 3000, 3}, {0xFFFFFFFFu, 20, 50, 3000, 2},
  };
  for (const auto& c : cases) {
    MotorParams p = valid();
    CHECK_FALSE(vdm::applyMotorParamsRequest(p, 5, c));
    CHECK(same(p, valid()));
  }
}

TEST_CASE("applyMotorParamsRequest: unused optional values are not checked") {
  MotorParams p = valid();
  const uint32_t v[5] = {17, 17, 50, 99999, 99};
  CHECK(vdm::applyMotorParamsRequest(p, 3, v));
  CHECK(p.minCounts == 3000);
  CHECK(p.maxRetries == 1);

  const uint32_t w[5] = {17, 17, 50, 400, 99};
  CHECK(vdm::applyMotorParamsRequest(p, 4, w));
  CHECK(p.minCounts == 400);
}

TEST_CASE("applyMotorParamsRequest: argument count outside 3..5") {
  const uint32_t v[5] = {17, 17, 50, 3000, 2};
  for (uint8_t argc : {0, 1, 2, 6, 255}) {
    MotorParams p = valid();
    CHECK_FALSE(vdm::applyMotorParamsRequest(p, argc, v));
    CHECK(same(p, valid()));
  }
}
