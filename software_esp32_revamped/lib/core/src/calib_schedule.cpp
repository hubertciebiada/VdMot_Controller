// Stub: contract in vdm/calib_schedule.h; implemented by the core implementer.
#include "vdm/calib_schedule.h"

namespace vdm {

uint32_t calibSlotKey(const LocalTime&) { return 0; }

CalibScheduler::CalibScheduler(uint16_t graceMinutes, uint32_t noTimeReportMs)
    : graceMinutes_(graceMinutes), noTimeReportMs_(noTimeReportMs) {}

void CalibScheduler::restoreLastSlot(uint32_t slotKey) { lastSlot_ = slotKey; }

CalibDecision CalibScheduler::evaluate(const CalibScheduleConfig&, const LocalTime&, uint32_t) {
  return CalibDecision::None;
}

}  // namespace vdm
