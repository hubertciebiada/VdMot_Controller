// Scheduled calibration: weekday bitmask + local hour:minute -> "calibrate
// all valves now" decision, once per calendar-day slot, tolerant to missing
// time, reboots and DST/NTP jumps. Hardware-free.
#pragma once

#include <stddef.h>
#include <stdint.h>

#include "vdm/common.h"
#include "vdm/config.h"

namespace vdm {

// Slot key of a local calendar date: yyyymmdd (e.g. 20260923). 0 when `t`
// is not valid: valid flag clear, date outside 2000..9999 or impossible
// (Feb 30), hour/minute/second out of range, or wday not matching the date.
uint32_t calibSlotKey(const LocalTime& t);

enum class CalibDecision : uint8_t {
  None,           // nothing to do
  Fire,           // send "staln 255" now; slot booked
  SkippedNoTime,  // today's slot window passed (or is passing) without valid time; reported once
};

// Rules (DESIGN.md "Calibration schedule"):
//  - A slot exists on local dates whose weekday bit is set in dayMask.
//  - The slot fires when local time is in [hh:mm, hh:mm + graceMinutes)
//    (same local date) and the slot key is newer than the last booked key.
//    A spring-forward gap that skips hh:mm still fires at the first minute
//    after it; a fall-back repetition does not fire twice (same date key).
//  - An NTP step backwards to an earlier date never re-fires a booked date
//    (keys only move forward).
//  - The booked key is persisted by glue (restoreLastSlot at boot), so an ESP
//    reboot inside the window does not calibrate again.
//  - While time is invalid nothing fires. If time is still invalid when the
//    scheduler is evaluated and the ESP has been up longer than
//    noTimeReportMs, SkippedNoTime is returned once per ESP boot.
//  - dayMask == 0 (bit 7 is ignored) or hour > 23 or minute > 59: never
//    fires. graceMinutes 0 is treated as 1.
//  - A local time that calibSlotKey() rejects counts as "no valid time".
//  - Stale bookings: a booked key in the future (clock was wrong) is
//    discarded when a valid time more than 2 days before it is seen.
class CalibScheduler {
 public:
  explicit CalibScheduler(uint16_t graceMinutes = 120, uint32_t noTimeReportMs = 3600000);

  // A key that is not a valid yyyymmdd date is ignored (-> 0).
  void restoreLastSlot(uint32_t slotKey);
  uint32_t lastSlot() const { return lastSlot_; }

  // Call about every 10 s. `upMs` is time since ESP boot.
  CalibDecision evaluate(const CalibScheduleConfig& cfg, const LocalTime& now, uint32_t upMs);

  // Minutes the firing was late relative to hh:mm (for the event), valid
  // right after evaluate() returned Fire.
  uint16_t lateMinutes() const { return lateMinutes_; }

  // yyyymmdd of the next date whose slot will still fire (today while its
  // window is open and it is not booked yet), looking up to 7 days ahead;
  // 0 when the schedule is off or the time is not valid. For /api/status.
  uint32_t nextSlot(const CalibScheduleConfig& cfg, const LocalTime& now) const;

 private:
  uint16_t graceMinutes_;
  uint32_t noTimeReportMs_;
  uint32_t lastSlot_ = 0;
  bool noTimeReported_ = false;
  uint16_t lateMinutes_ = 0;
};

}  // namespace vdm
