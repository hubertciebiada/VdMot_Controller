#include "vdm/buf_writer.h"

#include <string.h>

namespace vdm {

namespace {
const char kHex[] = "0123456789abcdef";
}  // namespace

BufWriter::BufWriter(char* buf, size_t capacity)
    : buf_(buf), cap_(buf != nullptr ? capacity : 0), len_(0), ok_(true) {
  if (cap_) buf_[0] = '\0';
}

void BufWriter::clear() {
  len_ = 0;
  ok_ = true;
  if (cap_) buf_[0] = '\0';
}

void BufWriter::truncate(size_t len) {
  if (len >= len_) return;
  len_ = len;
  buf_[len_] = '\0';
}

bool BufWriter::appendRaw(const char* s, size_t n) {
  if (cap_ == 0 || n >= cap_ - len_) {
    ok_ = false;
    return false;
  }
  memcpy(buf_ + len_, s, n);
  len_ += n;
  buf_[len_] = '\0';
  return true;
}

bool BufWriter::append(const char* s) {
  if (s == nullptr) {
    ok_ = false;
    return false;
  }
  return appendRaw(s, strlen(s));
}

bool BufWriter::append(char c) { return appendRaw(&c, 1); }

bool BufWriter::appendUnsigned(uint32_t v) {
  char digits[10];  // NOMUTATE: buffer size; all indexing uses sizeof(digits), so a larger buffer is unobservable
  size_t n = 0;
  do {
    digits[sizeof(digits) - 1 - n] = static_cast<char>('0' + v % 10);
    v /= 10;
    ++n;
  } while (v != 0);
  return appendRaw(digits + sizeof(digits) - n, n);
}

bool BufWriter::appendSigned(int32_t v) {
  char text[11];  // NOMUTATE: buffer size; all indexing uses sizeof(text), so a larger buffer is unobservable
  size_t n = 0;
  // Magnitude via unsigned arithmetic so INT32_MIN does not overflow.
  uint32_t magnitude = v < 0 ? 0u - static_cast<uint32_t>(v) : static_cast<uint32_t>(v);
  do {
    text[sizeof(text) - 1 - n] = static_cast<char>('0' + magnitude % 10);
    magnitude /= 10;
    ++n;
  } while (magnitude != 0);
  if (v < 0) {
    text[sizeof(text) - 1 - n] = '-';
    ++n;
  }
  return appendRaw(text + sizeof(text) - n, n);
}

bool BufWriter::appendHex2(uint8_t v) {
  const char text[2] = {kHex[v >> 4], kHex[v & 0x0F]};
  return appendRaw(text, 2);
}

bool BufWriter::appendOneWireAddress(const uint8_t (&addr)[8]) {
  char text[23];
  for (size_t i = 0; i < 8; ++i) {
    text[i * 3] = kHex[addr[i] >> 4];
    text[i * 3 + 1] = kHex[addr[i] & 0x0F];
    if (i < 7) text[i * 3 + 2] = '-';
  }
  return appendRaw(text, sizeof(text));
}

}  // namespace vdm
