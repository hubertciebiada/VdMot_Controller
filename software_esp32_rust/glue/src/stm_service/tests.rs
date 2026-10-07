//! Tests of `stm_service` (C++ `test_stm_service.cpp`): the scheduled calibration confirmed by
//! the STM, the NVS copy of the desired targets, the RTC records across software restarts and
//! the boot choice. The C++ sibling fakes of app, storage, net and logger are [`FakeHost`]; the
//! RTC records live in the fake board, so the multi-boot cases boot it again.
// host test code: the stack rule of the glue (design 2.4) is for the device
#![allow(clippy::large_stack_frames)]

use std::sync::{Arc, Mutex, MutexGuard};

use super::*;
use crate::testkit::board::TestPlatform;
use crate::testkit::{lock as tlock, Device, FakeBoard, Reset};
use vdm_esp_core::event_log::Severity;

/// Where the cases keep the two RTC records.
const RECORDS: RtcRecords = RtcRecords {
    targets: 100,
    lease: 160,
};
const CET: &str = "CET-1CEST,M3.5.0,M10.5.0/3";

/// The C++ sibling fakes of app, storage, net and logger.
struct HostState {
    // scripted
    submit_result: bool,
    calib: CalibInfo,
    revision: u32,
    calib_cfg: CalibScheduleConfig,
    calib_slot: u32,
    last_calib: i64,
    /// NVS `targets` (C++ `sib::storage().targets`)
    targets: Vec<u8>,
    save_targets_result: bool,
    local_time: LocalTime,
    // recorded
    events: Vec<Event>,
    submitted: Vec<StmCommand>,
    calib_infos: Vec<CalibInfo>,
    saved_calib_slots: Vec<u32>,
    saved_last_calibs: Vec<i64>,
    target_saves: u32,
    target_loads: u32,
}

impl Default for HostState {
    fn default() -> Self {
        HostState {
            submit_result: true,
            calib: CalibInfo::default(),
            revision: 0,
            calib_cfg: CalibScheduleConfig::default(),
            calib_slot: 0,
            last_calib: 0,
            targets: Vec::new(),
            save_targets_result: true,
            local_time: LocalTime::default(),
            events: Vec::new(),
            submitted: Vec::new(),
            calib_infos: Vec::new(),
            saved_calib_slots: Vec::new(),
            saved_last_calibs: Vec::new(),
            target_saves: 0,
            target_loads: 0,
        }
    }
}

#[derive(Clone, Default)]
struct FakeHost(Arc<Mutex<HostState>>);

impl FakeHost {
    fn s(&self) -> MutexGuard<'_, HostState> {
        tlock(&self.0)
    }
    fn with_code(&self, code: EventCode) -> Vec<Event> {
        self.s()
            .events
            .iter()
            .filter(|e| e.code == code)
            .cloned()
            .collect()
    }
    fn has(&self, code: EventCode) -> bool {
        !self.with_code(code).is_empty()
    }
    fn first(&self, code: EventCode) -> Event {
        match self.with_code(code).first() {
            Some(e) => e.clone(),
            None => panic!("no {code:?} event"),
        }
    }
}

impl StmServiceHost for FakeHost {
    fn log(&mut self, e: &Event) {
        self.s().events.push(e.clone());
    }
    fn submit(&mut self, cmd: &StmCommand) -> bool {
        let mut s = self.s();
        if !s.submit_result {
            return false;
        }
        s.submitted.push(cmd.clone());
        true
    }
    fn calib_info(&mut self) -> CalibInfo {
        self.s().calib
    }
    fn set_calib_info(&mut self, c: &CalibInfo) {
        let mut s = self.s();
        s.calib_infos.push(*c);
        s.calib = *c;
    }
    fn config_revision(&mut self) -> u32 {
        self.s().revision
    }
    fn calib_config(&mut self) -> CalibScheduleConfig {
        self.s().calib_cfg.clone()
    }
    fn load_calib_slot(&mut self) -> u32 {
        self.s().calib_slot
    }
    fn save_calib_slot(&mut self, slot: u32) {
        let mut s = self.s();
        s.saved_calib_slots.push(slot);
        s.calib_slot = slot;
    }
    fn load_last_calib(&mut self) -> i64 {
        self.s().last_calib
    }
    fn save_last_calib(&mut self, epoch: i64) {
        let mut s = self.s();
        s.saved_last_calibs.push(epoch);
        s.last_calib = epoch;
    }
    fn load_targets(&mut self, out: &mut [u8]) -> usize {
        let mut s = self.s();
        s.target_loads += 1;
        if s.targets.is_empty() || s.targets.len() > out.len() {
            return 0;
        }
        out[..s.targets.len()].copy_from_slice(&s.targets);
        s.targets.len()
    }
    fn save_targets(&mut self, data: &[u8]) -> bool {
        let mut s = self.s();
        s.target_saves += 1;
        if !s.save_targets_result || data.is_empty() {
            return false;
        }
        s.targets = data.to_vec();
        true
    }
    fn local_time(&mut self) -> LocalTime {
        self.s().local_time
    }
}

type Service<'a> = StmService<'a, TestPlatform, FakeHost>;

/// stm_service of a boot.
fn service<'a>(dev: &'a Device, shared: &'a StmServiceShared, host: &FakeHost) -> Service<'a> {
    let ports = StmServicePorts {
        clock: &dev.clock,
        wall: &dev.wall,
        rtc: &dev.rtc,
    };
    StmService::new(ports, shared, host.clone(), RECORDS)
}

/// Wednesday 2026-09-23 local (TZ UTC) hh:mm.
fn wednesday(hour: u8, minute: u8) -> LocalTime {
    LocalTime {
        valid: true,
        year: 2026,
        month: 9,
        mday: 23,
        wday: 3,
        hour,
        minute,
        second: 0,
        epoch: 1_790_121_600 + i64::from(hour) * 3600 + i64::from(minute) * 60,
    }
}

fn schedule_wednesday_at(host: &FakeHost, hour: u8) {
    let mut s = host.s();
    s.calib_cfg.day_mask = 1 << 3;
    s.calib_cfg.hour = hour;
    s.calib_cfg.minute = 0;
    s.revision = 1;
}

fn targets(valve: usize, pos: u8, src: TargetSource) -> PersistedTargets {
    let mut t = PersistedTargets::default();
    t.valid[valve] = true;
    t.pos[valve] = pos;
    t.source[valve] = src;
    t
}

fn encoded(t: &PersistedTargets) -> Vec<u8> {
    let mut b = [0u8; PERSISTED_TARGETS_SIZE];
    encode_targets(t, &mut b);
    b.to_vec()
}

/// Fires the Wednesday 03:00 slot at 03:05 (service at 10 s) and returns the submitted attempt.
fn fire(s: &mut Service<'_>, host: &FakeHost) -> u16 {
    schedule_wednesday_at(host, 3);
    host.s().local_time = wednesday(3, 5);
    s.begin();
    s.service(10_000);
    let sub = host.s().submitted.clone();
    assert_eq!(sub.len(), 1);
    sub[0].attempt
}

// ================================================================ scheduled calibration

#[test]
fn begin_restores_the_booked_slot_and_the_last_calibration_time() {
    let board = FakeBoard::new();
    let dev = board.boot();
    let shared = StmServiceShared::new();
    let host = FakeHost::default();
    let mut s = service(&dev, &shared, &host);
    host.s().calib_slot = 20_260_920;
    host.s().last_calib = 1_790_000_000;
    s.begin();
    let h = host.s();
    assert_eq!(h.calib_infos.len(), 1);
    assert_eq!(h.calib.last_scheduled_epoch, 1_790_000_000);
    assert_eq!(h.calib.next_slot, 0);
    assert_eq!(h.calib.next_epoch, 0);
}

#[test]
fn begin_the_restored_slot_is_booked_already() {
    // Rust addition: the booked slot of NVS counts, today's slot does not fire again
    let board = FakeBoard::new();
    let dev = board.boot();
    let shared = StmServiceShared::new();
    let host = FakeHost::default();
    let mut s = service(&dev, &shared, &host);
    schedule_wednesday_at(&host, 3);
    host.s().calib_slot = 20_260_923;
    host.s().local_time = wednesday(3, 5);
    s.begin();
    s.service(10_000);
    assert!(host.s().submitted.is_empty());
    assert_eq!(host.s().calib.next_slot, 20_260_930);
}

#[test]
fn a_due_slot_submits_one_scheduled_calibration_and_books_nothing_yet() {
    let board = FakeBoard::new();
    let dev = board.boot();
    let shared = StmServiceShared::new();
    let host = FakeHost::default();
    let mut s = service(&dev, &shared, &host);
    let attempt = fire(&mut s, &host);
    let c = host.s().submitted[0].clone();
    assert_eq!(c.kind, StmCommandType::Calibrate);
    assert_eq!(c.valve, ALL_VALVES);
    assert!(c.scheduled);
    assert_eq!(c.source, TargetSource::None);
    assert_eq!(attempt, 1);
    assert!(host.s().saved_calib_slots.is_empty());
    assert!(!host.has(EventCode::ScheduledCalibration));
    assert_eq!(host.s().calib.next_slot, 20_260_923); // not booked: still today
    s.service(20_000); // waiting for the STM: nothing more
    assert_eq!(host.s().submitted.len(), 1);
}

#[test]
fn the_stms_confirmation_books_the_slot_and_logs_it() {
    let board = FakeBoard::new();
    let dev = board.boot();
    let shared = StmServiceShared::new();
    let host = FakeHost::default();
    let mut s = service(&dev, &shared, &host);
    let attempt = fire(&mut s, &host);
    s.link()
        .post_scheduled_calib_result(attempt, true, CalibFailure::None);
    host.s().local_time = wednesday(3, 6);
    s.service(11_000);
    assert_eq!(host.s().saved_calib_slots, vec![20_260_923]);
    assert_eq!(host.s().saved_last_calibs, vec![wednesday(3, 6).epoch]);
    let e = host.first(EventCode::ScheduledCalibration);
    assert_eq!((e.arg1, e.arg2, e.valve), (20_260_923, 5, NO_VALVE));
    let ci = host.s().calib;
    assert_eq!(ci.last_scheduled_epoch, wednesday(3, 6).epoch);
    assert_eq!(ci.next_slot, 20_260_930);
    assert_eq!(ci.next_epoch, 1_790_121_600 + 7 * 86_400 + 3 * 3600);
    s.service(12_000); // the result is taken once
    assert_eq!(host.s().saved_calib_slots.len(), 1);
}

#[test]
fn the_next_slot_after_a_dst_change_takes_the_offset_in_force_then() {
    let board = FakeBoard::new();
    let dev = board.boot();
    dev.wall.set_time_zone(CET);
    let shared = StmServiceShared::new();
    let host = FakeHost::default();
    let mut s = service(&dev, &shared, &host);
    schedule_wednesday_at(&host, 3);
    // 2026-10-21 03:05 CEST = 01:05 UTC
    let mut t = wednesday(3, 5);
    t.month = 10;
    t.mday = 21;
    t.epoch = 1_792_544_700;
    host.s().local_time = t;
    s.begin();
    s.service(10_000);
    let attempt = host.s().submitted[0].attempt;
    s.link()
        .post_scheduled_calib_result(attempt, true, CalibFailure::None);
    t.minute = 6;
    t.epoch += 60;
    host.s().local_time = t;
    s.service(11_000);
    let ci = host.s().calib;
    assert_eq!(ci.next_slot, 20_261_028);
    assert_eq!(ci.next_epoch, 1_793_152_800); // 2026-10-28 03:00 CET = 02:00 UTC
}

#[test]
fn a_result_of_another_attempt_is_ignored() {
    let board = FakeBoard::new();
    let dev = board.boot();
    let shared = StmServiceShared::new();
    let host = FakeHost::default();
    let mut s = service(&dev, &shared, &host);
    let attempt = fire(&mut s, &host);
    s.link()
        .post_scheduled_calib_result(attempt + 1, true, CalibFailure::None);
    s.service(11_000);
    assert!(host.s().saved_calib_slots.is_empty());
    assert!(!host.has(EventCode::ScheduledCalibration));
    // the attempt still waits for its own result
    s.link()
        .post_scheduled_calib_result(attempt, true, CalibFailure::None);
    s.service(12_000);
    assert_eq!(host.s().saved_calib_slots, vec![20_260_923]);
}

#[test]
fn no_reply_is_reported_and_the_slot_fires_again_10_min_later() {
    let board = FakeBoard::new();
    let dev = board.boot();
    let shared = StmServiceShared::new();
    let host = FakeHost::default();
    let mut s = service(&dev, &shared, &host);
    let attempt = fire(&mut s, &host);
    s.link()
        .post_scheduled_calib_result(attempt, false, CalibFailure::NoReply);
    s.service(11_000);
    let e = host.first(EventCode::ScheduledCalibrationFailed);
    assert_eq!((e.arg1, e.arg2, e.valve), (20_260_923, 1, NO_VALVE));
    assert!(host.s().saved_calib_slots.is_empty());
    host.s().local_time = wednesday(3, 14);
    s.service(11_000 + 599_999);
    assert_eq!(host.s().submitted.len(), 1);
    host.s().local_time = wednesday(3, 15);
    s.service(11_000 + 600_000 + 10_000);
    let sub = host.s().submitted.clone();
    assert_eq!(sub.len(), 2);
    assert_eq!(sub[1].attempt, attempt + 1);
}

#[test]
fn without_a_result_within_60_s_the_attempt_failed_no_result() {
    let board = FakeBoard::new();
    let dev = board.boot();
    let shared = StmServiceShared::new();
    let host = FakeHost::default();
    let mut s = service(&dev, &shared, &host);
    fire(&mut s, &host);
    s.service(69_999);
    assert!(!host.has(EventCode::ScheduledCalibrationFailed));
    s.service(80_000);
    let e = host.first(EventCode::ScheduledCalibrationFailed);
    assert_eq!((e.arg1, e.arg2), (20_260_923, 3));
    // a late confirmation does not book it any more
    s.link()
        .post_scheduled_calib_result(1, true, CalibFailure::None);
    s.service(81_000);
    assert!(host.s().saved_calib_slots.is_empty());
}

#[test]
fn a_full_command_queue_is_a_failed_attempt_not_sent() {
    let board = FakeBoard::new();
    let dev = board.boot();
    let shared = StmServiceShared::new();
    let host = FakeHost::default();
    let mut s = service(&dev, &shared, &host);
    schedule_wednesday_at(&host, 3);
    host.s().local_time = wednesday(3, 1);
    host.s().submit_result = false;
    s.service(10_000);
    let e = host.first(EventCode::ScheduledCalibrationFailed);
    assert_eq!((e.arg1, e.arg2), (20_260_923, 2));
    assert!(host.s().saved_calib_slots.is_empty());
    assert!(!host.has(EventCode::ScheduledCalibration));
    assert_eq!(host.s().calib.next_slot, 20_260_923); // not booked: fires again 10 min later
    host.s().submit_result = true;
    host.s().local_time = wednesday(3, 11);
    s.service(610_000);
    assert_eq!(host.s().submitted.len(), 1);
    // the refused attempt was counted
    assert_eq!(host.s().submitted[0].attempt, 2);
}

#[test]
fn a_window_that_closes_without_a_confirmation_is_reported_as_missed() {
    let board = FakeBoard::new();
    let dev = board.boot();
    let shared = StmServiceShared::new();
    let host = FakeHost::default();
    let mut s = service(&dev, &shared, &host);
    let attempt = fire(&mut s, &host);
    s.link()
        .post_scheduled_calib_result(attempt, false, CalibFailure::NoReply);
    s.service(11_000);
    host.s().local_time = wednesday(5, 0);
    s.service(1_000_000);
    let e = host.first(EventCode::ScheduledCalibrationMissed);
    assert_eq!((e.arg1, e.arg2, e.valve), (20_260_923, 1, NO_VALVE));
    assert_eq!(e.severity, Severity::Error);
}

#[test]
fn the_schedule_is_evaluated_every_10_s_only() {
    let board = FakeBoard::new();
    let dev = board.boot();
    let shared = StmServiceShared::new();
    let host = FakeHost::default();
    let mut s = service(&dev, &shared, &host);
    schedule_wednesday_at(&host, 3);
    host.s().local_time = wednesday(3, 0);
    s.service(9_999);
    assert!(host.s().submitted.is_empty());
    assert!(host.s().calib_infos.is_empty());
    s.service(10_000);
    assert_eq!(host.s().submitted.len(), 1);
    assert_eq!(CALIBRATION_TICK_MS, 10_000);
}

#[test]
fn the_tick_follows_the_last_one_not_the_start() {
    // Rust addition: the 10 s run from the last evaluation
    let board = FakeBoard::new();
    let dev = board.boot();
    let shared = StmServiceShared::new();
    let host = FakeHost::default();
    let mut s = service(&dev, &shared, &host);
    schedule_wednesday_at(&host, 3);
    s.service(15_000);
    assert_eq!(host.s().calib_infos.len(), 1);
    s.service(24_999);
    assert_eq!(host.s().calib_infos.len(), 1);
    s.service(25_000);
    assert_eq!(host.s().calib_infos.len(), 2);
}

#[test]
fn missing_time_is_reported_once() {
    let board = FakeBoard::new();
    let dev = board.boot();
    let shared = StmServiceShared::new();
    let host = FakeHost::default();
    let mut s = service(&dev, &shared, &host);
    schedule_wednesday_at(&host, 3);
    s.service(3_600_001);
    let ev = host.with_code(EventCode::CalibTimeMissing);
    assert_eq!(ev.len(), 1);
    assert_eq!((ev[0].arg1, ev[0].arg2, ev[0].valve), (0, 0, NO_VALVE));
    s.service(3_700_001);
    assert_eq!(host.with_code(EventCode::CalibTimeMissing).len(), 1);
}

#[test]
fn the_schedule_follows_a_new_config_revision_only() {
    // Rust addition: the config is read again when its revision moved, not on every pass
    let board = FakeBoard::new();
    let dev = board.boot();
    let shared = StmServiceShared::new();
    let host = FakeHost::default();
    let mut s = service(&dev, &shared, &host);
    host.s().local_time = wednesday(3, 5);
    {
        let mut h = host.s();
        h.calib_cfg.day_mask = 0; // off
        h.calib_cfg.hour = 3;
        h.revision = 1;
    }
    s.service(10_000);
    assert!(host.s().submitted.is_empty());
    host.s().calib_cfg.day_mask = 1 << 3; // same revision: not seen
    s.service(20_000);
    assert!(host.s().submitted.is_empty());
    host.s().revision = 2;
    s.service(30_000);
    assert_eq!(host.s().submitted.len(), 1);
}

// ================================================================ desired targets (NVS)

#[test]
fn a_desired_target_change_is_written_to_nvs_5_min_later() {
    let board = FakeBoard::new();
    let dev = board.boot();
    let shared = StmServiceShared::new();
    let host = FakeHost::default();
    let mut s = service(&dev, &shared, &host);
    s.begin();
    s.link()
        .store_desired_targets(&targets(0, 42, TargetSource::Mqtt));
    s.service(1000);
    assert!(host.s().targets.is_empty());
    s.service(1000 + 299_999);
    assert!(host.s().targets.is_empty());
    s.service(1000 + 300_000);
    assert_eq!(
        host.s().targets,
        encoded(&targets(0, 42, TargetSource::Mqtt))
    );
}

#[test]
fn a_failed_nvs_write_is_retried_5_min_later() {
    let board = FakeBoard::new();
    let dev = board.boot();
    let shared = StmServiceShared::new();
    let host = FakeHost::default();
    let mut s = service(&dev, &shared, &host);
    s.begin();
    host.s().save_targets_result = false;
    s.link()
        .store_desired_targets(&targets(1, 7, TargetSource::Web));
    s.service(0);
    s.service(300_000);
    assert!(host.s().targets.is_empty());
    assert_eq!(host.s().target_saves, 1);
    host.s().save_targets_result = true;
    s.service(599_999);
    assert!(host.s().targets.is_empty());
    s.service(600_000);
    assert_eq!(host.s().targets, encoded(&targets(1, 7, TargetSource::Web)));
}

#[test]
fn the_value_nvs_holds_already_is_never_written_again() {
    let board = FakeBoard::new();
    let dev = board.boot();
    let shared = StmServiceShared::new();
    let host = FakeHost::default();
    let mut s = service(&dev, &shared, &host);
    host.s().targets = encoded(&targets(0, 42, TargetSource::Mqtt));
    s.begin();
    host.s().targets.clear();
    s.link()
        .store_desired_targets(&targets(0, 42, TargetSource::Mqtt));
    s.service(1000);
    s.service(10_000_000);
    assert!(host.s().targets.is_empty());
    assert_eq!(host.s().target_saves, 0);
}

#[test]
fn the_latest_handed_over_targets_win() {
    // Rust addition: the hand-over keeps the latest value only
    let board = FakeBoard::new();
    let dev = board.boot();
    let shared = StmServiceShared::new();
    let host = FakeHost::default();
    let mut s = service(&dev, &shared, &host);
    s.begin();
    let link = s.link();
    link.store_desired_targets(&targets(0, 42, TargetSource::Mqtt));
    link.store_desired_targets(&targets(1, 43, TargetSource::Web));
    s.flush_for_restart();
    assert_eq!(
        host.s().targets,
        encoded(&targets(1, 43, TargetSource::Web))
    );
}

#[test]
fn flush_for_restart_writes_dirty_targets_at_once_nothing_when_clean() {
    let board = FakeBoard::new();
    let dev = board.boot();
    let shared = StmServiceShared::new();
    let host = FakeHost::default();
    let mut s = service(&dev, &shared, &host);
    s.begin();
    s.flush_for_restart();
    assert!(host.s().targets.is_empty());
    s.link()
        .store_desired_targets(&targets(3, 9, TargetSource::Web));
    s.flush_for_restart();
    assert_eq!(host.s().targets, encoded(&targets(3, 9, TargetSource::Web)));
    host.s().targets.clear();
    s.flush_for_restart();
    assert!(host.s().targets.is_empty());
    assert_eq!(host.s().target_saves, 1);
}

// ================================================================ boot choice (RTC / NVS)

#[test]
fn after_power_on_only_nvs_targets_exist() {
    let board = FakeBoard::new();
    let dev = board.boot();
    let shared = StmServiceShared::new();
    let host = FakeHost::default();
    let mut s = service(&dev, &shared, &host);
    host.s().targets = encoded(&targets(2, 60, TargetSource::Web));
    s.begin();
    let (t, src) = s.link().boot_targets();
    assert_eq!(src, RestoreSource::Nvs);
    assert_eq!(t, targets(2, 60, TargetSource::Web));
    assert!(s.link().boot_lease().is_none());
    // what NVS holds is not written again
    s.flush_for_restart();
    assert_eq!(host.s().target_saves, 0);
}

#[test]
fn nothing_stored_boots_without_targets() {
    let board = FakeBoard::new();
    let dev = board.boot();
    let shared = StmServiceShared::new();
    let host = FakeHost::default();
    let mut s = service(&dev, &shared, &host);
    s.begin();
    let (t, src) = s.link().boot_targets();
    assert_eq!(src, RestoreSource::None);
    assert_eq!(t, PersistedTargets::default());
    assert_eq!(host.s().target_loads, 1);
}

#[test]
fn the_rtc_copies_survive_a_software_restart_and_win_over_nvs() {
    let board = FakeBoard::new();
    let lease = LeaseClientSnapshot {
        lost: true,
        active: true,
        mask: 0x005,
        lost_elapsed_ms: 123_456,
    };
    {
        let dev = board.boot();
        let shared = StmServiceShared::new();
        let host = FakeHost::default();
        let s = service(&dev, &shared, &host);
        s.link()
            .store_desired_targets(&targets(0, 33, TargetSource::Mqtt));
        s.link().store_lease_record(&lease);
        // the records sit at their places of the layout, in the C++ formats
        let rtc = dev.rtc.snapshot();
        assert_eq!(
            rtc[RECORDS.targets..RECORDS.targets + RTC_TARGETS_LEN],
            encoded(&targets(0, 33, TargetSource::Mqtt))[..]
        );
        let mut l = [0u8; RTC_LEASE_LEN];
        encode_lease_record(&lease, &mut l);
        assert_eq!(rtc[RECORDS.lease..RECORDS.lease + RTC_LEASE_LEN], l);
        board.reset(Reset::Software);
    }
    let dev = board.boot();
    let shared = StmServiceShared::new();
    let host = FakeHost::default();
    let mut s = service(&dev, &shared, &host);
    host.s().targets = encoded(&targets(0, 20, TargetSource::Web));
    s.begin();
    let (t, src) = s.link().boot_targets();
    assert_eq!(src, RestoreSource::Rtc);
    assert_eq!(t, targets(0, 33, TargetSource::Mqtt));
    assert_eq!(s.link().boot_lease(), Some(lease));
}

#[test]
fn rtc_targets_newer_than_nvs_are_written_to_nvs_equal_ones_are_not() {
    let board = FakeBoard::new();
    {
        let dev = board.boot();
        let shared = StmServiceShared::new();
        let host = FakeHost::default();
        let s = service(&dev, &shared, &host);
        s.link()
            .store_desired_targets(&targets(0, 33, TargetSource::Mqtt));
        board.reset(Reset::Software);
    }
    {
        let dev = board.boot();
        let shared = StmServiceShared::new();
        let host = FakeHost::default();
        let mut s = service(&dev, &shared, &host);
        host.s().targets = encoded(&targets(0, 20, TargetSource::Web));
        s.begin();
        s.flush_for_restart();
        assert_eq!(
            host.s().targets,
            encoded(&targets(0, 33, TargetSource::Mqtt))
        );
        board.reset(Reset::Software);
    }
    let dev = board.boot();
    let shared = StmServiceShared::new();
    let host = FakeHost::default();
    let mut s = service(&dev, &shared, &host);
    host.s().targets = encoded(&targets(0, 33, TargetSource::Mqtt));
    s.begin();
    host.s().targets.clear();
    s.flush_for_restart();
    assert!(host.s().targets.is_empty());
}

#[test]
fn rtc_targets_newer_than_nvs_are_saved_by_the_saver_5_min_after_boot() {
    // Rust addition: the boot time of the RTC copy starts the debounce of the saver
    let board = FakeBoard::new();
    {
        let dev = board.boot();
        let shared = StmServiceShared::new();
        let host = FakeHost::default();
        let s = service(&dev, &shared, &host);
        s.link()
            .store_desired_targets(&targets(4, 70, TargetSource::Web));
        board.reset(Reset::Panic);
    }
    let dev = board.boot();
    dev.clock.set_ms(2000);
    let shared = StmServiceShared::new();
    let host = FakeHost::default();
    let mut s = service(&dev, &shared, &host);
    s.begin();
    s.service(2000 + 299_999);
    assert!(host.s().targets.is_empty());
    s.service(2000 + 300_000);
    assert_eq!(
        host.s().targets,
        encoded(&targets(4, 70, TargetSource::Web))
    );
}

#[test]
fn a_power_on_forgets_the_rtc_copies() {
    let board = FakeBoard::new();
    {
        let dev = board.boot();
        let shared = StmServiceShared::new();
        let host = FakeHost::default();
        let s = service(&dev, &shared, &host);
        s.link()
            .store_desired_targets(&targets(0, 33, TargetSource::Mqtt));
        s.link().store_lease_record(&LeaseClientSnapshot::default());
        board.reset(Reset::PowerOn);
    }
    let dev = board.boot();
    let shared = StmServiceShared::new();
    let host = FakeHost::default();
    let mut s = service(&dev, &shared, &host);
    s.begin();
    let (_, src) = s.link().boot_targets();
    assert_eq!(src, RestoreSource::None);
    assert!(s.link().boot_lease().is_none());
}

#[test]
fn record_sizes_are_the_cpp_formats() {
    assert_eq!((RTC_TARGETS_LEN, RTC_LEASE_LEN), (46, 14));
}
