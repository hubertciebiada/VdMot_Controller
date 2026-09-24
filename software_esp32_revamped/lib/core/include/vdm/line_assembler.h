// Bounded line assembler for the STM application protocol (ESP side).
// Hardware-free. Same contract as software_stm32/lib/core LineAssembler so
// both ends of the link behave identically.
#pragma once

#include <stddef.h>
#include <stdint.h>

namespace vdm {

// Longest STM reply line the ESP accepts, without terminator and NUL.
// `gonec 255` with 34 sensors is ~827 chars; 1023 leaves margin.
constexpr size_t kStmMaxLineLen = 1023;

// Collects bytes into NUL-terminated lines inside a caller-provided buffer.
//  - CR, LF and CRLF terminate a line (CRLF counts once: an LF directly after
//    a CR that ended a line is swallowed, also across feed() calls); empty
//    lines are ignored.
//  - A line longer than capacity-1 characters is dropped up to its
//    terminator and counted once in overflowCount().
//  - A line containing a byte other than printable ASCII (0x20..0x7E) or TAB
//    is dropped up to its terminator and counted once in malformedCount().
//    (This catches 8E1 noise and bootloader bytes on the shared UART.)
//    Both are counted when the bad byte arrives, so an endless stream
//    without a terminator is still visible in the counters.
//  - Once a line is complete no further byte is consumed until release(), so
//    bytes after the terminator stay with the caller for the next line
//    (no data loss when several replies arrive in one read).
class LineAssembler {
 public:
  // capacity includes the terminating NUL. capacity < 2 (or a null buffer)
  // yields an assembler that drops every non-empty line as overflow.
  LineAssembler(char* storage, size_t capacity);

  // Consumes one byte. Returns false (byte not consumed) while a complete
  // line is pending.
  bool push(char c);

  // Consumes bytes until a line is complete or data is exhausted; returns
  // the number of bytes consumed (the rest must be offered again after
  // release()).
  size_t feed(const char* data, size_t len);

  bool hasLine() const { return ready_; }
  // Complete line when hasLine(), otherwise the partial line so far.
  const char* line() const { return cap_ ? buf_ : ""; }
  size_t length() const { return len_; }

  // Drops the pending line and starts collecting the next one.
  void release();
  // Drops everything, including a partial line and discard state. Used when
  // the UART is re-opened (flasher, STM reset).
  void reset();

  // Event counters; wrap modulo 2^32, consumers use differences.
  uint32_t overflowCount() const { return overflows_; }
  uint32_t malformedCount() const { return malformed_; }

 private:
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
