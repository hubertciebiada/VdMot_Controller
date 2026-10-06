//! Port of test/native/test_image_store.cpp: image name rules (every character class, lengths,
//! the ".bin" suffix, leading '.') and image paths (fit and overflow).

use super::*;
use crate::test_support::assert_text;
use std::string::String;
use std::vec::Vec;

/// The bare name, or "!" when refused (the C++ helper appends the cleared output, always "").
fn norm_cap(input: &[u8], cap: usize) -> String {
    let mut out = [b'x'; 64];
    match normalize_image_name(input, &mut out[..cap]) {
        Some(n) => String::from_utf8(out[..n].to_vec()).expect("ASCII"),
        None => "!".into(),
    }
}

fn norm(input: &[u8]) -> String {
    norm_cap(input, 40)
}

#[test]
fn name_rules() {
    assert_eq!(norm(b"fw-1_2.bin"), "fw-1_2");
    assert_eq!(norm(b"fw-1_2"), "fw-1_2");
    assert_eq!(norm(b"AZaz09._-"), "AZaz09._-");
    assert_eq!(norm(b"a.bin.bin"), "a.bin");
    assert_eq!(norm(b"x.BIN"), "x.BIN");
    assert_eq!(norm(b"a.bi"), "a.bi");
    let a31 = "a".repeat(31);
    assert_eq!(norm(a31.as_bytes()), a31);
    assert_eq!(norm((a31.clone() + ".bin").as_bytes()), a31);
    assert_eq!(norm("a".repeat(32).as_bytes()), "!");
    assert_eq!(norm(b""), "!");
    assert_eq!(norm(b".bin"), "!");
    assert_eq!(norm(b"bin"), "bin");
    assert_eq!(norm(b".x"), "!");
    for c in b" /+#\"\\:@~,!\x7f" {
        assert_eq!(norm(&[b'a', *c]), "!", "{}", c.escape_ascii());
    }
    for c in b"@[`{" {
        // just outside the letter and digit ranges
        assert_eq!(norm(&[b'a', *c]), "!", "{}", c.escape_ascii());
    }
    assert_eq!(norm(b"/0"), "!");
    assert_eq!(norm(b":9"), "!");
    // The output needs room for the name and the NUL.
    assert_eq!(norm_cap(b"abc", 4), "abc");
    assert_eq!(norm_cap(b"abc", 3), "!");
    // C++ normalizeImageName(nullptr, 3, out, 4) and a null output: no Rust form.
    let mut out = *b"zz\0\0";
    assert_eq!(normalize_image_name(b"a", &mut out[..0]), None);
    assert_eq!(&out, b"zz\0\0"); // Rust writes nothing on failure
}

#[test]
fn paths() {
    let mut out = [0u8; 40];
    let n = image_path(b"fw", false, &mut out).expect("fits");
    assert_text(&out[..n], "/stm/fw.bin");
    let n = image_path(b"fw", true, &mut out).expect("fits");
    assert_text(&out[..n], "/stm/fw.bin.part");
    assert_eq!(image_path(b"fw", true, &mut out[..17]), Some(16));
    assert_eq!(image_path(b"fw", true, &mut out[..16]), None);
    assert_eq!(image_path(b"fw", false, &mut out[..12]), Some(11));
    assert_eq!(image_path(b"fw", false, &mut out[..11]), None);
    // Rust addition: the name is a C string.
    let n = image_path(b"fw\0junk", false, &mut out).expect("fits");
    assert_text(&out[..n], "/stm/fw.bin");
    let collected: Vec<u8> = out[..n].to_vec();
    assert_eq!(collected.len(), 11);
}
