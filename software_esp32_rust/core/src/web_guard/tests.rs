//! Port of test/native/test_web_guard.cpp: Host/Origin rules, the request guard matrix, the
//! refusal texts and the repeat limiter.
//!
//! The C++ null pointers: a null host, hostname or allow list is the empty slice (the C++
//! treats them alike); a null Origin, X-VdMot or Content-Type is None (an absent header). A
//! GuardVerdict cannot hold 9: `from_raw(9) == None` stands for the C++ fallbacks. Rust writes
//! no NUL: the C++ checks of `strlen(buf)` and of `out[0] == '\0'` are the returned length and
//! the bytes after it.

use super::*;
use std::string::String;
use std::vec::Vec;

const LOCAL: u32 = 0x3301_A8C0; // 192.168.1.51
const IFACE: u32 = 0x0A00_000A; // 10.0.0.10

fn policy(allowed: &[u8]) -> HostPolicy<'_> {
    HostPolicy {
        local_ip: LOCAL,
        iface_ip: IFACE,
        hostname: b"vdmot-east",
        allowed,
    }
}

fn host(h: &str) -> bool {
    host_allowed(h.as_bytes(), &policy(b""))
}

fn host_with(h: &str, p: &HostPolicy<'_>) -> bool {
    host_allowed(h.as_bytes(), p)
}

fn origin(o: &str, p: &HostPolicy<'_>) -> bool {
    origin_allowed(Some(o.as_bytes()), p)
}

fn request(m: HttpMethod, s: GuardScope) -> GuardRequest<'static> {
    GuardRequest {
        method: m,
        scope: s,
        host: b"192.168.1.51",
        ..GuardRequest::default()
    }
}

fn set_type<'a>(r: &mut GuardRequest<'a>, v: Option<&'a [u8]>) {
    r.content_type = v;
    r.has_body = true;
}

/// The detail text in a buffer of `cap` bytes (C++ cap, NUL included).
fn refusal_text_cap(
    v: GuardVerdict,
    r: &GuardRequest<'_>,
    p: &HostPolicy<'_>,
    cap: usize,
) -> String {
    let mut buf = [0u8; 160];
    let n = guard_detail(v, r, p, &mut buf[..cap]);
    // C++ n == strlen(buf): the text is the first n bytes, nothing is written after it
    assert!(buf[n..].iter().all(|&c| c == 0));
    String::from_utf8(buf[..n].to_vec()).unwrap()
}

fn refusal_text(v: GuardVerdict, r: &GuardRequest<'_>, p: &HostPolicy<'_>) -> String {
    refusal_text_cap(v, r, p, 160)
}

#[test]
fn host_allowed_own_addresses_and_names() {
    assert!(host("192.168.1.51"));
    assert!(host("10.0.0.10"));
    assert!(host("192.168.1.51:80"));
    assert!(host("192.168.1.51:65535"));
    assert!(host("192.168.1.51:1"));
    assert!(host("192.168.1.51."));
    assert!(host("vdmot-east"));
    assert!(host("VDMOT-EAST.local"));
    assert!(host("vdmot-east.local."));
    assert!(host("vdmot-east.LOCAL:8080"));
    assert!(host("vdmot-east:80"));
}

#[test]
fn host_allowed_foreign_hosts_fail() {
    assert!(!host("vdmot-east.evil.com"));
    assert!(!host("vdmot-east.localx"));
    assert!(!host("vdmot-eas"));
    assert!(!host("xvdmot-east"));
    assert!(!host("vdmot-east.loca"));
    assert!(!host("192.168.1.52"));
    assert!(!host("[::1]:80"));
    assert!(!host("[::1]"));
    assert!(!host("192.168.1.51:123456"));
    assert!(!host("192.168.1.51:"));
    assert!(!host("192.168.1.51:8a"));
    assert!(!host("192.168.1.51:80:80"));
    assert!(!host("192.168.1.300"));
    assert!(!host("."));
    assert!(!host(":80"));
    assert!(!host("vdmot-east.."));
    let vd = HostPolicy {
        hostname: b"VdMot",
        ..policy(b"")
    };
    assert!(!host_with("vdmot.evil.com", &vd));
    assert!(host_with("vdmot.local", &vd));
}

#[test]
fn host_allowed_absent_and_empty_hosts_are_fine() {
    // C++ hostAllowed(nullptr, 0): no Rust form, the empty slice.
    assert!(host_allowed(b"", &policy(b"")));
    assert!(host_allowed(&b"x"[..0], &policy(b"")));
}

#[test]
fn host_allowed_address_0_never_matches() {
    let p = HostPolicy {
        local_ip: 0,
        iface_ip: 0,
        ..policy(b"")
    };
    assert!(!host_with("0.0.0.0", &p));
    assert!(!host_with("0.0.0.0", &policy(b"0.0.0.0")));
    assert!(!host_with("192.168.1.51", &p));
    assert!(host_with("vdmot-east", &p));
}

#[test]
fn host_allowed_the_allow_list_takes_names_and_addresses() {
    let p = policy(b" vdmot.lan , 192.168.2.7,Heating.Example.org ");
    assert!(host_with("vdmot.lan", &p));
    assert!(host_with("VDMOT.LAN:81", &p));
    assert!(host_with("192.168.2.7", &p));
    assert!(host_with("heating.example.org", &p));
    assert!(!host_with("192.168.2.8", &p));
    assert!(!host_with("vdmot", &p));
    assert!(!host_with("vdmot.lan.local", &p));
    assert!(!host_with("heating.example.or", &p));
    // a name entry never matches an address and the other way round
    assert!(!host_with("192.168.2.7", &policy(b"vdmot.lan")));
    assert!(!host_with("x", &policy(b"1.2.3.4")));
    assert!(!host_with("1.2.3.4", &policy(b"1.2.3.4x")));
    assert!(host_with("1.2.3.4", &policy(b",1.2.3.4")));
    assert!(host_with("b", &policy(b"a,,b")));
    assert!(!host_with("b", &policy(b"a,b.")));
    // C++ a null allow list and a null hostname: no Rust form, the empty C strings.
    let none = HostPolicy {
        allowed: b"",
        hostname: b"",
        ..policy(b"")
    };
    assert!(!host_with("vdmot-east", &none));
    assert!(host_with("192.168.1.51", &none));
}

#[test]
fn origin_allowed_rules() {
    let p = policy(b"vdmot.lan");
    assert!(origin_allowed(None, &p));
    assert!(origin("http://192.168.1.51", &p));
    assert!(origin("https://vdmot.lan:8443", &p));
    assert!(origin("HTTP://vdmot-east.local", &p));
    assert!(origin("HtTpS://10.0.0.10", &p));
    assert!(!origin("null", &p));
    assert!(!origin("http://evil", &p));
    assert!(!origin("ftp://192.168.1.51", &p));
    assert!(!origin("http://192.168.1.51/x", &p));
    assert!(!origin("http://192.168.1.51/", &p));
    assert!(!origin("http://", &p));
    assert!(!origin("https://", &p));
    assert!(!origin("http:/192.168.1.51", &p));
    assert!(!origin("", &p));
    assert!(!origin("192.168.1.51", &p));
}

#[test]
fn check_request_static_scope_is_never_checked() {
    let mut r = request(HttpMethod::Post, GuardScope::Static);
    r.host = b"evil.com";
    set_type(&mut r, Some(b"text/plain"));
    assert_eq!(check_request(&r, &policy(b"")), GuardVerdict::Allow);
}

#[test]
fn check_request_the_x_vdmot_marker_for_api_writes() {
    let p = policy(b"");
    assert_eq!(
        check_request(&request(HttpMethod::Get, GuardScope::Api), &p),
        GuardVerdict::Allow
    );
    assert_eq!(
        check_request(&request(HttpMethod::Other, GuardScope::Api), &p),
        GuardVerdict::Allow
    );
    let mut r = request(HttpMethod::Post, GuardScope::Api);
    assert_eq!(check_request(&r, &p), GuardVerdict::MissingHeader);
    r.marker = Some(b"0");
    assert_eq!(check_request(&r, &p), GuardVerdict::MissingHeader);
    r.marker = Some(b"11");
    assert_eq!(check_request(&r, &p), GuardVerdict::MissingHeader);
    r.marker = Some(b"");
    assert_eq!(check_request(&r, &p), GuardVerdict::MissingHeader);
    r.marker = Some(b"1");
    assert_eq!(check_request(&r, &p), GuardVerdict::Allow);
    r.marker = Some(&b"12"[..1]); // only the bytes of the value count (C++ markerLen 1)
    assert_eq!(check_request(&r, &p), GuardVerdict::Allow);
    let mut d = request(HttpMethod::Delete, GuardScope::Api);
    assert_eq!(check_request(&d, &p), GuardVerdict::MissingHeader);
    d.marker = Some(b"1");
    assert_eq!(check_request(&d, &p), GuardVerdict::Allow);
}

#[test]
fn check_request_json_content_type_for_bodies() {
    let p = policy(b"");
    let mut r = request(HttpMethod::Post, GuardScope::Api);
    r.marker = Some(b"1");
    assert_eq!(check_request(&r, &p), GuardVerdict::Allow); // no body, no type
    set_type(&mut r, Some(b"text/plain"));
    assert_eq!(check_request(&r, &p), GuardVerdict::BadContentType);
    set_type(&mut r, Some(b"APPLICATION/JSON"));
    assert_eq!(check_request(&r, &p), GuardVerdict::Allow);
    set_type(&mut r, Some(b"application/jsonx"));
    assert_eq!(check_request(&r, &p), GuardVerdict::BadContentType);
    set_type(&mut r, Some(b"application/jso"));
    assert_eq!(check_request(&r, &p), GuardVerdict::BadContentType);
    set_type(&mut r, None);
    assert_eq!(check_request(&r, &p), GuardVerdict::BadContentType);
    r.has_body = false;
    assert_eq!(check_request(&r, &p), GuardVerdict::Allow);
    // uploads keep their own multipart check
    let mut u = request(HttpMethod::Post, GuardScope::Api);
    u.upload = true;
    u.marker = Some(b"1");
    set_type(&mut u, Some(b"multipart/form-data"));
    assert_eq!(check_request(&u, &p), GuardVerdict::Allow);
    u.marker = None;
    assert_eq!(check_request(&u, &p), GuardVerdict::MissingHeader);
    // a DELETE with a body is not checked for its type
    let mut d = request(HttpMethod::Delete, GuardScope::Api);
    d.marker = Some(b"1");
    set_type(&mut d, Some(b"text/plain"));
    assert_eq!(check_request(&d, &p), GuardVerdict::Allow);
}

#[test]
fn check_request_legacy_scopes() {
    let p = policy(b"");
    let mut w = request(HttpMethod::Post, GuardScope::LegacyWrite);
    set_type(&mut w, Some(b"application/json"));
    assert_eq!(check_request(&w, &p), GuardVerdict::Allow); // no marker needed
    set_type(&mut w, Some(b"text/plain"));
    assert_eq!(check_request(&w, &p), GuardVerdict::BadContentType);
    let mut rd = request(HttpMethod::Get, GuardScope::LegacyRead);
    assert_eq!(check_request(&rd, &p), GuardVerdict::Allow);
    rd.host = b"evil.com";
    assert_eq!(check_request(&rd, &p), GuardVerdict::BadHost);
}

#[test]
fn check_request_order_host_origin_marker_content_type() {
    let p = policy(b"");
    let mut r = request(HttpMethod::Post, GuardScope::Api);
    set_type(&mut r, Some(b"text/plain"));
    r.origin = Some(b"http://evil");
    r.host = b"evil.com";
    assert_eq!(check_request(&r, &p), GuardVerdict::BadHost); // bad host wins over everything
    r.host = b"192.168.1.51";
    assert_eq!(check_request(&r, &p), GuardVerdict::BadOrigin);
    r.origin = Some(b"http://192.168.1.51");
    assert_eq!(check_request(&r, &p), GuardVerdict::MissingHeader);
    r.marker = Some(b"1");
    assert_eq!(check_request(&r, &p), GuardVerdict::BadContentType);
    // bad origin wins over a bad content type
    r.origin = Some(b"null");
    assert_eq!(check_request(&r, &p), GuardVerdict::BadOrigin);
}

#[test]
fn guard_codes_and_statuses() {
    assert_eq!(guard_error_code(GuardVerdict::Allow), "");
    assert_eq!(guard_error_code(GuardVerdict::BadHost), "host_not_allowed");
    assert_eq!(
        guard_error_code(GuardVerdict::BadOrigin),
        "origin_not_allowed"
    );
    assert_eq!(
        guard_error_code(GuardVerdict::MissingHeader),
        "header_required"
    );
    assert_eq!(
        guard_error_code(GuardVerdict::BadContentType),
        "unsupported_media_type"
    );
    // C++ guardErrorCode(GuardVerdict(9)) == "" and guardHttpStatus(GuardVerdict(9)) == 200: no
    // Rust form, a GuardVerdict cannot hold 9.
    assert_eq!(GuardVerdict::from_raw(9), None);
    assert_eq!(guard_http_status(GuardVerdict::Allow), 200);
    assert_eq!(guard_http_status(GuardVerdict::BadHost), 403);
    assert_eq!(guard_http_status(GuardVerdict::BadOrigin), 403);
    assert_eq!(guard_http_status(GuardVerdict::MissingHeader), 403);
    assert_eq!(guard_http_status(GuardVerdict::BadContentType), 415);
    assert_eq!(GuardVerdict::BadHost as u8, 1);
    assert_eq!(GuardVerdict::BadOrigin as u8, 2);
    assert_eq!(GuardVerdict::MissingHeader as u8, 3);
    assert_eq!(GuardVerdict::BadContentType as u8, 4);
    // the C++ numbers of the scopes and verdicts
    let verdicts = [
        GuardVerdict::Allow,
        GuardVerdict::BadHost,
        GuardVerdict::BadOrigin,
        GuardVerdict::MissingHeader,
        GuardVerdict::BadContentType,
    ];
    for (v, verdict) in (0u8..).zip(verdicts) {
        assert_eq!(GuardVerdict::from_raw(v), Some(verdict));
    }
    assert_eq!(GuardVerdict::from_raw(5), None);
    let scopes = [
        GuardScope::Static,
        GuardScope::Api,
        GuardScope::LegacyRead,
        GuardScope::LegacyWrite,
    ];
    for (v, scope) in (0u8..).zip(scopes) {
        assert_eq!(scope as u8, v);
        assert_eq!(GuardScope::from_raw(v), Some(scope));
    }
    assert_eq!(GuardScope::from_raw(4), None);
}

#[test]
fn guard_detail_texts() {
    let mut r = request(HttpMethod::Post, GuardScope::Api);
    r.host = b"evil.com:80";
    r.origin = Some(b"http://evil");
    let mut p = policy(b"");
    assert_eq!(
        refusal_text(GuardVerdict::BadHost, &r, &p),
        "evil.com:80: use 192.168.1.51 or add the name to web.allowedHosts"
    );
    p.local_ip = 0;
    assert_eq!(
        refusal_text(GuardVerdict::BadHost, &r, &p),
        "evil.com:80: use 10.0.0.10 or add the name to web.allowedHosts"
    );
    assert_eq!(refusal_text(GuardVerdict::BadOrigin, &r, &p), "http://evil");
    assert_eq!(
        refusal_text(GuardVerdict::MissingHeader, &r, &p),
        "X-VdMot: 1"
    );
    assert_eq!(
        refusal_text(GuardVerdict::BadContentType, &r, &p),
        "application/json required"
    );
    assert_eq!(refusal_text(GuardVerdict::Allow, &r, &p), "");
    // at most 48 characters of the host or origin are echoed
    let long_host = "h".repeat(60);
    r.host = long_host.as_bytes();
    assert_eq!(
        refusal_text(GuardVerdict::BadHost, &r, &p),
        "h".repeat(48) + ": use 10.0.0.10 or add the name to web.allowedHosts"
    );
    let long_origin = String::from("http://") + &"o".repeat(50);
    r.origin = Some(long_origin.as_bytes());
    assert_eq!(
        refusal_text(GuardVerdict::BadOrigin, &r, &p),
        long_origin[..48]
    );
    r.origin = None;
    assert_eq!(refusal_text(GuardVerdict::BadOrigin, &r, &p), "");
    // C++ a null host: the empty one
    r.host = b"";
    assert_eq!(
        refusal_text(GuardVerdict::BadHost, &r, &p),
        ": use 10.0.0.10 or add the name to web.allowedHosts"
    );
    // truncation to the buffer
    assert_eq!(
        refusal_text_cap(GuardVerdict::MissingHeader, &r, &p, 5),
        "X-Vd"
    );
    let mut one = *b"x";
    // C++ one[0] == '\0' after it: the 0 result, Rust writes no NUL
    assert_eq!(
        guard_detail(GuardVerdict::MissingHeader, &r, &p, &mut one),
        0
    );
    // C++ a null output: no Rust form. A buffer of 0 bytes:
    assert_eq!(
        guard_detail(GuardVerdict::MissingHeader, &r, &p, &mut one[..0]),
        0
    );
    assert_eq!(&one, b"x");
}

#[test]
fn repeat_limiter_once_per_key_and_interval() {
    let mut l = RepeatLimiter::default();
    assert!(l.allow(1, 1000));
    assert!(!l.allow(1, 1000));
    assert!(!l.allow(1, 60999));
    assert!(l.allow(2, 1500)); // keys are independent
    assert!(l.allow(1, 61000));
    assert!(!l.allow(1, 61001));
    assert!(l.allow(0, 0));
    assert!(l.allow(7, 0));
    assert!(!l.allow(8, 0));
    assert!(!l.allow(255, 0));
}

#[test]
fn repeat_limiter_custom_interval_and_millis_wrap() {
    let mut l = RepeatLimiter::new(100);
    assert!(l.allow(3, 0xFFFF_FFC0));
    assert!(!l.allow(3, 0x0000_0023)); // 99 ms later, across the wrap
    assert!(l.allow(3, 0x0000_0024));
    assert_eq!(RepeatLimiter::KEYS, 8);
}

// ---------------------------------------------------------------- Rust-only checks

#[test]
fn guard_detail_echo_ends_at_a_nul_like_printf_precision() {
    // C++ "%.*s" stops at a NUL inside the first 48 bytes of the host or origin
    let mut r = request(HttpMethod::Post, GuardScope::Api);
    r.host = b"ev\0il.com";
    r.origin = Some(b"http://a\0b");
    let p = policy(b"");
    assert_eq!(
        refusal_text(GuardVerdict::BadHost, &r, &p),
        "ev: use 192.168.1.51 or add the name to web.allowedHosts"
    );
    assert_eq!(refusal_text(GuardVerdict::BadOrigin, &r, &p), "http://a");
    // a NUL after the 48 echoed bytes is not reached
    let mut long: Vec<u8> = std::vec![b'h'; 48];
    long.extend_from_slice(b"\0tail");
    r.host = &long;
    assert_eq!(
        refusal_text(GuardVerdict::BadHost, &r, &p),
        "h".repeat(48) + ": use 192.168.1.51 or add the name to web.allowedHosts"
    );
}

#[test]
fn host_policy_c_strings_end_at_their_nul() {
    // the hostname and the allow list are C strings: a NUL-padded array works as in C++
    let p = HostPolicy {
        local_ip: LOCAL,
        iface_ip: IFACE,
        hostname: b"vdmot-east\0\0\0\0",
        allowed: b"vdmot.lan\0junk,evil.com",
    };
    assert!(host_with("vdmot-east", &p));
    assert!(host_with("vdmot-east.local", &p));
    assert!(host_with("vdmot.lan", &p));
    assert!(!host_with("evil.com", &p));
    assert!(!host_with("vdmot.lan\0junk", &p));
}
