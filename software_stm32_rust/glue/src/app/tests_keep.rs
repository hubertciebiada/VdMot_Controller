// skeep (2.2.8): a service hold that does not run out, and everything that ends it.
use vdm_stm_core::valve_codes::{ST_IDLE, VLV_FLAG_SVC_HOLD, VLV_FLAG_SVC_KEEP};

use super::stub_env::AppBench;
use super::SVMOV_HOLD_10S;

/// app_setup, then every valve idle and calibrated
fn begin() -> AppBench {
    let mut b = AppBench::new();
    b.app_setup();
    for mot in &mut b.m.mots {
        mot.status = ST_IDLE;
        mot.calibrated = true;
        mot.connected = true;
        mot.scaler = 36;
    }
    b.env.log.clear();
    b.env.take_tx();
    b
}

/// valve 4 in a service hold that is kept
fn kept() -> AppBench {
    let mut b = begin();
    b.m.valves[4].svc_hold = SVMOV_HOLD_10S;
    assert_eq!(b.app_keep(4, true), 0);
    b
}

#[test]
fn a_kept_hold_does_not_run_out_and_runs_out_again_after_skeep_0() {
    let mut b = kept();
    b.m.valves[5].svc_hold = SVMOV_HOLD_10S;
    for _ in 0..100 {
        b.app_10s_loop(10);
    }
    assert_eq!(b.m.valves[4].svc_hold, SVMOV_HOLD_10S);
    // the hold of another valve runs out as before
    assert_eq!(b.m.valves[5].svc_hold, 0);
    assert_eq!(b.app_keep(4, false), 0);
    assert!(!b.m.valves[4].svc_keep);
    b.app_10s_loop(10);
    assert_eq!(b.m.valves[4].svc_hold, SVMOV_HOLD_10S - 1);
}

#[test]
fn skeep_255_keeps_every_valve_an_invalid_valve_is_refused() {
    let mut b = begin();
    assert_eq!(b.app_keep(255, true), 0);
    for v in 0..12 {
        assert!(b.m.valves[v].svc_keep, "valve {v}");
    }
    assert_eq!(b.app_keep(255, false), 0);
    for v in 0..12 {
        assert!(!b.m.valves[v].svc_keep, "valve {v}");
    }
    assert_eq!(b.app_keep(12, true), -1);
    assert_eq!(b.app_keep(254, true), -1);
    assert!(b.m.valves.iter().all(|v| !v.svc_keep));
}

#[test]
fn gvlvy_reports_the_keep_next_to_the_hold() {
    let mut b = kept();
    assert_eq!(b.v3(4).flags, VLV_FLAG_SVC_HOLD + VLV_FLAG_SVC_KEEP);
    assert_eq!(b.v3(5).flags & VLV_FLAG_SVC_KEEP, 0);
}

#[test]
fn a_target_request_ends_the_keep_with_the_hold() {
    let mut b = kept();
    b.app_target_changed(4);
    assert_eq!(b.m.valves[4].svc_hold, 0);
    assert!(!b.m.valves[4].svc_keep);
}

#[test]
fn staln_staop_and_stdet_end_the_keep_with_the_hold() {
    let mut b = kept();
    assert_eq!(b.app_set_valvelearning(4), 0);
    assert!(!b.m.valves[4].svc_keep);
    let mut b = kept();
    assert_eq!(b.app_set_valveopen(4), 0);
    assert!(!b.m.valves[4].svc_keep);
    let mut b = kept();
    b.app_scan_valves();
    assert!(!b.m.valves[4].svc_keep);
}

#[test]
fn an_expired_lease_ends_the_keep_and_the_hold_runs_out() {
    let mut b = kept();
    b.app.app_lease_configure(5);
    b.app_1s_tick(300);
    assert_eq!(b.app.app_lease_state(), 2);
    b.app_10s_loop(10);
    assert!(!b.m.valves[4].svc_keep);
    assert_eq!(b.m.valves[4].svc_hold, SVMOV_HOLD_10S - 1);
}

#[test]
fn a_stop_restarts_a_kept_hold_and_keeps_it() {
    let mut b = kept();
    b.m.valves[4].svc_hold = 3;
    assert_eq!(b.app_stop(4), 0);
    assert_eq!(b.m.valves[4].svc_hold, SVMOV_HOLD_10S);
    assert!(b.m.valves[4].svc_keep);
}
