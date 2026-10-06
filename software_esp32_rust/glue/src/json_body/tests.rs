//! Unit tests of json_body: the limits, every error branch, the queries and the API that the web
//! server port uses. The parity with ArduinoJson itself is `tests_golden.rs`.
#![allow(clippy::large_stack_frames, clippy::large_stack_arrays)]

use super::*;

// ---------------------------------------------------------------- helpers

type E = DeserializationError;

/// Runs `f` on the outcome of `body`, parsed by a boxed document as the web working set holds it.
fn with<R>(body: &[u8], f: impl FnOnce(Result<JsonVariantConst<'_>, E>) -> R) -> R {
    let mut doc = Box::new(JsonDocument::EMPTY);
    let mut input = body.to_vec();
    f(doc.deserialize(&mut input))
}

fn root<R>(body: &[u8], f: impl FnOnce(JsonVariantConst<'_>) -> R) -> R {
    with(body, |r| {
        f(r.unwrap_or_else(|e| panic!("{e:?} for {}", show(body))))
    })
}

fn error(body: &[u8]) -> E {
    with(body, |r| match r {
        Ok(_) => panic!("parsed: {}", show(body)),
        Err(e) => e,
    })
}

/// The outcome without the value, and the pool bytes in use after it.
fn outcome(body: &[u8]) -> (Result<(), E>, usize) {
    let mut doc = Box::new(JsonDocument::EMPTY);
    let mut input = body.to_vec();
    let r = doc.deserialize(&mut input).map(|_| ());
    (r, doc.memory_usage())
}

fn show(body: &[u8]) -> String {
    String::from_utf8_lossy(&body[..body.len().min(80)]).into_owned()
}

/// `{"k0":0,...}` with `n` members.
fn object(n: usize) -> String {
    let members: Vec<String> = (0..n).map(|i| format!("\"k{i}\":{i}")).collect();
    format!("{{{}}}", members.join(","))
}

/// `[1,...]` with `n` elements followed by `tail` before the `]`.
fn array(n: usize, tail: &str) -> String {
    format!("[{}{tail}]", vec!["1"; n].join(","))
}

fn number(text: &str) -> Option<Number> {
    parse_number(text.as_bytes())
}

fn float(text: &str) -> u64 {
    match number(text) {
        Some(Number::Float(f)) => f.to_bits(),
        other => panic!("{text}: {other:?}"),
    }
}

// ---------------------------------------------------------------- document and errors

#[test]
fn errors_carry_the_names_and_codes_of_arduinojson() {
    let names = [
        (E::EmptyInput, "EmptyInput", 1),
        (E::IncompleteInput, "IncompleteInput", 2),
        (E::InvalidInput, "InvalidInput", 3),
        (E::NoMemory, "NoMemory", 4),
        (E::TooDeep, "TooDeep", 5),
    ];
    for (e, name, code) in names {
        assert_eq!(e.c_str(), name);
        assert_eq!(e as u8, code);
    }
}

#[test]
fn the_document_has_the_esp32_pool_and_is_cheap_to_box() {
    assert_eq!((SLOT_COUNT, SLOT_SIZE, NESTING_LIMIT), (32, 16, 10));
    assert_eq!(SLOT_COUNT * SLOT_SIZE, 512); // StaticJsonDocument<512>
    assert_eq!(core::mem::size_of::<Slot>(), 16);
    assert_eq!(core::mem::size_of::<JsonDocument>(), 536);
    assert_eq!(JsonDocument::EMPTY.memory_usage(), 0);
    assert_eq!(MAX_INPUT as u64, u64::from(u32::MAX));
    assert_eq!(MANTISSA_MAX, 0x000F_FFFF_FFFF_FFFF);
    assert_eq!(EMPTY_COLLECTION, u64::from(NONE) * 0x101);
}

#[test]
fn a_document_is_cleared_by_every_body() {
    let mut doc = Box::new(JsonDocument::EMPTY);
    let mut full = object(32).into_bytes();
    assert_eq!(doc.deserialize(&mut full).map(|r| r.size()).ok(), Some(32));
    assert_eq!(doc.memory_usage(), 512);
    let mut one = br#"{"a":1}"#.to_vec();
    assert_eq!(doc.deserialize(&mut one).map(|r| r.size()).ok(), Some(1));
    assert_eq!(doc.memory_usage(), 16);
    let mut bad = b"[1,2,x]".to_vec();
    assert!(doc.deserialize(&mut bad).is_err());
    assert_eq!(doc.memory_usage(), 48);
    let mut number = b"5".to_vec();
    let root = doc.deserialize(&mut number).expect("a number");
    assert_eq!(
        (root.as_i64(), root.is_object(), root.size()),
        (5, false, 0)
    );
    assert_eq!(doc.memory_usage(), 0);
}

// ---------------------------------------------------------------- error branches

#[test]
fn empty_input_is_whitespace_up_to_the_end_or_a_nul() {
    for body in [
        &b""[..],
        b" ",
        b" \t\r\n",
        b"\0",
        b"\0{}",
        b" \0{}",
        b"\n\0x",
    ] {
        assert_eq!(error(body), E::EmptyInput, "{}", show(body));
    }
    assert_eq!(error(&[b' '; 8192]), E::EmptyInput);
    // form feed, vertical TAB and NBSP are no whitespace
    for body in [&b"\x0c{}"[..], b"\x0b{}", b"\xc2\xa0{}"] {
        assert_eq!(error(body), E::InvalidInput, "{}", show(body));
    }
}

#[test]
fn incomplete_input_where_the_input_ends_inside_a_value() {
    let cases: [&[u8]; 30] = [
        b"{",
        b"{ ",
        b"{\"a\"",
        b"{\"a\" ",
        b"{a",
        b"{abc\0:1}",
        b"{\"a",
        b"{\"a\0\":1}",
        b"{\"a\":",
        b"{\"a\": ",
        b"{\"a\":1",
        b"{\"a\":1\0}",
        b"{\"a\":1,",
        b"{\"a\":\"abc",
        b"{\"a\":\"x\0y\"}",
        b"{\"a\":\"ab\\",
        b"{\"a\":\"\\\0\"}",
        b"{\"a\":\"\\u",
        b"{\"a\":\"\\u123",
        b"{\"a\":\"\\u12\0\"}",
        b"{\"a\":tr",
        b"{\"a\":t\0}",
        b"{\"a\":fals",
        b"{\"a\":nu",
        b"[",
        b"[1",
        b"[1,",
        b"[1 \0]",
        b"\"abc",
        b"{'a\":1}",
    ];
    for body in cases {
        assert_eq!(error(body), E::IncompleteInput, "{}", show(body));
    }
}

#[test]
fn invalid_input_in_every_branch() {
    let cases: [&[u8]; 36] = [
        b"{\"a\" 1}",             // no colon
        b"{\"a\":1 \"b\":2}",     // no comma in an object
        b"[1 2]",                 // no comma in an array
        b"{,}",                   // no key
        b"{\"a\":1,}",            // trailing comma in an object
        b"[1,]",                  // trailing comma in an array
        b"{a-b:1}",               // character after an unquoted key
        b"{$a:1}",                // no unquoted key
        b"{\xc3\xa9:1}",          // bytes >= 0x80 in an unquoted key
        b"{\"a\":abc}",           // a word is no value
        b"{\"a\":-}",             // sign without digits
        b"{\"a\":+-1}",           // two signs
        b"{\"a\":1.5.5}",         // text after a number
        b"{\"a\":1e5e5}",         // text after the exponent
        b"{\"a\":e}",             // exponent without mantissa
        b"{\"a\":NaN}",           // ARDUINOJSON_ENABLE_NAN 0
        b"{\"a\":Infinity}",      // ARDUINOJSON_ENABLE_INFINITY 0
        b"{\"a\":\"\\x41\"}",     // unknown escape
        b"{\"a\":\"\\'\"}",       // no \' even in quotes
        b"{'a':'\\''}",           // ... single ones
        b"{\"a\":\"\\0\"}",       // no octal escape
        b"{\"a\":\"\\u00G0\"}",   // no hex digit
        b"{\"a\":\"\\u00/0\"}",   // below '0'
        b"{\"a\":\"\\u00@0\"}",   // between '?' and 'A'
        b"{\"a\":\"\\u00\xc3\"}", // a byte >= 0x80
        b"{\"a\":True}",          // keywords are lowercase
        b"{\"a\":nulL}",          // ... completely
        b"{\"a\":fx}",            // ... from the second character on
        b"{\"a\":[1}",            // closing the wrong collection
        b"[{\"a\":1]",            // ... both ways
        b"{/*c*/\"a\":1}",        // ARDUINOJSON_ENABLE_COMMENTS 0
        b"//c\n{}",               // ... at the root too
        b"7 ",                    // a root number must end the input
        b"7x",                    // ... at its last character
        b"]",                     // no value at the root
        b"{\"a\":[]x}",           // after a nested collection
    ];
    for body in cases {
        assert_eq!(error(body), E::InvalidInput, "{}", show(body));
    }
}

#[test]
fn input_after_a_root_value_other_than_a_number_is_not_read() {
    for body in [
        &b"{}x"[..],
        b"{}}",
        b"{\"a\":1}\0garbage",
        b"[1]x",
        b"\"x\"y",
        b"truex",
        b"false ",
        b"nullx",
        b"7\0x",
    ] {
        assert!(with(body, |r| r.is_ok()), "{}", show(body));
    }
    assert!(root(b"7", |r| r.as_i64() == 7));
    assert!(root(b"{\"a\":1}\n", |r| r.size() == 1));
}

#[test]
fn members_and_elements_share_the_32_slots() {
    assert_eq!(outcome(object(31).as_bytes()), (Ok(()), 496));
    assert_eq!(outcome(object(32).as_bytes()), (Ok(()), 512));
    assert_eq!(outcome(object(33).as_bytes()), (Err(E::NoMemory), 512));
    assert_eq!(outcome(array(32, "").as_bytes()), (Ok(()), 512));
    assert_eq!(outcome(array(33, "").as_bytes()), (Err(E::NoMemory), 512));
    // a nested collection takes the slot of its member, its members one each
    assert_eq!(
        outcome(format!("{{\"a\":{}}}", object(31)).as_bytes()),
        (Ok(()), 512)
    );
    assert_eq!(
        outcome(format!("{{\"a\":{}}}", object(32)).as_bytes()),
        (Err(E::NoMemory), 512)
    );
    assert_eq!(
        outcome(format!("[{}]", array(31, "")).as_bytes()),
        (Ok(()), 512)
    );
    assert_eq!(
        outcome(b"{\"a\":{},\"b\":[],\"c\":{\"d\":[]}}"),
        (Ok(()), 64)
    );
    // an element takes its slot before it is parsed
    assert_eq!(
        outcome(array(31, ",").as_bytes()),
        (Err(E::InvalidInput), 512)
    );
    assert_eq!(outcome(array(32, ",").as_bytes()), (Err(E::NoMemory), 512));
    // a member after its key and colon
    let full = object(32);
    let open = &full[..full.len() - 1];
    assert_eq!(
        outcome(format!("{open},}}").as_bytes()),
        (Err(E::InvalidInput), 512)
    );
    assert_eq!(
        outcome(format!("{open},\"x\"}}").as_bytes()),
        (Err(E::InvalidInput), 512)
    );
    assert_eq!(
        outcome(format!("{open},\"x\":").as_bytes()),
        (Err(E::NoMemory), 512)
    );
    assert_eq!(
        outcome(format!("{open},\"k7\":5}}").as_bytes()),
        (Ok(()), 512)
    );
}

#[test]
fn nesting_stops_after_ten_levels() {
    let objects = |n: usize| format!("{}1{}", "{\"a\":".repeat(n), "}".repeat(n));
    let arrays = |n: usize| format!("{}{}", "[".repeat(n), "]".repeat(n));
    assert_eq!(outcome(objects(10).as_bytes()), (Ok(()), 160));
    assert_eq!(outcome(objects(11).as_bytes()), (Err(E::TooDeep), 160));
    // the eleventh array has its element slot when it is refused
    assert_eq!(outcome(arrays(10).as_bytes()), (Ok(()), 144));
    assert_eq!(outcome(arrays(11).as_bytes()), (Err(E::TooDeep), 160));
    assert_eq!(error("[".repeat(10).as_bytes()), E::IncompleteInput);
    assert_eq!(error("[".repeat(11).as_bytes()), E::TooDeep);
    let mixed = |n: usize| format!("{}1{}", "{\"a\":[".repeat(n), "]}".repeat(n));
    assert!(with(mixed(5).as_bytes(), |r| r.is_ok()));
    assert_eq!(error(format!("[{}]", mixed(5)).as_bytes()), E::TooDeep);
    // the deepest level holds scalars of any kind
    let deep = format!(
        "{}[1,\"x\",true,null,{{}},[]]{}",
        "[".repeat(8),
        "]".repeat(8)
    );
    assert!(root(deep.as_bytes(), |r| r.size() == 1));
}

#[test]
fn numbers_take_at_most_63_characters() {
    let ones = |n: usize| format!("{{\"a\":{}}}", "1".repeat(n));
    assert!(root(ones(63).as_bytes(), |r| r.member("a").is_f64()));
    assert_eq!(error(ones(64).as_bytes()), E::InvalidInput);
    assert!(root("9".repeat(63).as_bytes(), |r| r.is_f64()));
    assert_eq!(error("9".repeat(64).as_bytes()), E::InvalidInput);
}

// ---------------------------------------------------------------- values and queries

impl<'a> JsonVariantConst<'a> {
    /// The member `key` of an object.
    fn member(&self, key: &str) -> JsonVariantConst<'a> {
        self.as_object()
            .map(|o| o.get(key.as_bytes()))
            .expect("an object")
    }
}

#[test]
fn queries_convert_as_arduinojson() {
    let body = br#"{"u":7,"big":18446744073709551615,"i":-7,"min":-9223372036854775808,
        "f":1.5,"nf":-1.5,"t":true,"z":false,"n":null,"s":"42","x":"abc","o":{"a":1},"a":[1,2],
        "f0":0.0,"huge":1e19,"tiny":-0.0}"#;
    root(body, |r| {
        let q = |key: &str| {
            let v = r.member(key);
            (
                v.is_i64(),
                v.as_i64(),
                v.is_f64(),
                v.as_f64().to_bits(),
                v.is_bool(),
                v.as_bool(),
            )
        };
        assert_eq!(q("u"), (true, 7, true, 7f64.to_bits(), false, true));
        assert_eq!(
            q("big"),
            (false, 0, true, 0x43F0_0000_0000_0000, false, true)
        );
        assert_eq!(q("i"), (true, -7, true, (-7f64).to_bits(), false, true));
        assert_eq!(
            q("min"),
            (true, i64::MIN, true, 0xC3E0_0000_0000_0000, false, true)
        );
        assert_eq!(q("f"), (false, 1, true, 1.5f64.to_bits(), false, true));
        assert_eq!(q("nf"), (false, -1, true, (-1.5f64).to_bits(), false, true));
        assert_eq!(q("t"), (false, 1, false, 1f64.to_bits(), true, true));
        assert_eq!(q("z"), (false, 0, false, 0, true, false));
        assert_eq!(q("n"), (false, 0, false, 0, false, false));
        assert_eq!(q("s"), (false, 42, false, 42f64.to_bits(), false, true));
        assert_eq!(q("x"), (false, 0, false, 0, false, true));
        assert_eq!(q("o"), (false, 0, false, 0, false, true));
        assert_eq!(q("a"), (false, 0, false, 0, false, true));
        assert_eq!(q("f0"), (false, 0, true, 0, false, false));
        assert_eq!(q("huge"), (false, 0, true, 1e19f64.to_bits(), false, true));
        assert_eq!(q("tiny"), (false, 0, true, (-0f64).to_bits(), false, false));
        assert_eq!(q("missing"), (false, 0, false, 0, false, false));

        let kinds = |key: &str| {
            let v = r.member(key);
            (
                v.is_null(),
                v.is_str(),
                v.is_object(),
                v.is_array(),
                v.size(),
            )
        };
        assert_eq!(kinds("n"), (true, false, false, false, 0));
        assert_eq!(kinds("missing"), (true, false, false, false, 0));
        assert_eq!(kinds("s"), (false, true, false, false, 0));
        assert_eq!(kinds("o"), (false, false, true, false, 1));
        assert_eq!(kinds("a"), (false, false, false, true, 2));
        assert_eq!(kinds("u"), (false, false, false, false, 0));
        assert_eq!(r.member("s").as_str(), Some(&b"42"[..]));
        assert_eq!(r.member("u").as_str(), None);
        assert!(r.member("a").as_object().is_none());
        assert!(r.member("o").as_array().is_none());
        assert_eq!(
            r.member("o").as_object().map(|o| o.get(b"a").as_i64()),
            Some(1)
        );
        assert_eq!((r.is_object(), r.size()), (true, 16));
    });
}

#[test]
fn doubles_convert_to_i64_inside_the_range_only() {
    let to_i64 = |bits: u64| Number::Float(f64::from_bits(bits)).to_i64();
    assert_eq!(to_i64(0xC3E0_0000_0000_0000), i64::MIN); // -2^63
    assert_eq!(to_i64(0xC3E0_0000_0000_0001), 0); // the next double below
    assert_eq!(to_i64(I64_HIGHEST), 9_223_372_036_854_774_784);
    assert_eq!(to_i64(0x43E0_0000_0000_0000), 0); // 2^63
    assert_eq!(to_i64(NAN_BITS), 0);
    assert_eq!(to_i64(f64::INFINITY.to_bits()), 0);
    assert_eq!(Number::Unsigned(1 << 63).to_i64(), 0);
    assert_eq!(Number::Unsigned((1 << 63) - 1).to_i64(), i64::MAX);
    // the parser never makes a double that close to 2^63
    let as_i64 = |text: &str| root(text.as_bytes(), |r| r.as_i64());
    assert_eq!(as_i64("-9223372036854775808.0"), -9_223_372_036_854_769_664);
    assert_eq!(as_i64("-9223372036854780000"), 0);
    assert_eq!(as_i64("9223372036854780000.0"), 0);
    assert_eq!(as_i64("2.5e0"), 2);
    assert_eq!(as_i64("-2.5e0"), -2);
    assert_eq!(root(b"{\"s\":\"1e400\"}", |r| r.member("s").as_i64()), 0);
    assert_eq!(root(b"{\"s\":\"-12.5\"}", |r| r.member("s").as_i64()), -12);
    assert_eq!(root(b"{\"s\":\"-12\"}", |r| r.member("s").as_f64()), -12.0);
}

#[test]
fn a_string_is_a_c_string_for_strcmp() {
    root(
        br#"{"s":"factory-reset\u0000x","e":"","n":"\u0000"}"#,
        |r| {
            assert_eq!(r.member("s").as_str(), Some(&b"factory-reset"[..]));
            assert_eq!(r.member("e").as_str(), Some(&b""[..]));
            assert_eq!(r.member("n").as_str(), Some(&b""[..]));
            assert!(r.member("n").is_str());
        },
    );
    // the number view of a string ends at its NUL as well
    assert_eq!(root(br#"{"s":"4\u00002"}"#, |r| r.member("s").as_i64()), 4);
}

#[test]
fn strings_are_unescaped_in_place_into_the_body() {
    let mut doc = Box::new(JsonDocument::EMPTY);
    let mut input = br#"{"k":"a\nb", 'q' : "\u00e9" }"#.to_vec();
    let start = input.as_ptr() as usize;
    let root = doc.deserialize(&mut input).expect("parsed");
    let o = root.as_object().expect("an object");
    let s = o.get(b"k").as_str().expect("a string");
    assert_eq!(s, b"a\nb");
    assert_eq!(s.as_ptr() as usize - start, 2);
    assert_eq!(o.get(b"q").as_str(), Some(&b"\xc3\xa9"[..]));
    // keys and values with their NUL, packed from the start; the rest is the old input
    assert_eq!(&input[..12], b"k\0a\nb\0q\0\xc3\xa9\0,");
}

#[test]
fn escapes_are_decoded_like_the_library() {
    let decoded = |escaped: &str| {
        let body = format!("{{\"a\":\"{escaped}\"}}");
        root(body.as_bytes(), |r| r.member("a").value_bytes())
    };
    assert_eq!(decoded(r#"\"\\\/\b\f\n\r\t"#), b"\"\\/\x08\x0c\n\r\t");
    assert_eq!(decoded(r"\u0041\u007f\u0080"), b"A\x7f\xc2\x80");
    assert_eq!(
        decoded(r"\u07ff\u0800\uffff"),
        b"\xdf\xbf\xe0\xa0\x80\xef\xbf\xbf"
    );
    assert_eq!(decoded(r"\ud7ff\ue000"), b"\xed\x9f\xbf\xee\x80\x80");
    assert_eq!(
        decoded(r"\ud83d\ude00\uD800\uDC00"),
        b"\xf0\x9f\x98\x80\xf0\x90\x80\x80"
    );
    assert_eq!(decoded(r"\udbff\udfff"), b"\xf4\x8f\xbf\xbf");
    // a high surrogate waits for a low one, a low one pairs with the last high one or 0
    assert_eq!(decoded(r"\ud83d"), b"");
    assert_eq!(decoded(r"\ud83dx\ude00"), b"x\xf0\x9f\x98\x80");
    assert_eq!(decoded(r"\ud83d\u0041"), b"A");
    assert_eq!(
        decoded(r"\ud800\ud83d\ude00\ude01"),
        b"\xf0\x9f\x98\x80\xf0\x9f\x98\x81"
    );
    assert_eq!(
        decoded(r"\udc00\udfff"),
        b"\xf0\x90\x80\x80\xf0\x90\x8f\xbf"
    );
    // hex digits below 'A' are the character minus '0'
    assert_eq!(decoded(r"\u003:\u00??\uAbCd"), b":\xc3\xbf\xea\xaf\x8d");
    assert_eq!(decoded(r"x\u0000y"), b"x\0y");
    assert_eq!(decoded("\u{1}\t\u{7f}"), b"\x01\t\x7f");
}

impl JsonVariantConst<'_> {
    /// The stored bytes of a string, NULs included.
    fn value_bytes(&self) -> Vec<u8> {
        match self.value() {
            Value::Str(s) => s.to_vec(),
            _ => panic!("no string"),
        }
    }
}

#[test]
fn hex_digits_as_decode_hex_reads_them() {
    for c in 0..=255u8 {
        let expected = match c {
            b'0'..=b'?' => Some(c - b'0'),
            b'A'..=b'F' => Some(c - b'A' + 10),
            b'a'..=b'f' => Some(c - b'a' + 10),
            b'`' => Some(9), // '`' & ~0x20 is '@', one below 'A'
            _ => None,
        };
        let value = decode_hex(c);
        assert_eq!((value <= 0x0F).then_some(value), expected, "{c:#04x}");
    }
}

// ---------------------------------------------------------------- keys

#[test]
fn keys_quoted_and_unquoted() {
    root(
        br#"{a:1,'b':2,"c":3,09AZaz_`:4,true:5," d ":6,"":7}"#,
        |r| {
            let o = r.as_object().expect("an object");
            let keys: Vec<&[u8]> = o.iter().map(|(k, _)| k).collect();
            assert_eq!(
                keys,
                [&b"a"[..], b"b", b"c", b"09AZaz_`", b"true", b" d ", b""]
            );
            let values: Vec<i64> = o.into_iter().map(|(_, v)| v.as_i64()).collect();
            assert_eq!(values, [1, 2, 3, 4, 5, 6, 7]);
            assert_eq!(o.size(), 7);
        },
    );
    for body in [
        &b"{[:1}"[..],
        b"{\\a:1}",
        b"{^a:1}",
        b"{@:1}",
        b"{~:1}",
        b"{{}:1}",
        b"{a b:1}",
    ] {
        assert_eq!(error(body), E::InvalidInput, "{}", show(body));
    }
}

#[test]
fn lookups_compare_c_strings() {
    root(br#"{"target\u0000x":5,"Ab":1,"ab":2,"":3}"#, |r| {
        let o = r.as_object().expect("an object");
        assert_eq!(o.get(b"target").as_i64(), 5);
        assert_eq!(o.get(b"target\0anything").as_i64(), 5);
        assert_eq!(o.get(b"Ab").as_i64(), 1);
        assert_eq!(o.get(b"ab").as_i64(), 2);
        assert_eq!(o.get(b"").as_i64(), 3);
        assert_eq!(o.get(b"\0x").as_i64(), 3);
        for missing in [&b"targe"[..], b"targets", b"Target", b"AB", b"a"] {
            let v = o.get(missing);
            assert!(v.is_null() && !v.is_i64() && !v.is_f64() && !v.is_bool() && !v.is_str());
            assert!(!v.is_object() && !v.is_array() && v.as_object().is_none());
            assert_eq!(
                (v.as_i64(), v.as_f64(), v.as_bool(), v.as_str(), v.size()),
                (0, 0.0, false, None, 0)
            );
        }
    });
}

#[test]
fn a_duplicate_key_reuses_its_member() {
    let members = |body: &[u8]| {
        root(body, |r| {
            let o = r.as_object().expect("an object");
            o.iter()
                .map(|(k, v)| (k.to_vec(), v.as_i64(), v.is_null()))
                .collect::<Vec<_>>()
        })
    };
    let m = |k: &str, v: i64| (k.as_bytes().to_vec(), v, false);
    assert_eq!(members(br#"{"a":1,"b":2,"a":3}"#), [m("a", 3), m("b", 2)]);
    assert_eq!(members(br#"{a:1,"a":2,'a':3,"\u0061":4}"#), [m("a", 4)]);
    assert_eq!(members(br#"{"a\u0000b":1,"a":2}"#), [m("a", 2)]);
    assert_eq!(
        members(br#"{"a":1,"A":2,"ab":3}"#),
        [m("a", 1), m("A", 2), m("ab", 3)]
    );
    // null leaves the earlier value, any other value replaces it
    assert_eq!(members(br#"{"a":1,"a":null}"#), [m("a", 1)]);
    assert_eq!(members(br#"{"a":null,"a":1}"#), [m("a", 1)]);
    assert_eq!(members(br#"{"a":true,"a":false}"#), [m("a", 0)]);
    assert!(root(br#"{"a":{"b":1},"a":null}"#, |r| r
        .member("a")
        .member("b")
        .as_i64()
        == 1));
    assert!(root(br#"{"a":"x","a":[1,2]}"#, |r| r.member("a").size() == 2));
    // the slots of the replaced collection stay used
    assert_eq!(outcome(br#"{"a":{"b":1,"c":2},"a":{"d":3}}"#), (Ok(()), 64));
    assert_eq!(outcome(br#"{"a":[1,2,3],"a":4}"#), (Ok(()), 64));
    let leak = |first: usize, second: usize| {
        format!("{{\"a\":{},\"a\":{}}}", array(first, ""), array(second, ""))
    };
    assert_eq!(outcome(leak(15, 16).as_bytes()), (Ok(()), 512));
    assert_eq!(outcome(leak(15, 17).as_bytes()), (Err(E::NoMemory), 512));
    // a duplicate takes no slot, so it fits a full object
    let many = format!("{{{}\"a\":2}}", "\"a\":1,".repeat(1000));
    assert_eq!(outcome(many.as_bytes()), (Ok(()), 16));
}

#[test]
fn iteration_keeps_the_document_order() {
    root(br#"{"o":{"z":1,"y":2},"a":[3,"x",[],{}]}"#, |r| {
        let o = r.as_object().expect("an object");
        let keys: Vec<&[u8]> = o.iter().map(|(k, _)| k).collect();
        assert_eq!(keys, [&b"o"[..], b"a"]);
        let inner: Vec<(&[u8], i64)> = o
            .get(b"o")
            .as_object()
            .expect("o")
            .iter()
            .map(|(k, v)| (k, v.as_i64()))
            .collect();
        assert_eq!(inner, [(&b"z"[..], 1), (&b"y"[..], 2)]);
        let a = o.get(b"a").as_array().expect("an array");
        assert_eq!(a.size(), 4);
        let kinds: Vec<(bool, bool, bool, bool)> = a
            .into_iter()
            .map(|v| (v.is_i64(), v.is_str(), v.is_array(), v.is_object()))
            .collect();
        assert_eq!(
            kinds,
            [
                (true, false, false, false),
                (false, true, false, false),
                (false, false, true, false),
                (false, false, false, true)
            ]
        );
        assert_eq!(a.iter().count(), 4);
    });
}

// ---------------------------------------------------------------- numbers

#[test]
fn integers_are_unsigned_or_signed_up_to_64_bits() {
    assert_eq!(number("0"), Some(Number::Unsigned(0)));
    assert_eq!(number("+007"), Some(Number::Unsigned(7)));
    assert_eq!(number("-0"), Some(Number::Signed(0)));
    assert_eq!(number("-12"), Some(Number::Signed(-12)));
    assert_eq!(
        number("18446744073709551615"),
        Some(Number::Unsigned(u64::MAX))
    );
    assert_eq!(
        number("-9223372036854775808"),
        Some(Number::Signed(i64::MIN))
    );
    let f = |bits: u64| Some(Number::Float(f64::from_bits(bits)));
    assert_eq!(number("-9223372036854775809"), f(0xC3DF_FFFF_FFFF_FFFA));
    assert_eq!(number("18446744073709551620"), f(0x43EF_FFFF_FFFF_FFFF));
    // overflow at the last digit after the multiplication: ten times too large
    assert_eq!(number("18446744073709551616"), f(0x4424_0000_0000_0000));
    assert_eq!(number("99999999999999999999"), f(0x4415_AF1D_78B5_8C3A));
    for text in [
        "", "-", "+", "--1", "+-1", "1-2", "e5", "x", "1x", "0x10", "1.5.5", "1e5e5", "1e5.5",
    ] {
        assert_eq!(number(text), None, "{text}");
    }
}

#[test]
fn doubles_come_from_the_library_algorithm() {
    assert_eq!(float("0.1"), 0x3FB9_9999_9999_999A);
    assert_eq!(float("43.5"), 43.5f64.to_bits());
    assert_eq!(float("-0.0"), (-0f64).to_bits());
    assert_eq!(float("."), 0);
    assert_eq!(float("-."), (-0f64).to_bits());
    assert_eq!(float(".e"), 0);
    assert_eq!(float("1e"), 1f64.to_bits());
    assert_eq!(float("1e+"), 1f64.to_bits());
    assert_eq!(float("5."), 5f64.to_bits());
    assert_eq!(float("1E5"), 1e5f64.to_bits());
    assert_eq!(float("1e-5"), 0x3EE4_F8B5_88E3_68F1);
    // digits beyond the 52-bit mantissa are dropped
    assert_eq!(
        float("4503599627370495.5"),
        4_503_599_627_370_495f64.to_bits()
    );
    assert_eq!(
        float("4503599627370496.5"),
        4_503_599_627_370_490f64.to_bits()
    );
    assert_eq!(float("450359962737049.5"), 450_359_962_737_049f64.to_bits());
    assert_eq!(float("9223372036854775808.0"), 0x43DF_FFFF_FFFF_FFFA);
    // the exponent check while it is read: 308 including the mantissa shift
    assert_eq!(float("1e308"), 0x7FE1_CCF3_85EB_C8A0);
    assert_eq!(float("1e309"), f64::INFINITY.to_bits());
    assert_eq!(float("-1e999"), f64::NEG_INFINITY.to_bits());
    assert_eq!(float("0e400"), f64::INFINITY.to_bits());
    assert_eq!(float("-1e-400"), (-0f64).to_bits());
    assert_eq!(float("0.00001e312"), 0x7FAC_7B1F_3CAC_7433);
    assert_eq!(float("100e306"), 0x7FE1_CCF3_85EB_C8A0);
}

#[test]
fn long_number_strings_reach_the_ends_of_the_tables() {
    // 600 digits: 10^584 needs a power beyond 1e256
    let big = format!("1{}", "0".repeat(599));
    assert_eq!(number(&big).map(|n| n.to_f64().to_bits()), Some(NAN_BITS));
    assert_eq!(
        number(&format!("-{big}")).map(|n| n.to_f64().to_bits()),
        Some(NAN_BITS | 1 << 63)
    );
    // the int16 offset wraps like GCC: 65536 fraction digits are an offset of 0
    let wrapped = format!("0.{}1", "0".repeat(65535));
    assert_eq!(number(&wrapped), Some(Number::Float(1.0)));
    assert_eq!(make_float(1.0, 0), 1.0);
    assert_eq!(make_float(1.0, 1), 10.0);
    assert_eq!(make_float(1.0, -1).to_bits(), 0x3FB9_9999_9999_999A);
    assert_eq!(make_float(1.0, 511), f64::INFINITY);
    assert_eq!(make_float(1.0, -511), 0.0);
    assert_eq!(make_float(1.0, 512).to_bits(), NAN_BITS);
    assert_eq!(make_float(1.0, -512).to_bits(), NAN_BITS);
    // the factors in table order: 1e2 before 1e256
    let e256 = f64::from_bits(POSITIVE_POWERS[8]);
    assert_eq!(
        make_float(3.0, 256 + 2).to_bits(),
        (3.0 * 100.0 * e256).to_bits()
    );
}

// ---------------------------------------------------------------- the web server's use

/// `intField` of web_server.cpp over the API.
fn int_field(
    o: JsonObjectConst<'_>,
    key: &str,
    min: i64,
    max: i64,
    required: bool,
) -> Option<Option<i64>> {
    let v = o.get(key.as_bytes());
    if v.is_null() {
        return (!required).then_some(None);
    }
    if !v.is_i64() || v.is_bool() {
        return None;
    }
    let x = v.as_i64();
    (min..=max).contains(&x).then_some(Some(x))
}

#[test]
fn the_glue_reads_bodies_like_web_server_cpp() {
    // parseBody: the error name, or "object expected"
    let parse_body = |body: &[u8]| -> String {
        with(body, |r| match r {
            Err(e) => e.c_str().to_string(),
            Ok(root) if !root.is_object() => "object expected".to_string(),
            Ok(_) => "ok".to_string(),
        })
    };
    assert_eq!(parse_body(b"{"), "IncompleteInput");
    assert_eq!(parse_body(b"7"), "object expected");
    assert_eq!(parse_body(b"[]"), "object expected");
    assert_eq!(parse_body(object(33).as_bytes()), "NoMemory");
    assert_eq!(parse_body(br#"{"target":5}"#), "ok");

    root(
        br#"{"dir":"open","counts":10,"maxmA":20.0,"slot":true}"#,
        |r| {
            let o = r.as_object().expect("an object");
            // onlyKeys
            let known = |k: &[u8]| [&b"dir"[..], b"counts", b"maxmA"].contains(&k);
            assert!(!o.iter().all(|(k, _)| known(k)));
            assert_eq!(o.get(b"dir").as_str(), Some(&b"open"[..]));
            assert_eq!(int_field(o, "counts", 1, 10000, true), Some(Some(10)));
            assert_eq!(int_field(o, "maxmA", 5, 60, true), None); // a double is no integer
            assert_eq!(int_field(o, "slot", 0, 1, true), None); // nor a bool
            assert_eq!(int_field(o, "none", 0, 1, false), Some(None));
            assert_eq!(int_field(o, "none", 0, 1, true), None);
            assert_eq!(int_field(o, "counts", 11, 20, true), None);
            // targetField: any number, rounded by the caller
            assert!(
                o.get(b"maxmA").is_f64() && o.get(b"counts").is_f64() && !o.get(b"slot").is_f64()
            );
        },
    );
}
