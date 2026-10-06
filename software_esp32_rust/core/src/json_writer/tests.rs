//! Port of test/native/test_json_writer.cpp: every value kind, separators, nesting, misuse,
//! escaping, number formats, overflow at every capacity (never writes past it), reset,
//! json_escape. C++ cases that pass a null pointer have no Rust form (named in comments); the
//! C++ NUL the writer keeps after the text is not written in Rust. After the C++ cases: the
//! Rust-only parts (JsonValue impls, into_bytes, C-string keys and raw fragments).

use super::*;
use crate::common::Text;
use crate::test_support::{assert_text, Rng};
use std::vec;
use std::vec::Vec;

/// Writes a fixed document exercising every call.
fn write_sample(jw: &mut JsonWriter<'_>) {
    jw.begin_object();
    jw.kv("s", "a\"b");
    jw.kv("b", true);
    jw.kv("i", -12i32);
    jw.kv("u", 4_000_000_000u32);
    jw.kv("l", -9_000_000_000i64);
    jw.key("n");
    jw.null_value();
    jw.key("f");
    jw.fixed(215, 1);
    jw.key("d");
    jw.number(1.5, 2);
    jw.key("a");
    jw.begin_array();
    jw.value(1);
    jw.begin_object();
    jw.end_object();
    jw.begin_array();
    jw.end_array();
    jw.raw("{\"r\":1}");
    jw.value(&b"x"[..1]);
    jw.value("\x01z"); // a long escape followed by a short char
    jw.end_array();
    jw.end_object();
}

const SAMPLE: &[u8] = b"{\"s\":\"a\\\"b\",\"b\":true,\"i\":-12,\"u\":4000000000,\"l\":-9000000000,\
\"n\":null,\"f\":21.5,\"d\":1.50,\"a\":[1,{},[],{\"r\":1},\"x\",\"\\u0001z\"]}";

#[test]
fn full_sample_document() {
    let mut buf = [0u8; 256];
    let mut jw = JsonWriter::new(&mut buf);
    assert!(jw.ok());
    assert!(!jw.complete());
    assert_eq!(jw.length(), 0);
    assert!(jw.as_bytes().is_empty());
    write_sample(&mut jw);
    assert!(jw.ok());
    assert!(jw.complete());
    assert_text(jw.as_bytes(), SAMPLE);
    assert_eq!(jw.as_bytes().len(), jw.length());
}

#[test]
fn root_scalars_and_single_root() {
    let mut buf = [0u8; 32];
    {
        let mut jw = JsonWriter::new(&mut buf);
        jw.value(5i32);
        assert!(jw.complete());
        assert_text(jw.as_bytes(), "5");
        jw.value(6i32); // second root value
        assert!(!jw.ok());
        assert!(!jw.complete()); // Rust addition: poisoned at depth 0 after a root value
        assert_text(jw.as_bytes(), "5");
    }
    {
        let mut jw = JsonWriter::new(&mut buf);
        jw.value("x");
        assert_text(jw.as_bytes(), "\"x\"");
        assert!(jw.complete());
    }
    {
        let mut jw = JsonWriter::new(&mut buf);
        jw.begin_array();
        jw.end_array();
        assert!(jw.complete());
        jw.begin_array();
        assert!(!jw.ok());
        assert_text(jw.as_bytes(), "[]");
    }
}

#[test]
fn misuse_poisons_the_writer() {
    let mut buf = [0u8; 64];
    let poisoned = |jw: &mut JsonWriter<'_>| {
        assert!(!jw.ok());
        assert!(!jw.complete());
        let before = jw.as_bytes().to_vec();
        jw.begin_object();
        jw.key("k");
        jw.value(1);
        jw.end_object();
        assert_eq!(jw.as_bytes(), &before[..]); // every later call is a no-op
    };
    {
        let mut jw = JsonWriter::new(&mut buf);
        jw.key("k"); // key outside an object
        poisoned(&mut jw);
        assert!(jw.as_bytes().is_empty());
    }
    {
        let mut jw = JsonWriter::new(&mut buf);
        jw.begin_array();
        jw.key("k"); // key in an array
        poisoned(&mut jw);
        assert_text(jw.as_bytes(), "[");
    }
    {
        let mut jw = JsonWriter::new(&mut buf);
        jw.begin_object();
        jw.key("a");
        jw.key("b"); // two keys
        poisoned(&mut jw);
        assert_text(jw.as_bytes(), "{\"a\":");
    }
    {
        let mut jw = JsonWriter::new(&mut buf);
        jw.begin_object();
        jw.value(true); // value without key
        poisoned(&mut jw);
    }
    {
        let mut jw = JsonWriter::new(&mut buf);
        jw.begin_object();
        jw.key("a");
        jw.end_object(); // pending key
        poisoned(&mut jw);
    }
    {
        let mut jw = JsonWriter::new(&mut buf);
        jw.begin_object();
        jw.end_array();
        poisoned(&mut jw);
    }
    {
        let mut jw = JsonWriter::new(&mut buf);
        jw.begin_array();
        jw.end_object();
        poisoned(&mut jw);
    }
    {
        let mut jw = JsonWriter::new(&mut buf);
        jw.end_object();
        poisoned(&mut jw);
    }
    {
        let mut jw = JsonWriter::new(&mut buf);
        jw.end_array();
        poisoned(&mut jw);
    }
    // C++ key(nullptr), value(nullptr, 3) and raw(nullptr): no Rust form.
    {
        let mut jw = JsonWriter::new(&mut buf);
        jw.begin_array();
        jw.raw("");
        poisoned(&mut jw);
    }
}

#[test]
fn nesting_depth() {
    let mut buf = [0u8; 64];
    let mut jw = JsonWriter::new(&mut buf);
    for _ in 0..JsonWriter::MAX_DEPTH {
        jw.begin_array();
    }
    assert!(jw.ok());
    jw.begin_array();
    assert!(!jw.ok());
    assert_text(jw.as_bytes(), "[[[[[[[[");

    let mut ok = JsonWriter::new(&mut buf);
    for _ in 0..JsonWriter::MAX_DEPTH {
        ok.begin_object();
        ok.key("k");
    }
    ok.value(1);
    for _ in 0..JsonWriter::MAX_DEPTH {
        ok.end_object();
    }
    assert!(ok.complete());
    assert_text(
        ok.as_bytes(),
        "{\"k\":{\"k\":{\"k\":{\"k\":{\"k\":{\"k\":{\"k\":{\"k\":1}}}}}}}}",
    );
    let mut deep = JsonWriter::new(&mut buf);
    for _ in 0..JsonWriter::MAX_DEPTH {
        deep.begin_object();
        deep.key("k");
    }
    deep.begin_object();
    assert!(!deep.ok());
}

#[test]
fn string_escaping() {
    let mut buf = [0u8; 128];
    let mut jw = JsonWriter::new(&mut buf);
    jw.begin_array();
    jw.value("\"\\/\x08\x0c\n\r\t");
    jw.value(b"\x01\x1f\x7f \x10");
    jw.value(b"\xc3\xa4");
    jw.value(b"a\0b");
    jw.value(b"");
    jw.value(None::<&str>);
    jw.end_array();
    assert!(jw.complete());
    assert_text(
        jw.as_bytes(),
        b"[\"\\\"\\\\/\\b\\f\\n\\r\\t\",\"\\u0001\\u001f\x7f \\u0010\",\"\xc3\xa4\",\"a\\u0000b\",\
\"\",null]",
    );

    let mut k = JsonWriter::new(&mut buf);
    k.begin_object();
    k.key("a\"\n");
    k.value(false);
    k.end_object();
    assert_text(k.as_bytes(), "{\"a\\\"\\n\":false}");
}

#[test]
fn integers() {
    let mut buf = [0u8; 128];
    let mut jw = JsonWriter::new(&mut buf);
    jw.begin_array();
    jw.value(i32::MIN);
    jw.value(i32::MAX);
    jw.value(0u32);
    jw.value(u32::MAX);
    jw.value(i64::MIN);
    jw.value(i64::MAX);
    jw.end_array();
    assert_text(
        jw.as_bytes(),
        "[-2147483648,2147483647,0,4294967295,-9223372036854775808,9223372036854775807]",
    );
}

#[test]
fn fixed_point() {
    let cases: [(i32, u8, &str); 14] = [
        (215, 1, "21.5"),
        (-5, 1, "-0.5"),
        (12345, 3, "12.345"),
        (0, 1, "0.0"),
        (-1, 3, "-0.001"),
        (7, 0, "7"),
        (-7, 0, "-7"),
        (100, 2, "1.00"),
        (-100, 1, "-10.0"),
        (i32::MIN, 6, "-2147.483648"),
        (i32::MAX, 6, "2147.483647"),
        (i32::MIN, 0, "-2147483648"),
        (5, 6, "0.000005"),
        (0, 0, "0"),
    ];
    for (v, d, text) in cases {
        let mut buf = [0u8; 32];
        let mut jw = JsonWriter::new(&mut buf);
        jw.fixed(v, d);
        assert!(jw.complete(), "{text}");
        assert_text(jw.as_bytes(), text);
    }
    let mut buf = [0u8; 32];
    let mut jw = JsonWriter::new(&mut buf);
    jw.fixed(1, 7);
    assert!(!jw.ok());
    assert!(jw.as_bytes().is_empty());
}

#[test]
fn doubles() {
    let cases: [(f64, u8, &str); 11] = [
        (0.0, 0, "0"),
        (7.0, 0, "7"),
        (21.456, 2, "21.46"),
        (-3.0, 0, "-3"),
        (0.0, 3, "0.000"),
        (1e15, 0, "1000000000000000"),
        (-1e15, 1, "-1000000000000000.0"),
        (0.1234567, 6, "0.123457"),
        (f64::NAN, 2, "null"),
        (f64::INFINITY, 2, "null"),
        (f64::NEG_INFINITY, 0, "null"),
    ];
    for (v, d, text) in cases {
        let mut buf = [0u8; 40];
        let mut jw = JsonWriter::new(&mut buf);
        jw.number(v, d);
        assert!(jw.complete(), "{text}");
        assert_text(jw.as_bytes(), text);
    }
    let mut buf = [0u8; 40];
    let mut a = JsonWriter::new(&mut buf);
    a.number(1.0, 7);
    assert!(!a.ok());
    let mut b = JsonWriter::new(&mut buf);
    b.number(1.000_000_1e15, 0);
    assert!(!b.ok());
    let mut c = JsonWriter::new(&mut buf);
    c.number(-1.000_000_1e15, 0);
    assert!(!c.ok());
    let mut n = JsonWriter::new(&mut buf);
    n.begin_object();
    n.number(f64::NAN, 1); // null without key: misuse
    assert!(!n.ok());
}

#[test]
fn overflow_at_every_capacity_never_past_the_buffer() {
    let mut reference = [0u8; 256];
    let mut full = JsonWriter::new(&mut reference);
    write_sample(&mut full);
    let whole = full.as_bytes().to_vec();
    assert_eq!(&whole[..], SAMPLE);
    for cap in 0..=whole.len() + 1 {
        let mut buf = vec![b'#'; cap + 8];
        let fits = cap > whole.len();
        let got = {
            let mut jw = JsonWriter::new(&mut buf[..cap]);
            write_sample(&mut jw);
            assert_eq!(jw.ok(), fits, "cap {cap}");
            assert_eq!(jw.complete(), fits, "cap {cap}");
            assert_eq!(jw.as_bytes().len(), jw.length(), "cap {cap}");
            jw.as_bytes().to_vec()
        };
        assert!(buf[cap..].iter().all(|&c| c == b'#'), "cap {cap}");
        if cap == 0 {
            continue;
        }
        assert!(got.len() < cap, "cap {cap}");
        assert!(whole.starts_with(&got), "cap {cap}"); // a prefix at a token boundary
    }
}

#[test]
fn null_buffer_reset_and_c_str() {
    // C++ JsonWriter(nullptr, 100): an empty buffer.
    let mut none = JsonWriter::new(&mut []);
    assert!(!none.ok());
    none.value(1);
    assert!(none.as_bytes().is_empty());
    assert_eq!(none.length(), 0);
    none.reset();
    assert!(!none.ok());

    let mut one = [b'x'; 1];
    {
        let mut tiny = JsonWriter::new(&mut one);
        assert!(tiny.ok());
        assert!(tiny.as_bytes().is_empty());
        tiny.value(1);
        assert!(!tiny.ok());
        assert!(tiny.as_bytes().is_empty());
    }
    assert_eq!(one, [b'x']); // the C++ writes its NUL there, Rust writes nothing

    let mut buf = [0u8; 32];
    let mut jw = JsonWriter::new(&mut buf);
    jw.begin_object();
    jw.key("a");
    jw.reset();
    assert!(jw.ok());
    assert_eq!(jw.length(), 0);
    assert!(jw.as_bytes().is_empty());
    jw.begin_array();
    jw.value(true);
    jw.end_array();
    assert!(jw.complete());
    assert_text(jw.as_bytes(), "[true]");
    jw.value(1); // poisoned
    jw.reset(); // and recovered
    jw.value(2);
    assert_text(jw.as_bytes(), "2");
}

#[test]
fn kv_covers_every_value_overload() {
    let mut buf = [0u8; 128];
    let mut jw = JsonWriter::new(&mut buf);
    jw.begin_object();
    jw.kv("a", "s");
    jw.kv("b", false);
    jw.kv("c", -1i32);
    jw.kv("d", 1u32);
    jw.kv("e", 2i64);
    jw.kv("f", None::<&str>);
    jw.end_object();
    assert_text(
        jw.as_bytes(),
        "{\"a\":\"s\",\"b\":false,\"c\":-1,\"d\":1,\"e\":2,\"f\":null}",
    );
}

#[test]
fn json_escape_helper() {
    let mut out = [0u8; 16];
    let n = json_escape(b"a\"b", &mut out);
    assert_eq!(n, 4);
    assert_text(&out[..n], "a\\\"b");
    assert_eq!(json_escape(b"", &mut out), 0);
    let n = json_escape(b"\n", &mut out);
    assert_eq!(n, 2);
    assert_text(&out[..n], "\\n");
    // C++ jsonEscape(nullptr, ...) == 0: no Rust form.
    // Exact fit: the escaped text plus its NUL.
    let n = json_escape(b"abcd", &mut out[..5]);
    assert_eq!(n, 4);
    assert_text(&out[..n], "abcd");
    out = [b'z'; 16];
    assert_eq!(json_escape(b"abcd", &mut out[..4]), 0); // C++: and out = ""
    assert_eq!(out[4], b'z');
    // Escape sequences are never split: "a\n" needs 3 + 1.
    let n = json_escape(b"a\n", &mut out[..4]);
    assert_eq!(n, 3);
    assert_text(&out[..n], "a\\n");
    out = [b'z'; 16];
    assert_eq!(json_escape(b"a\n", &mut out[..3]), 0);
    assert_eq!(out[3], b'z');
    // Control bytes use \u00XX; the six chars plus NUL need 7.
    let n = json_escape(b"\x1f", &mut out[..7]);
    assert_eq!(n, 6);
    assert_text(&out[..n], "\\u001f");
    assert_eq!(json_escape(b"\x1f", &mut out[..6]), 0);
    let n = json_escape(b"\"\\/\t\x08\x0c\r", &mut out);
    assert_eq!(n, 13);
    assert_text(&out[..n], "\\\"\\\\/\\t\\b\\f\\r");
    out[0] = b'q';
    assert_eq!(json_escape(b"a", &mut out[..0]), 0);
    assert_eq!(out[0], b'q');
    // too small
    assert_eq!(json_escape(b"a", &mut out[..1]), 0);
    // Rust addition: the input is a C string.
    let n = json_escape(b"a\0\n", &mut out);
    assert_eq!(n, 1);
    assert_text(&out[..n], "a");
}

#[test]
fn random_call_sequences_stay_bounded() {
    // C++ std::mt19937 rng(1234). The C++ evaluates the two rng() calls of fixed() and number()
    // in an unspecified order; Rust takes them left to right.
    let mut rng = Rng::new(1234);
    for _ in 0..2000 {
        let cap = rng.below(80) as usize;
        let mut buf = vec![b'#'; cap + 4];
        let len = {
            let mut jw = JsonWriter::new(&mut buf[..cap]);
            for _ in 0..30 {
                match rng.below(10) {
                    0 => jw.begin_object(),
                    1 => jw.end_object(),
                    2 => jw.begin_array(),
                    3 => jw.end_array(),
                    4 => jw.key("k\"y"),
                    5 => jw.value("v\x01"),
                    6 => jw.value(rng.next_u32() as i32),
                    7 => jw.fixed(rng.next_u32() as i32, rng.below(8) as u8),
                    8 => jw.number(f64::from(rng.next_u32()) / 7.0, rng.below(8) as u8),
                    _ => jw.null_value(),
                }
            }
            assert_eq!(jw.as_bytes().len(), jw.length());
            jw.length()
        };
        assert!(buf[cap..].iter().all(|&c| c == b'#'));
        assert!(len < cap.max(1));
    }
}

#[test]
fn a_key_whose_colon_does_not_fit_fails_at_once() {
    let mut buf = [0u8; 5];
    let mut jw = JsonWriter::new(&mut buf);
    jw.begin_object();
    assert!(jw.ok());
    jw.key("a"); // "{\"a\"" fits, the ':' does not
    assert!(!jw.ok());
    assert_text(jw.as_bytes(), "{");
    let mut buf6 = [0u8; 6];
    let mut ok6 = JsonWriter::new(&mut buf6);
    ok6.begin_object();
    ok6.key("a");
    assert!(ok6.ok());
    assert_text(ok6.as_bytes(), "{\"a\":");
}

// ---------------------------------------------------------------- Rust-only parts

#[test]
fn every_json_value_impl() {
    let mut buf = [0u8; 160];
    let name: Text<4> = Text::from_slice(b"ab\"c").expect("fits");
    let mut jw = JsonWriter::new(&mut buf);
    jw.begin_array();
    jw.value(-8i8);
    jw.value(200u8);
    jw.value(-300i16);
    jw.value(60_000u16);
    jw.value(&b"sl"[..]);
    jw.value(b"ar");
    jw.value(&name);
    jw.value("a\0b"); // exact: the NUL is a character
    jw.value(Some(5));
    jw.value(Some("x"));
    jw.value(None::<i32>);
    jw.value(Some(true));
    jw.end_array();
    assert!(jw.complete());
    assert_text(
        jw.into_bytes(),
        "[-8,200,-300,60000,\"sl\",\"ar\",\"ab\\\"c\",\"a\\u0000b\",5,\"x\",null,true]",
    );
}

#[test]
fn keys_and_raw_fragments_are_c_strings() {
    let mut buf = [0u8; 64];
    let mut jw = JsonWriter::new(&mut buf);
    jw.begin_object();
    jw.key(b"a\0b");
    jw.raw(b"1\0x");
    jw.kv("c", 2);
    jw.end_object();
    assert_text(jw.as_bytes(), "{\"a\":1,\"c\":2}");
    let mut empty = JsonWriter::new(&mut buf);
    empty.begin_array();
    empty.raw(b"\0x"); // empty C string
    assert!(!empty.ok());
    assert_text(empty.as_bytes(), "[");
}

#[test]
fn into_bytes_outlives_the_writer() {
    let mut buf = [0u8; 16];
    let doc: &[u8] = {
        let mut jw = JsonWriter::new(&mut buf);
        jw.value(12);
        jw.into_bytes()
    };
    assert_eq!(doc, b"12");
    let mut tiny = [0u8; 2];
    let mut jw = JsonWriter::new(&mut tiny);
    jw.value(12); // does not fit
    assert_eq!(jw.into_bytes(), b"");
    let collected: Vec<u8> = buf[..2].to_vec();
    assert_eq!(collected, b"12");
}
