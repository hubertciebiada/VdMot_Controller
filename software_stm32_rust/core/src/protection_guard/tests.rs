//! Port of test/native/test_protection_guard.cpp.

use super::*;

#[test]
fn constants_the_limits_report_only_until_measured_on_the_hardware() {
    assert_eq!(PROTECT_TRIP_VALVES, 3);
    assert_eq!(PROTECT_WINDOW_S, 600);
    // C++ CHECK_FALSE(kProtectEnforce) at run time; clippy wants an assertion on a constant
    // in a const block, so this one is checked at compile time
    const { assert!(!PROTECT_ENFORCE) };
}

#[test]
fn trips_of_2_valves_within_600_s_do_not_suspend() {
    let mut g = ProtectionGuard::default();
    assert!(!g.suspended());
    assert!(!g.on_trip(0, 100));
    assert!(!g.on_trip(1, 200));
    assert!(!g.on_trip(1, 300));
    assert!(!g.suspended());
}

#[test]
fn trips_of_3_valves_within_600_s_suspend_until_the_next_start() {
    let mut g = ProtectionGuard::default();
    assert!(!g.on_trip(3, 1000));
    assert!(!g.on_trip(7, 1300));
    assert!(g.on_trip(11, 1599));
    assert!(g.suspended());
    assert!(g.on_trip(3, 100_000));
}

#[test]
fn three_trips_of_one_valve_do_not_suspend() {
    let mut g = ProtectionGuard::default();
    assert!(!g.on_trip(5, 10));
    assert!(!g.on_trip(5, 20));
    assert!(!g.on_trip(5, 30));
    assert!(!g.suspended());
}

#[test]
fn the_window_boundary_is_599_s_in_600_s_out() {
    let mut inside = ProtectionGuard::default();
    inside.on_trip(0, 0);
    inside.on_trip(1, 0);
    assert!(inside.on_trip(2, 599));
    let mut out = ProtectionGuard::default();
    out.on_trip(0, 0);
    out.on_trip(1, 0);
    assert!(!out.on_trip(2, 600));
    // the older trips left the window, a later trip of them counts again
    assert!(!out.on_trip(3, 1300));
    assert!(!out.on_trip(0, 1300));
    assert!(out.on_trip(4, 1301));
}

#[test]
fn a_valve_that_never_tripped_does_not_count_with_uptime_0() {
    let mut g = ProtectionGuard::default();
    assert!(!g.on_trip(0, 0));
    assert!(!g.on_trip(1, 0));
    assert!(!g.suspended());
    assert!(!g.on_trip(12, 0)); // no such valve
    assert!(g.on_trip(11, 0));
}
