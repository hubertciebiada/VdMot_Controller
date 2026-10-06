//! Port of test/native/test_factory_reset.cpp: boot decision, run-time latch clear, hold
//! detector.

use super::*;
use FactoryPinDecision as F;
use PinHoldState as S;

#[test]
fn factory_pin_at_boot_the_8_combinations() {
    assert_eq!(factory_pin_at_boot(false, false, false), F::Idle);
    assert_eq!(factory_pin_at_boot(false, true, false), F::Idle);
    assert_eq!(factory_pin_at_boot(false, false, true), F::ClearLatch);
    assert_eq!(factory_pin_at_boot(false, true, true), F::ClearLatch);
    assert_eq!(factory_pin_at_boot(true, false, true), F::KeepLatched);
    assert_eq!(factory_pin_at_boot(true, true, true), F::KeepLatched);
    assert_eq!(factory_pin_at_boot(true, false, false), F::Idle);
    assert_eq!(factory_pin_at_boot(true, true, false), F::Reset);
}

#[test]
fn factory_pin_runtime_clear_cases() {
    assert!(!factory_pin_runtime_clear(false, false));
    assert!(factory_pin_runtime_clear(false, true));
    assert!(!factory_pin_runtime_clear(true, false));
    assert!(!factory_pin_runtime_clear(true, true));
}

#[test]
fn pin_hold_held_after_5_s_of_low() {
    let mut h = PinHold::new(5000);
    h.begin(100);
    assert_eq!(h.sample(true, 100), S::Holding);
    assert_eq!(h.sample(true, 5099), S::Holding);
    assert_eq!(h.sample(true, 5100), S::Held);
    assert_eq!(h.sample(false, 5150), S::Held); // final
    h.begin(10_000);
    assert_eq!(h.sample(true, 10_000), S::Holding);
}

#[test]
fn pin_hold_a_high_sample_releases_for_good() {
    let mut h = PinHold::new(5000);
    h.begin(100);
    assert_eq!(h.sample(true, 1000), S::Holding);
    assert_eq!(h.sample(false, 2000), S::Released);
    assert_eq!(h.sample(true, 3000), S::Released);
    assert_eq!(h.sample(true, 9000), S::Released);
}

#[test]
fn pin_hold_begun_near_the_32_bit_wrap() {
    let mut h = PinHold::new(5000);
    let s: u32 = 0xFFFF_F000;
    h.begin(s);
    assert_eq!(h.sample(true, s.wrapping_add(4999)), S::Holding);
    assert_eq!(h.sample(true, s.wrapping_add(5000)), S::Held);
}

#[test]
fn pin_hold_starts_holding_before_begin() {
    // Rust addition: the state of a new detector (the C++ member initialisers).
    let mut h = PinHold::new(10);
    assert_eq!(h.sample(true, 9), S::Holding);
    assert_eq!(h.sample(true, 10), S::Held);
}
