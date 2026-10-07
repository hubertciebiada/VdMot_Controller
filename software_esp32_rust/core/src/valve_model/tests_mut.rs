//! Port of test/native/test_valve_model__mut.cpp: what the resets of assembly, restore,
//! failsafe, reboot and forget_stm_data leave behind; bus index 0; desired revisions. The Rust
//! additions at the end kill the cargo-mutants mutants the C++ cases leave alive and pin a kept
//! C++ quirk.

use super::*;

fn data(valve: u8) -> ValveData {
    ValveData {
        valve,
        position: 40,
        mean_current: 12,
        status: 1,
        temp1: TEMP_UNASSIGNED,
        temp2: TEMP_UNASSIGNED,
        moves: 7,
        open_count: 3000,
        close_count: 3100,
        ..ValveData::default()
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

/// Valve 0 active and known, desired 50 (web).
fn web_model() -> ValveModel {
    let mut m = ValveModel::default();
    m.set_active_mask(0x001);
    m.apply_valve_data(&data(0), 0);
    assert!(m.set_desired_target(0, 50, TargetSource::Web, 0));
    m
}

/// web_model with a push in flight and the target marked unconfirmed (a retried Failed
/// delivery).
fn unconfirmed_model() -> ValveModel {
    let mut m = web_model();
    let mut t = 0;
    for _ in 0..5 {
        assert!(m.next_target_push(t).is_some());
        m.on_target_timeout(0, t);
        t += 3000;
    }
    assert_eq!(m.valve(0).sync, TargetSync::Failed);
    assert!(m.next_target_push(t + 400_000).is_some()); // re-armed after failedRetryMs
    assert_ne!(m.valve(0).health & HEALTH_TARGET_UNCONFIRMED, 0);
    m
}

const NO_PCT: [u8; 12] = [0; 12];

#[test]
fn desired_revision_a_new_desired_value_with_the_same_source_counts() {
    let mut m = web_model();
    let rev = m.desired_revision();
    assert!(m.set_desired_target(0, 60, TargetSource::Web, 0));
    assert_eq!(m.desired_revision(), rev + 1);
}

#[test]
fn set_assembly_forgets_the_stm_target_the_failsafe_target_and_the_unconfirmed_flag() {
    let mut m = unconfirmed_model();
    m.apply_target(&target(0, 50), 0);
    let mut pct = [0u8; 12];
    pct[0] = 30;
    m.set_failsafe_drive(0x001, &pct);
    assert!(m.valve(0).fs_override);
    assert_eq!(m.valve(0).fs_target, 30);
    assert!(m.valve(0).stm_target_known);
    m.set_assembly(0, 0);
    assert!(!m.valve(0).stm_target_known);
    assert!(!m.valve(0).fs_override);
    assert_eq!(m.valve(0).fs_target, 0);
    assert_eq!(m.valve(0).sync, TargetSync::AwaitAck);
    assert_eq!(m.valve(0).health & HEALTH_TARGET_UNCONFIRMED, 0);
}

#[test]
fn restore_desired_attempts_and_the_unconfirmed_flag_start_over() {
    let mut m = unconfirmed_model();
    assert!(m.valve(0).push_attempts > 0);
    assert!(m.restore_desired(0, 60, TargetSource::Web));
    assert_eq!(m.valve(0).push_attempts, 0);
    assert_eq!(m.valve(0).sync, TargetSync::Pending);
    assert_eq!(m.valve(0).health & HEALTH_TARGET_UNCONFIRMED, 0);
}

#[test]
fn set_failsafe_drive_pct_100_is_a_failsafe_position_101_is_not() {
    let mut m = unconfirmed_model();
    let mut pct = [0u8; 12];
    pct[0] = 101;
    m.set_failsafe_drive(0x001, &pct);
    assert!(!m.valve(0).fs_override);
    assert_eq!(m.push_target(0), 50);
    pct[0] = 100;
    m.set_failsafe_drive(0x001, &pct);
    assert!(m.valve(0).fs_override);
    assert_eq!(m.push_target(0), 100);
    assert_eq!(m.valve(0).sync, TargetSync::Pending);
    assert_eq!(m.valve(0).push_attempts, 0);
    assert_eq!(m.valve(0).health & HEALTH_TARGET_UNCONFIRMED, 0);
}

#[test]
fn set_failsafe_drive_an_unchanged_pushed_value_leaves_a_delivery_in_flight_alone() {
    let mut m = web_model();
    assert!(m.next_target_push(0).is_some());
    assert_eq!(m.valve(0).sync, TargetSync::AwaitAck);
    m.set_failsafe_drive(0, &NO_PCT);
    assert_eq!(m.valve(0).sync, TargetSync::AwaitAck);
    assert_eq!(m.valve(0).push_attempts, 1);
}

#[test]
fn set_min_counts_every_active_valve_gets_the_stroke_check() {
    let mut m = ValveModel::default();
    m.set_active_mask(0x002);
    m.apply_valve_data(&data(1), 0);
    m.set_min_counts(3000);
    assert_eq!(m.valve(1).health, HEALTH_STROKE_SHORT);
}

#[test]
fn forget_stm_data_staleness_starts_over_no_flag_and_no_assembly_result_left() {
    let mut m = unconfirmed_model();
    m.tick(0);
    m.forget_stm_data();
    assert_eq!(m.valve(0).health & HEALTH_STALE, 0);
    assert_eq!(m.valve(0).health & HEALTH_TARGET_UNCONFIRMED, 0);
    assert_eq!(m.valve(0).sync, TargetSync::Pending);
    m.on_assembly_ack(ALL_VALVES, 0);
    assert_eq!(m.valve(0).sync, TargetSync::Pending);
    m.tick(60000); // measured from this tick, not from the tick before forget_stm_data
    assert_eq!(m.valve(0).health & HEALTH_STALE, 0);
    m.tick(120000);
    assert_ne!(m.valve(0).health & HEALTH_STALE, 0);
}

#[test]
fn on_stm_rebooted_no_assembly_result_is_pending_afterwards() {
    let mut m = web_model();
    m.on_stm_rebooted(0);
    assert_eq!(m.valve(0).sync, TargetSync::Pending);
    m.on_assembly_ack(ALL_VALVES, 0);
    assert_eq!(m.valve(0).sync, TargetSync::Pending);
    m.on_assembly_failed(ALL_VALVES, 0);
    assert_eq!(m.valve(0).sync, TargetSync::Pending);
}

#[test]
fn sensor_model_bus_index_0_for_stray_data_and_stale_readings() {
    let mut s = SensorModel::default();
    let mut l = OneWireList {
        count: 1,
        has_list: true,
        ..OneWireList::default()
    };
    l.ids[0] = id_with_crc(1);
    s.apply_temp_list(&l, 0);
    s.apply_volt_list(&l, 0);
    let td = TempData {
        valid: true,
        id: l.ids[0],
        value: 215,
    };
    assert!(s.apply_stray_temp_data(&td, 1000));
    assert_eq!(s.temp(0).raw, 215);
    let vd = VoltData {
        valid: true,
        id: l.ids[0],
        vad: 500,
    };
    assert!(s.apply_stray_volt_data(&vd, 1000));
    assert_eq!(s.volt(0).vad, 500);

    let mut m = ValveModel::default();
    m.set_active_mask(0x001);
    let mut vs = ValveSensors {
        is_list: true,
        ..ValveSensors::default()
    };
    vs.ids[0][0] = l.ids[0];
    m.apply_valve_sensors(&vs, &[]);
    m.apply_sensor_temps(&s, 2000, 60000, false);
    assert_eq!(m.valve(0).temp1, 215);
    m.apply_sensor_temps(&s, 100000, 60000, false); // seen before, too old now
    assert_eq!(m.valve(0).temp1, TEMP_READ_ERROR);
}

// ---------------------------------------------------------------- Rust additions

#[test]
fn set_desired_target_under_the_override_leaves_a_delivery_in_flight_alone() {
    let mut m = web_model();
    let mut pct = [0u8; 12];
    pct[0] = 30;
    m.set_failsafe_drive(0x001, &pct);
    assert_eq!(m.next_target_push(0), Some((0, 30)));
    assert!(m.set_desired_target(0, 60, TargetSource::Web, 10));
    assert_eq!(m.valve(0).desired, 60);
    assert_eq!(m.valve(0).sync, TargetSync::AwaitAck);
    assert_eq!(m.valve(0).push_attempts, 1);
    m.on_target_ack(0, 20);
    m.apply_target(&target(0, 30), 30);
    assert_eq!(m.valve(0).sync, TargetSync::Synced);
}

#[test]
fn set_failsafe_drive_follows_the_mask_bit_of_each_valve() {
    let mut m = ValveModel::default();
    m.set_active_mask(0x0FFF);
    for v in 0..VALVE_COUNT {
        m.apply_target(&target(v, 20), 0); // adopted: desired 20
    }
    m.set_failsafe_drive(0x0802, &[50; 12]);
    for v in 0..VALVE_COUNT {
        assert_eq!(m.valve(v).fs_override, v == 1 || v == 11, "valve {v}");
    }
}

#[test]
fn forget_stm_data_keeps_move_seq_and_fs_pct() {
    let mut m = ValveModel::default();
    m.set_active_mask(0x001);
    let mut d = ValveEx {
        valve: 0,
        target: 30,
        v3: true,
        fs_pct: 70,
        ..ValveEx::default()
    };
    d.last_move.requested_counts = 5;
    m.apply_valve_ex(&d, 0);
    assert_eq!(m.valve(0).move_seq, 1);
    assert_eq!(m.valve(0).fs_pct, 70);
    m.forget_stm_data();
    assert_eq!(m.valve(0).move_seq, 1);
    assert_eq!(m.valve(0).fs_pct, 70);
    assert!(!m.valve(0).has_v3);
}

/// Kept C++ behaviour (docs/rust/PORT-NOTES.md, valve_model): an inactive valve is never
/// pushed, so after an STM reboot its forced push stays due, no read-back syncs it and
/// is_busy() stays true; a valve deactivated while its target is Pending stays busy as well.
#[test]
fn an_inactive_valve_with_a_desired_target_stays_busy_after_an_stm_reboot() {
    let mut m = ValveModel::default();
    m.set_active_mask(0x003);
    m.apply_valve_data(&data(1), 0);
    m.apply_target(&target(1, 40), 0); // re-sync read-back: adopted, Synced
    m.set_active_mask(0x001);
    assert!(!m.is_busy(1));
    m.on_stm_rebooted(1000);
    m.apply_target(&target(1, 40), 2000); // equals the desired target
    assert_eq!(m.next_target_push(100_000), None);
    m.apply_target(&target(1, 40), 200_000);
    assert_eq!(m.valve(1).sync, TargetSync::Pending);
    assert!(m.valve(1).force_push);
    assert!(m.is_busy(1));

    let mut n = ValveModel::default();
    n.set_active_mask(0x001);
    n.apply_valve_data(&data(0), 0);
    n.apply_target(&target(0, 40), 0);
    assert!(n.set_desired_target(0, 60, TargetSource::Web, 10));
    n.set_active_mask(0);
    n.apply_target(&target(0, 40), 50_000); // the STM still has 40
    assert_eq!(n.valve(0).sync, TargetSync::Pending);
    assert!(n.is_busy(0));
}
