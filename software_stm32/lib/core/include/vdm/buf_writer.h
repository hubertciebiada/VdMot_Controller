// Bounded text formatting into a caller-provided buffer. Hardware-free.
#pragma once

#include <stddef.h>
#include <stdint.h>

namespace vdm {

// Appends text to a fixed buffer that is always NUL-terminated.
// Each append is all-or-nothing: if the piece does not fit, nothing of it is
// written, the call returns false and ok() stays false until clear().
class BufWriter {
 public:
  // capacity includes the terminating NUL; a capacity of 0 (or a null
  // buffer) makes every append fail.
  BufWriter(char* buf, size_t capacity);

  bool append(const char* s);
  bool append(char c);
  bool appendUnsigned(uint32_t v);
  bool appendSigned(int32_t v);
  // Two lowercase hex digits.
  bool appendHex2(uint8_t v);
  // 1-Wire ROM address as "28-84-37-94-97-ff-03-23" (lowercase).
  bool appendOneWireAddress(const uint8_t (&addr)[8]);

  const char* c_str() const { return cap_ ? buf_ : ""; }
  size_t length() const { return len_; }
  size_t capacity() const { return cap_; }
  bool ok() const { return ok_; }
  void clear();
  // Shortens the text to `len` characters (no-op if already shorter);
  // does not reset ok().
  void truncate(size_t len);

 private:
  bool appendRaw(const char* s, size_t n);

  char* buf_;
  size_t cap_;
  size_t len_;
  bool ok_;
};

template <size_t N>
class StaticBufWriter : public BufWriter {
  static_assert(N >= 1, "buffer needs room for the NUL");

 public:
  StaticBufWriter() : BufWriter(storage_, N) {}
  StaticBufWriter(const StaticBufWriter&) = delete;
  StaticBufWriter& operator=(const StaticBufWriter&) = delete;

 private:
  char storage_[N];
};

}  // namespace vdm
