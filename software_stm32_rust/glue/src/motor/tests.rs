// Port of software_stm32/test/native/glue/test_motor.cpp: smoke tests of motor.cpp with the
// valve sim (glue_motor, and glue_motor_c1 with the C1 MUX wiring: every case that drives the
// motor runs for both board revisions): safe pins, setup, a full calibration, a partial move, an
// open circuit, the presence test.
use core::sync::atomic::Ordering;
use std::vec;

use vdm_stm_core::calibration::MEAN_CURRENT_DEFAULT_MA;
use vdm_stm_core::valve_codes::{ST_IDLE, ST_OPENING, ST_OPEN_CIRCUIT, ST_PRESENT};

use crate::board::BoardRev;
use crate::hal::Out;
use crate::test_support::fake_board::{Ev, FakeBoard};

use super::bench::{start_valves, states, status_change, Bench, REVS};
use super::{valve_pins_safe, AState, CalibState, CMD_A_LEARN, CMD_A_OPEN, CMD_A_TEST};

#[test]
fn valve_pins_safe_presets_the_psu_latch_off_and_the_enables_low() {
    let board = FakeBoard::new();
    valve_pins_safe(&board);
    // C++ also checks the pin modes between these writes (PSU open drain, ENA0..5 outputs) and
    // the port clock of PB9: firmware set-up in Rust (design §1.3)
    assert_eq!(
        board.events_of(|_| true),
        vec![
            Ev::Write(Out::PsuEna, true),
            Ev::Write(Out::PsuEna, true),
            Ev::Write(Out::Ena0, false),
            Ev::Write(Out::Ena1, false),
            Ev::Write(Out::Ena2, false),
            Ev::Write(Out::Ena3, false),
            Ev::Write(Out::Ena4, false),
            Ev::Write(Out::Ena5, false),
        ]
    );
}

#[test]
fn valve_setup_sets_the_start_values_of_all_valves_and_the_1_ms_current_timer() {
    for rev in REVS {
        let mut b = Bench::new(rev);
        b.m.start_on_power = 30;
        assert_eq!(b.valve_setup(), 0);
        // C++: the modes of MUX, DIR, ENA5 (outputs) and REVIN (input): firmware set-up
        for v in 0..12 {
            let mot = &b.m.mots[v];
            assert_eq!(mot.actual_position, 30, "valve {v}");
            assert_eq!(mot.target_position, 30, "valve {v}");
            assert_eq!(
                mot.meancurrent,
                u32::from(MEAN_CURRENT_DEFAULT_MA),
                "valve {v}"
            );
            assert_eq!(mot.scaler, 89, "valve {v}");
            assert_eq!(mot.calib_retries, 0, "valve {v}");
            assert!(!mot.calib_active, "valve {v}");
        }
        assert_eq!(b.state(), AState::Init);
        // C++ ITimer0.callback == TimerHandler0: the firmware's TIM1 vector runs timer_handler0
        assert_eq!(b.tim1.interval_us, 1000);
        assert!(b.tim1.running());
    }
}

fn calibration_learns_the_stroke(rev: BoardRev) {
    let mut b = start_valves(rev);
    b.rig.valve[0].pulses_per_ms = 1.0;
    assert_eq!(b.appsetaction(CMD_A_LEARN, 0, 0), 0);
    b.run_ms(10);
    assert!(b.m.mots[0].calib_active);
    assert!(b.run_until(
        |b| !b.m.mots[0].calib_active && b.state() == AState::Idle,
        30000
    ));
    // Learn1 .. Learn4, Set, Set1, Set2, Idle
    assert_eq!(states(&b.rig), "5 6 7 8 9 10 11 1");
    let mot = b.m.mots[0];
    assert_eq!(mot.status, ST_IDLE);
    assert!((3598..=3602).contains(&mot.opening_count));
    assert!((3598..=3602).contains(&mot.closing_count));
    assert_eq!(
        mot.deadzone_count,
        mot.closing_count as i32 - mot.opening_count as i32
    );
    assert_eq!(mot.scaler, mot.opening_count / 100);
    assert_eq!(mot.actual_position, 50);
    assert!(!mot.calibration);
    assert_eq!(mot.calib_state, CalibState::Idle);
    // back at 50 %: 50 x scaler counts from the closed end stop, plus the pulse that stops the motor
    assert_eq!(b.rig.valve[0].position, (50 * mot.scaler + 1) as i32);
    assert_eq!(b.rig.conflicts, 0);
}

#[test]
fn calibration_three_strokes_learn_the_stroke_then_the_valve_goes_back_to_its_target() {
    for rev in REVS {
        calibration_learns_the_stroke(rev);
    }
}

#[test]
fn partial_move_opening_by_change_stops_on_pulse_scaler_x_change_plus_1() {
    for rev in REVS {
        let mut b = start_valves(rev);
        b.m.mots[0].scaler = 36;
        b.m.mots[0].status = ST_IDLE;
        let start = b.rig.valve[0].position;
        assert_eq!(b.appsetaction(CMD_A_OPEN, 0, 10), 0);
        b.run_ms(10);
        assert!(b.run_until(Bench::idle, 10000));
        assert_eq!(b.rig.valve[0].position - start, 36 * 10 + 1);
        assert_eq!(b.m.mots[0].actual_position, 60);
        assert_eq!(b.m.mots[0].status, ST_IDLE);
        assert_eq!(b.m.valves[0].movements, 1);
    }
}

#[test]
fn open_circuit_on_the_tick_after_the_undercurrent_counter_exceeds_its_timeout() {
    for rev in REVS {
        let mut b = start_valves(rev);
        b.rig.valve[1].connected = false;
        b.m.mots[1].status = ST_IDLE;
        b.m.mots[1].target_position = 60;
        assert_eq!(b.appsetaction(CMD_A_OPEN, 1, 10), 0);
        assert!(b.run_until(Bench::idle, 10000));
        let opening = status_change(&b.rig, 1, ST_OPENING).expect("opening");
        let opencir = status_change(&b.rig, 1, ST_OPEN_CIRCUIT).expect("open circuit");
        // valve_loop ticks (10 ms) after the start of the move: M_OPEN 1, relay settle 10,
        // M_START 1, M_TURNON 2 (timer_handler0 enables the motor in between), debounce 3,
        // undercurrent 201 (> 200), M_UNDERCURR 1
        assert_eq!(opencir.ms - opening.ms, 10 * (1 + 10 + 1 + 2 + 3 + 201 + 1));
        // the target: the position is unknown
        assert_eq!(b.m.mots[1].actual_position, 60);
        assert!(!b.m.mots[1].connected);
        assert_eq!(b.rig.valve[1].pulses, 0);
    }
}

#[test]
fn presence_test_a_connected_valve_is_present_a_disconnected_one_open_circuit() {
    for rev in REVS {
        let mut b = start_valves(rev);
        b.rig.valve[3].connected = false;
        assert_eq!(b.appsetaction(CMD_A_TEST, 2, 0), 0);
        b.run_ms(10);
        assert!(b.run_until(Bench::idle, 10000));
        assert_eq!(b.appsetaction(CMD_A_TEST, 3, 0), 0);
        b.run_ms(10);
        assert!(b.run_until(Bench::idle, 10000));
        assert_eq!(b.m.mots[2].status, ST_PRESENT);
        assert_eq!(b.m.mots[3].status, ST_OPEN_CIRCUIT);
        assert!(!b.m.mots[3].connected);
        assert_eq!(b.rig.valve[2].enables, 1);
    }
}

// Port of test_fakes.cpp "sim: runs valve_loop() every 10 ms while no timer does" (the bench runs
// the sim and the interrupts of the motor module).
#[test]
fn sim_runs_valve_loop_every_10_ms_while_no_timer_does() {
    let mut b = Bench::new(BoardRev::C2);
    b.install();
    b.run_ms(30);
    assert_eq!(b.flags.valve_loop_ticks.load(Ordering::SeqCst), 3);
    let now = b.board.now_us();
    assert!(b.tim2.attach_at(now, 10000));
    b.run_ms(30);
    // the timer's calls only
    assert_eq!(b.flags.valve_loop_ticks.load(Ordering::SeqCst), 6);
}

#[test]
fn appsetaction_is_taken_only_while_idle_without_a_pending_command_valve_11_yes_12_no() {
    for rev in REVS {
        let mut b = start_valves(rev);
        assert_eq!(b.appsetaction(CMD_A_TEST, 12, 0), -1);
        assert_eq!(b.appsetaction(CMD_A_TEST, 11, 0), 0);
        assert!(!b.idle());
        assert_eq!(b.appsetaction(CMD_A_TEST, 10, 0), -1);
        assert_eq!(b.appsetaction_with(CMD_A_TEST, 10, 0, true, 0), 0);
        // C++ irqDisables >= 4 and PRIMASK 0 afterwards: no Rust form, `&mut MotorShared` exists
        // in the main loop only under the lock of the firmware's IsrCell (design §2.3)
    }
}
