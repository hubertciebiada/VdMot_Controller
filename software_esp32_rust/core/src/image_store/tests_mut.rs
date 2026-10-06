//! Port of test/native/test_image_store__mut.cpp: the ".bin" suffix check reads exactly the
//! last four characters of the given length, and a one-byte output gets no name.

use super::*;
use std::string::String;

fn norm_n(input: &[u8]) -> String {
    let mut out = [b'x'; 64];
    match normalize_image_name(input, &mut out) {
        Some(n) => String::from_utf8(out[..n].to_vec()).expect("ASCII"),
        None => "!".into(),
    }
}

#[test]
fn the_suffix_is_the_last_four_characters_of_the_given_length() {
    // A name not terminated after its length: the byte past it is not part of the suffix.
    let unterminated = b"x.binz";
    assert_eq!(norm_n(&unterminated[..5]), "x");
    // Only the full ".bin" is a suffix, ".bi" followed by something else is not.
    assert_eq!(norm_n(b"x.bix"), "x.bix");
    // Names shorter than the suffix never look before their first character.
    let before = b".bin";
    assert_eq!(norm_n(&before[1..4]), "bin");
    assert_eq!(norm_n(&before[3..4]), "n");
}

#[test]
fn a_one_byte_output_is_cleared() {
    // C++: the output is "" afterwards; Rust refuses and writes nothing.
    let mut out = [b'z'; 1];
    assert_eq!(normalize_image_name(b"a", &mut out), None);
    assert_eq!(out, [b'z']);
}
