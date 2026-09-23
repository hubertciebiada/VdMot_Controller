// Stub: contract in vdm/line_assembler.h; implemented by the core implementer.
#include "vdm/line_assembler.h"

namespace vdm {

LineAssembler::LineAssembler(char* storage, size_t capacity)
    : buf_(storage),
      cap_(storage ? capacity : 0),
      len_(0),
      ready_(false),
      discarding_(false),
      discardIsOverflow_(false),
      lastWasCr_(false),
      overflows_(0),
      malformed_(0) {
  if (cap_) buf_[0] = '\0';
}

bool LineAssembler::push(char) { return false; }

size_t LineAssembler::feed(const char*, size_t) { return 0; }

void LineAssembler::release() {}

void LineAssembler::reset() {}

}  // namespace vdm
