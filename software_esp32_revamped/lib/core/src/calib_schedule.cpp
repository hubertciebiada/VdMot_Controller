#include "vdm/calib_schedule.h"

namespace vdm {

namespace {

constexpr uint16_t kMinYear = 2000;
constexpr uint16_t kMaxYear = 9999;

bool isLeap(uint32_t y) { return (y % 4 == 0 && y % 100 != 0) || y % 400 == 0; }

uint8_t daysInMonth(uint32_t y, uint32_t m) {
  static const uint8_t kDays[] = {31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31};
  return (m == 2 && isLeap(y)) ? 29 : kDays[m - 1];
}

bool dateValid(uint32_t y, uint32_t m, uint32_t d) {
  return y >= kMinYear && y <= kMaxYear && m >= 1 && m <= 12 && d >= 1 && d <= daysInMonth(y, m);
}

// Days since 1970-01-01 of a valid date (H. Hinnant's days_from_civil).
int32_t daysFromCivil(uint32_t y, uint32_t m, uint32_t d) {
  const int32_t yy = static_cast<int32_t>(y) - (m <= 2 ? 1 : 0);
  const int32_t era = yy / 400;
  const int32_t yoe = yy - era * 400;
  const int32_t mp = static_cast<int32_t>(m) + (m > 2 ? -3 : 9);
  const int32_t doy = (153 * mp + 2) / 5 + static_cast<int32_t>(d) - 1;
  const int32_t doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
  return era * 146097 + doe - 719468;
}

// Inverse of daysFromCivil, as a yyyymmdd key.
uint32_t keyFromDays(int32_t z) {
  z += 719468;
  const int32_t era = z / 146097;
  const int32_t doe = z - era * 146097;
  const int32_t yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
  const int32_t doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
  const int32_t mp = (5 * doy + 2) / 153;
  const int32_t d = doy - (153 * mp + 2) / 5 + 1;
  const int32_t m = mp < 10 ? mp + 3 : mp - 9;
  const int32_t y = yoe + era * 400 + (m <= 2 ? 1 : 0);
  return static_cast<uint32_t>(y) * 10000u + static_cast<uint32_t>(m) * 100u +
         static_cast<uint32_t>(d);
}

// 1970-01-01 was a Thursday (4).
uint8_t weekdayFromDays(int32_t days) { return static_cast<uint8_t>((days + 4) % 7); }

bool keyValid(uint32_t key) { return dateValid(key / 10000u, (key / 100u) % 100u, key % 100u); }

// Days since 1970-01-01 of a valid yyyymmdd key.
int32_t daysFromKey(uint32_t key) {
  return daysFromCivil(key / 10000u, (key / 100u) % 100u, key % 100u);
}

// Every field in range and the weekday consistent with the date.
bool localTimeValid(const LocalTime& t) {
  if (!t.valid || !dateValid(t.year, t.month, t.mday)) return false;
  if (t.hour > 23 || t.minute > 59 || t.second > 60) return false;
  return t.wday == weekdayFromDays(daysFromCivil(t.year, t.month, t.mday));
}

bool scheduleEnabled(const CalibScheduleConfig& cfg) {
  return (cfg.dayMask & 0x7Fu) != 0 && cfg.hour <= 23 && cfg.minute <= 59;
}

}  // namespace

uint32_t calibSlotKey(const LocalTime& t) {
  if (!localTimeValid(t)) return 0;
  return static_cast<uint32_t>(t.year) * 10000u + static_cast<uint32_t>(t.month) * 100u + t.mday;
}

CalibScheduler::CalibScheduler(uint16_t graceMinutes, uint32_t noTimeReportMs)
    : graceMinutes_(graceMinutes == 0 ? 1 : graceMinutes), noTimeReportMs_(noTimeReportMs) {}

void CalibScheduler::restoreLastSlot(uint32_t slotKey) {
  lastSlot_ = keyValid(slotKey) ? slotKey : 0;
}

CalibDecision CalibScheduler::evaluate(const CalibScheduleConfig& cfg, const LocalTime& now,
                                       uint32_t upMs) {
  lateMinutes_ = 0;
  const bool enabled = scheduleEnabled(cfg);
  const uint32_t key = calibSlotKey(now);
  if (key == 0) {
    if (enabled && !noTimeReported_ && upMs > noTimeReportMs_) {
      noTimeReported_ = true;
      return CalibDecision::SkippedNoTime;
    }
    return CalibDecision::None;
  }

  const int32_t today = daysFromKey(key);
  // lastSlot_ is 0 or a valid key (restoreLastSlot / evaluate).
  if (lastSlot_ != 0 && daysFromKey(lastSlot_) - today > 2) lastSlot_ = 0;

  if (!enabled || (cfg.dayMask & (1u << now.wday)) == 0 || key <= lastSlot_) {
    return CalibDecision::None;
  }
  const uint32_t nowMin = now.hour * 60u + now.minute;
  const uint32_t slotMin = cfg.hour * 60u + cfg.minute;
  if (nowMin < slotMin || nowMin >= slotMin + graceMinutes_) return CalibDecision::None;
  lastSlot_ = key;
  lateMinutes_ = static_cast<uint16_t>(nowMin - slotMin);
  return CalibDecision::Fire;
}

uint32_t CalibScheduler::nextSlot(const CalibScheduleConfig& cfg, const LocalTime& now) const {
  const uint32_t key = calibSlotKey(now);
  if (key == 0 || !scheduleEnabled(cfg)) return 0;
  const int32_t today = daysFromKey(key);
  const uint32_t nowMin = now.hour * 60u + now.minute;
  const uint32_t slotMin = cfg.hour * 60u + cfg.minute;
  for (int32_t d = 0; d <= 7; ++d) {
    const uint32_t k = keyFromDays(today + d);
    if ((cfg.dayMask & (1u << weekdayFromDays(today + d))) == 0 || k <= lastSlot_) continue;
    if (d == 0 && nowMin >= slotMin + graceMinutes_) continue;
    return k;
  }
  return 0;
}

}  // namespace vdm
