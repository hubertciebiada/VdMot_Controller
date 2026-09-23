// Stub: contract in vdm/poll_planner.h; implemented by the core implementer.
#include "vdm/poll_planner.h"

namespace vdm {

PollPlanner::PollPlanner(const PollCadence& cadence) : cadence_(cadence) {}

void PollPlanner::setProtocol(uint8_t) {}
void PollPlanner::setActiveMask(uint16_t) {}
void PollPlanner::setValveBusy(uint8_t, bool) {}
void PollPlanner::setSensorCounts(uint8_t, uint8_t) {}
void PollPlanner::requestResync() {}
void PollPlanner::requestTempList() {}
void PollPlanner::requestVoltList() {}
void PollPlanner::requestValveSensors() {}
void PollPlanner::requestMotorParams() {}
void PollPlanner::requestTarget(uint8_t) {}
void PollPlanner::requestProfile(uint8_t) {}
bool PollPlanner::next(uint32_t, RequestLine& out) {
  out = RequestLine{};
  return false;
}
void PollPlanner::onResult(const RequestLine&, bool, uint32_t) {}

}  // namespace vdm
