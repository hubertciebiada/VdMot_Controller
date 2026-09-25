// EndStopDetector inrush limit (W10, C-4): off, report only, enforced.
#include <stdint.h>

#include "doctest.h"
#include "vdm/end_stop_detector.h"

using vdm::EndStopDetector;
using Mode = vdm::EndStopDetector::InrushMode;
using Trip = vdm::EndStopDetector::Trip;

namespace {

// 1-based index of the first trip within n samples of raw, 0 if none
int firstTrip(EndStopDetector& d, int32_t raw, int n) {
  for (int i = 1; i <= n; ++i) {
    if (d.sample(raw) != Trip::None) return i;
  }
  return 0;
}

}  // namespace

TEST_CASE("inrush: constants") {
  CHECK(EndStopDetector::kInrushLimit == 2500);
  CHECK(EndStopDetector::kInrushConsecutive == 20);
}

TEST_CASE("inrush enforced: 2600 constant trips Hard at sample 21 with the raw current") {
  EndStopDetector d;
  d.arm(-500, 500, Mode::Enforce);
  CHECK(firstTrip(d, 2600, 300) == 21);
  CHECK(d.trip() == Trip::Hard);
  CHECK(d.tripCurrent() == 2600);
  CHECK(d.peak() == 2600);
  CHECK(d.inrushTrip());
  CHECK(d.inrushSeen());
  CHECK(d.current() == 0);
}

TEST_CASE("inrush enforced: negative currents trip like positive ones") {
  EndStopDetector d;
  d.arm(-500, 500, Mode::Enforce);
  CHECK(firstTrip(d, -2600, 300) == 21);
  CHECK(d.tripCurrent() == -2600);
  CHECK(d.peak() == 2600);
}

TEST_CASE("inrush: 2500 (the limit itself) and 2400 never trip in the inrush time") {
  EndStopDetector d;
  d.arm(-5000, 5000, Mode::Enforce);
  CHECK(firstTrip(d, 2500, 250) == 0);
  CHECK_FALSE(d.inrushSeen());
  d.idle();
  d.arm(-5000, 5000, Mode::Enforce);
  CHECK(firstTrip(d, 2400, 250) == 0);
  CHECK(d.trip() == Trip::None);
}

TEST_CASE("inrush: a sample below the limit restarts the run") {
  EndStopDetector d;
  d.arm(-5000, 5000, Mode::Enforce);
  CHECK(firstTrip(d, 2600, 20) == 0);
  CHECK(firstTrip(d, 0, 1) == 0);
  CHECK(firstTrip(d, 2600, 20) == 0);
  CHECK_FALSE(d.inrushSeen());
  CHECK(firstTrip(d, 2600, 1) == 1);
}

TEST_CASE("inrush: idle() restarts the run, the limit ends with the inrush time") {
  EndStopDetector d;
  d.arm(-5000, 5000, Mode::Enforce);
  CHECK(firstTrip(d, 2600, 20) == 0);
  d.idle();
  d.arm(-5000, 5000, Mode::Enforce);
  CHECK(firstTrip(d, 2600, 20) == 0);
  CHECK_FALSE(d.inrushSeen());
  d.idle();
  d.arm(-100000, 100000, Mode::Enforce);
  CHECK(firstTrip(d, 0, 240) == 0);
  // samples 241..250 are inside the inrush time, 251.. are filtered
  CHECK(firstTrip(d, 2600, 20) == 0);
  CHECK_FALSE(d.inrushSeen());
}

TEST_CASE("inrush reported: seen but no trip, the move goes on") {
  EndStopDetector d;
  d.arm(-100000, 100000, Mode::Report);
  CHECK(firstTrip(d, 2600, 21) == 0);
  CHECK(d.inrushSeen());
  CHECK_FALSE(d.inrushTrip());
  CHECK(d.trip() == Trip::None);
  CHECK(d.peak() == 0);
  d.arm(-100000, 100000, Mode::Report);
  CHECK_FALSE(d.inrushSeen());
}

TEST_CASE("inrush off: nothing is seen") {
  EndStopDetector d;
  d.arm(-100000, 100000);
  CHECK(firstTrip(d, 2800, 100) == 0);
  CHECK_FALSE(d.inrushSeen());
  d.idle();
  d.arm(-100000, 100000, Mode::Off);
  CHECK(firstTrip(d, 2800, 100) == 0);
  CHECK_FALSE(d.inrushSeen());
}

TEST_CASE("inrush enforced: a later trip after the inrush trip keeps the first") {
  EndStopDetector d;
  d.arm(-500, 500, Mode::Enforce);
  CHECK(firstTrip(d, 2600, 25) == 21);
  CHECK(d.sample(2600) == Trip::Hard);
  CHECK(d.tripCurrent() == 2600);
  CHECK(d.inrushTrip());
  d.arm(-500, 500, Mode::Enforce);
  CHECK_FALSE(d.inrushTrip());
  CHECK(d.trip() == Trip::None);
}
