#include "vdm/json_writer.h"

#include <math.h>
#include <stdio.h>
#include <string.h>

namespace vdm {

JsonWriter::JsonWriter(char* buf, size_t capacity) : buf_(buf), cap_(buf ? capacity : 0) {
  reset();
}

void JsonWriter::reset() {
  len_ = 0;
  ok_ = cap_ > 0;
  depth_ = 0;
  wroteRoot_ = false;
  keyPending_ = false;
  if (cap_) buf_[0] = '\0';
}

bool JsonWriter::put(char c) {
  if (len_ + 1 >= cap_) return false;
  buf_[len_++] = c;
  buf_[len_] = '\0';
  return true;
}

bool JsonWriter::putRaw(const char* s, size_t n) {
  if (n >= cap_ - len_) return false;
  memcpy(buf_ + len_, s, n);
  len_ += n;
  buf_[len_] = '\0';
  return true;
}

bool JsonWriter::putEscaped(const char* s, size_t n) {
  if (!put('"')) return false;
  for (size_t i = 0; i < n; ++i) {
    const unsigned char c = static_cast<unsigned char>(s[i]);
    bool okc = true;
    switch (c) {
      case '"': okc = putRaw("\\\"", 2); break;
      case '\\': okc = putRaw("\\\\", 2); break;
      case '\n': okc = putRaw("\\n", 2); break;
      case '\r': okc = putRaw("\\r", 2); break;
      case '\t': okc = putRaw("\\t", 2); break;
      case '\b': okc = putRaw("\\b", 2); break;
      case '\f': okc = putRaw("\\f", 2); break;
      default:
        if (c < 0x20) {
          char u[7];
          snprintf(u, sizeof u, "\\u%04x", static_cast<unsigned>(c));
          okc = putRaw(u, 6);
        } else {
          okc = put(static_cast<char>(c));
        }
    }
    if (!okc) return false;
  }
  return put('"');
}

bool JsonWriter::beforeValue() {
  if (!ok_) return false;
  if (depth_ == 0) {
    if (wroteRoot_) return false;
    wroteRoot_ = true;
    return true;
  }
  const uint8_t d = depth_ - 1;
  if (isObject_[d]) {
    if (!keyPending_) return false;
    keyPending_ = false;
    return true;
  }
  if (!first_[d] && !put(',')) return false;
  first_[d] = false;
  return true;
}

namespace {

// Runs `op`; on failure rolls the buffer back to `mark` and poisons the writer.
template <typename Op>
void guarded(char* buf, size_t cap, size_t& len, bool& ok, Op op) {
  if (!ok) return;
  const size_t mark = len;
  if (!op()) {
    len = mark;
    if (cap) buf[len] = '\0';
    ok = false;
  }
}

}  // namespace

void JsonWriter::beginObject() {
  guarded(buf_, cap_, len_, ok_, [&] {
    if (depth_ >= kMaxDepth || !beforeValue() || !put('{')) return false;
    isObject_[depth_] = true;
    first_[depth_] = true;
    ++depth_;
    return true;
  });
}

void JsonWriter::endObject() {
  guarded(buf_, cap_, len_, ok_, [&] {
    if (depth_ == 0 || !isObject_[depth_ - 1] || keyPending_ || !put('}')) return false;
    --depth_;
    return true;
  });
}

void JsonWriter::beginArray() {
  guarded(buf_, cap_, len_, ok_, [&] {
    if (depth_ >= kMaxDepth || !beforeValue() || !put('[')) return false;
    isObject_[depth_] = false;
    first_[depth_] = true;
    ++depth_;
    return true;
  });
}

void JsonWriter::endArray() {
  guarded(buf_, cap_, len_, ok_, [&] {
    if (depth_ == 0 || isObject_[depth_ - 1] || !put(']')) return false;
    --depth_;
    return true;
  });
}

void JsonWriter::key(const char* k) {
  guarded(buf_, cap_, len_, ok_, [&] {
    if (k == nullptr || depth_ == 0 || !isObject_[depth_ - 1] || keyPending_) return false;
    const uint8_t d = depth_ - 1;
    if (!first_[d] && !put(',')) return false;
    if (!putEscaped(k, strlen(k)) || !put(':')) return false;
    first_[d] = false;
    keyPending_ = true;
    return true;
  });
}

void JsonWriter::value(const char* s) {
  if (s == nullptr) {
    nullValue();
    return;
  }
  value(s, strlen(s));
}

void JsonWriter::value(const char* s, size_t len) {
  guarded(buf_, cap_, len_, ok_, [&] { return s != nullptr && beforeValue() && putEscaped(s, len); });
}

void JsonWriter::value(bool b) {
  guarded(buf_, cap_, len_, ok_,
          [&] { return beforeValue() && (b ? putRaw("true", 4) : putRaw("false", 5)); });
}

void JsonWriter::value(int32_t v) { value(static_cast<int64_t>(v)); }

void JsonWriter::value(uint32_t v) { value(static_cast<int64_t>(v)); }

void JsonWriter::value(int64_t v) {
  guarded(buf_, cap_, len_, ok_, [&] {
    char tmp[24];
    const int n = snprintf(tmp, sizeof tmp, "%lld", static_cast<long long>(v));
    return n > 0 && beforeValue() && putRaw(tmp, static_cast<size_t>(n));
  });
}

void JsonWriter::nullValue() {
  guarded(buf_, cap_, len_, ok_, [&] { return beforeValue() && putRaw("null", 4); });
}

void JsonWriter::fixed(int32_t scaled, uint8_t decimals) {
  guarded(buf_, cap_, len_, ok_, [&] {
    if (decimals > 6) return false;
    int64_t pow10 = 1;
    for (uint8_t i = 0; i < decimals; ++i) pow10 *= 10;
    const bool neg = scaled < 0;
    const int64_t mag = neg ? -static_cast<int64_t>(scaled) : static_cast<int64_t>(scaled);
    char tmp[32];
    int n;
    if (decimals == 0) {
      n = snprintf(tmp, sizeof tmp, "%s%lld", neg ? "-" : "", static_cast<long long>(mag));
    } else {
      n = snprintf(tmp, sizeof tmp, "%s%lld.%0*lld", neg ? "-" : "",
                   static_cast<long long>(mag / pow10), static_cast<int>(decimals),
                   static_cast<long long>(mag % pow10));
    }
    return n > 0 && static_cast<size_t>(n) < sizeof tmp && beforeValue() &&
           putRaw(tmp, static_cast<size_t>(n));
  });
}

void JsonWriter::number(double v, uint8_t decimals) {
  if (!isfinite(v)) {
    nullValue();
    return;
  }
  guarded(buf_, cap_, len_, ok_, [&] {
    if (decimals > 6 || fabs(v) > 1e15) return false;
    char tmp[40];
    const int n = snprintf(tmp, sizeof tmp, "%.*f", static_cast<int>(decimals), v);
    return n > 0 && static_cast<size_t>(n) < sizeof tmp && beforeValue() &&
           putRaw(tmp, static_cast<size_t>(n));
  });
}

void JsonWriter::raw(const char* json) {
  guarded(buf_, cap_, len_, ok_,
          [&] { return json != nullptr && json[0] != '\0' && beforeValue() && putRaw(json, strlen(json)); });
}

size_t jsonEscape(const char* s, char* out, size_t cap) {
  if (cap == 0) return 0;
  out[0] = '\0';
  if (s == nullptr) return 0;
  // Reuse the writer's escaping: write a string value, then strip the quotes.
  JsonWriter jw(out, cap);
  jw.value(s);
  if (!jw.ok()) {
    out[0] = '\0';
    return 0;
  }
  const size_t n = jw.length() - 2;
  memmove(out, out + 1, n);
  out[n] = '\0';
  return n;
}

}  // namespace vdm
