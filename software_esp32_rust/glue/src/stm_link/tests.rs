//! Tests of `stm_link` (C++ `test_stm_link.cpp`): UART and NRST wiring, bounded reads, the task
//! loop against the fake STM, the port of the STM session (stm_service hand-over, config trust,
//! flash image).
// host test code: the stack rule of the glue (design 2.4) is for the device
#![allow(clippy::large_stack_frames)]

use super::rig::{command, flash, image, Rig};
use super::*;
use crate::port::Fs;
use vdm_esp_core::common::ALL_VALVES;
use vdm_esp_core::config::MqttMode;
use vdm_esp_core::event_log::EventCode;
use vdm_esp_core::link_policy::LinkState;
use vdm_esp_core::stm_flasher::FlashError;
use vdm_esp_core::stm_types::StmCommandType;
use vdm_esp_core::valve_model::TargetSource;

#[test]
fn begin_serial2_8n1_at_115200_and_nrst_released() {
    // C++ also checked pins 5/17 and the 2048/512-byte buffers: the UART adapter's
    // configuration (board.rs, GLUE-DESIGN-ESP.md 1.2)
    let rig = Rig::new();
    let mut link = rig.link();
    link.begin();
    {
        let u = rig.dev.uart.state();
        assert!(u.open);
        assert_eq!(u.baud, 115_200);
        assert!(!u.even_parity);
        assert_eq!(u.configures, 1); // the port reopens with the ring sizes
        assert_eq!(u.rx_capacity, 2048);
    }
    assert_eq!(rig.dev.gpio.level(15), Some(false));
    assert_eq!(rig.dev.gpio.level(14), Some(false));
}

#[test]
fn release_reset_boot0_low_before_nrst_is_released_both_driven() {
    let rig = Rig::new();
    let mut link = rig.link();
    link.release_reset();
    assert_eq!(rig.dev.journal.entries(), vec!["gpio 14=0", "gpio 15=0"]);
}

#[test]
fn release_stm_reset_on_the_bare_pins_before_the_link_exists() {
    // main's first step (GLUE-DESIGN-ESP.md 6.4): the pins go into the link afterwards
    let rig = Rig::new();
    let mut nrst = rig.dev.gpio.output(15);
    let mut boot0 = rig.dev.gpio.output(14);
    release_stm_reset(&mut boot0, &mut nrst);
    assert_eq!(rig.dev.journal.entries(), vec!["gpio 14=0", "gpio 15=0"]);
    assert_eq!(rig.dev.gpio.level(15), Some(false));
    assert_eq!(rig.dev.gpio.level(14), Some(false));
}

#[test]
fn pulse_reset_nrst_high_for_100_ms() {
    let rig = Rig::new();
    let mut link = rig.link();
    link.pulse_reset();
    assert_eq!(rig.dev.journal.entries(), vec!["gpio 15=1", "gpio 15=0"]);
    assert_eq!(rig.dev.clock.sleeps(), vec![100]);
    assert_eq!(rig.dev.clock.ms(), 100);
    assert_eq!(RESET_PULSE_MS, 100);
}

#[test]
fn task_the_link_comes_up_with_a_protocol_1_stm_within_10_s() {
    let rig = Rig::new();
    let stm = rig.stm();
    stm.protocol(1);
    let mut link = rig.link();
    link.begin();
    rig.run_task(&mut link, 10_000);
    let h = rig.host.s();
    assert_eq!(h.published.link, LinkState::Up);
    assert_eq!(h.published.proto, 1);
    assert!(!stm.requests_of("gvers").is_empty());
    // C++ esp_task_wdt_add() once: the thread's watchdog is fed every pass
    assert_eq!(rig.dev.watchdog.feeds(), 5000);
}

#[test]
fn task_the_link_comes_up_with_a_protocol_2_stm_within_10_s() {
    let rig = Rig::new();
    let stm = rig.stm();
    stm.protocol(2);
    let mut link = rig.link();
    link.begin();
    rig.run_task(&mut link, 10_000);
    let h = rig.host.s();
    assert_eq!(h.published.link, LinkState::Up);
    assert_eq!(h.published.proto, 2);
    assert!(h.published.have_status);
}

#[test]
fn task_the_link_comes_up_with_a_protocol_3_stm_within_10_s() {
    let rig = Rig::new();
    let stm = rig.stm();
    stm.protocol(3);
    let mut link = rig.link();
    link.begin();
    rig.run_task(&mut link, 10_000);
    assert_eq!(rig.host.s().published.link, LinkState::Up);
    assert!(!stm.requests_of("gproto").is_empty());
    assert_eq!(rig.host.s().published.proto, 3);
}

#[test]
fn task_a_silent_stm_is_reset_by_the_link_policy() {
    let rig = Rig::new();
    let stm = rig.stm();
    stm.silent(true);
    let mut link = rig.link();
    link.begin();
    rig.run_task(&mut link, 70_000);
    assert_eq!(stm.resets(), 1);
    let j = &rig.dev.journal;
    let high = j.find("gpio 15=1", 0).expect("NRST asserted");
    assert_eq!(j.find("gpio 15=0", high), Some(high + 1));
    let pulses = rig.dev.clock.sleeps().iter().filter(|&&d| d == 100).count();
    assert_eq!(pulses, 1);
    assert!(rig.host.has(EventCode::StmResetByPolicy));
    assert_ne!(rig.host.s().published.link, LinkState::Up);
}

#[test]
fn task_a_target_command_becomes_an_stgtp_line() {
    let rig = Rig::new();
    rig.host.active_valve(2);
    let stm = rig.stm();
    let mut link = rig.link();
    link.begin();
    let mut c = command(StmCommandType::SetTarget, 2);
    c.pos = 40;
    c.source = TargetSource::Web;
    // the command arrives while the task runs (a restarted task would hold the link again)
    rig.run_with(&mut link, c, 8000, 9000);
    assert_eq!(rig.host.s().published.link, LinkState::Up);
    let sent = stm.requests_of("stgtp");
    assert_eq!(sent.last().map(String::as_str), Some("stgtp 2 40 "));
}

#[test]
fn task_a_profile_goes_to_the_profile_store_the_snapshot_counts_it() {
    let rig = Rig::new();
    rig.host.active_valve(2);
    let stm = rig.stm();
    stm.protocol(3);
    let mut link = rig.link();
    link.begin();
    rig.run_with(
        &mut link,
        command(StmCommandType::RequestProfile, 2),
        8000,
        9000,
    );
    assert_eq!(stm.requests_of("gprof").len(), 1);
    let h = rig.host.s();
    assert_eq!(h.stored_profiles.len(), 1);
    assert_eq!(h.stored_profiles[0].valve, 2);
    assert_eq!(h.stored_profiles[0].count, 3);
    assert_eq!(h.published.profile_seq[2], 1);
}

#[test]
fn task_a_user_reset_pulses_nrst_and_logs_it() {
    let rig = Rig::new();
    let stm = rig.stm();
    let mut link = rig.link();
    link.begin();
    rig.run_with(
        &mut link,
        command(StmCommandType::ResetStm, 0xFF),
        6000,
        6010,
    );
    assert_eq!(stm.resets(), 1);
    assert!(rig.host.has(EventCode::StmResetByUser));
}

#[test]
fn task_the_first_request_on_the_uart_is_gproto_after_the_5_s_hold() {
    let rig = Rig::new();
    let _stm = rig.stm();
    let mut link = rig.link();
    link.begin();
    let early = std::sync::Arc::new(std::sync::Mutex::new(None::<Vec<u8>>));
    {
        let (clock, uart, early) = (rig.dev.clock.clone(), rig.dev.uart.clone(), early.clone());
        rig.dev.clock.on_sleep(move |_| {
            if clock.ms() == 4990 {
                *early.lock().unwrap() = Some(uart.state().tx.clone());
            }
        });
    }
    rig.run_task(&mut link, 5100);
    assert_eq!(early.lock().unwrap().as_deref(), Some(&b""[..]));
    let tx = rig.dev.uart.state().tx.clone();
    assert!(tx.len() >= 9);
    assert_eq!(&tx[..9], b"gproto \r\n");
}

#[test]
fn task_one_pass_reads_at_most_512_bytes_from_the_uart() {
    let rig = Rig::new();
    let mut link = rig.link();
    link.begin();
    rig.dev.uart.inject(&[b'x'; 600], None);
    rig.run_passes(&mut link, 1);
    assert_eq!(rig.dev.uart.available(), 88);
    assert_eq!(READ_CHUNK * READ_CHUNKS_PER_PASS, 512);
}

#[test]
fn task_boot_targets_from_stm_service_are_restored_and_logged() {
    let rig = Rig::new();
    rig.host.active_valve(0);
    {
        let mut h = rig.host.s();
        h.boot_targets.valid[0] = true;
        h.boot_targets.pos[0] = 33;
        h.boot_targets.source[0] = TargetSource::Mqtt;
        h.boot_source = RestoreSource::Nvs;
    }
    let stm = rig.stm();
    let mut link = rig.link();
    link.begin();
    rig.run_task(&mut link, 10_000);
    let e = rig.host.first(EventCode::TargetsRestored);
    assert_eq!((e.arg1, e.arg2), (1, 2));
    let h = rig.host.s();
    assert_eq!(h.published.valves[0].desired, 33);
    assert_eq!(h.published.valves[0].source, TargetSource::Restored);
    // the fake STM does not keep it
    assert_eq!(
        stm.requests_of("stgtp").first().map(String::as_str),
        Some("stgtp 0 33 ")
    );
    assert_eq!(h.stored_targets.last().map(|t| t.pos[0]), Some(33));
}

#[test]
fn task_an_active_lease_record_from_stm_service_drives_the_failsafe_at_once() {
    let rig = Rig::new();
    rig.host.active_valve(0);
    {
        let mut h = rig.host.s();
        h.cfg.failsafe.timeout_min = 5;
        h.boot_lease = Some(LeaseClientSnapshot {
            lost: true,
            active: true,
            mask: 0x001,
            lost_elapsed_ms: 400_000,
        });
        h.regulator.mode = MqttMode::Mqtt;
    }
    let stm = rig.stm();
    let mut link = rig.link();
    link.begin();
    rig.run_task(&mut link, 10_000);
    assert_eq!(
        stm.requests_of("stgtp").first().map(String::as_str),
        Some("stgtp 0 50 ")
    );
    let h = rig.host.s();
    assert!(h.lease_records.last().is_some_and(|r| r.active));
    assert!(h.lease_records.len() >= 9); // once per second
}

#[test]
fn task_stored_config_the_failsafe_settings_are_pushed_to_a_protocol_3_stm() {
    let rig = Rig::new();
    rig.host.s().load_source = LoadSource::Stored;
    let stm = rig.stm();
    stm.protocol(3);
    let mut link = rig.link();
    link.begin();
    rig.run_task(&mut link, 10_000);
    assert_eq!(stm.requests_of("sfspo"), vec!["sfspo 255 255 "]);
}

#[test]
fn task_unsaved_default_config_nothing_is_pushed() {
    let rig = Rig::new();
    rig.host.s().load_source = LoadSource::Defaults;
    rig.host.s().config_saved = false;
    let stm = rig.stm();
    stm.protocol(3);
    let mut link = rig.link();
    link.begin();
    rig.run_task(&mut link, 10_000);
    assert!(!stm.requests_of("glcfg").is_empty());
    assert!(stm.requests_of("sfspo").is_empty());
    assert!(!rig.host.s().published.lease.config_trusted);
}

#[test]
fn task_default_config_saved_since_boot_or_restored_after_an_error_is_trusted() {
    let rig = Rig::new();
    rig.host.s().load_source = LoadSource::DefaultsAfterError;
    rig.host.s().config_saved = true;
    let stm = rig.stm();
    stm.protocol(3);
    let mut link = rig.link();
    link.begin();
    rig.run_task(&mut link, 10_000);
    assert_eq!(stm.requests_of("sfspo").len(), 1);
    assert!(rig.host.s().published.lease.config_trusted);
}

#[test]
fn task_after_error_defaults_without_a_save_are_not_trusted() {
    let rig = Rig::new();
    rig.host.s().load_source = LoadSource::DefaultsAfterError;
    rig.host.s().config_saved = false;
    let stm = rig.stm();
    stm.protocol(3);
    let mut link = rig.link();
    link.begin();
    rig.run_task(&mut link, 10_000);
    assert!(stm.requests_of("sfspo").is_empty());
}

#[test]
fn task_every_other_load_source_is_trusted_without_a_save() {
    // Rust addition: only the two default sources wait for a save
    for src in [LoadSource::Stored, LoadSource::Imported, LoadSource::Backup] {
        let rig = Rig::new();
        rig.host.s().load_source = src;
        let stm = rig.stm();
        stm.protocol(3);
        let mut link = rig.link();
        link.begin();
        rig.run_task(&mut link, 10_000);
        assert!(rig.host.s().published.lease.config_trusted, "{src:?}");
    }
}

#[test]
fn task_a_config_revision_change_is_applied_in_the_loop() {
    let rig = Rig::new();
    rig.host.s().load_source = LoadSource::Stored;
    let stm = rig.stm();
    stm.protocol(3);
    let mut link = rig.link();
    link.begin();
    {
        let (clock, host) = (rig.dev.clock.clone(), rig.host.clone());
        rig.dev.clock.on_sleep(move |_| {
            if clock.ms() == 9000 {
                host.active_valve(5);
            }
        });
    }
    rig.run_task(&mut link, 12_000);
    assert_eq!(
        stm.requests_of("sfspo"),
        vec!["sfspo 255 255 ", "sfspo 5 50 "]
    );
    // read at the start and once for the new revision
    assert_eq!(rig.host.s().config_reads, 2);
}

#[test]
fn task_a_config_change_takes_no_heap_block_and_is_applied_in_the_pass_that_sees_it() {
    // C++ "a config change without memory for its copy is applied by the next pass": the copy
    // is gone (GLUE-DESIGN-ESP.md 2.4), the session reads the config under the config lock
    let rig = Rig::new();
    rig.host.s().load_source = LoadSource::Stored;
    let stm = rig.stm();
    stm.protocol(3);
    let mut link = rig.link();
    link.begin();
    {
        let (clock, host, heap) = (
            rig.dev.clock.clone(),
            rig.host.clone(),
            rig.dev.heap.clone(),
        );
        rig.dev.clock.on_sleep(move |_| {
            if clock.ms() == 9000 {
                host.active_valve(5);
                heap.state().fail_all = true;
            }
        });
    }
    rig.run_task(&mut link, 12_000);
    assert_eq!(
        stm.requests_of("sfspo"),
        vec!["sfspo 255 255 ", "sfspo 5 50 "]
    );
    assert_eq!(rig.host.s().config_reads, 2);
    let heap = rig.dev.heap.state();
    assert!(heap.granted.is_empty() && heap.refused.is_empty());
}

#[test]
fn task_the_session_begins_with_its_config_without_a_heap_block() {
    // C++ "without memory for the config the session waits, then begins with it": no copy, no
    // wait; the first pass runs with the session begun on its config
    let rig = Rig::new();
    rig.host.active_valve(0);
    {
        let mut h = rig.host.s();
        h.boot_targets.valid[0] = true;
        h.boot_targets.pos[0] = 33;
        h.boot_targets.source[0] = TargetSource::Mqtt;
        h.boot_source = RestoreSource::Nvs;
    }
    let mut link = rig.link();
    link.begin();
    rig.dev.heap.state().fail_all = true;
    rig.run_passes(&mut link, 3);
    assert_eq!(rig.dev.clock.sleeps(), vec![2, 2, 2]);
    assert_eq!(rig.dev.watchdog.feeds(), 3);
    // the session began with its config: valve 0 active, its target restored
    let restored = rig.host.with_code(EventCode::TargetsRestored);
    assert_eq!(restored.len(), 1);
    assert_eq!(restored[0].arg1, 1);
    let heap = rig.dev.heap.state();
    assert!(heap.granted.is_empty() && heap.refused.is_empty());
}

#[test]
fn task_the_stm_save_before_an_esp_restart_reports_saved() {
    let rig = Rig::new();
    let stm = rig.stm();
    let mut link = rig.link();
    link.begin();
    {
        let (clock, host) = (rig.dev.clock.clone(), rig.host.clone());
        rig.dev.clock.on_sleep(move |_| {
            if clock.ms() == 7000 {
                host.s().save_state = StmSaveState::Waiting;
            }
        });
    }
    rig.run_task(&mut link, 9000);
    assert!(!stm.requests_of("eepst").is_empty());
    assert_eq!(rig.host.s().save_states, vec![StmSaveState::Saved]);
}

#[test]
fn task_a_scheduled_calibration_result_goes_to_stm_service() {
    let rig = Rig::new();
    let stm = rig.stm();
    let mut link = rig.link();
    link.begin();
    let mut c = command(StmCommandType::Calibrate, ALL_VALVES);
    c.scheduled = true;
    c.attempt = 3;
    rig.run_with(&mut link, c, 8000, 9000);
    assert_eq!(stm.requests_of("staln"), vec!["staln 255 "]);
    assert_eq!(
        rig.host.s().calib_results,
        vec![(3, true, CalibFailure::None)]
    );
}

#[test]
fn task_a_flash_of_an_image_for_another_board_is_refused_before_any_reset() {
    let rig = Rig::new();
    rig.dev.fs.put("/stm/c1.bin", &image("C1"));
    assert!(rig.dev.fs.mount());
    let stm = rig.stm();
    let mut link = rig.link();
    link.begin();
    rig.run_with(&mut link, flash("c1", false, ""), 8000, 20_000);
    assert_eq!(rig.host.s().flash_marks, 1);
    assert_eq!(stm.resets(), 0);
    let e = rig.host.first(EventCode::StmFlashFailed);
    assert_eq!(e.arg1, FlashError::BoardMismatch as i32);
    assert!(rig.host.has(EventCode::StmFlashStarted));
    assert_eq!(rig.dev.fs.open_handles(), 0);
    assert!(!rig.dev.uart.state().even_parity);
    assert_eq!(rig.host.s().published.link, LinkState::Up);
}

#[test]
fn task_a_missing_image_and_a_pending_restart_refuse_the_flash() {
    let rig = Rig::new();
    let _stm = rig.stm();
    let mut link = rig.link();
    link.begin();
    let c = flash("none", true, "");
    rig.run_with(&mut link, c.clone(), 8000, 9000);
    let e = rig.host.first(EventCode::StmFlashFailed);
    assert_eq!(e.arg1, FlashError::ImageRead as i32);
    assert_eq!(&e.text[..], b"none");
    rig.host.s().restart_pending = true;
    let now = rig.dev.clock.ms();
    rig.run_with(&mut link, c, now + 100, 1000);
    let last = rig.host.with_code(EventCode::StmFlashFailed);
    assert_eq!(
        last.last().map(|e| e.text.to_vec()),
        Some(b"restart pending".to_vec())
    );
}
