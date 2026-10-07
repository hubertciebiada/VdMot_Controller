//! Port of test/native/test_valve_model.cpp: target delivery state machine, health flags,
//! change detection, sensor bookkeeping.
//!
//! The C++ `nextTargetPush`/`nextVerify` leave their out-parameters alone on false; the Rust
//! functions return None, so a check of the untouched variables is the None result.

use super::*;
use crate::failsafe::LeaseMode;
use crate::stm_codec::{CAL_FLAG_EARLY_STOP, CAL_FLAG_LAST_FAILED};
use crate::stm_types::{StmCommand, StmCommandType, StmSaveState, StmSnapshot};
use crate::version::StmSupport;

fn status_data(valve: u8, status: u8) -> ValveData {
    ValveData {
        valve,
        position: 40,
        mean_current: 12,
        status,
        temp1: 215,
        temp2: TEMP_UNASSIGNED,
        moves: 7,
        open_count: 3000,
        close_count: 3100,
        dead_zone: -4,
        calib_retries: 0,
        ..ValveData::default()
    }
}

/// C++ `data(valve)`: status 1.
fn data(valve: u8) -> ValveData {
    status_data(valve, 1)
}

fn ex(valve: u8, target: u8, early: u32, rejected: u32) -> ValveEx {
    ValveEx {
        valve,
        status: 1,
        position: 33,
        target,
        mean_current: 14,
        open_count: 11,
        close_count: 12,
        dead_zone: -3,
        calib_retries: 1,
        moves: 99,
        cal_state: 2,
        cal_flags: CAL_FLAG_EARLY_STOP,
        early_stops: early,
        cmd_rejected: rejected,
        ..ValveEx::default()
    }
}

fn target(valve: u8, pos: u8) -> TargetReply {
    TargetReply { valve, target: pos }
}

fn id_with_crc(seed: u8) -> OneWireId {
    let mut id = OneWireId::default();
    id.b[0] = 0x28;
    for i in 1..7u8 {
        id.b[usize::from(i)] = seed.wrapping_mul(7).wrapping_add(i);
    }
    // Compute the Dallas CRC so the id is valid.
    let mut crc = 0u8;
    for &byte in &id.b[..7] {
        let mut x = byte;
        for _ in 0..8 {
            let mix = (crc ^ x) & 1;
            crc >>= 1;
            if mix != 0 {
                crc ^= 0x8C;
            }
            x >>= 1;
        }
    }
    id.b[7] = crc;
    id
}

/// Model with valve 0 active and known, desired 50 delivered and verified.
fn synced_model() -> ValveModel {
    let now = 1000;
    let mut m = ValveModel::default();
    m.set_active_mask(0x001);
    m.apply_valve_data(&data(0), now);
    m.apply_target(&target(0, 50), now);
    m
}

/// The valve of the next stgtp.
fn push_valve(m: &mut ValveModel, now: u32) -> Option<u8> {
    m.next_target_push(now).map(|(valve, _)| valve)
}

/// A goned reading of the sensor the list has at `bus`.
fn read(s: &mut SensorModel, l: &OneWireList, bus: u8, raw: i16, now: u32) {
    let td = TempData {
        valid: true,
        id: l.ids[usize::from(bus)],
        value: raw,
    };
    s.apply_temp_data(bus, &td, now);
}

#[test]
fn valve_status_texts_and_keys_cover_every_status() {
    let texts = [
        "",
        "idle",
        "opens",
        "closes",
        "failed",
        "unknown",
        "no valve",
        "full open",
        "connected",
        "blocked",
    ];
    let keys = [
        "nodata",
        "idle",
        "opening",
        "closing",
        "failed",
        "unknown",
        "novalve",
        "fullopen",
        "connected",
        "blocked",
    ];
    for (s, (text, key)) in (0..10u8).zip(texts.iter().zip(keys)) {
        assert_eq!(valve_status_text(s), *text);
        assert_eq!(valve_status_key(s), key);
    }
    assert_eq!(valve_status_text(10), "");
    assert_eq!(valve_status_text(255), "");
    assert_eq!(valve_status_key(10), "invalid");
    assert_eq!(valve_status_key(255), "invalid");
}

#[test]
fn target_source_and_sync_names() {
    assert_eq!(target_source_name(TargetSource::None), "none");
    assert_eq!(target_source_name(TargetSource::Stm), "stm");
    assert_eq!(target_source_name(TargetSource::Web), "web");
    assert_eq!(target_source_name(TargetSource::Mqtt), "mqtt");
    assert_eq!(target_source_name(TargetSource::Restored), "restored");
    assert_eq!(target_source_name(TargetSource::Assembly), "assembly");
    assert_eq!(TargetSource::Restored as u8, 4);
    assert_eq!(TargetSource::Assembly as u8, 5);
    // C++ targetSourceName(static_cast<TargetSource>(6)) == "unknown": no Rust form.
    assert_eq!(TargetSource::from_raw(6), None);
    assert_eq!(target_sync_name(TargetSync::Unknown), "unknown");
    assert_eq!(target_sync_name(TargetSync::Synced), "synced");
    assert_eq!(target_sync_name(TargetSync::Pending), "pending");
    assert_eq!(target_sync_name(TargetSync::AwaitAck), "await_ack");
    assert_eq!(target_sync_name(TargetSync::AwaitVerify), "await_verify");
    assert_eq!(target_sync_name(TargetSync::Failed), "failed");
    // C++ targetSyncName(static_cast<TargetSync>(6)) == "invalid": no Rust form.
    assert_eq!(TargetSync::from_raw(6), None);
}

#[test]
fn enum_numbers_round_trip() {
    let sources = [
        TargetSource::None,
        TargetSource::Stm,
        TargetSource::Web,
        TargetSource::Mqtt,
        TargetSource::Restored,
        TargetSource::Assembly,
    ];
    for (n, s) in (0u8..).zip(sources) {
        assert_eq!(s as u8, n);
        assert_eq!(TargetSource::from_raw(n), Some(s));
    }
    let syncs = [
        TargetSync::Unknown,
        TargetSync::Synced,
        TargetSync::Pending,
        TargetSync::AwaitAck,
        TargetSync::AwaitVerify,
        TargetSync::Failed,
    ];
    for (n, s) in (0u8..).zip(syncs) {
        assert_eq!(s as u8, n);
        assert_eq!(TargetSync::from_raw(n), Some(s));
    }
    assert_eq!(TargetSource::from_raw(255), None);
    assert_eq!(TargetSync::from_raw(255), None);
    assert_eq!(TargetSource::default(), TargetSource::None);
    assert_eq!(TargetSync::default(), TargetSync::Unknown);
}

#[test]
fn failsafe_kind_and_valve_at_failsafe() {
    let both = STM_FLAG_FS_LEASE | STM_FLAG_FS_BLOCKED;
    let others = !both; // C++ 0xFFFF & ~both
    let rows = [
        (false, 0, false, FailsafeKind::None),
        (false, STM_FLAG_FS_LEASE, false, FailsafeKind::None), // gvlvx: no v3 flags
        (false, STM_FLAG_FS_BLOCKED, false, FailsafeKind::None),
        (false, both, false, FailsafeKind::None),
        (false, 0, true, FailsafeKind::Lease), // ESP emulation
        (false, STM_FLAG_FS_BLOCKED, true, FailsafeKind::Lease),
        (true, 0, false, FailsafeKind::None),
        (true, others, false, FailsafeKind::None),
        (true, STM_FLAG_FS_LEASE, false, FailsafeKind::Lease),
        (true, STM_FLAG_FS_BLOCKED, false, FailsafeKind::Blocked),
        (true, both, false, FailsafeKind::Blocked), // blocked wins
        (true, 0, true, FailsafeKind::Lease),
        (true, STM_FLAG_FS_BLOCKED, true, FailsafeKind::Blocked),
    ];
    for (has_v3, flags, fs_override, kind) in rows {
        let v = ValveState {
            has_v3,
            stm_flags: flags,
            fs_override,
            ..ValveState::default()
        };
        let case = (has_v3, flags, fs_override);
        assert_eq!(failsafe_kind(&v), kind, "{case:?}");
        assert_eq!(
            valve_at_failsafe(&v),
            kind == FailsafeKind::Lease,
            "{case:?}"
        );
    }
}

#[test]
fn defaults_of_the_protocol_3_and_failsafe_fields() {
    let v = ValveState::default();
    assert!(!v.has_v3);
    assert_eq!(v.stm_flags, 0);
    assert_eq!(v.fault, 0);
    assert_eq!(v.fs_pct, FAILSAFE_HOLD);
    assert_eq!(v.drive, 0);
    assert_eq!(v.retry_s, 0);
    assert_eq!(v.retries, 0);
    assert!(!v.auto_retry);
    assert!(!v.fs_override);
    assert_eq!(v.fs_target, 0);
    assert!(!v.force_push);
    assert_eq!(failsafe_kind(&v), FailsafeKind::None);
    assert_eq!(HEALTH_FAILSAFE, 1 << 9);
    assert_eq!(HEALTH_STROKE_SHORT, 1 << 10);
    assert_eq!(CHANGE_FAILSAFE, 1 << 14);
    assert_eq!(TempReading::default().fail_streak, 0);
    assert_eq!(VoltReading::default().fail_streak, 0);
}

#[test]
fn defaults_of_the_other_fields_and_the_params() {
    let v = ValveState::default();
    assert_eq!(v, ValveState::EMPTY);
    assert!(!v.known);
    assert_eq!(v.temp1, TEMP_UNASSIGNED);
    assert_eq!(v.temp2, TEMP_UNASSIGNED);
    assert_eq!(v.source, TargetSource::None);
    assert_eq!(v.sync, TargetSync::Unknown);
    assert_eq!(v.last_move, MoveResult::default());
    assert!(v.sensor_id.iter().all(is_zero));
    assert_eq!(v.sensor_slot, [0, 0]);
    assert_eq!((v.health, v.revision), (0, 0));
    let t = TempReading::default();
    assert!(is_zero(&t.id));
    assert_eq!(t.raw, TEMP_UNASSIGNED);
    assert!(!t.seen);
    assert_eq!(t.last_seen_ms, 0);
    let r = VoltReading::default();
    assert!(is_zero(&r.id));
    assert_eq!(r.vad, VAD_FAILED);
    assert!(!r.seen);
    assert_eq!(r.last_seen_ms, 0);
    let p = ValveModelParams::default();
    assert_eq!(p.stale_ms, 60_000);
    assert_eq!(p.max_push_attempts, 5);
    assert_eq!(p.push_retry_ms, 2000);
    assert_eq!(p.failed_retry_ms, 300_000);
    assert_eq!(SENSOR_FAIL_DEBOUNCE, 2);
    assert_eq!(STM_TEMP_MAX_AGE_S, 200);
}

#[test]
fn health_and_change_bits() {
    let health = [
        HEALTH_BLOCKED,
        HEALTH_FAILED,
        HEALTH_NO_VALVE,
        HEALTH_CALIB_RETRIES,
        HEALTH_EARLY_STOP,
        HEALTH_CMD_REJECTED,
        HEALTH_STALE,
        HEALTH_TARGET_UNCONFIRMED,
        HEALTH_TEMP_FAILED,
        HEALTH_FAILSAFE,
        HEALTH_STROKE_SHORT,
    ];
    for (bit, flag) in health.into_iter().enumerate() {
        assert_eq!(flag, 1 << bit);
    }
    let changes = [
        CHANGE_STATUS,
        CHANGE_POSITION,
        CHANGE_TARGET,
        CHANGE_MEAN_CURRENT,
        CHANGE_TEMP1,
        CHANGE_TEMP2,
        CHANGE_COUNTERS,
        CHANGE_CALIB_RETRIES,
        CHANGE_EXTENDED,
        CHANGE_LAST_MOVE,
        CHANGE_SYNC,
        CHANGE_SENSORS,
        CHANGE_HEALTH,
        CHANGE_KNOWN,
        CHANGE_FAILSAFE,
    ];
    for (bit, group) in changes.into_iter().enumerate() {
        assert_eq!(group, 1 << bit);
    }
}

#[test]
fn stm_types_command_numbers_and_defaults() {
    // Command type numbers are external (StmQueueFull arg1): appended only.
    assert_eq!(StmCommandType::SetTarget as u8, 0);
    assert_eq!(StmCommandType::ConfigChanged as u8, 12);
    assert_eq!(StmCommandType::StopValve as u8, 13);
    assert_eq!(StmCommandType::LeaveSafeMode as u8, 14);
    let c = StmCommand::default();
    assert_eq!(c.kind, StmCommandType::ConfigChanged);
    assert!(c.board.is_empty()); // C++ board[0] == '\0'
    assert_eq!(c.attempt, 0);
    assert_eq!(StmSaveState::Idle as u8, 0);
    assert_eq!(StmSaveState::Waiting as u8, 1);
    assert_eq!(StmSaveState::Saved as u8, 2);
    assert_eq!(StmSaveState::Unavailable as u8, 3);
    assert_eq!(StmSaveState::TimedOut as u8, 4);
    let s = std::boxed::Box::new(StmSnapshot::default()); // large: not on the stack
    assert_eq!(s.support, StmSupport::Unknown);
    assert_eq!(s.lease.mode, LeaseMode::None);
    assert!(!s.have_learn_time);
    assert_eq!(s.learn_time_s, 0);
    assert!(!s.flash_pending);
}

/// Changes one field of a state.
type Mutation = fn(&mut ValveState);

#[test]
fn diff_valve_reports_exactly_the_changed_groups() {
    let a = ValveState::default();
    assert_eq!(diff_valve(&a, &a), 0);
    let cases: [(Mutation, u32); 27] = [
        (|s| s.status = 3, CHANGE_STATUS),
        (|s| s.calibrating = true, CHANGE_STATUS),
        (|s| s.position = 1, CHANGE_POSITION),
        (|s| s.desired = 1, CHANGE_TARGET),
        (|s| s.desired_valid = true, CHANGE_TARGET),
        (|s| s.mean_current = 1, CHANGE_MEAN_CURRENT),
        (|s| s.temp1 = 1, CHANGE_TEMP1),
        (|s| s.temp2 = 1, CHANGE_TEMP2),
        (|s| s.moves = 1, CHANGE_COUNTERS),
        (|s| s.open_count = 1, CHANGE_COUNTERS),
        (|s| s.close_count = 1, CHANGE_COUNTERS),
        (|s| s.dead_zone = -1, CHANGE_COUNTERS),
        (|s| s.calib_retries = 1, CHANGE_CALIB_RETRIES),
        (|s| s.cal_state = 1, CHANGE_EXTENDED),
        (|s| s.cal_flags = CAL_FLAG_LAST_FAILED, CHANGE_EXTENDED),
        (|s| s.early_stops = 1, CHANGE_EXTENDED),
        (|s| s.cmd_rejected = 1, CHANGE_EXTENDED),
        (|s| s.move_seq = 1, CHANGE_LAST_MOVE),
        (|s| s.sync = TargetSync::Pending, CHANGE_SYNC),
        (|s| s.stm_target_known = true, CHANGE_SYNC),
        (|s| s.stm_target = 1, CHANGE_SYNC),
        (|s| s.sensor_id[0].b[3] = 1, CHANGE_SENSORS),
        (|s| s.sensor_id[1].b[0] = 1, CHANGE_SENSORS),
        (|s| s.sensor_slot[0] = 1, CHANGE_SENSORS),
        (|s| s.sensor_slot[1] = 1, CHANGE_SENSORS),
        (|s| s.health = HEALTH_STALE, CHANGE_HEALTH),
        (|s| s.known = true, CHANGE_KNOWN),
    ];
    for (n, (mutate, bit)) in cases.into_iter().enumerate() {
        let mut b = ValveState::default();
        mutate(&mut b);
        assert_eq!(diff_valve(&a, &b), bit, "case {n}");
        assert_eq!(diff_valve(&b, &a), bit, "case {n}");
    }
    // Bookkeeping fields are not change groups.
    let b = ValveState {
        last_seen_ms: 5,
        last_push_ms: 5,
        revision: 9,
        push_attempts: 3,
        source: TargetSource::Web,
        ..ValveState::default()
    };
    assert_eq!(diff_valve(&a, &b), 0);
    let c = ValveState {
        status: 2,
        position: 9,
        ..ValveState::default()
    };
    assert_eq!(diff_valve(&a, &c), CHANGE_STATUS | CHANGE_POSITION);
}

#[test]
fn valve_bounds_and_defaults() {
    let mut m = ValveModel::default();
    assert_eq!(m.active_mask(), 0);
    assert!(!m.valve(0).known);
    assert_eq!(m.valve(11).sync, TargetSync::Unknown);
    assert!(core::ptr::eq(m.valve(12), m.valve(255)));
    assert!(!m.valve(12).known);
    assert!(!m.any_calibrating());
    assert!(!m.is_busy(0));
    assert!(!m.is_busy(12));
    assert_eq!(m.next_verify(), None);
    assert_eq!(m.next_target_push(0), None);
}

#[test]
fn set_active_mask_keeps_only_12_bits() {
    let mut m = ValveModel::default();
    m.set_active_mask(0xFFFF);
    assert_eq!(m.active_mask(), 0x0FFF);
    m.set_active_mask(0x8001);
    assert_eq!(m.active_mask(), 0x0001);
    m.set_active_mask(0);
    assert_eq!(m.active_mask(), 0);
}

#[test]
fn set_desired_target_validates_its_input() {
    let mut m = ValveModel::default();
    m.set_active_mask(0x0FFF);
    assert!(!m.set_desired_target(12, 10, TargetSource::Web, 0));
    assert!(!m.set_desired_target(255, 10, TargetSource::Web, 0));
    assert!(!m.set_desired_target(0, 101, TargetSource::Web, 0));
    assert!(!m.set_desired_target(0, 255, TargetSource::Web, 0));
    assert_eq!(m.valve(0).revision, 0);
    assert!(!m.valve(0).desired_valid);
    m.set_active_mask(0x0FFE);
    assert!(!m.set_desired_target(0, 10, TargetSource::Web, 0));
    assert!(!m.valve(0).desired_valid);

    assert!(m.set_desired_target(11, 100, TargetSource::Mqtt, 0));
    assert!(m.valve(11).desired_valid);
    assert_eq!(m.valve(11).desired, 100);
    assert_eq!(m.valve(11).source, TargetSource::Mqtt);
    assert_eq!(m.valve(11).sync, TargetSync::Pending);
    assert_eq!(m.valve(11).revision, 1);
    assert!(m.set_desired_target(1, 0, TargetSource::Web, 0));
    assert_eq!(m.valve(1).desired, 0);
}

#[test]
fn target_push_ack_and_verify_cycle() {
    let mut m = ValveModel::default();
    m.set_active_mask(0x001);
    assert!(m.set_desired_target(0, 60, TargetSource::Web, 100));
    // Unknown valve (no data yet): no push.
    assert_eq!(m.next_target_push(200), None);
    assert!(m.is_busy(0)); // Pending
    m.apply_valve_data(&data(0), 300);
    assert_eq!(m.next_target_push(400), Some((0, 60)));
    assert_eq!(m.valve(0).sync, TargetSync::AwaitAck);
    assert_eq!(m.valve(0).push_attempts, 1);
    assert_eq!(m.valve(0).last_push_ms, 400);
    assert!(!m.valve(0).stm_target_known);
    assert!(m.is_busy(0));
    // Nothing else to push while awaiting the ack.
    assert_eq!(m.next_target_push(10000), None);
    assert_eq!(m.next_verify(), None);

    m.on_target_ack(0, 450);
    assert_eq!(m.valve(0).sync, TargetSync::AwaitVerify);
    assert!(m.is_busy(0));
    assert_eq!(m.next_verify(), Some(0));

    let rev = m.valve(0).revision;
    m.apply_target(&target(0, 60), 500);
    assert_eq!(m.valve(0).sync, TargetSync::Synced);
    assert_eq!(m.valve(0).push_attempts, 0);
    assert!(m.valve(0).stm_target_known);
    assert_eq!(m.valve(0).stm_target, 60);
    assert_eq!(m.valve(0).source, TargetSource::Web);
    assert_eq!(m.valve(0).revision, rev + 1);
    assert_eq!(m.next_verify(), None);
    assert!(!m.is_busy(0));
    assert_eq!(m.next_target_push(100000), None);
}

#[test]
fn read_back_mismatch_after_the_ack_re_pushes_spaced_by_push_retry_ms() {
    let mut m = ValveModel::default();
    m.set_active_mask(0x001);
    m.apply_valve_data(&data(0), 0);
    m.set_desired_target(0, 70, TargetSource::Mqtt, 0);
    assert!(m.next_target_push(1000).is_some());
    m.on_target_ack(0, 1010);
    m.apply_target(&target(0, 30), 1020); // STM dropped it (calibrating)
    assert_eq!(m.valve(0).sync, TargetSync::Pending);
    assert_eq!(m.valve(0).push_attempts, 1); // attempts keep counting
    assert_eq!(m.valve(0).stm_target, 30);
    assert_eq!(m.next_target_push(2999), None);
    assert!(m.next_target_push(3000).is_some());
    assert_eq!(m.valve(0).push_attempts, 2);
}

#[test]
fn five_timed_out_pushes_end_in_failed_retried_after_failed_retry_ms() {
    let params = ValveModelParams::default();
    let mut m = ValveModel::new(params);
    m.set_active_mask(0x001);
    m.apply_valve_data(&data(0), 0);
    m.set_desired_target(0, 20, TargetSource::Web, 0);
    let mut now = 10000;
    for i in 1..=5u8 {
        assert!(m.next_target_push(now).is_some());
        assert_eq!(m.valve(0).push_attempts, i);
        m.on_target_timeout(0, now + 400);
        if i < 5 {
            assert_eq!(m.valve(0).sync, TargetSync::Pending);
            assert_eq!(m.valve(0).health & HEALTH_TARGET_UNCONFIRMED, 0);
            assert_eq!(m.next_target_push(now + params.push_retry_ms - 1), None);
        }
        now += params.push_retry_ms;
    }
    assert_eq!(m.valve(0).sync, TargetSync::Failed);
    assert_ne!(m.valve(0).health & HEALTH_TARGET_UNCONFIRMED, 0);
    assert!(!m.is_busy(0));
    let last_push = m.valve(0).last_push_ms;
    assert_eq!(
        m.next_target_push(last_push + params.failed_retry_ms - 1),
        None
    );
    assert!(m
        .next_target_push(last_push + params.failed_retry_ms)
        .is_some());
    assert_eq!(m.valve(0).sync, TargetSync::AwaitAck);
    assert_eq!(m.valve(0).push_attempts, 1);
    // Still unconfirmed while the retry is in progress: no flapping.
    assert_ne!(m.valve(0).health & HEALTH_TARGET_UNCONFIRMED, 0);
    m.on_target_timeout(0, last_push + params.failed_retry_ms + 400);
    assert_eq!(m.valve(0).sync, TargetSync::Pending);
    assert_ne!(m.valve(0).health & HEALTH_TARGET_UNCONFIRMED, 0);
    // A read-back that matches clears it.
    m.apply_target(&target(0, 20), last_push + params.failed_retry_ms + 500);
    assert_eq!(m.valve(0).sync, TargetSync::Synced);
    assert_eq!(m.valve(0).health & HEALTH_TARGET_UNCONFIRMED, 0);
    // Once confirmed, a later mismatch is an ordinary re-push, not unconfirmed.
    m.apply_target(&target(0, 21), last_push + params.failed_retry_ms + 600);
    assert_eq!(m.valve(0).sync, TargetSync::Pending);
    assert_eq!(m.valve(0).health & HEALTH_TARGET_UNCONFIRMED, 0);
}

#[test]
fn custom_max_push_attempts() {
    let mut m = ValveModel::new(ValveModelParams {
        max_push_attempts: 1,
        ..ValveModelParams::default()
    });
    m.set_active_mask(0x001);
    m.apply_valve_data(&data(0), 0);
    m.set_desired_target(0, 20, TargetSource::Web, 0);
    assert!(m.next_target_push(0).is_some());
    m.on_target_timeout(0, 400);
    assert_eq!(m.valve(0).sync, TargetSync::Failed);
}

#[test]
fn set_desired_target_on_a_failed_valve_re_arms_same_value_on_synced_is_a_no_op() {
    let mut m = ValveModel::new(ValveModelParams {
        max_push_attempts: 1,
        ..ValveModelParams::default()
    });
    m.set_active_mask(0x001);
    m.apply_valve_data(&data(0), 0);
    m.set_desired_target(0, 20, TargetSource::Web, 0);
    assert!(m.next_target_push(0).is_some());
    m.on_target_timeout(0, 400);
    assert_eq!(m.valve(0).sync, TargetSync::Failed);
    assert!(m.set_desired_target(0, 20, TargetSource::Mqtt, 500));
    assert_eq!(m.valve(0).sync, TargetSync::Pending);
    assert_eq!(m.valve(0).push_attempts, 0);
    assert_eq!(m.valve(0).source, TargetSource::Mqtt);
    assert_eq!(m.valve(0).health & HEALTH_TARGET_UNCONFIRMED, 0);

    let mut s = synced_model();
    let before = *s.valve(0);
    assert!(s.set_desired_target(0, 50, TargetSource::Mqtt, 2000));
    assert_eq!(s.valve(0).revision, before.revision);
    assert_eq!(s.valve(0).sync, TargetSync::Synced);
    assert_eq!(s.valve(0).source, TargetSource::Stm);
    // Same value while Pending stays Pending with its attempts.
    let mut q = ValveModel::default();
    q.set_active_mask(1);
    q.apply_valve_data(&data(0), 0);
    q.set_desired_target(0, 5, TargetSource::Web, 0);
    assert!(q.next_target_push(0).is_some());
    q.on_target_timeout(0, 400);
    assert!(q.set_desired_target(0, 5, TargetSource::Web, 500));
    assert_eq!(q.valve(0).sync, TargetSync::Pending);
    assert_eq!(q.valve(0).push_attempts, 1);
}

#[test]
fn a_new_desired_value_equal_to_the_known_stm_target_is_synced_at_once() {
    let mut m = synced_model();
    assert!(m.set_desired_target(0, 30, TargetSource::Web, 2000));
    assert_eq!(m.valve(0).sync, TargetSync::Pending);
    // Back to 50 before the push: the STM still has 50.
    assert!(m.set_desired_target(0, 50, TargetSource::Web, 2100));
    assert_eq!(m.valve(0).sync, TargetSync::Synced);
    assert_eq!(m.next_target_push(9000), None);

    // While a push is in flight the STM target is unknown: go Pending.
    assert!(m.set_desired_target(0, 30, TargetSource::Web, 9000));
    assert!(m.next_target_push(9000).is_some());
    assert!(m.set_desired_target(0, 50, TargetSource::Web, 9100));
    assert_eq!(m.valve(0).sync, TargetSync::Pending);
    assert_eq!(m.valve(0).push_attempts, 0);
    // The stale ack/timeout of the old push is ignored (sync is not AwaitAck).
    m.on_target_ack(0, 9200);
    assert_eq!(m.valve(0).sync, TargetSync::Pending);
    m.on_target_timeout(0, 9200);
    assert_eq!(m.valve(0).sync, TargetSync::Pending);

    // A read-back that arrives while the stgtp is in flight does not make the old value
    // current again.
    let mut r = synced_model();
    assert!(r.set_desired_target(0, 30, TargetSource::Web, 2000));
    assert!(r.next_target_push(4000).is_some());
    r.apply_target(&target(0, 50), 4050);
    assert!(r.valve(0).stm_target_known);
    assert!(r.set_desired_target(0, 50, TargetSource::Web, 4060));
    assert_eq!(r.valve(0).sync, TargetSync::Pending);

    // In AwaitVerify as well.
    let mut n = synced_model();
    assert!(n.set_desired_target(0, 30, TargetSource::Web, 2000));
    assert!(n.next_target_push(4000).is_some());
    n.on_target_ack(0, 4100);
    n.apply_target(&target(0, 50), 4150); // STM still reports the old value
    assert_eq!(n.valve(0).sync, TargetSync::Pending);
    assert!(n.next_target_push(6100).is_some());
    n.on_target_ack(0, 6200);
    assert_eq!(n.valve(0).sync, TargetSync::AwaitVerify);
    assert!(n.set_desired_target(0, 50, TargetSource::Web, 6300));
    assert_eq!(n.valve(0).sync, TargetSync::Pending);
}

#[test]
fn read_back_handling_per_sync_state() {
    // Adoption at ESP boot.
    let mut m = ValveModel::default();
    m.set_active_mask(0x003);
    m.apply_target(&target(1, 42), 10);
    assert!(m.valve(1).desired_valid);
    assert_eq!(m.valve(1).desired, 42);
    assert_eq!(m.valve(1).source, TargetSource::Stm);
    assert_eq!(m.valve(1).sync, TargetSync::Synced);
    assert!(m.valve(1).stm_target_known);
    assert_eq!(m.valve(1).revision, 1);
    // Synced + differing read-back -> Pending.
    m.apply_target(&target(1, 43), 20);
    assert_eq!(m.valve(1).sync, TargetSync::Pending);
    assert_eq!(m.valve(1).desired, 42);
    // Pending + equal read-back -> Synced.
    m.apply_target(&target(1, 42), 30);
    assert_eq!(m.valve(1).sync, TargetSync::Synced);
    // Boundary values are accepted.
    m.apply_target(&target(0, 100), 35);
    assert_eq!(m.valve(0).desired, 100);
    m.apply_target(&target(0, 0), 36);
    assert_eq!(m.valve(0).stm_target, 0);
    // Out-of-range replies are ignored.
    m.apply_target(&target(12, 42), 40);
    m.apply_target(&target(1, 101), 40);
    assert_eq!(m.valve(1).stm_target, 42);
    assert_eq!(m.valve(1).revision, 3);

    // AwaitAck: the read-back may predate the push; sync unchanged.
    m.apply_target(&target(0, 100), 45); // valve 0 synced: the push below is valve 1's
    m.apply_valve_data(&data(1), 50);
    m.set_desired_target(1, 10, TargetSource::Web, 60);
    assert!(m.next_target_push(70).is_some());
    m.apply_target(&target(1, 10), 80);
    assert_eq!(m.valve(1).sync, TargetSync::AwaitAck);
    assert!(m.valve(1).stm_target_known);
    assert_eq!(m.valve(1).stm_target, 10);

    // Failed + differing read-back stays Failed; equal -> Synced.
    let mut f = ValveModel::new(ValveModelParams {
        max_push_attempts: 1,
        ..ValveModelParams::default()
    });
    f.set_active_mask(1);
    f.apply_valve_data(&data(0), 0);
    f.set_desired_target(0, 90, TargetSource::Web, 0);
    assert!(f.next_target_push(0).is_some());
    f.on_target_timeout(0, 400);
    f.apply_target(&target(0, 80), 500);
    assert_eq!(f.valve(0).sync, TargetSync::Failed);
    f.apply_target(&target(0, 90), 600);
    assert_eq!(f.valve(0).sync, TargetSync::Synced);
    assert_eq!(f.valve(0).push_attempts, 0);
}

#[test]
fn pushes_wait_for_calibration_and_skip_inactive_valves() {
    let mut m = ValveModel::default();
    m.set_active_mask(0x003);
    let mut d0 = data(0);
    d0.calibrating = true;
    m.apply_valve_data(&d0, 0);
    m.apply_valve_data(&data(1), 0);
    m.set_desired_target(0, 10, TargetSource::Web, 0);
    m.set_desired_target(1, 20, TargetSource::Web, 0);
    m.set_active_mask(0x001); // valve 1 inactive now
    assert_eq!(m.next_target_push(100), None);
    assert!(m.any_calibrating());
    assert!(m.is_busy(0));
    d0.calibrating = false;
    m.apply_valve_data(&d0, 200);
    assert!(!m.any_calibrating());
    assert_eq!(m.next_target_push(300), Some((0, 10)));
    assert_eq!(m.next_target_push(300), None);
    assert_eq!(m.valve(1).sync, TargetSync::Pending);
}

#[test]
fn v2_targets_are_pushed_to_calibrating_valves_when_not_held() {
    let mut m = ValveModel::default();
    m.set_active_mask(0x001);
    let mut x = ex(0, 30, 0, 0); // calState 2: running
    x.calibrating = true;
    m.apply_valve_ex(&x, 0);
    assert!(m.set_desired_target(0, 70, TargetSource::Mqtt, 0));
    assert_eq!(m.next_target_push(100), None); // default: held like 1.x
    m.set_hold_targets_while_calibrating(false);
    assert_eq!(m.next_target_push(200), Some((0, 70)));
    m.on_target_ack(0, 250);
    x.target = 70;
    m.apply_valve_ex(&x, 300); // the STM keeps the target while calibrating
    assert_eq!(m.valve(0).sync, TargetSync::Synced);
    assert!(m.is_busy(0)); // still calibrating: fast polling
    m.set_hold_targets_while_calibrating(true);
    assert!(m.set_desired_target(0, 71, TargetSource::Mqtt, 400));
    assert_eq!(m.next_target_push(10000), None);
}

#[test]
fn a_push_that_could_not_be_queued_goes_back_to_pending() {
    let mut m = synced_model();
    assert!(m.set_desired_target(0, 80, TargetSource::Web, 2000));
    assert!(m.next_target_push(3000).is_some());
    assert_eq!(m.valve(0).push_attempts, 1);
    let rev = m.valve(0).revision;
    m.on_target_push_dropped(0, 3000);
    assert_eq!(m.valve(0).sync, TargetSync::Pending);
    assert_eq!(m.valve(0).push_attempts, 0);
    assert_eq!(m.valve(0).revision, rev + 1);
    assert!(m.is_busy(0));
    // Retried after pushRetryMs, and dropping forever never reaches Failed.
    assert_eq!(m.next_target_push(4999), None);
    let mut now = 5000;
    for _ in 0..20 {
        let pushed = m.next_target_push(now);
        assert!(pushed.is_some());
        assert_eq!(pushed.map(|(_, pos)| pos), Some(80));
        m.on_target_push_dropped(0, now);
        assert_eq!(m.valve(0).sync, TargetSync::Pending);
        now += 2000;
    }
    assert_eq!(m.valve(0).health, 0);
    // Queued at last: the normal cycle continues.
    assert!(m.next_target_push(now).is_some());
    assert_eq!(m.valve(0).push_attempts, 1);
    m.on_target_ack(0, now);
    m.apply_target(&target(0, 80), now);
    assert_eq!(m.valve(0).sync, TargetSync::Synced);

    // Only a valve in AwaitAck is affected; bad indices are ignored.
    let rev2 = m.valve(0).revision;
    m.on_target_push_dropped(0, now);
    m.on_target_push_dropped(12, now);
    assert_eq!(m.valve(0).sync, TargetSync::Synced);
    assert_eq!(m.valve(0).revision, rev2);
}

#[test]
fn a_dropped_retry_of_a_failed_delivery_keeps_the_flag_and_attempt_count_at_0() {
    let mut m = synced_model();
    assert!(m.set_desired_target(0, 90, TargetSource::Web, 2000));
    let mut now = 3000;
    for _ in 0..5 {
        assert!(m.next_target_push(now).is_some());
        m.on_target_timeout(0, now);
        now += 2000;
    }
    assert_eq!(m.valve(0).sync, TargetSync::Failed);
    now += 300000;
    assert!(m.next_target_push(now).is_some()); // re-armed retry
    m.on_target_push_dropped(0, now);
    assert_eq!(m.valve(0).sync, TargetSync::Pending);
    assert_eq!(m.valve(0).push_attempts, 0);
    assert_ne!(m.valve(0).health & HEALTH_TARGET_UNCONFIRMED, 0);
    assert!(m.next_target_push(now + 2000).is_some());
    assert_eq!(m.valve(0).push_attempts, 1);
}

#[test]
fn target_pushes_rotate_round_robin_over_valves() {
    let mut m = ValveModel::default();
    m.set_active_mask(0x0FFF);
    for i in 0..VALVE_COUNT {
        m.apply_valve_data(&data(i), 0);
        m.set_desired_target(i, i + 1, TargetSource::Web, 0);
    }
    for i in 0..VALVE_COUNT {
        assert_eq!(m.next_target_push(0), Some((i, i + 1)));
        m.on_target_timeout(i, 10);
    }
    // All just pushed: spacing blocks everything.
    assert_eq!(m.next_target_push(1999), None);
    // Cursor wrapped to valve 0; valve 0 first again, then 1.
    assert_eq!(push_valve(&mut m, 2000), Some(0));
    assert_eq!(push_valve(&mut m, 2000), Some(1));
    // The cursor moves past the valve just pushed.
    let mut c = ValveModel::default();
    c.set_active_mask(0x003);
    for i in 0..2 {
        c.apply_valve_data(&data(i), 0);
        c.set_desired_target(i, 9, TargetSource::Web, 0);
    }
    assert_eq!(push_valve(&mut c, 0), Some(0));
    c.on_target_timeout(0, 1);
    assert_eq!(push_valve(&mut c, 5000), Some(1));
    c.on_target_timeout(1, 5001);
    assert_eq!(push_valve(&mut c, 10000), Some(0));
    // Starting after valve 11 wraps to 0.
    let mut w = ValveModel::default();
    w.set_active_mask(0x0801);
    w.apply_valve_data(&data(0), 0);
    w.apply_valve_data(&data(11), 0);
    w.set_desired_target(11, 5, TargetSource::Web, 0);
    assert_eq!(push_valve(&mut w, 0), Some(11));
    w.set_desired_target(0, 6, TargetSource::Web, 0);
    assert_eq!(push_valve(&mut w, 0), Some(0));
}

#[test]
fn next_verify_returns_the_lowest_valve_awaiting_verification() {
    let mut m = ValveModel::default();
    m.set_active_mask(0x0FFF);
    for i in [3, 7] {
        m.apply_valve_data(&data(i), 0);
        m.set_desired_target(i, 1, TargetSource::Web, 0);
        assert!(m.next_target_push(0).is_some());
        m.on_target_ack(i, 1);
    }
    assert_eq!(m.next_verify(), Some(3));
    m.apply_target(&target(3, 1), 2);
    assert_eq!(m.next_verify(), Some(7));
    // Out-of-range ack/timeout are ignored.
    m.on_target_ack(12, 0);
    m.on_target_timeout(12, 0);
    // Ack/timeout for a valve that is not awaiting one are ignored.
    let rev = m.valve(3).revision;
    assert_eq!(m.valve(3).sync, TargetSync::Synced);
    m.on_target_timeout(3, 5);
    m.on_target_ack(3, 5);
    assert_eq!(m.valve(3).sync, TargetSync::Synced);
    assert_eq!(m.valve(3).revision, rev);
    assert_eq!(m.valve(7).sync, TargetSync::AwaitVerify);
    m.on_target_timeout(7, 5);
    assert_eq!(m.valve(7).sync, TargetSync::AwaitVerify);
}

#[test]
fn on_stm_rebooted_re_pushes_desired_targets_and_forgets_stm_targets() {
    let mut m = synced_model();
    m.set_active_mask(0x003);
    m.apply_valve_data(&data(1), 0);
    m.apply_valve_ex(&ex(0, 50, 5, 6), 1000);
    assert_eq!(m.valve(0).early_stops_at_boot, 5);
    let rev0 = m.valve(0).revision;
    let rev1 = m.valve(1).revision;
    m.on_stm_rebooted(2000);
    assert_eq!(m.valve(0).sync, TargetSync::Pending);
    assert_eq!(m.valve(0).desired, 50);
    assert!(!m.valve(0).stm_target_known);
    assert_eq!(m.valve(0).stm_target, 0);
    assert_eq!(m.valve(0).revision, rev0 + 1);
    assert_eq!(m.valve(1).sync, TargetSync::Unknown);
    assert_eq!(m.valve(1).revision, rev1); // nothing changed for valve 1

    // v2 counters re-baseline on the next gvlvx (STM counters restarted).
    m.apply_valve_ex(&ex(0, 50, 1, 0), 3000);
    assert_eq!(m.valve(0).early_stops_at_boot, 1);
    assert_eq!(m.valve(0).cmd_rejected_at_boot, 0);
    assert_eq!(m.valve(0).health & HEALTH_EARLY_STOP, 0);
    // Read-back equal, but the rebooted STM gets the target once more.
    assert_eq!(m.valve(0).sync, TargetSync::Pending);
    assert!(m.valve(0).force_push);
    // Valve without desired target adopts again.
    m.apply_target(&target(1, 77), 3100);
    assert_eq!(m.valve(1).desired, 77);
    assert_eq!(m.valve(1).source, TargetSource::Stm);
    // Every valve is handled, and baselines move up as well as down.
    m.apply_valve_ex(&ex(0, 50, 9, 9), 3200);
    assert_ne!(m.valve(0).health & HEALTH_EARLY_STOP, 0);
    m.on_stm_rebooted(3300);
    assert_eq!(m.valve(1).sync, TargetSync::Pending);
    assert_eq!(m.valve(0).sync, TargetSync::Pending);
    m.apply_valve_ex(&ex(0, 50, 12, 11), 3400);
    assert_eq!(m.valve(0).early_stops_at_boot, 12);
    assert_eq!(m.valve(0).cmd_rejected_at_boot, 11);
    assert_eq!(
        m.valve(0).health & (HEALTH_EARLY_STOP | HEALTH_CMD_REJECTED),
        0
    );
}

#[test]
fn apply_valve_data_copies_every_field_and_ignores_bad_indices() {
    let mut m = ValveModel::default();
    m.set_active_mask(0x001);
    let mut d = status_data(0, 2);
    d.calibrating = true;
    d.temp2 = 199;
    d.calib_retries = 2;
    m.apply_valve_data(&d, 1234);
    let v = *m.valve(0);
    assert!(v.known);
    assert_eq!(v.last_seen_ms, 1234);
    assert_eq!(v.status, 2);
    assert!(v.calibrating);
    assert_eq!(v.position, 40);
    assert_eq!(v.mean_current, 12);
    assert_eq!(v.temp1, 215);
    assert_eq!(v.temp2, 199);
    assert_eq!(v.moves, 7);
    assert_eq!(v.open_count, 3000);
    assert_eq!(v.close_count, 3100);
    assert_eq!(v.dead_zone, -4);
    assert_eq!(v.calib_retries, 2);
    assert!(!v.has_extended);
    assert_eq!(v.revision, 1);
    // Identical data later: only the timestamp moves, no revision bump.
    m.apply_valve_data(&d, 5000);
    assert_eq!(m.valve(0).last_seen_ms, 5000);
    assert_eq!(m.valve(0).revision, 1);
    let mut bad = data(12);
    m.apply_valve_data(&bad, 1);
    bad.valve = 255;
    m.apply_valve_data(&bad, 1);
    assert!(!m.valve(12).known);
}

#[test]
fn apply_valve_ex_extended_data_baselines_move_seq_and_read_back() {
    let mut m = ValveModel::default();
    m.set_active_mask(0x001);
    let mut e = ex(0, 25, 3, 4);
    e.last_move = MoveResult {
        dir: MoveDir::Close,
        requested_counts: 100,
        counted_counts: 90,
        stop: StopReason::EarlyEndStop,
        peak_current: 321,
        duration_ms: 4000,
    };
    m.apply_valve_ex(&e, 77);
    let v = *m.valve(0);
    assert!(v.known);
    assert_eq!(v.last_seen_ms, 77);
    assert!(v.has_extended);
    assert_eq!(v.status, 1);
    assert_eq!(v.position, 33);
    assert_eq!(v.mean_current, 14);
    assert_eq!(v.open_count, 11);
    assert_eq!(v.close_count, 12);
    assert_eq!(v.dead_zone, -3);
    assert_eq!(v.calib_retries, 1);
    assert_eq!(v.moves, 99);
    assert_eq!(v.cal_state, 2);
    assert_eq!(v.cal_flags, CAL_FLAG_EARLY_STOP);
    assert_eq!(v.early_stops, 3);
    assert_eq!(v.cmd_rejected, 4);
    assert_eq!(v.early_stops_at_boot, 3);
    assert_eq!(v.cmd_rejected_at_boot, 4);
    assert_eq!(v.move_seq, 1);
    assert_eq!(v.last_move.stop, StopReason::EarlyEndStop);
    assert_eq!(v.last_move.peak_current, 321);
    // Read-back adopted the target.
    assert_eq!(v.desired, 25);
    assert_eq!(v.source, TargetSource::Stm);
    assert_eq!(v.stm_target, 25);
    assert_eq!(v.health & (HEALTH_EARLY_STOP | HEALTH_CMD_REJECTED), 0);
    assert_ne!(v.health & HEALTH_CALIB_RETRIES, 0);

    // Same move again: no moveSeq change; each move field counts.
    m.apply_valve_ex(&e, 78);
    assert_eq!(m.valve(0).move_seq, 1);
    let mut seq = 1;
    for field in 0..6 {
        let mut n = e;
        match field {
            0 => n.last_move.dir = MoveDir::Open,
            1 => n.last_move.requested_counts = 101,
            2 => n.last_move.counted_counts = 91,
            3 => n.last_move.stop = StopReason::Target,
            4 => n.last_move.peak_current = 1,
            _ => n.last_move.duration_ms = 1,
        }
        m.apply_valve_ex(&n, 79);
        seq += 1;
        assert_eq!(m.valve(0).move_seq, seq, "field {field}");
        m.apply_valve_ex(&e, 80);
        seq += 1;
        assert_eq!(m.valve(0).move_seq, seq, "field {field}");
    }

    // Counter increases raise flags.
    m.apply_valve_ex(&ex(0, 25, 4, 4), 90);
    assert_ne!(m.valve(0).health & HEALTH_EARLY_STOP, 0);
    assert_eq!(m.valve(0).health & HEALTH_CMD_REJECTED, 0);
    m.apply_valve_ex(&ex(0, 25, 4, 5), 91);
    assert_ne!(m.valve(0).health & HEALTH_CMD_REJECTED, 0);
    // A counter below its baseline re-baselines (unnoticed STM reboot).
    m.apply_valve_ex(&ex(0, 25, 2, 5), 92);
    assert_eq!(m.valve(0).early_stops_at_boot, 2);
    assert_eq!(m.valve(0).cmd_rejected_at_boot, 5);
    assert_eq!(
        m.valve(0).health & (HEALTH_EARLY_STOP | HEALTH_CMD_REJECTED),
        0
    );
    m.apply_valve_ex(&ex(0, 25, 3, 1), 93);
    assert_eq!(m.valve(0).early_stops_at_boot, 3);
    assert_eq!(m.valve(0).cmd_rejected_at_boot, 1);

    // One counter at its baseline while the other rises: no re-baseline.
    m.apply_valve_ex(&ex(0, 25, 3, 2), 95);
    assert_eq!(m.valve(0).cmd_rejected_at_boot, 1);
    assert_ne!(m.valve(0).health & HEALTH_CMD_REJECTED, 0);
    m.apply_valve_ex(&ex(0, 25, 4, 1), 96);
    assert_eq!(m.valve(0).early_stops_at_boot, 3);
    assert_eq!(m.valve(0).cmd_rejected_at_boot, 1);
    assert_ne!(m.valve(0).health & HEALTH_EARLY_STOP, 0);

    // Target 100 is a valid read-back.
    m.apply_valve_ex(&ex(0, 100, 4, 1), 97);
    assert_eq!(m.valve(0).stm_target, 100);

    // Bad replies are ignored entirely.
    let rev = m.valve(0).revision;
    m.apply_valve_ex(&ex(12, 25, 0, 0), 94);
    m.apply_valve_ex(&ex(0, 101, 9, 9), 94);
    assert_eq!(m.valve(0).revision, rev);
    assert_eq!(m.valve(0).early_stops, 4);
}

#[test]
fn apply_valve_states_only_fills_unknown_valves() {
    let mut m = ValveModel::default();
    m.set_active_mask(0x0FFF);
    m.apply_valve_data(&status_data(0, 1), 0);
    let mut s = ValveStates::default();
    for (i, status) in (0u8..).zip(s.status.iter_mut()) {
        *status = if i < 10 { i } else { 0x89 };
    }
    m.apply_valve_states(&s, 50);
    assert_eq!(m.valve(0).status, 1); // known from gvlvd: untouched
    assert_eq!(m.valve(0).revision, 1);
    for i in 1..10 {
        assert!(m.valve(i).known);
        assert_eq!(m.valve(i).status, i);
        assert_eq!(m.valve(i).last_seen_ms, 0); // not proof of fresh data
    }
    assert_eq!(m.valve(10).status, 9); // 0x89 masked to 7 bits
    assert_eq!(m.valve(11).status, 9);
    assert_ne!(m.valve(10).health & HEALTH_BLOCKED, 0);
    s.status[1] = 4;
    m.apply_valve_states(&s, 60);
    assert_eq!(m.valve(1).status, 1);
    let mut fresh = ValveModel::default();
    s.status[0] = 8;
    fresh.apply_valve_states(&s, 0);
    assert!(fresh.valve(0).known);
    assert_eq!(fresh.valve(0).status, 8);
    assert_eq!(fresh.valve(0).revision, 1);
}

#[test]
fn apply_valve_sensors_resolves_ids_to_config_slots() {
    let mut slots = [OneWireId::default(); TEMP_SLOT_COUNT as usize];
    slots[4] = id_with_crc(1);
    slots[9] = id_with_crc(2);
    let garbage = id_with_crc(3); // valid CRC, not configured

    let mut m = ValveModel::default();
    m.set_active_mask(0x0FFF);
    let mut single = ValveSensors {
        is_list: false,
        valve: 2,
        ..ValveSensors::default()
    };
    single.ids[2][0] = slots[4];
    single.ids[2][1] = garbage;
    single.ids[3][0] = slots[9]; // ignored in the single form
    m.apply_valve_sensors(&single, &slots);
    assert_eq!(m.valve(2).sensor_id[0], slots[4]);
    assert_eq!(m.valve(2).sensor_id[1], garbage);
    assert_eq!(m.valve(2).sensor_slot[0], 5);
    assert_eq!(m.valve(2).sensor_slot[1], 0);
    assert_eq!(m.valve(2).revision, 1);
    assert!(is_zero(&m.valve(3).sensor_id[0]));
    assert_eq!(m.valve(3).revision, 0);

    // Out-of-range single form: ignored.
    single.valve = 12;
    m.apply_valve_sensors(&single, &slots);
    assert_eq!(m.valve(2).revision, 1);

    let mut list = ValveSensors {
        is_list: true,
        valve: 200, // ignored in the list form
        ..ValveSensors::default()
    };
    for (i, ids) in list.ids.iter_mut().enumerate() {
        ids[0] = if i % 2 == 1 {
            slots[9]
        } else {
            OneWireId::default()
        };
    }
    list.ids[0][0] = slots[4];
    list.ids[11][1] = slots[4];
    m.apply_valve_sensors(&list, &slots);
    for (i, ids) in (0..VALVE_COUNT).zip(list.ids) {
        assert_eq!(m.valve(i).sensor_id[0], ids[0]);
        let slot = if i == 0 {
            5
        } else if i % 2 == 1 {
            10
        } else {
            0
        };
        assert_eq!(m.valve(i).sensor_slot[0], slot, "valve {i}");
        assert_eq!(m.valve(i).sensor_id[1], ids[1]);
    }
    assert_eq!(m.valve(11).sensor_slot[1], 5);
    assert_eq!(m.valve(10).sensor_slot[1], 0);
    // A one-slot table resolves its only slot.
    let mut one = ValveSensors {
        valve: 7,
        ..ValveSensors::default()
    };
    one.ids[7][1] = slots[4];
    let table = [slots[4]];
    m.apply_valve_sensors(&one, &table);
    assert_eq!(m.valve(7).sensor_slot[1], 1);
    // No slot table: slots stay 0 even for configured ids.
    list.ids[1][0] = slots[4];
    // C++ applyValveSensors(list, nullptr, kTempSlotCount): no Rust form, the empty table is.
    m.apply_valve_sensors(&list, &[]);
    assert_eq!(m.valve(1).sensor_slot[0], 0);
    m.apply_valve_sensors(&list, &slots[..0]);
    assert_eq!(m.valve(1).sensor_slot[0], 0);
}

#[test]
fn apply_sensor_temps_v2_valve_temperatures_from_gvlon_and_goned() {
    let mut s = SensorModel::default();
    let mut l = OneWireList {
        count: 3,
        has_list: true,
        ..OneWireList::default()
    };
    l.ids[0] = id_with_crc(1);
    l.ids[1] = id_with_crc(2);
    l.ids[2] = id_with_crc(3);
    s.apply_temp_list(&l, 0);

    let mut m = ValveModel::default();
    m.set_active_mask(0x00F);
    let mut vs = ValveSensors {
        is_list: true,
        ..ValveSensors::default()
    };
    vs.ids[0][0] = id_with_crc(1);
    vs.ids[0][1] = id_with_crc(2);
    vs.ids[1][0] = id_with_crc(3);
    vs.ids[2][0] = id_with_crc(7); // not on the bus
    m.apply_valve_sensors(&vs, &[]);

    // Nothing read yet: nothing to publish.
    let rev0 = m.valve(0).revision;
    m.apply_sensor_temps(&s, 1000, 60000, false);
    assert_eq!(m.valve(0).temp1, TEMP_UNASSIGNED);
    assert_eq!(m.valve(0).revision, rev0);

    read(&mut s, &l, 0, 215, 1000);
    read(&mut s, &l, 1, -1270, 1000); // the STM reports a read error for this sensor twice
    read(&mut s, &l, 1, -1270, 1000);
    read(&mut s, &l, 2, 199, 1000);
    m.apply_sensor_temps(&s, 2000, 60000, false);
    assert_eq!(m.valve(0).temp1, 215);
    assert_eq!(m.valve(0).temp2, -1270);
    assert_eq!(m.valve(0).revision, rev0 + 1);
    assert_ne!(m.valve(0).health & HEALTH_TEMP_FAILED, 0);
    assert_eq!(m.valve(1).temp1, 199);
    assert_eq!(m.valve(1).temp2, TEMP_UNASSIGNED); // zero id
    assert_eq!(m.valve(2).temp1, TEMP_UNASSIGNED); // not on the bus, not settled
    assert_eq!(m.valve(3).temp1, TEMP_UNASSIGNED);
    // Unchanged readings: no revision bump.
    m.apply_sensor_temps(&s, 3000, 60000, false);
    assert_eq!(m.valve(0).revision, rev0 + 1);
    assert_ne!(
        diff_valve(&ValveState::default(), m.valve(1)) & CHANGE_TEMP1,
        0
    );

    // Readings older than maxAge are read errors; fresh ones come back.
    read(&mut s, &l, 0, 216, 50000);
    m.apply_sensor_temps(&s, 61001, 60000, false);
    assert_eq!(m.valve(0).temp1, 216);
    assert_eq!(m.valve(0).temp2, TEMP_READ_ERROR);
    assert_eq!(m.valve(1).temp1, TEMP_READ_ERROR);
    m.apply_sensor_temps(&s, 61000, 60000, false);
    assert_eq!(m.valve(1).temp1, 199); // exactly maxAge old: still fresh

    // A sensor that left the bus (list re-read without it).
    l.count = 1;
    s.apply_temp_list(&l, 62000);
    m.apply_sensor_temps(&s, 62000, 60000, false);
    assert_eq!(m.valve(0).temp1, 216);
    assert_eq!(m.valve(1).temp1, TEMP_UNASSIGNED); // gone, not settled
    m.apply_sensor_temps(&s, 62000, 60000, true);
    assert_eq!(m.valve(1).temp1, TEMP_READ_ERROR); // gone, settled
    assert_eq!(m.valve(2).temp1, TEMP_READ_ERROR); // never on the bus, settled
    assert_eq!(m.valve(0).temp1, 216);
    assert_eq!(m.valve(3).temp1, TEMP_UNASSIGNED); // nothing assigned stays unassigned
}

#[test]
fn apply_sensor_temps_a_sensor_on_the_bus_that_was_never_read_is_a_read_error_once_settled() {
    let mut s = SensorModel::default();
    let mut l = OneWireList {
        count: 2,
        has_list: true,
        ..OneWireList::default()
    };
    l.ids[0] = id_with_crc(1);
    l.ids[1] = id_with_crc(2);
    s.apply_temp_list(&l, 0);
    let td = TempData {
        valid: true,
        id: l.ids[1],
        value: 199,
    };
    s.apply_temp_data(1, &td, 1000);
    let mut m = ValveModel::default();
    m.set_active_mask(0x001);
    let mut vs = ValveSensors {
        is_list: true,
        ..ValveSensors::default()
    };
    vs.ids[0][0] = id_with_crc(1); // on the bus, never read
    vs.ids[0][1] = id_with_crc(2); // on the bus, fresh
    m.apply_valve_sensors(&vs, &[]);
    m.apply_sensor_temps(&s, 2000, 60000, false);
    assert_eq!(m.valve(0).temp1, TEMP_UNASSIGNED);
    assert_eq!(m.valve(0).temp2, 199);
    m.apply_sensor_temps(&s, 2000, 60000, true);
    assert_eq!(m.valve(0).temp1, TEMP_READ_ERROR);
    assert_eq!(m.valve(0).temp2, 199);
    assert_ne!(m.valve(0).health & HEALTH_TEMP_FAILED, 0);
}

#[test]
fn apply_sensor_temps_the_combined_table_per_sensor() {
    let mut s = SensorModel::default();
    let mut l = OneWireList {
        count: 2,
        has_list: true,
        ..OneWireList::default()
    };
    l.ids[0] = id_with_crc(1);
    l.ids[1] = id_with_crc(2);
    s.apply_temp_list(&l, 0);
    let mut m = ValveModel::default();
    m.set_active_mask(0x007);
    let mut vs = ValveSensors {
        is_list: true,
        ..ValveSensors::default()
    };
    vs.ids[0][0] = id_with_crc(1);
    vs.ids[0][1] = id_with_crc(2);
    let mut bad_crc = id_with_crc(1);
    bad_crc.b[7] ^= 1;
    vs.ids[1][0] = bad_crc;
    m.apply_valve_sensors(&vs, &[]);

    read(&mut s, &l, 0, 215, 1000);
    read(&mut s, &l, 1, 199, 1000);
    m.apply_sensor_temps(&s, 1000, 60000, true);
    assert_eq!(m.valve(0).temp1, 215);
    assert_eq!(m.valve(0).temp2, 199);
    assert_eq!(m.valve(1).temp1, TEMP_UNASSIGNED); // bad CRC: nothing assigned, even settled
    assert_eq!(m.valve(1).temp2, TEMP_UNASSIGNED); // zero id

    // One failed read after a good one: the previous value (debounce).
    read(&mut s, &l, 0, TEMP_READ_ERROR, 2000);
    m.apply_sensor_temps(&s, 2000, 60000, true);
    assert_eq!(m.valve(0).temp1, 215);
    assert_eq!(m.valve(0).temp2, 199); // sensor 2 independent of sensor 1
    read(&mut s, &l, 0, TEMP_READ_ERROR, 3000);
    m.apply_sensor_temps(&s, 3000, 60000, true);
    assert_eq!(m.valve(0).temp1, TEMP_READ_ERROR);
    assert_eq!(m.valve(0).temp2, 199);
    // The STM's temperature cycle is older than 200 s: every reading is stale.
    read(&mut s, &l, 0, 220, 4000);
    s.set_stm_temp_age(201, 4000);
    m.apply_sensor_temps(&s, 4000, 60000, true);
    assert_eq!(m.valve(0).temp1, TEMP_READ_ERROR);
    assert_eq!(m.valve(0).temp2, TEMP_READ_ERROR);
    s.set_stm_temp_age(0, 5000);
    m.apply_sensor_temps(&s, 5000, 60000, true);
    assert_eq!(m.valve(0).temp1, 220);
}

#[test]
fn health_flags_per_condition_and_activity() {
    let mut m = ValveModel::default();
    m.set_active_mask(0x001);
    m.apply_valve_data(&status_data(0, 9), 0);
    assert_eq!(m.valve(0).health, HEALTH_BLOCKED);
    m.apply_valve_data(&status_data(0, 4), 0);
    assert_eq!(m.valve(0).health, HEALTH_FAILED);
    m.apply_valve_data(&status_data(0, 6), 0);
    assert_eq!(m.valve(0).health, HEALTH_NO_VALVE);
    m.apply_valve_data(&status_data(0, 1), 0);
    assert_eq!(m.valve(0).health, 0);
    let mut d = status_data(0, 1);
    d.calib_retries = 1;
    m.apply_valve_data(&d, 0);
    assert_eq!(m.valve(0).health, HEALTH_CALIB_RETRIES);
    for bad in [TEMP_READ_ERROR, TEMP_POWER_ON, 1251, -551] {
        d = status_data(0, 1);
        d.temp1 = bad;
        m.apply_valve_data(&d, 0);
        assert_eq!(m.valve(0).health, HEALTH_TEMP_FAILED, "temp1 {bad}");
        d.temp1 = TEMP_UNASSIGNED;
        d.temp2 = bad;
        m.apply_valve_data(&d, 0);
        assert_eq!(m.valve(0).health, HEALTH_TEMP_FAILED, "temp2 {bad}");
    }
    d = status_data(0, 1);
    d.temp1 = TEMP_UNASSIGNED;
    d.temp2 = TEMP_UNASSIGNED;
    m.apply_valve_data(&d, 0);
    assert_eq!(m.valve(0).health, 0);
    d.temp1 = 1250;
    d.temp2 = -550;
    m.apply_valve_data(&d, 0);
    assert_eq!(m.valve(0).health, 0);

    // Inactive: only Blocked / Failed.
    m.set_active_mask(0);
    d = status_data(0, 6);
    d.calib_retries = 2;
    d.temp1 = TEMP_READ_ERROR;
    m.apply_valve_data(&d, 0);
    assert_eq!(m.valve(0).health, 0);
    m.apply_valve_data(&status_data(0, 9), 0);
    assert_eq!(m.valve(0).health, HEALTH_BLOCKED);
    m.apply_valve_data(&status_data(0, 4), 0);
    assert_eq!(m.valve(0).health, HEALTH_FAILED);
    // Activation recomputes immediately.
    m.apply_valve_data(&status_data(0, 6), 0);
    let rev = m.valve(0).revision;
    m.set_active_mask(1);
    assert_eq!(m.valve(0).health, HEALTH_NO_VALVE);
    assert_eq!(m.valve(0).revision, rev + 1);
    m.set_active_mask(1); // no change, no bump
    assert_eq!(m.valve(0).revision, rev + 1);
}

#[test]
fn staleness_measured_from_data_or_activation_latched_cleared_by_data() {
    let params = ValveModelParams::default();
    let mut m = ValveModel::new(params);
    m.set_active_mask(0x003);
    // Never-seen active valve: measured from the first tick.
    m.tick(1000);
    assert_eq!(m.valve(1).health, 0);
    m.tick(1000 + params.stale_ms - 1);
    assert_eq!(m.valve(1).health, 0);
    m.tick(1000 + params.stale_ms);
    assert_eq!(m.valve(1).health, HEALTH_STALE);
    assert_eq!(m.valve(0).health, HEALTH_STALE);

    m.apply_valve_data(&data(0), 100000);
    assert_eq!(m.valve(0).health, 0);
    m.tick(100000 + params.stale_ms - 1);
    assert_eq!(m.valve(0).health, 0);
    m.tick(100000 + params.stale_ms);
    assert_eq!(m.valve(0).health, HEALTH_STALE);
    // gvlst does not refresh.
    let s = ValveStates::default();
    m.apply_valve_states(&s, 200000);
    assert_eq!(m.valve(0).health, HEALTH_STALE);
    // gvlvx does.
    m.apply_valve_ex(&ex(0, 1, 0, 0), 300000);
    assert_eq!(m.valve(0).health & HEALTH_STALE, 0);

    // Latched across a millis() wrap.
    let mut w = ValveModel::default();
    w.set_active_mask(1);
    let t0: u32 = 0xFFFF_0000;
    w.apply_valve_data(&data(0), t0);
    w.tick(t0 + params.stale_ms);
    assert_ne!(w.valve(0).health & HEALTH_STALE, 0);
    w.tick(t0.wrapping_add(0x10000 + 5)); // wrapped: elapsed looks small again
    assert_ne!(w.valve(0).health & HEALTH_STALE, 0);

    // Deactivating clears; re-activating measures from the next tick.
    w.set_active_mask(0);
    assert_eq!(w.valve(0).health & HEALTH_STALE, 0);
    w.tick(50);
    assert_eq!(w.valve(0).health & HEALTH_STALE, 0);
    w.set_active_mask(1);
    w.tick(100);
    assert_eq!(w.valve(0).health & HEALTH_STALE, 0);
    w.tick(100 + params.stale_ms);
    assert_ne!(w.valve(0).health & HEALTH_STALE, 0);

    // Custom threshold.
    let mut q = ValveModel::new(ValveModelParams {
        stale_ms: 10,
        ..ValveModelParams::default()
    });
    q.set_active_mask(1);
    q.tick(0);
    q.tick(9);
    assert_eq!(q.valve(0).health, 0);
    q.tick(10);
    assert_eq!(q.valve(0).health, HEALTH_STALE);
}

#[test]
fn is_busy_branches() {
    let mut m = ValveModel::default();
    m.set_active_mask(0x001);
    m.apply_valve_data(&status_data(0, 1), 0);
    assert!(!m.is_busy(0));
    m.apply_valve_data(&status_data(0, 2), 0);
    assert!(m.is_busy(0));
    m.apply_valve_data(&status_data(0, 3), 0);
    assert!(m.is_busy(0));
    m.apply_valve_data(&status_data(0, 7), 0);
    assert!(!m.is_busy(0));
    let mut d = status_data(0, 1);
    d.calibrating = true;
    m.apply_valve_data(&d, 0);
    assert!(m.is_busy(0));
    m.apply_valve_data(&status_data(0, 1), 0);
    m.apply_target(&target(0, 5), 0);
    assert!(!m.is_busy(0));
}

#[test]
fn revision_bumps_on_every_meaningful_change_only() {
    let mut m = synced_model();
    let r = m.valve(0).revision;
    m.tick(2000); // not stale yet, nothing changes
    assert_eq!(m.valve(0).revision, r);
    let mut d = data(0);
    d.position = 41;
    m.apply_valve_data(&d, 2000);
    assert_eq!(m.valve(0).revision, r + 1);
    m.set_active_mask(0x001);
    assert_eq!(m.valve(0).revision, r + 1);
}

// ---------------------------------------------------------------- SensorModel

#[test]
fn temp_raw_valid_and_vad_valid_boundaries() {
    assert!(temp_raw_valid(0));
    assert!(temp_raw_valid(215));
    assert!(temp_raw_valid(-550));
    assert!(!temp_raw_valid(-551));
    assert!(temp_raw_valid(1250));
    assert!(!temp_raw_valid(1251));
    assert!(!temp_raw_valid(TEMP_UNASSIGNED));
    assert!(!temp_raw_valid(TEMP_READ_ERROR));
    assert!(!temp_raw_valid(TEMP_POWER_ON));
    assert!(temp_raw_valid(849));
    assert!(temp_raw_valid(851));
    assert!(temp_raw_valid(-499));
    assert!(temp_raw_valid(-501));
    assert!(!temp_raw_valid(i16::MIN));
    assert!(!temp_raw_valid(i16::MAX));
    assert!(!vad_valid(VAD_FAILED));
    assert!(!vad_valid(-5000));
    assert!(vad_valid(-999));
    assert!(vad_valid(0));
    assert!(vad_valid(i32::MAX));
}

#[test]
fn sensor_model_temperature_list_and_data() {
    let mut s = SensorModel::default();
    assert_eq!(s.temp_count(), 0);
    assert_eq!(s.find_temp(&id_with_crc(1)), None);
    let mut l = OneWireList {
        count: 3,
        has_list: false,
        ..OneWireList::default()
    };
    assert!(s.apply_temp_list(&l, 0));
    assert_eq!(s.temp_count(), 3);
    assert!(!s.apply_temp_list(&l, 0)); // same count
    l.has_list = true;
    l.ids[0] = id_with_crc(1);
    l.ids[1] = id_with_crc(2);
    l.ids[2] = id_with_crc(3);
    assert!(!s.apply_temp_list(&l, 0));
    assert_eq!(s.temp(0).id, id_with_crc(1));
    assert_eq!(s.temp(1).id, id_with_crc(2));
    assert!(is_zero(&s.temp(3).id));
    assert_eq!(s.find_temp(&id_with_crc(1)), Some(0));
    assert!(!s.temp(1).seen);
    assert_eq!(s.find_temp(&id_with_crc(3)), Some(2));
    assert_eq!(s.find_temp(&OneWireId::default()), None);

    let mut td = TempData {
        valid: true,
        id: id_with_crc(2),
        value: 205,
    };
    s.apply_temp_data(1, &td, 500);
    assert!(s.temp(1).seen);
    assert_eq!(s.temp(1).raw, 205);
    assert_eq!(s.temp(1).last_seen_ms, 500);
    assert!(s.temp_fresh(1, 500, 0));
    assert!(s.temp_fresh(1, 60500, 60000));
    assert!(!s.temp_fresh(1, 60501, 60000));
    assert!(!s.temp_fresh(0, 500, 60000)); // never seen
    assert!(!s.temp_fresh(34, 500, 60000));

    // Same list again keeps the reading; a different id at an index resets it.
    assert!(!s.apply_temp_list(&l, 600));
    assert_eq!(s.temp(1).raw, 205);
    l.ids[1] = id_with_crc(9);
    s.apply_temp_list(&l, 700);
    assert_eq!(s.temp(1).id, id_with_crc(9));
    assert!(!s.temp(1).seen);
    assert_eq!(s.temp(1).raw, TEMP_UNASSIGNED);

    // goned carries the authoritative id.
    td.id = id_with_crc(4);
    s.apply_temp_data(1, &td, 800);
    assert_eq!(s.temp(1).id, id_with_crc(4));
    assert_eq!(s.find_temp(&id_with_crc(4)), Some(1));

    // Invalid form marks not seen (the second one in a row).
    let inv = TempData::default();
    s.apply_temp_data(1, &inv, 900);
    assert!(s.temp(1).seen);
    s.apply_temp_data(1, &inv, 900);
    assert!(!s.temp(1).seen);
    assert_eq!(s.temp(1).raw, TEMP_UNASSIGNED);
    assert!(!s.temp_fresh(1, 900, 60000));

    // Out-of-range bus index ignored; temp() of it returns the empty reading.
    s.apply_temp_data(34, &td, 1000);
    assert!(!s.temp(34).seen);
    assert!(core::ptr::eq(s.temp(34), s.temp(200)));
    assert!(is_zero(&s.volt(0).id)); // nothing written past the temperature table
    assert!(!s.volt(0).seen);
    assert_eq!(s.volt_count(), 0);

    // Shrinking the count clears readings beyond it.
    s.apply_temp_data(2, &td, 1000);
    l.count = 2;
    l.has_list = false;
    assert!(s.apply_temp_list(&l, 1100));
    assert_eq!(s.temp_count(), 2);
    assert!(!s.temp(2).seen);
    assert!(is_zero(&s.temp(2).id));
    assert_eq!(s.find_temp(&id_with_crc(3)), None);

    // Ids beyond the count are ignored.
    l.count = 3;
    l.has_list = true;
    l.ids[3] = id_with_crc(11);
    s.apply_temp_list(&l, 1200);
    assert!(is_zero(&s.temp(3).id));
    assert_eq!(s.find_temp(&id_with_crc(11)), None);

    // Count is clamped to the bus maximum.
    l.count = 200;
    l.has_list = false;
    assert!(s.apply_temp_list(&l, 0));
    assert_eq!(s.temp_count(), TEMP_SLOT_COUNT);
    // A full table is searched to its end and no further.
    let mut vl = OneWireList {
        count: 1,
        has_list: true,
        ..OneWireList::default()
    };
    vl.ids[0] = id_with_crc(12);
    s.apply_volt_list(&vl, 0);
    assert_eq!(s.find_temp(&id_with_crc(12)), None);
    let last = TempData {
        valid: true,
        id: id_with_crc(13),
        value: 1,
    };
    s.apply_temp_data(33, &last, 0);
    assert_eq!(s.find_temp(&id_with_crc(13)), Some(33));

    s.clear();
    assert_eq!(s.temp_count(), 0);
    assert_eq!(s.volt_count(), 0);
    assert!(is_zero(&s.temp(0).id));
}

#[test]
fn sensor_model_volt_list_and_data() {
    let mut s = SensorModel::default();
    let mut l = OneWireList {
        count: 2,
        has_list: true,
        ..OneWireList::default()
    };
    l.ids[0] = id_with_crc(5);
    l.ids[1] = id_with_crc(6);
    assert!(s.apply_volt_list(&l, 0));
    assert_eq!(s.volt_count(), 2);
    assert!(!s.apply_volt_list(&l, 0));
    assert_eq!(s.find_volt(&id_with_crc(6)), Some(1));
    assert_eq!(s.find_volt(&id_with_crc(5)), Some(0));
    assert_eq!(s.volt(0).id, id_with_crc(5));
    assert!(is_zero(&s.volt(2).id));
    assert_eq!(s.find_volt(&OneWireId::default()), None);
    assert_eq!(s.find_volt(&id_with_crc(7)), None);

    let vd = VoltData {
        valid: true,
        id: id_with_crc(6),
        vad: 1234,
    };
    s.apply_volt_data(1, &vd, 10);
    assert!(s.volt(1).seen);
    assert_eq!(s.volt(1).vad, 1234);
    assert_eq!(s.volt(1).last_seen_ms, 10);
    let inv = VoltData::default();
    s.apply_volt_data(1, &inv, 20);
    assert!(s.volt(1).seen);
    s.apply_volt_data(1, &inv, 20);
    assert!(!s.volt(1).seen);
    assert_eq!(s.volt(1).vad, VAD_FAILED);
    s.apply_volt_data(8, &vd, 30);
    assert!(!s.volt(8).seen);
    assert!(core::ptr::eq(s.volt(8), s.volt(255)));
    assert_eq!(s.volt_count(), 2); // nothing written past the volt table
    assert_eq!(s.temp_count(), 0);
    // A full volt table is searched to its end and no further.
    let mut full = OneWireList {
        count: VOLT_SLOT_COUNT,
        has_list: true,
        ..OneWireList::default()
    };
    for (i, id) in (0..VOLT_SLOT_COUNT).zip(full.ids.iter_mut()) {
        *id = id_with_crc(40 + i);
    }
    s.apply_volt_list(&full, 0);
    assert_eq!(s.find_volt(&id_with_crc(47)), Some(7));
    assert_eq!(s.find_volt(&id_with_crc(48)), None);

    l.ids[1] = id_with_crc(8);
    s.apply_volt_data(1, &vd, 40);
    s.apply_volt_list(&l, 50);
    assert_eq!(s.volt(1).id, id_with_crc(8));
    assert!(!s.volt(1).seen);

    l.count = 20;
    l.has_list = false;
    assert!(s.apply_volt_list(&l, 0));
    assert_eq!(s.volt_count(), VOLT_SLOT_COUNT);
    l.count = 1;
    assert!(s.apply_volt_list(&l, 0));
    assert!(is_zero(&s.volt(1).id));
    assert_eq!(s.temp(0).raw, TEMP_UNASSIGNED);
}

#[test]
fn apply_sensor_temps_only_the_second_sensor_changes() {
    let mut s = SensorModel::default();
    let mut l = OneWireList {
        count: 2,
        has_list: true,
        ..OneWireList::default()
    };
    l.ids[0] = id_with_crc(1);
    l.ids[1] = id_with_crc(2);
    s.apply_temp_list(&l, 0);
    let mut m = ValveModel::default();
    m.set_active_mask(0x001);
    let mut vs = ValveSensors {
        is_list: true,
        ..ValveSensors::default()
    };
    vs.ids[0][0] = id_with_crc(1);
    vs.ids[0][1] = id_with_crc(2);
    m.apply_valve_sensors(&vs, &[]);
    let rev0 = m.valve(0).revision;
    // Sensor 1 not read yet (stays unassigned, equal to the old temp2).
    read(&mut s, &l, 1, 215, 1000);
    m.apply_sensor_temps(&s, 1000, 60000, false);
    assert_eq!(m.valve(0).temp1, TEMP_UNASSIGNED);
    assert_eq!(m.valve(0).temp2, 215);
    assert_eq!(m.valve(0).revision, rev0 + 1);
    // temp1 unchanged, temp2 changes again.
    read(&mut s, &l, 1, 220, 2000);
    m.apply_sensor_temps(&s, 2000, 60000, false);
    assert_eq!(m.valve(0).temp1, TEMP_UNASSIGNED);
    assert_eq!(m.valve(0).temp2, 220);
    assert_eq!(m.valve(0).revision, rev0 + 2);
    // temp1 changes to the old temp2 value, temp2 unchanged.
    read(&mut s, &l, 0, 220, 3000);
    m.apply_sensor_temps(&s, 3000, 60000, false);
    assert_eq!(m.valve(0).temp1, 220);
    assert_eq!(m.valve(0).temp2, 220);
    assert_eq!(m.valve(0).revision, rev0 + 3);
}
