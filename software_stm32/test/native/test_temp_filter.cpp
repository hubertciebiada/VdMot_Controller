#include <math.h>
#include <stdint.h>

#include "doctest.h"
#include "vdm/temp_filter.h"

using vdm::TempTrack;

namespace {

constexpr int16_t raw(double degC) { return static_cast<int16_t>(degC * 128); }

}  // namespace

TEST_CASE("rawToTenths: equals lround(raw / 128.0 * 10) for every 12-bit value from -55 to 125 degC") {
  for (int32_t r = -55 * 128; r <= 125 * 128; r += 8) {
    CAPTURE(r);
    CHECK(vdm::rawToTenths(static_cast<int16_t>(r)) == lround(r / 128.0 * 10));
  }
}

TEST_CASE("rawToTenths: equals lround(raw / 128.0 * 10) for every raw value") {
  for (int32_t r = INT16_MIN; r <= INT16_MAX; r++) {
    if (vdm::rawToTenths(static_cast<int16_t>(r)) != lround(r / 128.0 * 10)) {
      CAPTURE(r);
      FAIL("rawToTenths differs");
    }
  }
}

TEST_CASE("rawToTenths: halves round away from zero, the rest to the nearest tenth") {
  CHECK(vdm::rawToTenths(32) == 3);    // 0.25 degC = 2.5 tenths
  CHECK(vdm::rawToTenths(-32) == -3);
  CHECK(vdm::rawToTenths(6) == 0);     // 0.46875 tenths
  CHECK(vdm::rawToTenths(7) == 1);     // 0.546875 tenths
  CHECK(vdm::rawToTenths(-6) == 0);
  CHECK(vdm::rawToTenths(-7) == -1);
  CHECK(vdm::rawToTenths(0) == 0);
  CHECK(vdm::rawToTenths(INT16_MAX) == 2560);
  CHECK(vdm::rawToTenths(INT16_MIN) == -2560);
}

TEST_CASE("filterTemperature: a good read is reported and becomes the last value") {
  TempTrack t;
  CHECK(vdm::filterTemperature(t, raw(21.5)) == 215);
  CHECK(t.last == 215);
  CHECK(t.haveLast);
  CHECK(t.missed == 0);
  CHECK(t.errors == 0);
  CHECK(vdm::filterTemperature(t, raw(-10.25)) == -103);
  CHECK(t.last == -103);
}

TEST_CASE("filterTemperature: a failed read without history is -1270") {
  TempTrack t;
  CHECK(vdm::filterTemperature(t, vdm::kTempRawDisconnected) == -1270);
  CHECK(vdm::filterTemperature(t, vdm::kTempRawDisconnected - 1) == -1270);
  CHECK(t.errors == 2);
  CHECK_FALSE(t.haveLast);
  CHECK(t.missed == 0);
  CHECK(vdm::filterTemperature(t, vdm::kTempRawDisconnected + 1) == -550);  // the lowest real reading
}

TEST_CASE("filterTemperature: after a good value a failure is held twice, then -1270") {
  TempTrack t;
  vdm::filterTemperature(t, raw(20.0));
  CHECK(vdm::filterTemperature(t, vdm::kTempRawDisconnected) == 200);
  CHECK(t.missed == 1);
  CHECK(vdm::filterTemperature(t, vdm::kTempRawDisconnected) == 200);
  CHECK(t.missed == 2);
  CHECK(vdm::filterTemperature(t, vdm::kTempRawDisconnected) == -1270);
  CHECK(t.missed == 2);
  CHECK(vdm::filterTemperature(t, vdm::kTempRawDisconnected) == -1270);
  CHECK(t.errors == 4);
}

TEST_CASE("filterTemperature: a good value ends the hold and resets the missed reads") {
  TempTrack t;
  vdm::filterTemperature(t, raw(20.0));
  vdm::filterTemperature(t, vdm::kTempRawDisconnected);
  vdm::filterTemperature(t, vdm::kTempRawDisconnected);
  CHECK(vdm::filterTemperature(t, raw(22.0)) == 220);
  CHECK(t.missed == 0);
  CHECK(vdm::filterTemperature(t, vdm::kTempRawDisconnected) == 220);
  CHECK(vdm::filterTemperature(t, vdm::kTempRawDisconnected) == 220);
  CHECK(t.errors == 4);
}

TEST_CASE("filterTemperature: 85.0 degC only after a reading of at least 75.0 degC") {
  TempTrack none;
  CHECK(vdm::filterTemperature(none, vdm::kTempRaw85C) == -1270);
  CHECK(none.errors == 1);

  TempTrack low;
  vdm::filterTemperature(low, raw(74.9375));
  CHECK(vdm::filterTemperature(low, vdm::kTempRaw85C) == 749);  // held
  CHECK(low.errors == 1);

  TempTrack at;
  vdm::filterTemperature(at, raw(75.0));
  CHECK(vdm::filterTemperature(at, vdm::kTempRaw85C) == 850);

  TempTrack high;
  vdm::filterTemperature(high, raw(76.0));
  CHECK(vdm::filterTemperature(high, vdm::kTempRaw85C) == 850);
  CHECK(high.errors == 0);
  CHECK(vdm::filterTemperature(high, vdm::kTempRaw85C + 1) == 850);  // 85.0078: an ordinary value
}

TEST_CASE("filterTemperature: values next to 85.0 degC are ordinary readings") {
  TempTrack t;
  CHECK(vdm::filterTemperature(t, vdm::kTempRaw85C - 1) == 850);
  TempTrack u;
  CHECK(vdm::filterTemperature(u, vdm::kTempRaw85C + 16) == 851);
}

TEST_CASE("filterTemperature: the error count saturates") {
  TempTrack t;
  t.errors = UINT16_MAX - 1;
  vdm::filterTemperature(t, vdm::kTempRawDisconnected);
  CHECK(t.errors == UINT16_MAX);
  vdm::filterTemperature(t, vdm::kTempRawDisconnected);
  CHECK(t.errors == UINT16_MAX);
}

TEST_CASE("temp_filter: the constants") {
  CHECK(vdm::kTempRawDisconnected == -7040);
  CHECK(vdm::kTempRaw85C == 85 * 128);
  CHECK(vdm::kTemp85MinPrevious == 750);
  CHECK(vdm::kTempHoldCycles == 2);
  CHECK(vdm::kTempFailedTenths == -1270);
}
