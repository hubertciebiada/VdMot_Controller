#include <stdint.h>

#include "doctest.h"
#include "vdm/end_stop_detector.h"

using vdm::EndStopDetector;
using Trip = vdm::EndStopDetector::Trip;

namespace {

// Firmware 1.x TimerHandler0, kept as the reference for the filter and the
// bound check; its overcurrent counter summed over the whole move.
struct LegacyDetector {
  int current = 0;
  int old = 0;
  int debounce = 0;
  int overcnt = 0;
  int low = 0;
  int high = 0;

  bool sample(int raw) {
    if (debounce < 255) debounce++;
    if (debounce > 250) {
      current = (int)(((int32_t)old * 9800 + (int32_t)raw * 200) / 10000);
      old = current;
    }
    if (current > 600 || current < -600)
      if (overcnt < 255) overcnt++;
    return current > 1000 || current < -1000 || overcnt > 10 ||
           (debounce > 250 && (current > high || current < low));
  }
};

// Runs n samples of `raw`, returns the index of the first trip or -1.
int runUntilTrip(EndStopDetector& d, int32_t raw, int n) {
  for (int i = 0; i < n; ++i) {
    if (d.sample(raw) != Trip::None) return i;
  }
  return -1;
}

}  // namespace

TEST_CASE("EndStopDetector: filter held at 0 during the inrush time") {
  EndStopDetector d;
  d.arm(-340, 340);
  for (int i = 0; i < 250; ++i) {
    CHECK(d.sample(900) == Trip::None);
    CHECK(d.current() == 0);
  }
  CHECK(d.sample(900) == Trip::None);
  CHECK(d.current() == 18);  // 900 * 0.02
}

TEST_CASE("EndStopDetector: bound trip after the filter crosses the threshold") {
  EndStopDetector d;
  d.arm(-340, 340);
  const int idx = runUntilTrip(d, 500, 2000);
  REQUIRE(idx > 250);
  CHECK(d.trip() == Trip::Bound);
  CHECK(d.tripCurrent() > 340);
  CHECK(d.current() > 340);
  CHECK(d.peak() == d.current());
}

TEST_CASE("EndStopDetector: negative direction trips on the low bound") {
  EndStopDetector d;
  d.arm(-340, 340);
  REQUIRE(runUntilTrip(d, -500, 2000) > 250);
  CHECK(d.trip() == Trip::Bound);
  CHECK(d.tripCurrent() < -340);
  CHECK(d.peak() == -d.current());
}

TEST_CASE("EndStopDetector: current inside the bounds never trips") {
  EndStopDetector d;
  d.arm(-340, 340);
  CHECK(runUntilTrip(d, 300, 5000) == -1);
  CHECK(d.trip() == Trip::None);
  // integer IIR dead band of firmware 1.x: settles below 300 - 49
  CHECK(d.current() >= 251);
  CHECK(d.current() <= 300);
}

TEST_CASE("EndStopDetector: equal to the bound is not beyond it") {
  EndStopDetector d;
  d.arm(-100, 100);
  // settles at exactly 100 from a raw value of 149 (dead band 49)
  CHECK(runUntilTrip(d, 149, 5000) == -1);
  CHECK(d.current() == 100);
  CHECK(d.sample(151) == Trip::Bound);  // 100*0.98 + 151*0.02 = 101.02 -> 101 > 100
  CHECK(d.current() == 101);
}

TEST_CASE("EndStopDetector: matches firmware 1.x while no spike occurs") {
  const int raws[] = {0, 120, 350, 480, -200, -700, 20};
  for (int raw : raws) {
    EndStopDetector d;
    LegacyDetector l;
    d.arm(-340, 340);
    l.low = -340;
    l.high = 340;
    for (int i = 0; i < 1500; ++i) {
      const bool legacyTrip = l.sample(raw);
      const bool trip = d.sample(raw) != Trip::None;
      REQUIRE(trip == legacyTrip);
      REQUIRE(d.current() == l.current);
      if (trip) break;
    }
  }
}

TEST_CASE("EndStopDetector: safety limit needs more than 10 consecutive samples") {
  EndStopDetector d;
  d.arm(-100000, 100000);  // bounds out of the way
  // bring the filter above 60 mA
  int i = 0;
  while (d.current() <= 600) {
    REQUIRE(i++ < 5000);
    d.sample(990);
  }
  // the sample above was the first one over the limit
  CHECK(d.overCount() == 1);
  for (int k = 2; k <= 10; ++k) {
    CHECK(d.sample(990) == Trip::None);
    CHECK(d.overCount() == k);
  }
  CHECK(d.sample(990) == Trip::Safety);
  CHECK(d.trip() == Trip::Safety);
  CHECK(d.tripCurrent() > 600);
}

TEST_CASE("EndStopDetector: short spikes no longer add up (consecutive counter)") {
  EndStopDetector d;
  LegacyDetector l;
  d.arm(-100000, 100000);
  l.low = -100000;
  l.high = 100000;
  // settle both filters just below 60 mA
  for (int i = 0; i < 2000; ++i) {
    d.sample(640);
    l.sample(640);
  }
  REQUIRE(d.current() <= 600);
  REQUIRE(d.current() > 590);

  // repeated bursts over the limit, each shorter than 10 samples
  bool legacyTripped = false;
  for (int burst = 0; burst < 20 && !legacyTripped; ++burst) {
    for (int k = 0; k < 6; ++k) {
      CHECK(d.sample(1100) == Trip::None);
      legacyTripped = legacyTripped || l.sample(1100);
    }
    // back below the limit, then settle just below it again (from below)
    for (int k = 0; k < 100; ++k) {
      CHECK(d.sample(0) == Trip::None);
      legacyTripped = legacyTripped || l.sample(0);
    }
    for (int k = 0; k < 800; ++k) {
      CHECK(d.sample(640) == Trip::None);
      legacyTripped = legacyTripped || l.sample(640);
    }
  }
  CHECK(legacyTripped);  // 1.x stopped the motor on the sum of the bursts
  CHECK(d.trip() == Trip::None);
}

TEST_CASE("EndStopDetector: counter resets when the current drops to the limit") {
  EndStopDetector d;
  d.arm(-100000, 100000);
  while (d.current() <= 600) d.sample(900);
  CHECK(d.overCount() == 1);
  while (d.current() > 600) d.sample(0);
  CHECK(d.overCount() == 0);
}

TEST_CASE("EndStopDetector: hard limit trips on the first sample above it") {
  EndStopDetector d;
  d.arm(-100000, 100000);
  for (int i = 0; i < 250; ++i) d.sample(0);
  // drive the filter over 100 mA as fast as possible
  Trip t = Trip::None;
  int n = 0;
  while (t == Trip::None) {
    REQUIRE(n++ < 1000);
    t = d.sample(100000);
  }
  CHECK(t == Trip::Hard);
  CHECK(d.current() > 1000);
  CHECK(d.trip() == Trip::Hard);
}

TEST_CASE("EndStopDetector: hard beats safety beats bound") {
  EndStopDetector d;
  d.arm(-10, 10);
  for (int i = 0; i < 250; ++i) d.sample(0);
  Trip first = Trip::None;
  for (int i = 0; i < 5000; ++i) {
    const Trip t = d.sample(1200);
    if (first == Trip::None) first = t;
    if (t == Trip::Hard) break;
    if (d.current() > 600 && d.overCount() > 10) CHECK(t == Trip::Safety);
  }
  CHECK(first == Trip::Bound);
  CHECK(d.trip() == Trip::Bound);  // latched first trip
}

TEST_CASE("EndStopDetector: extreme raw values stay in range") {
  EndStopDetector d;
  d.arm(-340, 340);
  for (int i = 0; i < 400; ++i) d.sample(INT32_MAX);
  CHECK(d.current() > 1000);
  EndStopDetector e;
  e.arm(-340, 340);
  for (int i = 0; i < 400; ++i) e.sample(INT32_MIN);
  CHECK(e.current() < -1000);
  CHECK(e.peak() == -e.current());
}

TEST_CASE("EndStopDetector: idle resets filter, keeps statistics until arm") {
  EndStopDetector d;
  d.arm(-340, 340);
  runUntilTrip(d, 500, 2000);
  const int32_t peak = d.peak();
  REQUIRE(peak > 340);
  d.idle();
  CHECK(d.current() == 0);
  CHECK(d.overCount() == 0);
  CHECK(d.peak() == peak);
  CHECK(d.trip() == Trip::Bound);

  // inrush restarts after idle
  for (int i = 0; i < 250; ++i) CHECK(d.sample(900) == Trip::None);
  CHECK(d.current() == 0);

  d.arm(-340, 340);
  CHECK(d.peak() == 0);
  CHECK(d.trip() == Trip::None);
  CHECK(d.tripCurrent() == 0);
}
