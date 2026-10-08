//! Boot guard (new: design 6.2, 6.3, 6.5, 6.6), multi-boot against the fake board and its
//! bootloader without rollback support.

use super::*;
use crate::port::EspErr;
use crate::port::SlotInfo;
use crate::testkit::board::{Booted, APP_A, APP_B, APP_C, APP_CPP};
use crate::testkit::ota::SLOT_ADDR;
use crate::testkit::{run, Device, Ended, FakeBoard, FakeNvs, Reset, SlotImage};

type Booting = Ended<(BootGuard, BootReport)>;

fn decide(dev: &Device) -> Booting {
    run(|| BootGuard::boot(&dev.nvs, &dev.ota, &dev.rtc, &dev.system))
}

/// The next boot and its decision.
fn boot(board: &FakeBoard) -> (Device, Booting) {
    let dev = board.boot();
    let r = decide(&dev);
    (dev, r)
}

fn trial(boots: u8, stm_required: bool) -> BootVerdict {
    BootVerdict::Trial {
        boots,
        stm_required,
    }
}

fn events(r: &BootReport) -> Vec<GuardEvent> {
    r.events.iter().flatten().copied().collect()
}

fn ok_record(nvs: &FakeNvs) -> Option<AppId> {
    let b = nvs.get_blob(NVS_NAMESPACE, KEY_OK);
    decode_ok(&b.try_into().ok()?)
}

fn trial_record(nvs: &FakeNvs) -> Option<TrialRecord> {
    let b = nvs.get_blob(NVS_NAMESPACE, KEY_TRIAL);
    decode_trial(&b.try_into().ok()?)
}

fn mirror(dev: &Device) -> Option<Mirror> {
    load_mirror(&dev.rtc)
}

/// A board after the upload of B by a confirmed A: B in slot 1 selected for the next boot.
fn uploaded_b() -> FakeBoard {
    let board = FakeBoard::with_slots([SlotImage::glue(APP_A), SlotImage::glue(APP_B)], 1);
    board
        .nvs()
        .set_blob(NVS_NAMESPACE, KEY_OK, &encode_ok(APP_A));
    board
}

/// Boots B `n` times, each boot ending with `reset`; the last decision.
fn crash_loop(board: &FakeBoard, n: usize, reset: Reset) -> BootReport {
    let mut last = None;
    for _ in 0..n {
        let (dev, r) = boot(board);
        last = Some(r.returned().1);
        assert_eq!(dev.ota.running().app, Some(APP_B));
        board.reset(reset);
    }
    last.unwrap()
}

// ---------------------------------------------------------------- records

#[test]
fn records_round_trip_and_damaged_ones_count_as_absent() {
    assert_eq!(decode_ok(&encode_ok(APP_A)), Some(APP_A));
    let mut ok = encode_ok(APP_A);
    assert_eq!(&ok[..4], b"VDOK");
    ok[5] ^= 1;
    assert_eq!(decode_ok(&ok), None);
    let mut ok = encode_ok(APP_A);
    ok[0] = b'X';
    put_crc(&mut ok);
    assert_eq!(decode_ok(&ok), None);

    let t = TrialRecord {
        state: STATE_SWITCHED_BACK,
        boots: 4,
        stm_required: true,
        app: APP_B,
        away: Some(APP_C),
        fallback: 0x1_0000,
    };
    let r = encode_trial(&t);
    assert_eq!(&r[..8], &[b'V', b'D', b'O', b'T', 1, 2, 4, 3]);
    assert_eq!(&r[24..28], &[0, 0, 1, 0]);
    assert_eq!(decode_trial(&r), Some(t));
    let plain = TrialRecord {
        state: STATE_TRIAL,
        stm_required: false,
        away: None,
        ..t
    };
    let r = encode_trial(&plain);
    assert_eq!((r[7], &r[16..24]), (0, &[0u8; 8][..]));
    assert_eq!(decode_trial(&r), Some(plain));
    for (at, v) in [(0, b'X'), (4, 2), (5, 0), (5, 3), (9, 0x55)] {
        let mut bad = encode_trial(&t);
        bad[at] = v;
        if at != 9 {
            put_crc(&mut bad);
        }
        assert_eq!(decode_trial(&bad), None, "byte {at}");
    }

    let m = Mirror {
        app: APP_B,
        boots: 2,
        reason: 1,
        streak: 0,
        flags: 0,
        away: APP_B,
    };
    let r = encode_mirror(&m);
    assert_eq!(
        (&r[..4], r[12], r[13], r[14], r[15]),
        (&b"VBGD"[..], 2, 1, 0, 0)
    );
    assert_eq!(decode_mirror(&r), Some(m));
    let mut bad = r;
    bad[13] = 2;
    assert_eq!(decode_mirror(&bad), None);
    let mut bad = r;
    bad[0] = 0;
    put_crc(&mut bad);
    assert_eq!(decode_mirror(&bad), None);
    assert_eq!(decode_mirror(&[0xA5; MIRROR_LEN]), None); // power-on garbage
}

#[test]
fn constants_and_events() {
    assert_eq!((BOOT_LIMIT, BOOT_DEADLINE_MS), (3, 60_000));
    assert_eq!(FACTORY_RESET_KEEPS, ["otaOk", "otaTrial"]);
    assert_eq!((MIRROR_OFFSET, MIRROR_LEN), (0, 28));
    let e = GuardEvent::PreviousImageFailed(SwitchReason::Health);
    assert_eq!((e.code(), e.args()), (107, (-4, 2)));
    let e = GuardEvent::PreviousImageFailed(SwitchReason::BootLimit);
    assert_eq!(e.args(), (-4, 1));
    assert_eq!(GuardEvent::NoFallback.args(), (-3, 0));
    assert_eq!(GuardEvent::NoFallback.code(), 107);
}

#[test]
fn plan_steps_2_to_6() {
    let fallback = SlotInfo {
        address: SLOT_ADDR[0],
        size: 0x14_0000,
        app: Some(APP_A),
    };
    let base = Inputs {
        app: APP_B,
        old: None,
        mirror: None,
        stm_flag: true,
        fallback: Some(fallback),
    };
    let new_trial = TrialRecord {
        state: STATE_TRIAL,
        boots: 1,
        stm_required: true,
        app: APP_B,
        away: None,
        fallback: SLOT_ADDR[0],
    };
    assert_eq!(plan(&base), Plan::Trial(new_trial));
    assert_eq!(
        plan(&Inputs {
            fallback: None,
            ..base
        }),
        Plan::NoFallback
    );
    // the running trial keeps its flag and fallback address; the higher count wins
    let running = TrialRecord {
        boots: 2,
        stm_required: false,
        fallback: 0x99,
        ..new_trial
    };
    let mirror = Mirror {
        app: APP_B,
        boots: 1,
        reason: 0,
        streak: 0,
        flags: 0,
        away: AppId([0; 8]),
    };
    let next = Inputs {
        old: Some(running),
        mirror: Some(mirror),
        ..base
    };
    assert_eq!(
        plan(&next),
        Plan::Trial(TrialRecord {
            boots: 3,
            ..running
        })
    );
    let rtc_ahead = Inputs {
        mirror: Some(Mirror { boots: 3, ..mirror }),
        ..next
    };
    assert_eq!(
        plan(&rtc_ahead),
        Plan::Switch {
            record: TrialRecord {
                state: STATE_SWITCHED_BACK,
                boots: 4,
                away: Some(APP_B),
                ..running
            },
            target: Some(APP_A),
        }
    );
    // a mirror of another image, or with a breadcrumb, does not count
    for m in [
        Mirror {
            app: APP_C,
            ..mirror
        },
        Mirror {
            reason: 1,
            ..mirror
        },
    ] {
        let i = Inputs {
            mirror: Some(Mirror { boots: 5, ..m }),
            ..base
        };
        assert_eq!(plan(&i), Plan::Trial(new_trial));
    }
    // a record of another image or a switched-back one starts a new trial; its away id stays
    for old in [
        TrialRecord {
            app: APP_C,
            away: Some(APP_C),
            ..running
        },
        TrialRecord {
            state: STATE_SWITCHED_BACK,
            away: Some(APP_C),
            ..running
        },
    ] {
        let i = Inputs {
            old: Some(old),
            ..base
        };
        assert_eq!(
            plan(&i),
            Plan::Trial(TrialRecord {
                away: Some(APP_C),
                ..new_trial
            })
        );
    }
    // the fallback is the image that failed the last trial
    let i = Inputs {
        old: Some(TrialRecord {
            away: Some(APP_A),
            ..running
        }),
        ..base
    };
    assert_eq!(plan(&i), Plan::NoFallback);
    // boots saturate
    let i = Inputs {
        old: Some(TrialRecord {
            boots: 255,
            ..running
        }),
        ..base
    };
    assert!(matches!(plan(&i), Plan::Switch { record, .. } if record.boots == 255));
}

#[test]
fn a_confirmed_image_runs_without_trial_and_drops_a_stale_record() {
    let board = uploaded_b();
    board.ota().store().otadata = 0;
    let stale = TrialRecord {
        state: STATE_TRIAL,
        boots: 2,
        stm_required: false,
        app: APP_C,
        away: None,
        fallback: SLOT_ADDR[1],
    };
    board
        .nvs()
        .set_blob(NVS_NAMESPACE, KEY_TRIAL, &encode_trial(&stale));
    board.nvs().set_u8(NVS_NAMESPACE, KEY_STM, 1);
    let (dev, r) = boot(&board);
    let (g, report) = r.returned();
    assert_eq!(report.verdict, BootVerdict::Confirmed);
    assert_eq!(events(&report), vec![]);
    assert!(!g.on_trial());
    assert_eq!(g.verdict(), BootVerdict::Confirmed);
    assert_eq!(trial_record(&dev.nvs), None);
    assert!(!dev.nvs.has(NVS_NAMESPACE, KEY_STM)); // erased at every boot
    assert_eq!(ok_record(&dev.nvs), Some(APP_A));
    // F5: the mirror holds the crash streak of the confirmed image
    assert_eq!(
        mirror(&dev),
        Some(Mirror {
            app: APP_A,
            boots: 0,
            reason: 0,
            streak: 0,
            flags: 0,
            away: UNKNOWN_APP,
        })
    );
    assert_eq!(
        dev.journal.entries(),
        vec![
            "nvs remove vdmrev/otaStm",
            "nvs remove vdmrev/otaTrial",
            "ota mark_valid"
        ]
    );
    // the expensive check of the other slot is not needed
    assert_eq!(dev.ota.knobs().verifies, Vec::<u32>::new());
    assert_eq!(dev.ota.knobs().set_boots, Vec::<u32>::new());
}

#[test]
fn a_new_image_starts_its_trial_with_the_uploading_image_as_fallback() {
    let board = uploaded_b();
    board.nvs().set_u8(NVS_NAMESPACE, KEY_STM, 1);
    let (dev, r) = boot(&board);
    let (g, report) = r.returned();
    assert_eq!(report.verdict, trial(1, true));
    assert_eq!(events(&report), vec![]);
    assert!(g.on_trial());
    assert_eq!(g.verdict(), trial(1, true));
    assert!(!dev.nvs.has(NVS_NAMESPACE, KEY_STM));
    assert_eq!(dev.ota.knobs().verifies, vec![SLOT_ADDR[0]]);
    assert_eq!(dev.ota.knobs().mark_valids, 0);
    assert_eq!(
        trial_record(&dev.nvs),
        Some(TrialRecord {
            state: STATE_TRIAL,
            boots: 1,
            stm_required: true,
            app: APP_B,
            away: None,
            fallback: SLOT_ADDR[0],
        })
    );
    assert_eq!(
        mirror(&dev),
        Some(Mirror {
            app: APP_B,
            boots: 1,
            reason: 0,
            streak: 0,
            flags: 0,
            away: AppId([0; 8]),
        })
    );
    // the next boot of the trial keeps the flag of the first one (otaStm is gone)
    board.reset(Reset::Software);
    drop(dev);
    let (dev, r) = boot(&board);
    assert_eq!(r.returned().1.verdict, trial(2, true));
    assert_eq!(trial_record(&dev.nvs).unwrap().boots, 2);
    drop(dev);
    // a value other than 1 does not require the STM; nor does a missing namespace
    let board = uploaded_b();
    board.nvs().set_u8(NVS_NAMESPACE, KEY_STM, 2);
    let (_dev, r) = boot(&board);
    assert_eq!(r.returned().1.verdict, trial(1, false));
}

#[test]
fn a_crash_loop_switches_back_after_three_boots() {
    let board = uploaded_b();
    let report = crash_loop(&board, 3, Reset::Panic);
    assert_eq!(report.verdict, trial(3, false));
    // boot 4: over the limit
    let (dev, r) = boot(&board);
    assert_eq!(r, Ended::Reset(Reset::Software));
    assert_eq!(dev.ota.knobs().set_boots, vec![SLOT_ADDR[0]]);
    let t = trial_record(&dev.nvs).unwrap();
    assert_eq!(
        (t.state, t.boots, t.away),
        (STATE_SWITCHED_BACK, 4, Some(APP_B))
    );
    assert_eq!(ok_record(&dev.nvs), Some(APP_A)); // the target is the confirmed image
    assert_eq!(
        mirror(&dev),
        Some(Mirror {
            app: APP_B,
            boots: 4,
            reason: 1,
            streak: 0,
            flags: 0,
            away: APP_B,
        })
    );
    drop(dev);
    // the fallback boots, confirmed, and tells the log why
    let (dev, r) = boot(&board);
    assert_eq!(dev.ota.running().app, Some(APP_A));
    let (_, report) = r.returned();
    assert_eq!(report.verdict, BootVerdict::Confirmed);
    assert_eq!(
        events(&report),
        vec![GuardEvent::PreviousImageFailed(SwitchReason::BootLimit)]
    );
    assert_eq!(trial_record(&dev.nvs), None);
    // the breadcrumb is used up; the mirror keeps B as the image that failed (F5)
    assert_eq!(
        mirror(&dev),
        Some(Mirror {
            app: APP_A,
            boots: 0,
            reason: 0,
            streak: 0,
            flags: 0,
            away: APP_B,
        })
    );
    board.reset(Reset::Software);
    drop(dev);
    let (_dev, r) = boot(&board);
    assert_eq!(events(&r.returned().1), vec![]); // once
    assert_eq!(
        board
            .history()
            .iter()
            .map(|b| (b.slot, b.reset))
            .collect::<Vec<_>>(),
        vec![
            (1, Reset::PowerOn),
            (1, Reset::Panic),
            (1, Reset::Panic),
            (1, Reset::Panic),
            (0, Reset::Software),
            (0, Reset::Software)
        ]
    );
}

#[test]
fn a_switch_makes_its_target_the_confirmed_image() {
    // A was never confirmed (no otaOk): after B's switch back, A runs without a trial
    let board = FakeBoard::with_slots([SlotImage::glue(APP_A), SlotImage::glue(APP_B)], 1);
    crash_loop(&board, 3, Reset::TaskWdt);
    let (dev, r) = boot(&board);
    assert_eq!(r, Ended::Reset(Reset::Software));
    assert_eq!(ok_record(&dev.nvs), Some(APP_A));
    drop(dev);
    let (dev, r) = boot(&board);
    let (g, report) = r.returned();
    assert_eq!(report.verdict, BootVerdict::Confirmed);
    assert!(!g.on_trial());
    assert_eq!(dev.ota.knobs().mark_valids, 1);
}

#[test]
fn a_target_whose_record_failed_meets_the_ping_pong_check() {
    // the otaOk write at the switch fails: A starts a trial whose fallback is the image that just
    // failed, so A is confirmed at once (no turns between two failing images)
    let board = FakeBoard::with_slots([SlotImage::glue(APP_A), SlotImage::glue(APP_B)], 1);
    crash_loop(&board, 3, Reset::Panic);
    let dev = board.boot();
    dev.nvs.knobs().fail_set.insert(KEY_OK.to_string());
    assert_eq!(decide(&dev), Ended::Reset(Reset::Software));
    assert_eq!(ok_record(&dev.nvs), None);
    drop(dev);
    let (dev, r) = boot(&board);
    let (_, report) = r.returned();
    assert_eq!(report.verdict, BootVerdict::Confirmed);
    assert_eq!(
        events(&report),
        vec![
            GuardEvent::PreviousImageFailed(SwitchReason::BootLimit),
            GuardEvent::NoFallback
        ]
    );
    assert_eq!(ok_record(&dev.nvs), Some(APP_A));
}

#[test]
fn watchdog_resets_and_boot_deadline_restarts_count_as_boots() {
    let board = uploaded_b();
    let (dev, r) = boot(&board);
    assert_eq!(r.returned().1.verdict, trial(1, false));
    board.reset(Reset::TaskWdt);
    drop(dev);
    let (dev, r) = boot(&board);
    assert_eq!(r.returned().1.verdict, trial(2, false));
    // setup hangs: the boot deadline restarts the chip
    assert_eq!(run(|| dev.system.restart()), Ended::Reset(Reset::Software));
    drop(dev);
    let (dev, r) = boot(&board);
    assert_eq!(r.returned().1.verdict, trial(3, false));
    board.reset(Reset::Pin);
    drop(dev);
    let (_dev, r) = boot(&board);
    assert_eq!(r, Ended::Reset(Reset::Software));
}

#[test]
fn the_rtc_mirror_counts_when_the_nvs_write_fails() {
    let board = uploaded_b();
    for n in 1..=3 {
        let dev = board.boot();
        dev.nvs.knobs().fail_set.insert(KEY_TRIAL.to_string());
        let (_, report) = decide(&dev).returned();
        assert_eq!(report.verdict, trial(n, false));
        assert_eq!(trial_record(&dev.nvs), None);
        assert_eq!(mirror(&dev).unwrap().boots, n);
        board.reset(Reset::Panic);
    }
    let dev = board.boot();
    dev.nvs.knobs().fail_set.insert(KEY_TRIAL.to_string());
    assert_eq!(decide(&dev), Ended::Reset(Reset::Software));
    assert_eq!(mirror(&dev).unwrap().reason, 1);
    drop(dev);
    assert_eq!(board.boot().ota.running().app, Some(APP_A));
}

#[test]
fn without_nvs_the_mirror_still_counts_and_a_power_cycle_restarts_the_count() {
    let board = uploaded_b();
    let mut last = None;
    for reset in [
        Reset::Software,
        Reset::Software,
        Reset::PowerOn,
        Reset::Software,
    ] {
        let dev = board.boot();
        dev.nvs.knobs().init_failed = true;
        last = Some(decide(&dev).returned().1.verdict);
        board.reset(reset);
    }
    // boots 1, 2, 3, then the power cycle: 1 again
    assert_eq!(last, Some(trial(1, false)));
    let dev = board.boot();
    dev.nvs.knobs().init_failed = true;
    assert_eq!(decide(&dev).returned().1.verdict, trial(2, false));
}

#[test]
fn three_power_cycles_before_the_confirmation_switch_back() {
    // 6.6: counted as unconfirmed boots although the image may be fine
    let board = uploaded_b();
    crash_loop(&board, 3, Reset::PowerOn);
    let (_dev, r) = boot(&board);
    assert_eq!(r, Ended::Reset(Reset::Software));
}

#[test]
fn no_valid_fallback_confirms_the_running_image() {
    // a factory-fresh board: nothing in slot 1
    let board = FakeBoard::new();
    let (dev, r) = boot(&board);
    let (g, report) = r.returned();
    assert_eq!(report.verdict, BootVerdict::Confirmed);
    assert_eq!(events(&report), vec![GuardEvent::NoFallback]);
    assert!(!g.on_trial());
    assert_eq!(ok_record(&dev.nvs), Some(APP_A));
    assert_eq!(trial_record(&dev.nvs), None);
    assert_eq!(mirror(&dev), None);
    assert_eq!(dev.ota.knobs().mark_valids, 1);
    assert_eq!(dev.ota.knobs().verifies, Vec::<u32>::new()); // no description: no check
    board.reset(Reset::Software);
    drop(dev);
    let (_dev, r) = boot(&board);
    assert_eq!(events(&r.returned().1), vec![]);
    // a fallback whose description cannot be read, and no second slot at all
    let board = uploaded_b();
    board
        .nvs()
        .set_blob(NVS_NAMESPACE, KEY_OK, &encode_ok(APP_C));
    board.ota().store().slots[0] = SlotImage::broken(None);
    let (dev, r) = boot(&board);
    assert_eq!(events(&r.returned().1), vec![GuardEvent::NoFallback]);
    assert_eq!(ok_record(&dev.nvs), Some(APP_B));
    drop(dev);
    let board = uploaded_b();
    let dev = board.boot();
    dev.ota.knobs().no_other = true;
    assert_eq!(
        events(&decide(&dev).returned().1),
        vec![GuardEvent::NoFallback]
    );
}

#[test]
fn a_fallback_that_does_not_validate_means_no_trial() {
    // its description is readable (a failed upload), the image does not validate
    let board = uploaded_b();
    board
        .nvs()
        .set_blob(NVS_NAMESPACE, KEY_OK, &encode_ok(APP_C));
    board.ota().store().slots[0] = SlotImage::broken(Some(APP_A));
    let (dev, r) = boot(&board);
    let (g, report) = r.returned();
    assert_eq!(report.verdict, BootVerdict::Confirmed);
    assert_eq!(events(&report), vec![GuardEvent::NoFallback]);
    assert!(!g.on_trial());
    assert_eq!(dev.ota.knobs().verifies, vec![SLOT_ADDR[0]]);
    assert_eq!(ok_record(&dev.nvs), Some(APP_B));
}

#[test]
fn a_refused_switch_at_boot_keeps_the_image_running() {
    let board = uploaded_b();
    crash_loop(&board, 3, Reset::Panic);
    let dev = board.boot();
    dev.ota.knobs().set_boot_err = Some(EspErr::FLASH_OP_FAIL);
    let (g, report) = decide(&dev).returned();
    assert_eq!(report.verdict, BootVerdict::Confirmed);
    assert_eq!(events(&report), vec![GuardEvent::NoFallback]);
    assert!(!g.on_trial());
    assert_eq!(dev.ota.knobs().set_boots, vec![SLOT_ADDR[0]]);
    assert_eq!(ok_record(&dev.nvs), Some(APP_B));
    assert_eq!(trial_record(&dev.nvs), None);
    assert_eq!(mirror(&dev), None);
    assert_eq!(dev.ota.knobs().mark_valids, 1);
}

#[test]
fn the_image_that_failed_the_last_trial_is_never_switched_to_again() {
    // R0 could not write otaOk: when R1 fails and the guard switches back, R0 starts a trial
    // with R1 as its fallback; ping-pong is cut by confirming R0
    let board = FakeBoard::with_slots([SlotImage::glue(APP_A), SlotImage::glue(APP_B)], 0);
    let failed = TrialRecord {
        state: STATE_SWITCHED_BACK,
        boots: 4,
        stm_required: false,
        app: APP_B,
        away: Some(APP_B),
        fallback: SLOT_ADDR[0],
    };
    board
        .nvs()
        .set_blob(NVS_NAMESPACE, KEY_TRIAL, &encode_trial(&failed));
    let (dev, r) = boot(&board);
    let (g, report) = r.returned();
    assert_eq!(dev.ota.running().app, Some(APP_A));
    assert_eq!(report.verdict, BootVerdict::Confirmed);
    assert_eq!(events(&report), vec![GuardEvent::NoFallback]);
    assert!(!g.on_trial());
    assert_eq!(ok_record(&dev.nvs), Some(APP_A));
}

#[test]
fn the_failed_image_uploaded_again_gets_a_new_trial() {
    let board = uploaded_b();
    let failed = TrialRecord {
        state: STATE_SWITCHED_BACK,
        boots: 4,
        stm_required: false,
        app: APP_B,
        away: Some(APP_B),
        fallback: SLOT_ADDR[0],
    };
    board
        .nvs()
        .set_blob(NVS_NAMESPACE, KEY_TRIAL, &encode_trial(&failed));
    let (dev, r) = boot(&board);
    let (_, report) = r.returned();
    assert_eq!(report.verdict, trial(1, false));
    let t = trial_record(&dev.nvs).unwrap();
    assert_eq!(
        (t.state, t.boots, t.away, t.app),
        (STATE_TRIAL, 1, Some(APP_B), APP_B)
    );
}

#[test]
fn a_new_image_carries_the_switched_away_id_of_another_record() {
    let board = FakeBoard::with_slots([SlotImage::glue(APP_A), SlotImage::glue(APP_C)], 1);
    board
        .nvs()
        .set_blob(NVS_NAMESPACE, KEY_OK, &encode_ok(APP_A));
    let old = TrialRecord {
        state: STATE_SWITCHED_BACK,
        boots: 4,
        stm_required: true,
        app: APP_B,
        away: Some(APP_B),
        fallback: SLOT_ADDR[0],
    };
    board
        .nvs()
        .set_blob(NVS_NAMESPACE, KEY_TRIAL, &encode_trial(&old));
    // the mirror of B's switch is still there (a C++ fallback did not touch it): logged by C
    board.rtc().store(
        0,
        &encode_mirror(&Mirror {
            app: APP_B,
            boots: 4,
            reason: 1,
            streak: 0,
            flags: 0,
            away: APP_B,
        }),
    );
    board.reset(Reset::Software); // a warm boot: the RTC block stays
    let (dev, r) = boot(&board);
    let (_, report) = r.returned();
    assert_eq!(report.verdict, trial(1, false));
    assert_eq!(
        events(&report),
        vec![GuardEvent::PreviousImageFailed(SwitchReason::BootLimit)]
    );
    let t = trial_record(&dev.nvs).unwrap();
    assert_eq!((t.app, t.away, t.boots), (APP_C, Some(APP_B), 1));
}

#[test]
fn a_breadcrumb_of_the_running_image_itself_is_not_logged() {
    let board = uploaded_b();
    let rec = TrialRecord {
        state: STATE_SWITCHED_BACK,
        boots: 4,
        stm_required: false,
        app: APP_B,
        away: Some(APP_B),
        fallback: SLOT_ADDR[0],
    };
    board
        .nvs()
        .set_blob(NVS_NAMESPACE, KEY_TRIAL, &encode_trial(&rec));
    board.rtc().store(
        0,
        &encode_mirror(&Mirror {
            app: APP_B,
            boots: 4,
            reason: 2,
            streak: 0,
            flags: 0,
            away: APP_B,
        }),
    );
    board.reset(Reset::Software);
    let (_dev, r) = boot(&board);
    let (_, report) = r.returned();
    assert_eq!(events(&report), vec![]);
    assert_eq!(report.verdict, trial(1, false)); // the breadcrumb's boots do not count
}

#[test]
fn a_mirror_with_an_unknown_reason_is_not_logged() {
    let board = FakeBoard::new();
    board.rtc().store(
        0,
        &encode_mirror(&Mirror {
            app: APP_B,
            boots: 4,
            reason: 9,
            streak: 0,
            flags: 0,
            away: APP_B,
        }),
    );
    board.reset(Reset::Software);
    let (_dev, r) = boot(&board);
    assert_eq!(events(&r.returned().1), vec![GuardEvent::NoFallback]);
}

#[test]
fn an_unreadable_running_image_is_left_alone() {
    let board = FakeBoard::with_slots([SlotImage::glue(APP_A), SlotImage::glue(APP_B)], 0);
    board.ota().store().slots[0].app = None;
    board.nvs().set_u8(NVS_NAMESPACE, KEY_STM, 1);
    let (dev, r) = boot(&board);
    let (g, report) = r.returned();
    assert_eq!(report.verdict, BootVerdict::Confirmed);
    assert!(!g.on_trial());
    assert!(!dev.nvs.has(NVS_NAMESPACE, KEY_STM));
    assert_eq!(dev.nvs.keys(NVS_NAMESPACE), Vec::<String>::new());
    assert_eq!(dev.rtc.snapshot()[..4], [0xA5; 4]);
    assert_eq!(dev.ota.knobs().mark_valids, 0);
    // at run time it has nothing to confirm
    let mut g = g;
    g.mark_valid(&dev.nvs, &dev.ota, &dev.rtc);
    assert_eq!(dev.nvs.keys(NVS_NAMESPACE), Vec::<String>::new());
}

#[test]
fn an_all_zero_app_id_is_unknown_to_the_guard() {
    // an image without the ELF SHA-256 in its app descriptor names no build: it runs as it is
    // (no trial, no record), also when otaOk holds the same zeros
    for stored in [None, Some(UNKNOWN_APP)] {
        let board =
            FakeBoard::with_slots([SlotImage::glue(UNKNOWN_APP), SlotImage::glue(APP_B)], 0);
        if let Some(app) = stored {
            board.nvs().set_blob(NVS_NAMESPACE, KEY_OK, &encode_ok(app));
        }
        board.nvs().set_u8(NVS_NAMESPACE, KEY_STM, 1);
        let (dev, r) = boot(&board);
        let (g, report) = r.returned();
        assert_eq!(report.verdict, BootVerdict::Confirmed);
        assert_eq!(events(&report), vec![]);
        assert!(!g.on_trial());
        assert_eq!(trial_record(&dev.nvs), None);
        assert_eq!(ok_record(&dev.nvs), stored);
        assert!(!dev.nvs.has(NVS_NAMESPACE, KEY_STM)); // read once and erased, as always
        assert_eq!(dev.ota.knobs().mark_valids, 0);
        assert_eq!(dev.ota.knobs().verifies, Vec::<u32>::new());
        assert_eq!(dev.rtc.snapshot()[..4], [0xA5; 4]); // no mirror written
    }
    // a known image still finds the zero image a valid fallback (the legacy firmware)
    let board = FakeBoard::with_slots([SlotImage::glue(UNKNOWN_APP), SlotImage::glue(APP_B)], 1);
    let (dev, r) = boot(&board);
    assert_eq!(r.returned().1.verdict, trial(1, false));
    assert_eq!(trial_record(&dev.nvs).unwrap().fallback, SLOT_ADDR[0]);
}

#[test]
fn damaged_records_start_a_trial() {
    let board = uploaded_b();
    board.ota().store().otadata = 0;
    board
        .nvs()
        .set_blob(NVS_NAMESPACE, KEY_OK, &encode_ok(APP_A)[..15]);
    let (_dev, r) = boot(&board);
    assert_eq!(r.returned().1.verdict, trial(1, false));
    let board = uploaded_b();
    board.ota().store().otadata = 0;
    let mut long = encode_ok(APP_A).to_vec();
    long.push(0);
    board.nvs().set_blob(NVS_NAMESPACE, KEY_OK, &long);
    let (_dev, r) = boot(&board);
    assert_eq!(r.returned().1.verdict, trial(1, false));
    let board = uploaded_b();
    let mut short = encode_trial(&TrialRecord {
        state: STATE_TRIAL,
        boots: 3,
        stm_required: false,
        app: APP_B,
        away: None,
        fallback: SLOT_ADDR[0],
    })
    .to_vec();
    short.pop();
    board.nvs().set_blob(NVS_NAMESPACE, KEY_TRIAL, &short);
    let (_dev, r) = boot(&board);
    assert_eq!(r.returned().1.verdict, trial(1, false));
}

// ---------------------------------------------------------------- 6.3 at run time

#[test]
fn mark_valid_confirms_the_trial() {
    let board = uploaded_b();
    let (dev, r) = boot(&board);
    let (mut g, _) = r.returned();
    g.mark_valid(&dev.nvs, &dev.ota, &dev.rtc);
    assert!(!g.on_trial());
    assert_eq!(g.verdict(), BootVerdict::Confirmed);
    assert_eq!(ok_record(&dev.nvs), Some(APP_B));
    assert_eq!(trial_record(&dev.nvs), None);
    assert_eq!(mirror(&dev), None);
    assert_eq!(dev.ota.knobs().mark_valids, 1);
    // a second call (no trial any more) changes nothing
    let sets = dev.nvs.knobs().sets.clone();
    g.mark_valid(&dev.nvs, &dev.ota, &dev.rtc);
    assert_eq!(dev.nvs.knobs().sets, sets);
    assert_eq!(dev.ota.knobs().mark_valids, 1);
    board.reset(Reset::Panic);
    drop(dev);
    let (_dev, r) = boot(&board);
    let (_, report) = r.returned();
    assert_eq!(report.verdict, BootVerdict::Confirmed);
    assert_eq!(events(&report), vec![]);
}

#[test]
fn a_health_rollback_switches_back_with_its_breadcrumb() {
    let board = uploaded_b();
    let (dev, r) = boot(&board);
    let (mut g, _) = r.returned();
    let ended = run(|| {
        g.switch_to_fallback(
            &dev.nvs,
            &dev.ota,
            &dev.rtc,
            &dev.system,
            SwitchCause::Health,
        )
    });
    assert_eq!(ended, Ended::Reset(Reset::Software));
    let t = trial_record(&dev.nvs).unwrap();
    assert_eq!((t.state, t.away), (STATE_SWITCHED_BACK, Some(APP_B)));
    assert_eq!(
        mirror(&dev),
        Some(Mirror {
            app: APP_B,
            boots: 1,
            reason: 2,
            streak: 0,
            flags: 0,
            away: APP_B,
        })
    );
    assert_eq!(dev.ota.knobs().set_boots, vec![SLOT_ADDR[0]]);
    assert_eq!(ok_record(&dev.nvs), Some(APP_A));
    drop(dev);
    let (dev, r) = boot(&board);
    assert_eq!(dev.ota.running().app, Some(APP_A));
    assert_eq!(
        events(&r.returned().1),
        vec![GuardEvent::PreviousImageFailed(SwitchReason::Health)]
    );
}

#[test]
fn a_refused_switch_at_run_time_ends_the_trial_as_confirmed() {
    let board = uploaded_b();
    let (dev, r) = boot(&board);
    let (mut g, _) = r.returned();
    dev.ota.knobs().set_boot_err = Some(EspErr::OTA_VALIDATE_FAILED);
    let e = run(|| {
        g.switch_to_fallback(
            &dev.nvs,
            &dev.ota,
            &dev.rtc,
            &dev.system,
            SwitchCause::Health,
        )
    });
    assert_eq!(e, Ended::Returned(GuardEvent::NoFallback));
    assert!(!g.on_trial());
    assert_eq!(ok_record(&dev.nvs), Some(APP_B));
    assert_eq!(trial_record(&dev.nvs), None);
    assert_eq!(mirror(&dev), None);
    assert_eq!(dev.ota.knobs().mark_valids, 1);
}

#[test]
fn a_manual_switch_back_works_in_any_state() {
    // a confirmed image switches to the other slot; nothing is confirmed any more, so the other
    // image (B never ran here) boots on trial with A as its fallback
    let board = uploaded_b();
    board.ota().store().otadata = 0;
    let (dev, r) = boot(&board);
    let (mut g, _) = r.returned();
    assert!(BootGuard::fallback_available(&dev.ota));
    let e = run(|| {
        g.switch_to_fallback(
            &dev.nvs,
            &dev.ota,
            &dev.rtc,
            &dev.system,
            SwitchCause::Manual,
        )
    });
    assert_eq!(e, Ended::Reset(Reset::Software));
    assert_eq!(trial_record(&dev.nvs), None);
    assert_eq!(ok_record(&dev.nvs), None);
    // the selection first: a refused one leaves the confirmation as it was
    assert!(dev.journal.entries().ends_with(&[
        "ota set_boot 0x150000".into(),
        "nvs remove vdmrev/otaOk".into(),
        "esp_restart".into()
    ]));
    drop(dev);
    let (dev, r) = boot(&board);
    assert_eq!(dev.ota.running().app, Some(APP_B));
    let (_, report) = r.returned();
    assert_eq!(report.verdict, trial(1, false));
    assert_eq!(trial_record(&dev.nvs).unwrap().fallback, SLOT_ADDR[0]);
    drop(dev);
    // during a trial: switched back without a breadcrumb
    let board = uploaded_b();
    let (dev, r) = boot(&board);
    let (mut g, report) = r.returned();
    assert_eq!(report.verdict, trial(1, false));
    let e = run(|| {
        g.switch_to_fallback(
            &dev.nvs,
            &dev.ota,
            &dev.rtc,
            &dev.system,
            SwitchCause::Manual,
        )
    });
    assert_eq!(e, Ended::Reset(Reset::Software));
    let t = trial_record(&dev.nvs).unwrap();
    assert_eq!((t.state, t.away), (STATE_SWITCHED_BACK, None));
    assert_eq!(mirror(&dev), None);
    drop(dev);
    let (_dev, r) = boot(&board);
    let (_, report) = r.returned();
    assert_eq!(report.verdict, BootVerdict::Confirmed);
    assert_eq!(events(&report), vec![]);
}

#[test]
fn a_manual_switch_to_the_image_that_failed_its_trial_gives_it_a_new_trial() {
    // B failed its trial and the guard went back to A. A manual switch from the confirmed A to
    // B must not make B the confirmed image: B crashes again and the guard returns to A, where
    // a confirmed B would crash at every boot with nothing left to switch back to
    let board = FakeBoard::with_slots([SlotImage::glue(APP_A), SlotImage::glue(APP_B)], 0);
    board
        .nvs()
        .set_blob(NVS_NAMESPACE, KEY_OK, &encode_ok(APP_A));
    let failed = TrialRecord {
        state: STATE_SWITCHED_BACK,
        boots: 4,
        stm_required: false,
        app: APP_B,
        away: Some(APP_B),
        fallback: SLOT_ADDR[0],
    };
    board
        .nvs()
        .set_blob(NVS_NAMESPACE, KEY_TRIAL, &encode_trial(&failed));
    let (dev, r) = boot(&board);
    let (mut g, report) = r.returned();
    assert_eq!(report.verdict, BootVerdict::Confirmed);
    let e = run(|| {
        g.switch_to_fallback(
            &dev.nvs,
            &dev.ota,
            &dev.rtc,
            &dev.system,
            SwitchCause::Manual,
        )
    });
    assert_eq!(e, Ended::Reset(Reset::Software));
    assert_eq!(ok_record(&dev.nvs), None);
    drop(dev);
    let report = crash_loop(&board, 3, Reset::Panic);
    assert_eq!(report.verdict, trial(3, false));
    let (dev, r) = boot(&board);
    assert_eq!(r, Ended::Reset(Reset::Software)); // boot 4 of B: back to A
    assert_eq!(ok_record(&dev.nvs), Some(APP_A));
    drop(dev);
    let (dev, r) = boot(&board);
    assert_eq!(dev.ota.running().app, Some(APP_A));
    let (_, report) = r.returned();
    assert_eq!(report.verdict, BootVerdict::Confirmed);
    assert_eq!(
        events(&report),
        vec![GuardEvent::PreviousImageFailed(SwitchReason::BootLimit)]
    );
}

#[test]
fn a_manual_switch_to_the_cpp_firmware_leaves_no_confirmation_behind() {
    // A switches to the C++ firmware by hand; when the C++ firmware installs A again later, A
    // proves itself on trial instead of running as the image confirmed before the switch
    let board = FakeBoard::with_slots([SlotImage::foreign(APP_CPP), SlotImage::glue(APP_A)], 1);
    board
        .nvs()
        .set_blob(NVS_NAMESPACE, KEY_OK, &encode_ok(APP_A));
    let (dev, r) = boot(&board);
    let (mut g, report) = r.returned();
    assert_eq!(report.verdict, BootVerdict::Confirmed);
    let e = run(|| {
        g.switch_to_fallback(
            &dev.nvs,
            &dev.ota,
            &dev.rtc,
            &dev.system,
            SwitchCause::Manual,
        )
    });
    assert_eq!(e, Ended::Reset(Reset::Software));
    drop(dev);
    assert!(matches!(board.boot_any(), Booted::Foreign(a) if a == APP_CPP));
    // the C++ firmware uploads A into the other slot again and restarts into it
    board.ota().store().otadata = 1;
    board.reset(Reset::Software);
    let (dev, r) = boot(&board);
    assert_eq!(r.returned().1.verdict, trial(1, false));
    assert_eq!(trial_record(&dev.nvs).unwrap().fallback, SLOT_ADDR[0]);
}

#[test]
fn leaving_for_an_upload_removes_the_confirmation_only() {
    let board = uploaded_b();
    board.ota().store().otadata = 0;
    let (dev, r) = boot(&board);
    let (g, report) = r.returned();
    assert_eq!(report.verdict, BootVerdict::Confirmed);
    dev.journal.clear();
    g.leave_for_upload(&dev.nvs);
    assert_eq!(ok_record(&dev.nvs), None);
    assert_eq!(dev.journal.entries(), vec!["nvs remove vdmrev/otaOk"]);
    assert!(!g.on_trial());
    // without NVS nothing changes and nothing breaks
    dev.nvs.knobs().init_failed = true;
    g.leave_for_upload(&dev.nvs);
    assert_eq!(dev.journal.entries(), vec!["nvs remove vdmrev/otaOk"]);
}

#[test]
fn a_manual_switch_without_a_second_slot_keeps_running() {
    let board = FakeBoard::new();
    let (dev, r) = boot(&board);
    let (mut g, _) = r.returned();
    dev.ota.knobs().no_other = true;
    assert!(!BootGuard::fallback_available(&dev.ota));
    let e = run(|| {
        g.switch_to_fallback(
            &dev.nvs,
            &dev.ota,
            &dev.rtc,
            &dev.system,
            SwitchCause::Manual,
        )
    });
    assert_eq!(e, Ended::Returned(GuardEvent::NoFallback));
    dev.ota.knobs().no_other = false;
    assert!(!BootGuard::fallback_available(&dev.ota)); // slot 1 is empty
    assert_eq!(dev.ota.knobs().set_boots, Vec::<u32>::new());
    // an empty slot: the selection is refused
    let e = run(|| {
        g.switch_to_fallback(
            &dev.nvs,
            &dev.ota,
            &dev.rtc,
            &dev.system,
            SwitchCause::Manual,
        )
    });
    assert_eq!(e, Ended::Returned(GuardEvent::NoFallback));
    assert_eq!(dev.ota.knobs().set_boots, vec![SLOT_ADDR[1]]);
    assert_eq!(ok_record(&dev.nvs), Some(APP_A));
}

// ---------------------------------------------------------------- 6.5 interactions

#[test]
fn a_rust_factory_reset_keeps_the_trial_a_cpp_one_restarts_validation() {
    let board = uploaded_b();
    board.nvs().set_blob(NVS_NAMESPACE, "cfg", b"VDMC");
    board.nvs().set_u8(NVS_NAMESPACE, "frLatch", 1);
    let (dev, r) = boot(&board);
    assert_eq!(r.returned().1.verdict, trial(1, false));
    // the Rust factory reset erases vdmrev except the latch and the guard's keys
    let mut ns = dev.nvs.open(NVS_NAMESPACE, true).unwrap();
    for key in dev.nvs.keys(NVS_NAMESPACE) {
        if !FACTORY_RESET_KEEPS.contains(&key.as_str()) && key != "frLatch" {
            assert!(ns.remove(&key));
        }
    }
    drop(ns);
    assert_eq!(
        dev.nvs.keys(NVS_NAMESPACE),
        vec!["frLatch", "otaOk", "otaTrial"]
    );
    board.reset(Reset::Software);
    drop(dev);
    let (dev, r) = boot(&board);
    let (mut g, report) = r.returned();
    assert_eq!(report.verdict, trial(2, false));
    g.mark_valid(&dev.nvs, &dev.ota, &dev.rtc);
    board.reset(Reset::Software);
    drop(dev);
    // a C++ factory reset erases the whole namespace: the next Rust boot validates itself again
    let mut ns = board.nvs().open(NVS_NAMESPACE, true).unwrap();
    assert!(ns.erase_all());
    drop(ns);
    let (_dev, r) = boot(&board);
    assert_eq!(r.returned().1.verdict, trial(1, false));
}

#[test]
fn the_first_rust_boot_after_the_cpp_firmware_has_it_as_fallback() {
    let board = FakeBoard::with_slots([SlotImage::foreign(APP_CPP), SlotImage::glue(APP_B)], 1);
    let report = crash_loop(&board, 3, Reset::Panic);
    assert_eq!(report.verdict, trial(3, false));
    let (dev, r) = boot(&board);
    assert_eq!(r, Ended::Reset(Reset::Software));
    assert_eq!(ok_record(&dev.nvs), Some(APP_CPP));
    drop(dev);
    assert!(matches!(board.boot_any(), Booted::Foreign(a) if a == APP_CPP));
}

// ---------------------------------------------------------------- F5: confirmed crash loop

/// A confirmed in slot 0 and selected, `other` in slot 1.
fn confirmed_a(other: SlotImage) -> FakeBoard {
    let board = FakeBoard::with_slots([SlotImage::glue(APP_A), other], 0);
    board
        .nvs()
        .set_blob(NVS_NAMESPACE, KEY_OK, &encode_ok(APP_A));
    board
}

/// `n` boots of the image in slot 0, each confirmed and ended with `end`.
fn confirmed_boots(board: &FakeBoard, n: usize, end: Reset) {
    for _ in 0..n {
        let (dev, r) = boot(board);
        assert_eq!(r.returned().1.verdict, BootVerdict::Confirmed);
        assert_eq!(dev.ota.running().app, Some(APP_A));
        board.reset(end);
    }
}

#[test]
fn abnormal_ends_are_the_panic_and_the_watchdog_resets() {
    // unknown, power-on, pin, software, panic, interrupt watchdog, task watchdog, other
    // watchdogs, deep sleep, brownout, SDIO
    let want = [
        false, false, false, false, true, true, true, true, false, false, false,
    ];
    for (r, w) in (0u8..).zip(want) {
        assert_eq!(abnormal(r), w, "{r}");
    }
}

#[test]
fn a_confirmed_image_that_crashes_four_boots_in_a_row_runs_the_other_on_trial() {
    let board = confirmed_a(SlotImage::glue(APP_B));
    for (n, end) in [Reset::Panic, Reset::TaskWdt, Reset::Panic, Reset::Panic]
        .into_iter()
        .enumerate()
    {
        let (dev, r) = boot(&board);
        let (g, report) = r.returned();
        assert_eq!(report.verdict, BootVerdict::Confirmed);
        assert_eq!(events(&report), vec![]);
        assert!(!g.on_trial());
        assert_eq!(
            mirror(&dev),
            Some(Mirror {
                app: APP_A,
                boots: 0,
                reason: 0,
                streak: n as u8,
                flags: 0,
                away: UNKNOWN_APP,
            })
        );
        assert!(dev.ota.knobs().verifies.is_empty());
        board.reset(end);
    }
    // the fourth abnormal end in a row: B verifies, the switch to it confirms nothing
    let (dev, r) = boot(&board);
    assert_eq!(r, Ended::Reset(Reset::Software));
    assert_eq!(dev.ota.knobs().verifies, vec![SLOT_ADDR[1]]);
    assert_eq!(dev.ota.knobs().set_boots, vec![SLOT_ADDR[1]]);
    assert_eq!(ok_record(&dev.nvs), None);
    assert_eq!(
        mirror(&dev),
        Some(Mirror {
            app: APP_A,
            boots: 0,
            reason: 3,
            streak: 0,
            flags: 0,
            away: APP_A,
        })
    );
    drop(dev);
    // B runs on trial with A as its fallback and tells why
    let (dev, r) = boot(&board);
    assert_eq!(dev.ota.running().app, Some(APP_B));
    let (g, report) = r.returned();
    assert_eq!(report.verdict, trial(1, false));
    assert!(g.on_trial());
    assert_eq!(
        events(&report),
        vec![GuardEvent::PreviousImageFailed(SwitchReason::CrashLoop)]
    );
    assert_eq!(
        GuardEvent::PreviousImageFailed(SwitchReason::CrashLoop).args(),
        (-4, 3)
    );
    let t = trial_record(&dev.nvs).unwrap();
    assert_eq!((t.app, t.boots, t.fallback), (APP_B, 1, SLOT_ADDR[0]));
}

#[test]
fn the_crash_streak_ends_with_any_other_end_of_a_boot() {
    for other in [Reset::Software, Reset::Pin, Reset::PowerOn] {
        let board = confirmed_a(SlotImage::glue(APP_B));
        confirmed_boots(&board, 3, Reset::Panic);
        confirmed_boots(&board, 1, other);
        confirmed_boots(&board, 3, Reset::Panic);
        let (dev, r) = boot(&board);
        assert_eq!(r.returned().1.verdict, BootVerdict::Confirmed, "{other:?}");
        assert_eq!(mirror(&dev).unwrap().streak, 3, "{other:?}");
        assert!(dev.ota.knobs().set_boots.is_empty(), "{other:?}");
    }
}

#[test]
fn the_boot_deadline_counts_and_a_boot_of_ten_minutes_does_not() {
    let board = confirmed_a(SlotImage::glue(APP_B));
    // three boots end by the boot deadline: a software restart the mirror marks
    for n in 0..3u8 {
        let (dev, r) = boot(&board);
        let _ = r.returned();
        assert_eq!(mirror(&dev).unwrap().streak, n);
        note_deadline_restart(&dev.rtc);
        note_deadline_restart(&dev.rtc); // a mark, not a toggle
        assert_eq!(mirror(&dev).unwrap().flags, FLAG_DEADLINE);
        board.reset(Reset::Software);
    }
    // the fourth boot runs ten minutes, then panics: its end does not count
    let (dev, r) = boot(&board);
    let (g, _) = r.returned();
    assert_eq!(mirror(&dev).unwrap().streak, 3);
    g.note_stable(&dev.rtc);
    assert_eq!(
        mirror(&dev).map(|m| (m.app, m.streak, m.flags)),
        Some((APP_A, 0, FLAG_STABLE))
    );
    board.reset(Reset::Panic);
    drop(dev);
    let (dev, r) = boot(&board);
    let _ = r.returned();
    assert_eq!(mirror(&dev).unwrap().streak, 0);
    assert!(dev.ota.knobs().set_boots.is_empty());
}

#[test]
fn a_crash_loop_stays_without_another_image_that_verifies() {
    for other in [SlotImage::broken(Some(APP_B)), SlotImage::EMPTY] {
        let board = confirmed_a(other);
        confirmed_boots(&board, 4, Reset::Panic);
        let (dev, r) = boot(&board);
        let (_, report) = r.returned();
        assert_eq!(report.verdict, BootVerdict::Confirmed);
        assert_eq!(events(&report), vec![GuardEvent::CrashLoopStays]);
        assert!(dev.ota.knobs().set_boots.is_empty());
        assert_eq!(ok_record(&dev.nvs), Some(APP_A));
        assert_eq!(mirror(&dev).map(|m| (m.streak, m.reason)), Some((4, 0)));
        // the next crash says nothing more
        board.reset(Reset::Panic);
        drop(dev);
        let (dev, r) = boot(&board);
        assert_eq!(events(&r.returned().1), vec![]);
        assert_eq!(mirror(&dev).unwrap().streak, 5);
    }
    assert_eq!(GuardEvent::CrashLoopStays.args(), (-3, 3));
    // a selection the bootloader data refuses: the image keeps running, confirmed
    let board = confirmed_a(SlotImage::glue(APP_B));
    confirmed_boots(&board, 4, Reset::Panic);
    let dev = board.boot();
    dev.ota.knobs().set_boot_err = Some(EspErr::OTA_VALIDATE_FAILED);
    let r = decide(&dev);
    assert_eq!(events(&r.returned().1), vec![GuardEvent::CrashLoopStays]);
    assert_eq!(dev.ota.knobs().set_boots, vec![SLOT_ADDR[1]]);
    assert_eq!(ok_record(&dev.nvs), Some(APP_A));
    assert_eq!(mirror(&dev).map(|m| (m.streak, m.reason)), Some((4, 0)));
}

#[test]
fn a_crash_loop_does_not_go_back_to_the_image_that_failed_its_trial() {
    // B failed its trial at the boot limit: the guard went back to A, confirmed
    let board = uploaded_b();
    let _ = crash_loop(&board, 3, Reset::Panic);
    let (_dev, r) = boot(&board);
    assert_eq!(r, Ended::Reset(Reset::Software));
    confirmed_boots(&board, 4, Reset::Panic);
    let (dev, r) = boot(&board);
    let (g, report) = r.returned();
    assert_eq!(events(&report), vec![GuardEvent::CrashLoopStays]);
    assert!(dev.ota.knobs().set_boots.is_empty());
    assert_eq!(mirror(&dev).unwrap().away, APP_B);
    // a boot of 10 min ends the streak and keeps the image that failed
    g.note_stable(&dev.rtc);
    assert_eq!(
        mirror(&dev).map(|m| (m.app, m.streak, m.away)),
        Some((APP_A, 0, APP_B))
    );
    // a power cycle forgets it: the next crash loop gives B a new trial
    board.reset(Reset::PowerOn);
    drop(dev);
    confirmed_boots(&board, 4, Reset::Panic);
    let (dev, r) = boot(&board);
    assert_eq!(r, Ended::Reset(Reset::Software));
    assert_eq!(dev.ota.knobs().set_boots, vec![SLOT_ADDR[1]]);
}

#[test]
fn a_ten_minute_boot_and_the_boot_deadline_need_the_guard() {
    // the image of this boot ran its trial and was confirmed: the mark makes the mirror anew
    let board = uploaded_b();
    let (dev, r) = boot(&board);
    let (mut g, _) = r.returned();
    g.mark_valid(&dev.nvs, &dev.ota, &dev.rtc);
    assert_eq!(mirror(&dev), None);
    g.note_stable(&dev.rtc);
    assert_eq!(
        mirror(&dev),
        Some(Mirror {
            app: APP_B,
            boots: 0,
            reason: 0,
            streak: 0,
            flags: FLAG_STABLE,
            away: UNKNOWN_APP,
        })
    );
    // an image the guard does not know: nothing is marked
    let unknown = SlotImage {
        app: None,
        valid: true,
        foreign: false,
    };
    let board = FakeBoard::with_slots([unknown, SlotImage::glue(APP_B)], 0);
    board.reset(Reset::Software);
    let (dev, r) = boot(&board);
    let (g, _) = r.returned();
    let before = dev.rtc.snapshot();
    g.note_stable(&dev.rtc);
    note_deadline_restart(&dev.rtc);
    assert_eq!(dev.rtc.snapshot(), before);
}
