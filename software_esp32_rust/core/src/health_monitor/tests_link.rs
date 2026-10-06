//! Port of test/native/test_health_monitor__link.cpp: HealthMonitor events of the 2.1 link:
//! protocol 3 arguments, stroke warning, assembly targets and the gstax system events.

use super::*;
use crate::stm_codec::STM_FLAG_FS_LEASE;
use std::vec::Vec;

fn known(status: u8) -> ValveState {
    ValveState {
        known: true,
        status,
        ..ValveState::default()
    }
}

/// Events of `on_valve` for valve 3, active.
fn on_valve(hm: &HealthMonitor, a: &ValveState, b: &ValveState) -> Vec<Event> {
    let mut e: [Event; MAX_EVENTS_PER_UPDATE] = Default::default();
    let n = hm.on_valve(3, a, b, true, &mut e);
    e[..n].to_vec()
}

fn on_status(
    hm: &mut HealthMonitor,
    before: Option<&StmStatus>,
    after: &StmStatus,
    now: u32,
) -> Vec<Event> {
    let mut e: [Event; MAX_EVENTS_PER_UPDATE] = Default::default();
    let n = hm.on_stm_status(before, after, now, &mut e);
    e[..n].to_vec()
}

#[track_caller]
fn check_event(e: &Event, code: EventCode, valve: u8, a1: i32, a2: i32) {
    assert_eq!(e.code, code);
    assert_eq!(e.severity, event_default_severity(code));
    assert_eq!(e.valve, valve);
    assert_eq!(e.arg1, a1);
    assert_eq!(e.arg2, a2);
}

fn v3(status: u8, flags: u16, fs_pct: u8, fault: u8) -> ValveState {
    ValveState {
        has_v3: true,
        stm_flags: flags,
        fs_pct,
        fault,
        calib_retries: 2,
        ..known(status)
    }
}

fn v3_status() -> StmStatus {
    StmStatus {
        v3: true,
        uptime_s: 100,
        ..StmStatus::default()
    }
}

#[test]
fn on_valve_valve_blocked_names_the_failsafe_position_of_a_blocked_protocol_3_valve() {
    let hm = HealthMonitor::default();
    let r = on_valve(&hm, &known(1), &v3(9, STM_FLAG_FS_BLOCKED, 50, 0));
    assert_eq!(r.len(), 1);
    check_event(&r[0], EventCode::ValveBlocked, 3, 2, 50);
    let r = on_valve(&hm, &known(1), &v3(9, STM_FLAG_FS_BLOCKED, 0, 0));
    check_event(&r[0], EventCode::ValveBlocked, 3, 2, 0);
    // hold: stays at 0 %
    let r = on_valve(
        &hm,
        &known(1),
        &v3(9, STM_FLAG_FS_BLOCKED, FAILSAFE_HOLD, 0),
    );
    check_event(&r[0], EventCode::ValveBlocked, 3, 2, -1);
    let r = on_valve(&hm, &known(1), &v3(9, STM_FLAG_FS_LEASE, 50, 0)); // not the blocked flag
    check_event(&r[0], EventCode::ValveBlocked, 3, 2, -1);
    let mut no_v3 = v3(9, STM_FLAG_FS_BLOCKED, 50, 0);
    no_v3.has_v3 = false;
    let r = on_valve(&hm, &known(1), &no_v3);
    check_event(&r[0], EventCode::ValveBlocked, 3, 2, -1);
    // first snapshot
    let r = on_valve(
        &hm,
        &ValveState::default(),
        &v3(9, STM_FLAG_FS_BLOCKED, 40, 0),
    );
    check_event(&r[0], EventCode::ValveBlocked, 3, 2, 40);
}

#[test]
fn on_valve_valve_failed_carries_the_protocol_3_fault_minus_1_without() {
    let hm = HealthMonitor::default();
    let r = on_valve(&hm, &known(1), &v3(4, 0, 50, 3));
    assert_eq!(r.len(), 1);
    check_event(&r[0], EventCode::ValveFailed, 3, 2, 3);
    let r = on_valve(&hm, &known(1), &v3(4, 0, 50, 0));
    check_event(&r[0], EventCode::ValveFailed, 3, 2, 0);
    let mut no_v3 = v3(4, 0, 50, 3);
    no_v3.has_v3 = false;
    let r = on_valve(&hm, &known(1), &no_v3);
    check_event(&r[0], EventCode::ValveFailed, 3, 2, -1);
}

#[test]
fn on_valve_calib_failed_carries_the_failsafe_position_like_valve_blocked() {
    let hm = HealthMonitor::default();
    let mut a = v3(1, 0, 50, 0);
    a.calibrating = true;
    let b = v3(9, STM_FLAG_FS_BLOCKED, 60, 0);
    let r = on_valve(&hm, &a, &b);
    assert_eq!(r.len(), 1);
    check_event(&r[0], EventCode::CalibFailed, 3, 2, 60);
}

#[test]
fn on_valve_calib_started_arg1_2_for_an_automatic_retry() {
    let hm = HealthMonitor::default();
    let a = known(1);
    let mut b = known(1);
    b.calibrating = true;
    b.auto_retry = true;
    let r = on_valve(&hm, &a, &b);
    assert_eq!(r.len(), 1);
    check_event(&r[0], EventCode::CalibStarted, 3, 2, 0);
    b.auto_retry = false;
    let r = on_valve(&hm, &a, &b);
    check_event(&r[0], EventCode::CalibStarted, 3, 0, 0);
}

#[test]
fn on_valve_calib_stroke_short_once_when_the_flag_rises() {
    let mut hm = HealthMonitor::default();
    hm.set_min_counts(3000);
    let mut a = known(1);
    a.open_count = 3599;
    a.close_count = 5000;
    let mut b = a;
    b.health = HEALTH_STROKE_SHORT;
    let r = on_valve(&hm, &a, &b);
    assert_eq!(r.len(), 1);
    check_event(&r[0], EventCode::CalibStrokeShort, 3, 3599, 3000);
    assert!(on_valve(&hm, &b, &b).is_empty());
    b.open_count = 6000;
    b.close_count = 3100;
    let r = on_valve(&hm, &a, &b);
    check_event(&r[0], EventCode::CalibStrokeShort, 3, 3100, 3000);
    assert!(on_valve(&hm, &b, &a).is_empty()); // falling: silent
}

#[test]
fn on_valve_target_set_for_an_assembly_none_for_restored_or_adopted_targets() {
    let hm = HealthMonitor::default();
    let mut a = known(1);
    a.desired_valid = true;
    a.desired = 30;
    a.source = TargetSource::Web;
    let mut b = a;
    b.desired = 100;
    b.source = TargetSource::Assembly;
    let r = on_valve(&hm, &a, &b);
    assert_eq!(r.len(), 1);
    check_event(&r[0], EventCode::TargetSet, 3, 100, 5);
    b.source = TargetSource::Restored;
    assert!(on_valve(&hm, &a, &b).is_empty());
    b.source = TargetSource::Stm;
    assert!(on_valve(&hm, &a, &b).is_empty());
}

// ================================================================ gstax

#[test]
fn on_stm_status_nothing_without_protocol_3_data() {
    let mut hm = HealthMonitor::default();
    let mut s = v3_status();
    s.v3 = false;
    s.safe_mode = true;
    s.cfg_events = 1;
    s.sys_flags = STM_SYS_PROTECT_SUSPENDED;
    assert!(on_status(&mut hm, None, &s, 0).is_empty());
    let mut out: [Event; 1] = Default::default();
    let mut t = v3_status();
    t.safe_mode = true;
    assert_eq!(hm.on_stm_status(None, &t, 0, &mut []), 0); // C++ out nullptr
    assert_eq!(hm.on_stm_status(None, &t, 0, &mut out[..0]), 0);
}

#[test]
fn on_stm_status_safe_mode_rise_and_fall_safe_mode_on_the_first_status_reports() {
    let mut hm = HealthMonitor::default();
    let a = v3_status();
    let mut b = a;
    b.safe_mode = true;
    b.wdg_resets = 3;
    let r = on_status(&mut hm, Some(&a), &b, 0);
    assert_eq!(r.len(), 1);
    check_event(&r[0], EventCode::StmSafeMode, NO_VALVE, 3, 0);
    assert!(on_status(&mut hm, Some(&b), &b, 0).is_empty());
    let r = on_status(&mut hm, Some(&b), &a, 0);
    assert_eq!(r.len(), 1);
    check_event(&r[0], EventCode::StmSafeModeEnded, NO_VALVE, 0, 0);
    let r = on_status(&mut hm, None, &b, 0);
    assert_eq!(r.len(), 1);
    check_event(&r[0], EventCode::StmSafeMode, NO_VALVE, 3, 0);
    let mut old = a;
    old.v3 = false;
    old.safe_mode = true;
    let r = on_status(&mut hm, Some(&old), &b, 0); // a status without protocol 3 data counts as none
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].code, EventCode::StmSafeMode);
    assert!(on_status(&mut hm, None, &a, 0).is_empty());
}

#[test]
fn on_stm_status_config_repairs_on_the_first_status_of_a_young_stm_and_on_increases() {
    let mut hm = HealthMonitor::default();
    let mut s = v3_status();
    s.cfg_events = 1;
    s.cfg_flags = 0x24;
    s.uptime_s = 599;
    let r = on_status(&mut hm, None, &s, 0);
    assert_eq!(r.len(), 1);
    check_event(&r[0], EventCode::StmConfigRepaired, NO_VALVE, 0x24, 1);
    s.uptime_s = 600;
    assert!(on_status(&mut hm, None, &s, 0).is_empty()); // an ESP restart does not report an old repair
    let mut zero = s;
    zero.cfg_events = 0;
    zero.uptime_s = 1;
    assert!(on_status(&mut hm, None, &zero, 0).is_empty());
    assert!(on_status(&mut hm, Some(&s), &s, 0).is_empty()); // 1 -> 1
    let mut t = s;
    t.cfg_events = 2;
    let r = on_status(&mut hm, Some(&s), &t, 0);
    assert_eq!(r.len(), 1);
    check_event(&r[0], EventCode::StmConfigRepaired, NO_VALVE, 0x24, 2);
    assert!(on_status(&mut hm, Some(&t), &s, 0).is_empty()); // lower: nothing
}

#[test]
fn on_stm_status_uart_error_totals_baseline_first_one_event_per_10_min() {
    let mut hm = HealthMonitor::default();
    let mut a = v3_status();
    a.uart_ore = 1;
    a.uart_fe = 2;
    a.uart_ne = 3;
    a.rx_dropped = 4;
    assert!(on_status(&mut hm, None, &a, 1000).is_empty()); // baseline
    let mut b = a;
    b.uart_fe = 3;
    let r = on_status(&mut hm, Some(&a), &b, 2000);
    assert_eq!(r.len(), 1);
    check_event(&r[0], EventCode::StmUartErrors, NO_VALVE, 7, 4);
    let mut c = b;
    c.rx_dropped = 10;
    assert!(on_status(&mut hm, Some(&b), &c, 2000 + 599_999).is_empty());
    c.uart_ore = 5;
    let r = on_status(&mut hm, Some(&b), &c, 2000 + 600_000);
    assert_eq!(r.len(), 1);
    check_event(&r[0], EventCode::StmUartErrors, NO_VALVE, 11, 10);
    // A lower total (the STM restarted) re-baselines silently.
    let mut d = a;
    d.uart_ore = 0;
    assert!(on_status(&mut hm, Some(&c), &d, 3_000_000).is_empty());
    let mut e = d;
    e.uart_ne = 4;
    let r = on_status(&mut hm, Some(&d), &e, 3_300_000);
    assert_eq!(r.len(), 1);
    check_event(&r[0], EventCode::StmUartErrors, NO_VALVE, 6, 4);
    // The first status after a reboot is a new baseline.
    let mut f = e;
    f.uart_ore = 50;
    assert!(on_status(&mut hm, None, &f, 4_000_000).is_empty());
    f.rx_dropped = 5;
    let r = on_status(&mut hm, Some(&f), &f, 4_700_000);
    assert_eq!(r.len(), 1);
    check_event(&r[0], EventCode::StmUartErrors, NO_VALVE, 56, 5);
}

#[test]
fn on_stm_status_protection_suspended_once_per_stm_boot() {
    let mut hm = HealthMonitor::default();
    let a = v3_status();
    let mut b = a;
    b.sys_flags = STM_SYS_PROTECT_SUSPENDED;
    let r = on_status(&mut hm, Some(&a), &b, 0);
    assert_eq!(r.len(), 1);
    check_event(&r[0], EventCode::StmProtectionSuspended, NO_VALVE, 0, 0);
    assert!(on_status(&mut hm, Some(&b), &b, 0).is_empty());
    let r = on_status(&mut hm, None, &b, 0);
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].code, EventCode::StmProtectionSuspended);
    let mut other = a;
    other.sys_flags = 0x02; // reserved bit only
    assert!(on_status(&mut hm, Some(&a), &other, 0).is_empty());
}
