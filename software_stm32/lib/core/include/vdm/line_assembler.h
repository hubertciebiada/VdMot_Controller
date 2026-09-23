// Bounded line assembler for the text protocols (ESP UART, debug terminal).
// Hardware-free: no Arduino headers.
#pragma once

#include <stddef.h>
#include <stdint.h>

namespace vdm {

// Collects bytes into NUL-terminated lines inside a caller-provided buffer.
//  - CR, LF and CRLF terminate a line; empty lines are ignored.
//  - A line longer than capacity-1 characters is dropped up to its terminator
//    and counted as an overflow.
//  - A line containing a byte other than printable ASCII or TAB is dropped up
//    to its terminator and counted as malformed.
//  - Once a line is complete no further byte is consumed until release(), so
//    bytes following the terminator stay with the caller for the next line.
class LineAssembler {
 public:
  // capacity includes the terminating NUL. A capacity < 2 (or a null buffer)
  // yields an assembler that drops every non-empty line as overflow.
  LineAssembler(char* storage, size_t capacity);

  // Consumes one byte. Returns false (byte not consumed) while a complete
  // line is pending.
  bool push(char c);

  // Consumes bytes until a line is complete or data is exhausted and returns
  // the number of bytes consumed.
  size_t feed(const char* data, size_t len);

  bool hasLine() const { return ready_; }
  // Current line (complete when hasLine(), otherwise the partial line).
  const char* line() const { return cap_ ? buf_ : ""; }
  // Writable view of the complete line for in-place tokenizing; nullptr when
  // no line is pending.
  char* takeLine() { return ready_ ? buf_ : nullptr; }
  size_t length() const { return len_; }

  // Drops the pending line and starts collecting the next one.
  void release();
  // Drops everything, including a partial line and discard state.
  void reset();

  // Event counters; they wrap modulo 2^32, so consumers use differences.
  uint32_t overflowCount() const { return overflows_; }
  uint32_t malformedCount() const { return malformed_; }

 private:
  void startDiscard(bool overflow);
  void clearBuffer();

  char* buf_;
  size_t cap_;
  size_t len_;
  bool ready_;
  bool discarding_;
  uint32_t overflows_;
  uint32_t malformed_;
};

// LineAssembler owning its storage.
template <size_t N>
class StaticLineAssembler : public LineAssembler {
  static_assert(N >= 2, "line buffer needs room for one character and NUL");

 public:
  StaticLineAssembler() : LineAssembler(storage_, N) {}
  StaticLineAssembler(const StaticLineAssembler&) = delete;
  StaticLineAssembler& operator=(const StaticLineAssembler&) = delete;

 private:
  char storage_[N];
};

}  // namespace vdm
