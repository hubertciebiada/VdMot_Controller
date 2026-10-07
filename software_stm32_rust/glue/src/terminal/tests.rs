// Port of test/native/glue/test_terminal.cpp: banner, line handling, the debug terminal's
// commands with their output and calls, and the limits of the terminal's hardware access (S10):
// a motor output switched on by sena goes off after 2 s or above 60 mA, smux/sdir/sena only while
// the valve machine is idle, seteep only while the EEPROM is readable, stdet answers on the debug
// port only.

use super::*;
use crate::test_support::fake_board::FakeBoard;
use crate::test_support::io_fakes::FakeSerial;
use crate::test_support::stubs::Stubs;
use vdm_stm_core::replies_v2::EEP_STATE_WRITE_FAILED;

pub(super) const ID: FirmwareId = FirmwareId {
    version: b"2.1.7-revamped",
    tag: b"C2",
    build: b"1",
};

/// terminal.cpp with the debug UART, the fake board (pins, time) and the stubs; C2 wiring.
pub(super) struct Rig {
    pub term: Terminal,
    pub dbg: FakeSerial,
    pub board: FakeBoard,
    pub stubs: Stubs,
}

pub(super) const ENA: [Out; 6] = [
    Out::Ena0,
    Out::Ena1,
    Out::Ena2,
    Out::Ena3,
    Out::Ena4,
    Out::Ena5,
];

impl Rig {
    pub fn new() -> Self {
        Rig {
            term: Terminal::new(ID, false),
            dbg: FakeSerial::new(),
            board: FakeBoard::new(),
            stubs: Stubs::default(),
        }
    }

    /// Terminal_Init(), the banner taken
    pub fn begin() -> Self {
        let mut r = Rig::new();
        r.term.init(&mut r.dbg);
        r.dbg.take_tx();
        r
    }

    pub fn serve(&mut self) -> i16 {
        self.term
            .serve(&mut self.dbg, &self.board, &self.board, &mut self.stubs)
    }

    /// one line through serve(): its result and the terminal output
    pub fn command(&mut self, line: &str) -> (i16, String) {
        self.dbg.inject_str(line);
        let result = self.serve();
        (result, self.dbg.take_tx())
    }

    pub fn supervise(&mut self) {
        self.term
            .supervise(&mut self.dbg, &self.board, &self.board, &self.stubs);
    }

    pub fn calls(&self) -> Vec<String> {
        self.stubs.calls.calls.clone()
    }
}

#[test]
fn terminal_init_banner_with_version_and_revision() {
    let mut r = Rig::new();
    assert_eq!(r.term.init(&mut r.dbg), 0);
    // C++ also checks PA12/PA11, 115200 and 8N1: the firmware sets USART6 up
    assert_eq!(r.dbg.take_tx(), "VdMot Controller 2.1.7-revamped_C2\r\n");
    assert_eq!(r.dbg.flushes, 1);
}

#[test]
fn terminal_serve_no_complete_line_too_many_arguments_unknown_command() {
    let mut r = Rig::begin();
    let (res, out) = r.command("help");
    assert_eq!(res, -1);
    assert!(out.is_empty());
    let (res, out) = r.command("\n");
    assert_eq!(res, CMD_HELP);
    assert_eq!(out, "Help:\r\n*********************\r\n");
    let (res, out) = r.command("open 1 2 3 4\n");
    assert_eq!(res, -1);
    assert_eq!(out, "too many arguments\r\n");
    let (res, out) = r.command("xyzzy\n");
    assert_eq!(res, CMD_NONE);
    assert_eq!(out, "unknown command\r\n");
    assert!(r.calls().is_empty());
}

#[test]
fn terminal_serve_learn_open_and_close_hand_the_command_to_the_valve_machine() {
    let mut r = Rig::begin();
    assert_eq!(r.command("learn 3\n").0, CMD_LEARN);
    assert_eq!(r.command("open 4 50\n").0, CMD_OPEN);
    let (res, out) = r.command("close 5 100\n");
    assert_eq!(res, CMD_CLOSE);
    assert!(out.is_empty());
    let (res, out) = r.command("close 5 101\n");
    assert_eq!(res, CMD_CLOSE);
    assert_eq!(out, "to few arguments\r\n");
    r.stubs.motor.action = -1;
    let (res, out) = r.command("open 4 50\n");
    assert_eq!(res, CMD_OPEN);
    assert_eq!(out, "valve machine not idle\r\n");
    assert_eq!(
        r.calls(),
        vec![
            "appsetaction(l, 3, 0, 0)",
            "appsetaction(o, 4, 50, 0)",
            "appsetaction(c, 5, 100, 0)",
            "appsetaction(o, 4, 50, 0)"
        ]
    );
}

#[test]
fn terminal_serve_settar_gvers_and_stdet_255() {
    let mut r = Rig::begin();
    let (res, out) = r.command("settar 2 40\n");
    assert_eq!(res, CMD_CLOSE);
    assert_eq!(out, "set valve 2 to 40\r\n");
    assert_eq!(r.stubs.valves[2].target_position, 40);
    let (res, out) = r.command("gvers\n");
    assert_eq!(res, 0);
    assert_eq!(out, "Version: 2.1.7-revamped\r\n");
    let (res, out) = r.command("stdet 255\n");
    assert_eq!(res, 0);
    assert_eq!(
        out,
        "got detect valve status request - reset all valves\r\nstdet \r\n"
    );
    // C++: nothing on USART1; the terminal has no access to the ESP UART
    assert_eq!(r.calls(), vec!["app_scan_valves()"]);
    let (res, out) = r.command("stdet 3\n");
    assert_eq!(res, 0);
    assert_eq!(out, "got detect valve status request - error\r\nstdet \r\n");
    let (res, out) = r.command("stdet\n");
    assert_eq!(res, 0);
    assert_eq!(out, "got detect valve status request - error\r\n");
    assert_eq!(r.calls(), vec!["app_scan_valves()"]);
}

fn descr(text: &[u8]) -> [u8; 25] {
    let mut d = [0u8; 25];
    d[..text.len()].copy_from_slice(text);
    d
}

#[test]
fn terminal_serve_seteep_sets_the_base_fields_and_marks_everything_saveep_marks_everything() {
    let mut r = Rig::begin();
    let l = &mut r.stubs.eep_content.cfg.layout;
    l.descr = descr(b"old");
    l.b_slave = 3;
    l.one_wire_cfg = [1, 2, 3];
    let (res, out) = r.command("seteep\n");
    assert_eq!(res, 0);
    assert_eq!(out, "write EEPROM layout...\r\nset eeprom layout\r\n");
    let l = r.stubs.eep_content.cfg.layout;
    assert_eq!(l.descr, descr(b"VdMot Controller"));
    assert_eq!(l.b_slave, 0);
    assert_eq!(l.one_wire_cfg, [0, 0, 0]);
    let (res, out) = r.command("saveep\n");
    assert_eq!(res, 0);
    assert_eq!(out, "saved eeprom layout\r\n");
    assert_eq!(
        r.calls(),
        vec![
            "eeprom_state()",
            "eeprom_changed(0x00ff)",
            "eeprom_changed(0x00ff)"
        ]
    );
}

#[test]
fn terminal_serve_seteep_is_refused_while_the_eeprom_could_not_be_read() {
    let mut r = Rig::begin();
    r.stubs.eeprom.state = EEP_STATE_READ_FAILED;
    r.stubs.eep_content.cfg.layout.descr = descr(b"old");
    let (res, out) = r.command("seteep\n");
    assert_eq!(res, 0);
    assert_eq!(out, "eeprom not readable, write blocked\r\n");
    assert_eq!(r.stubs.eep_content.cfg.layout.descr, descr(b"old"));
    assert_eq!(r.calls(), vec!["eeprom_state()"]);
    r.stubs.eeprom.state = EEP_STATE_WRITE_FAILED;
    let (res, out) = r.command("seteep\n");
    assert_eq!(res, 0);
    assert_eq!(out, "write EEPROM layout...\r\nset eeprom layout\r\n");
}

#[test]
fn terminal_serve_getone_prints_the_sensor_data_stm_is_gone() {
    let mut r = Rig::begin();
    r.stubs.ow.sensor_data = "{\"cnt\":0}\r\n".to_string();
    let (res, out) = r.command("getone\n");
    assert_eq!(res, 0);
    assert_eq!(out, "{\"cnt\":0}\r\n");
    let (res, out) = r.command("stm 1\n");
    assert_eq!(res, CMD_NONE);
    assert_eq!(out, "unknown command\r\n");
}

#[test]
fn sena_the_output_and_the_valve_psu_on_off_after_2_s_the_valve_machine_gets_no_command() {
    for (ch, &pin) in ENA.iter().enumerate() {
        let mut r = Rig::begin();
        r.board.advance_ms(12345);
        r.stubs.calls.clear();
        let (res, out) = r.command(&format!("sena {ch} 1\n"));
        assert_eq!(res, 0);
        assert!(out.is_empty());
        assert_eq!(r.calls(), vec!["valve_idle()", "sysstat_safe_mode()"]);
        assert!(r.board.out(pin));
        assert!(!r.board.out(Out::PsuEna));
        for (other, &o) in ENA.iter().enumerate() {
            if other != ch {
                assert!(r.board.writes_of(o).is_empty(), "ch {ch} other {other}");
            }
        }
        assert!(r.term.manual_active());
        r.board.advance_ms(1999);
        r.supervise();
        assert!(r.board.out(pin));
        assert!(r.term.manual_active());
        assert!(r.dbg.take_tx().is_empty());
        r.board.advance_ms(1);
        r.supervise();
        assert!(!r.board.out(pin));
        assert!(r.board.out(Out::PsuEna));
        assert!(!r.term.manual_active());
        assert_eq!(r.dbg.take_tx(), "sena: output off\r\n");
        assert_eq!(r.board.writes_of(pin).len(), 2);
        r.supervise();
        assert!(r.dbg.take_tx().is_empty());
    }
}

#[test]
fn sena_off_at_once_above_60_ma_filtered_current_either_sign() {
    for current in [601, -601] {
        let mut r = Rig::begin();
        r.command("sena 2 1\n");
        r.stubs.analog_current = if current < 0 { -600 } else { 600 };
        r.supervise();
        assert!(r.term.manual_active());
        r.stubs.analog_current = current;
        r.supervise();
        assert!(!r.term.manual_active());
        assert!(!r.board.out(Out::Ena2));
    }
}

#[test]
fn sena_refused_while_the_valve_machine_works_or_in_safe_mode_0_switches_off_bad_arguments() {
    let mut r = Rig::begin();
    r.stubs.motor.idle = false;
    let (res, out) = r.command("sena 1 1\n");
    assert_eq!(res, 0);
    assert_eq!(out, "valve machine busy\r\n");
    assert!(r.board.writes_of(Out::Ena1).is_empty());
    assert!(!r.term.manual_active());
    r.stubs.motor.idle = true;
    r.stubs.sysstat.safe_mode = true;
    let (res, out) = r.command("sena 1 1\n");
    assert_eq!(res, 0);
    assert_eq!(out, "valve machine busy\r\n");
    assert!(r.board.writes_of(Out::Ena1).is_empty());
    assert!(r.board.writes_of(Out::PsuEna).is_empty());
    r.stubs.sysstat.safe_mode = false;
    // off: only that output, the manual enable of another output stays
    r.command("sena 1 1\n");
    assert_eq!(r.command("sena 4 0\n").0, 0);
    assert!(!r.board.out(Out::Ena4));
    assert_eq!(r.board.writes_of(Out::Ena4).len(), 1);
    assert!(r.term.manual_active());
    assert!(!r.board.out(Out::PsuEna));
    assert_eq!(r.command("sena 1 0\n").0, 0);
    assert!(!r.term.manual_active());
    assert!(!r.board.out(Out::Ena1));
    assert!(r.board.out(Out::PsuEna));
    // a second sena ends the first one
    r.command("sena 1 1\n");
    r.board.advance_ms(1500);
    r.command("sena 2 1\n");
    assert!(!r.board.out(Out::Ena1));
    assert!(r.board.out(Out::Ena2));
    assert!(!r.board.out(Out::PsuEna));
    r.board.advance_ms(1999);
    r.supervise();
    assert!(r.term.manual_active());
    r.board.clear_events();
    for bad in ["sena 6 1\n", "sena 1\n", "sena 1 x\n", "sena x 1\n"] {
        let (res, out) = r.command(bad);
        assert_eq!(res, 0, "{bad}");
        assert!(out.is_empty(), "{bad}");
    }
    assert!(r.board.events().is_empty());
}

#[test]
fn smux_and_sdir_only_while_the_valve_machine_is_idle() {
    let mut r = Rig::begin();
    assert_eq!(r.command("smux 1\n").0, 0);
    // C2: MUX_ON() drives the pin low
    assert!(!r.board.out(Out::Mux));
    let (res, out) = r.command("sdir 1\n");
    assert_eq!(res, 0);
    assert!(r.board.out(Out::Dir));
    assert!(out.is_empty());
    assert_eq!(r.command("smux 0\n").0, 0);
    assert!(r.board.out(Out::Mux));
    assert_eq!(r.command("sdir 0\n").0, 0);
    assert!(!r.board.out(Out::Dir));
    r.stubs.motor.idle = false;
    r.board.clear_events();
    let (res, out) = r.command("smux 1\n");
    assert_eq!(res, 0);
    assert_eq!(out, "valve machine busy\r\n");
    let (res, out) = r.command("sdir 1\n");
    assert_eq!(res, 0);
    assert_eq!(out, "valve machine busy\r\n");
    assert!(r.board.events().is_empty());
    assert_eq!(r.command("smux\n").0, 0);
    let (res, out) = r.command("sdir\n");
    assert_eq!(res, 0);
    assert!(out.is_empty());
}

#[test]
fn terminal_stlnt_stores_the_learn_time_through_comm_set_learntime_smotc_marks_only_a_change() {
    let mut r = Rig::begin();
    let (res, out) = r.command("stlnt 3600\n");
    assert_eq!(res, 0);
    assert_eq!(out, "set valve learning time to 3600\r\n");
    r.stubs.comm.set_learn_time = -1;
    assert_eq!(
        r.command("stlnt 5\n").1,
        "set valve learning time to - error\r\n"
    );
    assert_eq!(
        r.command("stlnt\n").1,
        "set valve learning time to - error\r\n"
    );
    assert_eq!(
        r.calls(),
        vec!["comm_set_learntime(3600)", "comm_set_learntime(5)"]
    );
    r.stubs.calls.clear();
    let (res, out) = r.command("smotc 17 17\n");
    assert_eq!(res, 0);
    assert_eq!(out, "got set motor characteristics request - valid\r\n");
    assert_eq!(r.calls(), vec!["motor_get_params()"]);
    r.stubs.calls.clear();
    assert_eq!(
        r.command("smotc 18 17\n").1,
        "got set motor characteristics request - valid\r\n"
    );
    assert_eq!(
        r.calls(),
        vec![
            "motor_get_params()",
            "motor_set_params(18, 17, 50, 3000, 0)",
            "eeprom_changed(0x0004)"
        ]
    );
    r.stubs.calls.clear();
    assert_eq!(
        r.command("smotc 17 99\n").1,
        "got set motor characteristics request - values out of bounds\r\n"
    );
    assert_eq!(r.calls(), vec!["motor_get_params()"]);
}
