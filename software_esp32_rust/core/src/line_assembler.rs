//! Bounded line assembler for the STM application protocol, ESP side (port of
//! `vdm/line_assembler.h`). Hardware-free. Same contract as the `software_stm32/lib/core`
//! LineAssembler so both ends of the link behave identically.

/// Longest STM reply line the ESP accepts, without terminator and NUL. `gonec 255` with 34
/// sensors is ~827 chars; 1023 leaves margin.
pub const STM_MAX_LINE_LEN: usize = 1023;

/// Collects bytes into lines in its own storage of `N` bytes (the C++ capacity, NUL included:
/// lines of up to N - 1 chars; `LineAssembler<{ STM_MAX_LINE_LEN + 1 }>` is the C++
/// `StaticLineAssembler<kStmMaxLineLen + 1>`).
///  - CR, LF and CRLF terminate a line (CRLF counts once: an LF directly after a CR that ended a
///    line is swallowed, also across feed() calls); empty lines are ignored.
///  - A line longer than N - 1 characters is dropped up to its terminator and counted once in
///    overflow_count().
///  - A line containing a byte other than printable ASCII (0x20..0x7E) or TAB is dropped up to
///    its terminator and counted once in malformed_count(). (This catches 8E1 noise and
///    bootloader bytes on the shared UART.) Both are counted when the bad byte arrives, so an
///    endless stream without a terminator is still visible in the counters.
///  - Once a line is complete no further byte is consumed until release(), so bytes after the
///    terminator stay with the caller for the next line (no data loss when several replies
///    arrive in one read).
///
/// N < 2 yields an assembler that drops every non-empty line as overflow (the C++ capacity 0,
/// 1 or a null buffer).
#[derive(Clone, Debug)]
pub struct LineAssembler<const N: usize> {
    buf: [u8; N],
    len: usize,
    ready: bool,
    discarding: bool,
    overflows: u32,
    malformed: u32,
}

fn is_line_char(c: u8) -> bool {
    c == b'\t' || (0x20..0x7F).contains(&c)
}

impl<const N: usize> LineAssembler<N> {
    pub const fn new() -> Self {
        Self {
            buf: [0; N],
            len: 0,
            ready: false,
            discarding: false,
            overflows: 0,
            malformed: 0,
        }
    }

    /// Consumes one byte. Returns false (byte not consumed) while a complete line is pending.
    pub fn push(&mut self, c: u8) -> bool {
        if self.ready {
            return false;
        }
        // CR, LF and CRLF all end a line; the LF of a CRLF (and any other empty line) finds
        // len == 0 and is ignored.
        if c == b'\r' || c == b'\n' {
            if self.discarding {
                self.discarding = false;
            } else if self.len > 0 {
                self.ready = true;
            }
            return true;
        }
        if self.discarding {
            return true;
        }
        if !is_line_char(c) {
            self.malformed = self.malformed.wrapping_add(1);
        } else if self.len + 1 >= N {
            self.overflows = self.overflows.wrapping_add(1);
        } else {
            self.buf[self.len] = c;
            self.len += 1;
            return true;
        }
        self.discarding = true;
        self.len = 0;
        true
    }

    /// Consumes bytes until a line is complete or data is exhausted; returns the number of bytes
    /// consumed (the rest must be offered again after release()).
    pub fn feed(&mut self, data: &[u8]) -> usize {
        data.iter()
            .position(|&c| !self.push(c))
            .unwrap_or(data.len())
    }

    pub fn has_line(&self) -> bool {
        self.ready
    }

    /// Complete line when has_line(), otherwise the partial line so far.
    pub fn line(&self) -> &[u8] {
        self.buf.get(..self.len).unwrap_or_default()
    }

    pub fn length(&self) -> usize {
        self.len
    }

    /// Drops the pending line and starts collecting the next one.
    pub fn release(&mut self) {
        self.ready = false;
        self.len = 0;
    }

    /// Drops everything, including a partial line and discard state. Used when the UART is
    /// re-opened (flasher, STM reset).
    pub fn reset(&mut self) {
        self.release();
        self.discarding = false;
    }

    /// Event counter; wraps modulo 2^32, consumers use differences.
    pub fn overflow_count(&self) -> u32 {
        self.overflows
    }

    /// Event counter; wraps modulo 2^32, consumers use differences.
    pub fn malformed_count(&self) -> u32 {
        self.malformed
    }
}

impl<const N: usize> Default for LineAssembler<N> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
