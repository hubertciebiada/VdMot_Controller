//! Port of test/native/test_web_guard__mut.cpp: case folding at 'Z', names starting with a
//! digit, the port suffix digits, one-char and empty hostnames, near-miss scheme prefixes, the
//! refusal text limits.

use super::*;

fn named(hostname: &[u8]) -> HostPolicy<'_> {
    HostPolicy {
        local_ip: 0x3301_A8C0,
        hostname,
        ..HostPolicy::default()
    }
}

fn host_ok(h: &str, p: &HostPolicy<'_>) -> bool {
    host_allowed(h.as_bytes(), p)
}

fn origin_ok(o: &str, p: &HostPolicy<'_>) -> bool {
    origin_allowed(Some(o.as_bytes()), p)
}

#[test]
fn web_guard_z_folds_to_z() {
    assert!(host_ok("VDMZ", &named(b"vdmz")));
    assert!(host_ok("vdmz", &named(b"VDMZ")));
}

#[test]
fn web_guard_a_name_that_starts_with_a_digit_is_a_name_not_an_address() {
    assert!(host_ok("1vdm", &named(b"1vdm")));
    assert!(!host_ok("1vdm", &named(b"vdm")));
}

#[test]
fn web_guard_every_char_of_the_port_suffix_must_be_a_digit_0_and_9_included() {
    let p = named(b"vdm");
    assert!(!host_ok("vdm:x1", &p));
    assert!(!host_ok("vdm:1x", &p));
    assert!(host_ok("vdm:9", &p));
    assert!(host_ok("vdm:0", &p));
    assert!(host_ok("vdm:90", &p));
}

#[test]
fn web_guard_a_one_char_hostname_matches_an_empty_one_never_matches_local() {
    assert!(host_ok("v", &named(b"v")));
    assert!(host_ok("v.local", &named(b"v")));
    assert!(!host_ok(".local", &named(b"")));
    assert!(!host_ok("x.local", &named(b"")));
}

#[test]
fn web_guard_a_scheme_missing_one_slash_is_not_a_scheme() {
    let p = named(b"vdm");
    assert!(!origin_ok("http:/Xvdm", &p));
    assert!(!origin_ok("https:/Xvdm", &p));
    assert!(origin_ok("http://vdm", &p));
    assert!(origin_ok("https://vdm", &p));
}

#[test]
fn web_guard_the_bad_host_text_holds_the_longest_address() {
    let p = HostPolicy {
        local_ip: 0xFFFF_FFFF,
        ..named(b"vdm")
    };
    let r = GuardRequest {
        host: b"x",
        ..GuardRequest::default()
    };
    let mut out = [0u8; 128];
    let n = guard_detail(GuardVerdict::BadHost, &r, &p, &mut out);
    assert_eq!(
        &out[..n],
        b"x: use 255.255.255.255 or add the name to web.allowedHosts"
    );
}

#[test]
fn web_guard_a_text_of_exactly_cap_chars_is_cut_to_cap_minus_1() {
    let r = GuardRequest::default();
    let p = named(b"vdm");
    let mut out = [0u8; 16];
    // "X-VdMot: 1" is 10 chars
    assert_eq!(
        guard_detail(GuardVerdict::MissingHeader, &r, &p, &mut out[..11]),
        10
    );
    assert_eq!(
        guard_detail(GuardVerdict::MissingHeader, &r, &p, &mut out[..10]),
        9
    );
    assert_eq!(&out[..9], b"X-VdMot: ");
}
