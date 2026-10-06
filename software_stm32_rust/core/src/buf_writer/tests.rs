//! Port of test/native/test_buf_writer.cpp.

use super::*;
use crate::test_support::text;

#[test]
fn starts_empty_and_terminated() {
    let mut buf = [b'x'; 4];
    let w = BufWriter::new(&mut buf[..]);
    assert_eq!(w.length(), 0);
    assert_eq!(w.capacity(), 4);
    assert!(w.ok());
    // C++ also checks buf[0] == '\0': the Rust writer does not write the NUL, its text is
    // as_bytes()
    assert!(w.as_bytes().is_empty());
}

#[test]
fn strings_and_characters_up_to_capacity() {
    let mut w = StaticBufWriter::<6>::default(); // 5 characters
    assert!(w.append(b"ab"));
    assert!(w.append_char(b'c'));
    assert!(w.append(b""));
    assert!(w.append(b"de"));
    assert_eq!(text(w.as_bytes()), "abcde");
    assert_eq!(w.length(), 5);
    assert!(w.ok());
    assert!(w.append(b"")); // empty always fits
    assert!(!w.append_char(b'f'));
    assert!(!w.ok());
    assert_eq!(text(w.as_bytes()), "abcde");
}

#[test]
fn all_or_nothing_appends() {
    let mut w = StaticBufWriter::<6>::default();
    assert!(w.append(b"abc"));
    assert!(!w.append(b"def")); // needs 3, only 2 free
    assert_eq!(text(w.as_bytes()), "abc");
    assert!(!w.ok());
    // Later smaller appends still work but ok() stays false.
    assert!(w.append(b"de"));
    assert_eq!(text(w.as_bytes()), "abcde");
    assert!(!w.ok());
    w.clear();
    assert!(w.ok());
    assert_eq!(w.length(), 0);
    assert!(w.as_bytes().is_empty());
}

#[test]
fn null_string_and_degenerate_buffers() {
    // C++ append(nullptr) fails and clears ok(): a Rust slice is never null, the nearest case
    // is a piece that does not fit, which must not write anything either
    let mut w = StaticBufWriter::<8>::default();
    assert!(!w.append(b"too long!"));
    assert!(!w.ok());
    assert_eq!(w.length(), 0);

    // the C++ null buffer is storage of length 0
    let mut none = BufWriter::new(&mut [0u8; 0][..]);
    assert_eq!(none.capacity(), 0);
    assert!(!none.append_char(b'a'));
    assert!(!none.append(b""));
    assert!(none.as_bytes().is_empty());
    none.clear();
    none.truncate(0);
    assert_eq!(none.length(), 0);

    let mut one = [b'x'; 1];
    {
        let mut tiny = BufWriter::new(&mut one[..]);
        assert!(tiny.append(b""));
        assert!(!tiny.append_char(b'a'));
        assert!(tiny.as_bytes().is_empty());
    }
    // C++: one[0] == '\0' (the NUL); the Rust writer never writes it
    assert_eq!(one[0], b'x');

    let mut zero = [b'x'; 1];
    {
        let mut empty = BufWriter::new(&mut zero[..0]);
        assert!(!empty.append(b""));
    }
    assert_eq!(zero[0], b'x'); // capacity 0: buffer is never touched
}

#[test]
fn unsigned_numbers() {
    let mut w = StaticBufWriter::<64>::default();
    assert!(w.append_unsigned(0));
    assert!(w.append_char(b' '));
    assert!(w.append_unsigned(9));
    assert!(w.append_char(b' '));
    assert!(w.append_unsigned(10));
    assert!(w.append_char(b' '));
    assert!(w.append_unsigned(65535));
    assert!(w.append_char(b' '));
    assert!(w.append_unsigned(u32::MAX));
    assert_eq!(text(w.as_bytes()), "0 9 10 65535 4294967295");
}

#[test]
fn signed_numbers() {
    let mut w = StaticBufWriter::<64>::default();
    assert!(w.append_signed(0));
    assert!(w.append_char(b' '));
    assert!(w.append_signed(-1));
    assert!(w.append_char(b' '));
    assert!(w.append_signed(-1270));
    assert!(w.append_char(b' '));
    assert!(w.append_signed(i32::MAX));
    assert!(w.append_char(b' '));
    assert!(w.append_signed(i32::MIN));
    assert!(w.append_char(b' '));
    assert!(w.append_signed(10));
    assert_eq!(text(w.as_bytes()), "0 -1 -1270 2147483647 -2147483648 10");
}

#[test]
fn numbers_that_do_not_fit_are_not_written() {
    let mut w = StaticBufWriter::<4>::default(); // 3 characters
    assert!(w.append_unsigned(999));
    assert!(!w.append_unsigned(1));
    w.clear();
    assert!(!w.append_unsigned(1000));
    assert_eq!(w.length(), 0);
    assert!(w.append_signed(-99));
    w.clear();
    assert!(!w.append_signed(-100));
    assert_eq!(w.length(), 0);
    assert!(!w.append_signed(i32::MIN));
    assert!(w.as_bytes().is_empty());
}

#[test]
fn hex_bytes() {
    let mut w = StaticBufWriter::<16>::default();
    assert!(w.append_hex2(0x00));
    assert!(w.append_hex2(0x0f));
    assert!(w.append_hex2(0xa0));
    assert!(w.append_hex2(0xff));
    assert!(w.append_hex2(0x19));
    assert_eq!(text(w.as_bytes()), "000fa0ff19");
    let mut small = StaticBufWriter::<2>::default();
    assert!(!small.append_hex2(0x12));
    assert_eq!(small.length(), 0);
}

#[test]
fn one_wire_addresses_match_the_v1_wire_format() {
    let addr: [u8; 8] = [0x28, 0x84, 0x37, 0x94, 0x97, 0xFF, 0x03, 0x23];
    let mut w = StaticBufWriter::<24>::default(); // exactly 23 characters + NUL
    assert!(w.append_one_wire_address(&addr));
    assert_eq!(text(w.as_bytes()), "28-84-37-94-97-ff-03-23");
    assert_eq!(w.length(), 23);

    let zero = [0u8; 8];
    let mut too_small = StaticBufWriter::<23>::default();
    assert!(!too_small.append_one_wire_address(&zero));
    assert_eq!(too_small.length(), 0);

    let mut z = StaticBufWriter::<32>::default();
    assert!(z.append_one_wire_address(&zero));
    assert_eq!(text(z.as_bytes()), "00-00-00-00-00-00-00-00");
}

#[test]
fn truncate() {
    let mut w = StaticBufWriter::<16>::default();
    assert!(w.append(b"hello"));
    w.truncate(10); // longer than content: no-op
    assert_eq!(text(w.as_bytes()), "hello");
    w.truncate(5); // equal to the content: no-op
    assert_eq!(text(w.as_bytes()), "hello");
    w.truncate(2);
    assert_eq!(text(w.as_bytes()), "he");
    assert_eq!(w.length(), 2);
    assert!(!w.append(b"0123456789abcdef"));
    w.truncate(0);
    assert!(w.as_bytes().is_empty());
    assert!(!w.ok()); // truncate does not reset the error flag
}
