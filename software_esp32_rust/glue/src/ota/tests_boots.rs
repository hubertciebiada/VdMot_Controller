//! Multi-boot scenarios of ota with the boot guard (new): an upload that boots on trial and is
//! confirmed by its health, a trial that fails its health and switches back (the breadcrumb in
//! RTC memory reaches the log of the fallback image), crashes of an image on trial, restarts
//! during a trial, and the manual switch back (restart reason 7).
#![allow(clippy::large_stack_frames)]

use super::support::*;
use super::*;
use crate::boot_guard::{BootVerdict, GuardEvent, SwitchReason};
use crate::port::EspErr;
use crate::testkit::board::{APP_A, APP_B};
use crate::testkit::ota::SLOT_ADDR;
use crate::testkit::{run, Ended, FakeBoard, FakeMd5, Reset, SlotImage};

fn restarted() -> Ended<()> {
    Ended::Reset(Reset::Software)
}

fn trial(boots: u8, stm_required: bool) -> BootVerdict {
    BootVerdict::Trial {
        boots,
        stm_required,
    }
}

/// The network up at the device address and the loopback server answering 200.
fn healthy_network(rig: &Rig) {
    {
        let mut s = rig.host.state();
        s.net_up = true;
        s.net_ip = DEVICE_IP;
    }
    rig.loopback(b"HTTP/1.1 200 OK\r\n", Vec::new());
}

/// The restart path of `svc` from `now`: the STM save, then the restart (or the switch).
fn restart_path(rig: &Rig, svc: &mut OtaService<'_, TestPlatform, FakeOtaHost>, now: u32) {
    svc.service_restart(now, true, true);
    rig.host.state().save_state = StmSaveState::Saved;
    assert_eq!(run(|| svc.service_restart(now, true, true)), restarted());
}

use crate::testkit::board::TestPlatform;

/// Ends `rig` and boots the board again.
fn reboot(rig: Rig) -> Rig {
    let board = rig.board.clone();
    drop(rig);
    Rig::boot(&board)
}

#[test]
fn an_uploaded_image_boots_on_trial_proves_its_health_and_is_confirmed() {
    let rig = Rig::confirmed();
    rig.host.state().link = LinkState::Up;
    let mut svc = rig.begun();
    let mut up = rig.upload();
    rig.dev.ota.knobs().next_app = Some(APP_B);
    let img = image(10_000);
    assert!(up.upload_begin(img.len() + 300, FakeMd5::hex(&img).as_bytes()));
    for chunk in img.chunks(1460) {
        assert!(up.upload_write(chunk));
    }
    assert!(up.upload_end(true));
    rig.dev.clock.set_ms(1000);
    restart_path(&rig, &mut svc, 1000);
    assert!(rig.ota_stm()); // the STM link was up at the upload
    drop(up);
    drop(svc);
    // B on trial, the STM link required
    let rig = reboot(rig);
    assert_eq!(rig.dev.ota.running().app, Some(APP_B));
    assert_eq!(rig.guard.verdict(), trial(1, true));
    assert!(!rig.ota_stm()); // read once into the trial record
    healthy_network(&rig);
    let mut svc = rig.begun();
    assert!(rig.shared.on_trial());
    rig.service_seconds(&mut svc, 0, 119_000, true, true);
    assert!(rig.shared.on_trial());
    rig.service_seconds(&mut svc, 120_000, 120_000, true, true);
    assert_eq!(rig.confirmed_app(), Some(APP_B.0));
    assert_eq!(rig.host.first(EventCode::AppMarkedValid).arg1, 120);
    assert!(!rig.shared.on_trial());
    assert!(rig.upload().upload_begin(10, b"")); // uploads are allowed again
    drop(svc);
    rig.board.reset(Reset::Software);
    let rig = reboot(rig);
    assert_eq!(rig.guard.verdict(), BootVerdict::Confirmed);
    let _svc = rig.begun();
    assert!(rig.host.state().events.is_empty());
}

#[test]
fn an_image_that_never_gets_healthy_is_switched_back_and_the_old_image_logs_why() {
    let rig = Rig::trial(false);
    let mut svc = rig.begun();
    rig.service_seconds(&mut svc, 0, 900_000, false, false);
    assert_eq!(rig.host.first(EventCode::RebootRequested).arg1, 4);
    restart_path(&rig, &mut svc, 901_000);
    assert_eq!(rig.dev.ota.knobs().set_boots, vec![SLOT_ADDR[0]]);
    drop(svc);
    let rig = reboot(rig);
    assert_eq!(rig.dev.ota.running().app, Some(APP_A));
    assert_eq!(rig.guard.verdict(), BootVerdict::Confirmed);
    let _svc = rig.begun();
    let e = rig.host.first(EventCode::EspOtaFailed);
    assert_eq!((e.arg1, e.arg2), (-4, 2)); // the previous image failed its health checks
    assert_eq!(
        GuardEvent::PreviousImageFailed(SwitchReason::Health).args(),
        (-4, 2)
    );
}

#[test]
fn three_crashes_of_an_image_on_trial_switch_back_before_its_fourth_start() {
    let board = FakeBoard::with_slots([SlotImage::glue(APP_A), SlotImage::glue(APP_B)], 1);
    for boots in 1..=3 {
        let rig = Rig::boot(&board);
        assert_eq!(rig.guard.verdict(), trial(boots, false));
        let _svc = rig.begun();
        board.reset(Reset::Panic); // B crashes before its confirmation
    }
    let dev = board.boot();
    let r = run(|| BootGuard::boot(&dev.nvs, &dev.ota, &dev.rtc, &dev.system));
    assert_eq!(r, Ended::Reset(Reset::Software)); // the guard switched in main
    drop(dev);
    let rig = Rig::boot(&board);
    assert_eq!(rig.dev.ota.running().app, Some(APP_A));
    let _svc = rig.begun();
    let e = rig.host.first(EventCode::EspOtaFailed);
    assert_eq!((e.arg1, e.arg2), (-4, 1)); // the boot limit
}

#[test]
fn a_watchdog_restart_during_the_trial_counts_as_a_boot() {
    let rig = Rig::trial(false);
    let mut svc = rig.begun();
    rig.request_restart(2, 1000, 10);
    rig.dev.clock.set_ms(1000);
    restart_path(&rig, &mut svc, 1000);
    drop(svc);
    let rig = reboot(rig);
    assert_eq!(rig.guard.verdict(), trial(2, false));
}

#[test]
fn a_user_restart_during_the_trial_with_the_network_up_confirms_the_image_first() {
    let rig = Rig::trial(false);
    let mut svc = rig.begun();
    rig.request_restart(0, 1000, 0);
    rig.dev.clock.set_ms(1000);
    restart_path(&rig, &mut svc, 1000); // net up, no STM link required
    assert!(rig.host.has(EventCode::AppMarkedValid));
    drop(svc);
    let rig = reboot(rig);
    assert_eq!(rig.dev.ota.running().app, Some(APP_B));
    assert_eq!(rig.guard.verdict(), BootVerdict::Confirmed);
}

#[test]
fn a_manual_switch_back_restarts_a_confirmed_image_into_the_other_one() {
    let board = FakeBoard::new();
    drop(Rig::boot(&board)); // A confirms itself (no other image)
    board.ota().store().slots[1] = SlotImage::glue(APP_B);
    board.reset(Reset::Software);
    let rig = Rig::boot(&board);
    assert_eq!(rig.guard.verdict(), BootVerdict::Confirmed);
    let mut svc = rig.begun();
    rig.request_restart(7, 1000, 0); // POST /api/system/ota/switch-back
    let e = rig.host.first(EventCode::RebootRequested);
    assert_eq!((e.arg1, e.severity), (7, Severity::Info));
    rig.dev.clock.set_ms(1000);
    restart_path(&rig, &mut svc, 1000);
    assert_eq!(rig.dev.ota.knobs().set_boots, vec![SLOT_ADDR[1]]);
    assert!(at_least_once(&rig, "logger.flush"));
    drop(svc);
    let rig = reboot(rig);
    assert_eq!(rig.dev.ota.running().app, Some(APP_B));
    assert_eq!(rig.guard.verdict(), BootVerdict::Confirmed);
    let _svc = rig.begun();
    assert!(rig.host.state().events.is_empty());
}

fn at_least_once(rig: &Rig, entry: &str) -> bool {
    rig.dev.journal.find(entry, 0).is_some()
}

#[test]
fn an_upload_that_ends_while_a_switch_back_waits_replaces_it_and_runs_on_trial() {
    // the switch would select the uploaded slot as the confirmed image without its trial
    let rig = Rig::confirmed();
    let mut svc = rig.begun();
    rig.request_restart(7, 1000, 0);
    let mut up = rig.upload();
    rig.dev.ota.knobs().next_app = Some(APP_B);
    assert!(up.upload_begin(5000, b""));
    assert!(up.upload_write(&image(5000)));
    assert!(up.upload_end(true));
    let reasons: Vec<i32> = rig
        .host
        .with_code(EventCode::RebootRequested)
        .iter()
        .map(|e| e.arg1)
        .collect();
    assert_eq!(reasons, vec![7, 1]);
    rig.request_restart(7, 0, 0); // a switch back does not replace the upload's restart
    assert_eq!(rig.host.with_code(EventCode::RebootRequested).len(), 2);
    rig.dev.clock.set_ms(1000);
    restart_path(&rig, &mut svc, 1000);
    assert!(rig.dev.ota.knobs().set_boots.is_empty()); // a plain restart into the upload
    assert_eq!(rig.confirmed_app(), Some(APP_A.0));
    drop(up);
    drop(svc);
    let rig = reboot(rig);
    assert_eq!(rig.dev.ota.running().app, Some(APP_B));
    assert_eq!(rig.guard.verdict(), trial(1, false)); // with A as its fallback
}

#[test]
fn a_manual_switch_back_during_a_trial_that_cannot_select_the_fallback_ends_the_trial() {
    let rig = Rig::trial(true);
    let mut svc = rig.begun();
    rig.service_seconds(&mut svc, 0, 30_000, true, true);
    assert!(rig.shared.health().pending);
    rig.dev.ota.knobs().set_boot_err = Some(EspErr::OTA_VALIDATE_FAILED);
    rig.request_restart(7, 0, 0);
    svc.service_restart(30_000, true, true);
    rig.host.state().save_state = StmSaveState::Saved;
    svc.service_restart(30_000, true, true); // refused: keeps running
    let e = rig.host.first(EventCode::EspOtaFailed);
    assert_eq!((e.arg1, e.arg2), (-3, 0));
    assert!(!rig.shared.restart_pending());
    assert!(!rig.shared.on_trial());
    assert_eq!(rig.confirmed_app(), Some(APP_B.0)); // the trial ended as confirmed
                                                    // the validation stops with it: no rollback of the confirmed image later
    rig.service_seconds(&mut svc, 31_000, 31_000, false, false);
    let h = rig.shared.health();
    assert!(!h.pending);
    assert!(h.stm_required);
    rig.service_seconds(&mut svc, 32_000, 1_000_000, false, false);
    assert!(!rig.shared.restart_pending());
    assert!(rig.upload().upload_begin(10, b"")); // uploads are allowed again
}
