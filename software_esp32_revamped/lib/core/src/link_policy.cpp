// Stub: contract in vdm/link_policy.h; implemented by the core implementer.
#include "vdm/link_policy.h"

namespace vdm {

const char* linkStateName(LinkState) { return ""; }

LinkPolicy::LinkPolicy(const LinkParams& params) : params_(params) {}

EnqueueResult LinkPolicy::enqueue(const RequestLine&, Priority, uint16_t) {
  return EnqueueResult::Invalid;
}
const RequestLine* LinkPolicy::nextToSend(uint32_t) { return nullptr; }
void LinkPolicy::onSent(uint32_t) {}
bool LinkPolicy::onReply(const Reply&, uint32_t, Completion&) { return false; }
void LinkPolicy::onParseError(uint32_t) {}
bool LinkPolicy::poll(uint32_t, Completion&) { return false; }
bool LinkPolicy::shouldResetStm(uint32_t) const { return false; }
void LinkPolicy::onStmReset(uint32_t, bool) {}
size_t LinkPolicy::suspend() { return 0; }
void LinkPolicy::resume(uint32_t) {}
LinkState LinkPolicy::state(uint32_t) const { return LinkState::Unknown; }
size_t LinkPolicy::queued(Priority) const { return 0; }
uint16_t LinkPolicy::timeoutFor(Cmd) const { return params_.timeoutMs; }

bool RebootDetector::onStatus(const StmStatus&) { return false; }
bool RebootDetector::onValveData(const ValveData&) { return false; }
bool RebootDetector::onLinkState(LinkState, LinkState) { return false; }
void RebootDetector::reset() {}

}  // namespace vdm
