//! Port of test/native/test_arg_parser.cpp. The C++ checks that `out` keeps its value on
//! failure are the `None` results (the Rust parsers have no out parameter).

use super::*;

#[test]
fn parse_u32_valid_values_and_range_boundaries() {
    assert_eq!(parse_u32(b"0", 0, 10), Some(0));
    assert_eq!(parse_u32(b"10", 0, 10), Some(10));
    assert_eq!(parse_u32(b"007", 0, 10), Some(7));
    assert_eq!(parse_u32(b"5", 5, 5), Some(5));
    assert_eq!(parse_u32(b"4294967295", 0, u32::MAX), Some(u32::MAX));
    assert_eq!(parse_u32(b"65535", 0, 65535), Some(65535));
}

#[test]
fn parse_u32_out_of_range_leaves_the_output_untouched() {
    assert_eq!(parse_u32(b"11", 0, 10), None);
    assert_eq!(parse_u32(b"4", 5, 10), None);
    assert_eq!(parse_u32(b"65536", 0, 65535), None);
    assert_eq!(parse_u32(b"4294967296", 0, u32::MAX), None);
    assert_eq!(parse_u32(b"99999999999999999999", 0, u32::MAX), None);
    assert_eq!(parse_u32(b"1", 10, 0), None); // inverted range
}

#[test]
fn parse_u32_overflow_check_per_digit_against_small_limits() {
    // limit 5: "7" must fail even though 7 > limit - digit would underflow.
    assert_eq!(parse_u32(b"7", 0, 5), None);
    assert_eq!(parse_u32(b"5", 0, 5), Some(5));
    assert_eq!(parse_u32(b"6", 0, 5), None);
    assert_eq!(parse_u32(b"10", 0, 9), None);
    assert_eq!(parse_u32(b"255", 0, 255), Some(255));
    assert_eq!(parse_u32(b"256", 0, 255), None);
    assert_eq!(parse_u32(b"260", 0, 255), None);
    assert_eq!(parse_u32(b"1000", 0, 255), None);
    assert_eq!(parse_u32(b"0000000000000255", 0, 255), Some(255));
}

#[test]
fn parse_u32_malformed_input() {
    assert_eq!(parse_u32(b"", 0, 10), None);
    // C++ parseU32(nullptr, ...): a Rust slice is never null
    assert_eq!(parse_u32(b"-1", 0, u32::MAX), None);
    assert_eq!(parse_u32(b"+1", 0, 10), None);
    assert_eq!(parse_u32(b" 1", 0, 10), None);
    assert_eq!(parse_u32(b"1 ", 0, 10), None);
    assert_eq!(parse_u32(b"1a", 0, 100), None);
    assert_eq!(parse_u32(b"a1", 0, 100), None);
    assert_eq!(parse_u32(b"0x10", 0, 100), None);
    assert_eq!(parse_u32(b"1.5", 0, 100), None);
    assert_eq!(parse_u32(b"/", 0, 100), None);
    assert_eq!(parse_u32(b":", 0, 100), None);
}

#[test]
fn parse_i32_valid_values_and_boundaries() {
    assert_eq!(parse_i32(b"0", -5, 5), Some(0));
    assert_eq!(parse_i32(b"-0", -5, 5), Some(0));
    assert_eq!(parse_i32(b"+5", -5, 5), Some(5));
    assert_eq!(parse_i32(b"-5", -5, 5), Some(-5));
    assert_eq!(parse_i32(b"2147483647", i32::MIN, i32::MAX), Some(i32::MAX));
    assert_eq!(
        parse_i32(b"-2147483648", i32::MIN, i32::MAX),
        Some(i32::MIN)
    );
    assert_eq!(
        parse_i32(b"-2147483647", i32::MIN, i32::MAX),
        Some(-2147483647)
    );
}

#[test]
fn parse_i32_out_of_range_and_malformed() {
    assert_eq!(parse_i32(b"6", -5, 5), None);
    assert_eq!(parse_i32(b"-6", -5, 5), None);
    assert_eq!(parse_i32(b"2147483648", i32::MIN, i32::MAX), None);
    assert_eq!(parse_i32(b"-2147483649", i32::MIN, i32::MAX), None);
    assert_eq!(parse_i32(b"", -5, 5), None);
    assert_eq!(parse_i32(b"-", -5, 5), None);
    assert_eq!(parse_i32(b"+", -5, 5), None);
    assert_eq!(parse_i32(b"--1", -5, 5), None);
    assert_eq!(parse_i32(b"+-1", -5, 5), None);
    assert_eq!(parse_i32(b"1-", -5, 5), None);
    assert_eq!(parse_i32(b" 1", -5, 5), None);
    // C++ parseI32(nullptr, ...): a Rust slice is never null
    assert_eq!(parse_i32(b"1", 5, -5), None); // inverted range
}

#[test]
fn parse_one_wire_address_valid_addresses_in_both_cases() {
    let a = parse_one_wire_address(b"28-84-37-94-97-ff-03-23").expect("valid");
    assert_eq!(a, [0x28, 0x84, 0x37, 0x94, 0x97, 0xFF, 0x03, 0x23]);

    let a = parse_one_wire_address(b"0A-bC-De-F0-9a-00-10-Ff").expect("valid");
    assert_eq!(a, [0x0A, 0xBC, 0xDE, 0xF0, 0x9A, 0x00, 0x10, 0xFF]);

    let a = parse_one_wire_address(b"00-00-00-00-00-00-00-00").expect("valid");
    assert!(is_zero_address(&a));
}

#[test]
fn parse_one_wire_address_malformed_input_leaves_the_output_untouched() {
    let bad: [&[u8]; 15] = [
        b"",
        b"28",
        b"28-84-37-94-97-ff-03",     // 7 bytes
        b"28-84-37-94-97-ff-03-2",   // short last byte
        b"28-84-37-94-97-ff-03-23-", // trailing separator
        b"28-84-37-94-97-ff-03-234", // too long
        b"28:84:37:94:97:ff:03:23",  // wrong separator
        b"28-84-37-94-97-ff-03 23",
        b"2g-84-37-94-97-ff-03-23", // non-hex
        b"g2-84-37-94-97-ff-03-23",
        b"28-84-37-94-97-ff-03-2G",
        b"28--4-37-94-97-ff-03-23",
        b"-28-84-37-94-97-ff-03-2",
        b" 28-84-37-94-97-ff-03-2",
        b"28-84-37-94-97-ff-0323",
    ];
    for s in bad {
        assert_eq!(parse_one_wire_address(s), None, "{:?}", s);
    }
    // C++ parseOneWireAddress(nullptr, a): a Rust slice is never null
}

#[test]
fn parse_one_wire_address_hex_digit_boundaries() {
    let a = parse_one_wire_address(b"09-0a-0f-90-a0-f0-AF-FA").expect("valid");
    assert_eq!(a, [0x09, 0x0A, 0x0F, 0x90, 0xA0, 0xF0, 0xAF, 0xFA]);
    // Characters adjacent to the valid ranges.
    let bad: [&[u8]; 12] = [
        b"/0-00-00-00-00-00-00-00",
        b":0-00-00-00-00-00-00-00",
        b"`0-00-00-00-00-00-00-00",
        b"g0-00-00-00-00-00-00-00",
        b"@0-00-00-00-00-00-00-00",
        b"G0-00-00-00-00-00-00-00",
        b"0/-00-00-00-00-00-00-00",
        b"0:-00-00-00-00-00-00-00",
        b"0`-00-00-00-00-00-00-00",
        b"0g-00-00-00-00-00-00-00",
        b"0@-00-00-00-00-00-00-00",
        b"0G-00-00-00-00-00-00-00",
    ];
    for s in bad {
        assert_eq!(parse_one_wire_address(s), None, "{:?}", s);
    }
}

#[test]
fn parse_one_wire_address_ends_at_a_nul_like_the_c_string() {
    // the C++ string ends at its NUL, so the text behind it is not seen
    assert_eq!(
        parse_one_wire_address(b"28-84-37-94-97-ff-03-23\0junk"),
        Some([0x28, 0x84, 0x37, 0x94, 0x97, 0xFF, 0x03, 0x23])
    );
    assert_eq!(parse_one_wire_address(b"28-84-37\0-94-97-ff-03-23"), None);
}

#[test]
fn is_zero_address_test() {
    let zero = [0u8; 8];
    assert!(is_zero_address(&zero));
    for i in 0..8 {
        let mut a = [0u8; 8];
        a[i] = 1;
        assert!(!is_zero_address(&a));
    }
}
