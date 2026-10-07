// Port of test/native/glue/test_terminal__mut.cpp: the read budget, the argument count and range
// of every command, the answers of stsnx/stsny, stvls, gvlon, staop and staln, failed calls; and
// new cases for the edges the mutation gate found (no C++ counterpart).

use super::tests::{Rig, ENA, ID};
use super::*;

impl Rig {
    /// every argument error of a command gives the same answer and no call
    fn check_refused(&mut self, lines: &[&str], answer: &str) {
        for line in lines {
            self.stubs.calls.clear();
            let (_, out) = self.command(&format!("{line}\n"));
            assert_eq!(out, answer, "{line}");
            assert!(self.calls().is_empty(), "{line}");
        }
    }
}

#[test]
fn terminal_serve_at_most_256_bytes_are_taken_per_call() {
    let mut r = Rig::begin();
    r.dbg.inject(&[b'x'; 300]);
    assert_eq!(r.serve(), -1);
    assert_eq!(r.dbg.available(), 44);
}

#[test]
fn learn_the_full_16_bit_range_a_refused_command_bad_arguments() {
    let mut r = Rig::begin();
    let (res, out) = r.command("learn 65535\n");
    assert_eq!(res, CMD_LEARN);
    assert!(out.is_empty());
    assert_eq!(r.calls(), vec!["appsetaction(l, 65535, 0, 0)"]);
    r.stubs.motor.action = 1;
    let (res, out) = r.command("learn 3\n");
    assert_eq!(res, CMD_LEARN);
    assert_eq!(out, "valve machine command not accepted\r\n");
    r.stubs.motor.action = 0;
    r.check_refused(
        &["learn", "learn x", "learn 1 2", "learn 65536"],
        "to few arguments\r\n",
    );
}

#[test]
fn open_and_close_100_percent_is_the_limit_a_refused_command_bad_arguments() {
    let mut r = Rig::begin();
    let (res, out) = r.command("open 4 100\n");
    assert_eq!(res, CMD_OPEN);
    assert!(out.is_empty());
    assert_eq!(r.calls(), vec!["appsetaction(o, 4, 100, 0)"]);
    r.stubs.motor.action = 1;
    assert_eq!(
        r.command("open 4 50\n"),
        (CMD_OPEN, "valve machine not idle\r\n".to_string())
    );
    assert_eq!(
        r.command("close 4 50\n"),
        (CMD_CLOSE, "valve machine not idle\r\n".to_string())
    );
    r.stubs.motor.action = 0;
    r.check_refused(
        &[
            "open 4 101",
            "open 4",
            "open 4 50 7",
            "open x 50",
            "open 4 x",
        ],
        "to few arguments\r\n",
    );
    r.check_refused(
        &[
            "close 4 101",
            "close 4",
            "close 4 50 7",
            "close x 50",
            "close 4 x",
        ],
        "to few arguments\r\n",
    );
}

#[test]
fn settar_the_last_valve_and_100_percent_nothing_beyond_bad_arguments() {
    let mut r = Rig::begin();
    assert_eq!(
        r.command("settar 11 100\n"),
        (CMD_CLOSE, "set valve 11 to 100\r\n".to_string())
    );
    assert_eq!(r.stubs.valves[11].target_position, 100);
    r.stubs.valves[1].target_position = 7;
    for quiet in ["settar 12 50\n", "settar 1 101\n"] {
        let (res, out) = r.command(quiet);
        assert_eq!(res, CMD_CLOSE, "{quiet}");
        assert!(out.is_empty(), "{quiet}");
    }
    assert_eq!(r.stubs.valves[1].target_position, 7);
    for bad in [
        "settar 1 50 7\n",
        "settar x 50\n",
        "settar 1 x\n",
        "settar 1\n",
    ] {
        assert_eq!(
            r.command(bad),
            (CMD_CLOSE, "to few arguments\r\n".to_string()),
            "{bad}"
        );
    }
    assert_eq!(r.stubs.valves[0].target_position, 0);
    assert_eq!(r.stubs.valves[1].target_position, 7);
}

#[test]
fn smux_sdir_and_stdet_take_exactly_one_number() {
    let mut r = Rig::begin();
    r.board.clear_events();
    for bad in ["smux 1 2\n", "sdir 1 2\n", "smux x\n", "sdir x\n"] {
        let (res, out) = r.command(bad);
        assert_eq!(res, 0, "{bad}");
        assert!(out.is_empty(), "{bad}");
    }
    assert!(r.board.events().is_empty());
    assert_eq!(
        r.command("stdet 255 1\n"),
        (0, "got detect valve status request - error\r\n".to_string())
    );
    assert_eq!(
        r.command("stdet x\n"),
        (0, "got detect valve status request - error\r\n".to_string())
    );
    assert!(r.calls().is_empty());
}

#[test]
fn sena_the_full_16_bit_value_switches_on() {
    let mut r = Rig::begin();
    assert_eq!(r.command("sena 1 65535\n").0, 0);
    assert!(r.term.manual_active());
}

#[test]
fn stsnx_and_stsny_slot_1_and_2_a_refused_index_bad_arguments() {
    let mut r = Rig::begin();
    assert_eq!(
        r.command("stsnx 3 65535\n"),
        (0, "comm: set 1st sensor index\r\n".to_string())
    );
    assert_eq!(
        r.command("stsny 4 5\n"),
        (0, "comm: set 2nd sensor index\r\n".to_string())
    );
    assert_eq!(
        r.calls(),
        vec![
            "comm_set_valve_sensor_index(3, 1, 65535)",
            "comm_set_valve_sensor_index(4, 2, 5)"
        ]
    );
    r.stubs.comm.set_sensor_index = 1;
    assert_eq!(
        r.command("stsny 4 5\n").1,
        "comm: set 2nd sensor index\r\ninvalid valve or sensor index\r\n"
    );
    r.stubs.comm.set_sensor_index = 0;
    r.check_refused(
        &[
            "stsnx 3",
            "stsnx 3 5 7",
            "stsnx x 5",
            "stsnx 3 x",
            "stsnx",
            "stsny 4",
        ],
        "to few arguments\r\n",
    );
}

#[test]
fn stvls_both_addresses_to_the_valve_then_the_sensors_matched_errors() {
    let mut r = Rig::begin();
    assert_eq!(
        r.command("stvls 2 28-00-00-00-00-00-00-01 00-00-00-00-00-00-00-00\n"),
        (0, "set valve sensors by address\r\n".to_string())
    );
    assert_eq!(
        r.calls(),
        vec![
            "comm_set_valve_sensors(2, 28-00-00-00-00-00-00-01, 00-00-00-00-00-00-00-00)",
            "app_match_sensors()"
        ]
    );
    r.stubs.calls.clear();
    r.stubs.comm.set_sensors = 1;
    assert_eq!(
        r.command("stvls 2 a b\n").1,
        "set valve sensors by address\r\ninvalid valve index\r\n"
    );
    assert_eq!(r.calls(), vec!["comm_set_valve_sensors(2, a, b)"]);
    r.stubs.comm.set_sensors = 0;
    r.check_refused(
        &["stvls 2 a", "stvls x a b", "stvls"],
        "set valve sensors by address\r\nto few arguments\r\n",
    );
}

#[test]
fn gvlon_the_addresses_of_the_last_valve_errors() {
    let mut r = Rig::begin();
    r.stubs.comm.first_sensor = "28-00-00-00-00-00-00-01".to_string();
    assert_eq!(
        r.command("gvlon 11\n").1,
        "cmd: get 1st and 2nd onewire sensor addresses - gvlon 11 \
         28-00-00-00-00-00-00-01 00-00-00-00-00-00-00-00 \r\n"
    );
    assert_eq!(r.calls(), vec!["comm_print_valve_sensor_ids(11, ' ')"]);
    r.check_refused(
        &["gvlon 12", "gvlon", "gvlon 1 2", "gvlon x"],
        "cmd: get 1st and 2nd onewire sensor addresses - error\r\n",
    );
}

#[test]
fn stlnt_0_s_is_a_valid_learn_time() {
    let mut r = Rig::begin();
    assert_eq!(
        r.command("stlnt 0\n"),
        (0, "set valve learning time to 0\r\n".to_string())
    );
    assert_eq!(r.calls(), vec!["comm_set_learntime(0)"]);
}

#[test]
fn staop_and_staln_the_valve_echoed_a_refused_valve_bad_arguments() {
    let mut r = Rig::begin();
    assert_eq!(
        r.command("staop 3\n"),
        (0, "got open valve request for 3\r\n".to_string())
    );
    assert_eq!(
        r.command("staln 255\n"),
        (0, "start learning for valve 255\r\n".to_string())
    );
    assert_eq!(
        r.calls(),
        vec!["app_set_valveopen(3)", "app_set_valvelearning(255)"]
    );
    r.stubs.app.set_valve_open = 1;
    r.stubs.app.set_valve_learning = 1;
    assert_eq!(
        r.command("staop 3\n").1,
        "got open valve request for - error\r\n"
    );
    assert_eq!(
        r.command("staln 3\n").1,
        "start learning for valve - error\r\n"
    );
    r.stubs.app.set_valve_open = 0;
    r.stubs.app.set_valve_learning = 0;
    r.check_refused(
        &["staop", "staop 1 2", "staop x"],
        "got open valve request for - error\r\n",
    );
    r.check_refused(
        &["staln", "staln 1 2", "staln x"],
        "start learning for valve - error\r\n",
    );
}

#[test]
fn smotc_exactly_two_numbers() {
    let mut r = Rig::begin();
    r.check_refused(
        &["smotc 17 17 1", "smotc x 17", "smotc 17 x", "smotc 17"],
        "got set motor characteristics request - error\r\n",
    );
}

// ---------------------------------------------------------------- new cases

#[test]
fn smux_on_a_c1_board_drives_the_pin_high() {
    let mut r = Rig::begin();
    r.term = Terminal::new(ID, true);
    r.command("smux 1\n");
    assert!(r.board.out(Out::Mux));
    r.command("smux 0\n");
    assert!(!r.board.out(Out::Mux));
    r.command("smux 2\n");
    assert!(r.board.out(Out::Mux));
}

#[test]
fn gmotc_stons_and_one_line_per_call() {
    let mut r = Rig::begin();
    r.stubs.motor_globals.low_fac = 12;
    r.stubs.motor_globals.high_fac = 34;
    assert_eq!(
        r.command("gmotc\n"),
        (
            0,
            "got get motor characteristics request - low: 12 high: \r\n34\r\n".to_string()
        )
    );
    assert_eq!(
        r.command("stons\n"),
        (0, "start new 1-wire search".to_string())
    );
    assert_eq!(r.calls(), vec!["temp_command(1)"]);
    // two lines: one per call
    r.dbg.inject_str("gvers\ngvers\n");
    assert_eq!(r.serve(), 0);
    assert_eq!(r.dbg.take_tx(), "Version: 2.1.7-revamped\r\n");
    assert_eq!(r.serve(), 0);
    assert_eq!(r.serve(), -1);
}

#[test]
fn sena_off_of_another_channel_and_the_enable_of_channel_5() {
    let mut r = Rig::begin();
    r.command("sena 5 1\n");
    assert!(r.board.out(Out::Ena5));
    for &pin in &ENA[..5] {
        assert!(r.board.writes_of(pin).is_empty());
    }
    // 0 for the channel that runs: the manual enable ends with the PSU
    r.command("sena 5 0\n");
    assert!(!r.term.manual_active());
    assert!(r.board.out(Out::PsuEna));
    // the time limit runs from the start of each sena
    r.board.advance_ms(5000);
    r.command("sena 3 1\n");
    r.board.advance_ms(1999);
    r.supervise();
    assert!(r.term.manual_active());
    r.board.advance_ms(1);
    r.supervise();
    assert!(!r.term.manual_active());
    assert!(!r.board.out(Out::Ena3));
}
