// No C++ counterpart in glue_motor: the C++ covers the motor parameters and the escalation only
// through glue_system (smotc, scalx with the real motor.cpp), and the sim never puts the filtered
// current exactly on the undercurrent threshold (its integer filter stays at 0 for 19 dmA). These
// cases pin that behaviour of motor.cpp at the module.
use vdm_stm_core::calibration::EscalationConfig;
use vdm_stm_core::motor_params::MotorParams;
use vdm_stm_core::move_classifier::{DIR_CLOSE, DIR_OPEN};

use crate::board::BoardRev;

use super::bench::Bench;
use super::{Io, M_RES_NOCURRENT, M_RES_OPENS, M_RES_TURNING};

#[test]
fn motor_parameters_go_to_ram_and_to_the_eeprom_mirror() {
    let mut b = Bench::new(BoardRev::C2);
    let p = MotorParams {
        low_fac: 12,
        high_fac: 30,
        start_on_power: 40,
        min_counts: 1500,
        max_retries: 2,
    };
    b.m.motor_set_params(&mut b.env, &p);
    assert_eq!(b.m.motor_get_params(), p);
    assert_eq!(b.m.currentbound_low_fac, 12);
    assert_eq!(b.m.currentbound_high_fac, 30);
    assert_eq!(b.m.start_on_power, 40);
    assert_eq!(b.m.no_of_min_counts, 1500);
    assert_eq!(b.m.max_calib_retries, 2);
    let l = &b.env.eep.layout;
    assert_eq!(
        (
            l.currentbound_low_fac,
            l.currentbound_high_fac,
            l.start_on_power,
            l.no_of_min_counts,
            l.max_calib_retries
        ),
        (12, 30, 40, 1500, 2)
    );
}

#[test]
fn escalation_goes_to_ram_and_the_mirror_and_raises_the_bounds_of_repeated_strokes_only() {
    let mut b = Bench::new(BoardRev::C2);
    b.valve_setup();
    let c = EscalationConfig {
        enable: 1,
        step_pct: 25,
        max_ma: 50,
    };
    b.m.motor_set_escalation(&mut b.env, &c);
    assert_eq!(b.m.motor_get_escalation(), c);
    assert_eq!(b.env.eep.escalation, c);
    let io = Io {
        pulse: &b.pulse,
        flags: &b.flags,
        hw: &b.board,
    };
    // the first pass: 20 mA x 1.7
    b.m.prepare_learn_stroke(&io, 0, DIR_CLOSE, true);
    assert_eq!((b.m.move_bound_low, b.m.move_bound_high), (-340, 340));
    // repetition 1: + 25 % (below the cap of 50 mA)
    b.m.calib_retries = 1;
    b.m.prepare_learn_stroke(&io, 0, DIR_CLOSE, true);
    assert_eq!((b.m.move_bound_low, b.m.move_bound_high), (-425, 425));
    // the factors apply to their side
    b.m.currentbound_low_fac = 20;
    b.m.prepare_learn_stroke(&io, 0, DIR_OPEN, true);
    assert_eq!((b.m.move_bound_low, b.m.move_bound_high), (-500, 425));
    // a normal move is never escalated
    b.m.prepare_normal_move(&io, 0, DIR_OPEN, 10);
    assert_eq!((b.m.move_bound_low, b.m.move_bound_high), (-400, 340));
}

/// The motor state machine of valve 0 turning (opened, settled, started, soft start done),
/// without timer_handler0: the tests set the filtered current themselves.
fn turning() -> Bench {
    let mut b = Bench::new(BoardRev::C2);
    b.motorcycle(0, b'n');
    assert_eq!(b.motorcycle(0, b'o'), M_RES_OPENS);
    // M_OPEN, then 10 ticks to settle the relay, then M_START
    for _ in 0..12 {
        b.motorcycle(0, 0);
    }
    b.m.isr_timer_fin = true;
    // M_TURNON sees the soft start done
    assert_eq!(b.motorcycle(0, 0), M_RES_TURNING);
    b
}

/// Turning ticks until the result is not M_RES_TURNING (at most `max`): (ticks, result).
fn run_turning(b: &mut Bench, current: i32, max: u32) -> (u32, u8) {
    for tick in 1..=max {
        b.m.current_ma = current;
        let r = b.motorcycle(0, 0);
        if r != M_RES_TURNING {
            return (tick, r);
        }
    }
    (max, M_RES_TURNING)
}

#[test]
fn undercurrent_exactly_20_in_either_direction_is_a_turning_motor() {
    for current in [20, -20] {
        let mut b = turning();
        assert_eq!(run_turning(&mut b, current, 400), (400, M_RES_TURNING));
    }
}

#[test]
fn undercurrent_below_20_ends_the_move_after_3_debounce_and_201_low_ticks() {
    for current in [19, -19, 0] {
        let mut b = turning();
        // ticks 4..204 count, the 204th sets M_UNDERCURR, the 205th stops the motor
        assert_eq!(
            run_turning(&mut b, current, 400),
            (205, M_RES_NOCURRENT),
            "{current}"
        );
    }
}
