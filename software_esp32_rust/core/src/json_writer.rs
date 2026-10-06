//! Tiny bounded JSON writer into a caller-provided buffer (port of `vdm/json_writer.h`).
//! Hardware-free, no heap, no floating point formatting surprises (fixed decimals).
//!
//! Usage (the same as a test in `json_writer/tests.rs`; no doctests in core, every mutant would
//! compile them):
//! ```ignore
//! let mut buf = [0u8; 256];
//! let mut jw = JsonWriter::new(&mut buf);
//! jw.begin_object();
//! jw.kv("a", 1);
//! jw.kv("s", "x\"y");
//! jw.end_object();
//! assert!(jw.complete());
//! assert_eq!(jw.as_bytes(), br#"{"a":1,"s":"x\"y"}"#);
//! ```
//! `ok()` false -> overflow or misuse; the text is truncated at the last complete token and
//! not valid JSON.
//!
//! Commas and nesting are handled by the writer. Misuse (value without key in an object, key
//! outside an object, unbalanced end, depth > [`JsonWriter::MAX_DEPTH`]) sets `ok()` to false
//! and every later call is a no-op.
//!
//! The buffer length is the C++ capacity (NUL included): at most `buf.len() - 1` bytes of JSON
//! fit; the NUL is not written. Strings are exact byte slices (the C++ `value(s, len)`: a NUL
//! byte is written as `\u0000`, bytes >= 0x80 pass through); `key()` and `raw()` take C strings
//! like the C++ (they end at a NUL).

use crate::common::{c_str, fmt_fit, format_f64_fixed};

/// JSON escape sequence of one byte; returns it and its length.
fn escape_char(c: u8) -> ([u8; 6], usize) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let e = match c {
        b'"' => b'"',
        b'\\' => b'\\',
        b'\n' => b'n',
        b'\r' => b'r',
        b'\t' => b't',
        0x08 => b'b',
        0x0C => b'f',
        _ => 0,
    };
    if e != 0 {
        ([b'\\', e, 0, 0, 0, 0], 2)
    } else if c >= 0x20 {
        ([c, 0, 0, 0, 0, 0], 1)
    } else {
        (
            [
                b'\\',
                b'u',
                b'0',
                b'0',
                HEX[usize::from(c >> 4)],
                HEX[usize::from(c & 0x0F)],
            ],
            6,
        )
    }
}

/// A value [`JsonWriter::value`] and [`JsonWriter::kv`] can write: the C++ `value()` overloads.
/// Strings: `&str`, `&[u8]`, `&[u8; N]`, `&Text<N>`; integers (i8..i64, u8..u32; the C++ has
/// no 64-bit unsigned overload); `bool`; `Option<T>` (None writes `null`, the C++
/// `value((const char*)nullptr)`).
pub trait JsonValue {
    fn write_json(self, jw: &mut JsonWriter<'_>);
}

impl<T: AsRef<[u8]> + ?Sized> JsonValue for &T {
    fn write_json(self, jw: &mut JsonWriter<'_>) {
        jw.value_bytes(self.as_ref());
    }
}

impl JsonValue for bool {
    fn write_json(self, jw: &mut JsonWriter<'_>) {
        jw.value_bool(self);
    }
}

macro_rules! json_int {
    ($($t:ty),*) => {$(
        impl JsonValue for $t {
            fn write_json(self, jw: &mut JsonWriter<'_>) {
                jw.value_i64(i64::from(self));
            }
        }
    )*};
}
json_int!(i8, u8, i16, u16, i32, u32, i64);

impl<T: JsonValue> JsonValue for Option<T> {
    fn write_json(self, jw: &mut JsonWriter<'_>) {
        match self {
            Some(v) => v.write_json(jw),
            None => jw.null_value(),
        }
    }
}

/// Bounded JSON writer over a caller buffer.
#[derive(Debug)]
pub struct JsonWriter<'a> {
    buf: &'a mut [u8],
    len: usize,
    ok: bool,
    depth: u8,
    wrote_root: bool,
    is_object: [bool; JsonWriter::MAX_DEPTH as usize],
    first: [bool; JsonWriter::MAX_DEPTH as usize],
    key_pending: bool,
}

impl<'a> JsonWriter<'a> {
    pub const MAX_DEPTH: u8 = 8;

    /// The buffer length includes the C++ NUL; an empty buffer -> `ok()` false.
    pub fn new(buf: &'a mut [u8]) -> Self {
        let mut jw = Self {
            buf,
            len: 0,
            ok: false,
            depth: 0,
            wrote_root: false,
            is_object: [false; Self::MAX_DEPTH as usize],
            first: [false; Self::MAX_DEPTH as usize],
            key_pending: false,
        };
        jw.reset();
        jw
    }

    /// Starts again with an empty document (also after misuse or overflow).
    pub fn reset(&mut self) {
        self.len = 0;
        self.ok = !self.buf.is_empty();
        self.depth = 0;
        self.wrote_root = false;
        self.key_pending = false;
    }

    /// No overflow and no misuse so far.
    pub fn ok(&self) -> bool {
        self.ok
    }

    /// True when ok() and every container has been closed.
    pub fn complete(&self) -> bool {
        self.ok && self.depth == 0 && self.wrote_root
    }

    /// Bytes written.
    pub fn length(&self) -> usize {
        self.len
    }

    /// The document so far (the C++ `c_str()`).
    pub fn as_bytes(&self) -> &[u8] {
        self.buf.get(..self.len).unwrap_or_default()
    }

    /// The document, for as long as the buffer lives.
    pub fn into_bytes(self) -> &'a [u8] {
        let len = self.len;
        self.buf.get(..len).unwrap_or_default()
    }

    fn put(&mut self, c: u8) -> bool {
        self.put_raw(&[c])
    }

    fn put_raw(&mut self, s: &[u8]) -> bool {
        if s.len() >= self.buf.len() - self.len {
            return false;
        }
        let end = self.len + s.len();
        if let Some(dst) = self.buf.get_mut(self.len..end) {
            dst.copy_from_slice(s);
            self.len = end;
        }
        true
    }

    fn put_escaped(&mut self, s: &[u8]) -> bool {
        self.put(b'"')
            && s.iter().all(|&c| {
                let (e, n) = escape_char(c);
                self.put_raw(&e[..n])
            })
            && self.put(b'"')
    }

    /// Only called inside guarded(), which has already checked `ok`.
    fn before_value(&mut self) -> bool {
        let Some(d) = self.depth.checked_sub(1).map(usize::from) else {
            if self.wrote_root {
                return false;
            }
            self.wrote_root = true;
            return true;
        };
        if self.is_object[d] {
            if !self.key_pending {
                return false;
            }
            self.key_pending = false;
            return true;
        }
        if !self.first[d] && !self.put(b',') {
            return false;
        }
        self.first[d] = false;
        true
    }

    /// Runs `op`; on failure rolls the buffer back to where it was and poisons the writer.
    fn guarded(&mut self, op: impl FnOnce(&mut Self) -> bool) {
        if !self.ok {
            return;
        }
        let mark = self.len;
        if !op(self) {
            self.len = mark;
            self.ok = false;
        }
    }

    fn begin(&mut self, open: u8, object: bool) {
        self.guarded(|w| {
            let d = usize::from(w.depth);
            if d >= usize::from(Self::MAX_DEPTH) || !w.before_value() || !w.put(open) {
                return false;
            }
            w.is_object[d] = object;
            w.first[d] = true;
            w.depth += 1;
            true
        });
    }

    fn end(&mut self, close: u8, object: bool) {
        self.guarded(|w| {
            let Some(d) = w.depth.checked_sub(1) else {
                return false;
            };
            if w.is_object[usize::from(d)] != object || w.key_pending || !w.put(close) {
                return false;
            }
            w.depth = d;
            true
        });
    }

    pub fn begin_object(&mut self) {
        self.begin(b'{', true);
    }

    pub fn end_object(&mut self) {
        self.end(b'}', true);
    }

    pub fn begin_array(&mut self) {
        self.begin(b'[', false);
    }

    pub fn end_array(&mut self) {
        self.end(b']', false);
    }

    /// Object member name (a C string); escaped like a string value.
    pub fn key(&mut self, k: impl AsRef<[u8]>) {
        let k = c_str(k.as_ref());
        self.guarded(|w| {
            let Some(d) = w.depth.checked_sub(1).map(usize::from) else {
                return false;
            };
            if !w.is_object[d] || w.key_pending {
                return false;
            }
            if !w.first[d] && !w.put(b',') {
                return false;
            }
            if !w.put_escaped(k) || !w.put(b':') {
                return false;
            }
            w.first[d] = false;
            w.key_pending = true;
            true
        });
    }

    /// One value (see [`JsonValue`]).
    pub fn value(&mut self, v: impl JsonValue) {
        v.write_json(self);
    }

    /// Convenience: key + value.
    pub fn kv(&mut self, k: impl AsRef<[u8]>, v: impl JsonValue) {
        self.key(k);
        self.value(v);
    }

    fn value_bytes(&mut self, s: &[u8]) {
        self.guarded(|w| w.before_value() && w.put_escaped(s));
    }

    fn value_bool(&mut self, b: bool) {
        self.guarded(|w| w.before_value() && w.put_raw(if b { b"true" } else { b"false" }));
    }

    fn value_i64(&mut self, v: i64) {
        self.guarded(|w| {
            let mut tmp = [0u8; 24];
            let n = fmt_fit(&mut tmp, format_args!("{v}"));
            w.before_value() && w.put_raw(&tmp[..n])
        });
    }

    pub fn null_value(&mut self) {
        self.guarded(|w| w.before_value() && w.put_raw(b"null"));
    }

    /// Fixed-point decimal: scaled / 10^decimals, e.g. (215, 1) -> 21.5, (-5, 1) -> -0.5,
    /// (12345, 3) -> 12.345. decimals 0..6.
    pub fn fixed(&mut self, scaled: i32, decimals: u8) {
        self.guarded(|w| {
            if decimals > 6 {
                return false;
            }
            let pow10 = 10i64.pow(u32::from(decimals));
            let sign = if scaled < 0 { "-" } else { "" };
            let mag = i64::from(scaled).abs();
            let mut tmp = [0u8; 32];
            let n = if decimals == 0 {
                fmt_fit(&mut tmp, format_args!("{sign}{mag}"))
            } else {
                let (int, frac, width) = (mag / pow10, mag % pow10, usize::from(decimals));
                fmt_fit(&mut tmp, format_args!("{sign}{int}.{frac:0width$}"))
            };
            w.before_value() && w.put_raw(&tmp[..n])
        });
    }

    /// Finite double with `decimals` (0..6) digits (C `%.*f`); NaN/Inf write null.
    pub fn number(&mut self, v: f64, decimals: u8) {
        if !v.is_finite() {
            self.null_value();
            return;
        }
        self.guarded(|w| {
            if decimals > 6 || v.abs() > 1e15 {
                return false;
            }
            let mut tmp = [0u8; 40];
            let n = format_f64_fixed(v, decimals, &mut tmp);
            w.before_value() && w.put_raw(&tmp[..n])
        });
    }

    /// Pre-formatted, trusted JSON fragment (e.g. a nested document; a C string) written
    /// verbatim as one value. Empty -> misuse.
    pub fn raw(&mut self, json: impl AsRef<[u8]>) {
        let json = c_str(json.as_ref());
        self.guarded(|w| !json.is_empty() && w.before_value() && w.put_raw(json));
    }
}

/// Writes the C string `s` JSON-escaped (without quotes) into `out`; for places that embed a
/// string in a larger template. Returns the length, 0 if it does not fit.
pub fn json_escape(s: &[u8], out: &mut [u8]) -> usize {
    let mut w = crate::common::TextBuf::new(out);
    for &c in c_str(s) {
        let (e, n) = escape_char(c);
        if !w.push_bytes(&e[..n]) {
            return 0;
        }
    }
    w.len()
}

#[cfg(test)]
mod tests;
