// The two Print cases of test/native/glue/test_fakes.cpp (the C++ fake stood for the core's
// Print.cpp, print.rs is its port) and the float forms against the outputs of the core's
// printFloat<T> compiled with g++ (scratch reference program, not committed).

use super::*;
use crate::test_support::io_fakes::FakeSerial;

fn printed(f: impl FnOnce(&mut FakeSerial)) -> String {
    let mut p = FakeSerial::new();
    f(&mut p);
    p.take_tx()
}

fn printed_bytes(f: impl FnOnce(&mut FakeSerial)) -> Vec<u8> {
    let mut p = FakeSerial::new();
    f(&mut p);
    p.tx
}

#[test]
fn fakes_print_numbers_are_formatted_like_on_the_32_bit_target() {
    assert_eq!(printed(|p| p.print_signed(-1, HEX)), "FFFFFFFF");
    assert_eq!(printed(|p| p.print_signed(-5, DEC)), "-5");
    assert_eq!(printed(|p| p.print_unsigned(65, DEC)), "65");
    assert_eq!(printed(|p| p.print_char(b'A')), "A");
    assert_eq!(printed(|p| p.println()), "\r\n");
    assert_eq!(printed(|p| p.print_signed(65, 0)), "A");
    assert_eq!(
        printed(|p| p.print_unsigned(3_000_000_000, DEC)),
        "3000000000"
    );
    assert_eq!(printed(|p| p.print_f64(1.5, 2)), "1.50");
    assert_eq!(printed(|p| p.print_signed(i32::MIN, DEC)), "-2147483648");
    assert_eq!(printed(|p| p.print_unsigned(0x423, DEC)), "1059");
    assert_eq!(printed(|p| p.print_signed(255, HEX)), "FF");
    assert_eq!(printed(|p| p.print_signed(5, 1)), "5");
    assert_eq!(printed(|p| p.print_f32(1.999, 2)), "2.00");
    assert_eq!(printed(|p| p.print_f64(-0.25, 1)), "-0.3");
    assert_eq!(printed(|p| p.println_text(b"gvers")), "gvers\r\n");
}

#[test]
fn fakes_println_of_every_argument_kind_ends_with_cr_lf() {
    assert_eq!(printed(|p| p.println_signed(7, DEC)), "7\r\n");
    assert_eq!(printed(|p| p.println_unsigned(7, DEC)), "7\r\n");
    assert_eq!(
        printed(|p| p.println_unsigned(u32::from(7u8), DEC)),
        "7\r\n"
    );
    assert_eq!(printed(|p| p.println_char(b'x')), "x\r\n");
    // println(String(12u) + ":" + String(3u)): no String without a heap, the text is the same
    assert_eq!(printed(|p| p.println_text(b"12:3")), "12:3\r\n");
}

#[test]
fn print_number_bases_and_digits() {
    assert_eq!(printed(|p| p.print_unsigned(0, DEC)), "0");
    assert_eq!(printed(|p| p.print_unsigned(0, HEX)), "0");
    assert_eq!(printed(|p| p.print_unsigned(u32::MAX, DEC)), "4294967295");
    assert_eq!(printed(|p| p.print_unsigned(0xABCDEF09, HEX)), "ABCDEF09");
    assert_eq!(printed(|p| p.print_unsigned(10, HEX)), "A");
    assert_eq!(printed(|p| p.print_unsigned(9, HEX)), "9");
    assert_eq!(printed(|p| p.print_unsigned(5, 2)), "101");
    assert_eq!(printed(|p| p.print_unsigned(u32::MAX, 2)), "1".repeat(32));
    assert_eq!(printed(|p| p.print_unsigned(64, 8)), "100");
    assert_eq!(printed(|p| p.print_unsigned(35, 36)), "Z");
    // base 1 prints decimal, base 0 the low byte
    assert_eq!(printed(|p| p.print_unsigned(12, 1)), "12");
    assert_eq!(printed(|p| p.print_unsigned(0x141, 0)), "A");
    // a digit beyond Z wraps in the char like the C++
    assert_eq!(
        printed(|p| p.print_unsigned(250, 251)).as_bytes(),
        [250u8.wrapping_add(55)]
    );
    assert_eq!(printed_bytes(|p| p.print_signed(-1, 0)), [0xFF]);
    assert_eq!(printed(|p| p.print_signed(0, DEC)), "0");
    assert_eq!(printed(|p| p.print_signed(i32::MAX, DEC)), "2147483647");
    assert_eq!(printed(|p| p.print_signed(-16, HEX)), "FFFFFFF0");
    assert_eq!(
        printed(|p| p.print_signed(-2, 2)),
        format!("{}0", "1".repeat(31))
    );
    assert_eq!(printed(|p| p.print_signed(-7, 1)), "4294967289");
    assert_eq!(printed(|p| p.println_signed(-7, DEC)), "-7\r\n");
    assert_eq!(printed(|p| p.println_unsigned(255, HEX)), "FF\r\n");
    assert_eq!(printed(|p| p.print(b"a\0b")).as_bytes(), b"a\0b");
}

#[test]
fn print_f64_matches_the_core() {
    for (v, digits, want) in [
        (1.5, 2, "1.50"),
        (-0.25, 1, "-0.3"),
        (0.1, 10, "0.1000000000"),
        (2.675, 2, "2.67"),
        (1e-7, 8, "0.00000010"),
        (123456.789, 3, "123456.789"),
        (4294967040.0, 0, "4294967040"),
        (4294967041.0, 0, "ovf"),
        (-4294967040.0, 0, "-4294967040"),
        (-4294967041.0, 0, "ovf"),
        (0.0, 2, "0.00"),
        (-0.0, 2, "0.00"),
        (1.7, 10, "1.7000000000"),
        (0.005, 2, "0.01"),
        (9.995, 2, "10.00"),
        (99.5, 0, "100"),
        (f64::NAN, 2, "nan"),
        (f64::NEG_INFINITY, 2, "inf"),
        (f64::INFINITY, 0, "inf"),
    ] {
        assert_eq!(printed(|p| p.print_f64(v, digits)), want, "{v} {digits}");
    }
}

#[test]
fn print_f32_rounds_in_float_like_the_core_template() {
    for (v, digits, want) in [
        (1.999f32, 2, "2.00"),
        (0.1, 10, "0.1000000000"),
        (1.7, 10, "1.7000000476"),
        (2.675, 2, "2.68"),
        (16777217.0, 1, "16777216.0"),
        (123_456.79, 3, "123456.789"),
        (4294967040.0, 0, "4294967040"),
        (1e10, 0, "ovf"),
        (-1e10, 0, "ovf"),
        (-1.5, 1, "-1.5"),
        (0.3, 6, "0.300000"),
        (3.4, 10, "3.4000000953"),
        (25.0625, 4, "25.0625"),
        (f32::NAN, 2, "nan"),
        (f32::INFINITY, 2, "inf"),
        (f32::NEG_INFINITY, 1, "inf"),
    ] {
        assert_eq!(printed(|p| p.print_f32(v, digits)), want, "{v} {digits}");
    }
}
