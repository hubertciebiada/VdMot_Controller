//! Bounded line assembler for the text protocols (ESP UART, debug terminal).
//! Hardware-free: no Arduino headers.

use crate::buf_writer::Storage;

fn is_terminator(c: u8) -> bool {
    c == b'\r' || c == b'\n'
}

fn is_line_char(c: u8) -> bool {
    c == b'\t' || (0x20..0x7F).contains(&c)
}

/// Collects bytes into lines inside a caller-provided buffer.
///  - CR, LF and CRLF terminate a line; empty lines are ignored.
///  - A line longer than capacity-1 characters is dropped up to its terminator
///    and counted as an overflow.
///  - A line containing a byte other than printable ASCII or TAB is dropped up
///    to its terminator and counted as malformed.
///  - Once a line is complete no further byte is consumed until release(), so
///    bytes following the terminator stay with the caller for the next line.
///  - expire() drops an unterminated line after an idle time, so the bytes of a
///    line whose sender went away are not glued to the next request.
///
/// The capacity is the storage length and includes the C++ NUL, which is not written. A
/// capacity < 2 (the C++ null buffer is storage of length 0) yields an assembler that drops
/// every non-empty line as overflow.
#[derive(Clone, Debug)]
pub struct LineAssembler<B> {
    buf: B,
    len: usize,
    ready: bool,
    discarding: bool,
    overflows: u32,
    malformed: u32,
    expired: u32,
}

/// A [`LineAssembler`] owning its storage of N bytes (lines of up to N - 1 characters).
pub type StaticLineAssembler<const N: usize> = LineAssembler<[u8; N]>;

impl<const N: usize> Default for LineAssembler<[u8; N]> {
    fn default() -> Self {
        LineAssembler::new([0; N])
    }
}

impl<B: Storage> LineAssembler<B> {
    pub fn new(storage: B) -> Self {
        LineAssembler {
            buf: storage,
            len: 0,
            ready: false,
            discarding: false,
            overflows: 0,
            malformed: 0,
            expired: 0,
        }
    }

    fn start_discard(&mut self, overflow: bool) {
        self.discarding = true;
        self.len = 0;
        if overflow {
            self.overflows = self.overflows.wrapping_add(1);
        } else {
            self.malformed = self.malformed.wrapping_add(1);
        }
    }

    /// Consumes one byte. Returns false (byte not consumed) while a complete
    /// line is pending.
    pub fn push(&mut self, c: u8) -> bool {
        if self.ready {
            return false;
        }

        if is_terminator(c) {
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
            self.start_discard(false);
        } else if self.len + 1 >= self.buf.as_ref().len() {
            self.start_discard(true);
        } else if let Some(slot) = self.buf.as_mut().get_mut(self.len) {
            *slot = c;
            self.len += 1;
        }
        true
    }

    /// Consumes bytes until a line is complete or data is exhausted and returns
    /// the number of bytes consumed.
    pub fn feed(&mut self, data: &[u8]) -> usize {
        let mut used = 0;
        while used < data.len() && self.push(data[used]) {
            used += 1;
        }
        used
    }

    pub fn has_line(&self) -> bool {
        self.ready
    }

    /// Current line (complete when has_line(), otherwise the partial line).
    pub fn line(&self) -> &[u8] {
        self.buf.as_ref().get(..self.len).unwrap_or_default()
    }

    /// Writable view of the complete line (the C++ tokenizes it in place); `None` when no
    /// line is pending.
    pub fn take_line(&mut self) -> Option<&mut [u8]> {
        if !self.ready {
            return None;
        }
        self.buf.as_mut().get_mut(..self.len)
    }

    pub fn length(&self) -> usize {
        self.len
    }

    /// Drops the pending line and starts collecting the next one.
    pub fn release(&mut self) {
        self.ready = false;
        self.len = 0;
    }

    /// Drops everything, including a partial line and discard state.
    pub fn reset(&mut self) {
        self.discarding = false;
        self.release();
    }

    /// True while bytes of an unterminated line were consumed (also while an
    /// overlong or malformed line is being discarded).
    pub fn partial(&self) -> bool {
        !self.ready && (self.len > 0 || self.discarding)
    }

    /// Drops a partial line if the last byte was consumed at last_byte_ms and
    /// more than timeout_ms have passed until now_ms (millisecond clock, may
    /// wrap). A dropped partial line is counted as expired; a line that was
    /// already being discarded is not counted again. Returns true if something
    /// was dropped.
    pub fn expire(&mut self, now_ms: u32, last_byte_ms: u32, timeout_ms: u32) -> bool {
        if !self.partial() || now_ms.wrapping_sub(last_byte_ms) <= timeout_ms {
            return false;
        }
        if !self.discarding {
            self.expired = self.expired.wrapping_add(1);
        }
        self.reset();
        true
    }

    /// Event counters; they wrap modulo 2^32, so consumers use differences.
    pub fn overflow_count(&self) -> u32 {
        self.overflows
    }

    pub fn malformed_count(&self) -> u32 {
        self.malformed
    }

    pub fn expired_count(&self) -> u32 {
        self.expired
    }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_fuzz;
