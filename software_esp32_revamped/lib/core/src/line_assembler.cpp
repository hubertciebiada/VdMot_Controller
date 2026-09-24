#include "vdm/line_assembler.h"

namespace vdm {

namespace {

bool isLineChar(char c) {
  const unsigned char u = static_cast<unsigned char>(c);
  return c == '\t' || (u >= 0x20 && u < 0x7F);
}

}  // namespace

LineAssembler::LineAssembler(char* storage, size_t capacity)
    : buf_(storage),
      cap_(storage ? capacity : 0),
      len_(0),
      ready_(false),
      discarding_(false),
      overflows_(0),
      malformed_(0) {
  if (cap_) buf_[0] = '\0';
}

bool LineAssembler::push(char c) {
  if (ready_) return false;

  // CR, LF and CRLF all end a line; the LF of a CRLF (and any other empty
  // line) finds len_ == 0 and is ignored.
  if (c == '\r' || c == '\n') {
    if (discarding_) {
      discarding_ = false;
    } else if (len_ > 0) {
      ready_ = true;
    }
    return true;
  }

  if (discarding_) return true;

  if (!isLineChar(c)) {
    ++malformed_;
  } else if (len_ + 1 >= cap_) {
    ++overflows_;
  } else {
    buf_[len_++] = c;
    buf_[len_] = '\0';
    return true;
  }
  discarding_ = true;
  len_ = 0;
  if (cap_) buf_[0] = '\0';
  return true;
}

size_t LineAssembler::feed(const char* data, size_t len) {
  if (data == nullptr) return 0;
  size_t used = 0;
  while (used < len && push(data[used])) ++used;
  return used;
}

void LineAssembler::release() {
  ready_ = false;
  len_ = 0;
  if (cap_) buf_[0] = '\0';
}

void LineAssembler::reset() {
  release();
  discarding_ = false;
}

}  // namespace vdm
