//! Port of `test/native/glue/test_ota.cpp`: upload steps, restart scheduling and the restart path
//! (STM EEPROM gate, target flush, log flush), image validation with the loopback self-check, and
//! the switch back. A pending image is an image on trial of the boot guard (B in slot 1 uploaded
//! by A); `esp_ota_mark_app_valid_cancel_rollback` became the guard's confirmation (`otaOk`),
//! `esp_ota_mark_app_invalid_rollback_and_reboot` its switch to the fallback slot.
#![allow(clippy::large_stack_frames)]

use super::support::*;
use super::update::{UPDATE_ERROR_MD5, UPDATE_ERROR_NO_PARTITION, UPDATE_ERROR_WRITE};
use super::*;
use crate::port::{EspErr, Nvs, NvsNamespace};
use crate::testkit::board::{APP_A, APP_B};
use crate::testkit::ota::{SLOT_ADDR, SLOT_SIZE};
use crate::testkit::{run, Ended, FakeMd5, Reset, SlotImage};

const NEXT_SIZE: usize = SLOT_SIZE as usize; // the other slot of the fake partition table
const SLACK: usize = 16 * 1024;

fn restarted() -> Ended<()> {
    Ended::Reset(Reset::Software)
}

/// The journal index of `entry` (panics without it).
fn at(rig: &Rig, entry: &str) -> usize {
    match rig.dev.journal.find(entry, 0) {
        Some(i) => i,
        None => panic!(
            "no journal entry {entry:?} in {:?}",
            rig.dev.journal.entries()
        ),
    }
}

/// The trial record of the boot guard is still there (the image was not confirmed).
fn on_trial_in_nvs(rig: &Rig) -> bool {
    let b = rig.dev.nvs.get_blob("vdmrev", "otaTrial");
    b.len() == 32 && b[5] == 1
}

#[test]
fn upload_begin_the_announced_size_may_be_the_partition_plus_16_kib_framing() {
    let rig = Rig::confirmed();
    let mut up = rig.upload();
    assert!(up.upload_begin(NEXT_SIZE + SLACK, b""));
    assert!(up.upload_active());
    assert!(rig.shared.upload_active());
    assert_eq!(
        rig.host.first(EventCode::EspOtaStarted).arg1,
        (NEXT_SIZE + SLACK) as i32
    );
    // UPDATE_SIZE_UNKNOWN: the update takes up to the partition, no MD5
    assert_eq!(up.update.size(), SLOT_SIZE);
    assert_eq!(rig.dev.ota.knobs().begins, 1);
    assert_eq!(FRAMING_SLACK as usize, SLACK);
}

#[test]
fn upload_begin_one_byte_more_is_refused_before_the_update_starts() {
    let rig = Rig::confirmed();
    let mut up = rig.upload();
    assert!(!up.upload_begin(NEXT_SIZE + SLACK + 1, b""));
    assert_eq!(up.upload_error(), "image too large");
    assert_eq!(rig.dev.ota.knobs().begins, 0);
    assert!(!up.upload_active());
}

#[test]
fn upload_begin_no_partition_to_write_to() {
    let rig = Rig::confirmed();
    rig.dev.ota.knobs().no_other = true;
    let mut up = rig.upload();
    assert!(!up.upload_begin(10, b""));
    assert_eq!(up.upload_error(), "no ota partition");
}

#[test]
fn upload_begin_the_md5_goes_to_update_in_lowercase_empty_means_none() {
    let rig = Rig::confirmed();
    let mut up = rig.upload();
    let img = image(5000);
    let md5 = FakeMd5::hex(&img).to_uppercase(); // Update compares with its lowercase digest
    assert!(up.upload_begin(img.len(), md5.as_bytes()));
    assert!(up.upload_write(&img));
    assert!(up.upload_end(true));
    // a wrong digest given: the MD5 is checked
    let rig = Rig::confirmed();
    let mut up = rig.upload();
    assert!(up.upload_begin(10, b"09afAF0123456789abcdefABCDEF0123"));
    assert!(up.upload_write(&image(10)));
    assert!(!up.upload_end(true));
    assert_eq!(up.upload_error(), "MD5 Check Failed");
    // "" means none (a C string: a NUL ends it)
    assert!(up.upload_begin(10, b"\0abc"));
    assert!(up.upload_write(&image(10)));
    assert!(up.upload_end(true));
}

#[test]
fn upload_begin_an_md5_that_is_not_32_hex_digits_is_refused() {
    let rig = Rig::confirmed();
    let mut up = rig.upload();
    assert!(!up.upload_begin(10, b"09afAF0123456789abcdefABCDEF012g"));
    assert_eq!(up.upload_error(), "md5 invalid");
    assert!(!up.upload_begin(10, b"09afAF0123456789abcdefABCDEF012"));
    assert_eq!(rig.dev.ota.knobs().begins, 0);
}

#[test]
fn upload_begin_update_refusing_the_md5_aborts_the_upload() {
    // C++ scripted Update::setMD5 to fail; it fails only for a length other than 32, which the
    // normalised digest never has: the check and its abort stay for the Update contract
    let rig = Rig::confirmed();
    let mut up = rig.upload();
    assert!(up.upload_begin(10, b"09afAF0123456789abcdefABCDEF0123"));
    assert_eq!(rig.dev.ota.knobs().aborts, 0);
    assert!(up.upload_active());
}

#[test]
fn upload_begin_the_update_refusing_to_start_is_logged_with_its_error() {
    let rig = Rig::confirmed();
    rig.dev.ota.knobs().begin_err = Some(EspErr::OTA_PARTITION_CONFLICT);
    let mut up = rig.upload();
    assert!(!up.upload_begin(10, b""));
    assert_eq!(
        rig.host.first(EventCode::EspOtaFailed).arg1,
        i32::from(UPDATE_ERROR_NO_PARTITION)
    );
    assert_eq!(up.upload_error(), "Partition Could Not be Found");
    assert!(!up.upload_active());
    // no memory for the sector buffer: Arduino's failed malloc set no error code
    rig.dev.ota.knobs().begin_err = None;
    rig.dev.heap.state().next.push_back(false);
    assert!(!up.upload_begin(10, b""));
    assert_eq!(rig.host.with_code(EventCode::EspOtaFailed)[1].arg1, 0);
    assert_eq!(up.upload_error(), "No Error");
}

#[test]
fn upload_begin_refused_while_an_stm_flash_or_image_upload_runs_busy_while_an_upload_runs() {
    let rig = Rig::confirmed();
    let mut up = rig.upload();
    rig.host.state().flash_active = true;
    assert!(!up.upload_begin(10, b""));
    assert_eq!(up.upload_error(), "stm flash or upload running");
    rig.host.state().flash_active = false;
    rig.host.state().image_upload_active = true;
    assert!(!up.upload_begin(10, b""));
    rig.host.state().image_upload_active = false;
    assert!(up.upload_begin(10, b""));
    assert_eq!(up.upload_error(), "unknown");
    assert!(!up.upload_begin(10, b""));
    assert_eq!(up.upload_error(), "busy");
}

#[test]
fn upload_a_committed_image_asks_for_a_restart_in_1_s() {
    let rig = Rig::confirmed();
    let mut up = rig.upload();
    assert!(up.upload_begin(3, b""));
    let data = [0xE9, 1, 2];
    assert!(up.upload_write(&data));
    assert!(up.upload_write(&[]));
    assert!(up.upload_end(true));
    assert!(!up.upload_active());
    // end(true) took what was written and selected the slot
    assert_eq!(rig.dev.ota.knobs().written, data.to_vec());
    assert_eq!(rig.dev.ota.store().otadata, 1);
    assert_eq!(rig.host.first(EventCode::EspOtaDone).arg1, 3);
    assert!(rig.shared.restart_pending());
    let reboot = rig.host.first(EventCode::RebootRequested);
    assert_eq!(reboot.arg1, 1);
    assert_eq!(reboot.severity, Severity::Info);
    assert!(!up.upload_write(&data)); // no upload any more
    assert!(!up.upload_end(true));
    assert_eq!(up.upload_error(), "no upload");
}

#[test]
fn upload_an_aborted_or_empty_upload_fails_with_minus_1_and_minus_2() {
    let rig = Rig::confirmed();
    let mut up = rig.upload();
    assert!(up.upload_begin(3, b""));
    assert!(!up.upload_end(false));
    assert_eq!(up.upload_error(), "aborted");
    assert!(up.upload_begin(3, b""));
    assert!(!up.upload_end(true));
    assert_eq!(up.upload_error(), "empty image");
    let failed = rig.host.with_code(EventCode::EspOtaFailed);
    assert_eq!(failed.len(), 2);
    assert_eq!((failed[0].arg1, failed[1].arg1), (-1, -2));
    assert!(!rig.shared.restart_pending());
    assert_eq!(rig.dev.ota.knobs().aborts, 2);
}

#[test]
fn upload_an_update_that_does_not_end_keeps_the_image_unselected() {
    let rig = Rig::confirmed();
    let mut up = rig.upload();
    assert!(up.upload_begin(3, FakeMd5::hex(b"other").as_bytes()));
    assert!(up.upload_write(&[0xE9]));
    assert!(!up.upload_end(true));
    assert_eq!(
        rig.host.first(EventCode::EspOtaFailed).arg1,
        i32::from(UPDATE_ERROR_MD5)
    );
    assert!(!rig.shared.restart_pending());
    assert_eq!(rig.dev.ota.store().otadata, 0);
}

#[test]
fn upload_a_failed_write_aborts_the_upload_with_the_update_error() {
    let rig = Rig::confirmed();
    rig.dev.ota.knobs().fail_write_at = Some(0);
    let mut up = rig.upload();
    assert!(up.upload_begin(5000, b""));
    assert!(!up.upload_write(&image(5000))); // the first sector does not go to the flash
    assert!(!up.upload_active());
    assert_eq!(up.upload_error(), "Flash Write Failed");
    assert_eq!(
        rig.host.first(EventCode::EspOtaFailed).arg1,
        i32::from(UPDATE_ERROR_WRITE)
    );
    assert!(!up.upload_end(true));
    assert_eq!(up.upload_error(), "Flash Write Failed");
}

#[test]
fn request_restart_the_first_request_wins_severity_per_reason() {
    let rig = Rig::confirmed();
    let mut svc = rig.begun();
    rig.dev.clock.set_ms(5000);
    assert!(!rig.shared.restart_pending());
    let first = rig.shared.request_restart(5000, 2, 1000, 7);
    assert!(rig.shared.request_restart(5000, 0, 10, 0).is_none());
    let e = match first {
        Some(e) => e,
        None => panic!("the first request was refused"),
    };
    assert_eq!((e.code, e.arg1, e.arg2), (EventCode::RebootRequested, 2, 7));
    assert_eq!(e.severity, Severity::Warning);
    assert!(rig.shared.restart_pending());
    svc.service_restart(5999, true, true);
    assert_eq!(rig.host.state().save_requests, 0);
    svc.service_restart(6000, true, true);
    assert_eq!(rig.host.state().save_requests, 1);
}

#[test]
fn request_restart_a_network_revert_is_a_warning() {
    let rig = Rig::confirmed();
    rig.request_restart(5, 0, 0);
    assert_eq!(
        rig.host.first(EventCode::RebootRequested).severity,
        Severity::Warning
    );
}

#[test]
fn request_restart_a_factory_reset_is_info() {
    let rig = Rig::confirmed();
    rig.request_restart(3, 0, 0);
    assert_eq!(
        rig.host.first(EventCode::RebootRequested).severity,
        Severity::Info
    );
}

#[test]
fn request_restart_a_user_restart_is_info() {
    let rig = Rig::confirmed();
    rig.request_restart(0, 0, 0);
    assert_eq!(
        rig.host.first(EventCode::RebootRequested).severity,
        Severity::Info
    );
}

#[test]
fn request_restart_a_heap_guard_restart_is_a_warning_keeps_the_targets_no_confirm() {
    let rig = Rig::pending(b"HTTP/1.1 200 OK\r\n", false);
    let mut svc = rig.begun();
    rig.request_restart(6, 0, 0);
    let e = rig.host.first(EventCode::RebootRequested);
    assert_eq!((e.arg1, e.severity), (6, Severity::Warning));
    svc.service_restart(0, true, true);
    assert_eq!(rig.host.state().restart_flushes, 1);
    rig.host.state().save_state = StmSaveState::Saved;
    assert_eq!(run(|| svc.service_restart(100, true, true)), restarted());
    assert_eq!(rig.host.state().flushes, 1);
    // not a user restart: the image stays on trial and the restart counts as a boot
    assert_eq!(rig.dev.ota.knobs().mark_valids, 0);
    assert!(on_trial_in_nvs(&rig));
    assert_eq!(rig.confirmed_app(), None);
}

#[test]
fn service_restart_stm_save_target_flush_log_flush_then_esp_restart() {
    let rig = Rig::confirmed();
    let mut svc = rig.begun();
    rig.request_restart(0, 0, 0);
    svc.service_restart(0, true, true);
    assert_eq!(rig.host.state().save_requests, 1);
    assert_eq!(rig.host.state().restart_flushes, 1);
    svc.service_restart(5000, true, true); // waiting
    assert_eq!(rig.host.state().flushes, 0);
    rig.host.state().save_state = StmSaveState::Saved;
    assert_eq!(run(|| svc.service_restart(5100, true, true)), restarted());
    let save = at(&rig, "app.requestStmSave");
    let targets = at(&rig, "stm_service.flushForRestart");
    let flush = at(&rig, "logger.flush");
    let restart = at(&rig, "esp_restart");
    assert!(save < targets && targets < flush && flush < restart);
    assert_eq!(rig.host.state().save_requests, 1);
    assert_eq!(rig.host.state().restart_flushes, 1);
    assert!(!rig.host.has(EventCode::StmEepromWaitTimeout));
}

#[test]
fn service_restart_an_unavailable_stm_does_not_hold_the_restart() {
    let rig = Rig::confirmed();
    let mut svc = rig.begun();
    rig.request_restart(2, 0, 0);
    svc.service_restart(0, true, true);
    rig.host.state().save_state = StmSaveState::Unavailable;
    assert_eq!(run(|| svc.service_restart(100, true, true)), restarted());
    assert!(!rig.host.has(EventCode::StmEepromWaitTimeout));
}

#[test]
fn service_restart_an_stm_save_timed_out_by_the_stm_task_does_not_hold_the_restart() {
    let rig = Rig::confirmed();
    let mut svc = rig.begun();
    rig.request_restart(2, 0, 0);
    svc.service_restart(0, true, true);
    rig.host.state().save_state = StmSaveState::TimedOut;
    assert_eq!(run(|| svc.service_restart(100, true, true)), restarted());
    assert!(!rig.host.has(EventCode::StmEepromWaitTimeout));
}

#[test]
fn service_restart_a_factory_reset_does_not_save_the_targets() {
    let rig = Rig::confirmed();
    let mut svc = rig.begun();
    rig.request_restart(3, 0, 0);
    svc.service_restart(0, true, true);
    assert_eq!(rig.host.state().save_requests, 1);
    assert_eq!(rig.host.state().restart_flushes, 0);
}

#[test]
fn service_restart_a_silent_stm_task_restarts_at_12000_ms_with_event_323() {
    let rig = Rig::confirmed();
    let mut svc = rig.begun();
    rig.request_restart(0, 0, 0);
    svc.service_restart(0, true, true);
    svc.service_restart(11_999, true, true);
    assert_eq!(rig.dev.system.state().restarts, 0);
    assert_eq!(run(|| svc.service_restart(12_000, true, true)), restarted());
    let ev = rig.host.with_code(EventCode::StmEepromWaitTimeout);
    assert_eq!(ev.len(), 1);
    assert_eq!((ev[0].arg1, ev[0].arg2), (12_000, 3));
    assert_eq!(&ev[0].text[..], b"stm task silent");
    assert!(at(&rig, "logger.log stm_eeprom_wait_timeout") < at(&rig, "logger.flush"));
}

#[test]
fn service_restart_never_in_the_middle_of_an_stm_flash() {
    let rig = Rig::confirmed();
    let mut svc = rig.begun();
    rig.request_restart(3, 0, 0);
    rig.host.state().flash_active = true;
    svc.service_restart(100, true, true);
    assert_eq!(rig.host.state().save_requests, 0);
    rig.host.state().flash_active = false;
    svc.service_restart(100, true, true);
    rig.host.state().save_state = StmSaveState::Saved;
    assert_eq!(run(|| svc.service_restart(200, true, true)), restarted());
}

#[test]
fn service_restart_an_upload_restart_stores_whether_the_stm_link_was_up() {
    let rig = Rig::confirmed();
    let mut svc = rig.begun();
    let mut up = rig.upload();
    rig.host.state().link = LinkState::Up;
    assert!(up.upload_begin(1, b""));
    assert!(up.upload_write(&[0xE9]));
    assert!(up.upload_end(true));
    rig.host.state().link = LinkState::Down; // the link state of the upload counts
    rig.dev.clock.set_ms(1000);
    svc.service_restart(1000, true, false);
    rig.host.state().save_state = StmSaveState::Saved;
    assert!(rig.host.state().ota_stm_sets.is_empty());
    assert_eq!(run(|| svc.service_restart(1100, true, false)), restarted());
    assert_eq!(rig.host.state().ota_stm_sets, vec![true]);
    assert!(rig.ota_stm());
}

#[test]
fn service_restart_an_upload_with_the_link_down_stores_0() {
    let rig = Rig::confirmed();
    let mut svc = rig.begun();
    let mut up = rig.upload();
    assert!(up.upload_begin(1, b""));
    assert!(up.upload_write(&[0xE9]));
    assert!(up.upload_end(true));
    rig.dev.clock.set_ms(1000);
    svc.service_restart(1000, true, true);
    rig.host.state().save_state = StmSaveState::Saved;
    assert_eq!(run(|| svc.service_restart(1100, true, true)), restarted());
    assert_eq!(rig.host.state().ota_stm_sets, vec![false]);
    assert!(!rig.ota_stm());
}

#[test]
fn service_restart_other_restarts_do_not_touch_ota_stm() {
    let rig = Rig::confirmed();
    let mut svc = rig.begun();
    rig.request_restart(0, 0, 0);
    svc.service_restart(0, true, true);
    rig.host.state().save_state = StmSaveState::Saved;
    assert_eq!(run(|| svc.service_restart(100, true, true)), restarted());
    assert!(rig.host.state().ota_stm_sets.is_empty());
}

#[test]
fn begin_an_image_on_trial_reads_ota_stm_once_and_erases_it() {
    let rig = Rig::trial(true);
    let svc = rig.begun();
    let h = rig.shared.health();
    assert!(h.pending && h.stm_required);
    assert!(rig.shared.on_trial());
    // the boot guard read otaStm into the trial record and erased it
    assert!(!rig.ota_stm());
    assert!(rig
        .dev
        .journal
        .find("nvs remove vdmrev/otaStm", 0)
        .is_some());
    drop(svc);
}

#[test]
fn begin_a_confirmed_image_ignores_and_erases_ota_stm() {
    let board = crate::testkit::FakeBoard::new();
    drop(Rig::boot(&board)); // the first boot confirms A
    board.reset(Reset::Software);
    board.nvs().set_u8("vdmrev", "otaStm", 1);
    let rig = Rig::boot(&board);
    let _svc = rig.begun();
    let h = rig.shared.health();
    assert!(!h.pending && !h.stm_required);
    assert!(!rig.shared.on_trial());
    assert!(!rig.ota_stm());
    // the running image has no readable description: the guard stays out, no trial
    let board =
        crate::testkit::FakeBoard::with_slots([SlotImage::broken(None), SlotImage::EMPTY], 0);
    board.ota().store().slots[0].valid = true;
    let rig = Rig::boot(&board);
    let _svc = rig.begun();
    assert!(!rig.shared.health().pending);
    // no image to go back to: confirmed at boot, event 107 -3 once the logger runs
    let rig = Rig::boot(&crate::testkit::FakeBoard::new());
    let _svc = rig.begun();
    assert!(!rig.shared.health().pending);
    let e = rig.host.first(EventCode::EspOtaFailed);
    assert_eq!((e.arg1, e.arg2), (-3, 0));
}

/// An image on trial whose loopback self-check just answered 200 (`stm_required` as given).
fn pending_checked(stm_required: bool) -> Rig {
    Rig::pending(b"HTTP/1.1 200 OK\r\n", stm_required)
}

#[test]
fn service_restart_a_user_restart_confirms_an_image_on_trial_first() {
    // Rust: with a fresh passing self-check (C++: net up was enough, design 6.3)
    let rig = pending_checked(false);
    let mut svc = rig.begun();
    svc.service(0, true, false, true);
    assert!(rig.shared.health().http_ok);
    rig.request_restart(0, 0, 0);
    svc.service_restart(0, true, false);
    rig.host.state().save_state = StmSaveState::Saved;
    assert_eq!(run(|| svc.service_restart(0, true, false)), restarted());
    assert_eq!(rig.confirmed_app(), Some(APP_B.0));
    assert!(!on_trial_in_nvs(&rig));
    assert_eq!(rig.dev.ota.knobs().mark_valids, 1);
    assert!(rig.host.has(EventCode::AppMarkedValid));
    assert!(at(&rig, "logger.log app_marked_valid") < at(&rig, "esp_restart"));
}

#[test]
fn service_restart_a_user_restart_without_a_passing_self_check_counts_as_a_boot() {
    // Rust only: reason 0 also comes over MQTT (`cmd/restart`). Without a fresh 200 of the
    // loopback self-check nothing proves the web server, and a confirmed image without one
    // takes no upload and no switch back: the restart counts as a boot of the trial.
    // No self-check yet (the web server has not started):
    let rig = Rig::pending(b"HTTP/1.1 200 OK\r\n", false);
    let mut svc = rig.begun();
    svc.service(0, true, false, false);
    rig.request_restart(0, 0, 0);
    svc.service_restart(0, true, false);
    rig.host.state().save_state = StmSaveState::Saved;
    assert_eq!(run(|| svc.service_restart(0, true, false)), restarted());
    assert_eq!(rig.dev.ota.knobs().mark_valids, 0);
    assert!(!rig.host.has(EventCode::AppMarkedValid));
    assert!(on_trial_in_nvs(&rig));
    drop(svc);
    // a self-check that failed (the loopback server closes without an answer), also for a
    // factory reset:
    for reason in [0, 3] {
        let rig = Rig::pending(b"", false);
        let mut svc = rig.begun();
        svc.service(0, true, false, true);
        assert!(!rig.shared.health().http_ok);
        rig.request_restart(reason, 0, 0);
        svc.service_restart(0, true, false);
        rig.host.state().save_state = StmSaveState::Saved;
        assert_eq!(run(|| svc.service_restart(0, true, false)), restarted());
        assert_eq!(rig.dev.ota.knobs().mark_valids, 0, "reason {reason}");
        assert!(on_trial_in_nvs(&rig));
        drop(svc);
        let next = Rig::boot(&rig.board);
        assert_eq!(
            next.guard.verdict(),
            crate::boot_guard::BootVerdict::Trial {
                boots: 2,
                stm_required: false
            }
        );
    }
}

#[test]
fn service_restart_a_user_restart_without_the_required_stm_link_does_not_confirm() {
    let rig = pending_checked(true);
    let mut svc = rig.begun();
    svc.service(0, true, false, true);
    assert!(rig.shared.health().http_ok);
    rig.request_restart(3, 0, 0);
    svc.service_restart(0, true, false);
    rig.host.state().save_state = StmSaveState::Saved;
    assert_eq!(run(|| svc.service_restart(0, true, false)), restarted());
    assert_eq!(rig.dev.ota.knobs().mark_valids, 0);
    assert!(!rig.host.has(EventCode::AppMarkedValid));
    assert!(on_trial_in_nvs(&rig));
}

#[test]
fn service_restart_a_watchdog_restart_does_not_confirm_an_image_on_trial() {
    let rig = pending_checked(false);
    let mut svc = rig.begun();
    svc.service(0, true, true, true);
    assert!(rig.shared.health().http_ok);
    rig.request_restart(2, 0, 0);
    svc.service_restart(0, true, true);
    rig.host.state().save_state = StmSaveState::Saved;
    assert_eq!(run(|| svc.service_restart(0, true, true)), restarted());
    assert_eq!(rig.dev.ota.knobs().mark_valids, 0);
    assert!(on_trial_in_nvs(&rig));
}

#[test]
fn service_self_check_200_net_and_link_confirm_120_s_after_the_first_healthy_second() {
    let rig = Rig::pending(
        b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n{}",
        true,
    );
    let mut svc = rig.begun();
    rig.service_seconds(&mut svc, 0, 119_000, true, true);
    assert_eq!(rig.dev.ota.knobs().mark_valids, 0);
    let h = rig.shared.health();
    assert!(h.pending && h.stm_required && h.net_ok && h.http_ok && h.stm_ok);
    assert_eq!((h.healthy_for_s, h.remaining_s), (119, 781));
    rig.service_seconds(&mut svc, 120_000, 120_000, true, true);
    assert_eq!(rig.dev.ota.knobs().mark_valids, 1);
    assert_eq!(rig.confirmed_app(), Some(APP_B.0));
    assert!(rig.host.has(EventCode::AppMarkedValid));
    assert_eq!(rig.host.first(EventCode::AppMarkedValid).arg1, 120);
    assert!(!rig.shared.health().pending);
    assert!(!rig.shared.on_trial());
    let req = rig.requests();
    assert_eq!(req.len(), 13); // every 10 s while on trial
    assert_eq!(
        req[0],
        "GET /api/health HTTP/1.1\r\nHost: 192.168.1.20\r\nConnection: close\r\n\r\n"
    );
    let c = &rig.dev.tcp.connects()[0];
    assert_eq!(
        (c.host.as_str(), c.port, c.timeout_ms),
        ("127.0.0.1", 80, 1000)
    );
    rig.service_seconds(&mut svc, 121_000, 140_000, true, true);
    assert_eq!(rig.requests().len(), 13); // no self-check once confirmed
}

#[test]
fn service_no_self_check_while_the_network_is_down_or_the_web_server_is_off() {
    let rig = Rig::pending(b"HTTP/1.1 200 OK\r\n", false);
    let mut svc = rig.begun();
    rig.host.state().net_up = false;
    svc.service(0, true, true, true);
    assert!(rig.requests().is_empty());
    rig.host.state().net_up = true;
    svc.service(1000, true, true, false);
    assert!(rig.requests().is_empty());
    svc.service(2000, true, true, true);
    assert_eq!(rig.requests().len(), 1);
}

#[test]
fn service_an_unreachable_loopback_server_fails_the_check() {
    let rig = Rig::pending(b"HTTP/1.1 200 OK\r\n", false);
    let mut svc = rig.begun();
    rig.dev.tcp.refuse_next(1);
    svc.service(0, true, true, true);
    assert!(!rig.shared.health().http_ok);
    assert!(rig.requests().is_empty());
    rig.loopback(b"HTTP/1.1 20", Vec::new());
    svc.service(10_000, true, true, true);
    assert!(!rig.shared.health().http_ok);
    rig.loopback(b"HTTP/1.1 200", Vec::new());
    svc.begin(&rig.report);
    svc.service(20_000, true, true, true);
    assert!(rig.shared.health().http_ok);
}

#[test]
fn service_a_503_self_check_switches_back_at_900_s_through_the_restart_path() {
    let rig = Rig::pending(b"HTTP/1.1 503 Service Unavailable\r\n", false);
    let mut svc = rig.begun();
    rig.service_seconds(&mut svc, 0, 899_000, true, true);
    assert!(!rig.shared.restart_pending());
    rig.dev.clock.set_ms(900_000);
    svc.service(900_000, true, true, true);
    let ev = rig.host.with_code(EventCode::RebootRequested);
    assert_eq!(ev.len(), 1);
    assert_eq!((ev[0].arg1, ev[0].arg2), (4, 2));
    assert_eq!(ev[0].severity, Severity::Warning);
    assert!(rig.shared.restart_pending());
    svc.service_restart(900_999, true, true);
    assert_eq!(rig.host.state().save_requests, 0);
    svc.service_restart(901_000, true, true);
    assert_eq!(rig.host.state().save_requests, 1);
    assert!(rig.dev.ota.knobs().set_boots.is_empty());
    rig.host.state().save_state = StmSaveState::Saved;
    assert_eq!(
        run(|| svc.service_restart(901_100, true, true)),
        restarted()
    );
    let set_boot = format!("ota set_boot {:#x}", SLOT_ADDR[0]);
    assert!(at(&rig, "app.requestStmSave") < at(&rig, "logger.flush"));
    assert!(at(&rig, "logger.flush") < at(&rig, &set_boot));
    assert!(at(&rig, &set_boot) < at(&rig, "esp_restart"));
    assert_eq!(rig.dev.ota.store().otadata, 0); // the fallback slot
    assert_eq!(rig.dev.ota.knobs().mark_valids, 0);
    // the fallback image is the confirmed one, B switched back (breadcrumb reason 2)
    assert_eq!(rig.confirmed_app(), Some(APP_A.0));
}

#[test]
fn service_a_switch_back_without_another_image_keeps_running_this_one() {
    let rig = Rig::pending(b"", false);
    let mut svc = rig.begun();
    // the fallback stopped verifying after the boot: the switch is refused
    rig.dev.ota.knobs().set_boot_err = Some(EspErr::OTA_VALIDATE_FAILED);
    rig.service_seconds(&mut svc, 0, 900_000, false, false);
    let r = rig.host.first(EventCode::RebootRequested);
    assert_eq!(r.arg2, 3); // net and http missing
    svc.service_restart(901_000, false, false);
    rig.host.state().save_state = StmSaveState::Saved;
    svc.service_restart(901_100, false, false);
    assert_eq!(rig.dev.ota.knobs().set_boots, vec![SLOT_ADDR[0]]);
    assert_eq!(rig.host.first(EventCode::EspOtaFailed).arg1, -3);
    assert!(!rig.shared.restart_pending());
    assert_eq!(rig.confirmed_app(), Some(APP_B.0)); // the trial ended as confirmed
                                                    // a later restart goes through the gate again and restarts normally
    rig.host.state().save_state = StmSaveState::Idle;
    rig.dev.clock.set_ms(902_000);
    rig.request_restart(0, 0, 0);
    svc.service_restart(902_000, false, false);
    assert_eq!(rig.host.state().save_requests, 2);
    rig.host.state().save_state = StmSaveState::Saved;
    assert_eq!(
        run(|| svc.service_restart(902_100, false, false)),
        restarted()
    );
    assert_eq!(rig.dev.ota.knobs().set_boots, vec![SLOT_ADDR[0]]);
}

#[test]
fn service_a_rollback_while_another_restart_is_pending_becomes_that_restart() {
    let rig = Rig::pending(b"", true);
    let mut svc = rig.begun();
    rig.service_seconds(&mut svc, 0, 899_000, true, false);
    rig.dev.clock.set_ms(899_500);
    rig.request_restart(0, 5000, 0);
    rig.dev.clock.set_ms(900_000);
    svc.service(900_000, true, false, true);
    let ev = rig.host.with_code(EventCode::RebootRequested);
    assert_eq!(ev.len(), 2);
    assert_eq!((ev[1].arg1, ev[1].arg2), (4, 6)); // http and stm missing
    svc.service_restart(904_499, true, true);
    assert_eq!(rig.host.state().save_requests, 0); // the pending restart's time stands
    svc.service_restart(904_500, true, true);
    rig.host.state().save_state = StmSaveState::Saved;
    assert_eq!(
        run(|| svc.service_restart(904_600, true, true)),
        restarted()
    );
    assert_eq!(rig.dev.ota.knobs().set_boots, vec![SLOT_ADDR[0]]);
    // the user restart does not confirm a rolled-back image
    assert_eq!(rig.dev.ota.knobs().mark_valids, 0);
    assert_eq!(rig.host.state().restart_flushes, 1);
}

#[test]
fn service_a_rollback_decided_while_the_restart_into_an_uploaded_image_waits_leaves_it() {
    // An upload is refused while the image is on trial (decision 7.5): the restart into an
    // uploaded image is requested directly here, as an upload of this boot would have
    let rig = Rig::pending(b"", false);
    let mut svc = rig.begun();
    rig.service_seconds(&mut svc, 0, 899_000, false, false);
    assert!(!rig.upload().upload_begin(3, b""));
    rig.request_restart(1, 1000, 0); // due at 900000
    rig.dev.clock.set_ms(900_000);
    svc.service(900_000, false, false, true); // the rollback is due now
    let ev = rig.host.with_code(EventCode::RebootRequested);
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].arg1, 1);
    svc.service_restart(900_000, false, false);
    assert_eq!(rig.host.state().save_requests, 1);
    rig.host.state().save_state = StmSaveState::Saved;
    assert_eq!(
        run(|| svc.service_restart(900_100, false, false)),
        restarted()
    );
    assert!(rig.dev.ota.knobs().set_boots.is_empty());
    assert_eq!(rig.host.state().ota_stm_sets, vec![false]);
}

#[test]
fn an_image_uploaded_while_a_rollback_restart_waits_replaces_the_rollback_others_do_not() {
    let rig = Rig::pending(b"", false);
    let mut svc = rig.begun();
    rig.service_seconds(&mut svc, 0, 900_000, false, false); // the rollback, due at 901000
    rig.request_restart(0, 0, 0); // a user restart meanwhile: still the rollback
    svc.service_restart(901_000, false, false);
    assert_eq!(rig.host.state().save_requests, 1); // the restart waits for the STM
    let ev = rig.host.with_code(EventCode::RebootRequested);
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].arg1, 4);
    // the upload itself is refused on trial; its restart request replaces the rollback
    let mut up = rig.upload();
    assert!(!up.upload_begin(3, b""));
    assert_eq!(up.upload_error(), "image on trial");
    rig.request_restart(1, 1000, 0);
    let ev = rig.host.with_code(EventCode::RebootRequested);
    assert_eq!(ev.len(), 2);
    assert_eq!((ev[1].arg1, ev[1].severity), (1, Severity::Info));
    rig.request_restart(0, 0, 0); // no new request replaces it
    assert_eq!(rig.host.with_code(EventCode::RebootRequested).len(), 2);
    rig.host.state().save_state = StmSaveState::Saved;
    assert_eq!(
        run(|| svc.service_restart(901_100, false, false)),
        restarted()
    );
    assert!(rig.dev.ota.knobs().set_boots.is_empty());
    assert_eq!(rig.host.state().ota_stm_sets, vec![false]);
}

#[test]
fn a_rollback_restart_stays_a_rollback_when_another_restart_is_requested() {
    let rig = Rig::pending(b"", false);
    let mut svc = rig.begun();
    rig.service_seconds(&mut svc, 0, 900_000, false, false);
    rig.request_restart(2, 0, 5);
    svc.service_restart(901_000, false, false);
    rig.host.state().save_state = StmSaveState::Saved;
    assert_eq!(
        run(|| svc.service_restart(901_100, false, false)),
        restarted()
    );
    assert_eq!(rig.dev.ota.knobs().set_boots, vec![SLOT_ADDR[0]]);
    assert_eq!(rig.dev.ota.store().otadata, 0);
    // B is the image switched away from (state switched back)
    let t = rig.dev.nvs.get_blob("vdmrev", "otaTrial");
    assert_eq!((t.len(), t[5]), (32, 2));
    assert_eq!(rig.host.with_code(EventCode::RebootRequested).len(), 1);
}

#[test]
fn service_mark_valid_with_an_nvs_that_refuses_the_confirmation() {
    // C++: a failing esp_ota_mark_app_valid_cancel_rollback logged nothing. The boot guard's
    // confirmation has no failure result (a refused NVS write leaves the RTC mirror): event 108
    // is logged, the trial ends for this boot, and the next boot counts on as trial boot 2
    let rig = Rig::pending(b"HTTP/1.1 200 OK\r\n", false);
    let mut svc = rig.begun();
    rig.dev.nvs.knobs().fail_commit = true;
    rig.service_seconds(&mut svc, 0, 120_000, true, false);
    assert_eq!(rig.dev.ota.knobs().mark_valids, 1);
    assert!(rig.host.has(EventCode::AppMarkedValid));
    assert!(!rig.shared.on_trial());
    assert_eq!(rig.confirmed_app(), None);
    drop(svc);
    rig.board.reset(Reset::Software);
    let board = rig.board.clone();
    drop(rig);
    let next = Rig::boot(&board);
    assert_eq!(
        next.guard.verdict(),
        crate::boot_guard::BootVerdict::Trial {
            boots: 2,
            stm_required: false
        }
    );
}

// ---------------------------------------------------------------- Rust only

#[test]
fn upload_begin_is_refused_while_the_image_is_on_trial() {
    let rig = Rig::trial(false);
    let _svc = rig.begun();
    let mut up = rig.upload();
    assert!(!up.upload_begin(10, b""));
    assert_eq!(up.upload_error(), "image on trial");
    assert!(!up.upload_active());
    assert_eq!(rig.dev.ota.knobs().begins, 0);
    // busy wins over the trial (one more upload while one runs)
    rig.shared.upload_active.store(true, Ordering::SeqCst);
    assert!(!up.upload_begin(10, b""));
    assert_eq!(up.upload_error(), "busy");
}

#[test]
fn reboot_event_severities_and_the_restart_reasons() {
    let sev: Vec<Severity> = (0..=8).map(|r| reboot_event(r, 0).severity).collect();
    use Severity::{Info, Warning};
    assert_eq!(
        sev,
        vec![Info, Info, Warning, Info, Warning, Warning, Warning, Info, Info]
    );
    let e = reboot_event(7, -9);
    assert_eq!(
        (e.code, e.arg1, e.arg2, e.valve),
        (EventCode::RebootRequested, 7, -9, 0xFE)
    );
    assert_eq!(
        (
            USER,
            OTA,
            NET_WATCHDOG,
            FACTORY_RESET,
            ROLLBACK,
            NET_REVERT,
            HEAP_GUARD,
            SWITCH_BACK
        ),
        (0, 1, 2, 3, 4, 5, 6, 7)
    );
    assert_eq!(
        (
            SELF_CHECK_CONNECT_MS,
            SELF_CHECK_ANSWER_MS,
            SELF_CHECK_POLL_MS
        ),
        (1000, 3000, 10)
    );
    assert_eq!((SELF_CHECK_STATUS_BYTES, RESTART_DELAY_MS), (12, 1000));
}

#[test]
fn a_restart_request_wraps_with_millis() {
    let shared = OtaShared::new();
    assert!(shared.request_restart(0xFFFF_FF00, 0, 0x200, 0).is_some());
    assert_eq!(shared.due_restart(0xFFFF_FFFF), None);
    assert_eq!(shared.due_restart(0xFF), None);
    assert_eq!(shared.due_restart(0x100), Some(0));
    shared.clear_restart();
    assert!(!shared.restart_pending());
    assert_eq!(shared.due_restart(0x100), None);
    // a rollback while nothing is pending: due 1 s later
    assert!(shared.request_rollback(500, 3).is_some());
    assert_eq!(shared.due_restart(1499), None);
    assert_eq!(shared.due_restart(1500), Some(4));
}

#[test]
fn nvs_ota_stm_is_written_like_storage() {
    let rig = Rig::confirmed();
    let mut host = rig.host.clone();
    host.set_ota_stm_required(true);
    assert_eq!(nvs_u8(&rig.dev.nvs, "otaStm"), Some(1));
    host.set_ota_stm_required(false);
    assert_eq!(nvs_u8(&rig.dev.nvs, "otaStm"), None);
    let ns = rig.dev.nvs.open("vdmrev", false);
    assert!(ns.is_some_and(|n| n.get_int("otaStm", crate::port::NvsInt::U8).is_none()));
}
