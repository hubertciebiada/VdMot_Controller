//! Shared constants, value types and bounded string/number helpers for the VdMot Revamped ESP
//! core (port of `vdm/common.h`). Hardware-free.
//!
//! Conventions used by every core module:
//!  - No heap, fixed-size storage only.
//!  - Valve and sensor indices are 0-based inside the core. The 1-based numbers seen by users
//!    (web, MQTT fallback segments) are converted at the edges.
//!  - Time is passed in explicitly: `now_ms` is a monotonic millisecond counter that wraps at
//!    2^32 (Arduino `millis()`). Always compare with [`elapsed_ms`] / [`time_reached`], never with
//!    `<` on raw timestamps.
//!  - Text is bytes (`&[u8]`), never assumed UTF-8. A C++ output buffer `char* out, size_t cap`
//!    is `out: &mut [u8]` with the C++ capacity as its length: one byte of it stays reserved for
//!    the NUL, which Rust does not write, so the limits are those of the C++ buffer. The
//!    functions return the text length; the text is `&out[..n]` ([`TextBuf`]).
//!  - A C++ `char f[N + 1]` member is a [`Text<N>`](Text).
//!  - A C++ NUL-terminated input is a `&[u8]` read up to its end or its first NUL, whichever
//!    comes first: `f(s)` in Rust gives what `f(s + NUL)` gives in C++ ([`c_str`]).

use core::fmt;

/// ACTUATOR_COUNT
pub const VALVE_COUNT: u8 = 12;
/// TEMP_SENSORS_COUNT (config slots and STM bus max)
pub const TEMP_SLOT_COUNT: u8 = 34;
/// VOLT_SENSORS_COUNT (config slots and STM bus max)
pub const VOLT_SLOT_COUNT: u8 = 8;
/// "all valves" selector on the STM protocol
pub const ALL_VALVES: u8 = 255;
/// "request/event is not about a valve"
pub const NO_VALVE: u8 = 0xFE;

/// chars, without NUL (legacy char[21])
pub const STATION_NAME_MAX: usize = 20;
/// valve/sensor names (legacy char[11])
pub const ITEM_NAME_MAX: usize = 10;
/// volt sensor unit (legacy char[9])
pub const UNIT_MAX: usize = 8;
/// "28-84-37-94-97-ff-03-23"
pub const ONE_WIRE_ID_TEXT_LEN: usize = 23;

// Temperatures are int16 tenths of a degree Celsius, exactly as the STM sends them. These raw
// values are sentinels, never real readings.
/// not assigned / never read
pub const TEMP_UNASSIGNED: i16 = -500;
/// DS18 DEVICE_DISCONNECTED_C
pub const TEMP_READ_ERROR: i16 = -1270;
/// DS18 power-on reset value (85.0)
pub const TEMP_POWER_ON: i16 = 850;
/// Voltage sensors: int32 in 10 mV units; this and below means CRC failure.
pub const VAD_FAILED: i32 = -1000;

// ---------------------------------------------------------------- time

/// Milliseconds elapsed from `since` to `now`, correct across one wrap.
pub fn elapsed_ms(now: u32, since: u32) -> u32 {
    now.wrapping_sub(since)
}

/// True when `now` is at or after `deadline` (wrap-safe, horizon 2^31 ms).
/// Only for deadlines that are always checked soon after they are set; a deadline that may sit
/// unchecked for 24.8 days reads as "in the future" again. Use [`Backoff`] (or [`elapsed_ms`]
/// from a start time) for those.
pub fn time_reached(now: u32, deadline: u32) -> bool {
    now.wrapping_sub(deadline) < 0x8000_0000
}

/// Retry pacing with exponential back-off (reconnects). Keeps the time of the last attempt, not
/// a deadline, so an attempt that is due stays due however long nobody asked (e.g. a connection
/// that was up for 30 days).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Backoff {
    min: u32,
    max: u32,
    /// wait after the next failure
    delay: u32,
    /// wait after the last failure
    wait_ms: u32,
    last_ms: u32,
    armed: bool,
}

impl Backoff {
    /// `min_ms` >= 1 (0 is raised to 1); `max_ms` < `min_ms` is raised to `min_ms`.
    pub fn new(min_ms: u32, max_ms: u32) -> Self {
        let min = min_ms.max(1);
        let max = max_ms.max(min);
        Self {
            min,
            max,
            delay: min,
            wait_ms: 0,
            last_ms: 0,
            armed: true,
        }
    }

    /// True when armed (no failed attempt since [`reset`](Self::reset)) or when the current
    /// delay has passed since the last failed attempt.
    pub fn due(&self, now_ms: u32) -> bool {
        self.armed || elapsed_ms(now_ms, self.last_ms) >= self.wait_ms
    }

    /// An attempt at `now_ms` failed: the next is due after the current delay, which then
    /// doubles up to the maximum.
    pub fn on_failure(&mut self, now_ms: u32) {
        self.armed = false;
        self.last_ms = now_ms;
        self.wait_ms = self.delay;
        self.delay = if self.delay >= self.max / 2 {
            self.max
        } else {
            self.delay * 2
        };
    }

    /// Success, or a forced retry: due at once, delay back to the minimum.
    pub fn reset(&mut self) {
        self.armed = true;
        self.delay = self.min;
    }

    /// Wait after the next failure.
    pub fn delay_ms(&self) -> u32 {
        self.delay
    }
}

/// Broken-down local wall-clock time, filled by glue from `localtime_r()` after the POSIX TZ
/// string has been applied. `valid` is false until SNTP has set the clock (year >= 2020).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LocalTime {
    pub valid: bool,
    /// e.g. 2026
    pub year: u16,
    /// 1..12
    pub month: u8,
    /// 1..31
    pub mday: u8,
    /// 0 = Sunday .. 6 = Saturday (tm_wday)
    pub wday: u8,
    /// 0..23
    pub hour: u8,
    /// 0..59
    pub minute: u8,
    /// 0..60
    pub second: u8,
    /// UTC seconds since 1970 of the same instant
    pub epoch: i64,
}

// ---------------------------------------------------------------- bounded text output

/// Text member of fixed capacity: the Rust form of a C++ `char f[N + 1]` member. Bytes (a C++
/// copy may cut a UTF-8 sequence), no NUL; set it with [`copy_string`].
pub type Text<const N: usize> = heapless::Vec<u8, N>;

/// Any [`Text<N>`](Text) behind a reference: `&mut Text<N>` coerces to `&mut TextView`.
pub type TextView = heapless::VecView<u8>;

/// Bounded text output into a caller buffer: the Rust form of a C++ `char* out, size_t cap`.
///
/// The buffer length is the C++ capacity, NUL included, so at most `buf.len() - 1` bytes of
/// text fit (none for an empty buffer); the NUL itself is not written. A write that does not fit
/// keeps what fits (like `snprintf`) and marks the output as overflowed; then [`fit`](Self::fit)
/// gives the C++ "0 (and "") when it does not fit" result and [`len`](Self::len) the truncated
/// one. Implements [`fmt::Write`], so `write!(w, "{}.{}", a, b)` works (integers only: floats go
/// through [`format_f64_fixed`]).
#[derive(Debug)]
pub struct TextBuf<'a> {
    buf: &'a mut [u8],
    len: usize,
    overflow: bool,
}

impl<'a> TextBuf<'a> {
    /// An empty text in `buf`.
    pub fn new(buf: &'a mut [u8]) -> Self {
        Self {
            buf,
            len: 0,
            overflow: false,
        }
    }

    /// Bytes written (what fits after an overflow).
    pub fn len(&self) -> usize {
        self.len
    }

    /// Nothing written.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// A write did not fit.
    pub fn overflowed(&self) -> bool {
        self.overflow
    }

    /// The text written so far.
    pub fn as_bytes(&self) -> &[u8] {
        self.buf.get(..self.len).unwrap_or_default()
    }

    /// Appends one byte; false (and overflowed) when there is no room for it and the NUL.
    pub fn push(&mut self, c: u8) -> bool {
        self.push_bytes(&[c])
    }

    /// Appends `s`, or what fits of it; false (and overflowed from then on) when it was cut.
    pub fn push_bytes(&mut self, s: &[u8]) -> bool {
        let room = self.buf.len().saturating_sub(1) - self.len;
        let n = s.len().min(room);
        if let (Some(dst), Some(src)) = (self.buf.get_mut(self.len..self.len + n), s.get(..n)) {
            dst.copy_from_slice(src);
            self.len += n;
        }
        let fits = n == s.len();
        self.overflow |= !fits;
        fits
    }

    /// The C++ "length, 0 (and "") when it does not fit" result.
    pub fn fit(&self) -> usize {
        if self.overflow {
            0
        } else {
            self.len
        }
    }

    /// The C++ "true, false when it does not fit" result, with the length.
    pub fn fit_opt(&self) -> Option<usize> {
        if self.overflow {
            None
        } else {
            Some(self.len)
        }
    }
}

impl fmt::Write for TextBuf<'_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        if self.push_bytes(s.as_bytes()) {
            Ok(())
        } else {
            Err(fmt::Error)
        }
    }
}

/// C `snprintf(out, cap, ...)` with the C++ "length, 0 (and "") when it does not fit" rule:
/// `fmt_fit(out, format_args!("{}.{}", a, b))`.
pub fn fmt_fit(out: &mut [u8], args: fmt::Arguments<'_>) -> usize {
    let mut w = TextBuf::new(out);
    let _ = fmt::Write::write_fmt(&mut w, args);
    w.fit()
}

/// C `snprintf(out, cap, ...)` keeping what fits: the C++ `n < cap ? n : cap - 1` rule (0 for
/// an empty buffer).
pub fn fmt_trunc(out: &mut [u8], args: fmt::Arguments<'_>) -> usize {
    let mut w = TextBuf::new(out);
    let _ = fmt::Write::write_fmt(&mut w, args);
    w.len()
}

/// C `snprintf(out, cap, "%.*f", decimals, v)` with integer arithmetic, exactly like glibc: the
/// exact binary value rounded half to even, a '-' for every negative sign bit (also "-0.00").
/// For finite `v` with |v| < 2^128 (every `f32`) and `decimals` 0..=18; returns the length, 0
/// when `v` or `decimals` is outside that range or the text does not fit.
pub fn format_f64_fixed(v: f64, decimals: u8, out: &mut [u8]) -> usize {
    let bits = v.to_bits();
    let neg = bits >> 63 != 0;
    let exp = ((bits >> 52) & 0x7FF) as i32;
    let frac = bits & 0x000F_FFFF_FFFF_FFFF;
    if exp == 0x7FF || decimals > 18 {
        return 0; // infinity, NaN
    }
    // v = m * 2^e; a normal number adds the implicit leading bit 52
    let (m, e) = if exp == 0 {
        (frac, -1074)
    } else {
        (frac + (1 << 52), exp - 1075)
    };
    let pow = 10u128.pow(u32::from(decimals));
    let (int_part, frac_part) = if e >= 0 {
        if e > 75 {
            return 0; // |v| >= 2^128
        }
        (u128::from(m) << e, 0)
    } else {
        // round(m * 10^decimals / 2^k), half to even; beyond k = 127 the value is below 2^-14
        // and rounds to 0 like at k = 127
        let k = e.unsigned_abs().min(127);
        let num = u128::from(m) * pow;
        let mut q = num >> k;
        let r = num - (q << k);
        let half = 1u128 << (k - 1);
        if r > half || (r == half && q & 1 == 1) {
            q += 1;
        }
        (q / pow, q % pow)
    };
    let mut w = TextBuf::new(out);
    if neg {
        w.push(b'-');
    }
    let _ = fmt::Write::write_fmt(&mut w, format_args!("{int_part}"));
    if decimals > 0 {
        let width = usize::from(decimals);
        let _ = fmt::Write::write_fmt(&mut w, format_args!(".{frac_part:0width$}"));
    }
    w.fit()
}

// ---------------------------------------------------------------- strings

/// A C string in a byte buffer: `s` up to its first NUL (all of `s` without one).
pub fn c_str(s: &[u8]) -> &[u8] {
    let n = s.iter().position(|&c| c == 0).unwrap_or(s.len());
    s.get(..n).unwrap_or(s)
}

/// True when `needle` occurs in `haystack` (C `strstr` != null; an empty needle always does).
pub fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    needle.is_empty() || haystack.windows(needle.len()).any(|w| w == needle)
}

/// Copies the C string `src` into the text member `dst` (C++ `copyString(dst, N + 1, src)`).
/// Returns false, and leaves the truncated prefix in `dst`, when `src` does not fit.
pub fn copy_string(dst: &mut TextView, src: &[u8]) -> bool {
    let src = c_str(src);
    let n = src.len().min(dst.capacity());
    dst.clear();
    let _ = dst.extend_from_slice(src.get(..n).unwrap_or_default());
    n == src.len()
}

/// Length of `s` bounded by `max` (like strnlen: up to the first NUL, the end of `s` or `max`).
pub fn bounded_length(s: &[u8], max: usize) -> usize {
    c_str(s).len().min(max)
}

/// Length (2..4) of the well-formed UTF-8 sequence starting at `s[0]` >= 0x80, looking at no
/// more than `s.len()` bytes; 0 when it is not one (stray continuation byte, overlong form,
/// surrogate, above U+10FFFF, truncated) or when it encodes a C1 control (U+0080..U+009F).
pub fn utf8_sequence_length(s: &[u8]) -> usize {
    let n = match s.first() {
        Some(0xC2..=0xDF) => 2,
        Some(0xE0..=0xEF) => 3,
        Some(0xF0..=0xF4) => 4,
        _ => return 0, // ASCII, continuation byte, C0/C1 overlong lead, or > U+13FFFF
    };
    // n bytes that decode to one char: continuation bytes, no overlong form, no surrogate,
    // at most U+10FFFF (the checks of the C++ decoder, done by core's UTF-8 validation)
    match decode_utf8(s.get(..n)) {
        Some(cp) if cp > 0x9F => n,
        _ => 0, // malformed, truncated or a C1 control
    }
}

/// The first code point of `seq` when all of it is well-formed UTF-8 (callers pass exactly the
/// bytes of one sequence).
fn decode_utf8(seq: Option<&[u8]>) -> Option<u32> {
    core::str::from_utf8(seq?)
        .ok()?
        .chars()
        .next()
        .map(u32::from)
}

/// All of `s` is printable text: ASCII 0x20..0x7E and well-formed UTF-8 without control
/// characters (the JSON writer passes it through).
pub fn is_printable_text(s: &[u8]) -> bool {
    let mut next = 0; // start of the next character
    for (i, &c) in s.iter().enumerate() {
        if i < next {
            continue; // inside a multibyte sequence
        }
        let n = if c >= 0x80 {
            utf8_sequence_length(s.get(i..).unwrap_or_default())
        } else {
            usize::from(c >= 0x20 && c != 0x7F)
        };
        if n == 0 {
            return false;
        }
        next = i + n;
    }
    true
}

/// Character policy for every user-supplied name that ends up in MQTT topics, HA discovery or
/// HTTP JSON (station, valve, sensor names, units): printable text ([`is_printable_text`], so
/// UTF-8 like "Küche" or "°C" is fine, as in the legacy firmware) except '+', '#', '/', '"',
/// '\\'. Length 0..`max_len` bytes (0 only when `allow_empty`). Spaces are allowed anywhere;
/// topic segments map them to '_' like the legacy firmware (buildSegment).
pub fn is_safe_name(s: &[u8], max_len: usize, allow_empty: bool) -> bool {
    let len = bounded_length(s, max_len.wrapping_add(1));
    if len > max_len {
        return false;
    }
    if len == 0 {
        return allow_empty;
    }
    let s = s.get(..len).unwrap_or_default();
    !s.iter()
        .any(|&c| matches!(c, b'+' | b'#' | b'/' | b'"' | b'\\'))
        && is_printable_text(s)
}

/// Network host name (DHCP, syslog) derived from the station name: ASCII letters, digits, '-'
/// and '_' are kept, every run of other bytes (spaces, UTF-8, punctuation) becomes one '-',
/// leading/trailing '-' are dropped, at most `out.len() - 1` chars. "VdMot" when nothing is
/// left. Returns the length (0 only for `out.len()` < 6).
pub fn build_hostname(station: &[u8], out: &mut [u8]) -> usize {
    const DEFAULT: &[u8; 5] = b"VdMot";
    let cap = out.len();
    if cap <= DEFAULT.len() {
        return 0;
    }
    let mut n = 0;
    let mut dash = false; // a run of replaced bytes is pending
    for &c in c_str(station) {
        if n + 1 >= cap {
            break;
        }
        let keep = c.is_ascii_alphanumeric() || c == b'-' || c == b'_';
        if !keep {
            dash = n > 0; // never a leading '-'
            continue;
        }
        if dash && c != b'-' && out[n - 1] != b'-' {
            out[n] = b'-';
            n += 1;
            if n + 1 >= cap {
                break;
            }
        }
        dash = false;
        out[n] = c;
        n += 1;
    }
    // drop the trailing and the leading '-'
    let end = out[..n]
        .iter()
        .rposition(|&c| c != b'-')
        .map_or(0, |last| last + 1);
    let first = out[..end].iter().take_while(|&&c| c == b'-').count();
    out.copy_within(first..end, 0);
    let n = end - first;
    if n == 0 {
        out[..DEFAULT.len()].copy_from_slice(DEFAULT);
        return DEFAULT.len();
    }
    n
}

/// Hostname/broker-host policy: 1..`max_len` chars of [A-Za-z0-9.-], not starting or ending
/// with '-' or '.'. Used for the MQTT broker host and NTP server.
pub fn is_host_name(s: &[u8], max_len: usize) -> bool {
    let len = bounded_length(s, max_len.wrapping_add(1));
    if len > max_len {
        return false;
    }
    match s.get(..len).unwrap_or_default() {
        [] | [b'-' | b'.', ..] | [.., b'-' | b'.'] => false,
        name => name
            .iter()
            .all(|&c| c.is_ascii_alphanumeric() || c == b'-' || c == b'.'),
    }
}

/// U+00C0..U+017F -> ASCII base letter; '\0' = two-letter form (`TWO_LETTER`), '_' = not a
/// letter (U+00D7, U+00F7).
const LATIN_BASE: &[u8; 192] = b"AAAAAA\0CEEEEIIII\
DNOOOOO_OUUUUY\0\0\
aaaaaa\0ceeeeiiii\
dnooooo_ouuuuy\0y\
AaAaAaCcCcCcCcDd\
DdEeEeEeEeEeGgGg\
GgGgHhHhIiIiIiIi\
Ii\0\0JjKkkLlLlLlL\
lLlNnNnNnnNnOoOo\
Oo\0\0RrRrRrSsSsSs\
SsTtTtTtUuUuUuUu\
UuUuWwYyYZzZzZzs";
const TWO_LETTER_CP: [u32; 9] = [0xC6, 0xDE, 0xDF, 0xE6, 0xFE, 0x132, 0x133, 0x152, 0x153];
const TWO_LETTER: &[u8; 18] = b"AETHssaethIJijOEoe";

fn is_id_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c == b'-'
}

/// HA discovery id part (node_id / object_id must be [A-Za-z0-9_-], HA discovery.py
/// TOPIC_MATCHER) of `input` (ends at a NUL): ASCII letters, digits, '_' and '-' are kept; every
/// other ASCII byte (space, '.', ',', ...) becomes '_'; a well-formed UTF-8 letter U+00C0..U+017F
/// becomes its ASCII base letter (ł -> l, ü -> u, é -> e), with Æ -> AE, æ -> ae, Þ -> TH,
/// þ -> th, ß -> ss, Ĳ -> IJ, ĳ -> ij, Œ -> OE, œ -> oe; × and ÷ and every other sequence (any
/// script, symbols) become one '_', every invalid byte one '_'. The output is never longer than
/// the input. Returns the length; 0 (and "") when it does not fit or the input is empty.
pub fn build_ha_id(input: &[u8], out: &mut [u8]) -> usize {
    let mut w = TextBuf::new(out);
    let mut rest = c_str(input);
    while let [c, ..] = *rest {
        let mut id = [b'_', 0];
        let mut id_len = 1;
        let mut step = 1;
        if c.is_ascii() {
            if is_id_char(c) {
                id[0] = c;
            }
        } else {
            let seq = utf8_sequence_length(rest);
            step = seq.max(1); // an invalid byte is one '_' of its own
            let cp = if seq == 2 {
                decode_utf8(rest.get(..2)).unwrap_or(0)
            } else {
                0
            };
            // U+00C0..U+017F; every other code point (and 0) misses the table
            if let Some(&base) = LATIN_BASE.get((cp as usize).wrapping_sub(0xC0)) {
                id[0] = base;
            }
            if let Some(k) = TWO_LETTER_CP.iter().position(|&t| t == cp) {
                id = [TWO_LETTER[2 * k], TWO_LETTER[2 * k + 1]];
                id_len = 2;
            }
        }
        if !w.push_bytes(&id[..id_len]) {
            return 0;
        }
        rest = rest.get(step..).unwrap_or_default();
    }
    w.len()
}

// ---------------------------------------------------------------- numbers

/// 1..10 decimal digits into a 64-bit accumulator.
fn parse_digits(s: &[u8]) -> Option<u64> {
    if s.is_empty() || s.len() > 10 {
        return None;
    }
    let mut v: u64 = 0;
    for &c in s {
        if !c.is_ascii_digit() {
            return None;
        }
        v = v * 10 + u64::from(c - b'0');
    }
    Some(v)
}

/// Strict decimal parser over all of `s`: 1..10 digits, nothing else (no sign, no spaces, no
/// '+', no hex, no exponent). Values above `max` are rejected.
pub fn parse_uint(s: &[u8], max: u32) -> Option<u32> {
    let v = parse_digits(s)?;
    if v > u64::from(max) {
        return None;
    }
    u32::try_from(v).ok()
}

/// Strict decimal parser over all of `s`: optional '-', then 1..10 digits, nothing else.
/// Values outside [`min`, `max`] are rejected.
pub fn parse_int(s: &[u8], min: i32, max: i32) -> Option<i32> {
    let (neg, digits) = match s {
        [b'-', rest @ ..] => (true, rest),
        _ => (false, s),
    };
    let mag = parse_digits(digits)? as i64;
    let v = if neg { -mag } else { mag };
    if v < i64::from(min) || v > i64::from(max) {
        return None;
    }
    i32::try_from(v).ok()
}

/// Dotted IPv4 "a.b.c.d" (each 0..255, 1..3 digits) to the legacy uint32 layout used in NVS:
/// first octet in the LOW byte ("192.168.1.2" -> 0x0201A8C0), i.e. what Arduino IPAddress casts
/// to.
pub fn parse_ipv4(s: &[u8]) -> Option<u32> {
    let mut octets = [0u8; 4];
    let mut parts = s.split(|&c| c == b'.');
    for octet in &mut octets {
        let part = parts.next()?;
        if part.len() > 3 {
            return None;
        }
        *octet = u8::try_from(parse_uint(part, 255)?).ok()?;
    }
    if parts.next().is_some() {
        return None;
    }
    Some(u32::from_le_bytes(octets))
}

/// Inverse of [`parse_ipv4`]. `out` needs >= 16 bytes; returns chars written (0 when it does
/// not fit).
pub fn format_ipv4(ip: u32, out: &mut [u8]) -> usize {
    let [a, b, c, d] = ip.to_le_bytes();
    fmt_fit(out, format_args!("{a}.{b}.{c}.{d}"))
}

/// The one rounding rule for target percentages (MQTT payloads with fractions): None for NaN,
/// infinities, v < 0 and v > 100; else floor(v + 0.5), so 43.5 -> 44 and -0.0 -> 0.
pub fn round_target_percent(v: f64) -> Option<u8> {
    if !(0.0..=100.0).contains(&v) {
        return None; // also NaN
    }
    Some((v + 0.5) as u8)
}

// ---------------------------------------------------------------- 1-Wire

/// 1-Wire ROM id, family byte first (`b[0]`) and CRC last (`b[7]`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OneWireId {
    pub b: [u8; 8],
}

/// All eight bytes are 0.
pub fn is_zero(id: &OneWireId) -> bool {
    id.b.iter().all(|&v| v == 0)
}

/// Dallas/Maxim CRC8 (poly 0x31 reflected) over `b[0..7]` equals `b[7]`.
pub fn crc_valid(id: &OneWireId) -> bool {
    let mut crc = 0u8;
    for &byte in &id.b[..7] {
        let mut x = byte;
        for _ in 0..8 {
            let mix = (crc ^ x) & 0x01;
            crc >>= 1;
            if mix != 0 {
                crc ^= 0x8C;
            }
            x >>= 1;
        }
    }
    crc == id.b[7]
}

fn hex_value(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// Parses exactly 23 chars "hh-hh-hh-hh-hh-hh-hh-hh", hex case-insensitive.
pub fn parse_one_wire_id(s: &[u8]) -> Option<OneWireId> {
    if s.len() != ONE_WIRE_ID_TEXT_LEN {
        return None;
    }
    let mut id = OneWireId::default();
    for (slot, chunk) in id.b.iter_mut().zip(s.chunks(3)) {
        let (hi, lo) = match chunk {
            [hi, lo, b'-'] | [hi, lo] => (hex_value(*hi)?, hex_value(*lo)?),
            _ => return None,
        };
        *slot = hi * 16 + lo;
    }
    Some(id)
}

/// Writes the 23-char lowercase form; `out` needs >= 24 bytes (0 when it does not fit).
pub fn format_one_wire_id(id: &OneWireId, out: &mut [u8]) -> usize {
    let [a, b, c, d, e, f, g, h] = id.b;
    fmt_fit(
        out,
        format_args!("{a:02x}-{b:02x}-{c:02x}-{d:02x}-{e:02x}-{f:02x}-{g:02x}-{h:02x}"),
    )
}

#[cfg(test)]
mod tests;
