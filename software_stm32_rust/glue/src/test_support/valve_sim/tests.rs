// Port of the valve sim cases of software_stm32/test/native/glue/test_fakes.cpp (glue_fakes). The
// case "sim: runs valve_loop() every 10 ms while no timer does" needs the motor state: it is in
// motor/tests.rs.
use std::cell::Cell;

use crate::board::BoardRev;
use crate::hal::{Out, Pins, RevIrq};
use crate::test_support::fake_board::FakeBoard;

use super::Rig;

const ENA: [Out; 6] = [
    Out::Ena0,
    Out::Ena1,
    Out::Ena2,
    Out::Ena3,
    Out::Ena4,
    Out::Ena5,
];

/// The sim alone for ms milliseconds (the EXTI handler counts the pulses).
fn run(board: &FakeBoard, rig: &mut Rig, ms: u32, exti_calls: &Cell<u32>) {
    for _ in 0..ms {
        board.advance_us_with(1000, &mut || {
            rig.on_ms(board, &mut || {
                board.fire_exti(|| exti_calls.set(exti_calls.get() + 1));
            });
        });
    }
}

fn mux_on(board: &FakeBoard, rev: BoardRev) {
    board.set(Out::Mux, rev.mux_on_high());
}

fn mux_off(board: &FakeBoard, rev: BoardRev) {
    board.set(Out::Mux, !rev.mux_on_high());
}

#[test]
fn sim_the_enable_pins_and_the_mux_select_each_of_the_12_valves() {
    for rev in [BoardRev::C2, BoardRev::C1] {
        let board = FakeBoard::new();
        let mut rig = Rig::new(rev);
        rig.install(0, [0; 12]);
        assert_eq!(rig.enabled_valve(&board), -1);
        for v in 0..12 {
            for pin in ENA {
                board.set(pin, false);
            }
            if v % 2 == 1 {
                mux_off(&board, rev);
            } else {
                mux_on(&board, rev);
            }
            board.set(ENA[v / 2], true);
            assert_eq!(rig.enabled_valve(&board), v as i32, "{rev:?}");
        }
    }
}

#[test]
fn sim_a_powered_enabled_valve_turns_one_pulse_per_1_over_pulses_per_ms_ms() {
    let rev = BoardRev::C2;
    let board = FakeBoard::new();
    let mut rig = Rig::new(rev);
    rig.install(0, [0; 12]);
    let exti_calls = Cell::new(0);
    board.attach();
    // PSU on
    board.set(Out::PsuEna, false);
    // odd valve: 3
    mux_off(&board, rev);
    // opening
    board.set(Out::Dir, false);
    board.set(Out::Ena1, true);
    run(&board, &mut rig, 100, &exti_calls);
    assert_eq!(rig.valve[3].position, 1820);
    assert_eq!(rig.valve[3].pulses, 20);
    assert_eq!(exti_calls.get(), 20);
    assert_eq!(rig.valve[3].enables, 1);
    assert_eq!(rig.current_dma(), 250);
    // 250 dmA -> 181 ADC counts above the reference -> 249 dmA in timer_handler0's conversion
    assert_eq!(rig.adc_counts() - 2048, 181);
    // closing: the sign of the current follows the direction
    board.set(Out::Dir, true);
    run(&board, &mut rig, 10, &exti_calls);
    assert_eq!(rig.valve[3].position, 1818);
    assert_eq!(rig.current_dma(), -250);
    // PSU off
    board.set(Out::PsuEna, true);
    run(&board, &mut rig, 10, &exti_calls);
    assert_eq!(rig.valve[3].position, 1818);
    assert_eq!(rig.current_dma(), 0);
}

#[test]
fn sim_end_stop_obstacle_open_circuit_short_inrush_and_coast() {
    let rev = BoardRev::C2;
    let board = FakeBoard::new();
    let mut rig = Rig::new(rev);
    rig.install(0, [0; 12]);
    let pulses = Cell::new(0);
    board.set(Out::PsuEna, false);
    // valve 0
    mux_on(&board, rev);
    board.set(Out::Dir, false);
    rig.valve[0].position = 3598;
    rig.valve[0].inrush_peak_dma = 900;
    rig.valve[0].inrush_ms = 2;
    board.set(Out::Ena0, true);
    run(&board, &mut rig, 2, &pulses);
    assert_eq!(rig.current_dma(), 900);
    run(&board, &mut rig, 20, &pulses);
    // stops at the end stop
    assert_eq!(rig.valve[0].position, 3600);
    assert_eq!(rig.current_dma(), 700);
    board.set(Out::Ena0, false);
    rig.valve[0].position = 100;
    rig.valve[0].jam_from = 105;
    rig.valve[0].jam_to = 110;
    rig.valve[0].coast_pulses = 3;
    board.set(Out::Ena0, true);
    run(&board, &mut rig, 40, &pulses);
    assert_eq!(rig.valve[0].position, 105);
    assert_eq!(rig.current_dma(), 700);
    board.set(Out::Dir, true);
    rig.valve[0].position = 120;
    run(&board, &mut rig, 50, &pulses);
    assert_eq!(rig.valve[0].position, 110);
    board.set(Out::Dir, false);
    rig.valve[0].jam_from = -1;
    rig.valve[0].position = 200;
    run(&board, &mut rig, 5, &pulses);
    assert_eq!(rig.valve[0].position, 201);
    board.set(Out::Ena0, false);
    run(&board, &mut rig, 20, &pulses);
    // 3 coast pulses after the enable went off
    assert_eq!(rig.valve[0].position, 204);
    assert_eq!(rig.current_dma(), 0);
    rig.valve[0].connected = false;
    board.set(Out::Ena0, true);
    run(&board, &mut rig, 10, &pulses);
    assert_eq!(rig.valve[0].position, 204);
    assert_eq!(rig.current_dma(), 0);
    rig.valve[0].connected = true;
    rig.valve[0].shorted = true;
    run(&board, &mut rig, 1, &pulses);
    assert_eq!(rig.current_dma(), 2000);
    assert_eq!(rig.valve[0].position, 204);
}
