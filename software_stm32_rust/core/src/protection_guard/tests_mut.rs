//! Port of test/native/test_protection_guard__mut.cpp.

use super::*;
use crate::legacy_layout::VALVE_COUNT;

#[test]
fn an_out_of_range_valve_reports_the_current_state_without_counting() {
    let mut g = ProtectionGuard::default();
    assert!(!g.on_trip(VALVE_COUNT, 100));
    assert!(!g.suspended());
    assert!(!g.on_trip(0, 100));
    assert!(!g.on_trip(1, 100));
    assert!(g.on_trip(2, 100));
    assert!(g.on_trip(VALVE_COUNT, 200));
    assert!(g.on_trip(255, 200));
    assert!(g.suspended());
}
