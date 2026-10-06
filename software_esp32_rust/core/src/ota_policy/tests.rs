//! Port of test/native/test_ota_policy.cpp: OtaValidator, normalize_md5.

use super::*;
use OtaValidatorDecision as D;

/// Self-checks ok every 10 s from `from` to `to` (inclusive) with update() every second.
fn run_healthy(v: &mut OtaValidator, from: u32, to: u32, net: bool, link: bool) -> D {
    let mut last = D::Wait;
    for i in 0..=to.wrapping_sub(from) / 1000 {
        let t = from.wrapping_add(i * 1000);
        if v.self_check_due(true, t) {
            v.on_self_check(true, t);
        }
        last = v.update(net, link, t);
        if last != D::Wait {
            return last;
        }
    }
    last
}

#[test]
fn not_pending_not_pending_forever() {
    let mut v = OtaValidator::default();
    assert_eq!(v.update(true, true, 0), D::NotPending);
    v.begin(false, true, 0);
    assert_eq!(v.update(true, true, 0), D::NotPending);
    assert_eq!(v.update(false, false, 4_000_000_000), D::NotPending);
    assert!(!v.pending());
    assert!(v.stm_required());
    assert!(!v.self_check_due(true, 0));
    assert_eq!(v.remaining_ms(0), 0);
    assert_eq!(v.healthy_for_ms(0), 0);
    assert_eq!(v.missing(), 0);
}

#[test]
fn self_check_due_at_once_then_every_10_s_only_with_the_web_server() {
    let mut v = OtaValidator::default();
    v.begin(true, false, 0);
    assert!(!v.self_check_due(false, 0));
    assert!(v.self_check_due(true, 0));
    assert!(v.self_check_due(true, 5000));
    v.on_self_check(true, 0);
    assert!(!v.self_check_due(true, 9999));
    assert!(v.self_check_due(true, 10_000));
    assert!(!v.self_check_due(false, 10_000));
    v.on_self_check(false, 10_000);
    assert!(!v.self_check_due(true, 19_999));
    assert!(v.self_check_due(true, 20_000));
}

#[test]
fn net_and_http_stm_not_required_valid_at_exactly_120_s() {
    let mut v = OtaValidator::default();
    v.begin(true, false, 0);
    assert!(v.pending());
    assert!(!v.stm_required());
    assert_eq!(v.remaining_ms(0), 900_000);
    assert_eq!(run_healthy(&mut v, 0, 119_000, true, false), D::Wait);
    assert_eq!(v.healthy_for_ms(119_000), 119_000);
    assert_eq!(v.healthy_for_ms(119_999), 119_999);
    assert_eq!(v.remaining_ms(119_000), 781_000);
    assert_eq!(v.missing(), 0);
    assert_eq!(v.update(true, false, 119_999), D::Wait);
    assert_eq!(v.update(true, false, 120_000), D::MarkValid);
    assert!(!v.pending());
    assert_eq!(v.update(true, false, 120_001), D::NotPending);
    assert_eq!(v.remaining_ms(120_001), 0);
    assert_eq!(v.healthy_for_ms(120_001), 0);
}

#[test]
fn stm_required_and_link_down_rollback_at_900_s_missing_stm() {
    let mut v = OtaValidator::default();
    v.begin(true, true, 0);
    assert_eq!(run_healthy(&mut v, 0, 899_000, true, false), D::Wait);
    assert_eq!(v.missing(), OtaValidator::CHECK_STM);
    assert_eq!(v.healthy_for_ms(899_000), 0);
    assert_eq!(v.remaining_ms(899_999), 1);
    assert_eq!(v.update(true, false, 899_999), D::Wait);
    assert_eq!(v.update(true, false, 900_000), D::Rollback);
    assert_eq!(v.missing(), OtaValidator::CHECK_STM);
    assert_eq!(v.update(true, true, 900_001), D::NotPending);
}

#[test]
fn stm_required_and_link_up_valid() {
    let mut v = OtaValidator::default();
    v.begin(true, true, 1000);
    assert_eq!(run_healthy(&mut v, 1000, 121_000, true, true), D::MarkValid);
}

#[test]
fn the_missing_bits() {
    let mut v = OtaValidator::default();
    v.begin(true, true, 0);
    assert_eq!(v.update(false, false, 0), D::Wait);
    assert_eq!(
        v.missing(),
        OtaValidator::CHECK_NET | OtaValidator::CHECK_HTTP | OtaValidator::CHECK_STM
    );
    v.on_self_check(true, 0);
    assert_eq!(v.update(false, true, 1000), D::Wait);
    assert_eq!(v.missing(), OtaValidator::CHECK_NET);
    assert_eq!(v.update(true, true, 2000), D::Wait);
    assert_eq!(v.missing(), 0);
    assert_eq!(OtaValidator::CHECK_NET, 1);
    assert_eq!(OtaValidator::CHECK_HTTP, 2);
    assert_eq!(OtaValidator::CHECK_STM, 4);
}

#[test]
fn a_self_check_result_is_fresh_for_30_s() {
    let mut v = OtaValidator::default();
    v.begin(true, false, 0);
    v.on_self_check(true, 0);
    assert!(v.http_ok(29_999));
    assert!(!v.http_ok(30_000));
    assert_eq!(v.update(true, false, 29_999), D::Wait);
    assert_eq!(v.missing(), 0);
    assert_eq!(v.update(true, false, 30_000), D::Wait);
    assert_eq!(v.missing(), OtaValidator::CHECK_HTTP);
    assert_eq!(v.healthy_for_ms(30_000), 0);
}

#[test]
fn a_failed_self_check_ends_the_healthy_period_at_once() {
    let mut v = OtaValidator::default();
    v.begin(true, false, 0);
    assert_eq!(run_healthy(&mut v, 0, 60_000, true, false), D::Wait);
    assert_eq!(v.healthy_for_ms(60_000), 60_000);
    v.on_self_check(false, 60_500);
    assert!(!v.http_ok(60_500));
    assert_eq!(v.healthy_for_ms(60_500), 0);
    assert_eq!(v.update(true, false, 61_000), D::Wait);
    assert_eq!(v.missing(), OtaValidator::CHECK_HTTP);
    v.on_self_check(true, 62_000);
    assert_eq!(v.update(true, false, 62_000), D::Wait);
    // The healthy period restarted at 62 s.
    assert_eq!(run_healthy(&mut v, 63_000, 181_000, true, false), D::Wait);
    assert_eq!(v.update(true, false, 182_000), D::MarkValid);
}

#[test]
fn network_and_link_without_a_self_check_never_mark_valid() {
    let mut v = OtaValidator::default();
    v.begin(true, true, 0);
    for t in (0..900_000).step_by(1000) {
        assert_eq!(v.update(true, true, t), D::Wait, "t {t}");
    }
    assert_eq!(v.update(true, true, 900_000), D::Rollback);
    assert_eq!(v.missing(), OtaValidator::CHECK_HTTP);
}

#[test]
fn an_unhealthy_second_restarts_the_period() {
    let mut v = OtaValidator::default();
    v.begin(true, false, 0);
    assert_eq!(run_healthy(&mut v, 0, 60_000, true, false), D::Wait);
    assert_eq!(v.update(false, false, 61_000), D::Wait);
    assert_eq!(v.healthy_for_ms(61_000), 0);
    assert_eq!(run_healthy(&mut v, 62_000, 181_000, true, false), D::Wait);
    assert_eq!(v.healthy_for_ms(181_000), 119_000);
    assert_eq!(v.update(true, false, 182_000), D::MarkValid);
}

#[test]
fn mark_valid_wins_over_rollback_in_the_same_second() {
    let mut v = OtaValidator::new(10, 30);
    v.begin(true, false, 0);
    v.on_self_check(true, 20);
    assert_eq!(v.update(true, false, 20), D::Wait);
    assert_eq!(v.update(true, false, 30), D::MarkValid);
    let mut r = OtaValidator::new(10, 30);
    r.begin(true, false, 0);
    r.on_self_check(true, 25);
    assert_eq!(r.update(true, false, 25), D::Wait);
    assert_eq!(r.update(true, false, 30), D::Rollback);
}

#[test]
fn begin_twice_re_arms_everything() {
    let mut v = OtaValidator::default();
    v.begin(true, true, 0);
    v.on_self_check(true, 0);
    assert_eq!(v.update(true, false, 900_000), D::Rollback);
    v.begin(true, false, 1_000_000);
    assert!(v.pending());
    assert!(!v.stm_required());
    assert_eq!(v.missing(), 0);
    assert!(v.self_check_due(true, 1_000_000));
    assert!(!v.http_ok(1_000_000));
    assert_eq!(v.remaining_ms(1_000_000), 900_000);
    assert_eq!(v.update(true, false, 1_000_000), D::Wait);
    assert_eq!(v.missing(), OtaValidator::CHECK_HTTP);
}

#[test]
fn start_near_the_32_bit_wrap() {
    let mut v = OtaValidator::default();
    let s: u32 = 0xFFFF_F000;
    v.begin(true, false, s);
    assert_eq!(v.remaining_ms(s.wrapping_add(1000)), 899_000);
    assert_eq!(
        run_healthy(&mut v, s, s.wrapping_add(119_000), true, false),
        D::Wait
    );
    assert_eq!(v.update(true, false, s.wrapping_add(120_000)), D::MarkValid);
    let mut r = OtaValidator::default();
    r.begin(true, false, s);
    assert_eq!(r.update(false, false, s.wrapping_add(899_999)), D::Wait);
    assert_eq!(r.update(false, false, s.wrapping_add(900_000)), D::Rollback);
}

#[test]
fn confirm_before_restart_over_user_net_link_and_stm_required() {
    for stm_req in [false, true] {
        for user in [false, true] {
            for net_up in [false, true] {
                for link in [false, true] {
                    let mut v = OtaValidator::default();
                    v.begin(true, stm_req, 0);
                    let expected = user && net_up && (!stm_req || link);
                    let case = (stm_req, user, net_up, link);
                    assert_eq!(
                        v.confirm_before_restart(user, net_up, link),
                        expected,
                        "{case:?}"
                    );
                    assert_eq!(v.pending(), !expected, "{case:?}");
                    assert_eq!(
                        !v.confirm_before_restart(true, true, true),
                        expected,
                        "{case:?}"
                    );
                }
            }
        }
    }
    let mut not_pending = OtaValidator::default();
    not_pending.begin(false, false, 0);
    assert!(!not_pending.confirm_before_restart(true, true, true));
    let mut confirmed = OtaValidator::default();
    confirmed.begin(true, false, 0);
    assert!(confirmed.confirm_before_restart(true, true, false));
    assert_eq!(confirmed.update(false, false, 900_000), D::NotPending);
}

#[test]
fn normalize_md5_cases() {
    // C++ `bool normalizeMd5(in, out)` (out unchanged on failure) is Option<[u8; 32]>.
    let lower = |s: &[u8]| normalize_md5(s).map(|o| o.to_vec());
    assert_eq!(
        lower(b"09afAF0123456789abcdefABCDEF0123").as_deref(),
        Some(&b"09afaf0123456789abcdefabcdef0123"[..])
    );
    assert_eq!(lower(b"GGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGG"), None);
    // C++ normalizeMd5(nullptr, out): no Rust form.
    assert_eq!(lower(b""), None);
    assert_eq!(lower(b"09afAF0123456789abcdefABCDEF012"), None); // 31
    assert_eq!(lower(b"09afAF0123456789abcdefABCDEF01234"), None); // 33
    assert_eq!(lower(b"09afAF0123456789abcdefABCDEF012g"), None);
    assert_eq!(lower(b"/9afAF0123456789abcdefABCDEF0123"), None); // '0' - 1
    assert_eq!(lower(b":9afAF0123456789abcdefABCDEF0123"), None); // '9' + 1
    assert_eq!(lower(b"`9afAF0123456789abcdefABCDEF0123"), None); // 'a' - 1
    assert_eq!(lower(b"@9afAF0123456789abcdefABCDEF0123"), None); // 'A' - 1
    assert_eq!(lower(b"G9afAF0123456789abcdefABCDEF0123"), None);
    assert_eq!(
        lower(b"0123456789abcdefABCDEF0000000000").as_deref(),
        Some(&b"0123456789abcdefabcdef0000000000"[..])
    );
    assert_eq!(
        lower(b"ffffffffffffffffffffffffffffffff").as_deref(),
        Some(&b"ffffffffffffffffffffffffffffffff"[..])
    );
    assert_eq!(
        lower(b"FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF").as_deref(),
        Some(&b"ffffffffffffffffffffffffffffffff"[..])
    );
    assert_eq!(
        lower(b"99999999999999999999999999999999").as_deref(),
        Some(&b"99999999999999999999999999999999"[..])
    );
    // Rust addition: the input is a C string (the C++ reads up to its NUL).
    assert_eq!(
        lower(b"ffffffffffffffffffffffffffffffff\0junk").as_deref(),
        Some(&b"ffffffffffffffffffffffffffffffff"[..])
    );
    assert_eq!(lower(b"fffffffffffffffffffffffffffffff\0f"), None);
}

#[test]
fn constants_and_default() {
    // Rust addition: the constants and the C++ default arguments (120 s, 900 s).
    assert_eq!(OtaValidator::SELF_CHECK_INTERVAL_MS, 10_000);
    assert_eq!(OtaValidator::HTTP_FRESH_MS, 30_000);
    assert_eq!(OtaValidator::default(), OtaValidator::new(120_000, 900_000));
    let mut v = OtaValidator::default();
    v.begin(true, false, 0);
    assert_eq!(v.remaining_ms(0), 900_000);
}
