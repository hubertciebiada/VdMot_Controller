// Stub: contract in vdm/ha_discovery.h; implemented by the core implementer.
#include "vdm/ha_discovery.h"

#include "vdm/json_writer.h"

namespace vdm {

const char* haComponentName(HaComponent) { return ""; }

DiscoveryIterator::DiscoveryIterator(const DiscoveryContext& ctx) : ctx_(ctx) {}
bool DiscoveryIterator::next(DiscoveryMessage&, JsonWriter&) { return false; }
void DiscoveryIterator::restart() { pos_ = 0; }

DropListIterator::DropListIterator(const DiscoveryContext& ctx) : ctx_(ctx) {}
bool DropListIterator::next(DiscoveryMessage&) { return false; }
void DropListIterator::restart() { pos_ = 0; }

bool discoveryTopicIsCurrent(const DiscoveryContext&, const char*, size_t) { return false; }

}  // namespace vdm
