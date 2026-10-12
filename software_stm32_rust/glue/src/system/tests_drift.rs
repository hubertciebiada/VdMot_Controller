// New case (no C++ counterpart): small opening steps of a valve that turns on after its enable
// went off. The valve reaches its open end stop below a counted 100 %; the step that meets the
// end stop takes it as the open end (move_classifier::open_end_reached), the scheduler moves the
// valve back down to its target, nothing counts as early and no calibration starts.

use vdm_stm_core::move_classifier::StopReason;
use vdm_stm_core::valve_codes::{ST_PRESENT, VLV_FLAG_EARLY_PENDING};

use super::bench::{
    sim, Boot, Case, K_COAST_CLOSE, K_COAST_OPEN, K_FLAGS, K_LAST_CNT, K_LAST_REQ, K_LAST_STOP,
};

/// gvlvx earlyStops
const K_EARLY_STOPS: usize = 12;

/// A target, until valve 0 idles there; no early stop on the way.
fn step(b: &mut Boot<'_>, pct: u8) {
    assert_eq!(b.exchange(&std::format!("stgtp 0 {pct}\n")), "stgtp\r\n");
    assert!(b.run_main_until(|b| b.idle_at(0, pct), 60000), "{pct}");
    assert_ne!(
        b.gvlvx(0, K_LAST_STOP),
        StopReason::EarlyEndStop as i64,
        "{pct}"
    );
    assert_eq!(b.gvlvx(0, K_EARLY_STOPS), 0, "{pct}");
    assert_eq!(b.gvlvy(0, K_FLAGS) & i64::from(VLV_FLAG_EARLY_PENDING), 0);
}

#[test]
fn w9_opening_steps_that_overshoot_end_at_the_open_end_and_move_back_to_the_target() {
    let mut case = Case::new("", "a case without a golden");
    let mut b = case.boot(sim(0x0FFF & !1), |_| {});
    b.run_main(40000);
    assert_eq!(b.m().mots[0].status, ST_PRESENT);
    b.calibrated_idle(1);
    {
        let mut m = b.m_mut();
        m.mots[0].opening_count = 3600;
        m.mots[0].closing_count = 3600;
        // the valve stands where it is counted: the drift comes from the coast alone
        let at = m.mots[0].actual_position;
        b.sim().valve[0].position = i32::from(at) * 36;
    }
    step(&mut b, 50);
    let calib_seq = b.m().mots[0].calib_seq;
    // each move turns 40 pulses more than counted
    b.sim().valve[0].coast_pulses = 40;
    let mut target = 50;
    let mut resync = None;
    while target < 100 {
        target += 4;
        let enables = b.sim().valve[0].enables;
        step(&mut b, target);
        // up to the open end stop and back down: two moves for one step
        if b.sim().valve[0].enables == enables + 2 {
            resync = Some(target);
            break;
        }
        assert_eq!(b.sim().valve[0].enables, enables + 1, "{target}");
    }
    let at = resync.expect("the open end stop ends the drift");
    assert!(at < 100, "{at}");
    // from the open end stop the valve stands where it is counted, less the coast of the move down
    let real = b.sim().valve[0].position;
    assert!((real - (i32::from(at) * 36 - 41)).abs() <= 2, "{at} {real}");
    // a step down and up again stay partial moves
    step(&mut b, at - 4);
    step(&mut b, at);
    step(&mut b, 100);
    assert_eq!(b.gvlvx(0, K_LAST_STOP), StopReason::EndStop as i64);
    assert_eq!(b.sim().valve[0].position, 3600);
    assert_eq!(b.m().mots[0].calib_seq, calib_seq);
    assert!(!b.m().mots[0].early_learn_due);
    b.finish();
}

// New case: the same valve calibrated by staln. Its coast probe learns the closing coast of 40
// pulses, gvlvy reports it (the opening coast is not learned), every closing step from the open
// end stop stops 40 pulses earlier and the valve stays where it is counted: no move reaches an
// end stop, no early stop, no calibration.
#[test]
fn w9_with_the_coast_learned_by_the_calibration_closing_steps_stay_where_they_are_counted() {
    let mut case = Case::new("", "a case without a golden");
    let mut b = case.boot(sim(0x0FFF & !1), |_| {});
    b.run_main(40000);
    assert_eq!(b.m().mots[0].status, ST_PRESENT);
    b.sim().valve[0].coast_pulses = 40;
    assert_eq!(b.exchange("staln 0\n"), "staln\r\n");
    assert!(b.run_main_until(|b| b.m().mots[0].calib_active, 10000));
    assert!(b.run_main_until(|b| !b.m().mots[0].calib_active && b.idle(), 120000));
    assert!(b.m().mots[0].calibrated);
    assert_eq!(b.gvlvy(0, K_COAST_OPEN), 0);
    assert_eq!(b.gvlvy(0, K_COAST_CLOSE), 40);
    assert_eq!(b.m().mots[0].scaler, 36);
    step(&mut b, 100);
    assert_eq!(b.sim().valve[0].position, 3600);
    let calib_seq = b.m().mots[0].calib_seq;
    let mut target = 100;
    while target > 52 {
        target -= 4;
        let enables = b.sim().valve[0].enables;
        step(&mut b, target);
        assert_eq!(b.sim().valve[0].enables, enables + 1, "{target}");
        // gvlvx lastReq: the pulses of the step, lastCnt: the pulses counted up to the stop
        assert_eq!(b.gvlvx(0, K_LAST_REQ), 144);
        assert_eq!(b.gvlvx(0, K_LAST_CNT), 105);
    }
    b.run_main(1000);
    // the pulse that stops the motor, once per move
    let real = b.sim().valve[0].position;
    assert!((-14..=0).contains(&(real - 52 * 36)), "{real}");
    assert_eq!(b.m().mots[0].calib_seq, calib_seq);
    assert!(!b.m().mots[0].early_learn_due);
    b.finish();
}
