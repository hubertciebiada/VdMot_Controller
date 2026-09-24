// Tiny bounded JSON writer into a caller-provided buffer. Hardware-free, no
// heap, no floating point formatting surprises (fixed decimals).
//
// Usage:
//   char buf[256]; JsonWriter jw(buf, sizeof buf);
//   jw.beginObject(); jw.key("a"); jw.value(1); jw.key("s"); jw.value("x\"y");
//   jw.endObject();   // buf == {"a":1,"s":"x\"y"}
//   if (!jw.ok()) -> overflow or misuse; buf still NUL-terminated (content
//                    is truncated at the last complete token and not valid JSON)
//
// Commas and nesting are handled by the writer. Misuse (value without key in
// an object, key outside an object, unbalanced end, depth > kMaxDepth) sets
// ok() to false and every later call is a no-op.
#pragma once

#include <stddef.h>
#include <stdint.h>

namespace vdm {

class JsonWriter {
 public:
  static constexpr uint8_t kMaxDepth = 8;

  // capacity includes the NUL. capacity 0 / null buffer -> ok() false.
  JsonWriter(char* buf, size_t capacity);

  void beginObject();
  void endObject();
  void beginArray();
  void endArray();
  // Object member name; escaped like a string value.
  void key(const char* k);

  void value(const char* s);          // null pointer writes null
  void value(const char* s, size_t len);  // exactly len bytes (may contain NUL -> \u0000)
  void value(bool b);
  void value(int32_t v);
  void value(uint32_t v);
  void value(int64_t v);
  void nullValue();
  // Fixed-point decimal: scaled / 10^decimals, e.g. (215, 1) -> 21.5,
  // (-5, 1) -> -0.5, (12345, 3) -> 12.345. decimals 0..6.
  void fixed(int32_t scaled, uint8_t decimals);
  // Finite double with `decimals` (0..6) digits; NaN/Inf write null.
  void number(double v, uint8_t decimals);
  // Pre-formatted, trusted JSON fragment (e.g. a nested document) written
  // verbatim as one value.
  void raw(const char* json);

  // Convenience: key + value.
  template <typename T>
  void kv(const char* k, T v) {
    key(k);
    value(v);
  }

  bool ok() const { return ok_; }
  // True when ok() and every container has been closed.
  bool complete() const { return ok_ && depth_ == 0 && wroteRoot_; }
  size_t length() const { return len_; }
  const char* c_str() const { return cap_ ? buf_ : ""; }
  void reset();

 private:
  bool beforeValue();
  bool put(char c);
  bool putRaw(const char* s, size_t n);
  bool putEscaped(const char* s, size_t n);
  bool putEscapedChar(unsigned char c);

  char* buf_;
  size_t cap_;
  size_t len_ = 0;
  bool ok_ = true;
  uint8_t depth_ = 0;
  bool wroteRoot_ = false;
  bool isObject_[kMaxDepth] = {false};
  bool first_[kMaxDepth] = {false};
  bool keyPending_ = false;
};

// Writes `s` JSON-escaped (without quotes) into out; for places that embed a
// string in a larger template. Returns chars written, 0 if it does not fit.
size_t jsonEscape(const char* s, char* out, size_t cap);

}  // namespace vdm
