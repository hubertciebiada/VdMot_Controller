//! The STM link as one hardware-free object: runs LinkPolicy, PollPlanner, ValveModel,
//! SensorModel, RebootDetector, HealthMonitor, LeaseClient, LearnTimeSync, ResetGate and the
//! StmFlasher, executes the commands of the other tasks and builds the STM snapshot. The
//! stm_link glue owns the UART, the NRST pin and the task loop and implements
//! [`StmSessionPort`] (port of `vdm/stm_session.h`).
//!
//! The C++ session keeps references to its port, its flash transport and its working snapshot.
//! The Rust session owns its port and its snapshot (the glue boxes the whole session, about the
//! size of the C++ object and its snapshot); the flash transport is an argument of
//! [`StmSession::flash_step`], the only call that uses it, so the glue keeps the UART for the
//! request lines between flash runs. The image of a flash run stays with the port, as in C++:
//! [`StmSessionPort::open_image`], [`image`](StmSessionPort::image) and
//! [`close_image`](StmSessionPort::close_image).

use crate::calib_schedule::{stm_learn_time, CalibFailure, LearnTimeSync};
use crate::common::{
    c_str, copy_string, elapsed_ms, format_one_wire_id, is_zero, OneWireId, Text, ALL_VALVES,
    NO_VALVE, TEMP_SLOT_COUNT, VALVE_COUNT, VOLT_SLOT_COUNT,
};
use crate::config::Config;
use crate::event_log::{
    event_default_severity, make_event, Event, EventCode, Severity, EVENT_TEXT_MAX,
};
use crate::failsafe::{
    regulator_cause, LeaseMode, LeaseState, LeaseStatus, RegulatorCause, RegulatorInput,
};
use crate::health_monitor::{HealthMonitor, MAX_EVENTS_PER_UPDATE};
use crate::lease_client::{effective_lease_config, LeaseClient, LeaseClientSnapshot};
use crate::line_assembler::{LineAssembler, STM_MAX_LINE_LEN};
use crate::link_policy::{
    Completion, EnqueueResult, LinkPolicy, LinkState, Outcome, Priority, RebootDetector,
    RebootDetectorRecovery,
};
use crate::poll_planner::{PollPlanner, ResyncStep};
use crate::reset_gate::{ResetGate, ResetGateState};
use crate::stm_codec::{
    build_assembly, build_calibrate, build_detect, build_eeprom_state, build_leave_safe_mode,
    build_match_sensors, build_scan_one_wire, build_service_move, build_set_breakaway,
    build_set_learn_movements, build_set_motor_chars, build_set_target, build_set_valve_sensors,
    build_stop, parse_reply, Cmd, ParseStatus, Profile, Reply, RequestLine, StmStatus,
};
use crate::stm_flasher::{
    board_tag_valid, flash_phase_name, FlashError, FlashImage, FlashOptions, FlashPhase,
    FlashTransport, StmFlasher,
};
use crate::stm_types::{StmCommand, StmCommandType, StmSaveState, StmSnapshot};
use crate::target_store::{capture_targets, restore_targets, PersistedTargets, RestoreSource};
use crate::valve_model::{
    expect_sensor, temp_raw_valid, vad_valid, SensorModel, ValveModel, ValveState,
};
use crate::version::{
    compare_version, format_version, min_stm_version, parse_version, StmSupport, Version,
};

/// What the session needs from the firmware around it.
pub trait StmSessionPort {
    fn log_event(&mut self, e: &Event);
    /// Pulses NRST (100 ms); returns the time after the pulse.
    fn pulse_reset(&mut self, now_ms: u32) -> u32;
    fn publish(&mut self, s: &StmSnapshot);
    /// A gprof reply (`p.valve < VALVE_COUNT`): the profile store of the firmware keeps it; the
    /// next snapshot counts it in `profile_seq[p.valve]`.
    fn store_profile(&mut self, p: &Profile);
    /// The flashed image becomes the last good one.
    fn request_last_good_copy(&mut self, image: &[u8]);
    /// An ESP restart is due (a flash would be cut).
    fn restart_pending(&mut self) -> bool;
    /// The flash is starting: other tasks see it before the next snapshot.
    fn mark_flash_active(&mut self);
    /// Desired targets changed: RTC copy and the hand-over to the NVS saver.
    fn store_desired_targets(&mut self, t: &PersistedTargets);
    fn post_scheduled_calib_result(&mut self, attempt: u16, ok: bool, reason: CalibFailure);
    fn set_stm_save_state(&mut self, s: StmSaveState);
    /// Once per second: the lease emulation for the RTC record.
    fn store_lease_record(&mut self, s: &LeaseClientSnapshot);
    /// Opens the image of a flash run (the C++ `openImage` returning a pointer); false when it
    /// cannot be opened. It is closed ([`close_image`](Self::close_image)) when the run ends or
    /// does not start.
    fn open_image(&mut self, name: &[u8]) -> bool;
    /// The image [`open_image`](Self::open_image) opened (the pointer the C++ call returned):
    /// every flasher step of the run reads it.
    fn image(&mut self) -> &mut dyn FlashImage;
    fn close_image(&mut self);
}

/// What waits for the STM EEPROM ([`ResetGate`]); the number is arg2 of
/// `StmEepromWaitTimeout` (C++ `StmSession::GateAction`).
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GateAction {
    None = 0,
    StmReset = 1,
    Flash = 2,
    EspRestart = 3,
}

/// An accepted svmov whose result (a new move of the valve) is awaited (C++
/// `StmSession::PendingMove`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct PendingMove {
    active: bool,
    move_seq: u32,
    since_ms: u32,
}

/// Failure edge tracking of one sensor slot (C++ `StmSession::SlotTrack`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct SlotTrack {
    known: bool,
    valid: bool,
}

/// mode, state, failsafe_mask, regulator, config_synced, config_failed, config_trusted,
/// timeout_min
type LeaseKey = (
    LeaseMode,
    LeaseState,
    u16,
    RegulatorCause,
    bool,
    bool,
    bool,
    u16,
);

/// The lease status fields whose change publishes a snapshot (remain_s and regulator_lost_s
/// count down by themselves and do not).
fn lease_key(s: &LeaseStatus) -> LeaseKey {
    (
        s.mode,
        s.state,
        s.failsafe_mask,
        s.regulator,
        s.config_synced,
        s.config_failed,
        s.config_trusted,
        s.timeout_min,
    )
}

/// An event with the default severity of its code.
fn event(code: EventCode, valve: u8, a1: i32, a2: i32, text: &[u8]) -> Event {
    make_event(code, event_default_severity(code), valve, a1, a2, text)
}

/// The valves whose bit is set in `mask`.
fn valves_of(mask: u16) -> impl Iterator<Item = u8> {
    (0..VALVE_COUNT).filter(move |v| (mask >> v) & 1 != 0)
}

const TEMPS: usize = TEMP_SLOT_COUNT as usize;
const VOLTS: usize = VOLT_SLOT_COUNT as usize;
const VALVES: usize = VALVE_COUNT as usize;

pub struct StmSession<P: StmSessionPort> {
    port: P,
    lines: LineAssembler<{ STM_MAX_LINE_LEN + 1 }>,
    link: LinkPolicy,
    planner: PollPlanner,
    model: ValveModel,
    sensors: SensorModel,
    reboot: RebootDetector,
    health: HealthMonitor,
    lease: LeaseClient,
    learn: LearnTimeSync,
    gate: ResetGate,
    flasher: StmFlasher,
    reply: Reply,

    // Sensor slots of the config (the valve sensor assignment, failure edges).
    slot_ids: [OneWireId; TEMPS],
    temp_active: [bool; TEMPS],
    volt_ids: [OneWireId; VOLTS],
    volt_active: [bool; VOLTS],
    snap: StmSnapshot,
    prev: [ValveState; VALVES],
    prev_link: LinkState,
    dirty: bool,
    published_once: bool,
    last_publish_ms: u32,
    min_version: Version,
    /// C++ `char[32]`
    logged_version: Text<31>,
    incompatible_logged: bool,
    prev_support: StmSupport,
    have_prev_status: bool,
    prev_status: StmStatus,
    last_desired_rev: u32,
    last_lease: LeaseStatus,

    /// CalibStarted of these valves carries arg1 = 1
    scheduled_mask: u16,
    scheduled_at_ms: u32,
    /// attempt of the scheduled staln in the queue
    sched_attempt: u16,
    moves: [PendingMove; VALVES],

    temp_track: [SlotTrack; TEMPS],
    volt_track: [SlotTrack; VOLTS],
    sensor_grace_from_ms: u32,
    sensor_grace: bool,
    last_temp_count: u8,
    last_volt_count: u8,
    counts_known: bool,

    gate_action: GateAction,
    /// the gate began with an answering STM
    gate_answered: bool,
    pending_flash: StmCommand,
    flash_pending: bool,
}

impl<P: StmSessionPort> StmSession<P> {
    /// DESIGN.md: temps stale after 60 s
    pub const SENSOR_STALE_MS: u32 = 60_000;
    /// after a bus scan / STM reset: lists re-read
    pub const SENSOR_GRACE_MS: u32 = 30_000;
    /// svmov accepted -> last_move expected
    pub const SERVICE_MOVE_WAIT_MS: u32 = 300_000;
    pub const SCHEDULED_CALIB_WINDOW_MS: u32 = 4 * 3600 * 1000;
    pub const PUBLISH_MIN_MS: u32 = 100;
    pub const TAG_SCHEDULED_CALIB: u16 = 1;
    pub const STM_BAUD: u32 = 115_200;

    pub fn new(port: P) -> Self {
        Self {
            port,
            lines: LineAssembler::new(),
            link: LinkPolicy::default(),
            planner: PollPlanner::default(),
            model: ValveModel::default(),
            sensors: SensorModel::default(),
            reboot: RebootDetector::default(),
            health: HealthMonitor::default(),
            lease: LeaseClient::default(),
            learn: LearnTimeSync::default(),
            gate: ResetGate::default(),
            flasher: StmFlasher::new(),
            reply: Reply::default(),
            slot_ids: [OneWireId::default(); TEMPS],
            temp_active: [false; TEMPS],
            volt_ids: [OneWireId::default(); VOLTS],
            volt_active: [false; VOLTS],
            snap: StmSnapshot::default(),
            prev: [ValveState::EMPTY; VALVES],
            prev_link: LinkState::Unknown,
            dirty: true,
            published_once: false,
            last_publish_ms: 0,
            min_version: parse_version(min_stm_version().as_bytes()),
            logged_version: Text::new(),
            incompatible_logged: false,
            prev_support: StmSupport::Unknown,
            have_prev_status: false,
            prev_status: StmStatus::default(),
            last_desired_rev: 0,
            last_lease: LeaseStatus::default(),
            scheduled_mask: 0,
            scheduled_at_ms: 0,
            sched_attempt: 0,
            moves: [PendingMove::default(); VALVES],
            temp_track: [SlotTrack::default(); TEMPS],
            volt_track: [SlotTrack::default(); VOLTS],
            sensor_grace_from_ms: 0,
            sensor_grace: true,
            last_temp_count: 0,
            last_volt_count: 0,
            counts_known: false,
            gate_action: GateAction::None,
            gate_answered: false,
            pending_flash: StmCommand::default(),
            flash_pending: false,
        }
    }

    // ------------------------------------------------------------ helpers

    fn log(&mut self, code: EventCode, valve: u8, a1: i32, a2: i32, text: &[u8]) {
        self.port.log_event(&event(code, valve, a1, a2, text));
    }

    fn log_events(&mut self, ev: &[Event]) {
        for e in ev {
            self.port.log_event(e);
        }
    }

    fn enqueue(&mut self, r: &RequestLine, p: Priority, tag: u16) -> bool {
        let res = self.link.enqueue(r, p, tag);
        if res == EnqueueResult::Full {
            self.log(EventCode::StmQueueFull, NO_VALVE, r.cmd as i32, 0, b"");
        }
        matches!(res, EnqueueResult::Queued | EnqueueResult::Coalesced)
    }

    fn stm_answers(&self, now_ms: u32) -> bool {
        matches!(self.link.state(now_ms), LinkState::Up | LinkState::Degraded)
    }

    fn too_old(&self) -> bool {
        self.planner.support() == StmSupport::TooOld
    }

    fn start_sensor_grace(&mut self, now_ms: u32) {
        self.sensor_grace = true;
        self.sensor_grace_from_ms = now_ms;
    }

    // ------------------------------------------------------------ config and start

    /// Config (initially and whenever it changed). `trusted` false: the ESP runs on defaults
    /// nobody saved (no lease config push).
    pub fn apply_config(&mut self, cfg: &Config, trusted: bool) {
        // Disjoint bits: `+` is the C++ `|`.
        let mask = (0u16..)
            .zip(&cfg.valves)
            .filter(|(_, v)| v.active)
            .fold(0u16, |m, (i, _)| m + (1 << i));
        self.model.set_active_mask(mask);
        self.planner.set_active_mask(mask);
        let mut ids_changed = false;
        let slots = self.slot_ids.iter_mut().zip(self.temp_active.iter_mut());
        for (t, (id, active)) in cfg.temps.iter().zip(slots) {
            if *id != t.id {
                ids_changed = true;
            }
            *id = t.id;
            *active = t.active;
        }
        let slots = self.volt_ids.iter_mut().zip(self.volt_active.iter_mut());
        for (t, (id, active)) in cfg.volts.iter().zip(slots) {
            *id = t.id;
            *active = t.active;
        }
        // Slot ids changed: re-resolve the valve sensor assignment.
        if ids_changed {
            self.planner.request_valve_sensors();
        }
        self.lease.set_config(&effective_lease_config(cfg));
        self.lease.set_config_trusted(trusted);
        self.learn.set_desired(stm_learn_time(&cfg.calib));
        self.dirty = true;
    }

    /// ESP boot, after the first [`apply_config`](Self::apply_config): restores the desired
    /// targets (TargetsRestored) and the lease record, starts the re-sync behind the STM
    /// start-up hold-off (the IO15 strap may have reset it).
    pub fn begin(
        &mut self,
        now_ms: u32,
        targets: &PersistedTargets,
        src: RestoreSource,
        lease: Option<&LeaseClientSnapshot>,
    ) {
        self.start_sensor_grace(now_ms);
        self.planner.request_resync();
        // R6: no reset here, but the IO15 strap most likely reset the STM while the ESP booted;
        // give it the same start-up hold-off as after a pulse.
        self.link.hold_after_esp_boot(now_ms);
        if let Some(lease) = lease {
            self.lease.restore(lease, now_ms);
        }
        let n = restore_targets(&mut self.model, targets);
        if n > 0 {
            self.log(
                EventCode::TargetsRestored,
                NO_VALVE,
                i32::from(n),
                src as i32,
                b"",
            );
        }
        self.last_desired_rev = self.model.desired_revision();
        self.dirty = true;
    }

    /// Everything the STM knew about this session is gone. Every caller marks the snapshot
    /// dirty (a reply, a completion, a link state change or the flasher).
    fn new_stm_session(&mut self, now_ms: u32) {
        self.model.on_stm_rebooted(now_ms);
        self.planner.request_resync();
        self.lease.on_stm_reboot();
        self.learn.on_stm_reboot();
        self.have_prev_status = false;
        self.snap.proto = self.planner.protocol(); // unknown until gproto answers again
        self.snap.support = self.planner.support();
        self.start_sensor_grace(now_ms);
    }

    /// STM rebooted/was reset/re-flashed by us: forget link-level history too.
    fn resync(&mut self, now_ms: u32) {
        self.lines.reset();
        self.reboot.reset();
        self.new_stm_session(now_ms);
        for m in &mut self.moves {
            m.active = false;
        }
    }

    fn after_stm_reset(&mut self, now_ms: u32, by_policy: bool) {
        self.link.on_stm_reset(now_ms, by_policy);
        self.resync(now_ms);
    }

    fn on_reboot_detected(&mut self, now_ms: u32, cause: i32) {
        self.log(EventCode::StmRebootDetected, NO_VALVE, cause, 0, b"");
        self.new_stm_session(now_ms);
    }

    // ------------------------------------------------------------ commands

    fn calibrate(&mut self, c: &StmCommand, now_ms: u32) {
        if c.scheduled && self.flashing() {
            self.port
                .post_scheduled_calib_result(c.attempt, false, CalibFailure::NotSent);
            return;
        }
        let tag = if c.scheduled {
            Self::TAG_SCHEDULED_CALIB
        } else {
            0
        };
        let queued =
            build_calibrate(c.valve).is_some_and(|r| self.enqueue(&r, Priority::User, tag));
        if !queued {
            if c.scheduled {
                self.port
                    .post_scheduled_calib_result(c.attempt, false, CalibFailure::NotSent);
            }
            return;
        }
        if c.scheduled {
            self.sched_attempt = c.attempt;
            self.scheduled_mask = self.model.active_mask();
            self.scheduled_at_ms = now_ms;
        } else {
            self.scheduled_mask = 0;
        }
    }

    fn set_motor_settings(&mut self, c: &StmCommand) {
        // All or nothing on the link queue too: Poll entries make room, so only queued
        // User/Config requests count.
        let v2 = self.planner.protocol() >= 2;
        let need = usize::from(c.has_motor)
            + usize::from(c.has_learn_movements)
            + usize::from(c.has_breakaway && v2);
        let held = self.link.queued_with(Priority::User) + self.link.queued_with(Priority::Config);
        if held + need > LinkPolicy::QUEUE_CAPACITY {
            self.log(EventCode::StmQueueFull, NO_VALVE, Cmd::Smotc as i32, 0, b"");
            return;
        }
        let mut any = false;
        if c.has_motor {
            if let Some(r) = build_set_motor_chars(&c.motor) {
                any |= self.enqueue(&r, Priority::User, 0);
            }
        }
        if c.has_learn_movements {
            if let Some(r) = build_set_learn_movements(u32::from(c.learn_movements)) {
                any |= self.enqueue(&r, Priority::User, 0);
            }
        }
        // v2 only: a v1 STM would ignore it (the web answers 409 before).
        if c.has_breakaway && v2 {
            if let Some(r) = build_set_breakaway(&c.breakaway) {
                any |= self.enqueue(&r, Priority::User, 0);
            }
        }
        if any {
            self.planner.request_motor_params();
        }
    }

    pub fn handle_command(&mut self, c: &StmCommand, now_ms: u32) {
        let proto = self.planner.protocol();
        // An STM below the minimum version gets targets only; other commands would go
        // unanswered (the web answers 409 before).
        if self.too_old()
            && !matches!(
                c.kind,
                StmCommandType::SetTarget
                    | StmCommandType::ResetStm
                    | StmCommandType::StartFlash
                    | StmCommandType::AbortFlash
                    | StmCommandType::ConfigChanged
            )
        {
            if c.kind == StmCommandType::Calibrate && c.scheduled {
                self.port
                    .post_scheduled_calib_result(c.attempt, false, CalibFailure::Unsupported);
            }
            return;
        }
        match c.kind {
            StmCommandType::SetTarget => {
                if !self
                    .model
                    .set_desired_target(c.valve, c.pos, c.source, now_ms)
                {
                    let valve = if c.valve < VALVE_COUNT {
                        i32::from(c.valve) + 1
                    } else {
                        0
                    };
                    self.log(
                        EventCode::MqttCommandRejected,
                        NO_VALVE,
                        valve,
                        0,
                        b"rejected by model",
                    );
                }
                self.dirty = true;
            }
            StmCommandType::Calibrate => self.calibrate(c, now_ms),
            StmCommandType::Assembly => {
                // No stgtp afterwards: it would end the STM's assembly hold.
                if build_assembly(c.valve).is_some_and(|r| self.enqueue(&r, Priority::User, 0)) {
                    self.model.set_assembly(c.valve, now_ms);
                }
            }
            StmCommandType::Detect => {
                self.enqueue(&build_detect(), Priority::User, 0);
            }
            StmCommandType::ScanSensors => {
                if self.enqueue(&build_scan_one_wire(), Priority::User, 0) {
                    self.sensors.clear();
                    self.planner.request_temp_list();
                    self.planner.request_volt_list();
                    self.planner.request_valve_sensors();
                    self.start_sensor_grace(now_ms);
                }
            }
            StmCommandType::SetValveSensors => {
                let [s1, s2] = &c.ids;
                if build_set_valve_sensors(c.valve, s1, s2)
                    .is_some_and(|r| self.enqueue(&r, Priority::User, 0))
                {
                    self.enqueue(&build_match_sensors(), Priority::User, 0);
                    self.planner.request_valve_sensors();
                }
            }
            StmCommandType::SetMotorSettings => self.set_motor_settings(c),
            StmCommandType::ServiceMove => {
                if proto >= 2 {
                    if let Some(r) = build_service_move(c.valve, c.dir, c.counts, c.max_ma) {
                        self.enqueue(&r, Priority::User, 0);
                    }
                }
            }
            StmCommandType::RequestProfile => self.planner.request_profile(c.valve),
            StmCommandType::ResetStm => {
                if self.flashing() {
                    // The flasher owns NRST; a pulse now would corrupt the flash run.
                    self.log(EventCode::StmFlashFailed, NO_VALVE, 0, 0, b"reset refused");
                } else if self.gate_action == GateAction::None {
                    // (a reset or flash that waits already: nothing)
                    self.gate_answered = self.stm_answers(now_ms);
                    self.gate.begin(now_ms, self.gate_answered);
                    self.gate_action = GateAction::StmReset;
                }
            }
            StmCommandType::StartFlash => self.request_flash(c, now_ms),
            StmCommandType::AbortFlash => {
                if self.flashing() {
                    self.flasher.abort();
                } else if self.gate_action == GateAction::Flash {
                    self.gate.reset();
                    self.gate_action = GateAction::None;
                    self.flash_pending = false;
                    self.dirty = true;
                }
            }
            // the glue re-reads the config on every revision change
            StmCommandType::ConfigChanged => {}
            StmCommandType::StopValve => {
                if proto >= 3 {
                    if let Some(r) = build_stop(c.valve) {
                        self.enqueue(&r, Priority::User, 0);
                    }
                }
            }
            StmCommandType::LeaveSafeMode => {
                if proto >= 3 {
                    self.enqueue(&build_leave_safe_mode(), Priority::User, 0);
                }
            }
        }
    }

    /// Early checks at once; the flash itself waits for the STM EEPROM (normal mode) or starts
    /// at once (blank mode: the STM is in the ROM bootloader).
    fn request_flash(&mut self, c: &StmCommand, now_ms: u32) {
        if self.flashing() || self.gate_action != GateAction::None {
            self.log(EventCode::StmFlashFailed, NO_VALVE, 0, 0, b"busy");
            return;
        }
        // An ESP restart would cut the flash run (STM half-erased, in reset or in 8E1). A
        // restart is due at the earliest 1 s after it was requested, and the flag is raised
        // right after this check, so the restart either made us refuse or sees the flag.
        if self.port.restart_pending() {
            self.log(
                EventCode::StmFlashFailed,
                NO_VALVE,
                0,
                0,
                b"restart pending",
            );
            return;
        }
        self.port.mark_flash_active(); // not only with the next snapshot (<= 100 ms)
        self.flash_pending = true;
        self.dirty = true;
        if c.blank {
            self.begin_flash(c, now_ms);
            return;
        }
        self.pending_flash.clone_from(c);
        self.gate_answered = self.stm_answers(now_ms);
        self.gate.begin(now_ms, self.gate_answered);
        self.gate_action = GateAction::Flash;
    }

    fn begin_flash(&mut self, c: &StmCommand, now_ms: u32) {
        self.flash_pending = false;
        self.dirty = true; // a refused start below publishes "idle" again
        let name = c_str(&c.image);
        if !self.port.open_image(name) {
            self.log(
                EventCode::StmFlashFailed,
                NO_VALVE,
                FlashError::ImageRead as i32,
                0,
                name,
            );
            return;
        }
        let mut opt = FlashOptions {
            blank: c.blank,
            force: c.force,
            ..FlashOptions::default()
        };
        // The running STM's tag wins over the user's choice.
        if board_tag_valid(&self.snap.version.hw) {
            // parse_version: a valid tag or ""
            copy_string(&mut opt.board_hw, &self.snap.version.hw);
        } else if board_tag_valid(&c.board) {
            copy_string(&mut opt.board_hw, &c.board);
        }
        self.link.suspend();
        if !self.flasher.begin(&opt, now_ms) {
            self.port.close_image();
            self.link.resume(now_ms);
            self.log(EventCode::StmFlashFailed, NO_VALVE, 0, 0, b"start refused");
            return;
        }
        self.lines.reset();
        copy_string(&mut self.snap.flash_image, name);
        self.snap.flash.clone_from(self.flasher.status());
        let size = self.port.image().size();
        self.log(EventCode::StmFlashStarted, NO_VALVE, size as i32, 0, name);
    }

    fn service_gate(&mut self, now_ms: u32) {
        if self.gate_action == GateAction::None {
            return;
        }
        let st = self.gate.update(now_ms);
        if st == ResetGateState::Waiting {
            return;
        }
        let action = self.gate_action;
        let waited = self.gate.waited_ms(now_ms);
        self.gate_action = GateAction::None;
        self.gate.reset();
        if st == ResetGateState::TimedOut {
            self.log(
                EventCode::StmEepromWaitTimeout,
                NO_VALVE,
                waited as i32,
                action as i32,
                b"",
            );
        }
        match action {
            GateAction::StmReset => {
                let after = self.port.pulse_reset(now_ms);
                self.log(EventCode::StmResetByUser, NO_VALVE, 0, 0, b"");
                self.after_stm_reset(after, false);
            }
            GateAction::Flash => {
                let c = self.pending_flash.clone();
                self.begin_flash(&c, now_ms);
            }
            GateAction::EspRestart => {
                let s = if st == ResetGateState::TimedOut {
                    StmSaveState::TimedOut
                } else if self.gate_answered {
                    StmSaveState::Saved
                } else {
                    StmSaveState::Unavailable
                };
                self.port.set_stm_save_state(s);
            }
            GateAction::None => {}
        }
    }

    // ------------------------------------------------------------ replies

    /// The gvers reply in `self.reply`.
    fn on_version(&mut self) {
        let rep = &self.reply;
        self.snap.version.clone_from(&rep.version);
        self.snap.build = rep.build;
        self.snap.compatible = compare_version(&rep.version, &self.min_version) >= 0;
        self.planner.on_version(&rep.version);
        self.snap.support = self.planner.support();
        self.snap.proto = self.planner.protocol();
        // Unsupported: the STM data shown so far is not trustworthy.
        if self.snap.support == StmSupport::TooOld && self.prev_support != StmSupport::TooOld {
            self.model.forget_stm_data();
            // Targets are still delivered (stgtp/gtgtp exist on every 1.x): read them back.
            for v in valves_of(self.model.active_mask()) {
                self.planner.request_target(v);
            }
        }
        self.prev_support = self.snap.support;
        let mut buf = [0u8; 32];
        // a parsed gvers version always fits
        let n = format_version(&self.reply.version, &mut buf);
        let ver = buf.get(..n).unwrap_or_default();
        if ver != &self.logged_version[..] {
            copy_string(&mut self.logged_version, ver);
            self.incompatible_logged = false;
            self.log(
                EventCode::StmVersion,
                NO_VALVE,
                i32::from(self.planner.protocol()),
                i32::from(self.snap.hw_id),
                ver,
            );
        }
        if !self.snap.compatible && !self.incompatible_logged {
            self.incompatible_logged = true;
            self.log(EventCode::StmIncompatible, NO_VALVE, 0, 0, ver);
        }
    }

    fn on_status(&mut self, s: &StmStatus, now_ms: u32) {
        self.snap.status = *s;
        self.snap.have_status = true;
        // A reboot also clears the kept previous status.
        let cause = self.reboot.on_status(s, now_ms);
        if cause != 0 {
            self.on_reboot_detected(now_ms, i32::from(cause));
        }
        if !s.v3 {
            return;
        }
        let mut ev: [Event; MAX_EVENTS_PER_UPDATE] = Default::default();
        let prev = if self.have_prev_status {
            Some(&self.prev_status)
        } else {
            None
        };
        let n = self.health.on_stm_status(prev, s, now_ms, &mut ev);
        self.log_events(ev.get(..n).unwrap_or_default());
        self.lease.on_status(s, now_ms);
        self.sensors.set_stm_temp_age(s.temp_age_s, now_ms);
        self.prev_status = *s;
        self.have_prev_status = true;
    }

    /// Applies the reply data in `self.reply`. `req` is the matched request or None for a stray
    /// line (then only self-identifying replies are applied).
    fn apply_reply(&mut self, req: Option<&RequestLine>, now_ms: u32) {
        self.dirty = true;
        match self.reply.cmd {
            Cmd::Gvlvd => {
                self.model.apply_valve_data(&self.reply.valve_data, now_ms);
                if self.reboot.on_valve_data(&self.reply.valve_data) {
                    self.on_reboot_detected(now_ms, 3);
                }
            }
            Cmd::Gvlvx | Cmd::Gvlvy => self.model.apply_valve_ex(&self.reply.valve_ex, now_ms),
            Cmd::Gvlst => {
                if req.is_some() {
                    self.model
                        .apply_valve_states(&self.reply.valve_states, now_ms);
                }
            }
            Cmd::Gtgtp => self.model.apply_target(&self.reply.target, now_ms),
            Cmd::Gonec => {
                let l = &self.reply.one_wire_list;
                if req.is_some() && self.sensors.apply_temp_list(l, now_ms) && !l.has_list {
                    self.planner.request_temp_list();
                }
                self.planner
                    .set_sensor_counts(self.sensors.temp_count(), self.sensors.volt_count());
            }
            Cmd::Gowvc => {
                let l = &self.reply.one_wire_list;
                if req.is_some() && self.sensors.apply_volt_list(l, now_ms) && !l.has_list {
                    self.planner.request_volt_list();
                }
                self.planner
                    .set_sensor_counts(self.sensors.temp_count(), self.sensors.volt_count());
            }
            Cmd::Goned => {
                // The reply carries no bus index: the matched request names it; a late reading
                // lands on the index of its id.
                let d = &self.reply.temp_data;
                if let Some(req) = req {
                    self.sensors.apply_temp_data(req.arg as u8, d, now_ms);
                } else if !self.sensors.apply_stray_temp_data(d, now_ms) && d.valid {
                    self.planner.request_temp_list();
                }
            }
            Cmd::Gowvd => {
                let d = &self.reply.volt_data;
                if let Some(req) = req {
                    self.sensors.apply_volt_data(req.arg as u8, d, now_ms);
                } else if !self.sensors.apply_stray_volt_data(d, now_ms) && d.valid {
                    self.planner.request_volt_list();
                }
            }
            Cmd::Gvlon => {
                if req.is_some() && !self.reply.gvlon_error {
                    self.model
                        .apply_valve_sensors(&self.reply.valve_sensors, &self.slot_ids);
                }
            }
            Cmd::Gmotc => {
                let m = self.reply.motor_chars;
                self.snap.motor = m;
                self.snap.have_motor = true;
                self.model.set_min_counts(m.min_counts);
                self.health.set_min_counts(m.min_counts);
            }
            Cmd::Gtlnm => self.snap.learn_movements = self.reply.learn_movements,
            Cmd::Gcalx => {
                self.snap.breakaway = self.reply.breakaway;
                self.snap.have_breakaway = true;
            }
            Cmd::Gvers => self.on_version(),
            Cmd::Ghwin => self.snap.hw_id = self.reply.hw_id,
            Cmd::Gproto => {
                self.planner.set_protocol(self.reply.proto);
                self.snap.proto = self.planner.protocol();
            }
            Cmd::Gstat | Cmd::Gstax => {
                let s = self.reply.status;
                self.on_status(&s, now_ms);
            }
            Cmd::Gprof => {
                let p = &self.reply.profile;
                if let Some(seq) = self.snap.profile_seq.get_mut(usize::from(p.valve)) {
                    self.port.store_profile(p);
                    *seq = seq.wrapping_add(1);
                }
            }
            _ => {}
        }
    }

    /// A completed svmov. `has_reply`: `self.reply` is the reply that completed it.
    fn on_service_move_result(&mut self, c: &Completion, has_reply: bool, now_ms: u32) {
        let v = c.request.valve;
        let Some(m) = self.moves.get_mut(usize::from(v)) else {
            return;
        };
        if c.outcome == Outcome::Ok {
            m.active = true;
            m.move_seq = self.model.valve(v).move_seq;
            m.since_ms = now_ms;
            self.planner.request_target(v); // fast read-back of gvlvx
            return;
        }
        // Rejected ("svmov v err n", "svmov -1 err n": a reply) or no answer (none).
        let code = if has_reply {
            i32::from(self.reply.service_move.error_code)
        } else {
            -1
        };
        let text: &[u8] = if c.outcome == Outcome::Rejected {
            b"rejected"
        } else {
            b"no reply"
        };
        self.port.log_event(&make_event(
            EventCode::ServiceMoveDone,
            Severity::Warning,
            v,
            -1,
            code,
            text,
        ));
    }

    /// `has_reply`: `self.reply` is the reply that completed the request (C++ `rep` not null).
    fn on_completion(&mut self, c: &Completion, has_reply: bool, now_ms: u32) {
        let ok = c.outcome == Outcome::Ok;
        let req = &c.request;
        let rep = if has_reply { Some(&self.reply) } else { None };
        match req.cmd {
            Cmd::Stgtp => {
                if ok {
                    self.model.on_target_ack(req.valve, now_ms);
                } else {
                    self.model.on_target_timeout(req.valve, now_ms);
                }
            }
            Cmd::Svmov => self.on_service_move_result(c, has_reply, now_ms),
            Cmd::Staln => {
                if c.tag == Self::TAG_SCHEDULED_CALIB {
                    let reason = if ok {
                        CalibFailure::None
                    } else {
                        CalibFailure::NoReply
                    };
                    self.port
                        .post_scheduled_calib_result(self.sched_attempt, ok, reason);
                }
            }
            Cmd::Staop => {
                if ok {
                    self.model.on_assembly_ack(req.valve, now_ms);
                } else {
                    self.model.on_assembly_failed(req.valve, now_ms);
                }
            }
            Cmd::Stons => {
                // A legacy STM does not re-match the sensors after its search.
                if ok && self.planner.protocol() < 2 {
                    self.planner.request_match_sensors(now_ms);
                }
            }
            Cmd::Masns => {
                if ok {
                    self.planner.request_valve_sensors();
                }
            }
            Cmd::Slhbt | Cmd::Slcfg | Cmd::Sfspo | Cmd::Glcfg => {
                self.lease.on_completion(req, c.outcome, rep, now_ms);
            }
            Cmd::Gtlnt | Cmd::Stlnt => self.learn.on_completion(req, c.outcome, rep, now_ms),
            Cmd::Eepst => {
                let idle = rep.is_some_and(|r| r.eeprom_idle);
                self.gate
                    .on_eepst(c.outcome != Outcome::Timeout, idle, now_ms);
            }
            Cmd::Gstat | Cmd::Gstax => {
                if c.outcome == Outcome::Timeout && self.reboot.on_status_failed() {
                    self.on_reboot_detected(now_ms, 4);
                }
            }
            Cmd::Sstop if ok => {
                if req.valve == ALL_VALVES {
                    for v in valves_of(self.model.active_mask()) {
                        self.planner.request_target(v);
                    }
                } else {
                    self.planner.request_target(req.valve);
                }
            }
            _ => {}
        }
        self.planner.on_result(req, ok, now_ms);
        self.snap.proto = self.planner.protocol();
        self.dirty = true;
    }

    /// The reply parsed into `self.reply` with status `ps`.
    fn on_parsed(&mut self, ps: ParseStatus, now_ms: u32) {
        if ps != ParseStatus::Ok {
            self.link.on_parse_error(now_ms);
            return;
        }
        match self.link.on_reply(&self.reply, now_ms) {
            Some(done) => {
                self.apply_reply(Some(&done.request), now_ms);
                self.on_completion(&done, true, now_ms);
            }
            None => self.apply_reply(None, now_ms),
        }
    }

    /// One complete reply line.
    pub fn on_line(&mut self, line: &[u8], now_ms: u32) {
        let ps = parse_reply(line, &mut self.reply);
        self.on_parsed(ps, now_ms);
    }

    /// Bytes read from the UART (not while flashing).
    pub fn on_rx(&mut self, data: &[u8], now_ms: u32) {
        let mut rest = data;
        // Without a pending line feed() consumes at least one byte: this ends.
        while !rest.is_empty() {
            let used = self.lines.feed(rest);
            rest = rest.get(used..).unwrap_or_default();
            if self.lines.has_line() {
                // The line goes before it is handled (C++: after): handling a reply never
                // touches the assembler.
                let ps = parse_reply(self.lines.line(), &mut self.reply);
                self.lines.release();
                self.on_parsed(ps, now_ms);
            }
        }
    }

    // ------------------------------------------------------------ loop

    /// The lease, learn-time, assembly and target requests and the read-back of a delivered
    /// target, at most one per call (C++: the if/else chain of `scheduleRequests`).
    fn schedule_config(&mut self, now_ms: u32, unsupported: bool) {
        if !unsupported {
            if let Some(r) = self.lease.next(now_ms) {
                self.enqueue(&r, Priority::Config, 0);
                return;
            }
            if let Some(r) = self.learn.next(now_ms) {
                self.enqueue(&r, Priority::Config, 0);
                return;
            }
        }
        if let Some(valve) = self.model.next_assembly_push(now_ms) {
            if !build_assembly(valve).is_some_and(|r| self.enqueue(&r, Priority::User, 0)) {
                self.model.on_target_push_dropped(valve, now_ms);
            }
        } else if let Some((valve, pos)) = self.model.next_target_push(now_ms) {
            // A push that cannot be queued is retried after push_retry_ms; it is not logged
            // (enqueue()) because that would repeat every 2 s while the queue stays full, the
            // link counts it in queue_full.
            let res = build_set_target(valve, pos).map_or(EnqueueResult::Invalid, |r| {
                self.link.enqueue(&r, Priority::Config, 0)
            });
            if !matches!(res, EnqueueResult::Queued | EnqueueResult::Coalesced) {
                self.model.on_target_push_dropped(valve, now_ms);
            }
        } else if let Some(valve) = self.model.next_verify() {
            self.planner.request_target(valve);
        }
    }

    fn schedule_requests(&mut self, now_ms: u32) {
        if self.gate.state() == ResetGateState::Waiting {
            let held =
                self.link.queued_with(Priority::User) + self.link.queued_with(Priority::Config) > 0
                    || self.link.busy_with(Priority::User)
                    || self.link.busy_with(Priority::Config);
            if self.gate.poll_due(now_ms, held)
                && self.enqueue(&build_eeprom_state(), Priority::User, 0)
            {
                self.gate.on_poll_sent(now_ms);
            }
        }
        let proto = self.planner.protocol();
        let unsupported = self.too_old();
        // Unknown protocol (re-sync) is treated like 1.x.
        self.model.set_hold_targets_while_calibrating(proto < 2);
        self.model
            .set_assembly_via_staop(proto >= 2 && !unsupported);
        self.lease.set_protocol(proto, now_ms);
        self.learn.set_protocol(if unsupported { 0 } else { proto });
        // Nothing but gproto and gvers before the version is known.
        let step = self.planner.resync_step();
        let version_known = step != ResyncStep::Proto && step != ResyncStep::Version;
        if version_known && self.link.queued_with(Priority::Config) == 0 {
            self.schedule_config(now_ms, unsupported);
        }
        for i in 0..VALVE_COUNT {
            self.planner.set_valve_busy(i, self.model.is_busy(i));
        }
        if self.link.queued_with(Priority::Poll) == 0 {
            if let Some(mut r) = self.planner.next(now_ms) {
                expect_sensor(&mut r, &self.sensors);
                let p = if self.planner.last_was_resync() {
                    Priority::Config
                } else {
                    Priority::Poll
                };
                self.enqueue(&r, p, 0);
            }
        }
    }

    /// Timeouts, the policy reset, the reset gate and request scheduling.
    pub fn poll(&mut self, now_ms: u32) {
        if let Some(done) = self.link.poll(now_ms) {
            self.on_completion(&done, false, now_ms);
        }
        if self.link.should_reset_stm(now_ms) {
            // No EEPROM wait: the STM has not answered for a minute.
            let st = *self.link.stats();
            self.log(
                EventCode::StmResetByPolicy,
                NO_VALVE,
                i32::from(st.consecutive_timeouts),
                (elapsed_ms(now_ms, st.last_reply_ms) / 1000) as i32,
                b"",
            );
            let after = self.port.pulse_reset(now_ms);
            self.after_stm_reset(after, true);
        }
        self.service_gate(now_ms);
        if !self.flashing() {
            self.schedule_requests(now_ms);
        }
    }

    /// The next request line for the UART; [`on_sent`](Self::on_sent) right after writing it.
    pub fn next_to_send(&mut self, now_ms: u32) -> Option<&RequestLine> {
        self.link.next_to_send(now_ms)
    }

    pub fn on_sent(&mut self, now_ms: u32) {
        self.link.on_sent(now_ms);
    }

    pub fn flashing(&self) -> bool {
        self.flasher.active()
    }

    /// One flasher step while [`flashing`](Self::flashing) (instead of on_rx/poll/next_to_send),
    /// over the transport of the run.
    pub fn flash_step(&mut self, transport: &mut dyn FlashTransport, now_ms: u32) {
        let phase = self.flasher.step(transport, self.port.image(), now_ms);
        self.snap.flash.clone_from(self.flasher.status());
        self.dirty = true;
        if phase != FlashPhase::Done && phase != FlashPhase::Failed {
            return;
        }
        self.port.close_image();
        transport.configure(Self::STM_BAUD, false); // the flasher restored 8N1 already; make sure
        let st = self.flasher.status();
        if phase == FlashPhase::Done {
            let mut buf = [0u8; 32]; // format_version() always fits
            let n = format_version(&st.app_version, &mut buf);
            let ms = elapsed_ms(st.finished_ms, st.started_ms) as i32;
            self.log(
                EventCode::StmFlashDone,
                NO_VALVE,
                ms,
                0,
                buf.get(..n).unwrap_or_default(),
            );
            self.port.request_last_good_copy(&self.snap.flash_image);
        } else {
            let (error, address, phase) = (st.error, st.error_address, st.error_phase);
            self.log(
                EventCode::StmFlashFailed,
                NO_VALVE,
                error as i32,
                address as i32,
                flash_phase_name(phase).as_bytes(),
            );
        }
        // The flasher reset the STM: 5 s hold-off, full re-sync, targets re-pushed.
        self.link.resume(now_ms);
        self.resync(now_ms);
    }

    /// Sensor slot failure/recovery edges (once per second).
    fn check_sensors(&mut self, now_ms: u32) {
        if self.sensor_grace {
            if elapsed_ms(now_ms, self.sensor_grace_from_ms) < Self::SENSOR_GRACE_MS {
                return;
            }
            self.sensor_grace = false;
        }
        let mut ev: [Event; MAX_EVENTS_PER_UPDATE] = Default::default();
        let temps = self.temp_track.iter_mut().zip(&self.slot_ids);
        for ((t, id), (&active, slot)) in temps.zip(self.temp_active.iter().zip(1u8..)) {
            if !active || is_zero(id) {
                *t = SlotTrack::default();
                continue;
            }
            let bus = self.sensors.find_temp(id);
            let rd = self.sensors.temp(bus.unwrap_or(0xFF));
            let valid = bus.is_some_and(|b| {
                temp_raw_valid(rd.raw) && self.sensors.temp_fresh(b, now_ms, Self::SENSOR_STALE_MS)
            });
            let n = self
                .health
                .on_temp_sensor(slot, t.known, t.valid, valid, rd.raw, &mut ev);
            let ev = ev.get_mut(..n).unwrap_or_default();
            // At most one event: the failure carries the sensor id.
            if let [e] = ev {
                if e.code == EventCode::TempSensorFailed {
                    let mut text = [0u8; EVENT_TEXT_MAX + 1];
                    let len = format_one_wire_id(id, &mut text);
                    copy_string(&mut e.text, text.get(..len).unwrap_or_default());
                }
            }
            for e in ev.iter() {
                self.port.log_event(e);
            }
            t.known = true;
            t.valid = valid;
        }
        let volts = self.volt_track.iter_mut().zip(&self.volt_ids);
        for ((t, id), (&active, slot)) in volts.zip(self.volt_active.iter().zip(1u8..)) {
            if !active || is_zero(id) {
                *t = SlotTrack::default();
                continue;
            }
            let bus = self.sensors.find_volt(id);
            let rd = self.sensors.volt(bus.unwrap_or(0xFF));
            let valid = bus.is_some()
                && rd.seen
                && vad_valid(rd.vad)
                && elapsed_ms(now_ms, rd.last_seen_ms) <= Self::SENSOR_STALE_MS;
            if t.known && t.valid && !valid {
                self.port.log_event(&event(
                    EventCode::VoltSensorFailed,
                    NO_VALVE,
                    i32::from(slot),
                    rd.vad,
                    b"",
                ));
            }
            t.known = true;
            t.valid = valid;
        }
        let tc = self.sensors.temp_count();
        let vc = self.sensors.volt_count();
        if self.counts_known && tc != self.last_temp_count {
            self.log(
                EventCode::SensorCountChanged,
                NO_VALVE,
                i32::from(tc),
                0,
                b"",
            );
        }
        if self.counts_known && vc != self.last_volt_count {
            self.log(
                EventCode::SensorCountChanged,
                NO_VALVE,
                i32::from(vc),
                1,
                b"",
            );
        }
        self.counts_known = true;
        self.last_temp_count = tc;
        self.last_volt_count = vc;
    }

    /// Once per second: regulator, lease, emulation, counters, sensors, service moves, the STM
    /// save before an ESP restart.
    pub fn every_second(&mut self, now_ms: u32, regulator: &RegulatorInput, save: StmSaveState) {
        let mut ev: [Event; MAX_EVENTS_PER_UPDATE] = Default::default();
        // K1: regulator, lease events, the ESP emulation.
        self.lease
            .set_regulator(regulator_cause(regulator), regulator.command_seq, now_ms);
        let n = self.lease.tick(now_ms, &mut ev);
        self.log_events(ev.get(..n).unwrap_or_default());
        self.model.set_failsafe_drive(
            self.lease.emulated_mask(now_ms),
            &self.lease.config().failsafe_pct,
        );
        let record = self.lease.snapshot(now_ms);
        self.port.store_lease_record(&record);
        let ls = self.lease.status(now_ms);
        if lease_key(&ls) != lease_key(&self.last_lease) {
            self.dirty = true;
        }
        self.last_lease = ls;
        // E4: the STM save before an ESP restart.
        if save == StmSaveState::Waiting && self.gate_action == GateAction::None {
            self.gate_answered = self.stm_answers(now_ms) && !self.flashing();
            self.gate.begin(now_ms, self.gate_answered);
            self.gate_action = GateAction::EspRestart;
            self.service_gate(now_ms);
        }
        // An unsupported STM sends no valve data: no stale flags.
        if !self.too_old() {
            self.model.tick(now_ms);
        }
        let st = *self.link.stats();
        let n = self.health.on_stm_counters(
            self.lines.overflow_count(),
            st.parse_errors.wrapping_add(self.lines.malformed_count()),
            0,
            now_ms,
            &mut ev,
        );
        self.log_events(ev.get(..n).unwrap_or_default());
        if self.snap.have_status {
            let s = &self.snap.status;
            let n = self
                .health
                .on_stm_counters(s.rx_overflow, s.parse_errors, 1, now_ms, &mut ev);
            self.log_events(ev.get(..n).unwrap_or_default());
        }
        if !self.flashing() {
            self.check_sensors(now_ms);
        }
        let settled = self.link.state(now_ms) == LinkState::Up
            && !self.planner.resync_active()
            && !self.sensor_grace;
        // v2+ has no gvlvd: valve temperatures from gvlon + goned (v1: from gvlvd).
        if self.planner.protocol() >= 2 {
            self.model
                .apply_sensor_temps(&self.sensors, now_ms, Self::SENSOR_STALE_MS, settled);
        }
        if settled != self.snap.sensors_settled {
            self.snap.sensors_settled = settled;
            self.dirty = true;
        }
        for (v, m) in (0u8..).zip(self.moves.iter_mut()) {
            if !m.active {
                continue;
            }
            let s = self.model.valve(v);
            if s.move_seq != m.move_seq {
                m.active = false;
                self.port.log_event(&event(
                    EventCode::ServiceMoveDone,
                    v,
                    s.last_move.counted_counts as i32,
                    s.last_move.stop as i32,
                    b"",
                ));
            } else if elapsed_ms(now_ms, m.since_ms) >= Self::SERVICE_MOVE_WAIT_MS {
                m.active = false;
            }
        }
        if self.scheduled_mask != 0
            && elapsed_ms(now_ms, self.scheduled_at_ms) >= Self::SCHEDULED_CALIB_WINDOW_MS
        {
            self.scheduled_mask = 0;
        }
    }

    /// Valve/link events, desired-target hand-over, snapshot (at most every PUBLISH_MIN_MS,
    /// only after a change).
    pub fn publish_if_due(&mut self, now_ms: u32) {
        let mut ev: [Event; MAX_EVENTS_PER_UPDATE] = Default::default();
        for (i, prev) in (0u8..).zip(self.prev.iter_mut()) {
            let cur = self.model.valve(i);
            if cur.revision == prev.revision {
                continue;
            }
            let active = (self.model.active_mask() >> i) & 1 != 0;
            let n = self.health.on_valve(i, prev, cur, active, &mut ev);
            for e in ev.iter_mut().take(n) {
                if e.code == EventCode::CalibStarted && (self.scheduled_mask >> i) & 1 != 0 {
                    e.arg1 = 1;
                    self.scheduled_mask &= !(1 << i);
                }
                self.port.log_event(e);
            }
            *prev = *cur;
            self.dirty = true;
        }
        let ls = self.link.state(now_ms);
        if ls != self.prev_link {
            let n = self.health.on_link(
                self.prev_link,
                ls,
                self.link.stats().consecutive_timeouts,
                &mut ev,
            );
            self.log_events(ev.get(..n).unwrap_or_default());
            // A link recovery is a reboot only when the STM status says so (2/3).
            let r = self
                .reboot
                .on_link_state(self.prev_link, ls, self.planner.protocol());
            if r == RebootDetectorRecovery::Reboot {
                self.on_reboot_detected(now_ms, 4);
            }
            if r == RebootDetectorRecovery::CheckStatus {
                self.planner.request_status();
            }
            self.prev_link = ls;
            self.dirty = true;
        }
        if self.model.desired_revision() != self.last_desired_rev {
            self.last_desired_rev = self.model.desired_revision();
            let mut t = PersistedTargets::default();
            capture_targets(&self.model, &mut t);
            self.port.store_desired_targets(&t);
        }
        if !self.dirty
            || (self.published_once
                && elapsed_ms(now_ms, self.last_publish_ms) < Self::PUBLISH_MIN_MS)
        {
            return;
        }
        for (i, v) in (0u8..).zip(self.snap.valves.iter_mut()) {
            *v = *self.model.valve(i);
        }
        self.snap.temp_count = self.sensors.temp_count();
        for (i, t) in (0u8..).zip(self.snap.temps.iter_mut()) {
            *t = *self.sensors.temp(i);
        }
        self.snap.volt_count = self.sensors.volt_count();
        for (i, t) in (0u8..).zip(self.snap.volts.iter_mut()) {
            *t = *self.sensors.volt(i);
        }
        self.snap.link = ls;
        self.snap.link_stats = *self.link.stats();
        self.snap.line_overflows = self.lines.overflow_count();
        self.snap.line_malformed = self.lines.malformed_count();
        self.snap.support = self.planner.support();
        self.snap.lease = self.lease.status(now_ms);
        self.snap.have_learn_time = self.learn.have_stm_value();
        self.snap.learn_time_s = self.learn.stm_value();
        self.snap.flash_pending = self.flash_pending;
        self.snap.taken_ms = now_ms;
        self.snap.revision = self.snap.revision.wrapping_add(1);
        self.port.publish(&self.snap);
        self.last_publish_ms = now_ms;
        self.published_once = true;
        self.dirty = false;
    }

    // ------------------------------------------------------------ for tests (and the glue)

    pub fn model(&self) -> &ValveModel {
        &self.model
    }

    pub fn sensors(&self) -> &SensorModel {
        &self.sensors
    }

    pub fn link(&self) -> &LinkPolicy {
        &self.link
    }

    pub fn planner(&self) -> &PollPlanner {
        &self.planner
    }

    pub fn lease(&self) -> &LeaseClient {
        &self.lease
    }

    pub fn snapshot(&self) -> &StmSnapshot {
        &self.snap
    }

    pub fn flash_pending(&self) -> bool {
        self.flash_pending
    }

    pub fn port(&self) -> &P {
        &self.port
    }

    pub fn port_mut(&mut self) -> &mut P {
        &mut self.port
    }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_mut;
