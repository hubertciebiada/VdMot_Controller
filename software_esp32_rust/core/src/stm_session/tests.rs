//! Port of test/native/test_stm_session.cpp: StmSession end to end against a scripted STM (line
//! level) and a fake port: start-up per protocol, unsupported STMs, lease and failsafe
//! emulation, assembly, restored targets, scheduled calibration results, EEPROM-safe resets and
//! restarts, link recovery, flashing, protocol 3 commands.

use super::*;
use crate::config::MqttMode;
use crate::failsafe::{HaStatus, LeaseMode, LeaseState};
use crate::test_support::session_rig::{cmd, cmd0, name, Rig};
use crate::valve_model::{TargetSource, TargetSync};
use std::cell::Cell;
use std::format;
use std::rc::Rc;
use std::string::String;
use std::vec::Vec;

fn text(e: &Event) -> String {
    String::from_utf8(e.text.to_vec()).expect("ASCII")
}

/// Position of the first line that `pred` accepts.
fn position(lines: &[String], pred: impl Fn(&str) -> bool) -> Option<usize> {
    lines.iter().position(|l| pred(l))
}

// ================================================================ start-up

#[test]
fn protocol_3_start_up_gproto_and_gvers_first_then_polls_lease_and_learn_time() {
    let mut r = Rig::new(3, 0);
    r.cfg.calib.day_mask = 0;
    r.start();
    r.run(10000);
    let lines = &r.stm().lines;
    assert!(lines.len() >= 3);
    assert_eq!(lines[0], "gproto");
    assert_eq!(lines[1], "gvers");
    let last = &r.port().last;
    assert_eq!(last.link, LinkState::Up);
    assert_eq!(last.proto, 3);
    assert_eq!(last.support, StmSupport::Supported);
    assert!(r.count("gvlvy 0") >= 1);
    assert!(r.count("gstax") >= 1);
    assert_eq!(r.count("slhbt 1"), 1); // MQTT off: the regulator is alive
    assert_eq!(r.count("glcfg"), 2); // read, push, verify
    assert_eq!(r.stm().lines_of("sfspo"), ["sfspo 255 255"]); // no valve active
    assert_eq!(last.lease.mode, LeaseMode::Stm);
    assert!(last.lease.config_synced);
    assert_eq!(r.count("gtlnt"), 1); // schedule off, the STM holds the default already
    assert!(last.have_learn_time);
    assert_eq!(last.learn_time_s, 604800);
    assert_eq!(r.count("stgtp"), 0);
    assert!(!r.port().has(EventCode::StmRebootDetected));
    assert!(r.port().lease_records >= 9);
    assert!(!r.port().lease.lost);
}

#[test]
fn protocol_1_start_up_polls_gvlvd_and_gtgtp_no_protocol_3_command() {
    let mut r = Rig::new(1, 0x00F);
    r.start();
    r.run(15000);
    assert_eq!(r.port().last.link, LinkState::Up);
    assert_eq!(r.port().last.proto, 1);
    assert!(r.count("gvlvd 0") >= 1);
    assert!(r.count("gtgtp 0") >= 1);
    assert_eq!(r.count("slhbt"), 0);
    assert_eq!(r.count("glcfg"), 0);
    assert_eq!(r.count("gtlnt"), 0);
    assert_eq!(r.count("gstat"), 0);
    assert_eq!(r.port().last.lease.mode, LeaseMode::Emulated);
}

#[test]
fn the_first_snapshot_waits_for_a_change_and_100_ms_between_snapshots() {
    let mut r = Rig::new(3, 0x00F);
    r.start();
    r.run(4);
    assert_eq!(r.port().publishes, 1);
    let rev = r.port().last.revision;
    r.run(50);
    assert_eq!(r.port().publishes, 1); // within 100 ms
    r.run(20000);
    assert!(r.port().last.revision > rev);
    assert!(r.port().last.taken_ms <= r.now);
}

// ================================================================ unsupported STM (W13)

#[test]
fn an_stm_below_1_4_0_gets_gvers_every_30_s_and_targets_only() {
    let mut r = Rig::new(1, 0x00F);
    r.stm_mut().too_old = true;
    r.start();
    r.run(10000);
    assert_eq!(r.port().last.support, StmSupport::TooOld);
    assert_eq!(r.count("gproto"), 3); // one probe, three attempts
    let gvers = r.count("gvers");
    assert_eq!(r.count("gvlvd"), 0);
    assert_eq!(r.count("gowvc"), 0);
    assert_eq!(r.count("ghwin"), 0);
    let t = r.target(1, 33, TargetSource::Web);
    r.command(&t);
    r.run(2000);
    assert_eq!(r.stm().lines_of("stgtp"), ["stgtp 1 33"]);
    assert!(r.count("gtgtp 1") >= 1);
    assert_eq!(r.port().last.valves[1].sync, TargetSync::Synced);
    // Other STM commands are dropped; a scheduled calibration reports it.
    let mut c = cmd(StmCommandType::Calibrate, ALL_VALVES);
    c.scheduled = true;
    c.attempt = 7;
    r.command(&c);
    r.command(&cmd0(StmCommandType::Detect));
    r.command(&cmd(StmCommandType::Assembly, 2));
    r.run(1000);
    assert_eq!(r.count("staln"), 0);
    assert_eq!(r.count("stdet"), 0);
    assert_eq!(r.count("staop"), 0);
    let calibs = &r.port().calibs;
    assert_eq!(calibs.len(), 1);
    assert_eq!(calibs[0].attempt, 7);
    assert!(!calibs[0].ok);
    assert_eq!(calibs[0].reason, CalibFailure::Unsupported);
    r.run(10 * 60000);
    assert!(r.count("gvers") >= gvers + 20);
    assert!(!r.port().has(EventCode::LinkDegraded));
    assert_eq!(r.port().with_code(EventCode::StmIncompatible).len(), 1);
    assert!(!r.port().last.valves[0].known);
}

#[test]
fn flashing_a_supported_firmware_after_an_unsupported_one_re_syncs() {
    let mut r = Rig::new(3, 0x00F);
    r.stm_mut().too_old = true;
    r.start();
    r.run(8000);
    assert_eq!(r.port().last.support, StmSupport::TooOld);
    r.stm_mut().too_old = false;
    r.run(31000);
    assert_eq!(r.port().last.support, StmSupport::Supported);
    assert_eq!(r.port().last.proto, 3);
    assert_eq!(r.count("gproto"), 4); // three unanswered attempts, then the answered one
}

// ================================================================ lease and failsafe (K1)

#[test]
fn protocol_3_heartbeat_every_60_s_and_slhbt_0_within_a_second_of_ha_offline() {
    let mut r = Rig::new(3, 0x00F);
    r.reg.mode = MqttMode::MqttHa;
    r.reg.broker_connected = true;
    r.start();
    r.run(130000);
    assert_eq!(r.count("slhbt 1"), 3);
    let hb = r.stm().lines_of("slhbt");
    r.reg.ha = HaStatus::Offline;
    let before = r.count("slhbt 0");
    r.run(1100);
    assert_eq!(r.count("slhbt 0"), before + 1);
    assert_eq!(hb.len(), 3);
    r.reg.ha = HaStatus::Online;
    r.run(1100);
    assert_eq!(r.count("slhbt 1"), 4);
}

#[test]
fn an_old_stm_gets_the_failsafe_positions_from_the_esp_after_the_timeout() {
    let mut r = Rig::new(1, 0x003);
    r.cfg.failsafe.timeout_min = 5;
    r.cfg.valves[1].failsafe_pct = 20;
    r.reg.mode = MqttMode::Mqtt;
    r.reg.broker_connected = true;
    r.start();
    r.run(15000);
    let t0 = r.target(0, 33, TargetSource::Mqtt);
    let t1 = r.target(1, 44, TargetSource::Mqtt);
    r.command(&t0);
    r.command(&t1);
    r.run(5000);
    assert_eq!(r.stm().target[0], 33);
    assert_eq!(r.stm().target[1], 44);
    r.reg.broker_connected = false; // the broker goes away
    r.run(5 * 60000 - 2000);
    assert_eq!(r.stm().target[0], 33);
    r.run(4000);
    assert_eq!(r.stm().target[0], 50);
    assert_eq!(r.stm().target[1], 20);
    assert_eq!(r.port().last.valves[0].desired, 33); // the MQTT target is unchanged
    assert!(r.port().last.valves[0].fs_override);
    assert_eq!(r.port().last.lease.state, LeaseState::Expired);
    let active = r.port().with_code(EventCode::FailsafeActive);
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].arg1, 0x003);
    assert_eq!(active[0].arg2, 2);
    assert!(r.port().lease.active);
    // Broker back: renewed after 120 s of life (no command), the targets return.
    r.reg.broker_connected = true;
    r.run(119000);
    assert_eq!(r.stm().target[0], 50);
    r.run(4000);
    assert_eq!(r.stm().target[0], 33);
    assert_eq!(r.stm().target[1], 44);
    assert!(r.port().has(EventCode::FailsafeEnded));
}

#[test]
fn after_an_esp_restart_an_active_lease_record_drives_the_failsafe_at_once() {
    let mut r = Rig::new(2, 0x001);
    r.cfg.failsafe.timeout_min = 5;
    let rec = LeaseClientSnapshot {
        lost: true,
        active: true,
        mask: 0x001,
        lost_elapsed_ms: 400000,
    };
    let mut t = PersistedTargets::default();
    t.valid[0] = true;
    t.pos[0] = 30;
    t.source[0] = TargetSource::Mqtt;
    r.reg.mode = MqttMode::Mqtt;
    r.stm_mut().target[0] = 30;
    r.start_with(&t, RestoreSource::Rtc, Some(&rec));
    r.run(20000);
    let pushes = r.stm().lines_of("stgtp 0");
    assert!(!pushes.is_empty());
    assert_eq!(pushes[0], "stgtp 0 50");
    assert!(!r.port().has(EventCode::FailsafeActive));
    assert_eq!(r.port().with_code(EventCode::TargetsRestored).len(), 1);
}

#[test]
fn an_assembly_valve_is_never_overridden_and_gets_staop_again_after_a_reboot() {
    let mut r = Rig::new(2, 0x001);
    r.cfg.failsafe.timeout_min = 5;
    r.reg.mode = MqttMode::Mqtt; // broker never up: the regulator is dead
    r.start();
    r.run(15000);
    r.command(&cmd(StmCommandType::Assembly, 0));
    r.run(2000);
    assert_eq!(r.count("staop 0"), 1);
    assert_eq!(r.port().last.valves[0].source, TargetSource::Assembly);
    // STM cold reboot (uptime back to 0).
    let now = r.now;
    r.stm_mut().reset(u64::from(now));
    r.run(20000);
    assert!(r.count("staop 0") >= 2);
    r.run(6 * 60000);
    assert_eq!(r.count("stgtp"), 0);
    assert!(!r.port().last.valves[0].fs_override);
    assert_eq!(r.stm().target[0], 100);
}

// ================================================================ assembly (W1)

#[test]
fn assembly_sends_staop_and_no_stgtp_a_web_target_afterwards_is_pushed() {
    let mut r = Rig::new(3, 0x004);
    r.start();
    r.run(10000);
    let t = r.target(2, 30, TargetSource::Web);
    r.command(&t);
    r.run(3000);
    assert_eq!(r.stm().target[2], 30);
    r.command(&cmd(StmCommandType::Assembly, 2));
    r.run(60000);
    assert_eq!(r.count("staop 2"), 1);
    assert_eq!(r.stm().lines_of("stgtp"), ["stgtp 2 30"]);
    let v = &r.port().last.valves[2];
    assert_eq!(v.desired, 100);
    assert_eq!(v.source, TargetSource::Assembly);
    assert_eq!(v.sync, TargetSync::Synced);
    let set = r.port().with_code(EventCode::TargetSet);
    let last = set.last().expect("TargetSet");
    assert_eq!(last.arg1, 100);
    assert_eq!(last.arg2, 5);
    let t = r.target(2, 40, TargetSource::Web);
    r.command(&t);
    r.run(3000);
    assert_eq!(
        r.stm().lines_of("stgtp").last().map(String::as_str),
        Some("stgtp 2 40")
    );
}

// ================================================================ restored targets (W2)

#[test]
fn restored_targets_no_stgtp_when_the_stm_holds_them_pushed_when_it_does_not() {
    let mut r = Rig::new(3, 0x003);
    let mut t = PersistedTargets::default();
    t.valid[0] = true;
    t.pos[0] = 33;
    t.source[0] = TargetSource::Mqtt;
    t.valid[1] = true;
    t.pos[1] = 77;
    t.source[1] = TargetSource::Web;
    r.stm_mut().target[0] = 33;
    r.start_with(&t, RestoreSource::Rtc, None);
    r.run(15000);
    assert_eq!(r.stm().lines_of("stgtp"), ["stgtp 1 77"]);
    assert_eq!(r.port().last.valves[0].source, TargetSource::Restored);
    assert_eq!(r.port().last.valves[0].sync, TargetSync::Synced);
    let ev = r.port().with_code(EventCode::TargetsRestored);
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].arg1, 2);
    assert_eq!(ev[0].arg2, 1);
    assert!(!r.port().has(EventCode::TargetSet));
}

#[test]
fn desired_target_changes_go_to_the_port_once_per_change() {
    let mut r = Rig::new(3, 0x001);
    r.start();
    r.run(10000);
    let n = r.port().targets.len();
    assert!(n >= 1); // the adopted STM target
    let t = r.target(0, 20, TargetSource::Web);
    r.command(&t);
    r.run(3000);
    let targets = &r.port().targets;
    assert_eq!(targets.len(), n + 1);
    let last = targets.last().expect("targets");
    assert!(last.valid[0]);
    assert_eq!(last.pos[0], 20);
    assert_eq!(last.source[0], TargetSource::Web);
    r.run(10000);
    assert_eq!(r.port().targets.len(), n + 1);
}

// ================================================================ scheduled calibration (E1)

#[test]
fn a_scheduled_staln_reports_its_stm_result_with_the_attempt() {
    let mut r = Rig::new(3, 0x00F);
    r.start();
    r.run(10000);
    let mut c = cmd(StmCommandType::Calibrate, ALL_VALVES);
    c.scheduled = true;
    c.attempt = 4;
    r.command(&c);
    r.run(1000);
    assert_eq!(r.count("staln 255"), 1);
    let calibs = &r.port().calibs;
    assert_eq!(calibs.len(), 1);
    assert_eq!(calibs[0].attempt, 4);
    assert!(calibs[0].ok);
    assert_eq!(calibs[0].reason, CalibFailure::None);
    // A manual one reports nothing.
    r.command(&cmd(StmCommandType::Calibrate, 1));
    r.run(1000);
    assert_eq!(r.port().calibs.len(), 1);
}

#[test]
fn a_scheduled_staln_without_an_answer_reports_no_reply() {
    let mut r = Rig::new(3, 0x00F);
    r.start();
    r.run(10000);
    r.stm_mut().set_answer("staln", |_, _| String::new());
    let mut c = cmd(StmCommandType::Calibrate, ALL_VALVES);
    c.scheduled = true;
    c.attempt = 9;
    r.command(&c);
    r.run(5000);
    let calibs = &r.port().calibs;
    assert_eq!(calibs.len(), 1);
    assert_eq!(calibs[0].attempt, 9);
    assert!(!calibs[0].ok);
    assert_eq!(calibs[0].reason, CalibFailure::NoReply);
}

// ================================================================ EEPROM-safe resets (E4)

#[test]
fn a_user_reset_waits_for_the_eeprom_motor_settings_eepst_until_1_then_nrst() {
    let mut r = Rig::new(3, 0x00F);
    r.start();
    r.run(10000);
    r.stm_mut().eep = 0;
    let mut m = cmd0(StmCommandType::SetMotorSettings);
    m.has_motor = true;
    m.motor.min_counts = 3100;
    r.command(&m);
    r.command(&cmd0(StmCommandType::ResetStm));
    r.run(1600);
    assert!(r.port().pulses.is_empty());
    let l = &r.stm().lines;
    let smotc = position(l, |s| s.starts_with("smotc")).expect("smotc");
    let eep = position(l, |s| s == "eepst").expect("eepst");
    assert!(smotc < eep);
    assert!(r.count("eepst") >= 3); // at once, then every 500 ms
    r.stm_mut().eep = 1;
    r.run(600);
    assert_eq!(r.port().pulses.len(), 1);
    assert!(r.port().has(EventCode::StmResetByUser));
    assert!(!r.port().has(EventCode::StmEepromWaitTimeout));
    r.command(&cmd0(StmCommandType::ResetStm));
    r.command(&cmd0(StmCommandType::ResetStm)); // a second one while the gate runs is ignored
    r.run(12000);
    assert_eq!(r.port().pulses.len(), 2);
}

#[test]
fn a_reset_gives_up_waiting_after_10_s_and_reports_it() {
    let mut r = Rig::new(3, 0x00F);
    r.start();
    r.run(10000);
    r.stm_mut().eep = 0;
    let t0 = r.now;
    r.command(&cmd0(StmCommandType::ResetStm));
    r.run(9990);
    assert!(r.port().pulses.is_empty());
    r.run(20);
    assert_eq!(r.port().pulses.len(), 1);
    assert_eq!(r.port().pulses[0] - t0, 10000);
    let ev = r.port().with_code(EventCode::StmEepromWaitTimeout);
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].arg1, 10000);
    assert_eq!(ev[0].arg2, 1);
}

#[test]
fn a_silent_stm_is_reset_at_once_by_the_user() {
    let mut r = Rig::new(3, 0x00F);
    r.start();
    r.stm_mut().silent = true;
    r.run(30000);
    assert_eq!(r.port().last.link, LinkState::Down);
    r.command(&cmd0(StmCommandType::ResetStm));
    r.run(4);
    assert_eq!(r.port().pulses.len(), 1);
    assert_eq!(r.count("eepst"), 0);
}

#[test]
fn the_stm_save_before_an_esp_restart_eepst_until_1_saved() {
    let mut r = Rig::new(3, 0x00F);
    r.start();
    r.run(10000);
    r.stm_mut().eep = 0;
    r.save = StmSaveState::Waiting;
    r.run(1100);
    let first = r.count("eepst");
    assert!(first >= 1);
    r.run(1000);
    assert!(r.count("eepst") >= first + 2);
    assert!(r.port().saves.is_empty());
    r.stm_mut().eep = 1;
    r.run(600);
    assert_eq!(r.port().saves, [StmSaveState::Saved]);
}

#[test]
fn the_stm_save_no_link_unavailable_at_once_no_answer_timed_out_at_10_s() {
    let mut down = Rig::new(3, 0x00F);
    down.start();
    down.run(3000); // still in the start-up hold: the STM does not answer yet
    down.save = StmSaveState::Waiting;
    down.run(1000);
    assert_eq!(down.port().saves, [StmSaveState::Unavailable]);
    assert_eq!(down.count("eepst"), 0);

    let mut r = Rig::new(3, 0x00F);
    r.start();
    r.run(10000);
    r.stm_mut().eep = 0;
    r.save = StmSaveState::Waiting;
    r.run_until(|r| r.count("eepst") > 0, 2000);
    let t0 = r.now;
    r.run_until(|r| !r.port().saves.is_empty(), 12000);
    assert_eq!(r.port().saves, [StmSaveState::TimedOut]);
    assert!(r.now - t0 >= 9000);
    let ev = r.port().with_code(EventCode::StmEepromWaitTimeout);
    assert_eq!(ev.len(), 1);
    assert!(ev[0].arg1 >= 10000);
    assert_eq!(ev[0].arg2, 3);
}

// ================================================================ link recovery (E8)

#[test]
fn a_link_interruption_on_protocol_3_checks_gstax_and_is_no_reboot() {
    let mut r = Rig::new(3, 0x00F);
    r.start();
    r.run(15000);
    let gproto = r.count("gproto");
    r.stm_mut().silent = true;
    r.run(8000);
    assert!(r.port().has(EventCode::LinkDown));
    let gstax = r.count("gstax");
    r.stm_mut().silent = false;
    r.run(3000);
    assert!(r.port().has(EventCode::LinkUp));
    assert!(r.count("gstax") > gstax); // C++ >= gstax + 1
    assert!(!r.port().has(EventCode::StmRebootDetected));
    assert_eq!(r.count("gproto"), gproto);
}

#[test]
fn an_stm_reset_while_the_link_was_down_is_detected_by_the_uptime() {
    let mut r = Rig::new(3, 0x00F);
    r.start();
    r.run(15000);
    r.stm_mut().silent = true;
    r.run(8000);
    let now = r.now;
    r.stm_mut().reset(u64::from(now));
    r.stm_mut().silent = false;
    r.run(5000);
    let ev = r.port().with_code(EventCode::StmRebootDetected);
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].arg1, 1);
    assert_eq!(r.count("gproto"), 2);
}

#[test]
fn a_link_interruption_on_protocol_1_is_a_reboot() {
    let mut r = Rig::new(1, 0x00F);
    r.start();
    r.run(15000);
    r.stm_mut().silent = true;
    r.run(8000);
    r.stm_mut().silent = false;
    r.run(5000);
    let ev = r.port().with_code(EventCode::StmRebootDetected);
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].arg1, 4);
}

// ================================================================ sensors (E5, E9)

#[test]
fn a_sensor_scan_on_a_v1_stm_matches_the_sensors_5_s_later() {
    let mut r = Rig::new(1, 0x00F);
    r.start();
    r.run(15000);
    r.command(&cmd0(StmCommandType::ScanSensors));
    r.run(4000);
    assert_eq!(r.count("stons"), 1);
    assert_eq!(r.count("masns"), 0);
    r.run(2000);
    assert_eq!(r.count("masns"), 1);
    r.run(1000);
    assert!(r.stm().lines_of("gvlon 255").len() >= 2);
}

#[test]
fn no_masns_after_a_scan_on_protocol_2_3() {
    let mut r = Rig::new(3, 0x00F);
    r.start();
    r.run(10000);
    r.command(&cmd0(StmCommandType::ScanSensors));
    r.run(8000);
    assert_eq!(r.count("stons"), 1);
    assert_eq!(r.count("masns"), 0);
}

#[test]
fn a_goned_reply_with_another_id_does_not_answer_the_request_and_lands_by_id() {
    let mut r = Rig::new(1, 0x00F);
    let id_a = "28-84-37-94-97-ff-03-23";
    let id_b = "28-aa-bb-cc-dd-ee-01-67";
    r.stm_mut().set_answer("gonec", move |l, _| {
        if l == "gonec" {
            String::from("gonec 2 ")
        } else {
            format!("gonec 2 {id_a},{id_b} ")
        }
    });
    let calls = Rc::new(Cell::new(0));
    let counter = calls.clone();
    r.stm_mut().set_answer("goned", move |l, _| {
        counter.set(counter.get() + 1);
        // bus 0 answers with sensor B's reading (a late reply for the other index)
        if l == "goned 0" {
            return format!("goned {id_b} 201 ");
        }
        format!("goned {id_b} 201 ")
    });
    r.start();
    r.run(40000);
    assert!(calls.get() > 0);
    assert_eq!(r.s.sensors().temp(1).raw, 201);
    assert!(r.s.sensors().temp(1).seen);
    assert!(!r.s.sensors().temp(0).seen); // never answered with its own id
    assert!(r.s.link().stats().stray_lines > 0);
}

// ================================================================ protocol 3 commands (S3, S8, S9)

#[test]
fn stop_and_safe_mode_exit_on_protocol_3_dropped_below() {
    let mut r = Rig::new(3, 0x007);
    r.start();
    r.run(10000);
    r.command(&cmd(StmCommandType::StopValve, 2));
    r.run(2);
    assert_eq!(r.count("sstop 2"), 1);
    r.run(200);
    assert!(r.count("gvlvy 2") >= 2); // read back at once
    r.command(&cmd(StmCommandType::StopValve, ALL_VALVES));
    r.command(&cmd0(StmCommandType::LeaveSafeMode));
    r.run(500);
    assert_eq!(r.count("sstop 255"), 1);
    assert_eq!(r.count("ssafe 0"), 1);
    let mut old = Rig::new(2, 0x00F);
    old.start();
    old.run(10000);
    old.command(&cmd(StmCommandType::StopValve, 2));
    old.command(&cmd0(StmCommandType::LeaveSafeMode));
    old.run(1000);
    assert_eq!(old.count("sstop"), 0);
    assert_eq!(old.count("ssafe"), 0);
}

#[test]
fn the_esp_schedule_switches_the_stm_time_trigger_off() {
    let mut r = Rig::new(3, 0x00F);
    r.cfg.calib.day_mask = 0x7F;
    r.cfg.calib.hour = 3;
    r.start();
    r.run(10000);
    assert_eq!(r.stm().lines_of("stlnt"), ["stlnt 0"]);
    assert_eq!(r.stm().learn_time, 0);
    assert_eq!(r.port().last.learn_time_s, 0);
    r.cfg.calib.day_mask = 0;
    let cfg = r.cfg.clone();
    r.s.apply_config(&cfg, true);
    r.run(2000);
    assert_eq!(r.stm().learn_time, 604800);
}

// ================================================================ flashing (W8, E4)

#[test]
fn a_flash_of_an_image_for_another_board_fails_in_validating_the_stm_untouched() {
    let mut r = Rig::new(3, 0x00F);
    r.start();
    r.run(10000);
    assert_eq!(&r.port().last.version.hw[..], b"C2");
    let mut img = vec_of(4096, 0x80);
    img[0..4].copy_from_slice(&0x2002_0000u32.to_le_bytes());
    img[4..8].copy_from_slice(&0x0800_01C5u32.to_le_bytes());
    let strs = b"\x01DEADBEEF\0\x01BEEFIT\0\x01VDM-HW:C1\0";
    img[2000..2000 + strs.len()].copy_from_slice(strs);
    r.port_mut().image.data = img;
    let mut c = cmd0(StmCommandType::StartFlash);
    c.image = name("vdm.bin");
    r.command(&c);
    assert_eq!(r.port().flash_marks, 1);
    assert!(r.s.flash_pending());
    r.run(3000);
    assert!(!r.s.flashing());
    assert!(!r.s.flash_pending());
    assert!(r.count("eepst") >= 1);
    assert_eq!(r.port().opened, ["vdm.bin"]);
    assert_eq!(r.port().closed, 1);
    let ev = r.port().with_code(EventCode::StmFlashFailed);
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].arg1, FlashError::BoardMismatch as i32);
    assert!(r.sim.resets.is_empty());
    assert!(r.port().pulses.is_empty());
    assert_eq!(r.port().last.flash.error, FlashError::BoardMismatch);
}

fn vec_of(n: usize, b: u8) -> Vec<u8> {
    std::vec![b; n]
}

#[test]
fn flash_requests_busy_restart_pending_unreadable_image_abort_while_waiting() {
    let mut r = Rig::new(3, 0x00F);
    r.start();
    r.run(10000);
    r.stm_mut().eep = 0;
    let mut c = cmd0(StmCommandType::StartFlash);
    c.image = name("a.bin");
    r.command(&c);
    assert!(r.s.flash_pending());
    r.command(&c); // a second one while waiting
    let failed = r.port().with_code(EventCode::StmFlashFailed);
    assert_eq!(text(failed.last().expect("failed")), "busy");
    r.command(&cmd0(StmCommandType::AbortFlash));
    assert!(!r.s.flash_pending());
    r.run(12000);
    assert!(r.port().opened.is_empty());
    r.port_mut().restart = true;
    r.command(&c);
    let failed = r.port().with_code(EventCode::StmFlashFailed);
    assert_eq!(text(failed.last().expect("failed")), "restart pending");
    assert!(!r.s.flash_pending());
    r.port_mut().restart = false;
    r.port_mut().image_ok = false;
    c.blank = true; // blank mode: no EEPROM wait
    r.command(&c);
    assert_eq!(r.port().opened.len(), 1);
    let failed = r.port().with_code(EventCode::StmFlashFailed);
    assert_eq!(
        failed.last().expect("failed").arg1,
        FlashError::ImageRead as i32
    );
    assert!(!r.s.flashing());
}

#[test]
fn constants() {
    type S = crate::test_support::session_rig::Session;
    assert_eq!(S::SENSOR_STALE_MS, 60_000);
    assert_eq!(S::SENSOR_GRACE_MS, 30_000);
    assert_eq!(S::SERVICE_MOVE_WAIT_MS, 300_000);
    assert_eq!(S::SCHEDULED_CALIB_WINDOW_MS, 14_400_000);
    assert_eq!(S::PUBLISH_MIN_MS, 100);
    assert_eq!(S::TAG_SCHEDULED_CALIB, 1);
    assert_eq!(S::STM_BAUD, 115_200);
    assert_eq!(GateAction::None as i32, 0);
    assert_eq!(GateAction::StmReset as i32, 1);
    assert_eq!(GateAction::Flash as i32, 2);
    assert_eq!(GateAction::EspRestart as i32, 3);
}
