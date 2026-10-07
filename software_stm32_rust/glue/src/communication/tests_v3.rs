// Port of test/native/glue/test_communication_v3.cpp, protocol 3: the commands of contracts.md
// 1.1 against the stub values of app/eeprom/sysstat/owDevices (every argument boundary, the err
// forms, the gvlvy/gstax goldens), the EEPROM marks only on a change (S13), the lease poll of
// gvlvd/gvlvx, the USART1 error counters (S6), stdet (S15) and the board revision of gvers (W8).

use super::tests::Rig;
use super::*;
use crate::test_support::onewire_sim::SimDevice;
use vdm_stm_core::move_classifier::MoveResult;
use vdm_stm_core::replies_v2::{
    EEP_STATE_OK, EEP_STATE_PENDING, EEP_STATE_READ_FAILED, EEP_STATE_WRITE_FAILED,
};
use vdm_stm_core::uart_errors::{
    UART_ERROR_FRAMING, UART_ERROR_NOISE, UART_ERROR_OVERRUN, UART_ERROR_PARITY,
};

fn golden_valve_3(r: &mut Rig) {
    let s = &mut r.stubs.motor.snapshot[3];
    s.status = 9;
    s.actual_position = 50;
    s.target_position = 30;
    s.meancurrent = 17;
    s.opening_count = 3567;
    s.closing_count = 3610;
    s.deadzone_count = 43;
    s.calib_retries = 2;
    s.movements = 0;
    s.calibration = false;
    s.calib_active = false;
    s.diag.early_warn = false;
    s.diag.last_cal_failed = true;
    s.diag.early_stops = 1;
    s.diag.last = MoveResult {
        dir: 0,
        requested_counts: 1750,
        counted_counts: 1750,
        stop_reason: 1,
        peak_current: 262,
        duration_ms: 6120,
    };
    r.stubs.valves[3].cmd_rejected = 4;
    r.stubs.app.valve_v3 = ValveV3Info {
        flags: 66,
        fault: 4,
        fs_pct: 50,
        drive: 50,
        retry_s: 3540,
        retries: 0,
    };
}

fn calls(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

#[test]
fn gproto_answers_3_gvers_carries_the_board_revision() {
    let mut r = Rig::begin();
    assert_eq!(r.request("gproto\n"), "gproto 3\r\n");
    assert_eq!(r.request("gvers\n"), "gvers 2.1.7-revamped_C2 1 \r\n");
    // C++ kHardwareMarker == "VDM-HW:C2": the marker is in the ID block (vdm-stm-boot id_block)
}

#[test]
fn stdet_255_tests_every_valve_another_number_is_an_error_no_number_no_reply() {
    let mut r = Rig::begin();
    let e = r.exchange("stdet 255\n");
    assert_eq!(e.reply, "stdet \r\n");
    assert_eq!(e.calls, vec!["app_scan_valves()"]);
    let e = r.exchange("stdet 3\n");
    assert_eq!(e.reply, "stdet err\r\n");
    assert!(e.calls.is_empty());
    assert_eq!(r.exchange("stdet 254\n").reply, "stdet err\r\n");
    assert_eq!(r.exchange("stdet 256\n").reply, "stdet err\r\n");
    let e = r.exchange("stdet x\n");
    assert!(e.reply.is_empty());
    assert!(e.calls.is_empty());
    assert!(r.exchange("stdet\n").reply.is_empty());
    assert!(r.exchange("stdet 255 1\n").reply.is_empty());
}

#[test]
fn gvlvd_and_gvlvx_renew_the_lease_through_app_lease_poll_gvlvy_and_the_rest_do_not() {
    let mut r = Rig::begin();
    assert_eq!(r.exchange("gvlvd 0\n").calls[0], "app_lease_poll()");
    assert_eq!(r.exchange("gvlvx 0\n").calls[0], "app_lease_poll()");
    assert_eq!(r.exchange("gvlvd 12\n").calls, vec!["app_lease_poll()"]);
    assert_eq!(r.calls_of("app_lease_poll").len(), 1);
    let e = r.exchange("gvlvy 0\n");
    assert!(!e.calls.contains(&"app_lease_poll()".to_string()));
    assert!(r.exchange("gtgtp 0\n").calls.is_empty());
}

#[test]
fn slhbt_0_and_1_reach_the_lease_the_reply_carries_its_state_and_remaining_time() {
    let mut r = Rig::begin();
    r.stubs.app.lease_state = 1;
    r.stubs.app.lease_remaining_s = 3540;
    let e = r.exchange("slhbt 1\n");
    assert_eq!(e.reply, "slhbt 1 3540\r\n");
    assert_eq!(
        e.calls,
        calls(&[
            "app_lease_heartbeat(1)",
            "app_lease_state()",
            "app_lease_remaining_s()"
        ])
    );
    r.stubs.app.lease_state = 2;
    r.stubs.app.lease_remaining_s = 0;
    let e = r.exchange("slhbt 0\n");
    assert_eq!(e.reply, "slhbt 2 0\r\n");
    assert_eq!(e.calls[0], "app_lease_heartbeat(0)");
    for bad in [
        "slhbt 2\n",
        "slhbt\n",
        "slhbt 1 1\n",
        "slhbt x\n",
        "slhbt -1\n",
    ] {
        let e = r.exchange(bad);
        assert_eq!(e.reply, "slhbt err\r\n", "{bad}");
        assert!(e.calls.is_empty(), "{bad}");
    }
}

#[test]
fn slcfg_0_and_5_to_1440_are_stored_and_configure_the_lease_the_rest_is_an_error() {
    let mut r = Rig::begin();
    r.stubs.eep_content.cfg.lease_timeout_min = 60;
    let e = r.exchange("slcfg 60\n");
    assert_eq!(e.reply, "slcfg ok\r\n");
    assert_eq!(
        e.calls,
        calls(&[
            "eeprom_state()",
            "app_lease_configure(60)",
            "app_lease_command()"
        ])
    );
    for m in [0u32, 5, 1440] {
        let e = r.exchange(&format!("slcfg {m}\n"));
        assert_eq!(e.reply, "slcfg ok\r\n", "{m}");
        assert_eq!(u32::from(r.stubs.eep_content.cfg.lease_timeout_min), m);
        assert_eq!(
            e.calls,
            vec![
                "eeprom_changed(0x0020)".to_string(),
                format!("app_lease_configure({m})"),
                "app_lease_command()".to_string()
            ]
        );
    }
    for bad in [
        "slcfg 4\n",
        "slcfg 1441\n",
        "slcfg 65596\n",
        "slcfg\n",
        "slcfg 5 5\n",
        "slcfg x\n",
    ] {
        let e = r.exchange(bad);
        assert_eq!(e.reply, "slcfg err\r\n", "{bad}");
        assert!(e.calls.is_empty(), "{bad}");
    }
    assert_eq!(r.stubs.eep_content.cfg.lease_timeout_min, 1440);
}

#[test]
fn sfspo_one_valve_or_all_0_to_100_or_255_stored_only_when_it_changes() {
    let mut r = Rig::begin();
    r.stubs.eep_content.cfg.failsafe_pct = [50; 12];
    let e = r.exchange("sfspo 3 40\n");
    assert_eq!(e.reply, "sfspo 3 ok\r\n");
    assert_eq!(
        e.calls,
        calls(&[
            "eeprom_changed(0x0040)",
            "app_set_failsafe(3, 40)",
            "app_lease_command()"
        ])
    );
    let fs = r.stubs.eep_content.cfg.failsafe_pct;
    assert_eq!((fs[3], fs[2], fs[4]), (40, 50, 50));
    let e = r.exchange("sfspo 3 40\n");
    assert_eq!(e.reply, "sfspo 3 ok\r\n");
    assert_eq!(
        e.calls,
        calls(&[
            "eeprom_state()",
            "app_set_failsafe(3, 40)",
            "app_lease_command()"
        ])
    );
    assert_eq!(r.exchange("sfspo 0 0\n").reply, "sfspo 0 ok\r\n");
    assert_eq!(r.exchange("sfspo 11 100\n").reply, "sfspo 11 ok\r\n");
    assert_eq!(r.exchange("sfspo 5 255\n").reply, "sfspo 5 ok\r\n");
    let fs = r.stubs.eep_content.cfg.failsafe_pct;
    assert_eq!((fs[0], fs[11], fs[5]), (0, 100, 255));
    let e = r.exchange("sfspo 255 40\n");
    assert_eq!(e.reply, "sfspo 255 ok\r\n");
    assert_eq!(
        e.calls,
        calls(&[
            "eeprom_changed(0x0040)",
            "app_set_failsafe(255, 40)",
            "app_lease_command()"
        ])
    );
    assert_eq!(r.stubs.eep_content.cfg.failsafe_pct, [40; 12]);
    assert_eq!(
        r.exchange("sfspo 255 40\n").calls,
        calls(&[
            "eeprom_state()",
            "app_set_failsafe(255, 40)",
            "app_lease_command()"
        ])
    );
    // only one valve differs: still one mark
    r.stubs.eep_content.cfg.failsafe_pct[7] = 41;
    assert_eq!(
        r.exchange("sfspo 255 40\n").calls[0],
        "eeprom_changed(0x0040)"
    );
    assert_eq!(r.stubs.eep_content.cfg.failsafe_pct[7], 40);
    for bad in [
        "sfspo 3 101\n",
        "sfspo 3 254\n",
        "sfspo 3\n",
        "sfspo 3 40 1\n",
        "sfspo 3 x\n",
    ] {
        let e = r.exchange(bad);
        assert_eq!(e.reply, "sfspo 3 err 1\r\n", "{bad}");
        assert!(e.calls.is_empty(), "{bad}");
    }
    for bad in [
        "sfspo 12 1\n",
        "sfspo 254 1\n",
        "sfspo\n",
        "sfspo x 1\n",
        "sfspo 256 1\n",
    ] {
        let e = r.exchange(bad);
        assert_eq!(e.reply, "sfspo -1 err 1\r\n", "{bad}");
        assert!(e.calls.is_empty(), "{bad}");
    }
    assert_eq!(r.exchange("sfspo 255 101\n").reply, "sfspo 255 err 1\r\n");
    assert_eq!(r.stubs.eep_content.cfg.failsafe_pct[3], 40);
}

#[test]
fn slcfg_and_sfspo_while_the_eeprom_read_has_failed_a_value_equal_to_the_mirror_is_marked_too() {
    let mut r = Rig::begin();
    // the defaults of a failed read in the mirror
    r.stubs.eep_content.cfg.lease_timeout_min = 60;
    r.stubs.eep_content.cfg.failsafe_pct = [50; 12];
    r.stubs.eeprom.state = EEP_STATE_READ_FAILED;
    let e = r.exchange("slcfg 60\n");
    assert_eq!(e.reply, "slcfg ok\r\n");
    assert_eq!(
        e.calls,
        calls(&[
            "eeprom_state()",
            "eeprom_changed(0x0020)",
            "app_lease_configure(60)",
            "app_lease_command()"
        ])
    );
    let e = r.exchange("sfspo 3 50\n");
    assert_eq!(e.reply, "sfspo 3 ok\r\n");
    assert_eq!(
        e.calls,
        calls(&[
            "eeprom_state()",
            "eeprom_changed(0x0040)",
            "app_set_failsafe(3, 50)",
            "app_lease_command()"
        ])
    );
    assert_eq!(
        r.exchange("sfspo 255 50\n").calls,
        calls(&[
            "eeprom_state()",
            "eeprom_changed(0x0040)",
            "app_set_failsafe(255, 50)",
            "app_lease_command()"
        ])
    );
    // a changed value is marked without asking
    assert_eq!(
        r.exchange("slcfg 90\n").calls,
        calls(&[
            "eeprom_changed(0x0020)",
            "app_lease_configure(90)",
            "app_lease_command()"
        ])
    );
    assert_eq!(r.stubs.eep_content.cfg.lease_timeout_min, 90);
    // an invalid request marks nothing
    assert!(r.exchange("slcfg 4\n").calls.is_empty());
    assert!(r.exchange("sfspo 3 101\n").calls.is_empty());
    // readable EEPROM: an equal value is not marked, in every other state
    for state in [EEP_STATE_OK, EEP_STATE_PENDING, EEP_STATE_WRITE_FAILED] {
        r.stubs.eeprom.state = state;
        assert_eq!(
            r.exchange("slcfg 90\n").calls,
            calls(&[
                "eeprom_state()",
                "app_lease_configure(90)",
                "app_lease_command()"
            ]),
            "{state}"
        );
        assert_eq!(
            r.exchange("sfspo 3 50\n").calls,
            calls(&[
                "eeprom_state()",
                "app_set_failsafe(3, 50)",
                "app_lease_command()"
            ]),
            "{state}"
        );
    }
}

#[test]
fn glcfg_the_lease_timeout_and_the_failsafe_positions_of_the_valves_extra_arguments_ignored() {
    let mut r = Rig::begin();
    r.stubs.app.lease_timeout = 60;
    r.stubs.app.failsafe_pct = 40;
    let e = r.exchange("glcfg\n");
    assert_eq!(e.reply, "glcfg 60 40 40 40 40 40 40 40 40 40 40 40 40\r\n");
    assert_eq!(e.calls.len(), 14);
    assert_eq!(e.calls[0], "app_lease_command()");
    assert_eq!(e.calls[1], "app_failsafe_pct(0)");
    assert_eq!(e.calls[12], "app_failsafe_pct(11)");
    assert_eq!(e.calls[13], "app_lease_timeout()");
    assert_eq!(
        r.exchange("glcfg 1 2\n").reply,
        "glcfg 60 40 40 40 40 40 40 40 40 40 40 40 40\r\n"
    );
}

#[test]
fn gvlvy_the_golden_of_contracts_1_2_gvlvx_the_same_first_values_none_for_a_bad_index() {
    let mut r = Rig::begin();
    golden_valve_3(&mut r);
    let e = r.exchange("gvlvy 3\n");
    assert_eq!(
        e.reply,
        "gvlvy 3 9 50 30 17 3567 3610 43 2 0 8 1 4 0 1750 1750 1 262 6120 66 4 50 50 3540 0\r\n"
    );
    assert_eq!(
        e.calls,
        calls(&[
            "valve_get_snapshot(3)",
            "app_learn_pending(3, 9, 0)",
            "app_get_valve_v3(3)"
        ])
    );
    assert_eq!(
        r.request("gvlvx 3\n"),
        "gvlvx 3 9 50 30 17 3567 3610 43 2 0 8 1 4 0 1750 1750 1 262 6120\r\n"
    );
    r.stubs.app.valve_v3 = ValveV3Info {
        flags: 1023,
        fault: 5,
        fs_pct: 255,
        drive: 100,
        retry_s: 86400,
        retries: 255,
    };
    assert_eq!(
        r.request("gvlvy 3\n"),
        "gvlvy 3 9 50 30 17 3567 3610 43 2 0 8 1 4 0 1750 1750 1 262 6120 1023 5 255 100 86400 255\r\n"
    );
    assert!(r.request("gvlvy 11\n").starts_with("gvlvy 11 "));
    for bad in ["gvlvy 12\n", "gvlvy\n", "gvlvy 1 2\n", "gvlvy x\n"] {
        let e = r.exchange(bad);
        assert!(e.reply.is_empty(), "{bad}");
        assert!(e.calls.is_empty(), "{bad}");
    }
}

#[test]
fn gstax_the_golden_of_contracts_1_3_gstat_the_same_first_values() {
    let mut r = Rig::begin();
    r.stubs.sysstat.uptime = 86400;
    r.stubs.sysstat.resets = 3;
    r.stubs.sysstat.reason = BootReason::Pin;
    r.stubs.app.lease_state = 1;
    r.stubs.app.lease_remaining_s = 3540;
    r.stubs.app.lease_client = true;
    r.stubs.app.lease_timeout = 60;
    r.stubs.eeprom.writes = 12;
    r.stubs.app.temp_age_s = 2;
    r.stubs.ow.scan_age_s = 3600;
    // two malformed lines
    r.request_bytes(b"\x01\n");
    r.request_bytes(b"\x02\n");
    assert_eq!(
        r.request("gstax\n"),
        "gstax 86400 3 2 0 2 0 1 3540 1 60 0 0 0 0 0 0 0 0 0 12 2 3600 0\r\n"
    );
    assert_eq!(
        r.request("gstax 1 2\n"),
        "gstax 86400 3 2 0 2 0 1 3540 1 60 0 0 0 0 0 0 0 0 0 12 2 3600 0\r\n"
    );
    assert_eq!(r.request("gstat\n"), "gstat 86400 3 2 0 2 0\r\n");
}

#[test]
fn gstax_every_field_from_its_source() {
    let mut r = Rig::begin();
    r.stubs.sysstat.uptime = 1;
    r.stubs.sysstat.resets = 2;
    r.stubs.sysstat.reason = BootReason::IndependentWatchdog;
    r.stubs.eeprom.state = EEP_STATE_WRITE_FAILED;
    r.stubs.app.lease_state = 2;
    r.stubs.app.lease_remaining_s = 7;
    r.stubs.app.lease_client = false;
    r.stubs.app.lease_timeout = 1440;
    r.stubs.app.failsafe_mask = 0x0A5;
    r.stubs.sysstat.safe_mode = true;
    r.stubs.sysstat.wdg_resets = 3;
    r.stubs.eeprom.cfg_flags = 0x81;
    r.stubs.eeprom.cfg_events = 5;
    r.stubs.eeprom.writes = 4_294_967_295;
    r.stubs.app.temp_age_s = 13;
    r.stubs.ow.scan_age_s = 86401;
    r.stubs.app.protect_suspended = true;
    // USART1 errors: overrun, framing, noise and parity (both count as noise), and a full ring
    r.esp.next_error = UART_ERROR_OVERRUN;
    r.esp.inject(b"a");
    r.esp.next_error = UART_ERROR_FRAMING | UART_ERROR_PARITY;
    r.esp.inject(b"b");
    r.esp.next_error = UART_ERROR_NOISE;
    r.esp.inject(b"c");
    r.esp.inject(b"\n");
    r.run();
    // the ring holds 1023
    r.esp.inject(&[b'\n'; SERIAL_RX_BUFFER_SIZE as usize + 1]);
    // empty lines: 512 bytes per call (bounded, so a loop that stops reading fails here)
    for _ in 0..2 {
        r.run();
    }
    assert_eq!(r.esp.available(), 0);
    let reply = r.request("gstax\n");
    assert_eq!(
        reply,
        "gstax 1 2 4 0 0 2 2 7 0 1440 165 1 3 1 1 2 2 129 5 4294967295 13 86401 1\r\n"
    );
    r.stubs.app.protect_suspended = false;
    r.stubs.sysstat.safe_mode = false;
    r.stubs.app.lease_client = true;
    assert_eq!(
        r.request("gstax\n"),
        "gstax 1 2 4 0 0 2 2 7 1 1440 165 0 3 1 1 2 2 129 5 4294967295 13 86401 0\r\n"
    );
}

#[test]
fn the_usart1_rx_path_counts_errors_and_still_stores_the_bytes() {
    // C++ also checks that communication_setup() installs its RX callback with interrupts off:
    // the Rust firmware calls comm_rx_irq from its USART1 interrupt
    let mut r = Rig::new();
    r.setup();
    r.dbg.take_tx();
    r.esp.next_error = UART_ERROR_OVERRUN;
    assert_eq!(r.request("gproto\n"), "gproto 3\r\n");
    assert_eq!(
        r.request("gstax\n"),
        "gstax 0 0 0 0 0 0 0 0 0 0 0 0 0 1 0 0 0 0 0 0 0 0 0\r\n"
    );
}

#[test]
fn sstop_one_valve_or_all_through_app_stop_an_invalid_index_or_a_refusal_is_an_error() {
    let mut r = Rig::begin();
    let e = r.exchange("sstop 3\n");
    assert_eq!(e.reply, "sstop 3 ok\r\n");
    assert_eq!(e.calls, vec!["app_stop(3)"]);
    assert_eq!(r.exchange("sstop 0\n").reply, "sstop 0 ok\r\n");
    assert_eq!(r.exchange("sstop 11\n").reply, "sstop 11 ok\r\n");
    let e = r.exchange("sstop 255\n");
    assert_eq!(e.reply, "sstop 255 ok\r\n");
    assert_eq!(e.calls, vec!["app_stop(255)"]);
    for bad in [
        "sstop 12\n",
        "sstop 20\n",
        "sstop 254\n",
        "sstop\n",
        "sstop 1 2\n",
        "sstop x\n",
    ] {
        let e = r.exchange(bad);
        assert_eq!(e.reply, "sstop -1 err 1\r\n", "{bad}");
        assert!(e.calls.is_empty(), "{bad}");
    }
    r.stubs.app.stop = -1;
    assert_eq!(r.exchange("sstop 3\n").reply, "sstop -1 err 1\r\n");
}

#[test]
fn gtlnt_the_stored_learn_time_extra_arguments_ignored() {
    let mut r = Rig::begin();
    r.stubs.app.learn_time = 604800;
    let e = r.exchange("gtlnt\n");
    assert_eq!(e.reply, "gtlnt 604800\r\n");
    assert_eq!(e.calls, vec!["app_get_learntime()"]);
    r.stubs.app.learn_time = 0;
    assert_eq!(r.request("gtlnt 5\n"), "gtlnt 0\r\n");
}

#[test]
fn ssafe_only_0_leaves_safe_mode() {
    let mut r = Rig::begin();
    let e = r.exchange("ssafe 0\n");
    assert_eq!(e.reply, "ssafe ok\r\n");
    assert_eq!(e.calls, vec!["sysstat_leave_safe_mode()"]);
    for bad in ["ssafe 1\n", "ssafe\n", "ssafe 0 0\n", "ssafe x\n"] {
        let e = r.exchange(bad);
        assert_eq!(e.reply, "ssafe err\r\n", "{bad}");
        assert!(e.calls.is_empty(), "{bad}");
    }
}

#[test]
fn stlnt_the_learn_time_goes_to_the_app_and_is_stored_only_when_it_changes() {
    let mut r = Rig::begin();
    r.stubs.eep_content.cfg.learn_time_s = 604800;
    let e = r.exchange("stlnt 3600\n");
    assert_eq!(e.reply, "stlnt\r\n");
    assert_eq!(
        e.calls,
        calls(&["app_set_learntime(3600)", "eeprom_changed(0x0010)"])
    );
    assert_eq!(r.stubs.eep_content.cfg.learn_time_s, 3600);
    let e = r.exchange("stlnt 3600\n");
    assert_eq!(e.reply, "stlnt\r\n");
    assert_eq!(e.calls, vec!["app_set_learntime(3600)"]);
    assert_eq!(comm_set_learntime(&mut r.stubs, 0), 0);
    assert_eq!(r.stubs.eep_content.cfg.learn_time_s, 0);
    r.stubs.app.set_learn_time = -1;
    let e = r.exchange("stlnt 7\n");
    assert!(e.reply.is_empty());
    assert_eq!(e.calls, vec!["app_set_learntime(7)"]);
    assert_eq!(comm_set_learntime(&mut r.stubs, 8), -1);
    assert_eq!(r.stubs.eep_content.cfg.learn_time_s, 0);
}

#[test]
fn stlnm_every_request_resets_the_counters_the_eeprom_is_marked_only_for_a_new_value() {
    let mut r = Rig::begin();
    r.stubs.eep_content.cfg.layout.number_of_movements = 2000;
    let e = r.exchange("stlnm 2000\n");
    assert_eq!(e.reply, "stlnm\r\n");
    assert_eq!(e.calls, vec!["app_set_learnmovements(2000)"]);
    let e = r.exchange("stlnm 50\n");
    assert_eq!(e.reply, "stlnm\r\n");
    assert_eq!(
        e.calls,
        calls(&["app_set_learnmovements(50)", "eeprom_changed(0x0002)"])
    );
    assert_eq!(r.stubs.eep_content.cfg.layout.number_of_movements, 50);
}

#[test]
fn scalx_the_escalation_is_stored_only_when_it_changes() {
    let mut r = Rig::begin();
    let esc = |enable, step_pct, max_ma| EscalationConfig {
        enable,
        step_pct,
        max_ma,
    };
    r.stubs.eep_content.cfg.escalation = esc(1, 30, 40);
    let e = r.exchange("scalx 1 30 40\n");
    assert_eq!(e.reply, "scalx ok\r\n");
    assert_eq!(e.calls, vec!["motor_set_escalation(1, 30, 40)"]);
    r.stubs.eep_content.cfg.escalation = esc(1, 30, 41);
    assert_eq!(
        r.exchange("scalx 1 30 40\n").calls,
        calls(&["motor_set_escalation(1, 30, 40)", "eeprom_changed(0x0008)"])
    );
    r.stubs.eep_content.cfg.escalation = esc(1, 31, 40);
    assert_eq!(
        r.exchange("scalx 1 30 40\n").calls.last().unwrap(),
        "eeprom_changed(0x0008)"
    );
    r.stubs.eep_content.cfg.escalation = esc(0, 30, 40);
    assert_eq!(
        r.exchange("scalx 1 30 40\n").calls.last().unwrap(),
        "eeprom_changed(0x0008)"
    );
}

#[test]
fn stsnx_stsny_the_slot_of_the_valve_is_marked_only_when_the_address_changes() {
    let mut r = Rig::begin();
    r.stubs.sensors.tempsensors[4].address = [0x28, 1, 2, 3, 4, 5, 6, 0];
    r.stubs.sensors.no_of_ds18_devices = 5;
    let e = r.exchange("stsnx 2 4\n");
    assert_eq!(e.reply, "stsnx\r\n");
    assert_eq!(e.calls, vec!["eeprom_changed_slot(2)"]);
    let s = r.stubs.eep_content.cfg.layout.owsensors1[2];
    assert_eq!(s.familycode, 0x28);
    assert_eq!(s.romcode[0], 1);
    assert_eq!(s.romcode[5], 6);
    assert_eq!(r.stubs.valves[2].sensorindex1, 4);
    assert!(r.exchange("stsnx 2 4\n").calls.is_empty());
    let e = r.exchange("stsny 2 4\n");
    assert_eq!(e.reply, "stsny\r\n");
    assert_eq!(e.calls, vec!["eeprom_changed_slot(14)"]);
    assert_eq!(r.stubs.valves[2].sensorindex2, 4);
    assert_eq!(
        r.exchange("stsny 11 4\n").calls,
        vec!["eeprom_changed_slot(23)"]
    );
    // no sensor 5
    assert!(r.exchange("stsny 11 5\n").reply.is_empty());
    // one byte differs: stored again
    r.stubs.eep_content.cfg.layout.owsensors1[2].crc = 1;
    assert_eq!(
        r.exchange("stsnx 2 4\n").calls,
        vec!["eeprom_changed_slot(2)"]
    );
    assert_eq!(r.stubs.eep_content.cfg.layout.owsensors1[2].crc, 0);
}

#[test]
fn stvls_both_slots_of_the_valve_each_marked_only_when_it_changes() {
    let mut r = Rig::begin();
    // 28-07-00-00-00-00-00-crc with a valid CRC (C++: found by the fake DallasTemperature)
    let rom = SimDevice::new(0x28, 7, crate::test_support::onewire_sim::Kind::Ds18).rom;
    let text = rom
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join("-");
    // never assigned
    let slot = &mut r.stubs.eep_content.cfg.layout.owsensors2[3];
    slot.familycode = 0xFF;
    slot.romcode = [0xFF; 6];
    slot.crc = 0xFF;
    let e = r.exchange(&format!("stvls 3 {text} 00-00-00-00-00-00-00-00\n"));
    assert_eq!(e.reply, "stvls 3\r\n");
    assert_eq!(
        e.calls,
        calls(&["eeprom_changed_slot(3)", "eeprom_changed_slot(15)"])
    );
    let layout = r.stubs.eep_content.cfg.layout;
    assert_eq!(layout.owsensors1[3].familycode, 0x28);
    assert_eq!(layout.owsensors1[3].crc, rom[7]);
    assert_eq!(layout.owsensors2[3].familycode, 0);
    let e = r.exchange(&format!("stvls 3 {text} 00-00-00-00-00-00-00-00\n"));
    assert_eq!(e.reply, "stvls 3\r\n");
    assert!(e.calls.is_empty());
    // an invalid address changes nothing
    assert!(r
        .exchange("stvls 3 28-00-00-00-00-00-00-01 zz\n")
        .calls
        .is_empty());
    assert!(r
        .exchange(&format!("stvls 12 {text} {text}\n"))
        .reply
        .is_empty());
}

#[test]
fn comm_set_valve_sensor_index_invalid_valve_slot_or_sensor_changes_nothing() {
    let mut r = Rig::begin();
    r.stubs.sensors.no_of_ds18_devices = 2;
    assert_eq!(comm_set_valve_sensor_index(&mut r.stubs, 12, 1, 0), -1);
    assert_eq!(comm_set_valve_sensor_index(&mut r.stubs, 0, 1, 2), -1);
    assert_eq!(comm_set_valve_sensor_index(&mut r.stubs, 0, 0, 1), -1);
    assert_eq!(comm_set_valve_sensor_index(&mut r.stubs, 0, 3, 1), -1);
    assert!(r.calls().is_empty());
    r.stubs.sensors.no_of_ds18_devices = MAXONEWIRECNT as u8 + 1;
    assert_eq!(
        comm_set_valve_sensor_index(&mut r.stubs, 0, 1, MAXONEWIRECNT as u16),
        -1
    );
    assert_eq!(
        comm_set_valve_sensors(
            &mut r.stubs,
            12,
            b"00-00-00-00-00-00-00-00",
            b"00-00-00-00-00-00-00-00"
        ),
        -1
    );
    assert!(r.calls().is_empty());
}
