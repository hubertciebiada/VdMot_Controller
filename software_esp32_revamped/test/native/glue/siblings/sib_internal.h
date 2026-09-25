// Shared state of the sibling fakes that tests do not use directly.
#pragma once

#include <vdm/event_log.h>

namespace sib {

// The RAM log behind the logger fake (read(), readSince(), lastSeq()).
vdm::EventLog& eventLog();

}  // namespace sib
