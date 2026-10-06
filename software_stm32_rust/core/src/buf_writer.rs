//! Bounded text formatting into a caller-provided buffer. Hardware-free.

/// The storage of a [`BufWriter`] or a `LineAssembler`: a byte array it owns or a slice it
/// borrows.
pub trait Storage: AsRef<[u8]> + AsMut<[u8]> {}

impl<T: AsRef<[u8]> + AsMut<[u8]>> Storage for T {}

const HEX: &[u8; 16] = b"0123456789abcdef";

/// Appends text to a fixed buffer.
/// Each append is all-or-nothing: if the piece does not fit, nothing of it is
/// written, the call returns false and `ok()` stays false until `clear()`.
///
/// The capacity is the length of the storage and includes the terminating NUL of the C++
/// writer: storage of N bytes holds at most N - 1 characters, and storage of 0 bytes (the C++
/// null buffer) makes every append fail. The NUL itself is not written; [`as_bytes`] is the
/// text.
///
/// [`as_bytes`]: BufWriter::as_bytes
#[derive(Clone, Debug)]
pub struct BufWriter<B> {
    buf: B,
    len: usize,
    ok: bool,
}

/// A [`BufWriter`] owning its storage of N bytes (N - 1 characters).
pub type StaticBufWriter<const N: usize> = BufWriter<[u8; N]>;

impl<const N: usize> Default for BufWriter<[u8; N]> {
    fn default() -> Self {
        BufWriter::new([0; N])
    }
}

impl<B: Storage> BufWriter<B> {
    /// A writer over `buf`; its length is the capacity (NUL included).
    pub fn new(buf: B) -> Self {
        BufWriter {
            buf,
            len: 0,
            ok: true,
        }
    }

    pub fn append(&mut self, s: &[u8]) -> bool {
        self.append_raw(s)
    }

    pub fn append_char(&mut self, c: u8) -> bool {
        self.append_raw(&[c])
    }

    pub fn append_unsigned(&mut self, v: u32) -> bool {
        let mut digits = [0u8; 10];
        let mut v = v;
        let mut n = digits.len();
        loop {
            n -= 1;
            digits[n] = b'0' + (v % 10) as u8;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        self.append_raw(&digits[n..])
    }

    pub fn append_signed(&mut self, v: i32) -> bool {
        let mut text = [0u8; 11];
        // the magnitude as unsigned, so i32::MIN does not overflow
        let mut magnitude = v.unsigned_abs();
        let mut n = text.len();
        loop {
            n -= 1;
            text[n] = b'0' + (magnitude % 10) as u8;
            magnitude /= 10;
            if magnitude == 0 {
                break;
            }
        }
        if v < 0 {
            n -= 1;
            text[n] = b'-';
        }
        self.append_raw(&text[n..])
    }

    /// Two lowercase hex digits.
    pub fn append_hex2(&mut self, v: u8) -> bool {
        self.append_raw(&[HEX[usize::from(v >> 4)], HEX[usize::from(v & 0x0F)]])
    }

    /// 1-Wire ROM address as "28-84-37-94-97-ff-03-23" (lowercase).
    pub fn append_one_wire_address(&mut self, addr: &[u8; 8]) -> bool {
        let mut text = [b'-'; 23];
        for (i, &b) in addr.iter().enumerate() {
            text[i * 3] = HEX[usize::from(b >> 4)];
            text[i * 3 + 1] = HEX[usize::from(b & 0x0F)];
        }
        self.append_raw(&text)
    }

    /// The text written so far (without the C++ NUL).
    pub fn as_bytes(&self) -> &[u8] {
        self.buf.as_ref().get(..self.len).unwrap_or_default()
    }

    pub fn length(&self) -> usize {
        self.len
    }

    /// The storage length, the C++ capacity (NUL included).
    pub fn capacity(&self) -> usize {
        self.buf.as_ref().len()
    }

    pub fn ok(&self) -> bool {
        self.ok
    }

    pub fn clear(&mut self) {
        self.len = 0;
        self.ok = true;
    }

    /// Shortens the text to `len` characters (no-op if already shorter);
    /// does not reset ok().
    pub fn truncate(&mut self, len: usize) {
        self.len = self.len.min(len);
    }

    fn append_raw(&mut self, s: &[u8]) -> bool {
        let cap = self.capacity();
        // room for the text and the NUL the C++ writer keeps behind it
        if cap == 0 || s.len() >= cap - self.len {
            self.ok = false;
            return false;
        }
        let end = self.len + s.len();
        if let Some(dst) = self.buf.as_mut().get_mut(self.len..end) {
            dst.copy_from_slice(s);
            self.len = end;
        }
        true
    }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_fuzz;
