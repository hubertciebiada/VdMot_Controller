//! Port of test/native/test_common.cpp: time, Backoff, bounded strings, UTF-8, names, host
//! names, HA ids, strict number parsers, IPv4, target rounding and 1-Wire ids. Every boundary
//! is checked from both sides (written against mutation testing). C++ cases that pass a null
//! pointer have no Rust form (a slice is never null); they are named in comments. After the
//! tests of the C++ file: the Rust-only helpers (TextBuf, fmt_*, format_f64_fixed, c_str,
//! contains_bytes).

use super::*;
use crate::test_support::{assert_text, Lcg};
use std::format;
use std::string::{String, ToString};
use std::vec::Vec;

/// Reference Dallas/Maxim CRC-8 of the first 7 ROM bytes (bitwise, reflected 0x31),
/// independent of crc_valid().
fn ref_crc(x: &OneWireId) -> u8 {
    let mut crc = 0u8;
    for &byte in &x.b[..7] {
        for bit in 0..8 {
            let mix = ((crc ^ (byte >> bit)) & 1) != 0;
            crc >>= 1;
            if mix {
                crc ^= 0x8C;
            }
        }
    }
    crc
}

#[test]
fn elapsed_ms_and_time_reached() {
    assert_eq!(elapsed_ms(100, 40), 60);
    assert_eq!(elapsed_ms(40, 100), 0xFFFF_FFC4);
    assert_eq!(elapsed_ms(7, 7), 0);
    assert!(time_reached(100, 100));
    assert!(!time_reached(99, 100));
    assert!(time_reached(101, 100));
    assert!(time_reached(100, 50));
    assert!(!time_reached(50, 100));
    assert!(!time_reached(0, 1));
    assert!(time_reached(0x7FFF_FFFF, 0));
    assert!(!time_reached(0x8000_0000, 0));
    assert!(time_reached(3, 0xFFFF_FFFE));
    assert!(!time_reached(0xFFFF_FFFE, 3));
}

#[test]
fn backoff_exact_doubling_and_cap() {
    let mut b = Backoff::new(10, 21);
    assert_eq!(b.delay_ms(), 10);
    b.on_failure(0);
    assert_eq!(b.delay_ms(), 21); // 10 >= 21/2: straight to the cap
    assert!(!b.due(9));
    assert!(b.due(10));
    b.on_failure(10);
    assert_eq!(b.delay_ms(), 21);
    assert!(!b.due(30));
    assert!(b.due(31));

    let mut c = Backoff::new(3, 14); // 3 -> 6 -> 12 -> 14
    c.on_failure(0);
    assert_eq!(c.delay_ms(), 6);
    c.on_failure(0);
    assert_eq!(c.delay_ms(), 12);
    c.on_failure(0);
    assert_eq!(c.delay_ms(), 14);

    let mut one = Backoff::new(1, 1);
    assert_eq!(one.delay_ms(), 1);
    one.on_failure(5);
    assert_eq!(one.delay_ms(), 1);
    assert!(!one.due(5));
    assert!(one.due(6));

    let mut inv = Backoff::new(50, 10); // max below min is raised to min
    inv.on_failure(0);
    assert_eq!(inv.delay_ms(), 50);
    assert!(!inv.due(49));
    assert!(inv.due(50));
}

#[test]
fn backoff_reset_re_arms_at_once() {
    let mut b = Backoff::new(1000, 8000);
    b.on_failure(0);
    b.on_failure(1000);
    assert_eq!(b.delay_ms(), 4000);
    assert!(!b.due(1001));
    b.reset();
    assert!(b.due(1001));
    assert_eq!(b.delay_ms(), 1000);
    b.on_failure(2000);
    assert!(!b.due(2999));
    assert!(b.due(3000));
}

#[test]
fn backoff_zero_minimum_is_one() {
    // Rust addition: the header contract "minMs >= 1" (0 is raised to 1).
    let mut b = Backoff::new(0, 0);
    assert_eq!(b.delay_ms(), 1);
    assert!(b.due(0)); // armed
    b.on_failure(100);
    assert!(!b.due(100));
    assert!(b.due(101));
    assert_eq!(b.delay_ms(), 1);
    let mut c = Backoff::new(0, 5);
    c.on_failure(0);
    assert_eq!(c.delay_ms(), 2);
}

#[test]
fn copy_string_cases() {
    // C++ cap 0 ("writes nothing") and a null source have no Rust form: a Text<N> always has
    // room for N bytes and the NUL, and a slice is never null. C++ cap = N + 1 below.
    let mut t0: Text<0> = Text::new();
    assert!(copy_string(&mut t0, b""));
    assert!(t0.is_empty());
    assert!(!copy_string(&mut t0, b"a"));
    assert!(t0.is_empty());

    let mut t3: Text<3> = Text::new();
    assert!(copy_string(&mut t3, b"abc"));
    assert_text(&t3, "abc");
    let mut t2: Text<2> = Text::new();
    assert!(!copy_string(&mut t2, b"abc"));
    assert_text(&t2, "ab");
    let mut t7: Text<7> = Text::new();
    assert!(copy_string(&mut t7, b"ab"));
    assert_text(&t7, "ab");
    // Rust additions: the source is a C string, the old content is replaced.
    assert!(copy_string(&mut t7, b"x\0yz"));
    assert_text(&t7, "x");
    assert!(copy_string(&mut t7, b"1234567"));
    assert_text(&t7, "1234567");
    assert!(!copy_string(&mut t7, b"12345678"));
    assert_text(&t7, "1234567");
}

#[test]
fn bounded_length_cases() {
    // C++ boundedLength(nullptr, 5) == 0: no Rust form, the empty slice gives the same.
    assert_eq!(bounded_length(b"", 5), 0);
    assert_eq!(bounded_length(b"abc", 5), 3);
    assert_eq!(bounded_length(b"abc", 3), 3);
    assert_eq!(bounded_length(b"abc", 2), 2);
    assert_eq!(bounded_length(b"abc", 0), 0);
    // A NUL ends the string (strnlen).
    assert_eq!(bounded_length(b"ab\0cd", 5), 2);
    assert_eq!(bounded_length(b"\0", 5), 0);
}

#[test]
fn utf8_sequence_length_lead_byte_ranges() {
    // Every lead byte with valid-looking continuation bytes.
    for b0 in 0..=255u8 {
        let s = [b0, 0xa0, 0x90, 0x90];
        let expect = match b0 {
            0xC2..=0xDF => 2,
            0xE0 => 3, // cp 0x0810: not overlong
            0xED => 0, // 0xD810: surrogate
            0xE1..=0xEF => 3,
            0xF0..=0xF3 => 4,
            _ => 0, // 0xF4: 0x120410 > 0x10FFFF
        };
        assert_eq!(utf8_sequence_length(&s), expect, "b0 {b0:#x}");
    }
    // Leads outside the 4-byte range must not be decoded as 4-byte sequences.
    assert_eq!(utf8_sequence_length(b"\xc1\x80\x80\x80"), 0);
    assert_eq!(utf8_sequence_length(b"\xc0\x80\x80\x80"), 0);
    assert_eq!(utf8_sequence_length(b"\xfb\x80\x80\x80"), 0);
    assert_eq!(utf8_sequence_length(b"\xf9\x80\x80\x80"), 0);
    assert_eq!(utf8_sequence_length(b"\x81\x80\x80\x80"), 0);
    assert_eq!(utf8_sequence_length(b"\xf1\x80\x80\x80"), 4);
    assert_eq!(utf8_sequence_length(b"\xf4\x80\x80\x80"), 4);
    assert_eq!(utf8_sequence_length(b"\xef\x80\x80"), 3);
    assert_eq!(utf8_sequence_length(b"\xe1\x80\x80"), 3);
}

#[test]
fn utf8_sequence_length_avail_and_continuation_bytes() {
    // C++ (s, avail) is the slice s[..avail]; avail past the literal includes its NUL.
    assert_eq!(utf8_sequence_length(b"\xc3\xa4"), 2);
    assert_eq!(utf8_sequence_length(&b"\xc3\xa4"[..1]), 0);
    assert_eq!(utf8_sequence_length(b"\xe2\x82\xac"), 3);
    assert_eq!(utf8_sequence_length(&b"\xe2\x82\xac"[..2]), 0);
    assert_eq!(utf8_sequence_length(b"\xe2\x82\xac\0"), 3);
    assert_eq!(utf8_sequence_length(b"\xf0\x9f\x98\x80"), 4);
    assert_eq!(utf8_sequence_length(&b"\xf0\x9f\x98\x80"[..3]), 0);
    assert_eq!(utf8_sequence_length(b"\xf0\x9f\x98\x80\0"), 4);
    // Continuation must be 10xxxxxx in every position.
    assert_eq!(utf8_sequence_length(b"\xc3\xc0"), 0);
    assert_eq!(utf8_sequence_length(b"\xc3\x7f"), 0);
    assert_eq!(utf8_sequence_length(b"\xc3\xbf"), 2);
    assert_eq!(utf8_sequence_length(b"\xc3\x80"), 2);
    assert_eq!(utf8_sequence_length(b"\xe2\x82\xc0"), 0);
    assert_eq!(utf8_sequence_length(b"\xe2\x02\xac"), 0);
    assert_eq!(utf8_sequence_length(b"\xf0\x9f\x98\x40"), 0);
    assert_eq!(utf8_sequence_length(b"\xf0\x9f\xd8\x80"), 0);
    // Exact code point values at every limit.
    assert_eq!(utf8_sequence_length(b"\xc2\x9f"), 0); // U+009F
    assert_eq!(utf8_sequence_length(b"\xc2\xa0"), 2); // U+00A0
    assert_eq!(utf8_sequence_length(b"\xe0\x9f\xbf"), 0); // overlong U+07FF
    assert_eq!(utf8_sequence_length(b"\xe0\xa0\x80"), 3); // U+0800
    assert_eq!(utf8_sequence_length(b"\xed\x9f\xbf"), 3); // U+D7FF
    assert_eq!(utf8_sequence_length(b"\xed\xa0\x80"), 0); // U+D800
    assert_eq!(utf8_sequence_length(b"\xed\xbf\xbf"), 0); // U+DFFF
    assert_eq!(utf8_sequence_length(b"\xee\x80\x80"), 3); // U+E000
    assert_eq!(utf8_sequence_length(b"\xf0\x8f\xbf\xbf"), 0); // overlong U+FFFF
    assert_eq!(utf8_sequence_length(b"\xf0\x90\x80\x80"), 4); // U+10000
    assert_eq!(utf8_sequence_length(b"\xf4\x8f\xbf\xbf"), 4); // U+10FFFF
    assert_eq!(utf8_sequence_length(b"\xf4\x90\x80\x80"), 0); // U+110000
    assert_eq!(utf8_sequence_length(b"\xdf\xbf"), 2); // U+07FF
    assert_eq!(utf8_sequence_length(b"\xef\xbf\xbf"), 3); // U+FFFF
    assert_eq!(utf8_sequence_length(b""), 0); // C++ (nullptr, 0)
    assert_eq!(utf8_sequence_length(&b"\xc3\xa4"[..0]), 0);
}

#[test]
fn is_printable_text_cases() {
    // also the C++ (nullptr, 0); C++ isPrintableText(nullptr, 1) == false has no Rust form
    assert!(is_printable_text(b""));
    assert!(is_printable_text(b" "));
    assert!(is_printable_text(b"~"));
    assert!(!is_printable_text(b"\x1f"));
    assert!(!is_printable_text(b"\x7f"));
    assert!(!is_printable_text(b"\x80"));
    assert!(!is_printable_text(b"\x01"));
    assert!(!is_printable_text(b"a\x00b"));
    // A multibyte sequence is skipped as a whole, then ASCII continues.
    assert!(is_printable_text(b"\xc3\xa4a"));
    assert!(!is_printable_text(b"\xc3\xa4\x01"));
    assert!(is_printable_text(b"\xe2\x82\xacb\xc3\xa4"));
    assert!(!is_printable_text(b"\xe2\x82\xac\x1f"));
    assert!(is_printable_text(b"\xf0\x9f\x98\x80z"));
    assert!(!is_printable_text(b"\xf0\x9f\x98\x80\x7f"));
    // The sequence is bounded by len.
    assert!(!is_printable_text(&b"a\xe2\x82\xac"[..3]));
    assert!(is_printable_text(b"a\xe2\x82\xac"));
}

#[test]
fn is_safe_name_cases() {
    assert!(is_safe_name(b"abc", 3, false));
    assert!(!is_safe_name(b"abcd", 3, false));
    assert!(is_safe_name(b"abcd", 4, false));
    assert!(is_safe_name(b"", 0, true));
    assert!(!is_safe_name(b"", 0, false));
    assert!(!is_safe_name(b"a", 0, true));
    for bad in [
        &b"+"[..],
        b"#",
        b"/",
        b"\"",
        b"\\",
        b"x+",
        b"x#",
        b"x/",
        b"x\"",
        b"x\\",
        b"\x01",
    ] {
        assert!(!is_safe_name(bad, 5, false), "{}", bad.escape_ascii());
        assert!(!is_safe_name(bad, 5, true), "{}", bad.escape_ascii());
    }
    for ok in [&b"-"[..], b".", b"a b", b"*", b"$", b"\xc3\xa4"] {
        assert!(is_safe_name(ok, 5, false), "{}", ok.escape_ascii());
    }
    // Rust addition: the name is a C string (ends at a NUL), a huge limit does not wrap.
    assert!(is_safe_name(b"ab\0/", 3, false));
    assert!(is_safe_name(b"ab", usize::MAX - 1, false));
}

#[test]
fn build_hostname_details() {
    let cases: [(&[u8], &str); 28] = [
        (b"a", "a"),
        (b"z", "z"),
        (b"A", "A"),
        (b"Z", "Z"),
        (b"0", "0"),
        (b"9", "9"),
        (b"-", "VdMot"),
        (b"_", "_"),
        (b"a`b", "a-b"),
        (b"a{b", "a-b"),
        (b"a@b", "a-b"),
        (b"a[b", "a-b"),
        (b"a/b", "a-b"),
        (b"a:b", "a-b"),
        (b"a-b", "a-b"),
        (b"a --b", "a--b"),
        (b"a- b", "a-b"),
        (b"a -b", "a-b"),
        (b"a  b", "a-b"),
        (b"a_ b", "a_-b"),
        (b" a", "a"),
        (b"a ", "a"),
        (b"--a", "a"),
        (b"a--", "a"),
        (b"-_-", "_"),
        (b"---abc", "abc"),
        (b"_a_", "_a_"),
        (b"a b c", "a-b-c"),
    ];
    for (input, want) in cases {
        let mut out = [b'x'; 21];
        let n = build_hostname(input, &mut out);
        assert_eq!(n, want.len(), "{}", input.escape_ascii());
        assert_text(&out[..n], want);
    }
    // Capacity exactly the default name + NUL.
    let mut six = [0u8; 6];
    assert_eq!(build_hostname(b"", &mut six), 5);
    assert_text(&six[..5], "VdMot");
    assert_eq!(build_hostname(b"abcdefg", &mut six), 5);
    assert_text(&six[..5], "abcde");
    // A separator that would be the last byte is dropped, not left dangling.
    assert_eq!(build_hostname(b"abcd e", &mut six), 4);
    assert_text(&six[..4], "abcd");
    assert_eq!(build_hostname(b"abc de", &mut six), 5);
    assert_text(&six[..5], "abc-d");
    assert_eq!(build_hostname(b"ab cde", &mut six), 5);
    assert_text(&six[..5], "ab-cd");
    // Leading '-' kept from the input are removed by moving exactly the rest.
    assert_eq!(build_hostname(b"--abc", &mut six), 3);
    assert_text(&six[..3], "abc");
    let mut five = *b"yyyy\0";
    assert_eq!(build_hostname(b"ab", &mut five), 0);
    // C++ writes the NUL into five[0]; Rust writes nothing
    assert_eq!(&five, b"yyyy\0");
    // Full-length result.
    let mut big = [b'x'; 21];
    assert_eq!(build_hostname(b"abcdefghijklmnopqrstuvwxyz", &mut big), 20);
    assert_text(&big[..20], "abcdefghijklmnopqrst");
    let mut big = [b'x'; 21];
    assert_eq!(build_hostname(b"--ab", &mut big), 2);
    assert_text(&big[..2], "ab");
    // Rust addition: the station is a C string.
    assert_eq!(build_hostname(b"ab\0cd", &mut big), 2);
    assert_text(&big[..2], "ab");
}

#[test]
fn is_host_name_cases() {
    assert!(is_host_name(b"a", 5));
    assert!(is_host_name(b"a.b-c", 5));
    assert!(!is_host_name(b"a.b-cd", 5));
    assert!(is_host_name(b"aZ09z", 5));
    assert!(is_host_name(b"A-9", 5));
    assert!(!is_host_name(b"", 5));
    // C++ isHostName(nullptr, 5) == false: no Rust form.
    assert!(!is_host_name(b"-a", 5));
    assert!(!is_host_name(b".a", 5));
    assert!(!is_host_name(b"a-", 5));
    assert!(!is_host_name(b"a.", 5));
    for bad in [
        &b"_a"[..],
        b"@a",
        b"[a",
        b"`a",
        b"{a",
        b"/a",
        b":a",
        b" a",
        b"\xc3\xa4a",
    ] {
        assert!(!is_host_name(bad, 9), "{}", bad.escape_ascii());
    }
    for bad in [
        &b"a_b"[..],
        b"a b",
        b"a/b",
        b"a@b",
        b"a[b",
        b"a`b",
        b"a{b",
        b"a:b",
        b"a\xc3\xa4",
    ] {
        assert!(!is_host_name(bad, 9), "{}", bad.escape_ascii());
    }
    // Rust addition: a C string, and the limit counts before the NUL.
    assert!(is_host_name(b"ab\0-", 2));
    assert!(!is_host_name(b"\0a", 5));
}

#[test]
fn parse_uint_cases() {
    // C++ `bool parseUint(..., uint32_t& out)` (out unchanged on failure) is Option<u32>.
    assert_eq!(parse_uint(b"0", 0), Some(0));
    assert_eq!(parse_uint(b"9", 9), Some(9));
    assert_eq!(parse_uint(b"10", 9), None);
    assert_eq!(parse_uint(b"4294967295", 0xFFFF_FFFF), Some(0xFFFF_FFFF));
    assert_eq!(parse_uint(b"4294967296", 0xFFFF_FFFF), None);
    assert_eq!(parse_uint(b"9999999999", 0xFFFF_FFFF), None);
    assert_eq!(parse_uint(b"00000000001", 100), None); // 11 digits
    assert_eq!(parse_uint(b"0000000001", 100), Some(1));
    assert_eq!(parse_uint(b"12345", 99999), Some(12345));
    assert_eq!(parse_uint(&b"123"[..2], 99999), Some(12)); // only len bytes
    assert_eq!(parse_uint(b"/", 100), None);
    assert_eq!(parse_uint(b":", 100), None);
    assert_eq!(parse_uint(b"1a", 100), None);
    assert_eq!(parse_uint(b"a1", 100), None);
    // C++ parseUint(nullptr, 1, ...) == false: no Rust form.
    assert_eq!(parse_uint(&b"1"[..0], 100), None);
    assert_eq!(parse_uint(b"-1", 100), None);
}

#[test]
fn parse_int_cases() {
    assert_eq!(parse_int(b"-5", -5, 5), Some(-5));
    assert_eq!(parse_int(b"-6", -5, 5), None);
    assert_eq!(parse_int(b"5", -5, 5), Some(5));
    assert_eq!(parse_int(b"6", -5, 5), None);
    assert_eq!(parse_int(b"0", 0, 0), Some(0));
    assert_eq!(parse_int(b"-0", 0, 0), Some(0));
    assert_eq!(
        parse_int(b"-2147483648", i32::MIN, i32::MAX),
        Some(i32::MIN)
    );
    assert_eq!(parse_int(b"2147483647", i32::MIN, i32::MAX), Some(i32::MAX));
    assert_eq!(parse_int(b"2147483648", i32::MIN, i32::MAX), None);
    assert_eq!(parse_int(b"-2147483649", i32::MIN, i32::MAX), None);
    assert_eq!(parse_int(b"-123", -1000, 1000), Some(-123));
    assert_eq!(parse_int(b"123", -1000, 1000), Some(123));
    assert_eq!(parse_int(b"", -5, 5), None);
    // C++ parseInt(nullptr, 1, ...) == false: no Rust form.
    assert_eq!(parse_int(b"-", -5, 5), None);
    assert_eq!(parse_int(b"+1", -5, 5), None);
    assert_eq!(parse_int(b"1-", -5, 5), None);
    assert_eq!(parse_int(b"--1", -5, 5), None);
}

#[test]
fn parse_ipv4_and_format_ipv4() {
    assert_eq!(parse_ipv4(b"1.2.3.4"), Some(0x0403_0201));
    assert_eq!(parse_ipv4(b"0.0.0.0"), Some(0));
    assert_eq!(parse_ipv4(b"255.255.255.255"), Some(0xFFFF_FFFF));
    assert_eq!(parse_ipv4(&b"10.0.0.1x"[..8]), Some(0x0100_000A)); // only len bytes
    assert_eq!(parse_ipv4(b"10.0.0.1x"), None);
    assert_eq!(parse_ipv4(b"1.2.3.4."), None);
    assert_eq!(parse_ipv4(b"1.2.3.4.5"), None);
    assert_eq!(parse_ipv4(b"1.2.3"), None);
    assert_eq!(parse_ipv4(b"1.2.3."), None);
    assert_eq!(parse_ipv4(b"1..3.4"), None);
    assert_eq!(parse_ipv4(b".1.2.3"), None);
    assert_eq!(parse_ipv4(b"0001.2.3.4"), None); // 4 digits
    assert_eq!(parse_ipv4(b"001.002.003.004"), Some(0x0403_0201));
    assert_eq!(parse_ipv4(b"256.1.1.1"), None);
    assert_eq!(parse_ipv4(b"1.1.1.256"), None);
    // C++ parseIpv4(nullptr, 7, ...) == false: no Rust form.
    assert_eq!(parse_ipv4(b""), None);
    assert_eq!(parse_ipv4(&b"1.2.3.4"[..6]), None);
    assert_eq!(parse_ipv4(b"192.168.1.2"), Some(0x0201_A8C0));

    let mut buf = [b'x'; 16];
    let n = format_ipv4(0x0403_0201, &mut buf);
    assert_eq!(n, 7);
    assert_text(&buf[..n], "1.2.3.4");
    let n = format_ipv4(0xFFFF_FFFF, &mut buf);
    assert_eq!(n, 15);
    assert_text(&buf[..n], "255.255.255.255");
    assert_eq!(format_ipv4(0xFFFF_FFFF, &mut buf[..16]), 15);
    assert_eq!(format_ipv4(0xFFFF_FFFF, &mut buf[..15]), 0); // C++: and out = ""
    assert_eq!(format_ipv4(0x0403_0201, &mut buf[..8]), 7);
    assert_eq!(format_ipv4(0x0403_0201, &mut buf[..7]), 0);
    buf[0] = b'x';
    assert_eq!(format_ipv4(0x0403_0201, &mut buf[..0]), 0);
    assert_eq!(buf[0], b'x');
}

#[test]
fn one_wire_id_compare_zero_and_crc() {
    let a = OneWireId::default();
    let mut b = OneWireId::default();
    assert!(is_zero(&a));
    assert_eq!(a, b);
    for i in 0..8 {
        b = a;
        b.b[i] = 1;
        assert!(!is_zero(&b), "i {i}");
        assert_ne!(a, b, "i {i}");
    }
    b = a;
    b.b[7] = 0x80;
    assert!(!is_zero(&b));

    let id = parse_one_wire_id(b"28-84-37-94-97-ff-03-23").expect("valid id");
    assert_eq!(crc_valid(&id), ref_crc(&id) == id.b[7]);
    let zero = OneWireId::default();
    assert!(crc_valid(&zero));
    // Maxim application note 27 example ROM: 02 1C B8 01 00 00 00, CRC A2.
    let mut maxim = OneWireId {
        b: [0x02, 0x1C, 0xB8, 0x01, 0x00, 0x00, 0x00, 0xA2],
    };
    assert!(crc_valid(&maxim));
    maxim.b[7] = 0xA3;
    assert!(!crc_valid(&maxim));
}

#[test]
fn one_wire_id_crc_of_fixed_seed_random_ids() {
    // C++: uint32_t seed = 12345; seed = seed * 1103515245u + 12345u; byte = seed >> 16.
    let mut lcg = Lcg::ansi_c(12345);
    let mut valid = 0;
    for round in 0..2000usize {
        let mut r = OneWireId::default();
        for i in 0..7 {
            r.b[i] = (lcg.next_state() >> 16) as u8;
        }
        r.b[7] = ref_crc(&r);
        assert!(crc_valid(&r), "round {round}");
        valid += usize::from(crc_valid(&r));
        r.b[7] ^= 1 << (round % 8);
        assert!(!crc_valid(&r), "round {round}");
        r.b[7] ^= 1 << (round % 8);
        r.b[round % 7] ^= 0x01;
        assert!(!crc_valid(&r), "round {round}");
    }
    assert_eq!(valid, 2000);
}

#[test]
fn parse_one_wire_id_and_format_one_wire_id() {
    let id = parse_one_wire_id(b"00-19-9a-AF-f0-0F-a9-Ff").expect("valid id");
    assert_eq!(id.b, [0x00, 0x19, 0x9A, 0xAF, 0xF0, 0x0F, 0xA9, 0xFF]);
    let id = parse_one_wire_id(b"10-2b-3c-4d-5e-6f-7a-8b").expect("valid id");
    assert_eq!(id.b, [0x10, 0x2B, 0x3C, 0x4D, 0x5E, 0x6F, 0x7A, 0x8B]);
    // C++ "unchanged on failure" is the None of the Option.
    assert_eq!(parse_one_wire_id(&b"10-2b-3c-4d-5e-6f-7a-8b"[..22]), None);
    assert_eq!(parse_one_wire_id(b"10-2b-3c-4d-5e-6f-7a-8b-"), None);
    // C++ parseOneWireId(nullptr, 23, ...) == false: no Rust form.
    let bad: [&[u8; 23]; 12] = [
        b"g0-2b-3c-4d-5e-6f-7a-8b",
        b"1g-2b-3c-4d-5e-6f-7a-8b",
        b"10-2b-3c-4d-5e-6f-7a-8G",
        b"10:2b-3c-4d-5e-6f-7a-8b",
        b"10-2b-3c-4d-5e-6f-7a:8b",
        b"10-2b-3c-4d-5e-6f-7a-/b",
        b"10-2b-3c-4d-5e-6f-7a-:b",
        b"10-2b-3c-4d-5e-6f-7a-@b",
        b"10-2b-3c-4d-5e-6f-7a-`b",
        b"10-2b-3c-4d-5e-6f-7a-8g",
        b"10-2b-3c-4d-5e-6f-7a-8G",
        b" 0-2b-3c-4d-5e-6f-7a-8b",
    ];
    for b in bad {
        assert_eq!(parse_one_wire_id(b), None, "{}", b.escape_ascii());
    }
    // Last byte may be followed by anything outside len.
    assert!(parse_one_wire_id(&b"10-2b-3c-4d-5e-6f-7a-8bX"[..23]).is_some());
    // Every hex digit value in both nibble positions.
    let hex = b"0123456789abcdefABCDEF";
    let val = |c: u8| -> u8 {
        if c <= b'9' {
            c - b'0'
        } else {
            (c | 0x20) - b'a' + 10
        }
    };
    let mut id = OneWireId::default();
    for i in 0..22 {
        let (x, y) = (hex[i] as char, hex[21 - i] as char);
        let t = format!("{x}{y}-00-00-00-00-00-00-{y}{x}");
        id = parse_one_wire_id(t.as_bytes()).expect(&t);
        assert_eq!(id.b[0], val(hex[i]) * 16 + val(hex[21 - i]), "{t}");
        assert_eq!(id.b[7], val(hex[21 - i]) * 16 + val(hex[i]), "{t}");
    }

    let mut out = [b'x'; 24];
    assert_eq!(format_one_wire_id(&id, &mut out[..0]), 0);
    assert_eq!(out[0], b'x');
    assert_eq!(format_one_wire_id(&id, &mut out[..23]), 0); // C++: and out = ""
    let id = parse_one_wire_id(b"00-19-9a-af-f0-0f-a9-ff").expect("valid id");
    let n = format_one_wire_id(&id, &mut out);
    assert_eq!(n, 23);
    assert_text(&out[..n], "00-19-9a-af-f0-0f-a9-ff");
}

fn ha_id(input: &[u8]) -> String {
    let mut out = [b'x'; 64];
    let n = build_ha_id(input, &mut out);
    out[..n].escape_ascii().to_string()
}

#[test]
fn build_ha_id_maps_names_to_ha_ids() {
    assert_eq!(ha_id(b"VdMot"), "VdMot");
    assert_eq!(ha_id(b"Dom 1"), "Dom_1");
    assert_eq!(ha_id(b"\xC5\x81azienka"), "Lazienka"); // Łazienka
    assert_eq!(ha_id(b"K\xC3\xBCche"), "Kuche"); // Küche
    assert_eq!(ha_id(b"Stra\xC3\x9Fe"), "Strasse"); // Straße
    assert_eq!(ha_id(b"\xC3\x86ble"), "AEble"); // Æble
    assert_eq!(ha_id(b"\xC5\xBC\xC3\xB3\xC5\x82w"), "zolw"); // żółw
    assert_eq!(ha_id(b"\xC4\x8Ce\xC5\xA1ky"), "Cesky"); // Česky
    assert_eq!(ha_id(b"Bad.1"), "Bad_1");
    assert_eq!(ha_id(b"a-b_c"), "a-b_c");
    // Every other sequence is one '_': symbols, other scripts, 3 and 4 bytes.
    assert_eq!(ha_id(b"\xE2\x82\xAC"), "_"); // €
    assert_eq!(ha_id(b"\xC3\x97"), "_"); // ×
    assert_eq!(ha_id(b"\xC3\xB7"), "_"); // ÷
    assert_eq!(ha_id(b"\xC2\xBF"), "_"); // ¿ U+00BF, below the table
    assert_eq!(ha_id(b"\xC6\x80"), "_"); // ƀ U+0180, above the table
    assert_eq!(ha_id(b"\xCE\xA9"), "_"); // Ω
    assert_eq!(ha_id(b"\xE4\xB8\xAD"), "_"); // 中 (its first two bytes would read as U+0138)
    assert_eq!(ha_id(b"\xF0\x9F\x98\x80a"), "_a");
    // Invalid UTF-8: one '_' per byte.
    assert_eq!(ha_id(b"\x80"), "_");
    assert_eq!(ha_id(b"\xC5a"), "_a"); // lead byte without continuation
    assert_eq!(ha_id(b"a\xC5"), "a_"); // cut at the end
    assert_eq!(ha_id(b"\xC0\xB0"), "__"); // overlong
    assert_eq!(ha_id(b"\xC2\x80"), "__"); // C1 control
    assert_eq!(ha_id(b"\xED\xA0\x80"), "___"); // surrogate
    assert_eq!(ha_id(b"\xFF\xFE"), "__");
    // Every ASCII byte: letters, digits, '_' and '-' are kept, the rest is '_'.
    for c in 1..0x80u8 {
        let keep = c.is_ascii_alphanumeric() || c == b'_' || c == b'-';
        let want = if keep {
            String::from(c as char)
        } else {
            "_".into()
        };
        assert_eq!(ha_id(&[c]), want, "c {c:#x}");
    }
}

#[test]
fn build_ha_id_maps_u00c0_to_u017f_to_base_letters() {
    const EXPECT: [&str; 192] = [
        "A", "A", "A", "A", "A", "A", "AE", "C", "E", "E", "E", "E", "I", "I", "I", "I", // C0
        "D", "N", "O", "O", "O", "O", "O", "_", "O", "U", "U", "U", "U", "Y", "TH",
        "ss", // D0
        "a", "a", "a", "a", "a", "a", "ae", "c", "e", "e", "e", "e", "i", "i", "i", "i", // E0
        "d", "n", "o", "o", "o", "o", "o", "_", "o", "u", "u", "u", "u", "y", "th", "y", // F0
        "A", "a", "A", "a", "A", "a", "C", "c", "C", "c", "C", "c", "C", "c", "D", "d", // 100
        "D", "d", "E", "e", "E", "e", "E", "e", "E", "e", "E", "e", "G", "g", "G", "g", // 110
        "G", "g", "G", "g", "H", "h", "H", "h", "I", "i", "I", "i", "I", "i", "I", "i", // 120
        "I", "i", "IJ", "ij", "J", "j", "K", "k", "k", "L", "l", "L", "l", "L", "l",
        "L", // 130
        "l", "L", "l", "N", "n", "N", "n", "N", "n", "n", "N", "n", "O", "o", "O", "o", // 140
        "O", "o", "OE", "oe", "R", "r", "R", "r", "R", "r", "S", "s", "S", "s", "S",
        "s", // 150
        "S", "s", "T", "t", "T", "t", "T", "t", "U", "u", "U", "u", "U", "u", "U", "u", // 160
        "U", "u", "U", "u", "W", "w", "Y", "y", "Y", "Z", "z", "Z", "z", "Z", "z", "s", // 170
    ];
    for cp in 0xC0u32..=0x17F {
        let input = [0xC0 | (cp >> 6) as u8, 0x80 | (cp & 0x3F) as u8];
        assert_eq!(ha_id(&input), EXPECT[(cp - 0xC0) as usize], "cp {cp:#x}");
    }
    // Never longer than the input: every 2-byte sequence gives 1 or 2 chars.
    for cp in 0x80u32..0x800 {
        let input = [0xC0 | (cp >> 6) as u8, 0x80 | (cp & 0x3F) as u8];
        let mut out = [0u8; 3];
        let n = build_ha_id(&input, &mut out);
        assert!((1..=2).contains(&n), "cp {cp:#x}");
    }
}

#[test]
fn build_ha_id_capacity_and_input_bounds() {
    let mut out = [b'x'; 8];
    let n = build_ha_id(b"VdMot", &mut out[..6]); // capacity len + 1
    assert_eq!(n, 5);
    assert_text(&out[..n], "VdMot");
    // C++: and out = ""
    assert_eq!(build_ha_id(b"VdMot", &mut out[..5]), 0);
    // A two-letter form fits as a whole or not at all.
    let n = build_ha_id(b"a\xC3\x9F", &mut out[..4]);
    assert_eq!(n, 3);
    assert_text(&out[..n], "ass");
    assert_eq!(build_ha_id(b"a\xC3\x9F", &mut out[..3]), 0);
    // Empty, no room; C++ (nullptr, 5) and a null output have no Rust form.
    assert_eq!(build_ha_id(b"", &mut out), 0);
    out[0] = b'x';
    assert_eq!(build_ha_id(b"ab", &mut out[..0]), 0);
    assert_eq!(out[0], b'x');
    // At most len bytes; a NUL ends the input earlier.
    let n = build_ha_id(&b"abc"[..2], &mut out);
    assert_eq!(n, 2);
    assert_text(&out[..n], "ab");
    let n = build_ha_id(b"a\0b", &mut out);
    assert_eq!(n, 1);
    assert_text(&out[..n], "a");
    // A sequence cut by len is an invalid byte; nothing past len is read.
    let n = build_ha_id(&b"\xC3\xBC"[..1], &mut out);
    assert_eq!(n, 1);
    assert_text(&out[..n], "_");
    let n = build_ha_id(&[b'a', 0xC5], &mut out);
    assert_eq!(n, 2);
    assert_text(&out[..n], "a_");
}

#[test]
fn round_target_percent_cases() {
    let rows: [(f64, Option<u8>); 17] = [
        (43.7, Some(44)),
        (43.5, Some(44)),
        (43.49999, Some(43)),
        (0.49, Some(0)),
        (0.5, Some(1)),
        (99.5, Some(100)),
        (99.49, Some(99)),
        (100.0, Some(100)),
        (0.0, Some(0)),
        (-0.0, Some(0)),
        (100.4, None),
        (100.0000001, None),
        (-0.4, None),
        (-1e-300, None),
        (f64::INFINITY, None),
        (f64::NEG_INFINITY, None),
        (f64::NAN, None),
    ];
    for (v, want) in rows {
        assert_eq!(round_target_percent(v), want, "v {v}");
    }
    // C++ quirk kept (docs/rust/PORT-NOTES.md): floor(v + 0.5) rounds the largest double below
    // 0.5 up, the sum is 1.0 in double precision.
    assert_eq!(round_target_percent(0.499_999_999_999_999_94), Some(1));
}

// ---------------------------------------------------------------- Rust-only helpers

#[test]
fn constants_keep_the_cpp_values() {
    assert_eq!((VALVE_COUNT, TEMP_SLOT_COUNT, VOLT_SLOT_COUNT), (12, 34, 8));
    assert_eq!((ALL_VALVES, NO_VALVE), (255, 0xFE));
    assert_eq!(
        (
            STATION_NAME_MAX,
            ITEM_NAME_MAX,
            UNIT_MAX,
            ONE_WIRE_ID_TEXT_LEN
        ),
        (20, 10, 8, 23)
    );
    assert_eq!(
        (TEMP_UNASSIGNED, TEMP_READ_ERROR, TEMP_POWER_ON),
        (-500, -1270, 850)
    );
    assert_eq!(VAD_FAILED, -1000);
    let t = LocalTime::default();
    assert!(!t.valid);
    assert_eq!((t.year, t.month, t.mday, t.wday, t.epoch), (0, 0, 0, 0, 0));
}

#[test]
fn text_buf_keeps_what_fits_and_reports_the_overflow() {
    let mut buf = [b'#'; 6];
    let mut w = TextBuf::new(&mut buf[..5]); // C++ cap 5: 4 chars
    assert!(w.is_empty());
    assert_eq!(w.len(), 0);
    assert_eq!(w.fit_opt(), Some(0));
    assert!(w.push(b'a'));
    assert!(w.push_bytes(b"bc"));
    assert!(!w.is_empty());
    assert_eq!(w.as_bytes(), b"abc");
    assert_eq!((w.len(), w.fit(), w.fit_opt()), (3, 3, Some(3)));
    assert!(!w.overflowed());
    assert!(!w.push_bytes(b"de")); // "d" fits
    assert!(w.overflowed());
    assert_eq!(w.as_bytes(), b"abcd");
    assert_eq!((w.len(), w.fit(), w.fit_opt()), (4, 0, None));
    assert!(w.push_bytes(b"")); // nothing to cut
    assert!(!w.push(b'e'));
    assert!(w.overflowed()); // stays overflowed
    assert_eq!(w.len(), 4);
    assert_eq!(&buf, b"abcd##"); // the NUL byte is reserved, not written

    let mut empty: [u8; 0] = [];
    let mut e = TextBuf::new(&mut empty);
    assert!(!e.push(b'x'));
    assert_eq!((e.len(), e.fit(), e.fit_opt()), (0, 0, None));
    assert_eq!(e.as_bytes(), b"");
    let mut one = [b'#'; 1];
    let mut o = TextBuf::new(&mut one);
    assert!(o.push_bytes(b""));
    assert!(!o.overflowed());
    assert!(!o.push(b'x'));
    assert_eq!(one, [b'#']);
}

#[test]
fn text_buf_formats_like_snprintf() {
    use core::fmt::Write as _;
    let mut buf = [0u8; 16];
    let mut w = TextBuf::new(&mut buf);
    assert!(write!(w, "{}-{:03}", 7u8, -5i32).is_ok());
    assert_eq!(w.as_bytes(), b"7--05");
    let digits = "0123456789";
    assert!(write!(w, "{:x}{digits}", 0xABCDu32).is_err());
    assert_eq!(w.as_bytes(), b"7--05abcd012345");
    assert_eq!(w.fit(), 0);

    let mut out = [0u8; 8];
    assert_eq!(fmt_fit(&mut out, format_args!("{}.{}", 12, 345)), 6);
    assert_eq!(&out[..6], b"12.345");
    assert_eq!(fmt_fit(&mut out, format_args!("{}", 12_345_678)), 0); // 8 chars need 9
    assert_eq!(fmt_fit(&mut out, format_args!("{}", 1_234_567)), 7);
    assert_eq!(fmt_trunc(&mut out, format_args!("{}", 123_456_789)), 7);
    assert_eq!(&out[..7], b"1234567");
    assert_eq!(fmt_trunc(&mut out, format_args!("{}", 42)), 2);
    assert_eq!(fmt_trunc(&mut out[..0], format_args!("{}", 42)), 0);
    assert_eq!(fmt_fit(&mut out[..0], format_args!("")), 0);
}

#[test]
fn c_str_and_contains_bytes() {
    assert_eq!(c_str(b"abc"), b"abc");
    assert_eq!(c_str(b"ab\0c\0"), b"ab");
    assert_eq!(c_str(b"\0abc"), b"");
    assert_eq!(c_str(b""), b"");
    assert!(contains_bytes(b"2.0.0-revamped", b"revamped"));
    assert!(contains_bytes(b"revamped", b"revamped"));
    assert!(!contains_bytes(b"revampe", b"revamped"));
    assert!(!contains_bytes(b"-Revamped", b"revamped"));
    assert!(contains_bytes(b"a//b", b"//"));
    assert!(!contains_bytes(b"a/b/", b"//"));
    assert!(contains_bytes(b"", b"")); // strstr(s, "") == s
    assert!(contains_bytes(b"x", b""));
    assert!(!contains_bytes(b"", b"x"));
}

/// `snprintf("%.*f", d, v)` of glibc for d = 0..=6 (reference: g++ 12 / glibc 2.36).
const PRINTF_F: [(f64, [&str; 7]); 25] = [
    (
        -0.0,
        [
            "-0",
            "-0.0",
            "-0.00",
            "-0.000",
            "-0.0000",
            "-0.00000",
            "-0.000000",
        ],
    ),
    (
        0.0,
        ["0", "0.0", "0.00", "0.000", "0.0000", "0.00000", "0.000000"],
    ),
    (
        -0.001,
        [
            "-0",
            "-0.0",
            "-0.00",
            "-0.001",
            "-0.0010",
            "-0.00100",
            "-0.001000",
        ],
    ),
    (
        0.5,
        ["0", "0.5", "0.50", "0.500", "0.5000", "0.50000", "0.500000"],
    ),
    (
        1.5,
        ["2", "1.5", "1.50", "1.500", "1.5000", "1.50000", "1.500000"],
    ),
    (
        2.5,
        ["2", "2.5", "2.50", "2.500", "2.5000", "2.50000", "2.500000"],
    ),
    (
        0.125,
        ["0", "0.1", "0.12", "0.125", "0.1250", "0.12500", "0.125000"],
    ),
    (
        0.375,
        ["0", "0.4", "0.38", "0.375", "0.3750", "0.37500", "0.375000"],
    ),
    (
        1.005,
        ["1", "1.0", "1.00", "1.005", "1.0050", "1.00500", "1.005000"],
    ),
    (
        21.456,
        [
            "21",
            "21.5",
            "21.46",
            "21.456",
            "21.4560",
            "21.45600",
            "21.456000",
        ],
    ),
    (
        1e15,
        [
            "1000000000000000",
            "1000000000000000.0",
            "1000000000000000.00",
            "1000000000000000.000",
            "1000000000000000.0000",
            "1000000000000000.00000",
            "1000000000000000.000000",
        ],
    ),
    (
        -1e15,
        [
            "-1000000000000000",
            "-1000000000000000.0",
            "-1000000000000000.00",
            "-1000000000000000.000",
            "-1000000000000000.0000",
            "-1000000000000000.00000",
            "-1000000000000000.000000",
        ],
    ),
    (
        0.1234567,
        ["0", "0.1", "0.12", "0.123", "0.1235", "0.12346", "0.123457"],
    ),
    (
        5e-324,
        ["0", "0.0", "0.00", "0.000", "0.0000", "0.00000", "0.000000"],
    ),
    (
        -5e-324,
        [
            "-0",
            "-0.0",
            "-0.00",
            "-0.000",
            "-0.0000",
            "-0.00000",
            "-0.000000",
        ],
    ),
    (
        1e20,
        [
            "100000000000000000000",
            "100000000000000000000.0",
            "100000000000000000000.00",
            "100000000000000000000.000",
            "100000000000000000000.0000",
            "100000000000000000000.00000",
            "100000000000000000000.000000",
        ],
    ),
    (
        4_503_599_627_370_496.0,
        [
            "4503599627370496",
            "4503599627370496.0",
            "4503599627370496.00",
            "4503599627370496.000",
            "4503599627370496.0000",
            "4503599627370496.00000",
            "4503599627370496.000000",
        ],
    ),
    (
        9_007_199_254_740_992.0,
        [
            "9007199254740992",
            "9007199254740992.0",
            "9007199254740992.00",
            "9007199254740992.000",
            "9007199254740992.0000",
            "9007199254740992.00000",
            "9007199254740992.000000",
        ],
    ),
    (
        0.045,
        ["0", "0.0", "0.04", "0.045", "0.0450", "0.04500", "0.045000"],
    ),
    (
        2.675,
        ["3", "2.7", "2.67", "2.675", "2.6750", "2.67500", "2.675000"],
    ),
    (
        1e-7,
        ["0", "0.0", "0.00", "0.000", "0.0000", "0.00000", "0.000000"],
    ),
    (
        0.0000005,
        ["0", "0.0", "0.00", "0.000", "0.0000", "0.00000", "0.000000"],
    ),
    (
        0.0000015,
        ["0", "0.0", "0.00", "0.000", "0.0000", "0.00000", "0.000002"],
    ),
    (
        0.0000025,
        ["0", "0.0", "0.00", "0.000", "0.0000", "0.00000", "0.000003"],
    ),
    (
        123_456.789_012_5,
        [
            "123457",
            "123456.8",
            "123456.79",
            "123456.789",
            "123456.7890",
            "123456.78901",
            "123456.789012",
        ],
    ),
];

fn f(v: f64, d: u8) -> String {
    let mut out = [0u8; 64];
    let n = format_f64_fixed(v, d, &mut out);
    String::from_utf8(out[..n].to_vec()).expect("ASCII")
}

#[test]
fn format_f64_fixed_matches_glibc_printf() {
    for (v, texts) in PRINTF_F {
        for (d, want) in texts.iter().enumerate() {
            assert_eq!(f(v, d as u8), *want, "v {v:e} d {d}");
        }
    }
    // The whole f32 range and the 2^128 limit (glibc: "%.6f" of FLT_MAX, "%.0f" of
    // (2^53 - 1) * 2^75).
    assert_eq!(
        f(f64::from(f32::MAX), 6),
        "340282346638528859811704183484516925440.000000"
    );
    let below = f64::from_bits((1150u64 << 52) | 0x000F_FFFF_FFFF_FFFF); // (2^53 - 1) * 2^75
    assert_eq!(f(below, 0), "340282366920938425684442744474606501888");
    assert_eq!(f(-below, 1), "-340282366920938425684442744474606501888.0");
    assert_eq!(f(f64::from_bits(1151u64 << 52), 0), ""); // 2^128
    assert_eq!(f(1e300, 0), "");
    // 18 decimals (glibc "%.18f" of 0.1 and 1/3), 19 refused.
    assert_eq!(f(0.1, 18), "0.100000000000000006");
    assert_eq!(f(1.0 / 3.0, 18), "0.333333333333333315");
    assert_eq!(f(0.1, 19), "");
    // Not finite.
    assert_eq!(f(f64::NAN, 2), "");
    assert_eq!(f(f64::INFINITY, 0), "");
    assert_eq!(f(f64::NEG_INFINITY, 0), "");
    // Ties at the exact binary value: half to even, in the integer and the last decimal.
    assert_eq!(f(3.5, 0), "4");
    assert_eq!(f(4.5, 0), "4");
    assert_eq!(f(-2.5, 0), "-2");
    assert_eq!(f(0.625, 2), "0.62");
    assert_eq!(f(0.875, 2), "0.88");
    assert_eq!(f(1.0, 0), "1");
    assert_eq!(f(2f64.powi(-60), 18), "0.000000000000000001");
    // Integers from 2^52 up (no fraction bits).
    assert_eq!(f(2f64.powi(53) + 2.0, 1), "9007199254740994.0");
    assert_eq!(f(-(2f64.powi(60)), 0), "-1152921504606846976");
    // The text must fit, with the C++ NUL.
    let mut out = [0u8; 5];
    assert_eq!(format_f64_fixed(1.25, 2, &mut out), 4);
    assert_eq!(&out[..4], b"1.25");
    assert_eq!(format_f64_fixed(-1.25, 2, &mut out), 0);
    assert_eq!(format_f64_fixed(1.25, 2, &mut out[..4]), 0);
}

#[test]
fn format_f64_fixed_random_values_read_back() {
    // Every output parses back to the nearest of the value at that precision.
    let mut rng = crate::test_support::Rng::new(4711);
    let mut tested = Vec::new();
    for _ in 0..2000 {
        let bits = (u64::from(rng.next_u32()) << 32) | u64::from(rng.next_u32());
        let v = f64::from_bits(bits);
        if !v.is_finite() || v.abs() >= 1e15 {
            continue;
        }
        let d = (rng.below(7)) as u8;
        let text = f(v, d);
        let back: f64 = text.parse().expect("number");
        // half a unit of the last decimal, plus the rounding of the text back to a double
        let bound = 0.5 * 10f64.powi(-i32::from(d)) * (1.0 + 1e-9) + v.abs() * f64::EPSILON;
        assert!((back - v).abs() <= bound, "{v:e} d {d} -> {text}");
        tested.push(text);
    }
    assert!(tested.len() > 500);
}
