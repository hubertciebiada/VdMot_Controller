// calib_schedule: slot keys, window/grace boundaries, weekday mask, minute
// offset, once per date, reboot, DST spring-forward and fall-back, NTP
// steps, stale bookings, missing time reporting, next slot, long runs.
#include <stdint.h>

#include <random>

#include "doctest.h"
#include "vdm/calib_schedule.h"

using namespace vdm;

namespace {

// Weekday by Sakamoto's method (independent of the implementation).
uint8_t weekday(int y, int m, int d) {
  static const int t[] = {0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4};
  if (m < 3) y -= 1;
  return static_cast<uint8_t>((y + y / 4 - y / 100 + y / 400 + t[m - 1] + d) % 7);
}

LocalTime at(int y, int mo, int d, int h, int mi, int s = 0) {
  LocalTime t;
  t.valid = true;
  t.year = static_cast<uint16_t>(y);
  t.month = static_cast<uint8_t>(mo);
  t.mday = static_cast<uint8_t>(d);
  t.wday = weekday(y, mo, d);
  t.hour = static_cast<uint8_t>(h);
  t.minute = static_cast<uint8_t>(mi);
  t.second = static_cast<uint8_t>(s);
  return t;
}

CalibScheduleConfig cfg(uint8_t mask, uint8_t hour, uint8_t minute = 0) {
  CalibScheduleConfig c;
  c.dayMask = mask;
  c.hour = hour;
  c.minute = minute;
  return c;
}

constexpr uint8_t kSun = 1u << 0;
constexpr uint8_t kMon = 1u << 1;
constexpr uint8_t kWed = 1u << 3;
constexpr uint8_t kThu = 1u << 4;
constexpr uint8_t kFri = 1u << 5;
constexpr uint8_t kAll = 0x7F;
constexpr uint32_t kUp = 5000;  // uptime well below the no-time report

bool isLeap(int y) { return (y % 4 == 0 && y % 100 != 0) || y % 400 == 0; }
int monthDays(int y, int m) {
  static const int d[] = {31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31};
  return m == 2 && isLeap(y) ? 29 : d[m - 1];
}
void nextDay(int& y, int& m, int& d) {
  if (++d > monthDays(y, m)) {
    d = 1;
    if (++m > 12) {
      m = 1;
      ++y;
    }
  }
}

}  // namespace

TEST_CASE("calib: weekday helper sanity") {
  CHECK(weekday(2026, 9, 23) == 3);   // Wednesday
  CHECK(weekday(2026, 9, 27) == 0);   // Sunday
  CHECK(weekday(2026, 12, 31) == 4);  // Thursday
  CHECK(weekday(2000, 1, 1) == 6);    // Saturday
}

TEST_CASE("calib: slot key of valid and invalid local times") {
  CHECK(calibSlotKey(at(2026, 9, 23, 14, 3, 5)) == 20260923u);
  CHECK(calibSlotKey(at(2000, 1, 1, 0, 0)) == 20000101u);
  CHECK(calibSlotKey(at(9999, 12, 31, 23, 59, 60)) == 99991231u);
  CHECK(calibSlotKey(at(2028, 2, 29, 0, 0)) == 20280229u);
  CHECK(calibSlotKey(at(2000, 2, 29, 0, 0)) == 20000229u);

  LocalTime t = at(2026, 9, 23, 0, 0);
  t.valid = false;
  CHECK(calibSlotKey(t) == 0);
  t = at(2026, 9, 23, 0, 0);
  t.wday = 4;
  CHECK(calibSlotKey(t) == 0);
  t = at(2026, 9, 23, 24, 0);
  CHECK(calibSlotKey(t) == 0);
  t = at(2026, 9, 23, 23, 60);
  CHECK(calibSlotKey(t) == 0);
  t = at(2026, 9, 23, 23, 59, 61);
  CHECK(calibSlotKey(t) == 0);

  LocalTime bad = at(2027, 3, 1, 0, 0);
  bad.month = 2;
  bad.mday = 29;  // 2027 is not a leap year
  CHECK(calibSlotKey(bad) == 0);
  bad = at(1900, 3, 1, 0, 0);
  bad.month = 2;
  bad.mday = 29;
  CHECK(calibSlotKey(bad) == 0);
  bad = at(2026, 4, 30, 0, 0);
  bad.mday = 31;
  CHECK(calibSlotKey(bad) == 0);
  bad = at(2026, 1, 1, 0, 0);
  bad.mday = 0;
  CHECK(calibSlotKey(bad) == 0);
  bad = at(2026, 1, 1, 0, 0);
  bad.month = 0;
  CHECK(calibSlotKey(bad) == 0);
  bad = at(2026, 1, 1, 0, 0);
  bad.month = 13;
  CHECK(calibSlotKey(bad) == 0);
  bad = at(2026, 1, 1, 0, 0);
  bad.year = 1999;
  CHECK(calibSlotKey(bad) == 0);
  bad = at(2026, 1, 1, 0, 0);
  bad.year = 10000;
  CHECK(calibSlotKey(bad) == 0);
  CHECK(calibSlotKey(at(1999, 12, 31, 0, 0)) == 0);
  CHECK(calibSlotKey(at(2026, 1, 31, 0, 0)) == 20260131u);
  LocalTime feb = at(2026, 3, 1, 0, 0);
  feb.month = 2;
  feb.mday = 28;
  feb.wday = weekday(2026, 2, 28);
  CHECK(calibSlotKey(feb) == 20260228u);
}

TEST_CASE("calib: last day of every month and century leap rules") {
  for (int y : {2026, 2028, 2100, 2400, 2104}) {
    for (int m = 1; m <= 12; ++m) {
      CAPTURE(y);
      CAPTURE(m);
      const int last = monthDays(y, m);
      CHECK(calibSlotKey(at(y, m, last, 0, 0)) == static_cast<uint32_t>(y * 10000 + m * 100 + last));
      LocalTime over = at(y, m, last, 0, 0);
      over.mday = static_cast<uint8_t>(last + 1);
      for (uint8_t w = 0; w < 7; ++w) {
        over.wday = w;
        CHECK(calibSlotKey(over) == 0);
      }
    }
  }
  CHECK(monthDays(2100, 2) == 28);
  CHECK(monthDays(2400, 2) == 29);
  // Weekdays across centuries come from the date, not from a cache.
  CHECK(calibSlotKey(at(2100, 3, 1, 0, 0)) == 21000301u);
  CHECK(calibSlotKey(at(9999, 1, 1, 0, 0)) == 99990101u);
}

TEST_CASE("calib: dates across centuries") {
  // Every 13th day from 2000 to 2800: key, weekday check and next-day
  // arithmetic agree with an independent calendar.
  int y = 2000, m = 1, d = 1;
  int count = 0;
  while (y < 2800) {
    for (int i = 0; i < 13; ++i) nextDay(y, m, d);
    int ny = y, nm = m, nd = d;
    nextDay(ny, nm, nd);
    CalibScheduler s;
    const LocalTime t = at(y, m, d, 12, 0);
    REQUIRE(calibSlotKey(t) == static_cast<uint32_t>(y * 10000 + m * 100 + d));
    CHECK(s.nextSlot(cfg(kAll, 3), t) == static_cast<uint32_t>(ny * 10000 + nm * 100 + nd));
    const uint8_t wd = weekday(y, m, d);
    CHECK(s.nextSlot(cfg(static_cast<uint8_t>(1u << wd), 3), t) != 0);
    ++count;
  }
  CHECK(count > 20000);
}

TEST_CASE("calib: every day around 400-year era boundaries") {
  // The civil-date arithmetic has its edge cases at the end of each 400-year
  // era (Feb 29 of 2000/2400) and around century years.
  const int spans[][2] = {{2000, 2001}, {2099, 2101}, {2399, 2401}};
  for (const auto& sp : spans) {
    int y = sp[0], m = 1, d = 1;
    while (y < sp[1] + 1) {
      int ny = y, nm = m, nd = d;
      nextDay(ny, nm, nd);
      CalibScheduler s;
      CHECK(s.nextSlot(cfg(kAll, 3), at(y, m, d, 12, 0)) ==
            static_cast<uint32_t>(ny * 10000 + nm * 100 + nd));
      y = ny;
      m = nm;
      d = nd;
    }
  }
}

TEST_CASE("calib: impossible dates are rejected for every weekday value") {
  struct D {
    int y, m, d;
  };
  const D bad[] = {{10000, 1, 1}, {1999, 12, 31}, {2026, 13, 1}, {2026, 0, 1}, {2026, 1, 0},
                   {2026, 1, 32}, {2100, 2, 29}, {2026, 2, 29}, {2026, 4, 31}};
  for (const D& b : bad) {
    CAPTURE(b.y);
    CAPTURE(b.m);
    CAPTURE(b.d);
    LocalTime t = at(2026, 1, 1, 0, 0);
    t.year = static_cast<uint16_t>(b.y);
    t.month = static_cast<uint8_t>(b.m);
    t.mday = static_cast<uint8_t>(b.d);
    for (uint8_t w = 0; w < 7; ++w) {
      t.wday = w;
      CHECK(calibSlotKey(t) == 0);
    }
    CalibScheduler s;
    s.restoreLastSlot(static_cast<uint32_t>(b.y * 10000 + b.m * 100 + b.d));
    CHECK(s.lastSlot() == 0);
  }
  CHECK(calibSlotKey(at(9999, 12, 31, 0, 0)) != 0);
}

TEST_CASE("calib: fires once in the window on a selected day") {
  CalibScheduler s;
  const CalibScheduleConfig c = cfg(kSun | kWed, 3, 15);
  CHECK(s.lastSlot() == 0);
  CHECK(s.evaluate(c, at(2026, 9, 23, 3, 14, 59), kUp) == CalibDecision::None);
  CHECK(s.evaluate(c, at(2026, 9, 23, 3, 15, 0), kUp) == CalibDecision::Fire);
  CHECK(s.lateMinutes() == 0);
  CHECK(s.lastSlot() == 20260923u);
  CHECK(s.evaluate(c, at(2026, 9, 23, 3, 15, 10), kUp) == CalibDecision::None);
  CHECK(s.lateMinutes() == 0);
  CHECK(s.evaluate(c, at(2026, 9, 23, 5, 14), kUp) == CalibDecision::None);
  // Thursday is not selected.
  CHECK(s.evaluate(c, at(2026, 9, 24, 3, 15), kUp) == CalibDecision::None);
  CHECK(s.lastSlot() == 20260923u);
  // Sunday is.
  CHECK(s.evaluate(c, at(2026, 9, 27, 3, 16), kUp) == CalibDecision::Fire);
  CHECK(s.lateMinutes() == 1);
  CHECK(s.lastSlot() == 20260927u);
}

TEST_CASE("calib: grace window boundaries") {
  const CalibScheduleConfig c = cfg(kAll, 3, 15);
  {
    CalibScheduler s;
    CHECK(s.evaluate(c, at(2026, 9, 23, 5, 14, 59), kUp) == CalibDecision::Fire);  // +119
    CHECK(s.lateMinutes() == 119);
  }
  {
    CalibScheduler s;
    CHECK(s.evaluate(c, at(2026, 9, 23, 5, 15), kUp) == CalibDecision::None);  // +120
    CHECK(s.lastSlot() == 0);
  }
  {
    CalibScheduler s(10);
    CHECK(s.evaluate(c, at(2026, 9, 23, 3, 25), kUp) == CalibDecision::None);
    CHECK(s.evaluate(c, at(2026, 9, 23, 3, 24), kUp) == CalibDecision::Fire);
    CHECK(s.lateMinutes() == 9);
  }
  {
    CalibScheduler s(0);  // treated as 1 minute
    CHECK(s.evaluate(c, at(2026, 9, 23, 3, 16), kUp) == CalibDecision::None);
    CHECK(s.evaluate(c, at(2026, 9, 23, 3, 14), kUp) == CalibDecision::None);
    CHECK(s.evaluate(c, at(2026, 9, 23, 3, 15, 59), kUp) == CalibDecision::Fire);
  }
  {
    CalibScheduler s(1440);
    CHECK(s.evaluate(c, at(2026, 9, 23, 23, 59), kUp) == CalibDecision::Fire);
    CHECK(s.lateMinutes() == 20 * 60 + 44);
  }
  {
    // The window never crosses midnight into the next date.
    CalibScheduler s;
    const CalibScheduleConfig late = cfg(kWed, 23, 30);
    CHECK(s.evaluate(late, at(2026, 9, 24, 0, 10), kUp) == CalibDecision::None);
    CHECK(s.evaluate(late, at(2026, 9, 23, 23, 59), kUp) == CalibDecision::Fire);
    CHECK(s.lateMinutes() == 29);
  }
}

TEST_CASE("calib: every weekday bit maps to tm_wday") {
  // 2026-09-27 is a Sunday; the next six days cover Monday..Saturday.
  for (int bit = 0; bit < 7; ++bit) {
    CAPTURE(bit);
    CalibScheduler s;
    const CalibScheduleConfig c = cfg(static_cast<uint8_t>(1u << bit), 0, 0);
    int fired = 0;
    int y = 2026, m = 9, d = 27;
    for (int day = 0; day < 7; ++day) {
      if (s.evaluate(c, at(y, m, d, 0, 0), kUp) == CalibDecision::Fire) {
        ++fired;
        CHECK(weekday(y, m, d) == bit);
      }
      nextDay(y, m, d);
    }
    CHECK(fired == 1);
  }
}

TEST_CASE("calib: disabled schedules never fire") {
  const CalibScheduleConfig off[] = {cfg(0, 3), cfg(0x80, 3), cfg(kAll, 24), cfg(kAll, 3, 60),
                                     cfg(kAll, 255, 255)};
  for (const CalibScheduleConfig& c : off) {
    CalibScheduler s;
    for (int h = 0; h < 24; ++h) {
      CHECK(s.evaluate(c, at(2026, 9, 23, h, 0), kUp) == CalibDecision::None);
    }
    LocalTime none;
    CHECK(s.evaluate(c, none, 7200000) == CalibDecision::None);  // no report either
  }
  // Bit 7 is ignored, the other bits still count.
  CalibScheduler s;
  CHECK(s.evaluate(cfg(0x80 | kWed, 1), at(2026, 9, 23, 1, 0), kUp) == CalibDecision::Fire);
  // Boundaries that are still enabled.
  CalibScheduler s2;
  CHECK(s2.evaluate(cfg(kAll, 23, 59), at(2026, 9, 23, 23, 59), kUp) == CalibDecision::Fire);
}

TEST_CASE("calib: reboot inside the window does not fire again") {
  const CalibScheduleConfig c = cfg(kWed, 3);
  CalibScheduler a;
  CHECK(a.evaluate(c, at(2026, 9, 23, 3, 1), kUp) == CalibDecision::Fire);
  CalibScheduler b;  // after reboot
  b.restoreLastSlot(a.lastSlot());
  CHECK(b.lastSlot() == 20260923u);
  CHECK(b.evaluate(c, at(2026, 9, 23, 3, 30), kUp) == CalibDecision::None);
  CHECK(b.evaluate(c, at(2026, 9, 30, 3, 30), kUp) == CalibDecision::Fire);
}

TEST_CASE("calib: restoring an invalid key is ignored") {
  const uint32_t bad[] = {0, 1, 20261301, 20260001, 20260230, 20260100, 99999999u,
                          0xFFFFFFFFu, 19991231};
  for (uint32_t k : bad) {
    CAPTURE(k);
    CalibScheduler s;
    s.restoreLastSlot(20260101);
    s.restoreLastSlot(k);
    CHECK(s.lastSlot() == 0);
  }
  CalibScheduler s;
  s.restoreLastSlot(20280229);
  CHECK(s.lastSlot() == 20280229u);
  s.restoreLastSlot(99991231);
  CHECK(s.lastSlot() == 99991231u);
}

TEST_CASE("calib: DST spring forward fires after the gap, fall back fires once") {
  // EU 2026: 29 March 02:00 -> 03:00 (Sunday), 25 October 03:00 -> 02:00 (Sunday).
  const CalibScheduleConfig c = cfg(kSun, 2, 30);
  CalibScheduler s;
  CHECK(s.evaluate(c, at(2026, 3, 29, 1, 59, 50), kUp) == CalibDecision::None);
  CHECK(s.evaluate(c, at(2026, 3, 29, 3, 0, 0), kUp) == CalibDecision::Fire);
  CHECK(s.lateMinutes() == 30);

  CalibScheduler f;
  CHECK(f.evaluate(c, at(2026, 10, 25, 2, 30), kUp) == CalibDecision::Fire);
  CHECK(f.evaluate(c, at(2026, 10, 25, 2, 59), kUp) == CalibDecision::None);
  // Clock goes back to 02:00 and passes 02:30 a second time.
  for (int m = 0; m < 60; ++m) {
    CHECK(f.evaluate(c, at(2026, 10, 25, 2, m), kUp) == CalibDecision::None);
  }
  CHECK(f.lastSlot() == 20261025u);
}

TEST_CASE("calib: NTP steps and stale bookings") {
  const CalibScheduleConfig c = cfg(kAll, 3);
  SUBCASE("a step back to an earlier date never re-fires a booked date") {
    CalibScheduler s;
    CHECK(s.evaluate(c, at(2026, 9, 23, 3, 0), kUp) == CalibDecision::Fire);
    CHECK(s.evaluate(c, at(2026, 9, 22, 3, 0), kUp) == CalibDecision::None);  // 1 day back
    CHECK(s.lastSlot() == 20260923u);
    CHECK(s.evaluate(c, at(2026, 9, 21, 3, 0), kUp) == CalibDecision::None);  // 2 days back
    CHECK(s.lastSlot() == 20260923u);
    CHECK(s.evaluate(c, at(2026, 9, 24, 3, 0), kUp) == CalibDecision::Fire);
  }
  SUBCASE("a booking more than 2 days in the future is discarded") {
    CalibScheduler s;
    s.restoreLastSlot(20260930);  // written while the clock was wrong
    CHECK(s.evaluate(c, at(2026, 9, 27, 3, 0), kUp) == CalibDecision::Fire);
    CHECK(s.lastSlot() == 20260927u);
  }
  SUBCASE("exactly 2 days ahead is kept") {
    CalibScheduler s;
    s.restoreLastSlot(20260930);
    CHECK(s.evaluate(c, at(2026, 9, 28, 3, 0), kUp) == CalibDecision::None);
    CHECK(s.lastSlot() == 20260930u);
  }
  SUBCASE("discarding happens outside the window too, and across months/years") {
    CalibScheduler s;
    s.restoreLastSlot(20270102);
    CHECK(s.evaluate(c, at(2026, 12, 31, 12, 0), kUp) == CalibDecision::None);
    CHECK(s.lastSlot() == 20270102u);  // 2 days: kept
    s.restoreLastSlot(20270103);
    CHECK(s.evaluate(c, at(2026, 12, 31, 12, 0), kUp) == CalibDecision::None);
    CHECK(s.lastSlot() == 0);  // 3 days: discarded
    // A far-future booking is discarded even with the schedule off.
    s.restoreLastSlot(20990101);
    CHECK(s.evaluate(cfg(0, 3), at(2026, 12, 31, 12, 0), kUp) == CalibDecision::None);
    CHECK(s.lastSlot() == 0);
  }
  SUBCASE("a step forward past the window skips that date") {
    CalibScheduler s;
    CHECK(s.evaluate(c, at(2026, 9, 23, 2, 59), kUp) == CalibDecision::None);
    CHECK(s.evaluate(c, at(2026, 9, 23, 5, 0), kUp) == CalibDecision::None);
    CHECK(s.lastSlot() == 0);
  }
}

TEST_CASE("calib: missing time is reported once per boot after the delay") {
  CalibScheduler s(120, 3600000);
  const CalibScheduleConfig c = cfg(kAll, 3);
  LocalTime none;
  CHECK(s.evaluate(c, none, 0) == CalibDecision::None);
  CHECK(s.evaluate(c, none, 3600000) == CalibDecision::None);
  CHECK(s.evaluate(c, none, 3600001) == CalibDecision::SkippedNoTime);
  CHECK(s.evaluate(c, none, 3600002) == CalibDecision::None);
  CHECK(s.evaluate(c, none, 99999999) == CalibDecision::None);
  // Garbage fields count as no time.
  LocalTime garbage = at(2026, 9, 23, 3, 0);
  garbage.wday = 6;
  CHECK(s.evaluate(c, garbage, 99999999) == CalibDecision::None);
  CHECK(s.lastSlot() == 0);
  // Once time is valid the schedule works normally.
  CHECK(s.evaluate(c, at(2026, 9, 23, 3, 0), 99999999) == CalibDecision::Fire);

  CalibScheduler g(120, 3600000);
  CHECK(g.evaluate(c, garbage, 3600001) == CalibDecision::SkippedNoTime);

  CalibScheduler quick(120, 0);
  CHECK(quick.evaluate(c, none, 0) == CalibDecision::None);
  CHECK(quick.evaluate(c, none, 1) == CalibDecision::SkippedNoTime);

  // A report is not consumed while the schedule is off.
  CalibScheduler off(120, 10);
  CHECK(off.evaluate(cfg(0, 3), none, 20) == CalibDecision::None);
  CHECK(off.evaluate(c, none, 20) == CalibDecision::SkippedNoTime);
}

TEST_CASE("calib: lateMinutes is only meaningful after Fire") {
  CalibScheduler s;
  const CalibScheduleConfig c = cfg(kAll, 3);
  CHECK(s.evaluate(c, at(2026, 9, 23, 4, 5), kUp) == CalibDecision::Fire);
  CHECK(s.lateMinutes() == 65);
  CHECK(s.evaluate(c, at(2026, 9, 23, 4, 6), kUp) == CalibDecision::None);
  CHECK(s.lateMinutes() == 0);
  CHECK(s.evaluate(c, at(2026, 9, 24, 3, 7), kUp) == CalibDecision::Fire);
  CHECK(s.lateMinutes() == 7);
  LocalTime none;
  CHECK(s.evaluate(c, none, kUp) == CalibDecision::None);
  CHECK(s.lateMinutes() == 0);
}

TEST_CASE("calib: next slot") {
  const CalibScheduleConfig c = cfg(kSun | kWed, 0, 0);
  CalibScheduler s;
  // Wednesday 2026-09-23: window 00:00..02:00.
  CHECK(s.nextSlot(c, at(2026, 9, 23, 1, 59)) == 20260923u);
  CHECK(s.nextSlot(c, at(2026, 9, 23, 2, 0)) == 20260927u);
  CHECK(s.nextSlot(c, at(2026, 9, 22, 23, 0)) == 20260923u);
  CHECK(s.evaluate(c, at(2026, 9, 23, 0, 30), kUp) == CalibDecision::Fire);
  CHECK(s.nextSlot(c, at(2026, 9, 23, 0, 31)) == 20260927u);
  // Month and year roll-over.
  CalibScheduler t;
  CHECK(t.nextSlot(cfg(kThu, 5), at(2026, 9, 30, 6, 0)) == 20261001u);
  CHECK(t.nextSlot(cfg(kFri, 5), at(2026, 12, 31, 6, 0)) == 20270101u);
  CHECK(t.nextSlot(cfg(kThu, 5), at(2026, 12, 31, 7, 0)) == 20270107u);  // 7 days ahead
  CHECK(t.nextSlot(cfg(kThu, 5), at(2026, 12, 31, 6, 59)) == 20261231u);
  CHECK(t.nextSlot(cfg(kMon, 5), at(2028, 2, 28, 8, 0)) == 20280306u);  // leap year
  CHECK(t.nextSlot(cfg(kMon, 5), at(2028, 2, 28, 6, 0)) == 20280228u);
  // Off or no time.
  CHECK(t.nextSlot(cfg(0, 5), at(2026, 12, 31, 6, 0)) == 0);
  CHECK(t.nextSlot(cfg(kAll, 24), at(2026, 12, 31, 6, 0)) == 0);
  CHECK(t.nextSlot(c, LocalTime{}) == 0);
  // A booking in the (near) future is skipped.
  CalibScheduler u;
  u.restoreLastSlot(20260927);
  CHECK(u.nextSlot(c, at(2026, 9, 26, 0, 0)) == 20260930u);
  // Every date within 7 days booked -> 0.
  CalibScheduler v;
  v.restoreLastSlot(20261230);  // today .. today+7 booked, today+8 is not looked at
  CHECK(v.nextSlot(cfg(kAll, 5), at(2026, 12, 23, 6, 0)) == 0);
  v.restoreLastSlot(20261229);
  CHECK(v.nextSlot(cfg(kAll, 5), at(2026, 12, 23, 6, 0)) == 20261230u);
  // Minutes count: window 05:30..07:30.
  CalibScheduler w;
  CHECK(w.nextSlot(cfg(kAll, 5, 30), at(2026, 12, 23, 7, 29)) == 20261223u);
  CHECK(w.nextSlot(cfg(kAll, 5, 30), at(2026, 12, 23, 7, 30)) == 20261224u);
  CHECK(w.nextSlot(cfg(kAll, 5, 30), at(2026, 12, 23, 5, 29)) == 20261223u);
}

TEST_CASE("calib: a long run fires exactly once per selected date" * doctest::test_suite("fuzz")) {
  // Every minute for 3 years (incl. a leap day), with random reboots.
  std::mt19937 rng(8080);
  const CalibScheduleConfig c = cfg(kSun | kWed, 3, 45);
  CalibScheduler s;
  uint32_t persisted = 0;
  int fires = 0, expected = 0, skipped = 0;
  int y = 2026, m = 1, d = 1;
  while (y < 2029) {
    const uint8_t wd = weekday(y, m, d);
    if (wd == 0 || wd == 3) ++expected;
    int firedToday = 0;
    for (int minute = 0; minute < 1440; ++minute) {
      if (rng() % 5000 == 0) {  // ESP reboot
        s = CalibScheduler();
        s.restoreLastSlot(persisted);
      }
      const CalibDecision dec = s.evaluate(c, at(y, m, d, minute / 60, minute % 60), kUp);
      if (dec == CalibDecision::Fire) {
        ++fires;
        ++firedToday;
        persisted = s.lastSlot();
        CHECK(minute >= 3 * 60 + 45);
        CHECK(minute < 3 * 60 + 45 + 120);
        CHECK(s.lastSlot() == static_cast<uint32_t>(y * 10000 + m * 100 + d));
      }
      if (dec == CalibDecision::SkippedNoTime) ++skipped;
    }
    CHECK(firedToday <= 1);
    nextDay(y, m, d);
  }
  CHECK(skipped == 0);
  CHECK(fires == expected);
  CHECK(expected > 300);
}
