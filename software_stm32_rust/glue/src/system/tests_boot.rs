// Port of test/native/glue/test_system_boot.cpp (glue_system): end-to-end smoke tests of the
// whole STM glue with the valve sim: the controller boots through the window, answers today's
// v1 requests byte-exact, and a soft reset reboots it. Every case also compares its whole
// transcript with the C++ golden (design §7.4).

use super::bench::{Case, PLAIN};
use super::golden::Reset;

#[test]
fn a_new_controller_answers_gvers_gproto_gtgtp_gvlvd_gstat_and_stgtp_like_2_0_0() {
    let mut case = Case::new(
        "boot__a_new_controller_answers_gvers_gproto_gtgtp_gvlvd_gstat_and_stgt",
        "system: a new controller answers gvers, gproto, gtgtp, gvlvd, gstat and stgtp like 2.0.0",
    );
    let mut b = case.boot(PLAIN, |_| {});
    assert_eq!(b.s.board.now_us() / 1000, 3511);
    assert_eq!(b.exchange("gvers\n"), "gvers 2.1.7-revamped_C2 1 \r\n");
    assert_eq!(b.exchange("gproto\n"), "gproto 3\r\n");
    // erased EEPROM: startOnPower loads its default 30
    assert_eq!(b.exchange("gtgtp 0\n"), "gtgtp 0 30 \r\n");
    // valve 0 is still under its presence test: status 5 (unknown)
    assert_eq!(
        b.exchange("gvlvd 0\n"),
        "gvlvd 0 30 20 5 -500 -500 0 0 0 0 0 \r\n"
    );
    // the first start stores the configuration with its CRC: eepState 1 until the write 3 s later
    assert_eq!(b.exchange("gstat\n"), "gstat 3 0 1 0 0 1\r\n");
    assert_eq!(b.exchange("stgtp 0 50\n"), "stgtp\r\n");
    assert_eq!(b.exchange("gtgtp 0\n"), "gtgtp 0 50 \r\n");
    b.finish();
    case.check_golden();
}

#[test]
fn the_presence_test_finds_every_connected_valve_an_open_one_is_reported() {
    let mut case = Case::new(
        "boot__the_presence_test_finds_every_connected_valve_an_open_one_is_rep",
        "system: the presence test finds every connected valve, an open one is reported",
    );
    let mut b = case.boot(PLAIN, |_| {});
    b.sim().valve[5].connected = false;
    b.run_main(40000);
    let reply = b.exchange("gvlst\n");
    assert_eq!(reply, "gvlst 12 8,8,8,8,8,6,8,8,8,8,8,8 \r\n");
    assert_eq!(b.sim().conflicts, 0);
    b.finish();
    case.check_golden();
}

#[test]
fn reset_answers_waits_for_the_eeprom_and_restarts_the_controller_software_reset() {
    let mut case = Case::new(
        "boot__reset_answers_waits_for_the_eeprom_and_restarts_the_controller_s",
        "system: reset answers, waits for the EEPROM and restarts the controller (software reset)",
    );
    let mut b = case.boot(PLAIN, |_| {});
    b.inject("reset\n");
    let reset = b.run(|b| {
        while b.tx().is_empty() {
            b.run_main(1);
        }
        assert_eq!(b.take_tx(), "reset \r\n");
        // the first start's configuration write comes first
        b.run_main(4000);
        panic!("the controller did not reset");
    });
    assert_eq!(reset, Some(Reset::Software));
    assert_eq!(case.boot_index(), 1);
    let mut b = case.boot(PLAIN, |_| {});
    assert_eq!(b.exchange("gstat\n"), "gstat 3 1 3 0 0 0\r\n");
    b.finish();
    case.check_golden();
}
