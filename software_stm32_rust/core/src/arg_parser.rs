//! Strict parsers for protocol arguments. Hardware-free.
//!
//! The C++ `bool parseX(const char* s, ..., T& out)` writes `out` only on success; the Rust
//! functions return `Some(value)` on success and `None` otherwise. A C string is the byte
//! slice of its characters (a token never holds a NUL: the line assembler drops such lines).

/// Parses the digit-only magnitude; `None` on empty input, a non-digit or a
/// value above `limit`.
fn parse_magnitude(s: &[u8], limit: u32) -> Option<u32> {
    if s.is_empty() {
        return None;
    }
    let mut value: u32 = 0;
    for &c in s {
        if !c.is_ascii_digit() {
            return None;
        }
        let digit = u32::from(c - b'0');
        if value > limit / 10 || (value == limit / 10 && digit > limit % 10) {
            return None;
        }
        value = value * 10 + digit;
    }
    Some(value)
}

fn hex_value(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// Decimal digits only (no sign, no whitespace, no trailing characters),
/// overflow-checked, value within [lo, hi].
pub fn parse_u32(s: &[u8], lo: u32, hi: u32) -> Option<u32> {
    let value = parse_magnitude(s, hi)?;
    if value < lo {
        return None;
    }
    Some(value)
}

/// Optional leading '+' or '-', then decimal digits; otherwise as [`parse_u32`].
pub fn parse_i32(s: &[u8], lo: i32, hi: i32) -> Option<i32> {
    let (negative, digits) = match s.split_first() {
        Some((b'-', rest)) => (true, rest),
        Some((b'+', rest)) => (false, rest),
        _ => (false, s),
    };
    // 2147483648 is representable only as a negative value.
    let limit = if negative {
        2_147_483_648
    } else {
        2_147_483_647
    };
    let magnitude = parse_magnitude(digits, limit)?;
    let value = if negative {
        0i32.wrapping_sub_unsigned(magnitude)
    } else {
        magnitude as i32
    };
    if value < lo || value > hi {
        return None;
    }
    Some(value)
}

/// Length of the text form "28-84-37-94-97-ff-03-23" of a 1-Wire ROM address.
pub const ONE_WIRE_ADDRESS_TEXT_LEN: usize = 23;

/// Parses exactly two hex digits (either case) per byte separated by '-'.
pub fn parse_one_wire_address(s: &[u8]) -> Option<[u8; 8]> {
    // the C++ reads the string up to its NUL: a byte behind the slice reads as NUL
    let at = |k: usize| s.get(k).copied().unwrap_or(0);
    let mut parsed = [0u8; 8];
    for (i, byte) in parsed.iter_mut().enumerate() {
        let p = i * 3;
        let hi = hex_value(at(p))?;
        let lo = hex_value(at(p + 1))?;
        let sep = if i < 7 { b'-' } else { 0 };
        if at(p + 2) != sep {
            return None;
        }
        *byte = hi * 16 + lo;
    }
    Some(parsed)
}

/// True if all 8 bytes are zero (the protocol's "no sensor" address).
pub fn is_zero_address(addr: &[u8; 8]) -> bool {
    addr.iter().all(|&b| b == 0)
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_fuzz;
