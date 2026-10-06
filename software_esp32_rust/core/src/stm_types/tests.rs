//! `vdm/stm_types.h` has no C++ test file of its own: these tests pin the C++ member
//! initialisers and enum numbers that the STM task, the restart gate and the glue rely on.

use super::*;
use crate::failsafe::{LeaseMode, LeaseState, RegulatorCause};
use crate::stm_flasher::{BoardCheck, FlashError, FlashPhase};

#[test]
fn command_type_numbers_are_the_cpp_declaration_order() {
    let all = [
        StmCommandType::SetTarget,
        StmCommandType::Calibrate,
        StmCommandType::Assembly,
        StmCommandType::Detect,
        StmCommandType::ScanSensors,
        StmCommandType::SetValveSensors,
        StmCommandType::SetMotorSettings,
        StmCommandType::ServiceMove,
        StmCommandType::RequestProfile,
        StmCommandType::ResetStm,
        StmCommandType::StartFlash,
        StmCommandType::AbortFlash,
        StmCommandType::ConfigChanged,
        StmCommandType::StopValve,
        StmCommandType::LeaveSafeMode,
    ];
    for (i, t) in all.iter().enumerate() {
        assert_eq!(*t as usize, i);
    }
}

#[test]
fn command_defaults_are_the_cpp_member_initialisers() {
    let c = StmCommand::default();
    assert_eq!(c.kind, StmCommandType::ConfigChanged);
    assert_eq!(c.valve, NO_VALVE);
    assert_eq!(c.pos, 0);
    assert_eq!(c.source, TargetSource::None);
    assert_eq!(c.ids, [OneWireId { b: [0; 8] }; 2]);
    assert!(!c.has_motor);
    assert_eq!(c.motor, MotorChars::default());
    assert_eq!(
        (
            c.motor.low_factor,
            c.motor.high_factor,
            c.motor.start_on_power
        ),
        (17, 17, 30)
    );
    assert_eq!(
        (
            c.motor.min_counts,
            c.motor.max_calib_retries,
            c.motor.field_count
        ),
        (3000, 2, 5)
    );
    assert!(!c.has_learn_movements);
    assert_eq!(c.learn_movements, 0);
    assert!(!c.has_breakaway);
    assert_eq!(
        c.breakaway,
        Breakaway {
            enable: false,
            step_pct: 0,
            max_ma: 60
        }
    );
    assert_eq!(c.dir, MoveDir::Open);
    assert_eq!(c.counts, 0);
    assert_eq!(c.max_ma, 0);
    assert!(c.image.is_empty());
    assert_eq!(c.image.capacity(), 31); // C++ char[32]
    assert!(!c.blank);
    assert!(!c.force);
    assert!(!c.scheduled);
    assert!(c.board.is_empty());
    assert_eq!(c.board.capacity(), 3); // C++ char[4]
    assert_eq!(c.attempt, 0);
}

#[test]
fn save_state_numbers_and_from_raw() {
    let all = [
        (0, StmSaveState::Idle),
        (1, StmSaveState::Waiting),
        (2, StmSaveState::Saved),
        (3, StmSaveState::Unavailable),
        (4, StmSaveState::TimedOut),
    ];
    for (v, s) in all {
        assert_eq!(s as u8, v);
        assert_eq!(StmSaveState::from_raw(v), Some(s));
    }
    assert_eq!(StmSaveState::from_raw(5), None);
    assert_eq!(StmSaveState::from_raw(255), None);
    assert_eq!(StmSaveState::default(), StmSaveState::Idle);
}

#[test]
fn snapshot_defaults_are_the_cpp_member_initialisers() {
    let s = StmSnapshot::default();
    assert_eq!(s.revision, 0);
    assert_eq!(s.taken_ms, 0);
    assert_eq!(s.valves.len(), 12);
    for v in &s.valves {
        assert_eq!(*v, ValveState::EMPTY);
    }
    assert_eq!(s.temps.len(), 34);
    for t in &s.temps {
        assert_eq!(*t, TempReading::EMPTY);
        assert_eq!(t.raw, -500);
    }
    assert_eq!(s.temp_count, 0);
    assert_eq!(s.volts.len(), 8);
    for v in &s.volts {
        assert_eq!(*v, VoltReading::EMPTY);
        assert_eq!(v.vad, -1000);
    }
    assert_eq!(s.volt_count, 0);
    assert_eq!(s.link, LinkState::Unknown);
    assert_eq!(s.link_stats, LinkStats::default());
    assert_eq!(s.link_stats.sent, 0);
    assert_eq!(s.proto, 0);
    assert_eq!(s.version, Version::default());
    assert!(!s.version.valid);
    assert_eq!(s.build, 0);
    assert_eq!(s.hw_id, 0);
    assert!(s.compatible);
    assert!(!s.have_motor);
    assert_eq!(s.motor, MotorChars::default());
    assert_eq!(s.learn_movements, 0);
    assert!(!s.have_breakaway);
    assert_eq!(s.breakaway, Breakaway::default());
    assert!(!s.have_status);
    assert_eq!(s.status, StmStatus::default());
    assert_eq!(s.status.lease, LeaseState::Off);
    assert!(!s.sensors_settled);
    assert_eq!(s.line_overflows, 0);
    assert_eq!(s.line_malformed, 0);
    assert_eq!(s.flash, FlashStatus::default());
    assert_eq!(s.flash.phase, FlashPhase::Idle);
    assert_eq!(s.flash.error, FlashError::None);
    assert_eq!(s.flash.board, BoardCheck::Ok);
    assert!(s.flash_image.is_empty());
    assert_eq!(s.flash_image.capacity(), 31); // C++ char[32]
    assert_eq!(s.profile_seq, [0; 12]);
    assert_eq!(s.support, StmSupport::Unknown);
    assert_eq!(s.lease, LeaseStatus::default());
    assert_eq!(s.lease.mode, LeaseMode::None);
    assert_eq!(s.lease.regulator, RegulatorCause::BrokerDown);
    assert!(s.lease.config_trusted);
    assert!(!s.have_learn_time);
    assert_eq!(s.learn_time_s, 0);
    assert!(!s.flash_pending);
}

#[test]
fn a_snapshot_copy_is_equal_and_independent() {
    let mut a = StmSnapshot {
        revision: 7,
        ..StmSnapshot::default()
    };
    a.valves[11].position = 42;
    a.temps[33].raw = 215;
    let _ = a.flash_image.extend_from_slice(b"vdm_2.1.7.bin");
    let mut b = StmSnapshot::default();
    b.clone_from(&a);
    assert_eq!(b, a);
    a.valves[11].position = 43;
    assert_eq!(b.valves[11].position, 42);
    assert_ne!(b, a);
}
