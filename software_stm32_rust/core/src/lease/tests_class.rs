//! Port of test/native/test_lease__class.cpp: Lease expiry, renewal sources, client window,
//! saturation, warm restore.

use super::*;

#[test]
fn timeout_0_is_off_whatever_happens() {
    let mut l = Lease::default();
    assert_eq!(l.timeout(), 0);
    assert_eq!(l.state(), LeaseState::Off);
    l.advance(100_000);
    assert_eq!(l.state(), LeaseState::Off);
    assert_eq!(l.remaining_s(), 0);
    l.heartbeat(true);
    l.valve_poll();
    assert_eq!(l.state(), LeaseState::Off);
    assert_eq!(l.remaining_s(), 0);
}

#[test]
fn runs_down_to_the_second_expires_and_is_renewed_by_slhbt_1_only() {
    let mut l = Lease::default();
    l.set_timeout(5);
    assert_eq!(l.timeout(), 5);
    assert_eq!(l.state(), LeaseState::Running);
    assert_eq!(l.remaining_s(), 300);
    l.advance(299);
    assert_eq!(l.state(), LeaseState::Running);
    assert_eq!(l.remaining_s(), 1);
    l.advance(1);
    assert_eq!(l.state(), LeaseState::Expired);
    assert_eq!(l.remaining_s(), 0);
    l.heartbeat(false);
    assert_eq!(l.state(), LeaseState::Expired);
    assert!(l.client_present());
    l.heartbeat(true);
    assert_eq!(l.state(), LeaseState::Running);
    assert_eq!(l.remaining_s(), 300);
}

#[test]
fn a_valve_poll_renews_only_while_no_lease_client_is_present() {
    let mut l = Lease::default();
    l.set_timeout(5);
    l.advance(200);
    assert!(!l.client_present());
    l.valve_poll();
    assert_eq!(l.remaining_s(), 300);
    l.lease_command();
    assert!(l.client_present());
    l.advance(200);
    l.valve_poll();
    assert_eq!(l.remaining_s(), 100);
    l.advance(99);
    assert!(l.client_present()); // 299 s since the lease command
    l.valve_poll();
    assert_eq!(l.remaining_s(), 1);
    l.advance(1);
    assert!(!l.client_present()); // 300 s
    assert_eq!(l.state(), LeaseState::Expired);
    l.valve_poll();
    assert_eq!(l.state(), LeaseState::Running);
    assert_eq!(l.remaining_s(), 300);
}

#[test]
fn set_timeout_renews_only_when_the_lease_is_switched_on() {
    let mut off = Lease::default();
    off.advance(7200);
    off.set_timeout(60);
    assert_eq!(off.state(), LeaseState::Running);
    assert_eq!(off.remaining_s(), 3600);

    let mut on = Lease::default();
    on.set_timeout(30);
    on.advance(1000);
    on.set_timeout(60);
    assert_eq!(on.remaining_s(), 3600 - 1000);
    on.set_timeout(5);
    assert_eq!(on.state(), LeaseState::Expired);
    on.set_timeout(0);
    assert_eq!(on.state(), LeaseState::Off);
    on.set_timeout(0);
    assert_eq!(on.state(), LeaseState::Off);
    on.set_timeout(5);
    assert_eq!(on.remaining_s(), 300);
}

#[test]
fn expiry_is_exactly_timeout_times_60_s_for_the_largest_timeout() {
    let mut l = Lease::default();
    l.set_timeout(1440);
    l.advance(86399);
    assert_eq!(l.remaining_s(), 1);
    l.advance(1);
    assert_eq!(l.state(), LeaseState::Expired);
}

#[test]
fn the_client_window_and_client_seen_within() {
    let mut l = Lease::default();
    assert!(!l.client_seen_within(86400));
    assert!(!l.client_present());
    l.advance(10);
    l.lease_command();
    l.advance(86399);
    assert!(l.client_seen_within(86400));
    assert!(!l.client_seen_within(86399));
    l.advance(1);
    assert!(!l.client_seen_within(86400));
    l.lease_command();
    assert!(l.client_seen_within(1));
}

#[test]
fn the_counters_saturate_instead_of_wrapping() {
    let mut l = Lease::default();
    l.set_timeout(1440);
    l.lease_command();
    l.advance(u32::MAX - 5);
    assert_eq!(l.snapshot().since_renewal_s, u32::MAX - 5);
    l.advance(5);
    assert_eq!(l.snapshot().since_renewal_s, u32::MAX);
    l.advance(10);
    assert_eq!(l.state(), LeaseState::Expired);
    assert_eq!(l.snapshot().since_renewal_s, u32::MAX);
    assert_eq!(l.snapshot().since_client_s, u32::MAX);
    l.advance(u32::MAX);
    assert_eq!(l.snapshot().since_renewal_s, u32::MAX);
    assert!(!l.client_present());
}

#[test]
fn snapshot_and_restore_round_trip() {
    let mut a = Lease::default();
    a.set_timeout(5);
    a.advance(120);
    a.heartbeat(false);
    a.advance(7);
    let s: Snapshot = a.snapshot();
    assert_eq!(s.since_renewal_s, 127);
    assert_eq!(s.since_client_s, 7);
    assert!(s.client);
    let mut b = Lease::default();
    b.set_timeout(5);
    b.restore(&s);
    assert_eq!(b.remaining_s(), 173);
    assert!(b.client_present());
    b.restore(&Snapshot {
        since_renewal_s: 400,
        since_client_s: 300,
        client: false,
    });
    assert_eq!(b.state(), LeaseState::Expired);
    assert!(!b.client_present());
    assert_eq!(b.snapshot().since_client_s, 300);
    assert!(!b.snapshot().client);
}
