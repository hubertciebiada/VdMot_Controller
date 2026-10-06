// The ID block (docs/rust/GLUE-DESIGN-STM.md §6.2, §7.3): NUL layout, at most 64 bytes, the
// version as the first NUL-terminated run, exactly one board marker. The ESP's own scan of
// the built images runs in the image check (tools/rust/stm/esp_validate.cpp).
#![allow(
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::unwrap_used
)]

use super::*;

fn block(version: &[u8], tag: &[u8]) -> ([u8; ID_BLOCK_MAX], IdLayout) {
    let mut out = [0xEE; ID_BLOCK_MAX];
    let layout = write(version, tag, &mut out).unwrap();
    (out, layout)
}

#[test]
fn the_block_of_the_design() {
    let (out, l) = block(b"2.2.0-revamped", b"C2");
    let expected = b"\x002.2.0-revamped\x00VDM-HW:C2\x00DEADBEEF\x00BEEFIT\x00";
    assert_eq!(l.len, 42);
    assert_eq!(&out[..l.len], expected);
    assert!(out[l.len..].iter().all(|&b| b == 0));
    assert_eq!(
        l,
        IdLayout {
            version: 1,
            version_len: 14,
            tag: 23,
            tag_len: 2,
            pattern: 26,
            reply: 35,
            len: 42
        }
    );
    assert_eq!(
        &out[l.version..l.version + l.version_len],
        b"2.2.0-revamped"
    );
    assert_eq!(&out[l.tag..l.tag + l.tag_len], b"C2");
    assert_eq!(&out[l.pattern..l.pattern + 8], b"DEADBEEF");
    assert_eq!(&out[l.reply..l.reply + 6], b"BEEFIT");
}

#[test]
fn every_field_is_nul_delimited() {
    let (out, l) = block(b"2.2.0-revamped-dev", b"C12");
    assert_eq!(out[0], 0);
    assert_eq!(out[l.version + l.version_len], 0);
    assert_eq!(&out[l.tag - 7..l.tag], b"VDM-HW:");
    assert_eq!(out[l.tag + l.tag_len], 0);
    assert_eq!(out[l.pattern + 8], 0);
    assert_eq!(out[l.reply + 6], 0);
    assert_eq!(l.len, l.reply + 7);
    // runs between the NULs: version, marker, pattern, reply
    let runs: std::vec::Vec<&[u8]> = out[..l.len]
        .split(|&b| b == 0)
        .filter(|r| !r.is_empty())
        .collect();
    assert_eq!(
        runs,
        [
            &b"2.2.0-revamped-dev"[..],
            b"VDM-HW:C12",
            b"DEADBEEF",
            b"BEEFIT"
        ]
    );
}

#[test]
fn the_longest_fields_fit_into_64_bytes() {
    let version = [b'9'; VERSION_MAX];
    let (_, l) = block(&version, b"C99");
    assert!(l.len <= ID_BLOCK_MAX);
    assert_eq!(l.len, 60);
}

#[test]
fn bad_versions_and_tags_are_refused_and_nothing_is_written() {
    let mut out = [0xEE; ID_BLOCK_MAX];
    assert_eq!(write(b"", b"C2", &mut out), Err(IdError::Version));
    assert_eq!(write(&[b'1'; 32], b"C2", &mut out), Err(IdError::Version));
    assert_eq!(write(b"2.2.0 x", b"C2", &mut out), Err(IdError::Version));
    assert_eq!(write(b"2.2.0\x00", b"C2", &mut out), Err(IdError::Version));
    assert_eq!(write(b"2.2.0", b"C", &mut out), Err(IdError::Tag));
    assert_eq!(write(b"2.2.0", b"X2", &mut out), Err(IdError::Tag));
    assert!(out.iter().all(|&b| b == 0xEE));
}

#[test]
fn version_ok_is_printable_ascii_without_space_up_to_31() {
    assert!(version_ok(b"2.2.0-revamped"));
    assert!(version_ok(&[b'!'; 31]));
    assert!(version_ok(b"~"));
    assert!(!version_ok(&[b'1'; 32]));
    assert!(!version_ok(b""));
    assert!(!version_ok(b" "));
    assert!(!version_ok(b"\x7f"));
    assert!(!version_ok(b"\x1f"));
    assert!(!version_ok(b"2.2.0\xff"));
}

#[test]
fn tag_ok_is_c_and_one_or_two_digits() {
    assert!(tag_ok(b"C1"));
    assert!(tag_ok(b"C2"));
    assert!(tag_ok(b"C12"));
    assert!(tag_ok(b"C0"));
    assert!(tag_ok(b"C9"));
    assert!(!tag_ok(b"C"));
    assert!(!tag_ok(b"C123"));
    assert!(!tag_ok(b"c1"));
    assert!(!tag_ok(b"D1"));
    assert!(!tag_ok(b"C1a"));
    assert!(!tag_ok(b"C/"));
    assert!(!tag_ok(b"C:"));
    assert!(!tag_ok(b""));
}

#[test]
fn constants() {
    assert_eq!(ID_BLOCK_ADDR, 0x0800_0200);
    assert_eq!(ID_BLOCK_MAX, 64);
    assert_eq!(&MARKER_PREFIX, b"VDM-HW:");
    assert_eq!(BootId::STANDARD.pattern, *b"DEADBEEF");
    assert_eq!(BootId::STANDARD.reply, *b"BEEFIT");
    assert_eq!(IdLayout::new(0, 0).len, 26);
}
