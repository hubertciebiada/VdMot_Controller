// Port of test/native/glue/test_i2c_bus.cpp: the bus recovery clocks SCL while a slave holds SDA
// low (at most 9 clocks), then sends a STOP; restart() stops the driver, recovers and starts it
// again.

use super::*;
use crate::test_support::fake_board::{Ev, FakeBoard};

/// SDA is held low by a slave until SCL has been pulled low `clocks` times.
fn hold_sda_for(board: &FakeBoard, clocks: usize) {
    *board.sda_input.borrow_mut() = Some(Box::new(move |events: &[Ev]| {
        let lows = events
            .iter()
            .filter(|e| **e == Ev::LineWrite(I2cLine::Scl, false))
            .count();
        Some(lows >= clocks)
    }));
}

fn scl_clocks(board: &FakeBoard) -> usize {
    let highs = board
        .events()
        .iter()
        .filter(|e| **e == Ev::LineWrite(I2cLine::Scl, true))
        .count();
    // the first HIGH presets the latch, the last one is the STOP
    highs - 2
}

fn line_modes(board: &FakeBoard) -> [LineMode; 2] {
    board.line_modes()
}

#[test]
fn recover_two_clocks_free_the_bus_then_a_stop_whole_pin_sequence_with_5_us_half_clocks() {
    let mut board = FakeBoard::new();
    hold_sda_for(&board, 2);
    let clock = board.clone();
    recover(&mut board, &clock);
    use I2cLine::{Scl, Sda};
    use LineMode::{Input, OutputOpenDrain};
    let expected = vec![
        Ev::LineMode(Sda, Input),
        Ev::LineWrite(Scl, true),
        Ev::LineMode(Scl, OutputOpenDrain),
        Ev::DelayUs(5),
        Ev::LineWrite(Scl, false),
        Ev::DelayUs(5),
        Ev::LineWrite(Scl, true),
        Ev::DelayUs(5),
        Ev::LineWrite(Scl, false),
        Ev::DelayUs(5),
        Ev::LineWrite(Scl, true),
        Ev::DelayUs(5),
        // STOP: SDA rises while SCL is high
        Ev::LineWrite(Scl, false),
        Ev::LineWrite(Sda, false),
        Ev::LineMode(Sda, OutputOpenDrain),
        Ev::DelayUs(5),
        Ev::LineWrite(Scl, true),
        Ev::DelayUs(5),
        Ev::LineWrite(Sda, true),
        Ev::DelayUs(5),
        Ev::LineMode(Sda, Input),
        Ev::LineMode(Scl, Input),
    ];
    assert_eq!(board.events(), expected);
    assert_eq!(board.now_us(), 40);
}

#[test]
fn recover_a_free_bus_gets_no_clock_a_stuck_one_at_most_9() {
    let held = [0, 1, 8, 9, 10, 20];
    let clocks = [0, 1, 8, 9, 9, 9];
    for (h, c) in held.into_iter().zip(clocks) {
        let mut board = FakeBoard::new();
        hold_sda_for(&board, h);
        let clock = board.clone();
        recover(&mut board, &clock);
        assert_eq!(scl_clocks(&board), c, "held {h}");
        assert_eq!(line_modes(&board), [LineMode::Input; 2], "held {h}");
    }
}

#[test]
fn restart_driver_stopped_bus_recovered_driver_started_again_on_the_eeprom_pins() {
    let mut board = FakeBoard::new();
    Wire::begin(&mut board);
    board.clear_events();
    hold_sda_for(&board, 1);
    let clock = board.clone();
    restart(&mut board, &clock);
    let events = board.events();
    assert_eq!(events.len(), 20);
    assert_eq!(events[0], Ev::WireEnd);
    assert_eq!(events[1], Ev::LineMode(I2cLine::Sda, LineMode::Input));
    assert_eq!(scl_clocks(&board), 1);
    assert_eq!(
        events[events.len() - 2],
        Ev::LineMode(I2cLine::Scl, LineMode::Input)
    );
    assert_eq!(events[events.len() - 1], Ev::WireBegin);
    // C++ Wire.sda / Wire.scl == the EEPROM pins: no Rust form, the driver owns PB6/PB7
    assert_eq!(events.iter().filter(|e| **e == Ev::WireEnd).count(), 1);
    assert!(board.wire_running.get());
}
