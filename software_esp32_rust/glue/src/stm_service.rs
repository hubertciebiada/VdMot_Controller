//! App-thread side of the STM link (C++ `stm_service.cpp`, `stm_service.h`): the scheduled
//! calibration (DESIGN.md section 14, booked only when the STM confirmed its `staln`), the NVS
//! copy of the desired targets and the RTC records that survive ESP software restarts (desired
//! targets and the ESP failsafe emulation). It talks to the stm thread through the command queue
//! and the hand-over of [`StmServiceShared`].
//!
//! Parts:
//! - [`StmService`]: the app thread (C++ `begin`, `service`, `flushForRestart` and their file
//!   statics); its calls into other glue modules go through [`StmServiceHost`].
//! - [`StmServiceShared`]: the hand-over from the stm thread (the C++ `gMux` section) and the
//!   boot choice the stm thread starts with.
//! - [`StmServiceLink`]: the stm thread's calls (C++ `storeDesiredTargets`, `storeLeaseRecord`,
//!   `postScheduledCalibResult`, `bootTargets`, `bootLease`), over the shared part and the RTC.
//!
//! The RTC records keep the C++ formats (core `encode_targets`, `encode_lease_record`, each with
//! its own check) at the places the firmware's RTC layout gives ([`RtcRecords`]); after a
//! power-on they fail their check and count as absent.

use std::sync::{Mutex, MutexGuard, PoisonError};

use vdm_esp_core::calib_schedule::{calib_slot_epoch, CalibDecision, CalibFailure, CalibScheduler};
use vdm_esp_core::common::{elapsed_ms, LocalTime, ALL_VALVES, NO_VALVE};
use vdm_esp_core::config::CalibScheduleConfig;
use vdm_esp_core::event_log::{event_default_severity, make_event, Event, EventCode};
use vdm_esp_core::lease_client::{
    decode_lease_record, encode_lease_record, LeaseClientSnapshot, LEASE_RECORD_SIZE,
};
use vdm_esp_core::stm_types::{StmCommand, StmCommandType};
use vdm_esp_core::target_store::{
    choose_targets, decode_targets, encode_targets, PersistedTargets, RestoreSource, TargetSaver,
    PERSISTED_TARGETS_SIZE,
};

use crate::port::{Clock, Platform, Rtc, WallClock};
use crate::shared::CalibInfo;

/// The calibration schedule is evaluated this often (its STM results at once).
pub const CALIBRATION_TICK_MS: u32 = 10_000;
/// Size of the RTC record of the desired targets (the NVS `targets` format).
pub const RTC_TARGETS_LEN: usize = PERSISTED_TARGETS_SIZE;
/// Size of the RTC record of the ESP failsafe emulation.
pub const RTC_LEASE_LEN: usize = LEASE_RECORD_SIZE;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // a panic aborts the firmware, so a poisoned lock exists only in a failing test
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Places of the two RTC records in the firmware's RTC layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RtcRecords {
    /// [`RTC_TARGETS_LEN`] bytes: the desired targets.
    pub targets: usize,
    /// [`RTC_LEASE_LEN`] bytes: the ESP failsafe emulation.
    pub lease: usize,
}

/// The result of a scheduled `staln` (C++ `CalibResult`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CalibResult {
    attempt: u16,
    ok: bool,
    reason: CalibFailure,
}

/// stm thread -> app thread (C++ `gHandTargets`, `gHandResult` and their flags under `gMux`):
/// the latest value of each, taken once.
#[derive(Default)]
struct HandOver {
    targets: Option<PersistedTargets>,
    result: Option<CalibResult>,
}

/// What [`StmService::begin`] chose for the stm thread's start.
#[derive(Default)]
struct BootChoice {
    targets: PersistedTargets,
    source: RestoreSource,
    lease: Option<LeaseClientSnapshot>,
}

/// The hand-over between the stm thread and the app thread, and the boot choice. `main`
/// creates it at boot, a test per case.
#[derive(Default)]
pub struct StmServiceShared {
    hand: Mutex<HandOver>,
    boot: Mutex<BootChoice>,
}

impl StmServiceShared {
    /// Nothing handed over, no boot targets, no lease record.
    pub fn new() -> Self {
        Self::default()
    }

    fn take_targets(&self) -> Option<PersistedTargets> {
        lock(&self.hand).targets.take()
    }

    fn take_result(&self) -> Option<CalibResult> {
        lock(&self.hand).result.take()
    }
}

/// The stm thread's calls into stm_service: the RTC copies and the hand-over to the app thread.
pub struct StmServiceLink<'a, R> {
    shared: &'a StmServiceShared,
    rtc: &'a R,
    records: RtcRecords,
}

impl<'a, R: Rtc> StmServiceLink<'a, R> {
    /// The calls over the shared part and the RTC records at `records`.
    pub fn new(shared: &'a StmServiceShared, rtc: &'a R, records: RtcRecords) -> Self {
        StmServiceLink {
            shared,
            rtc,
            records,
        }
    }

    /// Desired targets changed: the RTC copy at once, the NVS copy by [`StmService::service`].
    pub fn store_desired_targets(&self, t: &PersistedTargets) {
        let mut bytes = [0u8; RTC_TARGETS_LEN];
        encode_targets(t, &mut bytes);
        self.rtc.store(self.records.targets, &bytes);
        lock(&self.shared.hand).targets = Some(*t);
    }

    /// The ESP failsafe emulation for the next software restart (RTC).
    pub fn store_lease_record(&self, s: &LeaseClientSnapshot) {
        let mut bytes = [0u8; RTC_LEASE_LEN];
        encode_lease_record(s, &mut bytes);
        self.rtc.store(self.records.lease, &bytes);
    }

    /// Result of the scheduled staln of `attempt` (`StmCommand::attempt`).
    pub fn post_scheduled_calib_result(&self, attempt: u16, ok: bool, reason: CalibFailure) {
        lock(&self.shared.hand).result = Some(CalibResult {
            attempt,
            ok,
            reason,
        });
    }

    /// The boot copy of the desired targets [`StmService::begin`] chose, and where it came from.
    pub fn boot_targets(&self) -> (PersistedTargets, RestoreSource) {
        let b = lock(&self.shared.boot);
        (b.targets, b.source)
    }

    /// The lease record of the RTC; `None` without a valid record.
    pub fn boot_lease(&self) -> Option<LeaseClientSnapshot> {
        lock(&self.shared.boot).lease
    }
}

/// What stm_service calls in the other glue modules (C++ `app::`, `storage::`, `net::` and
/// `logger::`), one method per C++ call; the firmware wiring implements it.
pub trait StmServiceHost {
    /// C++ `logger::log(code, valve, arg1, arg2)`.
    fn log(&mut self, e: &Event);
    /// C++ `app::submit(c)`: false when the queue is full.
    fn submit(&mut self, cmd: &StmCommand) -> bool;
    /// C++ `app::calibInfo()`.
    fn calib_info(&mut self) -> CalibInfo;
    /// C++ `app::setCalibInfo(c)`.
    fn set_calib_info(&mut self, c: &CalibInfo);
    /// C++ `storage::configRevision()`.
    fn config_revision(&mut self) -> u32;
    /// C++ `storage::calibConfig()`: the schedule of the active config.
    fn calib_config(&mut self) -> CalibScheduleConfig;
    /// C++ `storage::loadCalibSlot()`: the booked slot, 0 = none.
    fn load_calib_slot(&mut self) -> u32;
    /// C++ `storage::saveCalibSlot(slot)`.
    fn save_calib_slot(&mut self, slot: u32);
    /// C++ `storage::loadLastCalib()`: epoch of the last calibration, 0 = none.
    fn load_last_calib(&mut self) -> i64;
    /// C++ `storage::saveLastCalib(epoch)`.
    fn save_last_calib(&mut self, epoch: i64);
    /// C++ `storage::loadTargets(out, cap)`: the stored targets, their length (0: none).
    fn load_targets(&mut self, out: &mut [u8]) -> usize;
    /// C++ `storage::saveTargets(data, len)`.
    fn save_targets(&mut self, data: &[u8]) -> bool;
    /// C++ `net::localTime()`: valid only once SNTP set the clock.
    fn local_time(&mut self) -> LocalTime;
}

/// The ports of stm_service: the clock, the wall clock of the slot epochs and the RTC records.
pub struct StmServicePorts<'a, P: Platform> {
    pub clock: &'a P::Clock,
    pub wall: &'a P::WallClock,
    pub rtc: &'a P::Rtc,
}

/// The app thread's part of stm_service (C++ `begin`, `service`, `flushForRestart`).
pub struct StmService<'a, P: Platform, H: StmServiceHost> {
    clock: &'a P::Clock,
    wall: &'a P::WallClock,
    rtc: &'a P::Rtc,
    records: RtcRecords,
    shared: &'a StmServiceShared,
    host: H,
    /// The only part of the config read here (C++ `gCalibCfg`).
    calib_cfg: CalibScheduleConfig,
    cfg_revision: u32,
    calib: CalibScheduler,
    last_calib_ms: u32,
    attempt_id: u16,
    saver: TargetSaver,
}

impl<'a, P: Platform, H: StmServiceHost> StmService<'a, P, H> {
    /// Nothing restored yet ([`StmService::begin`] follows); `records`: the places of the RTC
    /// records in the firmware's RTC layout.
    pub fn new(
        ports: StmServicePorts<'a, P>,
        shared: &'a StmServiceShared,
        host: H,
        records: RtcRecords,
    ) -> Self {
        StmService {
            clock: ports.clock,
            wall: ports.wall,
            rtc: ports.rtc,
            records,
            shared,
            host,
            calib_cfg: CalibScheduleConfig::default(),
            cfg_revision: 0,
            calib: CalibScheduler::default(),
            last_calib_ms: 0,
            attempt_id: 0,
            saver: TargetSaver::default(),
        }
    }

    /// The stm thread's calls over the same shared part and RTC records.
    pub fn link(&self) -> StmServiceLink<'a, P::Rtc> {
        StmServiceLink::new(self.shared, self.rtc, self.records)
    }

    /// `app::setup`, after the config was loaded: the booked slot and the last calibration time,
    /// the boot copy of the desired targets (RTC, else NVS; NVS catches up with newer RTC
    /// targets) and the RTC lease record.
    pub fn begin(&mut self) {
        let slot = self.host.load_calib_slot();
        self.calib.restore_last_slot(slot);
        let last = self.host.load_last_calib();
        self.set_calib_info(last, 0, &LocalTime::default());
        let mut nvs = [0u8; PERSISTED_TARGETS_SIZE];
        let n = self.host.load_targets(&mut nvs).min(nvs.len());
        let mut rtc = [0u8; RTC_TARGETS_LEN];
        self.rtc.load(self.records.targets, &mut rtc);
        let mut targets = PersistedTargets::default();
        let source = choose_targets(&rtc, &nvs[..n], &mut targets);
        let mut stored = PersistedTargets::default();
        decode_targets(&nvs[..n], &mut stored);
        self.saver.prime_stored(&stored);
        // targets newer than NVS (a restart without a flush): NVS catches up
        if source == RestoreSource::Rtc {
            self.saver.update(&targets, self.clock.now_ms());
        }
        let mut lease = [0u8; RTC_LEASE_LEN];
        self.rtc.load(self.records.lease, &mut lease);
        *lock(&self.shared.boot) = BootChoice {
            targets,
            source,
            lease: decode_lease_record(&lease),
        };
    }

    /// App thread, every second: the scheduled calibration (evaluated every
    /// [`CALIBRATION_TICK_MS`], its STM results at once) and the desired-target NVS saver.
    pub fn service(&mut self, now_ms: u32) {
        let revision = self.host.config_revision();
        if revision != self.cfg_revision {
            self.cfg_revision = revision;
            self.calib_cfg = self.host.calib_config();
        }
        self.calibration_result(now_ms);
        self.targets_tick(now_ms);
        if elapsed_ms(now_ms, self.last_calib_ms) < CALIBRATION_TICK_MS {
            return;
        }
        self.last_calib_ms = now_ms;
        self.calibration_tick(now_ms);
    }

    /// App thread, before an ESP restart (not for a factory reset): the desired targets go to
    /// NVS now when they differ from what NVS holds.
    pub fn flush_for_restart(&mut self) {
        let now = self.clock.now_ms();
        if let Some(t) = self.shared.take_targets() {
            self.saver.update(&t, now);
        }
        if self.saver.dirty() {
            self.save_targets(now);
        }
    }

    fn log(&mut self, code: EventCode, arg1: i32, arg2: i32) {
        let e = make_event(
            code,
            event_default_severity(code),
            NO_VALVE,
            arg1,
            arg2,
            b"",
        );
        self.host.log(&e);
    }

    /// Epoch of `slot` at `calib.hour:calib.minute` local time, 0 = none. The second pass takes
    /// the UTC offset in force at the slot (a DST change before it); like the C++ `tm` copy, the
    /// seconds and the weekday of the reference stay 0.
    fn slot_epoch(&self, slot: u32, now: &LocalTime) -> i64 {
        let (hour, minute) = (self.calib_cfg.hour, self.calib_cfg.minute);
        let first = calib_slot_epoch(slot, hour, minute, now);
        // a conversion that fails leaves no valid reference: 0
        let at = self
            .wall
            .local_time(first)
            .map_or_else(LocalTime::default, |t| LocalTime {
                valid: true,
                year: t.year,
                month: t.month,
                mday: t.mday,
                hour: t.hour,
                minute: t.minute,
                epoch: first,
                ..LocalTime::default()
            });
        calib_slot_epoch(slot, hour, minute, &at)
    }

    fn set_calib_info(&mut self, last_epoch: i64, next_slot: u32, now: &LocalTime) {
        let ci = CalibInfo {
            last_scheduled_epoch: last_epoch,
            next_slot,
            next_epoch: self.slot_epoch(next_slot, now),
        };
        self.host.set_calib_info(&ci);
    }

    fn failed(&mut self, reason: CalibFailure) {
        let slot = self.calib.attempt_slot() as i32;
        self.log(EventCode::ScheduledCalibrationFailed, slot, reason as i32);
    }

    fn calibration_tick(&mut self, now_ms: u32) {
        let lt = self.host.local_time();
        match self.calib.evaluate(&self.calib_cfg, &lt, now_ms) {
            CalibDecision::Fire => {
                self.attempt_id = self.attempt_id.wrapping_add(1);
                // the default source is the C++ TargetSource::None
                let c = StmCommand {
                    kind: StmCommandType::Calibrate,
                    valve: ALL_VALVES,
                    scheduled: true,
                    attempt: self.attempt_id,
                    ..StmCommand::default()
                };
                if !self.host.submit(&c) {
                    self.calib.on_result(false, now_ms);
                    self.failed(CalibFailure::NotSent);
                }
            }
            CalibDecision::SkippedNoTime => self.log(EventCode::CalibTimeMissing, 0, 0),
            CalibDecision::NoResult => self.failed(CalibFailure::NoResult),
            CalibDecision::Missed => {
                let slot = self.calib.attempt_slot() as i32;
                let attempts = i32::from(self.calib.attempts());
                self.log(EventCode::ScheduledCalibrationMissed, slot, attempts);
            }
            CalibDecision::None => {}
        }
        let last = self.host.calib_info().last_scheduled_epoch;
        let next = self.calib.next_slot(&self.calib_cfg, &lt);
        self.set_calib_info(last, next, &lt);
    }

    /// The STM confirmed (or not) the staln of the current attempt.
    fn calibration_result(&mut self, now_ms: u32) {
        let Some(r) = self.shared.take_result() else {
            return;
        };
        // another attempt's result, or NoResult was reported already
        if r.attempt != self.attempt_id || !self.calib.attempt_pending() {
            return;
        }
        if !r.ok {
            self.calib.on_result(false, now_ms);
            self.failed(r.reason);
            return;
        }
        self.calib.on_result(true, now_ms);
        let lt = self.host.local_time();
        let slot = self.calib.last_slot();
        self.host.save_calib_slot(slot);
        self.host.save_last_calib(lt.epoch);
        let late = i32::from(self.calib.late_minutes());
        self.log(EventCode::ScheduledCalibration, slot as i32, late);
        let next = self.calib.next_slot(&self.calib_cfg, &lt);
        self.set_calib_info(lt.epoch, next, &lt);
    }

    fn save_targets(&mut self, now_ms: u32) {
        let ok = self.host.save_targets(self.saver.bytes());
        self.saver.saved(ok, now_ms);
    }

    fn targets_tick(&mut self, now_ms: u32) {
        if let Some(t) = self.shared.take_targets() {
            self.saver.update(&t, now_ms);
        }
        if self.saver.due(now_ms) {
            self.save_targets(now_ms);
        }
    }
}

#[cfg(test)]
mod tests;
