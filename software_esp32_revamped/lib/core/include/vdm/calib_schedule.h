// Scheduled calibration: weekday bitmask + local hour:minute -> "calibrate
// all valves now" decision, once per calendar-day slot, tolerant to missing
// time, reboots and DST/NTP jumps. Hardware-free.
#pragma once

#include <stddef.h>
#include <stdint.h>

#include "vdm/common.h"
#include "vdm/config.h"

namespace vdm {

// Slot key of a local calendar date: yyyymmdd (e.g. 20260923).
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
//  - dayMask == 0 or hour > 23 or minute > 59: never fires.
//  - Stale bookings: a booked key in the future (clock was wrong) is
//    discarded when a valid time more than 2 days before it is seen.
class CalibScheduler {
 public:
  explicit CalibScheduler(uint16_t graceMinutes = 120, uint32_t noTimeReportMs = 3600000);

  void restoreLastSlot(uint32_t slotKey);
  uint32_t lastSlot() const { return lastSlot_; }

  // Call about every 10 s. `upMs` is time since ESP boot.
  CalibDecision evaluate(const CalibScheduleConfig& cfg, const LocalTime& now, uint32_t upMs);

  // Minutes the firing was late relative to hh:mm (for the event), valid
  // right after evaluate() returned Fire.
  uint16_t lateMinutes() const { return lateMinutes_; }

 private:
  uint16_t graceMinutes_;
  uint32_t noTimeReportMs_;
  uint32_t lastSlot_ = 0;
  bool noTimeReported_ = false;
  uint16_t lateMinutes_ = 0;
};

}  // namespace vdm
