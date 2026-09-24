#include <stdint.h>

#include <initializer_list>

#include "doctest.h"
#include "vdm/motor_params.h"

using vdm::MotorParams;
using vdm::ParamsRequest;

namespace {

MotorParams valid() { return MotorParams{17, 23, 40, 3000, 1}; }

bool same(const MotorParams& a, const MotorParams& b) { return vdm::sameMotorParams(a, b); }

}  // namespace

TEST_CASE("ParamRange::contains is inclusive") {
  const vdm::ParamRange r{5, 50, 17};
  CHECK_FALSE(r.contains(4));
  CHECK(r.contains(5));
  CHECK(r.contains(50));
  CHECK_FALSE(r.contains(51));
  CHECK_FALSE(r.contains(0xFFFFFFFFu));
}

TEST_CASE("range table matches the 1.x start-up limits and defaults") {
  CHECK(vdm::kLowFacRange.min == 10);
  CHECK(vdm::kLowFacRange.max == 40);
  CHECK(vdm::kHighFacRange.min == 10);
  CHECK(vdm::kHighFacRange.max == 40);
  CHECK(vdm::kStartOnPowerRange.max == 100);
  CHECK(vdm::kMinCountsRange.max == 60000);
  CHECK(vdm::kMaxRetriesRange.max == 2);
  const MotorParams d = vdm::kMotorParamsDefault;
  CHECK(same(d, MotorParams{17, 17, 30, 3000, 2}));
  CHECK(vdm::motorParamsValid(d));
}

TEST_CASE("motorParamsValid checks every field at both ends") {
  CHECK(vdm::motorParamsValid(valid()));
  CHECK(vdm::motorParamsValid(MotorParams{10, 10, 0, 0, 0}));
  CHECK(vdm::motorParamsValid(MotorParams{40, 40, 100, 60000, 2}));

  MotorParams p = valid();
  p.lowFac = 9;
  CHECK_FALSE(vdm::motorParamsValid(p));
  p = valid();
  p.lowFac = 41;
  CHECK_FALSE(vdm::motorParamsValid(p));
  p = valid();
  p.highFac = 9;
  CHECK_FALSE(vdm::motorParamsValid(p));
  p = valid();
  p.highFac = 41;
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

  // 1.x smotc stored 5..9 and 41..50 but 1.x dropped them at the next start;
  // an upgrade must not bring them into effect (factor 8 would stop every move)
  for (uint8_t f : {5, 8, 9, 41, 50}) {
    const MotorParams r = vdm::sanitizeMotorParams(MotorParams{f, f, 30, 3000, 2});
    CHECK(r.lowFac == 17);
    CHECK(r.highFac == 17);
  }
  CHECK(vdm::sanitizeMotorParams(MotorParams{10, 40, 30, 3000, 2}).lowFac == 10);
  CHECK(vdm::sanitizeMotorParams(MotorParams{10, 40, 30, 3000, 2}).highFac == 40);

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
  REQUIRE(vdm::applyMotorParamsRequest(p, 3, v) == ParamsRequest::Applied);
  CHECK(same(p, MotorParams{10, 20, 55, 3000, 1}));
}

TEST_CASE("applyMotorParamsRequest: optional minCounts and retries") {
  MotorParams p = valid();
  const uint32_t v4[5] = {10, 20, 55, 100, 99};
  REQUIRE(vdm::applyMotorParamsRequest(p, 4, v4) == ParamsRequest::Applied);
  CHECK(same(p, MotorParams{10, 20, 55, 100, 1}));

  const uint32_t v5[5] = {10, 40, 100, 60000, 2};
  REQUIRE(vdm::applyMotorParamsRequest(p, 5, v5) == ParamsRequest::Applied);
  CHECK(same(p, MotorParams{10, 40, 100, 60000, 2}));

  const uint32_t v0[5] = {10, 10, 0, 0, 0};
  REQUIRE(vdm::applyMotorParamsRequest(p, 5, v0) == ParamsRequest::Applied);
  CHECK(same(p, MotorParams{10, 10, 0, 0, 0}));
}

TEST_CASE("applyMotorParamsRequest: an out-of-range value keeps its field, the others are applied") {
  struct Case {
    uint32_t v[5];
    MotorParams expected;
  };
  // valid() is {17, 23, 40, 3000, 1}; the in-range values of each request are 18, 24, 50, 2500, 2
  const Case cases[] = {
      {{9, 24, 50, 2500, 2}, {17, 24, 50, 2500, 2}},
      {{41, 24, 50, 2500, 2}, {17, 24, 50, 2500, 2}},
      {{5, 24, 50, 2500, 2}, {17, 24, 50, 2500, 2}},    // legacy web page: factor 0.5
      {{18, 9, 50, 2500, 2}, {18, 23, 50, 2500, 2}},
      {{18, 50, 50, 2500, 2}, {18, 23, 50, 2500, 2}},   // legacy web page: factor 5.0
      {{18, 24, 101, 2500, 2}, {18, 24, 40, 2500, 2}},
      {{18, 24, 50, 60001, 2}, {18, 24, 50, 3000, 2}},
      {{18, 24, 50, 2500, 3}, {18, 24, 50, 2500, 1}},
      {{0xFFFFFFFFu, 24, 50, 2500, 2}, {17, 24, 50, 2500, 2}},
      {{8, 45, 50, 2500, 2}, {17, 23, 50, 2500, 2}},    // both factors out of range
      {{0, 0, 200, 70000, 9}, {17, 23, 40, 3000, 1}},   // nothing in range
  };
  for (const auto& c : cases) {
    MotorParams p = valid();
    CHECK(vdm::applyMotorParamsRequest(p, 5, c.v) == ParamsRequest::Partial);
    CHECK(same(p, c.expected));
  }
}

TEST_CASE("applyMotorParamsRequest: legacy ESP saves factor 0.8 together with a new start %") {
  // smotc 8 17 60 3000 2: the factor stays, start % and the rest are taken as by 1.x
  MotorParams p{17, 17, 30, 3000, 2};
  const uint32_t v[5] = {8, 17, 60, 2500, 0};
  CHECK(vdm::applyMotorParamsRequest(p, 5, v) == ParamsRequest::Partial);
  CHECK(same(p, MotorParams{17, 17, 60, 2500, 0}));
}

TEST_CASE("applyMotorParamsRequest: unused optional values are not checked") {
  MotorParams p = valid();
  const uint32_t v[5] = {17, 17, 50, 99999, 99};
  CHECK(vdm::applyMotorParamsRequest(p, 3, v) == ParamsRequest::Applied);
  CHECK(p.minCounts == 3000);
  CHECK(p.maxRetries == 1);

  const uint32_t w[5] = {17, 17, 50, 400, 99};
  CHECK(vdm::applyMotorParamsRequest(p, 4, w) == ParamsRequest::Applied);
  CHECK(p.minCounts == 400);
}

TEST_CASE("applyMotorParamsRequest: argument count outside 3..5 applies nothing") {
  const uint32_t v[5] = {18, 18, 50, 3000, 2};
  for (uint8_t argc : {0, 1, 2, 6, 255}) {
    MotorParams p = valid();
    CHECK(vdm::applyMotorParamsRequest(p, argc, v) == ParamsRequest::Rejected);
    CHECK(same(p, valid()));
  }
}

TEST_CASE("sameMotorParams compares every field") {
  const MotorParams a = valid();
  CHECK(vdm::sameMotorParams(a, a));
  MotorParams b = a;
  b.lowFac++;
  CHECK_FALSE(vdm::sameMotorParams(a, b));
  b = a;
  b.highFac++;
  CHECK_FALSE(vdm::sameMotorParams(a, b));
  b = a;
  b.startOnPower++;
  CHECK_FALSE(vdm::sameMotorParams(a, b));
  b = a;
  b.minCounts++;
  CHECK_FALSE(vdm::sameMotorParams(a, b));
  b = a;
  b.maxRetries++;
  CHECK_FALSE(vdm::sameMotorParams(a, b));
}
