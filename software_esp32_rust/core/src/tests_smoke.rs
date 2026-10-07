//! Port of test/native/test_smoke.cpp: the core links, the shared helpers and the JSON writer
//! work. The module tests are in `<module>/tests.rs`; these cases keep the C++ smoke test whole.
//! C++ cases that pass a null pointer have no Rust form (a slice is never null); they are named
//! in comments where they stood.

use crate::common::{
    build_hostname, elapsed_ms, format_ipv4, format_one_wire_id, is_printable_text, is_safe_name,
    parse_int, parse_ipv4, parse_one_wire_id, parse_uint, time_reached, utf8_sequence_length,
    Backoff,
};
use crate::config::{set_defaults, Config};
use crate::json_writer::JsonWriter;
use crate::test_support::assert_text;
use crate::version::{
    compare_version, firmware_version, format_version, is_revamped, min_stm_version, parse_version,
};

#[test]
fn version_parses_legacy_and_revamped_stm_strings() {
    let v = parse_version(b"1.4.9_Dev_C2");
    assert!(v.valid);
    assert_eq!((v.major, v.minor, v.patch), (1, 4, 9));
    assert_text(&v.suffix, "_Dev");
    assert_text(&v.hw, "C2");
    assert!(!is_revamped(&v));

    let s = b"2.0.0-revamped_C2";
    let r = parse_version(s);
    assert!(r.valid);
    assert!(is_revamped(&r));
    assert_text(&r.hw, "C2");
    assert!(compare_version(&r, &v) > 0);

    let mut out = [0u8; 32];
    let n = format_version(&r, &mut out);
    assert_eq!(n, s.len());
    assert_text(&out[..n], s);

    // C++ parseVersion() false and `bad.valid` false: the result's `valid`
    assert!(!parse_version(b"1.4").valid);
    assert!(!parse_version(b"1.4.9 C2").valid);
}

#[test]
fn firmware_version_carries_the_revamped_suffix_on_target_builds() {
    // Native builds have no VDM_VERSION flag.
    assert_eq!(firmware_version(), "0.0.0-native");
    assert_eq!(min_stm_version(), "1.4.0");
}

#[test]
fn strict_number_and_address_helpers() {
    // C++ `bool parseUint(..., out)` leaves `out` alone on failure: the None of the Option
    assert_eq!(parse_uint(b"100", 100), Some(100));
    assert_eq!(parse_uint(b"101", 100), None);
    assert_eq!(parse_uint(b"+1", 100), None);
    assert_eq!(parse_uint(b"", 100), None);

    assert_eq!(parse_int(b"-1270", -32768, 32767), Some(-1270));
    assert_eq!(parse_int(b"-", -5, 5), None);

    let ip = parse_ipv4(b"192.168.1.2");
    assert_eq!(ip, Some(0x0201_A8C0));
    let mut buf = [0u8; 16];
    let n = format_ipv4(0x0201_A8C0, &mut buf);
    assert_eq!(n, 11);
    assert_text(&buf[..n], "192.168.1.2");
    assert_eq!(parse_ipv4(b"192.168.1.256"), None);
    assert_eq!(parse_ipv4(b"1.2.3"), None);

    let id = parse_one_wire_id(b"28-84-37-94-97-FF-03-23").expect("a valid 1-Wire id");
    let mut text = [0u8; 24];
    let n = format_one_wire_id(&id, &mut text);
    assert_eq!(n, 23);
    assert_text(&text[..n], "28-84-37-94-97-ff-03-23");
    assert_eq!(elapsed_ms(5, 0xFFFF_FFFB), 10);
    assert!(time_reached(5, 0xFFFF_FFFB));
}

#[test]
fn backoff_exponential_capped_due_however_old_the_last_attempt_is() {
    let mut b = Backoff::new(5000, 60000);
    assert!(b.due(0));
    assert!(b.due(0x9000_0000)); // armed: due at any time
    b.on_failure(1000);
    assert!(!b.due(5999));
    assert!(b.due(6000));
    assert_eq!(b.delay_ms(), 10000);
    b.on_failure(6000);
    assert!(!b.due(15999));
    assert!(b.due(16000));
    b.on_failure(16000); // 20 s
    b.on_failure(36000); // 40 s
    assert_eq!(b.delay_ms(), 60000);
    b.on_failure(76000); // 60 s, capped from here on
    assert!(!b.due(135_999));
    assert!(b.due(136_000));
    b.on_failure(136_000);
    assert_eq!(b.delay_ms(), 60000);
    assert!(!b.due(195_999));
    assert!(b.due(196_000));

    // Success at boot, connection lost 30 days later: due at once (a deadline compared with
    // time_reached() would read as 20 days in the future).
    b.reset();
    assert_eq!(b.delay_ms(), 5000);
    let day30: u32 = 30 * 86_400 * 1000;
    assert!(day30 > 0x8000_0000);
    assert!(b.due(day30));
    assert!(!time_reached(day30, 0)); // the defect Backoff avoids
    b.on_failure(day30);
    assert!(!b.due(day30 + 4999));
    assert!(b.due(day30 + 5000));
    // Across the millis() wrap.
    b.reset();
    b.on_failure(0xFFFF_F000);
    assert!(!b.due(0xFFFF_FFFF));
    assert!(b.due(0x0000_0388)); // 0x1000 + 0x388 = 5000 ms later

    // Degenerate parameters.
    let mut z = Backoff::new(0, 0);
    z.on_failure(10);
    assert_eq!(z.delay_ms(), 1);
    assert!(!z.due(10));
    assert!(z.due(11));
    let mut odd = Backoff::new(7, 20);
    odd.on_failure(0);
    assert_eq!(odd.delay_ms(), 14);
    odd.on_failure(7);
    assert_eq!(odd.delay_ms(), 20);
    odd.on_failure(21);
    assert_eq!(odd.delay_ms(), 20);
    assert!(!odd.due(40));
    assert!(odd.due(41));
}

#[test]
fn utf8_text_and_the_name_policy() {
    // (bytes, avail, length): C++ utf8SequenceLength(s, avail) is the slice s[..avail]
    let seqs: [(&[u8], usize, usize); 27] = [
        (b"\xc2\xa0", 2, 2),         // U+00A0, first after the C1 block
        (b"\xc2\x9f", 2, 0),         // U+009F C1 control
        (b"\xc2\x80", 2, 0),         // U+0080 C1 control
        (b"\xc3\xa4", 2, 2),         // a-umlaut
        (b"\xc3\xa4", 1, 0),         // truncated by avail
        (b"\xc3a", 2, 0),            // bad continuation
        (b"\xc0\xaf", 2, 0),         // overlong '/'
        (b"\xc1\xbf", 2, 0),         // overlong
        (b"\xdf\xbf", 2, 2),         // U+07FF
        (b"\xe0\xa0\x80", 3, 3),     // U+0800
        (b"\xe0\x9f\xbf", 3, 0),     // overlong U+07FF
        (b"\xe2\x82\xac", 3, 3),     // euro sign
        (b"\xed\x9f\xbf", 3, 3),     // U+D7FF
        (b"\xed\xa0\x80", 3, 0),     // U+D800 surrogate
        (b"\xed\xbf\xbf", 3, 0),     // U+DFFF surrogate
        (b"\xee\x80\x80", 3, 3),     // U+E000
        (b"\xef\xbf\xbf", 3, 3),     // U+FFFF
        (b"\xf0\x90\x80\x80", 4, 4), // U+10000
        (b"\xf0\x8f\xbf\xbf", 4, 0), // overlong
        (b"\xf0\x9f\x98\x80", 4, 4), // emoji
        (b"\xf0\x9f\x98\x80", 3, 0), // truncated
        (b"\xf4\x8f\xbf\xbf", 4, 4), // U+10FFFF
        (b"\xf4\x90\x80\x80", 4, 0), // above U+10FFFF
        (b"\xf5\x80\x80\x80", 4, 0),
        (b"\xff", 1, 0),
        (b"\x80", 1, 0), // stray continuation
        (b"a", 1, 0),    // ASCII is not a sequence
    ];
    for (s, avail, len) in seqs {
        assert_eq!(
            utf8_sequence_length(&s[..avail]),
            len,
            "{}",
            s.escape_ascii()
        );
    }
    // C++ utf8SequenceLength(nullptr, 4) == 0: no Rust form
    assert_eq!(utf8_sequence_length(&b"\xc3\xa4"[..0]), 0);

    assert!(is_printable_text(b"")); // also the C++ (nullptr, 0)
                                     // C++ isPrintableText(nullptr, 1) == false: no Rust form
    assert!(is_printable_text(b" ~"));
    assert!(is_printable_text(b"K\xc3\xbcche \xe2\x82\xac"));
    assert!(!is_printable_text(b"a\x1f"));
    assert!(!is_printable_text(b"a\x7f"));
    assert!(!is_printable_text(b"a\xc3"));
    assert!(!is_printable_text(b"\xc3\xa4\xa4"));
    assert!(is_printable_text(&b"a\x01"[..1])); // only `len` bytes count

    assert!(is_safe_name(b"K\xc3\xbcche", 10, false));
    assert!(is_safe_name(b" Bad ", 10, false));
    assert!(is_safe_name(b"\xc2\xb0C", 8, false));
    assert!(is_safe_name(b"", 10, true));
    assert!(!is_safe_name(b"", 10, false));
    // C++ isSafeName(nullptr, 10, true) == false: no Rust form
    assert!(!is_safe_name(
        b"\xc3\xa4\xc3\xa4\xc3\xa4\xc3\xa4\xc3\xa4\xc3\xa4",
        11,
        false
    )); // 12 bytes
    assert!(is_safe_name(
        b"\xc3\xa4\xc3\xa4\xc3\xa4\xc3\xa4\xc3\xa4",
        10,
        false
    ));
    let bad: [&[u8]; 9] = [
        b"a+",
        b"#",
        b"a/b",
        b"q\"",
        b"b\\",
        b"a\tb",
        b"a\x7f",
        b"a\xc2\x85",
        b"a\xe4",
    ];
    for s in bad {
        assert!(!is_safe_name(s, 10, false), "{}", s.escape_ascii());
    }
}

#[test]
fn build_hostname_a_dhcp_host_name_from_the_station_name() {
    let cases: [(&[u8], &str); 12] = [
        (b"VdMot", "VdMot"),
        (b"VdMot_FBH-2", "VdMot_FBH-2"),
        (b"My Station", "My-Station"),
        (b"  Haus  Ost  ", "Haus-Ost"),
        (b"Fu\xc3\x9fboden", "Fu-boden"),
        (b"K\xc3\xbcche OG", "K-che-OG"),
        (b"a - b", "a-b"),
        (b"-x-", "x"),
        (b"--", "VdMot"),
        (b"\xc3\xa4\xc3\xb6", "VdMot"),
        (b"", "VdMot"),
        (b"a.b,c", "a-b-c"),
    ];
    let mut out = [0u8; 21];
    for (station, want) in cases {
        let n = build_hostname(station, &mut out);
        assert_eq!(n, want.len(), "{}", station.escape_ascii());
        assert_text(&out[..n], want);
    }
    // C++ buildHostname(nullptr, ..) == 5, "VdMot": the empty slice, above
    // Bounded by cap, never ends with '-'.
    let mut small = [0u8; 6];
    let n = build_hostname(b"abcd efgh", &mut small);
    assert_eq!(n, 4);
    assert_text(&small[..n], "abcd");
    let n = build_hostname(b"abcdefgh", &mut small);
    assert_eq!(n, 5);
    assert_text(&small[..n], "abcde");
    let mut tiny = *b"xxxx\0";
    assert_eq!(build_hostname(b"abc", &mut tiny), 0);
    // C++ tiny[0] == '\0': Rust writes no NUL, the 0 result says it
    assert_eq!(&tiny, b"xxxx\0");
    // C++ buildHostname("abc", nullptr, 8): no Rust form; capacity 0 is the empty output
    assert_eq!(build_hostname(b"abc", &mut tiny[..0]), 0);
}

#[test]
fn json_writer_escapes_nests_and_stays_bounded() {
    let mut buf = [0u8; 64];
    let mut jw = JsonWriter::new(&mut buf);
    jw.begin_object();
    jw.kv("name", "a\"b\\c\n");
    jw.key("t");
    jw.fixed(-5, 1);
    jw.key("list");
    jw.begin_array();
    jw.value(true);
    jw.null_value();
    jw.value(4_000_000_000u32);
    jw.end_array();
    jw.end_object();
    assert!(jw.complete());
    assert_text(
        jw.as_bytes(),
        "{\"name\":\"a\\\"b\\\\c\\n\",\"t\":-0.5,\"list\":[true,null,4000000000]}",
    );

    let mut tiny = [0u8; 8];
    let mut small = JsonWriter::new(&mut tiny);
    small.begin_array();
    small.value("too long for this buffer");
    assert!(!small.ok());
    assert_text(small.as_bytes(), "[");

    let mut misuse = JsonWriter::new(&mut buf);
    misuse.begin_object();
    misuse.value(1); // value without key
    assert!(!misuse.ok());
}

#[test]
fn config_defaults_are_the_documented_factory_values() {
    let mut c = Config::default();
    c.mqtt.port = 1;
    set_defaults(&mut c);
    assert_text(&c.station, "VdMot");
    assert_eq!(c.mqtt.port, 1883);
    assert_eq!(c.calib.day_mask, 9);
    assert_text(&c.time.tz_posix, "CET-1CEST,M3.5.0,M10.5.0/3");
}
