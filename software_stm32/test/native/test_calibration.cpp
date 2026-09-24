#include <stdint.h>

#include <initializer_list>

#include "doctest.h"
#include "vdm/calibration.h"
#include "vdm/move_classifier.h"

using vdm::CalibrationVerdict;
using vdm::EscalationConfig;

TEST_CASE("endStopBound: learned mean times factor, 15 mA floor (S01)") {
  CHECK(vdm::endStopBound(20, 17) == 340);   // 1.x default: 34 mA
  CHECK(vdm::endStopBound(30, 17) == 510);   // a high learned mean raises the threshold
  CHECK(vdm::endStopBound(15, 17) == 255);
  CHECK(vdm::endStopBound(14, 17) == 255);   // floor
  CHECK(vdm::endStopBound(0, 17) == 255);
  CHECK(vdm::endStopBound(16, 10) == 160);
  CHECK(vdm::endStopBound(65535, 50) == 65535 * 50);
  CHECK(vdm::endStopBound(20, 0) == 0);
}

TEST_CASE("endStopBound: an explicit floor") {
  CHECK(vdm::endStopBound(16, 17, 20) == 340);
  CHECK(vdm::endStopBound(20, 17, 20) == 340);
  CHECK(vdm::endStopBound(25, 17, 20) == 425);
  CHECK(vdm::endStopBound(0, 17, 0) == 0);
}

TEST_CASE("calibrationFloor: closing strokes never below the 1.x closing threshold") {
  CHECK(vdm::kCalibrationCloseFloor_mA == 20);
  CHECK(vdm::calibrationFloor(vdm::kDirClose) == 20);
  CHECK(vdm::calibrationFloor(vdm::kDirOpen) == vdm::kMeanCurrentFloor_mA);

  // owner's valves: learned mean 16..17 mA, factor 1.7; 1.x closed at 34 mA
  for (uint16_t mean : {16, 17}) {
    const int32_t closing = vdm::endStopBound(mean, 17, vdm::calibrationFloor(vdm::kDirClose));
    CHECK(closing == 340);
    CHECK(vdm::endStopBound(mean, 17, vdm::calibrationFloor(vdm::kDirOpen)) == mean * 17);
  }
  // a valve that needs more current than 20 mA keeps its learned mean
  CHECK(vdm::endStopBound(24, 17, vdm::calibrationFloor(vdm::kDirClose)) == 408);
}

TEST_CASE("escalation config validation") {
  CHECK(vdm::escalationValid(vdm::kEscalationDefault));
  CHECK(vdm::escalationValid(EscalationConfig{0, 0, 20}));
  CHECK(vdm::escalationValid(EscalationConfig{1, 100, 60}));
  CHECK_FALSE(vdm::escalationValid(EscalationConfig{2, 25, 50}));
  CHECK_FALSE(vdm::escalationValid(EscalationConfig{1, 101, 50}));
  CHECK_FALSE(vdm::escalationValid(EscalationConfig{1, 25, 19}));
  CHECK_FALSE(vdm::escalationValid(EscalationConfig{1, 25, 61}));

  const EscalationConfig bad{1, 25, 61};
  const EscalationConfig s = vdm::sanitizeEscalation(bad);
  CHECK(s.enable == vdm::kEscalationDefault.enable);
  CHECK(s.stepPct == vdm::kEscalationDefault.stepPct);
  CHECK(s.maxmA == vdm::kEscalationDefault.maxmA);
  const EscalationConfig good{1, 40, 45};
  CHECK(vdm::sanitizeEscalation(good).stepPct == 40);
  CHECK(vdm::kEscalationDefault.enable == 0);
}

TEST_CASE("escalatedBound: grows by stepPct per repetition (F01)") {
  const EscalationConfig c{1, 25, 60};
  CHECK(vdm::escalatedBound(340, 0, c) == 340);
  CHECK(vdm::escalatedBound(340, 1, c) == 425);
  CHECK(vdm::escalatedBound(340, 2, c) == 510);
  CHECK(vdm::escalatedBound(340, 3, c) == 595);
  CHECK(vdm::escalatedBound(340, 4, c) == 600);  // capped by maxmA
  CHECK(vdm::escalatedBound(340, 255, c) == 600);
}

TEST_CASE("escalatedBound: disabled or zero step leaves the bound") {
  CHECK(vdm::escalatedBound(340, 2, EscalationConfig{0, 25, 60}) == 340);
  CHECK(vdm::escalatedBound(340, 2, EscalationConfig{1, 0, 60}) == 340);
  CHECK(vdm::escalatedBound(0, 2, EscalationConfig{1, 25, 60}) == 0);
  CHECK(vdm::escalatedBound(-5, 2, EscalationConfig{1, 25, 60}) == -5);
}

TEST_CASE("escalatedBound: cap by maxmA and by the 60 mA safety limit") {
  CHECK(vdm::escalatedBound(340, 1, EscalationConfig{1, 100, 40}) == 400);
  CHECK(vdm::escalatedBound(340, 1, EscalationConfig{1, 100, 20}) == 340);   // already above the cap
  CHECK(vdm::escalatedBound(700, 2, EscalationConfig{1, 100, 60}) == 700);   // never lowered
  CHECK(vdm::escalatedBound(599, 1, EscalationConfig{1, 1, 60}) == 600);     // 604.99 -> cap
  CHECK(vdm::escalatedBound(340, 1, EscalationConfig{1, 100, 200}) == 600);  // bad cap: safety limit
  CHECK(vdm::escalatedBound(600, 1, EscalationConfig{1, 50, 60}) == 600);
}

TEST_CASE("evaluateCalibration: accept needs both strokes at the minimum") {
  CHECK(vdm::evaluateCalibration(3000, 3000, 3000, 0, 2) == CalibrationVerdict::Accept);
  CHECK(vdm::evaluateCalibration(2999, 3000, 3000, 0, 2) == CalibrationVerdict::Retry);
  CHECK(vdm::evaluateCalibration(3000, 2999, 3000, 0, 2) == CalibrationVerdict::Retry);
  CHECK(vdm::evaluateCalibration(0, 0, 0, 0, 0) == CalibrationVerdict::Blocked);
}

TEST_CASE("evaluateCalibration: never accepts a stroke that gives scaler 0") {
  CHECK(vdm::evaluateCalibration(99, 5000, 0, 0, 2) == CalibrationVerdict::Retry);
  CHECK(vdm::evaluateCalibration(5000, 99, 50, 0, 2) == CalibrationVerdict::Retry);
  CHECK(vdm::evaluateCalibration(100, 100, 0, 0, 2) == CalibrationVerdict::Accept);
}

TEST_CASE("evaluateCalibration: retries then BLOCKS like 1.x") {
  // maxRetries 2: passes 1, 2 retry, pass 3 blocks
  CHECK(vdm::evaluateCalibration(80, 100, 3000, 0, 2) == CalibrationVerdict::Retry);
  CHECK(vdm::evaluateCalibration(80, 100, 3000, 1, 2) == CalibrationVerdict::Retry);
  CHECK(vdm::evaluateCalibration(80, 100, 3000, 2, 2) == CalibrationVerdict::Blocked);
  CHECK(vdm::evaluateCalibration(80, 100, 3000, 0, 0) == CalibrationVerdict::Blocked);
  CHECK(vdm::evaluateCalibration(80, 100, 3000, 255, 255) == CalibrationVerdict::Blocked);
  // a good pass after failed ones is accepted
  CHECK(vdm::evaluateCalibration(4000, 4100, 3000, 2, 2) == CalibrationVerdict::Accept);
}

TEST_CASE("strokeMeanCurrent: |sum| / samples in mA") {
  CHECK(vdm::strokeMeanCurrent(0, 0) == 0);
  CHECK(vdm::strokeMeanCurrent(12345, 0) == 0);
  CHECK(vdm::strokeMeanCurrent(1700, 10) == 17);
  CHECK(vdm::strokeMeanCurrent(-1700, 10) == 17);
  CHECK(vdm::strokeMeanCurrent(1799, 10) == 17);
  CHECK(vdm::strokeMeanCurrent(INT32_MIN, 1) == 0xFFFF);
  CHECK(vdm::strokeMeanCurrent(INT32_MAX, 65535) == 3276);
}

TEST_CASE("learnMeanCurrent: only strokes with enough samples (S02)") {
  const uint16_t n = vdm::kMinMeanSamples;
  CHECK(vdm::learnMeanCurrent(20, 16, n, 18, n) == 17);
  CHECK(vdm::learnMeanCurrent(20, 16, n, 18, n - 1) == 16);
  CHECK(vdm::learnMeanCurrent(20, 16, n - 1, 18, n) == 18);
  CHECK(vdm::learnMeanCurrent(20, 16, n - 1, 18, n - 1) == 20);
  CHECK(vdm::learnMeanCurrent(20, 0, n, 0, n) == 20);
  CHECK(vdm::learnMeanCurrent(20, 0, n, 30, n) == 30);
  CHECK(vdm::learnMeanCurrent(20, 65535, 65535, 65535, 65535) == 65535);
  CHECK(vdm::learnMeanCurrent(20, 17, 0, 19, 0) == 20);
}
