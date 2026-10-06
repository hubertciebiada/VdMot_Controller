//! Port of test/native/test_valve_model__link.cpp: ValveModel / SensorModel behaviour of the
//! 2.1 link: assembly hold, restored targets, failsafe emulation, protocol 3 fields, forced
//! pushes after an STM reboot, sensor debounce and id-matched late replies.

use super::*;
use crate::stm_codec::{build_temp_data, build_valve_data, build_volt_data, STM_FLAG_RETRY};

fn data(valve: u8) -> ValveData {
    ValveData {
        valve,
        position: 40,
        status: 1,
        open_count: 3000,
        close_count: 3100,
        ..ValveData::default()
    }
}

fn ex_v3(valve: u8, target: u8, flags: u16) -> ValveEx {
    ValveEx {
        valve,
        status: 1,
        target,
        open_count: 5000,
        close_count: 5200,
        v3: true,
        flags,
        fs_pct: 50,
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

/// Valves in `mask` active and known, desired `pos` delivered and verified.
fn synced(mask: u16, pos: u8) -> ValveModel {
    let mut m = ValveModel::default();
    m.set_active_mask(mask);
    for v in 0..VALVE_COUNT {
        if (mask >> v) & 1 == 0 {
            continue;
        }
        m.apply_valve_data(&data(v), 0);
        m.apply_target(&target(v, pos), 0);
        assert_eq!(m.valve(v).sync, TargetSync::Synced);
    }
    m
}

const PCT_50: [u8; 12] = [50; 12];

// ================================================================ helpers

#[test]
fn stroke_near_minimum_below_1_2_x_min_counts_with_every_count_known() {
    assert!(stroke_near_minimum(3599, 5000, 3000));
    assert!(!stroke_near_minimum(3600, 5000, 3000));
    assert!(stroke_near_minimum(5000, 3599, 3000)); // the smaller of the two counts
    assert!(!stroke_near_minimum(5000, 3600, 3000));
    assert!(!stroke_near_minimum(100, 100, 0));
    assert!(!stroke_near_minimum(0, 100, 3000));
    assert!(!stroke_near_minimum(100, 0, 3000));
    assert!(stroke_near_minimum(1, 1, 1));
    assert!(stroke_near_minimum(0xFFFF_FFFF, 78641, 65535)); // no overflow: 78641 * 5 < 65535 * 6
    assert!(!stroke_near_minimum(78642, 0xFFFF_FFFF, 65535));
    assert!(!stroke_near_minimum(0xFFFF_FFFF, 0xFFFF_FFFF, 65535));
}

#[test]
fn expect_sensor_goned_gowvd_expect_the_id_at_their_bus_index_others_untouched() {
    let mut s = SensorModel::default();
    let mut l = OneWireList {
        count: 2,
        has_list: true,
        ..OneWireList::default()
    };
    l.ids[0] = id_with_crc(1);
    l.ids[1] = id_with_crc(2);
    s.apply_temp_list(&l, 0);
    l.ids[1] = id_with_crc(5);
    s.apply_volt_list(&l, 0);
    let mut r = build_temp_data(1).expect("goned 1");
    expect_sensor(&mut r, &s);
    assert_eq!(r.expect, id_with_crc(2));
    let mut r = build_temp_data(5).expect("goned 5");
    expect_sensor(&mut r, &s);
    assert!(is_zero(&r.expect)); // unknown index: any id
    let mut r = build_volt_data(1).expect("gowvd 1");
    expect_sensor(&mut r, &s);
    assert_eq!(r.expect, id_with_crc(5));
    let mut r = build_valve_data(1).expect("gvlvd 1");
    r.expect = id_with_crc(9);
    expect_sensor(&mut r, &s);
    assert_eq!(r.expect, id_with_crc(9));
}

// ================================================================ protocol 3 fields

#[test]
fn gvlvy_fields_enter_the_state_gvlvx_and_gvlvd_clear_them() {
    let mut m = ValveModel::default();
    m.set_active_mask(0x001);
    let mut d = ex_v3(0, 30, STM_FLAG_RETRY | STM_FLAG_FS_BLOCKED);
    d.fault = 4;
    d.fs_pct = 60;
    d.drive = 60;
    d.retry_s = 3540;
    d.retries = 2;
    m.apply_valve_ex(&d, 0);
    let v = *m.valve(0);
    assert!(v.has_v3);
    assert_eq!(v.stm_flags, STM_FLAG_RETRY | STM_FLAG_FS_BLOCKED);
    assert_eq!(v.fault, 4);
    assert_eq!(v.fs_pct, 60);
    assert_eq!(v.drive, 60);
    assert_eq!(v.retry_s, 3540);
    assert_eq!(v.retries, 2);
    assert!(v.auto_retry);
    let rev = v.revision;
    d.retry_s = 3530; // counts down on every poll: not a change
    m.apply_valve_ex(&d, 10);
    assert_eq!(m.valve(0).revision, rev);
    assert_eq!(m.valve(0).retry_s, 3530);
    let mut x = d;
    x.v3 = false;
    m.apply_valve_ex(&x, 20);
    let v = *m.valve(0);
    assert!(!v.has_v3);
    assert_eq!(v.stm_flags, 0);
    assert_eq!(v.fault, 0);
    assert_eq!(v.drive, 0);
    assert_eq!(v.retry_s, 0);
    assert_eq!(v.retries, 0);
    assert!(!v.auto_retry);
    assert_eq!(v.fs_pct, 60); // fsPct stays (the ESP config applies on 1/2)
    m.apply_valve_ex(&d, 30);
    assert!(m.valve(0).has_v3);
    m.apply_valve_data(&data(0), 40);
    assert!(!m.valve(0).has_v3);
    assert_eq!(m.valve(0).stm_flags, 0);
    assert_eq!(m.valve(0).fault, 0);
}

#[test]
fn auto_retry_set_when_retries_rise_cleared_at_the_calibration_end_or_with_retries_0() {
    let mut m = ValveModel::default();
    m.set_active_mask(0x001);
    let mut d = ex_v3(0, 30, 0);
    m.apply_valve_ex(&d, 0);
    assert!(!m.valve(0).auto_retry);
    d.retries = 1;
    m.apply_valve_ex(&d, 10);
    assert!(m.valve(0).auto_retry);
    d.calibrating = true;
    m.apply_valve_ex(&d, 20);
    assert!(m.valve(0).auto_retry); // the retry calibration runs
    m.apply_valve_ex(&d, 25); // equal retries keep it
    assert!(m.valve(0).auto_retry);
    d.calibrating = false;
    m.apply_valve_ex(&d, 30);
    assert!(!m.valve(0).auto_retry);
    m.apply_valve_ex(&d, 35); // equal retries do not set it again
    assert!(!m.valve(0).auto_retry);
    d.retries = 2;
    m.apply_valve_ex(&d, 40);
    assert!(m.valve(0).auto_retry);
    d.retries = 0;
    m.apply_valve_ex(&d, 50);
    assert!(!m.valve(0).auto_retry);
    d.retries = 3;
    d.calibrating = false;
    m.apply_valve_ex(&d, 60);
    assert!(m.valve(0).auto_retry);
}

#[test]
fn health_the_stm_lease_flag_sets_failsafe_and_nothing_else_active_valves_only() {
    let mut m = ValveModel::default();
    m.set_active_mask(0x001);
    m.apply_valve_ex(&ex_v3(0, 30, STM_FLAG_FS_LEASE), 0);
    assert_eq!(m.valve(0).health, HEALTH_FAILSAFE);
    m.apply_valve_ex(&ex_v3(0, 30, STM_FLAG_FS_BLOCKED), 0);
    assert_eq!(m.valve(0).health, 0);
    m.apply_valve_ex(&ex_v3(1, 30, STM_FLAG_FS_LEASE), 0);
    assert_eq!(m.valve(1).health, 0); // inactive
}

#[test]
fn health_stroke_short_from_set_min_counts_and_the_counts() {
    let mut m = ValveModel::default();
    m.set_active_mask(0x003);
    let mut d = data(0);
    d.open_count = 3599;
    d.close_count = 5000;
    m.apply_valve_data(&d, 0);
    d.valve = 1;
    m.apply_valve_data(&d, 0);
    m.set_active_mask(0x001);
    assert_eq!(m.valve(0).health, 0); // minCounts unknown
    let rev = m.valve(0).revision;
    m.set_min_counts(3000);
    assert_eq!(m.min_counts(), 3000);
    assert_eq!(m.valve(0).health, HEALTH_STROKE_SHORT);
    assert_eq!(m.valve(0).revision, rev + 1);
    assert_eq!(m.valve(1).health, 0); // inactive
    d.valve = 0;
    d.open_count = 3600;
    m.apply_valve_data(&d, 0);
    assert_eq!(m.valve(0).health, 0);
    m.set_min_counts(3001);
    assert_eq!(m.valve(0).health, HEALTH_STROKE_SHORT);
    m.set_min_counts(0);
    assert_eq!(m.valve(0).health, 0);
}

#[test]
fn diff_valve_the_failsafe_group_covers_every_protocol_3_and_emulation_field() {
    let a = ValveState::default();
    let mut b = ValveState::default();
    assert_eq!(diff_valve(&a, &b), 0);
    b.has_v3 = true;
    assert_eq!(diff_valve(&a, &b), CHANGE_FAILSAFE);
    b = a;
    b.stm_flags = 1;
    assert_eq!(diff_valve(&a, &b), CHANGE_FAILSAFE);
    b = a;
    b.fault = 1;
    assert_eq!(diff_valve(&a, &b), CHANGE_FAILSAFE);
    b = a;
    b.fs_pct = 1;
    assert_eq!(diff_valve(&a, &b), CHANGE_FAILSAFE);
    b = a;
    b.drive = 1;
    assert_eq!(diff_valve(&a, &b), CHANGE_FAILSAFE);
    b = a;
    b.retries = 1;
    assert_eq!(diff_valve(&a, &b), CHANGE_FAILSAFE);
    b = a;
    b.auto_retry = true;
    assert_eq!(diff_valve(&a, &b), CHANGE_FAILSAFE);
    b = a;
    b.fs_override = true;
    assert_eq!(diff_valve(&a, &b), CHANGE_FAILSAFE);
    b = a;
    b.fs_target = 1;
    assert_eq!(diff_valve(&a, &b), CHANGE_FAILSAFE);
    b = a;
    b.retry_s = 1;
    b.force_push = true;
    assert_eq!(diff_valve(&a, &b), 0);
}

// ================================================================ assembly (W1)

#[test]
fn assembly_desired_100_held_without_stgtp_until_the_staop_result_then_verified() {
    let mut m = synced(0x008, 20);
    let rev = m.desired_revision();
    m.set_assembly(3, 100);
    let v = *m.valve(3);
    assert_eq!(v.desired, 100);
    assert_eq!(v.source, TargetSource::Assembly);
    assert_eq!(v.sync, TargetSync::AwaitAck);
    assert_eq!(m.desired_revision(), rev + 1);
    assert_eq!(m.next_target_push(100000), None);
    m.apply_target(&target(3, 20), 200); // read-back while waiting changes nothing
    assert_eq!(m.valve(3).sync, TargetSync::AwaitAck);
    m.on_target_ack(3, 300); // a stgtp ack is not the staop's
    assert_eq!(m.valve(3).sync, TargetSync::AwaitAck);
    m.on_target_timeout(3, 300);
    assert_eq!(m.valve(3).sync, TargetSync::AwaitAck);
    m.on_assembly_ack(3, 400);
    assert_eq!(m.valve(3).sync, TargetSync::AwaitVerify);
    m.on_assembly_ack(3, 450); // only once
    assert_eq!(m.valve(3).sync, TargetSync::AwaitVerify);
    m.apply_target(&target(3, 100), 500);
    assert_eq!(m.valve(3).sync, TargetSync::Synced);
    assert_eq!(m.valve(3).desired, 100);
    assert_eq!(m.valve(3).source, TargetSource::Assembly);
}

#[test]
fn assembly_a_differing_read_back_after_the_ack_pushes_stgtp_100() {
    let mut m = synced(0x008, 20);
    m.set_assembly(3, 100);
    m.on_assembly_ack(3, 400);
    m.apply_target(&target(3, 20), 500);
    assert_eq!(m.valve(3).sync, TargetSync::Pending);
    assert_eq!(m.next_target_push(600), Some((3, 100)));
}

#[test]
fn assembly_all_valves_addresses_the_active_ones_a_failure_affects_the_named_valve_only() {
    let mut m = synced(0x005, 20);
    m.set_assembly(ALL_VALVES, 100);
    assert_eq!(m.valve(0).source, TargetSource::Assembly);
    assert_eq!(m.valve(2).source, TargetSource::Assembly);
    assert_eq!(m.valve(1).source, TargetSource::None);
    assert!(!m.valve(1).desired_valid);
    m.on_assembly_failed(2, 200);
    assert_eq!(m.valve(2).sync, TargetSync::Pending);
    assert_eq!(m.valve(2).push_attempts, 0);
    assert_eq!(m.valve(0).sync, TargetSync::AwaitAck);
    m.on_assembly_ack(ALL_VALVES, 300);
    assert_eq!(m.valve(0).sync, TargetSync::AwaitVerify);
    assert_eq!(m.valve(2).sync, TargetSync::Pending); // no longer waiting for the staop
    assert_eq!(m.next_target_push(400), Some((2, 100)));
}

#[test]
fn assembly_failures_after_max_push_attempts_staop_deliveries_give_failed() {
    let mut m = ValveModel::new(ValveModelParams {
        max_push_attempts: 2,
        ..ValveModelParams::default()
    });
    m.set_active_mask(0x001);
    m.apply_valve_data(&data(0), 0);
    m.set_assembly_via_staop(true);
    m.set_assembly(0, 0);
    m.on_assembly_failed(0, 0);
    assert_eq!(m.next_assembly_push(10000), Some(0));
    m.on_assembly_failed(0, 10000);
    assert_eq!(m.valve(0).sync, TargetSync::Pending);
    assert!(m.next_assembly_push(20000).is_some());
    m.on_assembly_failed(0, 20000);
    assert_eq!(m.valve(0).sync, TargetSync::Failed);
}

#[test]
fn assembly_a_web_target_ends_it_source_web_pending_push_40() {
    let mut m = synced(0x008, 20);
    m.set_assembly(3, 100);
    m.on_assembly_ack(3, 400);
    m.apply_target(&target(3, 100), 500);
    assert!(m.set_desired_target(3, 40, TargetSource::Web, 600));
    assert_eq!(m.valve(3).source, TargetSource::Web);
    assert_eq!(m.valve(3).sync, TargetSync::Pending);
    assert_eq!(m.next_target_push(700).map(|(_, pos)| pos), Some(40));
}

#[test]
fn assembly_a_web_target_of_100_still_ends_it_with_a_stgtp_100() {
    let mut m = synced(0x008, 20);
    m.set_assembly(3, 100);
    m.on_assembly_ack(3, 400);
    m.apply_target(&target(3, 100), 500);
    assert_eq!(m.valve(3).sync, TargetSync::Synced);
    assert!(m.set_desired_target(3, 100, TargetSource::Mqtt, 600));
    assert_eq!(m.valve(3).source, TargetSource::Mqtt);
    assert_eq!(m.valve(3).sync, TargetSync::Pending); // the STM holds 100, the stgtp ends its hold
    assert!(m.valve(3).force_push);
    assert_eq!(m.next_target_push(650).map(|(_, pos)| pos), Some(100));
    m.on_target_ack(3, 660);
    m.apply_target(&target(3, 100), 670);
    assert_eq!(m.valve(3).sync, TargetSync::Synced);
    assert!(m.set_desired_target(3, 100, TargetSource::Web, 680)); // same value, not an assembly
    assert_eq!(m.valve(3).sync, TargetSync::Synced);
    // While the staop is outstanding a web target cancels the wait.
    m.set_assembly(3, 700);
    assert!(m.set_desired_target(3, 100, TargetSource::Web, 800));
    assert_eq!(m.valve(3).sync, TargetSync::Pending);
    m.on_assembly_ack(3, 900); // a late staop ack changes nothing
    assert_eq!(m.valve(3).sync, TargetSync::Pending);
}

#[test]
fn assembly_via_staop_an_assembly_valve_is_delivered_by_staop_never_stgtp() {
    let mut m = synced(0x003, 20);
    m.set_assembly_via_staop(true);
    m.set_assembly(1, 0);
    m.on_assembly_ack(1, 10);
    m.apply_target(&target(1, 100), 20);
    assert_eq!(m.valve(1).sync, TargetSync::Synced);
    m.on_stm_rebooted(1000);
    assert_eq!(m.valve(1).sync, TargetSync::Pending);
    m.apply_target(&target(0, 20), 1100);
    m.apply_target(&target(1, 100), 1100);
    // valve 0: forced stgtp; valve 1: staop only.
    assert_eq!(m.next_target_push(1200).map(|(valve, _)| valve), Some(0));
    assert_eq!(m.next_target_push(1200), None);
    assert_eq!(m.next_assembly_push(1200), Some(1));
    assert_eq!(m.valve(1).sync, TargetSync::AwaitAck);
    assert_eq!(m.valve(1).push_attempts, 1);
    assert!(!m.valve(1).force_push);
    assert_eq!(m.next_assembly_push(1200), None);
    m.on_target_ack(1, 1300); // not the staop result
    assert_eq!(m.valve(1).sync, TargetSync::AwaitAck);
    m.on_assembly_ack(1, 1300);
    assert_eq!(m.valve(1).sync, TargetSync::AwaitVerify);
    // protocol 3: 100 without the Assembly flag is not the hold.
    m.apply_valve_ex(&ex_v3(1, 100, 0), 1400);
    assert_eq!(m.valve(1).sync, TargetSync::Pending);
    assert_eq!(m.next_assembly_push(1200 + 1999), None); // pushRetryMs since the staop
    assert!(m.next_assembly_push(1200 + 2000).is_some());
    m.on_assembly_ack(1, 3500);
    m.apply_valve_ex(&ex_v3(1, 100, STM_FLAG_ASSEMBLY), 3600);
    assert_eq!(m.valve(1).sync, TargetSync::Synced);
    // Without staop delivery (protocol 1) the same valve gets stgtp 100.
    m.set_assembly_via_staop(false);
    m.on_stm_rebooted(4000);
    m.apply_target(&target(1, 30), 4100);
    assert_eq!(m.next_assembly_push(10000), None);
    assert_eq!(m.next_target_push(10000).map(|(valve, _)| valve), Some(0));
    assert_eq!(m.next_target_push(10000), Some((1, 100)));
}

#[test]
fn assembly_via_staop_a_staop_that_could_not_be_queued_goes_back_to_pending_uncounted() {
    let mut m = synced(0x001, 20);
    m.set_assembly_via_staop(true);
    m.set_assembly(0, 0);
    m.on_assembly_failed(0, 0);
    assert!(m.next_assembly_push(5000).is_some());
    assert_eq!(m.valve(0).push_attempts, 1);
    m.on_target_push_dropped(0, 5000);
    assert_eq!(m.valve(0).sync, TargetSync::Pending);
    assert_eq!(m.valve(0).push_attempts, 0);
    m.on_assembly_ack(0, 5100); // no staop outstanding any more
    assert_eq!(m.valve(0).sync, TargetSync::Pending);
}

// ================================================================ forced push after a reboot (L18)

#[test]
fn reboot_one_stgtp_per_valve_even_when_the_read_back_equals_then_none() {
    let mut m = synced(0x005, 30);
    m.on_stm_rebooted(1000);
    assert!(m.valve(0).force_push);
    assert!(m.valve(2).force_push);
    assert!(!m.valve(1).force_push); // no desired target
    m.apply_target(&target(0, 30), 1100);
    m.apply_target(&target(2, 30), 1100);
    assert_eq!(m.next_target_push(1200), Some((0, 30)));
    assert!(!m.valve(0).force_push);
    assert_eq!(m.next_target_push(1200).map(|(valve, _)| valve), Some(2));
    assert_eq!(m.next_target_push(1200), None);
    m.on_target_ack(0, 1300);
    m.on_target_ack(2, 1300);
    m.apply_target(&target(0, 30), 1400);
    m.apply_target(&target(2, 30), 1400);
    assert_eq!(m.valve(0).sync, TargetSync::Synced);
    assert_eq!(m.valve(2).sync, TargetSync::Synced);
    assert_eq!(m.next_target_push(100000), None);
}

// ================================================================ restored targets (W2)

#[test]
fn restore_desired_active_valves_only_restored_or_assembly_pending() {
    let mut m = ValveModel::default();
    m.set_active_mask(0x00B);
    let rev = m.desired_revision();
    assert!(m.restore_desired(0, 42, TargetSource::Mqtt));
    assert_eq!(m.desired_revision(), rev + 1);
    assert_eq!(m.valve(0).desired, 42);
    assert!(m.valve(0).desired_valid);
    assert_eq!(m.valve(0).source, TargetSource::Restored);
    assert_eq!(m.valve(0).sync, TargetSync::Pending);
    assert!(m.restore_desired(1, 100, TargetSource::Assembly));
    assert_eq!(m.valve(1).source, TargetSource::Assembly);
    assert!(m.restore_desired(3, 7, TargetSource::Web));
    assert_eq!(m.valve(3).source, TargetSource::Restored);
    assert!(m.restore_desired(3, 8, TargetSource::Stm));
    assert_eq!(m.valve(3).source, TargetSource::Restored);
    assert!(!m.restore_desired(2, 42, TargetSource::Mqtt)); // inactive
    assert!(!m.valve(2).desired_valid);
    assert!(!m.restore_desired(0, 101, TargetSource::Mqtt));
    assert!(!m.restore_desired(12, 1, TargetSource::Mqtt));
    assert_eq!(m.valve(0).desired, 42);
    assert!(m.restore_desired(0, 100, TargetSource::Mqtt));
    assert!(m.restore_desired(0, 0, TargetSource::Mqtt));
    assert_eq!(m.valve(0).desired, 0);
}

#[test]
fn restore_desired_no_push_before_the_read_back_equal_read_back_is_synced_without_one() {
    let mut m = ValveModel::default();
    m.set_active_mask(0x003);
    assert!(m.restore_desired(0, 42, TargetSource::Mqtt));
    assert!(m.restore_desired(1, 42, TargetSource::Mqtt));
    m.apply_valve_data(&data(0), 100); // known, but the STM target is not read yet
    m.apply_valve_data(&data(1), 100);
    assert_eq!(m.next_target_push(1000), None);
    let rev = m.desired_revision();
    m.apply_target(&target(0, 42), 1100);
    assert_eq!(m.valve(0).sync, TargetSync::Synced);
    assert_eq!(m.valve(0).push_attempts, 0);
    assert_eq!(m.valve(0).source, TargetSource::Restored);
    m.apply_target(&target(1, 50), 1100);
    assert_eq!(m.valve(1).sync, TargetSync::Pending);
    assert_eq!(m.desired_revision(), rev); // read-backs of restored valves change no desired value
    assert_eq!(m.next_target_push(1200), Some((1, 42)));
}

#[test]
fn restore_desired_a_web_target_before_the_read_back_wins() {
    let mut m = ValveModel::default();
    m.set_active_mask(0x001);
    assert!(m.restore_desired(0, 42, TargetSource::Mqtt));
    m.apply_valve_data(&data(0), 100);
    assert!(m.set_desired_target(0, 60, TargetSource::Web, 200));
    assert_eq!(m.next_target_push(300).map(|(_, pos)| pos), Some(60));
}

#[test]
fn desired_revision_web_targets_assembly_and_adoption_count_read_backs_pushes_and_overrides_do_not()
{
    let mut m = ValveModel::default();
    m.set_active_mask(0x003);
    let mut rev = m.desired_revision();
    m.apply_target(&target(1, 33), 0); // adoption
    rev += 1;
    assert_eq!(m.desired_revision(), rev);
    m.apply_target(&target(1, 34), 0); // later read-back
    assert_eq!(m.desired_revision(), rev);
    m.apply_valve_data(&data(0), 0);
    assert!(m.set_desired_target(0, 20, TargetSource::Web, 0));
    rev += 1;
    assert_eq!(m.desired_revision(), rev);
    assert!(m.set_desired_target(0, 20, TargetSource::Web, 0)); // same value
    assert_eq!(m.desired_revision(), rev);
    let (valve, _) = m.next_target_push(0).expect("stgtp");
    m.on_target_ack(valve, 10);
    m.apply_target(&target(0, 20), 20);
    assert_eq!(m.desired_revision(), rev);
    m.set_failsafe_drive(0x001, &PCT_50);
    assert_eq!(m.desired_revision(), rev);
    m.set_failsafe_drive(0, &PCT_50);
    assert_eq!(m.desired_revision(), rev);
    m.set_assembly(0, 30);
    rev += 1;
    assert_eq!(m.desired_revision(), rev);
    assert!(m.set_desired_target(0, 20, TargetSource::Mqtt, 40));
    rev += 1;
    assert_eq!(m.desired_revision(), rev);
}

// ================================================================ failsafe emulation (K1)

#[test]
fn failsafe_drive_the_override_pushes_pct_desired_stays_its_end_pushes_desired() {
    let mut m = synced(0x001, 30);
    let pct = [50u8; 12];
    m.set_failsafe_drive(0x001, &pct);
    let v = *m.valve(0);
    assert!(v.fs_override);
    assert_eq!(v.fs_target, 50);
    assert_eq!(v.desired, 30);
    assert_eq!(v.sync, TargetSync::Pending);
    assert_eq!(m.push_target(0), 50);
    assert_ne!(v.health & HEALTH_FAILSAFE, 0);
    assert!(valve_at_failsafe(&v));
    assert_eq!(m.next_target_push(1000).map(|(_, pos)| pos), Some(50));
    m.on_target_ack(0, 1100);
    m.apply_target(&target(0, 50), 1200);
    assert_eq!(m.valve(0).sync, TargetSync::Synced);
    assert_eq!(m.valve(0).desired, 30);
    // A target set during the failsafe is stored and waits.
    assert!(m.set_desired_target(0, 40, TargetSource::Web, 1300));
    assert_eq!(m.valve(0).desired, 40);
    assert_eq!(m.valve(0).sync, TargetSync::Synced);
    assert_eq!(m.next_target_push(100000), None);
    // The same mask again changes nothing.
    m.set_failsafe_drive(0x001, &pct);
    assert_eq!(m.valve(0).sync, TargetSync::Synced);
    m.set_failsafe_drive(0, &pct);
    assert!(!m.valve(0).fs_override);
    assert_eq!(m.valve(0).fs_target, 0);
    assert_eq!(m.valve(0).sync, TargetSync::Pending);
    assert_eq!(m.valve(0).health & HEALTH_FAILSAFE, 0);
    assert_eq!(m.next_target_push(200000).map(|(_, pos)| pos), Some(40));
}

#[test]
fn failsafe_drive_a_known_stm_target_equal_to_pct_is_synced_at_once() {
    let mut m = synced(0x001, 50);
    m.set_failsafe_drive(0x001, &PCT_50);
    assert!(m.valve(0).fs_override);
    assert_eq!(m.valve(0).sync, TargetSync::Synced);
    let mut pct = [50u8; 12];
    pct[0] = 60;
    m.set_failsafe_drive(0x001, &pct); // another failsafe position: Pending, attempts reset
    assert_eq!(m.valve(0).sync, TargetSync::Pending);
    assert_eq!(m.valve(0).push_attempts, 0);
    assert_eq!(m.push_target(0), 60);
}

#[test]
fn failsafe_drive_assembly_inactive_no_desired_target_and_hold_are_never_overridden() {
    let mut m = synced(0x007, 30);
    m.set_assembly(1, 0);
    m.set_active_mask(0x00B); // valve 2 inactive, valve 3 active without a desired target
    let mut pct = [50u8; 12];
    pct[0] = FAILSAFE_HOLD;
    m.set_failsafe_drive(0x0FFF, &pct);
    assert!(!m.valve(0).fs_override); // hold
    assert!(!m.valve(1).fs_override); // assembly
    assert!(!m.valve(2).fs_override); // inactive
    assert!(!m.valve(3).fs_override); // no desired target
    assert!(!m.valve(4).fs_override);
    assert_eq!(m.valve(0).fs_pct, FAILSAFE_HOLD); // fsPct follows the ESP config without v3 data
    assert_eq!(m.valve(1).fs_pct, 50);
    assert_eq!(m.push_target(12), 0);
}

#[test]
fn failsafe_drive_protocol_3_valves_keep_the_stm_fs_pct() {
    let mut m = ValveModel::default();
    m.set_active_mask(0x001);
    let mut d = ex_v3(0, 30, 0);
    d.fs_pct = 70;
    m.apply_valve_ex(&d, 0);
    m.set_failsafe_drive(0, &PCT_50);
    assert_eq!(m.valve(0).fs_pct, 70);
}

#[test]
fn set_assembly_ends_a_running_failsafe_override_of_that_valve() {
    let mut m = synced(0x001, 30);
    m.set_failsafe_drive(0x001, &PCT_50);
    assert!(m.valve(0).fs_override);
    m.set_assembly(0, 0);
    assert!(!m.valve(0).fs_override);
    assert_eq!(m.push_target(0), 100);
}

// ================================================================ unsupported STM (W13)

#[test]
fn forget_stm_data_stm_data_back_to_defaults_desired_and_the_override_kept() {
    let mut m = synced(0x003, 30);
    m.apply_valve_ex(&ex_v3(1, 30, STM_FLAG_FS_LEASE), 0);
    m.set_failsafe_drive(0x001, &PCT_50);
    let rev = m.valve(0).revision;
    let drev = m.desired_revision();
    m.forget_stm_data();
    let v = *m.valve(0);
    assert!(!v.known);
    assert_eq!(v.status, 0);
    assert_eq!(v.position, 0);
    assert_eq!(v.open_count, 0);
    assert!(!v.stm_target_known);
    assert!(v.desired_valid);
    assert_eq!(v.desired, 30);
    assert_eq!(v.source, TargetSource::Stm);
    assert_eq!(v.sync, TargetSync::Pending);
    assert!(v.fs_override);
    assert_eq!(v.fs_target, 50);
    assert_eq!(v.revision, rev + 1);
    assert_eq!(m.desired_revision(), drev);
    assert!(!m.valve(1).has_v3);
    assert_eq!(m.valve(1).stm_flags, 0);
    // No push before the STM target is read back again; a gtgtp enables it.
    assert_eq!(m.next_target_push(100000), None);
    m.apply_target(&target(0, 20), 100000);
    assert_eq!(m.next_target_push(100000), Some((0, 50)));
}

#[test]
fn forget_stm_data_a_valve_without_a_desired_target_is_unknown() {
    let mut m = ValveModel::default();
    m.set_active_mask(0x001);
    m.apply_valve_data(&data(0), 0);
    m.forget_stm_data();
    assert_eq!(m.valve(0).sync, TargetSync::Unknown);
    assert!(!m.valve(0).desired_valid);
}

// ================================================================ sensors (E9, S1)

#[test]
fn sensor_model_one_failed_temperature_is_held_the_second_applies_good_resets() {
    let mut s = SensorModel::default();
    let good = TempData {
        valid: true,
        id: id_with_crc(1),
        value: 215,
    };
    s.apply_temp_data(0, &good, 100);
    let mut bad = good;
    bad.value = TEMP_READ_ERROR;
    s.apply_temp_data(0, &bad, 200);
    assert_eq!(s.temp(0).raw, 215);
    assert_eq!(s.temp(0).fail_streak, 1);
    assert_eq!(s.temp(0).last_seen_ms, 100);
    assert!(s.temp_fresh(0, 200, 60000));
    s.apply_temp_data(0, &bad, 300);
    assert_eq!(s.temp(0).raw, TEMP_READ_ERROR);
    assert_eq!(s.temp(0).fail_streak, 2);
    assert_eq!(s.temp(0).last_seen_ms, 300);
    assert!(s.temp(0).seen);
    s.apply_temp_data(0, &good, 400);
    assert_eq!(s.temp(0).fail_streak, 0);
    assert_eq!(s.temp(0).raw, 215);
    let none = TempData::default(); // "goned 0" counts like a failure
    s.apply_temp_data(0, &none, 500);
    assert_eq!(s.temp(0).raw, 215);
    assert!(s.temp(0).seen);
    s.apply_temp_data(0, &none, 600);
    assert!(!s.temp(0).seen);
    assert_eq!(s.temp(0).raw, TEMP_UNASSIGNED);
    for _ in 0..300 {
        s.apply_temp_data(0, &bad, 700);
    }
    assert_eq!(s.temp(0).fail_streak, 255);
    for raw in [TEMP_POWER_ON, 1251, TEMP_UNASSIGNED] {
        s.apply_temp_data(0, &good, 800);
        bad.value = raw;
        s.apply_temp_data(0, &bad, 900);
        assert_eq!(s.temp(0).raw, 215, "{raw}");
    }
}

#[test]
fn sensor_model_one_failed_volt_reading_is_held_the_second_applies() {
    let mut s = SensorModel::default();
    let good = VoltData {
        valid: true,
        id: id_with_crc(1),
        vad: 330,
    };
    s.apply_volt_data(0, &good, 100);
    let mut bad = good;
    bad.vad = VAD_FAILED;
    s.apply_volt_data(0, &bad, 200);
    assert_eq!(s.volt(0).vad, 330);
    assert_eq!(s.volt(0).fail_streak, 1);
    s.apply_volt_data(0, &bad, 300);
    assert_eq!(s.volt(0).vad, VAD_FAILED);
    assert!(s.volt(0).seen);
    assert_eq!(s.volt(0).last_seen_ms, 300);
    s.apply_volt_data(0, &good, 400);
    assert_eq!(s.volt(0).fail_streak, 0);
    for _ in 0..300 {
        s.apply_volt_data(0, &bad, 500);
    }
    assert_eq!(s.volt(0).fail_streak, 255);
}

#[test]
fn sensor_model_a_stray_reading_lands_on_the_bus_index_of_its_id() {
    let mut s = SensorModel::default();
    let mut l = OneWireList {
        count: 5,
        has_list: true,
        ..OneWireList::default()
    };
    for (i, id) in (1..=5u8).zip(l.ids.iter_mut()) {
        *id = id_with_crc(i);
    }
    s.apply_temp_list(&l, 0);
    s.apply_volt_list(&l, 0);
    let mut d = TempData {
        valid: true,
        id: id_with_crc(5),
        value: 222,
    };
    assert!(s.apply_stray_temp_data(&d, 100));
    assert_eq!(s.temp(4).raw, 222);
    assert_eq!(s.temp(4).last_seen_ms, 100);
    d.id = id_with_crc(9);
    assert!(!s.apply_stray_temp_data(&d, 200));
    for i in 0..4 {
        assert!(!s.temp(i).seen);
    }
    let none = TempData::default();
    assert!(!s.apply_stray_temp_data(&none, 300));
    let mut v = VoltData {
        valid: true,
        id: id_with_crc(2),
        vad: 55,
    };
    assert!(s.apply_stray_volt_data(&v, 400));
    assert_eq!(s.volt(1).vad, 55);
    v.id = id_with_crc(9);
    assert!(!s.apply_stray_volt_data(&v, 500));
    let vnone = VoltData::default();
    assert!(!s.apply_stray_volt_data(&vnone, 600));
    assert!(!s.volt(0).seen);
}

#[test]
fn sensor_model_the_stm_temperature_age_makes_readings_stale_above_200_s() {
    let mut s = SensorModel::default();
    let d = TempData {
        valid: true,
        id: id_with_crc(1),
        value: 215,
    };
    s.apply_temp_data(0, &d, 100000);
    s.set_stm_temp_age(180, 100000);
    assert!(s.temp_fresh(0, 100000, 60000));
    assert!(s.temp_fresh(0, 120999, 60000)); // 180 + 20 = 200
    assert!(!s.temp_fresh(0, 121000, 60000));
    s.set_stm_temp_age(201, 121000);
    assert!(!s.temp_fresh(0, 121000, 60000));
    s.set_stm_temp_age(200, 121000);
    assert!(s.temp_fresh(0, 121000, 60000));
    s.set_stm_temp_age(0xFFFF_FFFF, 121000); // no overflow
    assert!(!s.temp_fresh(0, 121000, 60000));
}
