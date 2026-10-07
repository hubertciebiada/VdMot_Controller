// Port of test/native/glue/test_communication__mut.cpp: the first and last valve and sensor
// index of every command that takes one, the separators of the gonec/gowvc lists, the 5-value
// smotc, svmov with one argument, the pause of reset; and new cases for the edges the mutation
// gate found (no C++ counterpart).

use super::tests::Rig;
use super::*;
use vdm_stm_core::replies_v2::EEP_STATE_READ_FAILED;

const ADDR_A: DeviceAddress = [0x28, 1, 2, 3, 4, 5, 6, 0x77];
const ADDR_B: DeviceAddress = [0x26, 9, 8, 7, 6, 5, 4, 0x33];

#[test]
fn stgtp_gtgtp_valve_11_positions_0_and_100_valve_12_is_refused() {
    let mut r = Rig::begin();
    assert_eq!(r.request("stgtp 11 100\n"), "stgtp\r\n");
    assert_eq!(r.stubs.valves[11].target_position, 100);
    assert_eq!(r.request("stgtp 1 0\n"), "stgtp\r\n");
    assert_eq!(r.stubs.valves[1].target_position, 0);
    assert_eq!(r.request("gtgtp 11\n"), "gtgtp 11 100 \r\n");
    assert!(r.request("gtgtp 12\n").is_empty());
}

#[test]
fn gvlvd_gvlvx_gprof_gvlvy_valves_0_and_11_answer_12_does_not() {
    let mut r = Rig::begin();
    for cmd in ["gvlvd", "gvlvx", "gprof", "gvlvy"] {
        assert!(
            r.request(&format!("{cmd} 0\n"))
                .starts_with(&format!("{cmd} 0")),
            "{cmd}"
        );
        assert!(
            r.request(&format!("{cmd} 11\n"))
                .starts_with(&format!("{cmd} 11")),
            "{cmd}"
        );
    }
    assert!(r.request("gvlvx 12\n").is_empty());
}

#[test]
fn gonec_255_one_space_before_the_list_254_and_256_are_no_list() {
    let mut r = Rig::begin();
    r.stubs.sensors.no_of_ds18_devices = 1;
    r.stubs.sensors.tempsensors[0].address = ADDR_A;
    assert_eq!(
        r.request("gonec 255\n"),
        "gonec 1 28-01-02-03-04-05-06-77 \r\n"
    );
    assert!(r.request("gonec 254\n").is_empty());
    assert!(r.request("gonec 256\n").is_empty());
    assert!(r.request("gonec 0\n").is_empty());
}

#[test]
fn gowvc_255_no_list_a_list_of_two_254_and_256_are_no_list() {
    let mut r = Rig::begin();
    assert_eq!(r.request("gowvc 255\n"), "gowvc 0 \r\n");
    r.stubs.sensors.no_of_ds2438_devices = 2;
    r.stubs.sensors.voltsensors[0].address = ADDR_A;
    r.stubs.sensors.voltsensors[1].address = ADDR_B;
    assert_eq!(
        r.request("gowvc 255\n"),
        "gowvc 2 28-01-02-03-04-05-06-77,26-09-08-07-06-05-04-33 \r\n"
    );
    assert!(r.request("gowvc 254\n").is_empty());
    assert!(r.request("gowvc 256\n").is_empty());
    assert!(r.request("gowvc 0\n").is_empty());
}

#[test]
fn goned_and_gowvd_the_first_and_the_last_sensor_index() {
    let mut r = Rig::begin();
    r.stubs.sensors.tempsensors[0].address = ADDR_A;
    r.stubs.sensors.tempsensors[MAXONEWIRECNT - 1].address = ADDR_B;
    r.stubs.sensors.voltsensors[MAXDS2438CNT - 1].address = ADDR_B;
    assert!(r
        .request("goned 0\n")
        .starts_with("goned 28-01-02-03-04-05-06-77 "));
    assert!(r
        .request(&format!("goned {}\n", MAXONEWIRECNT - 1))
        .starts_with("goned 26-09-"));
    assert_eq!(
        r.request(&format!("goned {}\n", MAXONEWIRECNT)),
        "goned 0 \r\n"
    );
    assert!(r
        .request(&format!("gowvd {}\n", MAXDS2438CNT - 1))
        .starts_with("gowvd 26-09-"));
}

#[test]
fn gvlon_valves_0_and_11_255_254_and_256_are_errors() {
    let mut r = Rig::begin();
    assert!(r.request("gvlon 0\n").starts_with("gvlon 0 "));
    assert!(r.request("gvlon 11\n").starts_with("gvlon 11 "));
    assert!(r.request("gvlon 255\n").starts_with("gvlon 12 "));
    assert_eq!(r.request("gvlon 254\n"), "goned error \r\n");
    assert_eq!(r.request("gvlon 256\n"), "goned error \r\n");
}

#[test]
fn stsnx_stvls_the_first_and_last_valve_and_sensor_index() {
    let mut r = Rig::begin();
    r.stubs.sensors.no_of_ds18_devices = MAXONEWIRECNT as u8;
    assert_eq!(r.request("stsnx 0 0\n"), "stsnx\r\n");
    assert_eq!(
        r.request(&format!("stsny 11 {}\n", MAXONEWIRECNT - 1)),
        "stsny\r\n"
    );
    assert!(r.request("stsnx 12 0\n").is_empty());
    assert!(r
        .request(&format!("stsnx 0 {}\n", MAXONEWIRECNT))
        .is_empty());
    assert_eq!(
        r.request("stvls 0 00-00-00-00-00-00-00-00 00-00-00-00-00-00-00-00\n"),
        "stvls 0\r\n"
    );
    assert_eq!(
        r.request("stvls 11 00-00-00-00-00-00-00-00 00-00-00-00-00-00-00-00\n"),
        "stvls 11\r\n"
    );
    assert_eq!(comm_set_valve_sensors(&mut r.stubs, 0, b"", b""), 0);
    assert_eq!(comm_set_valve_sensors(&mut r.stubs, 12, b"", b""), -1);
}

#[test]
fn staop_staln_stdet_0_and_65535_reach_the_handler() {
    let mut r = Rig::begin();
    r.request("staop 0\n");
    r.request("staop 65535\n");
    r.request("staln 65535\n");
    assert_eq!(
        r.calls_of("app_set_valveopen"),
        vec!["app_set_valveopen(0)", "app_set_valveopen(65535)"]
    );
    assert_eq!(
        r.calls_of("app_set_valvelearning"),
        vec!["app_set_valvelearning(65535)"]
    );
    assert_eq!(r.request("stdet 0\n"), "stdet err\r\n");
    assert_eq!(r.request("stdet 65535\n"), "stdet err\r\n");
    assert!(r.request("stdet 65536\n").is_empty());
}

#[test]
fn stlnt_0_and_stlnm_0_are_taken() {
    let mut r = Rig::begin();
    assert_eq!(r.request("stlnt 0\n"), "stlnt\r\n");
    assert_eq!(
        r.calls_of("app_set_learntime"),
        vec!["app_set_learntime(0)"]
    );
    r.request("stlnm 0\n");
    assert_eq!(
        r.calls_of("app_set_learnmovements"),
        vec!["app_set_learnmovements(0)"]
    );
}

#[test]
fn smotc_five_values_zeros_included() {
    let mut r = Rig::begin();
    assert_eq!(r.request("smotc 18 19 0 0 0\n"), "smotc\r\n");
    // too many arguments: dropped
    assert!(r.request("smotc 18 19 0 0 0 0\n").is_empty());
}

#[test]
fn svmov_valves_0_and_11_one_argument_reports_its_valve() {
    let mut r = Rig::begin();
    assert_eq!(r.request("svmov 0 1 100 20\n"), "svmov 0 ok\r\n");
    assert_eq!(r.request("svmov 11 1 100 20\n"), "svmov 11 ok\r\n");
    assert_eq!(r.request("svmov 3\n"), "svmov 3 err 1\r\n");
}

#[test]
fn scalx_enable_0_and_step_0_are_taken() {
    let mut r = Rig::begin();
    assert_eq!(r.request("scalx 0 0 40\n"), "scalx ok\r\n");
}

#[test]
fn reset_the_reply_200_ms_for_it_to_leave_then_the_reset_request() {
    let mut r = Rig::begin();
    let before = r.board.now_us();
    assert_eq!(r.request("reset\n"), "reset \r\n");
    assert_eq!(r.board.now_us() - before, 200_000);
    assert_eq!(r.calls_of("reset_STM32"), vec!["reset_STM32()"]);
}

// ---------------------------------------------------------------- new cases

#[test]
fn stvls_and_stsny_of_valve_0_store_into_its_second_slot() {
    let mut r = Rig::begin();
    let mut a = [0x28, 0x33, 0, 0, 0, 0, 0, 0];
    a[7] = crate::onewire::crc8(&a[..7]);
    r.stubs.sensors.tempsensors[1].address = a;
    r.stubs.sensors.no_of_ds18_devices = 2;
    let e = r.exchange("stsny 0 1\n");
    assert_eq!(e.reply, "stsny\r\n");
    assert_eq!(e.calls, vec!["eeprom_changed_slot(12)"]);
    let layout = r.stubs.eep_content.cfg.layout;
    assert_eq!(layout.owsensors2[0].romcode[0], 0x33);
    assert_eq!(layout.owsensors1[0].familycode, 0);
    assert_eq!(r.stubs.valves[0].sensorindex2, 1);
    assert_eq!(r.stubs.valves[0].sensorindex1, 0);
    // stvls of valve 0: the first address into slot 0, the second into slot 12
    r.stubs.eep_content.cfg.layout.owsensors2[0] = Default::default();
    let text = "28-33-00-00-00-00-00-".to_string() + &format!("{:02x}", a[7]);
    let e = r.exchange(&format!("stvls 0 00-00-00-00-00-00-00-00 {text}\n"));
    assert_eq!(e.reply, "stvls 0\r\n");
    assert_eq!(e.calls, vec!["eeprom_changed_slot(12)"]);
    assert_eq!(r.stubs.eep_content.cfg.layout.owsensors2[0].crc, a[7]);
    assert_eq!(r.stubs.eep_content.cfg.layout.owsensors1[0].familycode, 0);
}

#[test]
fn stored_sensor_slots_keep_the_byte_order_of_the_address() {
    let mut r = Rig::begin();
    let mut a = [0x28, 1, 2, 3, 4, 5, 6, 0];
    a[7] = crate::onewire::crc8(&a[..7]);
    r.stubs.sensors.tempsensors[0].address = a;
    r.stubs.sensors.no_of_ds18_devices = 1;
    assert_eq!(r.request("stsnx 5 0\n"), "stsnx\r\n");
    let s = r.stubs.eep_content.cfg.layout.owsensors1[5];
    assert_eq!(s.familycode, 0x28);
    assert_eq!(s.romcode, [1, 2, 3, 4, 5, 6]);
    assert_eq!(s.crc, a[7]);
    // a slot that differs in the last rom byte only is stored again
    r.stubs.eep_content.cfg.layout.owsensors1[5].romcode[5] = 9;
    assert_eq!(
        r.exchange("stsnx 5 0\n").calls,
        vec!["eeprom_changed_slot(5)"]
    );
}

#[test]
fn gvlvd_and_gvlvx_report_temperatures_counts_and_clamps() {
    let mut r = Rig::begin();
    let v = &mut r.stubs.valves[4];
    v.sensorindex1 = 33;
    v.sensorindex2 = 34;
    v.meancurrent = 0x8000_0001;
    v.movements = u32::MAX;
    v.opening_count = 70000;
    v.closing_count = 1;
    v.deadzone_count = -69999;
    r.stubs.sensors.tempsensors[33].temperature = -1270;
    assert_eq!(
        r.request("gvlvd 4\n"),
        "gvlvd 4 0 -2147483647 0 -1270 -500 -1 70000 1 -69999 0 \r\n"
    );
    // gvlvx: the mean current is clamped to 65535, the status carries the calibration bit
    let s = &mut r.stubs.motor.snapshot[4];
    s.meancurrent = 70000;
    s.status = 8;
    s.calibration = true;
    s.calib_active = true;
    r.stubs.app.learn_pending = true;
    let e = r.exchange("gvlvx 4\n");
    assert_eq!(
        e.reply,
        "gvlvx 4 136 0 0 65535 0 0 0 0 0 2 0 0 0 0 0 0 0 0\r\n"
    );
    assert_eq!(e.calls[2], "app_learn_pending(4, 8, 1)");
    r.stubs.motor.snapshot[4].meancurrent = 65535;
    r.stubs.motor.snapshot[4].calib_active = false;
    assert!(r
        .request("gvlvx 4\n")
        .starts_with("gvlvx 4 136 0 0 65535 0 0 0 0 0 1 "));
}

#[test]
fn gvlst_lists_the_status_of_every_valve() {
    let mut r = Rig::begin();
    for (v, s) in r.stubs.valves.iter_mut().enumerate() {
        s.status = v as u8 + 1;
    }
    assert_eq!(
        r.request("gvlst\n"),
        "gvlst 12 1,2,3,4,5,6,7,8,9,10,11,12 \r\n"
    );
    assert_eq!(r.dbg.take_tx(), "cmd: get valve status\r\n");
}

#[test]
fn gvlon_of_every_valve_lists_both_slots_with_commas() {
    let mut r = Rig::begin();
    r.stubs.sensors.tempsensors[0].address = ADDR_A;
    r.stubs.sensors.tempsensors[1].address = ADDR_B;
    for v in r.stubs.valves.iter_mut() {
        v.sensorindex1 = 1;
        v.sensorindex2 = 65535;
    }
    r.stubs.valves[0].sensorindex1 = 0;
    let all = r.request("gvlon 255\n");
    let mut want = String::from("gvlon 12 28-01-02-03-04-05-06-77,00-00-00-00-00-00-00-00");
    for _ in 1..12 {
        want += ",26-09-08-07-06-05-04-33,00-00-00-00-00-00-00-00";
    }
    want += " \r\n";
    assert_eq!(all, want);
    assert_eq!(
        r.request("gvlon 1\n"),
        "gvlon 1 26-09-08-07-06-05-04-33 00-00-00-00-00-00-00-00 \r\n"
    );
    assert_eq!(
        r.dbg.take_tx(),
        "cmd: get 1st and 2nd onewire sensor addressescmd: get 1st and 2nd onewire sensor addresses"
    );
    assert!(r.request("gvlon 1 2\n").starts_with("goned error"));
    assert_eq!(
        r.dbg.take_tx(),
        "cmd: get 1st and 2nd onewire sensor addresses - error\r\n"
    );
    let mut out = FakeSerialPrint::default();
    comm_print_valve_sensor_ids(&r.stubs, &mut out, 12, b' ');
    assert!(out.0.is_empty());
}

#[derive(Default)]
struct FakeSerialPrint(Vec<u8>);

impl Print for FakeSerialPrint {
    fn write_bytes(&mut self, bytes: &[u8]) {
        self.0.extend_from_slice(bytes);
    }
}

#[test]
fn the_gonec_list_stops_at_the_table_size_and_goned_reads_the_last_sensor() {
    let mut r = Rig::begin();
    r.stubs.sensors.no_of_ds18_devices = 40;
    r.stubs.sensors.tempsensors[33].temperature = 77;
    let reply = r.request("gonec 255\n");
    assert!(reply.starts_with("gonec 34 00-00-"));
    // 34 addresses, 33 commas
    assert_eq!(reply.matches(',').count(), 33);
    assert_eq!(r.request("gonec\n"), "gonec 34 \r\n");
    assert_eq!(
        r.request("goned 33\n"),
        "goned 00-00-00-00-00-00-00-00 77 \r\n"
    );
    r.stubs.sensors.no_of_ds2438_devices = 9;
    r.stubs.sensors.voltsensors[7].vad = -1000;
    assert_eq!(r.request("gowvc\n"), "gowvc 8 \r\n");
    assert_eq!(r.request("gowvc 255\n").matches(',').count(), 7);
    assert_eq!(
        r.request("gowvd 7\n"),
        "gowvd 00-00-00-00-00-00-00-00 -1000 \r\n"
    );
    assert_eq!(r.request("gowvd\n"), "gowvd 0 \r\n");
    assert_eq!(r.request("gowvd 0 1\n"), "gowvd 0 \r\n");
    assert_eq!(r.request("goned 0 1\n"), "goned 0 \r\n");
}

#[test]
fn the_debug_lines_of_the_v1_commands() {
    let mut r = Rig::begin();
    r.request("stons\n");
    r.request("stlnt 9\n");
    r.request("stlnt x\n");
    r.request("stlnm 70000\n");
    r.request("stlnm\n");
    r.request("gtlnm\n");
    r.request("ESPalive\n");
    r.request("staop 3\n");
    r.request("staop\n");
    r.request("staln 4\n");
    r.request("staln\n");
    r.request("gmotc\n");
    r.request("stdet 255\n");
    r.request("stdet 1\n");
    r.request("stdet\n");
    r.request("gvers\n");
    r.request("ghwin\n");
    r.request("masns\n");
    r.request("eepst\n");
    r.request("stvls 1 a b\n");
    r.request("stvls 1 a\n");
    r.request("gvlvx 12\n");
    r.request("gprof 12\n");
    r.request("gvlvy 12\n");
    r.request("stsnx 0 99\n");
    assert_eq!(
        r.dbg.take_tx(),
        "start new 1-wire search\r\n\
         set valve learning time to 9\r\n\
         set valve learning time to - error\r\n\
         set valve learning movements to 65534\r\n\
         set valve learning movements to - error\r\n\
         get valve learning movements\
         received ESPalive\r\n\
         got open valve request for 3\r\n\
         got open valve request for - error\r\n\
         start learning for valve 4\r\n\
         start learning for valve - error\r\n\
         got get motor characteristics request \r\n\
         got detect valve status request - reset all valves\r\n\
         got detect valve status request - error\r\n\
         got detect valve status request - error\r\n\
         got version request\r\n\
         got hw info request\r\n\
         got match sensors request\r\n\
         got get eeprom status request \r\n\
         comm: set valve sensors\r\n\
         invalid arguments\r\n\
         gvlvx: invalid arguments\r\n\
         gprof: invalid arguments\r\n\
         gvlvy: invalid arguments\r\n\
         comm: set 1st sensor index\r\n\
         invalid arguments\r\n"
    );
    r.request("smotc 17 17 50\n");
    r.request("smotc 1 1\n");
    r.request("stsny 0 0\n");
    r.request("stgtp 1 1\n");
    r.request("gtgtp 1\n");
    assert_eq!(
        r.dbg.take_tx(),
        "got set motor characteristics request - valid\r\n\
         got set motor characteristics request - invalid arguments\r\n\
         comm: set 2nd sensor index\r\n\
         invalid arguments\r\n\
         set target pos\r\n\
         get target pos\r\n"
    );
}

#[test]
fn ghwin_reports_the_device_id_of_the_chip() {
    let mut r = Rig::begin();
    r.system.dev_id = 0x431;
    assert_eq!(r.request("ghwin\n"), "ghwin 1073 \r\n");
}

#[test]
fn a_line_of_too_many_arguments_is_counted_and_the_loop_goes_on() {
    let mut r = Rig::begin();
    r.esp.inject_str("gtgtp 1 2 3 4 5 6\ngtgtp 1\n");
    assert_eq!(r.run(), 0);
    assert_eq!(r.esp.take_tx(), "gtgtp 1 0 \r\n");
    assert_eq!(
        r.dbg.take_tx(),
        "comm: too many arguments\r\nget target pos\r\n"
    );
    r.esp.inject_str("gtgtp 1 2 3 4 5 6\n");
    assert_eq!(r.run(), -1);
    // an empty line is no request
    r.esp.inject_str("   \n");
    assert_eq!(r.run(), -1);
    assert!(r.esp.take_tx().is_empty());
    assert_eq!(r.request("gstat\n"), "gstat 0 0 0 0 2 0\r\n");
}

#[test]
fn an_overlong_line_counts_as_overflow_and_a_partial_line_waits_for_its_end() {
    let mut r = Rig::begin();
    let long = "x".repeat(130) + "\n";
    assert!(r.request(&long).is_empty());
    assert_eq!(r.request("gstat\n"), "gstat 0 0 0 1 0 0\r\n");
    // the bytes of a request that arrive in two runs
    r.esp.inject_str("gtg");
    assert_eq!(r.run(), -1);
    r.board.advance_ms(50);
    assert_eq!(r.request("tp 3\n"), "gtgtp 3 0 \r\n");
    assert_eq!(r.dbg.take_tx(), "get target pos\r\n");
    // a byte waiting in the ring keeps a partial line
    r.esp.inject_str("gtgtp");
    r.run();
    r.board.advance_ms(500);
    let budget = "y".repeat(512);
    r.esp.inject_str(&budget);
    r.esp.inject_str("z");
    r.run();
    assert!(r.dbg.take_tx().is_empty());
    assert_eq!(r.esp.available(), 1);
    // the last byte restarts the idle time; once it is over the partial line is dropped
    r.run();
    assert!(r.dbg.take_tx().is_empty());
    r.board.advance_ms(101);
    r.run();
    assert_eq!(r.dbg.take_tx(), "comm: incomplete line dropped\r\n");
}

#[test]
fn a_request_of_127_characters_is_answered_one_of_128_is_an_overflow() {
    // COMM_LINE_SIZE 128: 127 characters and the terminator
    let mut r = Rig::begin();
    let line = |n: usize| format!("{:<n$}\n", "gtgtp 3");
    assert_eq!(line(127).len(), 128);
    assert_eq!(r.request(&line(127)), "gtgtp 3 0 \r\n");
    assert_eq!(r.request("gstat\n"), "gstat 0 0 0 0 0 0\r\n");
    assert!(r.request(&line(128)).is_empty());
    assert_eq!(r.request("gstat\n"), "gstat 0 0 0 1 0 0\r\n");
}

#[test]
fn smotc_and_scalx_store_only_a_change_slcfg_keeps_an_equal_timeout_unmarked() {
    let mut r = Rig::begin();
    // a value out of range: "smotc err", the values in range applied and stored
    let e = r.exchange("smotc 99 18 40\n");
    assert_eq!(e.reply, "smotc err\r\n");
    assert_eq!(
        e.calls,
        vec![
            "motor_get_params()",
            "motor_set_params(17, 18, 40, 3000, 0)",
            "eeprom_changed(0x0004)"
        ]
    );
    // not a number: nothing changes
    let e = r.exchange("smotc 17 x 40\n");
    assert_eq!(e.reply, "smotc err\r\n");
    assert_eq!(e.calls, vec!["motor_get_params()"]);
    let e = r.exchange("smotc x 17 40\n");
    assert_eq!(e.calls, vec!["motor_get_params()"]);
    let e = r.exchange("smotc 17 17 40 3000 x\n");
    assert_eq!(e.reply, "smotc err\r\n");
    assert_eq!(e.calls, vec!["motor_get_params()"]);
    // the same values: no write
    let e = r.exchange("smotc 17 18 40\n");
    assert_eq!(e.reply, "smotc\r\n");
    assert_eq!(e.calls, vec!["motor_get_params()"]);
    r.stubs.eeprom.state = EEP_STATE_READ_FAILED;
    r.stubs.eep_content.cfg.escalation = EscalationConfig {
        enable: 1,
        step_pct: 30,
        max_ma: 40,
    };
    // scalx compares with the mirror, whatever the EEPROM state
    assert_eq!(
        r.exchange("scalx 1 30 40\n").calls,
        vec!["motor_set_escalation(1, 30, 40)"]
    );
    assert_eq!(r.request("scalx 1 101 40\n"), "scalx err\r\n");
    assert_eq!(r.request("scalx 1 30 19\n"), "scalx err\r\n");
    assert_eq!(r.request("scalx 1 30 61\n"), "scalx err\r\n");
    assert_eq!(r.request("scalx 1 30\n"), "scalx err\r\n");
    assert_eq!(r.request("scalx 1 100 60\n"), "scalx ok\r\n");
    assert_eq!(r.request("scalx 1 0 20\n"), "scalx ok\r\n");
}

#[test]
fn svmov_checks_every_argument_range() {
    let mut r = Rig::begin();
    for (line, reply) in [
        ("svmov 0 0 1 5\n", "svmov 0 ok\r\n"),
        ("svmov 0 1 10000 60\n", "svmov 0 ok\r\n"),
        ("svmov 0 2 100 20\n", "svmov 0 err 1\r\n"),
        ("svmov 0 1 10001 20\n", "svmov 0 err 1\r\n"),
        ("svmov 0 1 100 4\n", "svmov 0 err 1\r\n"),
        ("svmov 0 1 100 61\n", "svmov 0 err 1\r\n"),
        ("svmov 0 1 100\n", "svmov 0 err 1\r\n"),
        ("svmov\n", "svmov -1 err 1\r\n"),
        ("svmov x 1 100 20\n", "svmov -1 err 1\r\n"),
    ] {
        assert_eq!(r.request(line), reply, "{line}");
    }
    assert_eq!(
        r.calls_of("app_service_move"),
        vec![
            "app_service_move(0, 0, 1, 5)",
            "app_service_move(0, 1, 10000, 60)"
        ]
    );
    r.stubs.app.service_move = 1;
    assert_eq!(r.request("svmov 0 1 100 20\n"), "svmov 0 err 2\r\n");
    assert_eq!(SVMOV_COUNTS_MIN, 1);
    assert_eq!(SVMOV_COUNTS_MAX, 10000);
    assert_eq!(SVMOV_MAXMA_MIN, 5);
    assert_eq!(SVMOV_MAXMA_MAX, 60);
}

#[test]
fn gprof_sends_the_profile_of_the_valve() {
    let mut r = Rig::begin();
    r.stubs.motor.profile.add(0, 0);
    r.stubs.motor.profile.finish(5, 120);
    let e = r.exchange("gprof 7\n");
    assert_eq!(e.reply, "gprof 7 2 0:0 5:120\r\n");
    assert_eq!(e.calls, vec!["valve_get_profile(7)"]);
}

#[test]
fn the_rx_interrupt_counts_every_error_kind() {
    let mut c = UartErrorCounters::default();
    comm_rx_irq(&mut c, 0x0F, true);
    assert_eq!(
        c,
        UartErrorCounters {
            overrun: 1,
            framing: 1,
            noise: 1,
            dropped: 1
        }
    );
    comm_rx_irq(&mut c, 0, false);
    assert_eq!(c.dropped, 1);
    assert_eq!(c.overrun, 1);
}

#[test]
fn the_reply_buffer_holds_the_longest_gprof() {
    // C++ replyLine: kProfileReplyMaxLen + 1 bytes, the NUL included
    let r = Rig::begin();
    assert_eq!(r.comm.reply_line.capacity(), PROFILE_REPLY_MAX_LEN + 1);
    assert_eq!(PROFILE_REPLY_MAX_LEN, 396);
}

#[test]
fn stsnx_and_stsny_refuse_every_index_past_the_table_without_a_call() {
    let mut r = Rig::begin();
    r.stubs.sensors.no_of_ds18_devices = 40;
    for line in [
        "stsnx 0 34\n",
        "stsny 0 35\n",
        "stsnx 0 65535\n",
        "stsnx 0 65536\n",
    ] {
        let e = r.exchange(line);
        assert!(e.reply.is_empty(), "{line}");
        assert!(e.calls.is_empty(), "{line}");
    }
    assert_eq!(r.stubs.valves[0], ValveGlobals::default());
    assert_eq!(r.request("stsny 0 33\n"), "stsny\r\n");
    assert_eq!(r.stubs.valves[0].sensorindex2, 33);
}
