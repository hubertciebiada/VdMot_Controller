//! Port of the arg_parser cases of test/native/test_fuzz.cpp (same seeds and iteration
//! counts).

use super::*;
use crate::buf_writer::StaticBufWriter;
use crate::test_support::{random_bytes, text, Rng};

#[test]
fn parse_u32_parse_i32_agree_with_strtoll_on_random_text() {
    let mut rng = Rng::new(0xDEADBEEF);
    const CHARS: &[u8] = b"0123456789+- x";
    for round in 0..20000 {
        let len = rng.range_usize(0, 12);
        let s: std::vec::Vec<u8> = (0..len)
            .map(|_| {
                let pick = rng.range_usize(0, CHARS.len() - 1);
                if round % 3 == 0 {
                    CHARS[pick]
                } else {
                    b'0' + (pick % 10) as u8
                }
            })
            .collect();
        let a = rng.next_u32() % 100_000;
        let mut b = rng.next_u32();
        if round % 4 == 0 {
            b = a + rng.next_u32() % 1000;
        }
        let (lo, hi) = (a.min(b), a.max(b));

        // Reference: digits only, fits into [lo, hi].
        let digits_only = !s.is_empty() && s.iter().all(u8::is_ascii_digit);
        let reference = if digits_only {
            text(&s)
                .parse::<u64>()
                .ok()
                .filter(|&v| v >= u64::from(lo) && v <= u64::from(hi))
        } else {
            None
        };
        assert_eq!(
            parse_u32(&s, lo, hi).map(u64::from),
            reference,
            "{:?}",
            text(&s)
        );

        let slo = lo as i32 - 50_000;
        let shi = (hi / 2) as i32;
        let mut sreference = None;
        if !s.is_empty() {
            let digits_from = usize::from(s[0] == b'+' || s[0] == b'-');
            let digits = &s[digits_from..];
            if !digits.is_empty() && digits.iter().all(u8::is_ascii_digit) {
                sreference = text(&s).parse::<i64>().ok().filter(|&v| {
                    v >= i64::from(i32::MIN)
                        && v <= i64::from(i32::MAX)
                        && slo <= shi
                        && v >= i64::from(slo)
                        && v <= i64::from(shi)
                });
            }
        }
        assert_eq!(
            parse_i32(&s, slo, shi).map(i64::from),
            sreference,
            "{:?}",
            text(&s)
        );
    }
}

#[test]
fn parse_one_wire_address_round_trips_and_rejects_random_text() {
    let mut rng = Rng::new(0x1111);
    for _round in 0..5000 {
        let mut addr = [0u8; 8];
        for b in &mut addr {
            *b = rng.byte();
        }
        let mut w = StaticBufWriter::<24>::default();
        assert!(w.append_one_wire_address(&addr));
        assert_eq!(parse_one_wire_address(w.as_bytes()), Some(addr));

        // Corrupt one character: only another hex digit in a digit slot keeps it valid.
        let mut corrupt = w.as_bytes().to_vec();
        let pos = rng.range_usize(0, corrupt.len() - 1);
        let replacement = rng.byte();
        corrupt[pos] = replacement;
        let is_sep_slot = pos % 3 == 2;
        let is_hex = replacement.is_ascii_hexdigit();
        let expect_ok = if is_sep_slot {
            replacement == b'-'
        } else {
            is_hex
        };
        assert_eq!(
            parse_one_wire_address(&corrupt).is_some(),
            expect_ok,
            "{:?}",
            text(&corrupt)
        );
    }
    for _round in 0..5000 {
        let junk = random_bytes(&mut rng, 40);
        // stop at the first NUL like the C++ std::string(junk.c_str())
        let end = junk.iter().position(|&b| b == 0).unwrap_or(junk.len());
        let s = &junk[..end];
        if parse_one_wire_address(s).is_some() {
            assert_eq!(s.len(), ONE_WIRE_ADDRESS_TEXT_LEN);
        }
    }
}
