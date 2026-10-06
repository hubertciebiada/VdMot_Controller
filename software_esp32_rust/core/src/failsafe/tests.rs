//! Port of test/native/test_failsafe.cpp: constants, ranges, the regulator rule and every
//! name. The C++ "unknown" names of out-of-range values have no Rust form (an enum cannot hold
//! them); the raw values are refused by `from_raw`, tested instead.

use super::*;

#[test]
fn constants() {
    assert_eq!(FAILSAFE_HOLD, 255);
    assert_eq!(FAILSAFE_PCT_DEFAULT, 50);
    assert_eq!(FAILSAFE_TIMEOUT_DEFAULT_MIN, 60);
    assert_eq!(FAILSAFE_TIMEOUT_MIN_MIN, 5);
    assert_eq!(FAILSAFE_TIMEOUT_MAX_MIN, 1440);
}

#[test]
fn lease_timeout_range_is_0_or_5_to_1440() {
    assert!(lease_timeout_valid(0));
    assert!(!lease_timeout_valid(1));
    assert!(!lease_timeout_valid(4));
    assert!(lease_timeout_valid(5));
    assert!(lease_timeout_valid(6));
    assert!(lease_timeout_valid(60));
    assert!(lease_timeout_valid(1439));
    assert!(lease_timeout_valid(1440));
    assert!(!lease_timeout_valid(1441));
    assert!(!lease_timeout_valid(65536));
    assert!(!lease_timeout_valid(u32::MAX));
}

#[test]
fn position_range_is_0_to_100_or_255() {
    assert!(failsafe_pct_valid(0));
    assert!(failsafe_pct_valid(1));
    assert!(failsafe_pct_valid(50));
    assert!(failsafe_pct_valid(99));
    assert!(failsafe_pct_valid(100));
    assert!(!failsafe_pct_valid(101));
    assert!(!failsafe_pct_valid(254));
    assert!(failsafe_pct_valid(255));
    assert!(!failsafe_pct_valid(256));
    assert!(!failsafe_pct_valid(u32::MAX));
}

#[test]
fn regulator_cause_for_every_mode_session_and_ha_status() {
    use HaStatus::{Offline, Online, Unknown};
    use MqttMode::{Mqtt, MqttHa, Off};
    use RegulatorCause::{Alive, BrokerDown, HaOffline};
    let rows = [
        (Off, false, Unknown, Alive),
        (Off, false, Online, Alive),
        (Off, false, Offline, Alive),
        (Off, true, Unknown, Alive),
        (Off, true, Online, Alive),
        (Off, true, Offline, Alive),
        (Mqtt, false, Unknown, BrokerDown),
        (Mqtt, false, Online, BrokerDown),
        (Mqtt, false, Offline, BrokerDown),
        (Mqtt, true, Unknown, Alive),
        (Mqtt, true, Online, Alive),
        (Mqtt, true, Offline, Alive), // HA ignored
        (MqttHa, false, Unknown, BrokerDown),
        (MqttHa, false, Online, BrokerDown),
        (MqttHa, false, Offline, BrokerDown),
        (MqttHa, true, Unknown, Alive),
        (MqttHa, true, Online, Alive),
        (MqttHa, true, Offline, HaOffline),
    ];
    for (mode, connected, ha, cause) in rows {
        let input = RegulatorInput {
            mode,
            broker_connected: connected,
            ha,
            command_seq: 7, // never part of the rule
        };
        assert_eq!(
            regulator_cause(&input),
            cause,
            "{mode:?} {connected} {ha:?}"
        );
    }
}

#[test]
fn defaults_of_the_shared_structs() {
    let input = RegulatorInput::default();
    assert_eq!(input.mode, MqttMode::Off);
    assert!(!input.broker_connected);
    assert_eq!(input.ha, HaStatus::Unknown);
    assert_eq!(input.command_seq, 0);
    assert_eq!(regulator_cause(&input), RegulatorCause::Alive);

    let s = LeaseStatus::default();
    assert_eq!(s.mode, LeaseMode::None);
    assert_eq!(s.state, LeaseState::Off);
    assert_eq!(s.remain_s, 0);
    assert_eq!(s.timeout_min, 0);
    assert_eq!(s.failsafe_mask, 0);
    assert_eq!(s.regulator, RegulatorCause::BrokerDown);
    assert_eq!(s.regulator_lost_s, 0);
    assert!(!s.config_synced);
    assert!(!s.config_failed);
    assert!(s.config_trusted);
}

#[test]
fn names_of_every_value_and_out_of_range() {
    assert_eq!(lease_state_name(LeaseState::Off), "off");
    assert_eq!(lease_state_name(LeaseState::Running), "running");
    assert_eq!(lease_state_name(LeaseState::Expired), "expired");
    assert_eq!(LeaseState::from_raw(3), None);

    assert_eq!(lease_mode_name(LeaseMode::None), "none");
    assert_eq!(lease_mode_name(LeaseMode::Stm), "stm");
    assert_eq!(lease_mode_name(LeaseMode::Emulated), "esp");
    assert_eq!(LeaseMode::from_raw(3), None);

    assert_eq!(ha_status_name(HaStatus::Unknown), "unknown");
    assert_eq!(ha_status_name(HaStatus::Online), "online");
    assert_eq!(ha_status_name(HaStatus::Offline), "offline");
    assert_eq!(HaStatus::from_raw(3), None);

    assert_eq!(regulator_cause_name(RegulatorCause::Alive), "alive");
    assert_eq!(
        regulator_cause_name(RegulatorCause::BrokerDown),
        "broker_down"
    );
    assert_eq!(
        regulator_cause_name(RegulatorCause::HaOffline),
        "ha_offline"
    );
    assert_eq!(RegulatorCause::from_raw(3), None);

    assert_eq!(failsafe_kind_name(FailsafeKind::None), "off");
    assert_eq!(failsafe_kind_name(FailsafeKind::Lease), "lease");
    assert_eq!(failsafe_kind_name(FailsafeKind::Blocked), "blocked");
    assert_eq!(FailsafeKind::from_raw(3), None);
}

#[test]
fn raw_values_round_trip() {
    // Rust addition: from_raw is the inverse of the discriminants.
    for v in 0..=2u8 {
        assert_eq!(LeaseState::from_raw(v).map(|s| s as u8), Some(v));
        assert_eq!(LeaseMode::from_raw(v).map(|s| s as u8), Some(v));
        assert_eq!(HaStatus::from_raw(v).map(|s| s as u8), Some(v));
        assert_eq!(RegulatorCause::from_raw(v).map(|s| s as u8), Some(v));
        assert_eq!(FailsafeKind::from_raw(v).map(|s| s as u8), Some(v));
    }
    assert_eq!(LeaseState::from_raw(255), None);
    assert_eq!(MqttMode::MqttHa as u8, 2);
    assert_eq!(MqttMode::default(), MqttMode::Off);
    assert_eq!(FailsafeKind::default(), FailsafeKind::None);
    assert_eq!(LeaseMode::default(), LeaseMode::None);
    assert_eq!(LeaseState::default(), LeaseState::Off);
    assert_eq!(RegulatorCause::default(), RegulatorCause::Alive);
}
