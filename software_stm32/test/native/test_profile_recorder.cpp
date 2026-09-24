#include <stdint.h>

#include <initializer_list>

#include <vector>

#include "doctest.h"
#include "vdm/profile_recorder.h"

using vdm::ProfileRecorder;

namespace {

std::vector<uint16_t> counts(const ProfileRecorder& p) {
  std::vector<uint16_t> v;
  for (uint8_t i = 0; i < p.size(); ++i) v.push_back(p.at(i).count);
  return v;
}

}  // namespace

TEST_CASE("ProfileRecorder: empty after reset") {
  ProfileRecorder p;
  CHECK(p.size() == 0);
  CHECK(p.spacing() == 1);
  p.add(5, 100);
  p.reset();
  CHECK(p.size() == 0);
  CHECK(p.spacing() == 1);
  CHECK(p.at(0).count == 0);
  CHECK(p.at(0).current == 0);
}

TEST_CASE("ProfileRecorder: short move keeps every pulse") {
  ProfileRecorder p;
  p.reset();
  for (uint32_t c = 0; c < 10; ++c) p.add(c, static_cast<int32_t>(c * 10));
  REQUIRE(p.size() == 10);
  for (uint8_t i = 0; i < 10; ++i) {
    CHECK(p.at(i).count == i);
    CHECK(p.at(i).current == i * 10);
  }
  CHECK(p.at(10).count == 0);
}

TEST_CASE("ProfileRecorder: repeated polls with the same count record once") {
  ProfileRecorder p;
  p.reset();
  p.add(0, 1);
  p.add(0, 2);
  p.add(0, 3);
  CHECK(p.size() == 1);
  CHECK(p.at(0).current == 1);
}

TEST_CASE("ProfileRecorder: never more than 32 samples, equally spaced") {
  for (uint32_t total : {33u, 64u, 100u, 1000u, 4000u, 65535u}) {
    ProfileRecorder p;
    p.reset();
    for (uint32_t c = 0; c <= total; ++c) p.add(c, 50);
    CHECK(p.size() <= vdm::kProfileSamples);
    CHECK(p.size() >= vdm::kProfileSamples / 2);
    const auto v = counts(p);
    for (size_t i = 1; i < v.size(); ++i) CHECK(v[i] - v[i - 1] == p.spacing());
    CHECK(v.front() == 0);
    // the samples cover the move
    CHECK(static_cast<uint32_t>(v.back()) + 2u * p.spacing() > total);
  }
}

TEST_CASE("ProfileRecorder: coarse polling (several pulses per poll) stays on the grid") {
  for (uint32_t step : {3u, 7u, 50u, 333u}) {
    ProfileRecorder p;
    p.reset();
    for (uint32_t c = 0; c <= 5000; c += step) p.add(c, 42);
    CHECK(p.size() <= vdm::kProfileSamples);
    CHECK(p.size() >= 2);
    const auto v = counts(p);
    const uint32_t s = p.spacing();
    for (size_t i = 0; i < v.size(); ++i) {
      // the first poll at or after a grid point: less than one poll step behind it
      CHECK(v[i] % s < step);
      if (i > 0) CHECK(v[i] / s > v[i - 1] / s);
    }
    // no grid cell that a poll hit is missing
    for (uint32_t c = 0; c <= 5000; c += step) {
      bool found = false;
      for (uint16_t x : v) found = found || (x / s == c / s);
      if (c % s < step) CHECK(found);
    }
  }
}

TEST_CASE("ProfileRecorder: current is stored as magnitude, saturated") {
  ProfileRecorder p;
  p.reset();
  p.add(0, -345);
  p.add(1, 70000);
  p.add(2, INT32_MIN);
  CHECK(p.at(0).current == 345);
  CHECK(p.at(1).current == 65535);
  CHECK(p.at(2).current == 65535);
}

TEST_CASE("ProfileRecorder: counts saturate at 65535") {
  ProfileRecorder p;
  p.reset();
  p.add(70000, 1);
  CHECK(p.size() == 1);
  CHECK(p.at(0).count == 65535);
  p.add(80000, 2);  // same saturated count: not a new sampling point
  CHECK(p.size() == 1);
}

TEST_CASE("ProfileRecorder: decreasing counts are ignored") {
  ProfileRecorder p;
  p.reset();
  p.add(10, 1);
  p.add(5, 2);
  CHECK(p.size() == 1);
  p.finish(3, 9);
  CHECK(p.size() == 1);
  CHECK(p.at(0).current == 1);
}

TEST_CASE("ProfileRecorder: finish appends the stop point") {
  ProfileRecorder p;
  p.reset();
  p.finish(0, 12);  // move that never turned
  REQUIRE(p.size() == 1);
  CHECK(p.at(0).count == 0);
  CHECK(p.at(0).current == 12);

  p.reset();
  for (uint32_t c = 0; c < 5; ++c) p.add(c, 100);
  p.finish(4, -520);  // same count as the last sample: current replaced
  CHECK(p.size() == 5);
  CHECK(p.at(4).current == 520);
  p.finish(9, 610);
  CHECK(p.size() == 6);
  CHECK(p.at(5).count == 9);
  CHECK(p.at(5).current == 610);
}

TEST_CASE("ProfileRecorder: finish on a full buffer compacts first") {
  ProfileRecorder p;
  p.reset();
  for (uint32_t c = 0; c < 32; ++c) p.add(c, 1);
  REQUIRE(p.size() == 32);
  p.finish(40, 700);
  CHECK(p.size() == 17);
  CHECK(p.at(16).count == 40);
  CHECK(p.at(16).current == 700);
  CHECK(p.at(15).count == 30);
  CHECK(p.spacing() == 2);
}

TEST_CASE("ProfileRecorder: spacing saturates on absurdly long moves") {
  ProfileRecorder p;
  p.reset();
  // 65535 pulses with 32 samples need a spacing of 4096 at most
  for (uint32_t c = 0; c <= 0xFFFF; ++c) p.add(c, 1);
  CHECK(p.spacing() <= 4096);
  for (int i = 0; i < 40; ++i) p.finish(0xFFFF, 2);
  CHECK(p.size() <= vdm::kProfileSamples);
}

TEST_CASE("ProfileRecorder: copy keeps the samples") {
  ProfileRecorder p;
  p.reset();
  for (uint32_t c = 0; c < 3; ++c) p.add(c, 5);
  ProfileRecorder q = p;
  p.reset();
  CHECK(q.size() == 3);
  CHECK(q.at(2).count == 2);
}
