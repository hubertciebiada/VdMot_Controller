//! Port of test/native/test_mqtt_values.cpp: systemState (legacy common/state), sensor slots,
//! segments, targets, problem flag, names, calibration ends, the on-change key of a valve.
//!
//! C++ cases that pass a null pointer have no Rust form; where the C++ null array means "no
//! valves" (with a count) the empty slice is tested, where it is a full snapshot that may be
//! missing it is `None`. The C++ checks of the NUL a builder writes are the returned lengths.

use super::*;
use crate::common::{copy_string, TEMP_READ_ERROR, VAD_FAILED};
use crate::test_support::assert_text;
use crate::valve_model::{
    diff_valve, HEALTH_CALIB_RETRIES, HEALTH_CMD_REJECTED, HEALTH_EARLY_STOP, HEALTH_FAILSAFE,
    HEALTH_STROKE_SHORT,
};
use std::boxed::Box;
use std::vec::Vec;

/// A valve with a value in every field (none at its default).
pub(super) fn full_valve() -> ValveState {
    let mut v = ValveState {
        known: true,
        last_seen_ms: 1000,
        status: 3,
        calibrating: true,
        position: 40,
        mean_current: 12,
        temp1: 215,
        temp2: 198,
        moves: 70,
        open_count: 30,
        close_count: 29,
        dead_zone: -4,
        calib_retries: 1,
        has_extended: true,
        cal_state: 2,
        cal_flags: 1,
        early_stops: 5,
        cmd_rejected: 6,
        early_stops_at_boot: 2,
        cmd_rejected_at_boot: 3,
        move_seq: 8,
        desired_valid: true,
        desired: 55,
        source: TargetSource::Web,
        stm_target_known: true,
        stm_target: 50,
        sync: TargetSync::Pending,
        push_attempts: 2,
        last_push_ms: 900,
        sensor_slot: [3, 4],
        health: 1,
        revision: 17,
        has_v3: true,
        stm_flags: 2,
        fault: 1,
        fs_pct: 30,
        drive: 45,
        retry_s: 60,
        retries: 1,
        auto_retry: true,
        fs_override: true,
        fs_target: 20,
        force_push: true,
        ..ValveState::default()
    };
    v.last_move.counted_counts = 100;
    v.sensor_id[0].b[0] = 0x28;
    v.sensor_id[1].b[7] = 0x11;
    v
}

type Change = (&'static str, fn(&mut ValveState));

#[test]
fn valve_compat_key_exactly_the_fields_of_the_compat_groups_change_the_key() {
    let base = full_valve();
    let key = valve_compat_key(&base);
    assert_eq!(valve_compat_key(&full_valve()), key);
    let changes: [Change; 49] = [
        ("known", |v| v.known = false),
        ("lastSeenMs", |v| v.last_seen_ms += 1),
        ("status", |v| v.status += 1),
        ("calibrating", |v| v.calibrating = false),
        ("position", |v| v.position += 1),
        ("meanCurrent", |v| v.mean_current += 1),
        ("temp1", |v| v.temp1 += 1),
        ("temp2", |v| v.temp2 += 1),
        ("moves", |v| v.moves += 1),
        ("openCount", |v| v.open_count += 1),
        ("closeCount", |v| v.close_count += 1),
        ("deadZone", |v| v.dead_zone += 1),
        ("calibRetries", |v| v.calib_retries += 1),
        ("hasExtended", |v| v.has_extended = false),
        ("calState", |v| v.cal_state += 1),
        ("calFlags", |v| v.cal_flags += 1),
        ("earlyStops", |v| v.early_stops += 1),
        ("cmdRejected", |v| v.cmd_rejected += 1),
        ("earlyStopsAtBoot", |v| v.early_stops_at_boot += 1),
        ("cmdRejectedAtBoot", |v| v.cmd_rejected_at_boot += 1),
        ("lastMove", |v| v.last_move.counted_counts += 1),
        ("moveSeq", |v| v.move_seq += 1),
        ("desiredValid", |v| v.desired_valid = false),
        ("desired", |v| v.desired += 1),
        ("source", |v| v.source = TargetSource::Mqtt),
        ("stmTargetKnown", |v| v.stm_target_known = false),
        ("stmTarget", |v| v.stm_target += 1),
        ("sync", |v| v.sync = TargetSync::Synced),
        ("pushAttempts", |v| v.push_attempts += 1),
        ("lastPushMs", |v| v.last_push_ms += 1),
        ("sensorId[0] first byte", |v| v.sensor_id[0].b[0] += 1),
        ("sensorId[0] last byte", |v| v.sensor_id[0].b[7] += 1),
        ("sensorId[1] first byte", |v| v.sensor_id[1].b[0] += 1),
        ("sensorId[1] last byte", |v| v.sensor_id[1].b[7] += 1),
        ("sensorSlot[0]", |v| v.sensor_slot[0] += 1),
        ("sensorSlot[1]", |v| v.sensor_slot[1] += 1),
        ("health", |v| v.health = 0x100),
        ("revision", |v| v.revision += 1),
        ("hasV3", |v| v.has_v3 = false),
        ("stmFlags", |v| v.stm_flags = 0x100),
        ("fault", |v| v.fault += 1),
        ("fsPct", |v| v.fs_pct += 1),
        ("drive", |v| v.drive += 1),
        ("retryS", |v| v.retry_s += 1),
        ("retries", |v| v.retries += 1),
        ("autoRetry", |v| v.auto_retry = false),
        ("fsOverride", |v| v.fs_override = false),
        ("fsTarget", |v| v.fs_target += 1),
        ("forcePush", |v| v.force_push = false),
    ];
    let mut keyed = 0;
    for (name, change) in changes {
        let mut v = base;
        change(&mut v);
        let compat = diff_valve(&base, &v) & VALVE_COMPAT_MASK != 0;
        assert_eq!(valve_compat_key(&v) != key, compat, "{name}");
        keyed += usize::from(compat);
    }
    assert_eq!(keyed, 33); // the fields of the compat groups (sensor ids at both ends)
}

#[test]
fn valve_compat_key_the_default_valve_and_the_padding() {
    // C++ memsets every byte of b (padding included), then assigns ValveState{}: the key reads
    // the fields only, so the reassigned valve keys like a new one.
    let a = ValveState::default();
    let mut b = full_valve();
    assert_ne!(valve_compat_key(&b), valve_compat_key(&a));
    b = ValveState::default();
    assert_eq!(valve_compat_key(&a), valve_compat_key(&b));
    assert_ne!(valve_compat_key(&a), valve_compat_key(&full_valve()));
}

#[test]
fn system_state_cases() {
    let none = SystemFlags::default();
    // C++ systemState(link, nullptr, 0, 0): the empty slice.
    assert_eq!(system_state(LinkState::Up, &[], 0, none), 0);
    assert_eq!(system_state(LinkState::Unknown, &[], 0, none), 1);
    assert_eq!(system_state(LinkState::Degraded, &[], 0, none), 1);
    assert_eq!(system_state(LinkState::Booting, &[], 0, none), 1);
    assert_eq!(system_state(LinkState::Suspended, &[], 0, none), 1);
    assert_eq!(system_state(LinkState::Down, &[], 0, none), 2);

    let mut v = [ValveState::default(); N];
    assert_eq!(system_state(LinkState::Up, &v, 0x0FFF, none), 0);
    v[4].health = HEALTH_BLOCKED;
    assert_eq!(system_state(LinkState::Up, &v, 0x0FFF, none), 2);
    assert_eq!(system_state(LinkState::Up, &v, 0x0FEF, none), 0); // inactive
    assert_eq!(system_state(LinkState::Up, &v[..4], 0x0FFF, none), 0); // beyond count
    assert_eq!(system_state(LinkState::Up, &v[..5], 0x0FFF, none), 2);
    v[4].health = HEALTH_FAILED;
    assert_eq!(system_state(LinkState::Degraded, &v, 0x0010, none), 2);
    let others = [
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
    for f in others {
        v[4].health = f;
        assert_eq!(system_state(LinkState::Up, &v, 0x0010, none), 1, "{f:#x}");
        assert_eq!(system_state(LinkState::Up, &v, 0x0000, none), 0, "{f:#x}");
        assert_eq!(system_state(LinkState::Down, &v, 0x0010, none), 2, "{f:#x}");
    }
    // Blocked on a later valve wins over an info flag on an earlier one.
    v[0].health = HEALTH_STALE;
    v[11].health = HEALTH_BLOCKED;
    assert_eq!(system_state(LinkState::Up, &v, 0x0FFF, none), 2);
    // The first valve counts as well.
    v[11].health = 0;
    v[0].health = HEALTH_BLOCKED;
    assert_eq!(system_state(LinkState::Up, &v[..1], 0x0001, none), 2);
    // count above 12 is clamped (no read past the array): C++ count 255 on 12 valves. The
    // Rust slice can be longer than 12; the entries past 12 never count.
    v[11].health = 0;
    v[0].health = 0;
    v[4].health = 0;
    assert_eq!(system_state(LinkState::Up, &v, 0xFFFF, none), 0);
    let mut long = [ValveState::default(); 16];
    long[12].health = HEALTH_BLOCKED;
    long[15].health = HEALTH_STALE;
    assert_eq!(system_state(LinkState::Up, &long, 0xFFFF, none), 0);
}

#[test]
fn system_state_safe_mode_and_failsafe() {
    let v = [ValveState::default(); N];
    let safe = SystemFlags {
        safe_mode: true,
        ..SystemFlags::default()
    };
    let fs = SystemFlags {
        failsafe: true,
        ..SystemFlags::default()
    };
    let both = SystemFlags {
        safe_mode: true,
        failsafe: true,
    };
    assert_eq!(
        system_state(LinkState::Up, &v, 0x0FFF, SystemFlags::default()),
        0
    );
    assert_eq!(system_state(LinkState::Up, &v, 0x0FFF, safe), 2);
    assert_eq!(system_state(LinkState::Up, &[], 0, safe), 2);
    assert_eq!(system_state(LinkState::Up, &v, 0x0FFF, fs), 1);
    assert_eq!(system_state(LinkState::Up, &[], 0, fs), 1);
    assert_eq!(system_state(LinkState::Up, &v, 0x0FFF, both), 2);
    assert_eq!(system_state(LinkState::Degraded, &v, 0x0FFF, safe), 2);
    assert_eq!(system_state(LinkState::Down, &v, 0x0FFF, fs), 2);
    // An error of a valve outranks the failsafe.
    let mut v = v;
    v[2].health = HEALTH_FAILED;
    assert_eq!(system_state(LinkState::Up, &v, 0x0FFF, fs), 2);
    v[2].health = HEALTH_FAILSAFE;
    assert_eq!(system_state(LinkState::Up, &v, 0x0FFF, fs), 1);
    assert_eq!(
        system_state(LinkState::Up, &v, 0x0FFF, SystemFlags::default()),
        1
    );
}

fn id(last: u8) -> OneWireId {
    let mut o = OneWireId::default();
    o.b[0] = 0x28;
    o.b[7] = last;
    o
}

/// C++ `seg(c, k, i, bus, cap)`: the segment written into a 16-byte buffer of capacity `cap`.
fn seg(c: &Config, k: ItemKind, i: u8, bus: Option<u8>, cap: usize) -> Vec<u8> {
    let mut out = [b'X'; 16];
    let n = sensor_topic_segment(c, k, i, bus, &mut out[..cap]);
    out[..n].to_vec()
}

fn seg16(c: &Config, k: ItemKind, i: u8, bus: Option<u8>) -> Vec<u8> {
    seg(c, k, i, bus, 16)
}

#[test]
fn active_valve_mask_cases() {
    let mut c = Box::new(Config::default());
    assert_eq!(active_valve_mask(&c), 0);
    c.valves[0].active = true;
    c.valves[11].active = true;
    c.valves[5].active = true;
    assert_eq!(active_valve_mask(&c), 0x0821);
}

#[test]
fn slot_temperatures_and_volts_from_the_bus_readings() {
    let mut c = Box::new(Config::default());
    c.temps[2].id = id(3);
    c.temps[2].offset = -5;
    let mut t = [TempReading::default(); 3];
    t[1].id = id(3);
    t[1].seen = true;
    t[1].raw = 215;
    t[1].last_seen_ms = 1000;
    assert_eq!(slot_temp_tenths(&c, &t, 3, 61000, 60000), Some(210));
    assert_eq!(slot_temp_tenths(&c, &t, 3, 61001, 60000), None); // stale
    assert_eq!(slot_temp_tenths(&c, &t[..1], 3, 1000, 60000), None); // beyond count
    assert_eq!(slot_temp_tenths(&c, &t, 0, 1000, 60000), None);
    assert_eq!(
        slot_temp_tenths(&c, &t, TEMP_SLOT_COUNT + 1, 1000, 60000),
        None
    );
    assert_eq!(slot_temp_tenths(&c, &t, 2, 1000, 60000), None); // empty slot
    t[1].raw = TEMP_READ_ERROR;
    assert_eq!(slot_temp_tenths(&c, &t, 3, 1000, 60000), None);
    t[1].raw = 215;
    t[1].seen = false;
    assert_eq!(slot_temp_tenths(&c, &t, 3, 1000, 60000), None);
    // C++ slotTempTenths(c, nullptr, 3, ..): the empty slice.
    assert_eq!(slot_temp_tenths(&c, &[], 3, 1000, 60000), None);
    // C++ "tenths unchanged" after the failures: the None results above.

    c.volts[1].id = id(9);
    c.volts[1].offset = 1.0;
    c.volts[1].factor = 2.0;
    let mut v = [VoltReading::default(); 2];
    v[0].id = id(9);
    v[0].seen = true;
    v[0].vad = 1234;
    v[0].last_seen_ms = 0;
    let value = slot_volt_value(&c, &v, 1, 60000, 60000).expect("value");
    assert!((value - 26.68).abs() < 1e-9, "{value}");
    assert_eq!(slot_volt_value(&c, &v, 1, 60001, 60000), None);
    assert_eq!(slot_volt_value(&c, &v[..0], 1, 0, 60000), None);
    assert_eq!(slot_volt_value(&c, &v, 0, 0, 60000), None);
    assert_eq!(slot_volt_value(&c, &v, VOLT_SLOT_COUNT, 0, 60000), None);
    v[0].vad = VAD_FAILED;
    assert_eq!(slot_volt_value(&c, &v, 1, 0, 60000), None);
    v[0].vad = 1234;
    v[0].seen = false;
    assert_eq!(slot_volt_value(&c, &v, 1, 0, 60000), None);
}

#[test]
fn published_temps_and_volts() {
    let mut c = Box::new(Config::default());
    let mut valves = [ValveState::default(); N];
    c.temps[4].active = true;
    assert!(!temp_published(&c, Some(&valves), 4)); // no id
    c.temps[4].id = id(5);
    assert!(temp_published(&c, Some(&valves), 4));
    valves[7].sensor_slot[1] = 5; // slot 5 (1-based) = index 4
    assert!(temp_assigned_to_valve(Some(&valves), 5));
    assert!(!temp_assigned_to_valve(Some(&valves), 4));
    assert!(!temp_assigned_to_valve(Some(&valves), 0));
    assert!(!temp_assigned_to_valve(None, 5));
    assert!(temp_published(&c, Some(&valves), 4)); // all_temps
    c.mqtt.all_temps = false;
    assert!(!temp_published(&c, Some(&valves), 4));
    valves[7].sensor_slot[1] = 0;
    valves[0].sensor_slot[0] = 5;
    assert!(!temp_published(&c, Some(&valves), 4));
    assert!(temp_published(&c, None, 4));
    c.temps[4].active = false;
    assert!(!temp_published(&c, None, 4));
    assert!(!temp_published(&c, None, TEMP_SLOT_COUNT));
    // E23: every configured volt slot is published; discovery only the active ones.
    c.volts[3].id = id(1);
    assert!(volt_published_mqtt(&c, 3));
    assert!(!volt_announced(&c, 3));
    c.volts[3].active = true;
    assert!(volt_announced(&c, 3));
    c.volts[3].id = OneWireId::default();
    assert!(!volt_published_mqtt(&c, 3));
    assert!(!volt_announced(&c, 3));
    assert!(!volt_published_mqtt(&c, VOLT_SLOT_COUNT));
    assert!(!volt_announced(&c, VOLT_SLOT_COUNT));
}

#[test]
fn bus_index_and_sensor_topic_segments_e22() {
    let mut t = [TempReading::default(); 4];
    t[2].id = id(7);
    t[3].id = id(8);
    assert_eq!(find_temp_bus(&t, &id(7)), Some(2));
    assert_eq!(find_temp_bus(&t, &id(8)), Some(3));
    assert_eq!(find_temp_bus(&t[..3], &id(8)), None);
    assert_eq!(find_temp_bus(&t, &OneWireId::default()), None);
    // C++ findTempBus(nullptr, 4, ..): the empty slice.
    assert_eq!(find_temp_bus(&[], &id(7)), None);
    // C++ findTempBus(t, 255, ..): the count is the slice; a longer bus counts 34 entries.
    assert_eq!(find_temp_bus(&t, &id(7)), Some(2));
    let mut v = [VoltReading::default(); 2];
    v[1].id = id(9);
    assert_eq!(find_volt_bus(&v, &id(9)), Some(1));
    assert_eq!(find_volt_bus(&v[..1], &id(9)), None);
    assert_eq!(find_volt_bus(&v, &OneWireId::default()), None);
    assert_eq!(find_volt_bus(&[], &id(9)), None);
    let mut many = [VoltReading::default(); VOLT_SLOT_COUNT as usize + 1];
    many[usize::from(VOLT_SLOT_COUNT)].id = id(9); // beyond the bus maximum
    assert_eq!(find_volt_bus(&many, &id(9)), None);

    let mut c = Box::new(Config::default());
    assert_text(&seg16(&c, ItemKind::Temp, 0, Some(2)), "3");
    assert_text(&seg16(&c, ItemKind::Temp, 0, Some(0)), "1");
    assert_text(&seg16(&c, ItemKind::Temp, 0, None), "");
    copy_string(&mut c.temps[0].name, b"Wohn zi");
    assert_text(&seg16(&c, ItemKind::Temp, 0, Some(2)), "Wohn_zi");
    assert_text(&seg16(&c, ItemKind::Temp, 0, None), "Wohn_zi");
    copy_string(&mut c.temps[1].topic, b"Bad/WC");
    assert_text(&seg16(&c, ItemKind::Temp, 1, None), "Bad/WC");
    assert_text(&seg16(&c, ItemKind::Volt, 7, Some(33)), "34");
    assert_text(&seg(&c, ItemKind::Volt, 7, Some(33), 2), ""); // does not fit
    assert_text(&seg(&c, ItemKind::Volt, 7, Some(33), 3), "34");
    copy_string(&mut c.volts[7].name, b"Batt");
    assert_text(&seg16(&c, ItemKind::Volt, 7, None), "Batt");
    assert_text(&seg16(&c, ItemKind::Volt, VOLT_SLOT_COUNT, Some(1)), "");
    assert_text(&seg16(&c, ItemKind::Temp, TEMP_SLOT_COUNT, Some(1)), "");
    assert_text(&seg16(&c, ItemKind::Valve, 0, Some(1)), "");
    // C++ sensorTopicSegment(.., nullptr, 4): no Rust form. Capacity 0 writes nothing.
    let mut one = [b'X'; 1];
    assert_eq!(
        sensor_topic_segment(&c, ItemKind::Temp, 5, Some(1), &mut one[..0]),
        0
    );
    assert_eq!(one[0], b'X');
}

#[test]
fn published_target_w5() {
    let mut v = ValveState::default();
    // Not separate: the desired target.
    assert_eq!(published_target(&v, false), None);
    v.desired_valid = true;
    v.desired = 60;
    v.stm_target_known = true;
    v.stm_target = 30;
    assert_eq!(published_target(&v, false), Some(60));
    // Separate: the read-back.
    assert_eq!(published_target(&v, true), Some(30));
    v.stm_target_known = false;
    // C++ "out unchanged" after a false: the None result.
    assert_eq!(published_target(&v, true), None);
    // ESP emulation: the desired target (the STM holds the failsafe position).
    v.fs_override = true;
    assert_eq!(published_target(&v, true), Some(60));
    v.desired_valid = false;
    assert_eq!(published_target(&v, true), None);
    // A restored target is held until it was synced.
    v = ValveState {
        desired_valid: true,
        desired: 50,
        source: TargetSource::Restored,
        sync: TargetSync::Pending,
        stm_target_known: true,
        stm_target: 50,
        ..ValveState::default()
    };
    assert_eq!(published_target(&v, true), None);
    assert_eq!(published_target(&v, false), Some(50)); // not separate: unchanged rule
    v.sync = TargetSync::Synced;
    assert_eq!(published_target(&v, true), Some(50));
    v.source = TargetSource::Web;
    v.sync = TargetSync::Pending;
    assert_eq!(published_target(&v, true), Some(50));
}

#[test]
fn valve_problem_and_stm_online() {
    let mut v = ValveState::default();
    assert!(!valve_problem(&v));
    let problems = [
        HEALTH_BLOCKED,
        HEALTH_FAILED,
        HEALTH_NO_VALVE,
        HEALTH_STALE,
        HEALTH_TARGET_UNCONFIRMED,
        HEALTH_TEMP_FAILED,
    ];
    for f in problems {
        v.health = f;
        assert!(valve_problem(&v), "{f:#x}");
    }
    for f in [
        HEALTH_CALIB_RETRIES,
        HEALTH_EARLY_STOP,
        HEALTH_CMD_REJECTED,
        HEALTH_FAILSAFE,
        HEALTH_STROKE_SHORT,
    ] {
        v.health = f;
        assert!(!valve_problem(&v), "{f:#x}");
    }
    assert_eq!(PROBLEM_MASK, 0x01C7);
    assert!(stm_online(LinkState::Up));
    assert!(stm_online(LinkState::Degraded));
    for s in [
        LinkState::Unknown,
        LinkState::Down,
        LinkState::Booting,
        LinkState::Suspended,
    ] {
        assert!(!stm_online(s), "{s:?}");
    }
}

#[test]
fn valve_display_names() {
    let mut out = [0u8; 16];
    assert_eq!(valve_display_name(b"Bad 1", 0, &mut out), 5);
    assert_text(&out[..5], "Bad 1");
    assert_eq!(valve_display_name(b"", 0, &mut out), 7);
    assert_text(&out[..7], "Valve 1");
    // C++ valveDisplayName(nullptr, 11, ..): the empty name.
    assert_eq!(valve_display_name(b"", 11, &mut out), 8);
    assert_text(&out[..8], "Valve 12");
    assert_eq!(valve_display_name(b"\xc5\x81azienka", 3, &mut out), 9);
    let mut eight = [b'X'; 8];
    assert_eq!(valve_display_name(b"", 11, &mut eight), 0);
    let mut nine = [0u8; 9];
    assert_eq!(valve_display_name(b"", 11, &mut nine), 8);
    assert_eq!(valve_display_name(b"abcdefgh", 0, &mut nine), 8);
    assert_eq!(valve_display_name(b"abcdefghi", 0, &mut nine), 0);
    // C++ valveDisplayName("x", 0, nullptr, 4): no Rust form.
    assert_eq!(valve_display_name(b"x", 0, &mut out[..0]), 0);
}

#[test]
fn calibration_end_tracker() {
    let mut t = CalibEndTracker::default();
    let mut v = [ValveState::default(); N];
    let a = LocalTime {
        valid: true,
        year: 2026,
        epoch: 100,
        ..LocalTime::default()
    };
    let b = LocalTime { epoch: 200, ..a };
    v[3].calibrating = true;
    assert_eq!(t.observe(Some(&v), &a), 0); // the first observation only learns the state
    assert!(!t.ended(3));
    v[3].calibrating = false;
    v[5].calibrating = true;
    assert_eq!(t.observe(Some(&v), &a), 0x0008);
    assert!(t.ended(3));
    assert!(t.dirty(3));
    assert_eq!(t.end(3).epoch, 100);
    assert!(!t.ended(5));
    t.clear_dirty(3);
    assert!(!t.dirty(3));
    assert!(t.ended(3));
    v[5].calibrating = false;
    assert_eq!(t.observe(Some(&v), &b), 0x0020);
    assert_eq!(t.end(5).epoch, 200);
    assert_eq!(t.end(3).epoch, 100);
    assert!(t.dirty(5));
    assert_eq!(t.observe(Some(&v), &b), 0);
    // C++ observe(nullptr, b): None.
    assert_eq!(t.observe(None, &b), 0);
    assert!(!t.ended(12));
    assert!(!t.dirty(12));
    t.clear_dirty(12);
    assert!(t.dirty(5));
    // out of range reads entry 0
    assert_eq!(t.end(12).epoch, t.end(0).epoch);
    // A calibration that starts at the first observation and ends later.
    let mut u = CalibEndTracker::default();
    v[0].calibrating = true;
    assert_eq!(u.observe(Some(&v), &a), 0);
    v[0].calibrating = false;
    v[11].calibrating = true;
    assert_eq!(u.observe(Some(&v), &b), 0x0001);
    v[11].calibrating = false;
    assert_eq!(u.observe(Some(&v), &b), 0x0800);
    assert!(u.ended(11));
}
