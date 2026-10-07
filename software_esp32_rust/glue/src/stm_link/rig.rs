//! The rig of the stm_link tests (C++ `glue_test.h` with the sibling fakes of app, storage,
//! stm_service, logger, mqtt and ota, and `support/fake_stm`): a booted device, the fake host,
//! the fake STM on the UART and the task driver (C++ `runTask`).
// host test code: the stack rule of the glue (design 2.4) is for the device
#![allow(clippy::large_stack_frames)]

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard};

use super::*;
use crate::testkit::board::TestPlatform;
use crate::testkit::{lock as tlock, run, Device, Ended, FakeBoard, FakeStm};
use vdm_esp_core::common::copy_string;
use vdm_esp_core::event_log::EventCode;
use vdm_esp_core::stm_types::StmCommandType;

/// The sibling fakes of app, storage, stm_service, logger, mqtt and ota (C++ `sib::*`).
pub(super) struct HostState {
    // app
    pub to_receive: VecDeque<StmCommand>,
    /// the last publish (C++ `sib::app().published`)
    pub published: Box<StmSnapshot>,
    pub publishes: u32,
    pub stored_profiles: Vec<Profile>,
    pub flash_marks: u32,
    pub save_state: StmSaveState,
    pub save_states: Vec<StmSaveState>,
    // storage
    pub cfg: Box<Config>,
    pub revision: u32,
    pub load_source: LoadSource,
    pub config_saved: bool,
    pub config_reads: u32,
    pub last_good_copies: Vec<Vec<u8>>,
    // stm_service
    pub boot_targets: PersistedTargets,
    pub boot_source: RestoreSource,
    pub boot_lease: Option<LeaseClientSnapshot>,
    pub stored_targets: Vec<PersistedTargets>,
    pub lease_records: Vec<LeaseClientSnapshot>,
    pub calib_results: Vec<(u16, bool, CalibFailure)>,
    // logger
    pub events: Vec<Event>,
    // mqtt
    pub regulator: RegulatorInput,
    // ota
    pub restart_pending: bool,
}

impl Default for HostState {
    fn default() -> Self {
        HostState {
            to_receive: VecDeque::new(),
            published: Box::default(),
            publishes: 0,
            stored_profiles: Vec::new(),
            flash_marks: 0,
            save_state: StmSaveState::Idle,
            save_states: Vec::new(),
            cfg: Box::default(),
            revision: 0,
            load_source: LoadSource::Stored,
            config_saved: false,
            config_reads: 0,
            last_good_copies: Vec::new(),
            boot_targets: PersistedTargets::default(),
            boot_source: RestoreSource::None,
            boot_lease: None,
            stored_targets: Vec::new(),
            lease_records: Vec::new(),
            calib_results: Vec::new(),
            events: Vec::new(),
            regulator: RegulatorInput::default(),
            restart_pending: false,
        }
    }
}

/// The host of the tests; clones share the state.
#[derive(Clone, Default)]
pub(super) struct FakeHost(Arc<Mutex<HostState>>);

impl FakeHost {
    pub fn s(&self) -> MutexGuard<'_, HostState> {
        tlock(&self.0)
    }
    pub fn with_code(&self, code: EventCode) -> Vec<Event> {
        self.s()
            .events
            .iter()
            .filter(|e| e.code == code)
            .cloned()
            .collect()
    }
    pub fn has(&self, code: EventCode) -> bool {
        !self.with_code(code).is_empty()
    }
    pub fn first(&self, code: EventCode) -> Event {
        match self.with_code(code).first() {
            Some(e) => e.clone(),
            None => panic!("no {code:?} event"),
        }
    }
    /// C++ `activeValve(v)`: the valve active in the config, a new revision.
    pub fn active_valve(&self, v: usize) {
        let mut s = self.s();
        s.cfg.valves[v].active = true;
        s.revision += 1;
    }
}

impl StmLinkHost for FakeHost {
    fn log_event(&mut self, e: &Event) {
        self.s().events.push(e.clone());
    }
    fn publish(&mut self, snap: &StmSnapshot) {
        let mut s = self.s();
        s.publishes += 1;
        StmSnapshot::clone_from(&mut s.published, snap);
    }
    fn store_profile(&mut self, p: &Profile) {
        self.s().stored_profiles.push(*p);
    }
    fn request_last_good_copy(&mut self, image: &[u8]) {
        self.s().last_good_copies.push(image.to_vec());
    }
    fn restart_pending(&mut self) -> bool {
        self.s().restart_pending
    }
    fn mark_flash_active(&mut self) {
        self.s().flash_marks += 1;
    }
    fn store_desired_targets(&mut self, t: &PersistedTargets) {
        self.s().stored_targets.push(*t);
    }
    fn post_scheduled_calib_result(&mut self, attempt: u16, ok: bool, reason: CalibFailure) {
        self.s().calib_results.push((attempt, ok, reason));
    }
    fn set_stm_save_state(&mut self, st: StmSaveState) {
        let mut s = self.s();
        s.save_states.push(st);
        s.save_state = st;
    }
    fn store_lease_record(&mut self, r: &LeaseClientSnapshot) {
        self.s().lease_records.push(*r);
    }
    fn config_revision(&mut self) -> u32 {
        self.s().revision
    }
    fn with_config(&mut self, f: &mut dyn FnMut(&Config)) {
        let mut s = self.s();
        s.config_reads += 1;
        f(&s.cfg);
    }
    fn boot_load_source(&mut self) -> LoadSource {
        self.s().load_source
    }
    fn config_saved_since_boot(&mut self) -> bool {
        self.s().config_saved
    }
    fn boot_targets(&mut self) -> (PersistedTargets, RestoreSource) {
        let s = self.s();
        (s.boot_targets, s.boot_source)
    }
    fn boot_lease(&mut self) -> Option<LeaseClientSnapshot> {
        self.s().boot_lease
    }
    fn receive(&mut self) -> Option<StmCommand> {
        self.s().to_receive.pop_front()
    }
    fn regulator_state(&mut self) -> RegulatorInput {
        self.s().regulator
    }
    fn stm_save_state(&mut self) -> StmSaveState {
        self.s().save_state
    }
}

pub(super) type Link<'a> = StmLink<'a, TestPlatform, FakeHost>;

/// One boot: the device and the host.
pub(super) struct Rig {
    pub dev: Device,
    pub host: FakeHost,
}

impl Rig {
    pub fn new() -> Rig {
        Rig {
            dev: FakeBoard::new().boot(),
            host: FakeHost::default(),
        }
    }

    /// The fake STM on Serial2 and NRST.
    pub fn stm(&self) -> FakeStm {
        FakeStm::attach(&self.dev.uart, &self.dev.gpio, &self.dev.clock)
    }

    /// The link over the device's UART, NRST (IO15), BOOT0 (IO14) and LittleFS.
    pub fn link(&self) -> Link<'_> {
        let ports = StmLinkPorts {
            clock: &self.dev.clock,
            fs: &self.dev.fs,
            heap: &self.dev.heap,
            uart: self.dev.uart.clone(),
            nrst: self.dev.gpio.output(15),
            boot0: self.dev.gpio.output(14),
        };
        StmLink::new(ports, self.host.clone())
    }

    /// C++ `runTask(ms)`: the thread (a fresh start) for `ms` of fake time, 2 ms per pass.
    pub fn run_task(&self, link: &mut Link<'_>, ms: u64) {
        self.run_passes(link, ms / 2);
    }

    /// The thread (a fresh start) for `n` passes: the thread body of `app::run_task`.
    pub fn run_passes(&self, link: &mut Link<'_>, n: u64) {
        self.dev.clock.stop_after_sleeps(n);
        let wd = self.dev.watchdog.clone();
        let clock = &self.dev.clock;
        assert_eq!(
            run(|| {
                link.start(wd);
                loop {
                    let d = link.pass();
                    clock.sleep_ms(d);
                }
            }),
            Ended::<()>::Stopped
        );
    }

    /// C++ `runWith(c, atMs, ms)`: `cmd` arrives at `at_ms` while the thread runs `ms`.
    pub fn run_with(&self, link: &mut Link<'_>, cmd: StmCommand, at_ms: u64, ms: u64) {
        self.push_at(at_ms, cmd);
        self.run_task(link, ms);
    }

    /// `cmd` arrives at `at_ms` (C++ `onDelay` pushing to `toReceive`).
    pub fn push_at(&self, at_ms: u64, cmd: StmCommand) {
        let clock = self.dev.clock.clone();
        let host = self.host.clone();
        self.dev.clock.on_sleep(move |_| {
            if clock.ms() == at_ms {
                host.s().to_receive.push_back(cmd.clone());
            }
        });
    }
}

/// A command of `kind` for `valve`.
pub(super) fn command(kind: StmCommandType, valve: u8) -> StmCommand {
    StmCommand {
        kind,
        valve,
        ..StmCommand::default()
    }
}

/// A flash of `/stm/<image>.bin`.
pub(super) fn flash(image: &str, blank: bool, board: &str) -> StmCommand {
    let mut c = command(StmCommandType::StartFlash, 0xFF);
    copy_string(&mut c.image, image.as_bytes());
    copy_string(&mut c.board, board.as_bytes());
    c.blank = blank;
    c
}

/// A 4 KiB image with vectors, the handshake strings and the board marker `tag`.
pub(super) fn image(tag: &str) -> Vec<u8> {
    let mut img = vec![0x80u8; 4096];
    img[0..4].copy_from_slice(&0x2002_0000u32.to_le_bytes());
    img[4..8].copy_from_slice(&0x0800_01C5u32.to_le_bytes());
    let mut strs = b"\x01DEADBEEF\0\x01BEEFIT\0".to_vec();
    strs.extend_from_slice(b"VDM-HW:");
    strs.extend_from_slice(tag.as_bytes());
    strs.push(0);
    img[2000..2000 + strs.len()].copy_from_slice(&strs);
    img
}
