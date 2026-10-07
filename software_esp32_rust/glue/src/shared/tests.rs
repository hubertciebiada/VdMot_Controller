//! Tests of the shared data of C++ `app.cpp` (the cases of `test_app.cpp` and
//! `test_app__mut.cpp` on the queue, the snapshot plumbing, the profile store, the save state
//! and the calibration info).
// host test code: the stack rule of the glue (design 2.4) is for the device
#![allow(clippy::large_stack_frames)]

use super::*;
use vdm_esp_core::stm_codec::ProfileSample;
use vdm_esp_core::stm_flasher::FlashPhase;

#[test]
fn submit_the_queue_takes_16_commands_receive_hands_them_out_in_order() {
    // C++ app::submit() before setup() (no queue yet): no Rust form, the queue exists with
    // AppShared
    let s = AppShared::new();
    let mut c = StmCommand::default();
    for i in 0..16 {
        c.valve = i;
        assert!(s.submit(&c), "command {i}");
    }
    assert!(!s.submit(&c));
    for i in 0..16 {
        assert_eq!(s.receive().map(|o| o.valve), Some(i));
    }
    assert!(s.receive().is_none());
    assert_eq!(COMMAND_QUEUE_DEPTH, 16);
}

#[test]
fn submit_receive_a_full_or_an_empty_queue_never_waits() {
    // C++ checked that the fake time did not move: the Rust queue has no waiting call at all, a
    // full and an empty queue answer at once and stay usable
    let s = AppShared::new();
    let c = StmCommand::default();
    for _ in 0..16 {
        assert!(s.submit(&c));
    }
    assert!(!s.submit(&c));
    assert!(!s.submit(&c));
    for _ in 0..16 {
        assert!(s.receive().is_some());
    }
    assert!(s.receive().is_none());
    assert!(s.receive().is_none());
    assert!(s.submit(&c));
}

#[test]
fn snapshot_a_published_snapshot_is_what_the_accessors_return() {
    let a = AppShared::new();
    let mut s = Box::<StmSnapshot>::default();
    s.revision = 9;
    s.link = LinkState::Up;
    s.proto = 2;
    s.support = StmSupport::Supported;
    s.flash.phase = FlashPhase::Erasing;
    s.hw_id = 0x431;
    a.publish_stm_snapshot(&s);
    assert_eq!(a.stm_snapshot_revision(), 9);
    assert_eq!(a.stm_link_state(), LinkState::Up);
    assert_eq!(a.stm_protocol(), 2);
    assert_eq!(a.stm_support(), StmSupport::Supported);
    assert!(a.stm_flash_active());
    let mut out = Box::<StmSnapshot>::default();
    a.read_stm_snapshot(&mut out);
    assert_eq!(out.hw_id, 0x431);
    assert_eq!(*out, *s);
    s.flash.phase = FlashPhase::Done;
    a.publish_stm_snapshot(&s);
    assert!(!a.stm_flash_active());
}

#[test]
fn snapshot_the_flash_is_active_in_every_phase_but_idle_done_and_failed() {
    let a = AppShared::new();
    let mut s = Box::<StmSnapshot>::default();
    for p in (0..=255).filter_map(FlashPhase::from_raw) {
        s.flash.phase = p;
        a.publish_stm_snapshot(&s);
        let idle = matches!(p, FlashPhase::Idle | FlashPhase::Done | FlashPhase::Failed);
        assert_eq!(a.stm_flash_active(), !idle, "{p:?}");
    }
    // the mark of the stm thread holds until the next snapshot
    a.mark_stm_flash_active();
    assert!(a.stm_flash_active());
    s.flash.phase = FlashPhase::Idle;
    a.publish_stm_snapshot(&s);
    assert!(!a.stm_flash_active());
}

#[test]
fn snapshot_every_link_state_and_support_level_is_kept() {
    let a = AppShared::new();
    let mut s = Box::<StmSnapshot>::default();
    for l in (0..=255).filter_map(LinkState::from_raw) {
        s.link = l;
        a.publish_stm_snapshot(&s);
        assert_eq!(a.stm_link_state(), l);
    }
    for v in (0..=255).filter_map(StmSupport::from_raw) {
        s.support = v;
        a.publish_stm_snapshot(&s);
        assert_eq!(a.stm_support(), v);
    }
    s.proto = 3;
    s.revision = u32::MAX;
    a.publish_stm_snapshot(&s);
    assert_eq!((a.stm_protocol(), a.stm_snapshot_revision()), (3, u32::MAX));
}

#[test]
fn snapshot_protocol_and_revision_are_0_before_the_first_snapshot() {
    let a = AppShared::default();
    assert_eq!(a.stm_protocol(), 0);
    assert_eq!(a.stm_snapshot_revision(), 0);
    assert_eq!(a.stm_link_state(), LinkState::Unknown);
    assert_eq!(a.stm_support(), StmSupport::Unknown);
    assert!(!a.stm_flash_active());
    let mut out = Box::<StmSnapshot>::default();
    out.hw_id = 7;
    a.read_stm_snapshot(&mut out);
    assert_eq!(*out, StmSnapshot::default());
}

#[test]
fn profiles_the_store_keeps_the_last_profile_of_every_valve() {
    // C++ storeProfile()/readProfile() before setup() (no store yet): no Rust form
    let a = AppShared::new();
    let mut out = Profile {
        count: 9,
        ..Profile::default()
    };
    a.read_profile(3, &mut out);
    assert_eq!(out.count, 0); // none yet
    let mut p = Profile {
        valve: 3,
        count: 2,
        ..Profile::default()
    };
    p.samples[1] = ProfileSample {
        count: 77,
        current: 5,
    };
    a.store_profile(&p);
    a.read_profile(3, &mut out);
    assert_eq!(out, p);
    assert_eq!(out.samples[1].count, 77);
    a.read_profile(2, &mut out);
    assert_eq!(out.count, 0);
    p.count = 4;
    a.store_profile(&p); // the last one counts
    a.read_profile(3, &mut out);
    assert_eq!(out.count, 4);
    p.valve = 11;
    p.count = 1;
    a.store_profile(&p);
    a.read_profile(11, &mut out);
    assert_eq!((out.valve, out.count), (11, 1));
    p.valve = 12; // out of range: ignored, read as none
    a.store_profile(&p);
    out.count = 9;
    a.read_profile(12, &mut out);
    assert_eq!(out, Profile::default());
    a.read_profile(11, &mut out);
    assert_eq!(out.count, 1);
    a.read_profile(0, &mut out);
    assert_eq!(out, Profile::default());
}

#[test]
fn save_state_calibration_info_and_flash_mark() {
    let a = AppShared::new();
    assert_eq!(a.stm_save_state(), StmSaveState::Idle);
    a.request_stm_save();
    assert_eq!(a.stm_save_state(), StmSaveState::Waiting);
    a.set_stm_save_state(StmSaveState::Saved);
    assert_eq!(a.stm_save_state(), StmSaveState::Saved);
    for v in (0..=255).filter_map(StmSaveState::from_raw) {
        a.set_stm_save_state(v);
        assert_eq!(a.stm_save_state(), v);
    }
    assert_eq!(a.calib_info(), CalibInfo::default());
    let ci = CalibInfo {
        last_scheduled_epoch: 5,
        next_slot: 20_261_001,
        next_epoch: 7,
    };
    a.set_calib_info(&ci);
    assert_eq!(a.calib_info(), ci);
    assert!(!a.stm_flash_active());
    a.mark_stm_flash_active();
    assert!(a.stm_flash_active());
}

#[test]
fn health_found_tasks_and_the_smallest_largest_block() {
    let a = AppShared::new();
    assert!((0..MONITORED_TASKS).all(|i| !a.task_found(i)));
    a.mark_task_found(3);
    a.mark_task_found(MONITORED_TASKS); // out of range: ignored
    assert_eq!(
        (0..=MONITORED_TASKS)
            .filter(|&i| a.task_found(i))
            .collect::<Vec<_>>(),
        vec![3]
    );
    assert_eq!(a.min_largest(), 0);
    a.set_min_largest(110_000);
    assert_eq!(a.min_largest(), 110_000);
}
