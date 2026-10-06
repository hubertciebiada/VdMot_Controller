//! Port of test/native/test_version.cpp: gvers grammar (all documented examples, every
//! rejection), comparison, revamped detection, canonical formatting, build constants, fuzz.

use super::*;
use crate::test_support::{assert_text, Rng};
use std::string::String;
use std::vec::Vec;

fn ver(s: &str) -> Version {
    let v = parse_version(s.as_bytes());
    assert!(v.valid, "{s}");
    v
}

fn fmt(v: &Version) -> String {
    let mut out = [0u8; 40];
    let n = format_version(v, &mut out);
    String::from_utf8(out[..n].to_vec()).expect("ASCII")
}

#[test]
fn documented_examples() {
    let cases: [(&str, u16, u16, u16, &str, &str); 21] = [
        ("1.4.9_Dev_C2", 1, 4, 9, "_Dev", "C2"),
        ("1.4.9_C1", 1, 4, 9, "", "C1"),
        ("2.0.0-revamped_C2", 2, 0, 0, "-revamped", "C2"),
        ("2.0.0-revamped-dev", 2, 0, 0, "-revamped-dev", ""),
        ("1.4.12+hc2", 1, 4, 12, "+hc2", ""),
        ("1.4.10", 1, 4, 10, "", ""),
        ("0.0.0", 0, 0, 0, "", ""),
        ("65535.65535.65535", 65535, 65535, 65535, "", ""),
        ("00001.02.3", 1, 2, 3, "", ""),
        ("1.4.9_C99", 1, 4, 9, "", "C99"),
        ("1.4.9_C0", 1, 4, 9, "", "C0"),
        ("1.4.9_C123", 1, 4, 9, "_C123", ""),
        ("1.4.9_C", 1, 4, 9, "_C", ""),
        ("1.4.9_c2", 1, 4, 9, "_c2", ""),
        ("1.4.9_C2_Dev", 1, 4, 9, "_C2_Dev", ""),
        ("1.4.9_C2x", 1, 4, 9, "_C2x", ""),
        ("1.4.9-", 1, 4, 9, "-", ""),
        ("1.4.9_", 1, 4, 9, "_", ""),
        ("1.4.9+", 1, 4, 9, "+", ""),
        ("1.4.9-a.b_c+d-e", 1, 4, 9, "-a.b_c+d-e", ""),
        ("1.4.9__C2", 1, 4, 9, "_", "C2"),
    ];
    for (text, major, minor, patch, suffix, hw) in cases {
        let v = parse_version(text.as_bytes());
        assert!(v.valid, "{text}");
        assert_eq!((v.major, v.minor, v.patch), (major, minor, patch), "{text}");
        assert_text(&v.suffix, suffix);
        assert_text(&v.hw, hw);
    }
}

#[test]
fn longest_strings() {
    // 31 chars: the longest accepted; suffix of 26 chars fits.
    let x25 = "x".repeat(25);
    let s31 = String::from("1.2.3-") + &x25;
    assert_eq!(s31.len(), 31);
    let v = parse_version(s31.as_bytes());
    assert!(v.valid);
    assert_text(&v.suffix, String::from("-") + &x25);
    assert_eq!(fmt(&v), s31);
    let hw31 = String::from("1.2.3-") + &"x".repeat(21) + "_C12";
    assert_eq!(hw31.len(), 31);
    let v = parse_version(hw31.as_bytes());
    assert!(v.valid);
    assert_text(&v.hw, "C12");
    assert_eq!(fmt(&v), hw31);
    let v = parse_version((s31 + "x").as_bytes());
    assert!(!v.valid);
}

#[test]
fn every_rejection_resets_the_output() {
    let bad: [&[u8]; 25] = [
        b"",
        b"1",
        b"1.4",
        b"1.4.",
        b"1..4.9",
        b".1.4.9",
        b"a.b.c",
        b"1.4.9 C2",
        b"1.4.9#",
        b"1.4.9x",
        b"1.4.9.1",
        b"1.4.9a",
        b"65536.0.0",
        b"0.65536.0",
        b"0.0.65536",
        b"123456.0.0",
        b"-1.4.9",
        b"+1.4.9",
        b"1.4.-9",
        b"1,4,9",
        b"1.4.9_Dev!",
        b"1.4.9-\x80",
        b"1.4.9\n",
        b" 1.4.9",
        b"1.4.9 ",
    ];
    for b in bad {
        // C++: the output held "2.0.0-revamped_C2" before; Rust returns a fresh value.
        let v = parse_version(b);
        assert!(!v.valid, "{}", b.escape_ascii());
        assert_eq!(v, Version::default(), "{}", b.escape_ascii());
        assert_eq!(v.major, 0);
        assert!(v.suffix.is_empty());
        assert!(v.hw.is_empty());
    }
    // C++ parseVersion(nullptr, 5, v): no Rust form.
    // Only `len` bytes count; a NUL inside is a bad char.
    let v = parse_version(&b"1.4.9_C2junk"[..8]);
    assert!(v.valid);
    assert_text(&v.hw, "C2");
    assert!(!parse_version(b"1.4.9\0x").valid);
}

#[test]
fn comparison_uses_the_numeric_part_only() {
    assert_eq!(compare_version(&ver("1.4.9"), &ver("1.4.9")), 0);
    assert_eq!(compare_version(&ver("2.0.0-revamped"), &ver("2.0.0")), 0);
    assert_eq!(compare_version(&ver("1.4.9_C1"), &ver("1.4.9_Dev_C2")), 0);
    assert!(compare_version(&ver("1.4.10"), &ver("1.4.9")) > 0);
    assert!(compare_version(&ver("1.4.9"), &ver("1.4.10")) < 0);
    assert!(compare_version(&ver("1.5.0"), &ver("1.4.99")) > 0);
    assert!(compare_version(&ver("1.4.99"), &ver("1.5.0")) < 0);
    assert!(compare_version(&ver("2.0.0"), &ver("1.99.99")) > 0);
    assert!(compare_version(&ver("1.99.99"), &ver("2.0.0")) < 0);
    assert!(compare_version(&ver("2.0.0-revamped"), &ver("1.4.0")) > 0);
    assert!(compare_version(&ver("1.3.9"), &ver("1.4.0")) < 0);
    let invalid = Version::default();
    assert!(compare_version(&invalid, &ver("0.0.0")) < 0);
    assert!(compare_version(&ver("0.0.0"), &invalid) > 0);
    assert_eq!(compare_version(&invalid, &invalid), 0);
    assert_eq!(compare_version(&ver("1.0.0"), &ver("0.0.1")), 1);
    assert_eq!(compare_version(&ver("0.0.1"), &ver("1.0.0")), -1);
    assert_eq!(compare_version(&invalid, &ver("1.0.0")), -1);
    assert_eq!(compare_version(&ver("1.0.0"), &invalid), 1);
}

#[test]
fn character_classes_and_length_edges() {
    let v = ver("1.4.9-azAZ09._+-");
    assert_text(&v.suffix, "-azAZ09._+-");
    for b in [
        "1.4.9-`", "1.4.9-{", "1.4.9-@", "1.4.9-[", "1.4.9-/", "1.4.9-:", "1.4.9-~", "1.4.9-!",
    ] {
        assert!(!parse_version(b.as_bytes()).valid, "{b}");
    }
    // Components: at most 5 digits even when the value is small.
    assert!(parse_version(b"00000.00000.00000").valid);
    assert!(!parse_version(b"000001.2.3").valid);
    assert!(!parse_version(b"1.000001.3").valid);
    assert!(!parse_version(b"1.2.000001").valid);
    // Only `len` bytes are read (buffers need not be terminated).
    let v = parse_version(&b"1.4.95"[..5]);
    assert!(v.valid);
    assert_eq!(v.patch, 9);
    let v = parse_version(&b"1.4.9_C2_"[..8]);
    assert!(v.valid);
    assert_text(&v.hw, "C2");
    assert!(v.suffix.is_empty());
    let v = parse_version(&b"1.4.9_C99"[..9]);
    assert!(v.valid);
    assert_text(&v.hw, "C99");
    assert!(!parse_version(&b"1.4.9"[..0]).valid);
    assert!(!parse_version(b"1").valid);
    assert!(!parse_version(b"1.").valid);
    assert!(!parse_version(b"1.4").valid);
    assert!(!parse_version(b"1.4.").valid);
    // "_C" followed by a non-digit is part of the suffix.
    let v = ver("1.4.9_Cx");
    assert_text(&v.suffix, "_Cx");
    assert!(v.hw.is_empty());
    let v = ver("1.4.9_C1x");
    assert_text(&v.suffix, "_C1x");
    let v = ver("1.4.9_D12");
    assert_text(&v.suffix, "_D12");
}

#[test]
fn comparison_returns_exactly_minus_one_zero_or_one() {
    assert_eq!(compare_version(&ver("1.2.0"), &ver("1.1.9")), 1);
    assert_eq!(compare_version(&ver("1.1.9"), &ver("1.2.0")), -1);
    assert_eq!(compare_version(&ver("1.1.2"), &ver("1.1.1")), 1);
    assert_eq!(compare_version(&ver("1.1.1"), &ver("1.1.2")), -1);
    assert_eq!(compare_version(&ver("3.0.0"), &ver("1.9.9")), 1);
    assert_eq!(compare_version(&ver("1.9.9"), &ver("3.0.0")), -1);
}

#[test]
fn revamped_detection() {
    assert!(is_revamped(&ver("2.0.0-revamped")));
    assert!(is_revamped(&ver("2.0.0-revamped_C2")));
    assert!(is_revamped(&ver("2.0.0-revamped-dev")));
    assert!(is_revamped(&ver("2.0.0_xrevampedx")));
    assert!(!is_revamped(&ver("2.0.0-Revamped")));
    assert!(!is_revamped(&ver("1.4.9_Dev_C2")));
    assert!(!is_revamped(&ver("2.0.0")));
    let fake = Version {
        suffix: Text::from_slice(b"-revamped").expect("fits"), // not valid -> never revamped
        ..Version::default()
    };
    assert!(!is_revamped(&fake));
}

#[test]
fn canonical_formatting() {
    for t in [
        "1.4.9_Dev_C2",
        "1.4.9_C1",
        "2.0.0-revamped_C2",
        "2.0.0-revamped-dev",
        "1.4.12+hc2",
        "65535.65535.65535",
    ] {
        assert_eq!(fmt(&ver(t)), t);
    }
    assert_eq!(fmt(&ver("00001.02.3")), "1.2.3");
    let mut out = [b'z'; 16];
    let v = ver("1.4.9_C1"); // 8 chars
    let n = format_version(&v, &mut out[..9]);
    assert_eq!(n, 8);
    assert_text(&out[..n], "1.4.9_C1");
    out = [b'z'; 16];
    assert_eq!(format_version(&v, &mut out[..8]), 0); // C++: and out = ""
    assert_eq!(out[8], b'z'); // never past cap
    out[0] = b'q';
    assert_eq!(format_version(&v, &mut out[..0]), 0);
    assert_eq!(out[0], b'q');
    let invalid = Version::default();
    assert_eq!(format_version(&invalid, &mut out), 0);
}

#[test]
fn build_constants() {
    assert_eq!(firmware_version(), "0.0.0-native");
    assert_eq!(min_stm_version(), "1.4.0");
    let fw = parse_version(firmware_version().as_bytes());
    let min = parse_version(min_stm_version().as_bytes());
    assert!(fw.valid);
    assert!(min.valid);
    // The revamped STM passes the gate the old ESP applies.
    assert!(compare_version(&ver("2.0.0-revamped_C2"), &min) >= 0);
}

#[test]
fn stm_support_against_the_1_4_0_minimum() {
    assert_eq!(stm_support(&ver("1.3.7_C2")), StmSupport::TooOld);
    assert_eq!(stm_support(&ver("1.3.99")), StmSupport::TooOld);
    assert_eq!(stm_support(&ver("0.9.9")), StmSupport::TooOld);
    assert_eq!(stm_support(&ver("1.4.0_C1")), StmSupport::Supported);
    assert_eq!(stm_support(&ver("1.4.0")), StmSupport::Supported);
    assert_eq!(stm_support(&ver("1.4.9_Dev_C2")), StmSupport::Supported);
    assert_eq!(
        stm_support(&ver("2.1.0-revamped_C2")),
        StmSupport::Supported
    );
    assert_eq!(stm_support(&Version::default()), StmSupport::Unknown);
    let invalid = parse_version(b"1.4");
    assert!(!invalid.valid);
    assert_eq!(stm_support(&invalid), StmSupport::Unknown);

    assert_eq!(stm_support_name(StmSupport::Unknown), "unknown");
    assert_eq!(stm_support_name(StmSupport::Supported), "ok");
    assert_eq!(stm_support_name(StmSupport::TooOld), "too_old");
    // C++ stmSupportName(static_cast<StmSupport>(3)) == "unknown": a Rust enum cannot hold 3,
    // the raw value is refused by from_raw.
    assert_eq!(StmSupport::from_raw(3), None);
    assert_eq!(StmSupport::from_raw(0), Some(StmSupport::Unknown));
    assert_eq!(StmSupport::from_raw(1), Some(StmSupport::Supported));
    assert_eq!(StmSupport::from_raw(2), Some(StmSupport::TooOld));
    assert_eq!(StmSupport::default(), StmSupport::Unknown);
    assert_eq!(StmSupport::TooOld as u8, 2);
}

#[test]
fn fuzz() {
    let mut rng = Rng::new(777);
    let alphabet = b"0123456789._-+Cc revampedDv\x01\xff";
    for _ in 0..20000 {
        let n = rng.below(40) as usize;
        let s: Vec<u8> = (0..n)
            .map(|_| alphabet[rng.below(alphabet.len() as u32) as usize])
            .collect();
        let v = parse_version(&s);
        if v.valid {
            assert!(s.len() <= 31);
            assert!(v.suffix.len() < 27);
            assert!(v.hw.len() < 4);
            // Canonical form parses back to the same fields.
            let mut out = [0u8; 40];
            let len = format_version(&v, &mut out);
            assert!(len > 0);
            let w = parse_version(&out[..len]);
            assert!(w.valid);
            assert_eq!(compare_version(&v, &w), 0);
            assert_eq!(v.suffix, w.suffix);
            assert_eq!(v.hw, w.hw);
        } else {
            assert_eq!(v, Version::default());
        }
    }
}
