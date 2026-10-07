//! The valve application (port of `software_stm32/src/app.cpp` and `include/app.h`): start
//! values and configuration of the valves, the decisions of the main loop over the valve state
//! machine (`app_loop`: presence tests, moves and calibrations through the valve scheduler), the
//! 1 s and 10 s countdowns (lease, learn time, movement trigger, automatic retries), the protocol
//! 3 state (lease, failsafe positions, stop, protection guard, temperature hold) and the warm
//! state that survives a warm reset in the no-init RAM.
//!
//! Every function that reads or writes valve data takes the motor state ([`MotorShared`]) that
//! the main loop holds under the lock of the firmware's IsrCell (design §2.3: the `app_loop` pass
//! runs in one lock while no valve moves). The calls into the other modules go through
//! [`AppEnv`].

use vdm_stm_core::calibration::{sanitize_escalation, EscalationConfig};
use vdm_stm_core::config_blocks::{CalibRecord, CALIB_FAILED, CALIB_VALID};
use vdm_stm_core::config_store::{
    ConfigImage, CFG_READ_FAILED, CFG_SAFETY_CORRUPT, LEASE_SOURCE_DEFAULT,
};
use vdm_stm_core::failsafe::{
    drive_target, sanitize_failsafe_pct, Drive, DriveSource, FAILSAFE_HOLD,
};
use vdm_stm_core::fault_retry::{FaultRetry, Snapshot as RetrySnapshot};
use vdm_stm_core::lease::{
    sanitize_lease_timeout, Lease, LeaseState, Snapshot as LeaseSnapshot, LEASE_TIMEOUT_DEFAULT_MIN,
};
use vdm_stm_core::legacy_layout::{SensorSlot, VALVE_COUNT};
use vdm_stm_core::motor_params::{sanitize_motor_params, MotorParams};
use vdm_stm_core::move_classifier::StopReason;
use vdm_stm_core::protection_guard::ProtectionGuard;
use vdm_stm_core::settings::{
    countdown, effective_learn_time, sanitize_learn_movements, LEARN_MOVEMENTS_DEFAULT,
    LEARN_TIME_CLIENT_WINDOW_S, LEARN_TIME_DEFAULT_S,
};
use vdm_stm_core::system_stats::BootReason;
use vdm_stm_core::target_rejection::{reject_target, NO_REJECTED_TARGET};
use vdm_stm_core::temp_refresh::TempRefresh;
use vdm_stm_core::valve_codes::{
    ValveFault, ST_BLOCKED, ST_FAILED, ST_FULL_OPEN, ST_IDLE, ST_OPEN_CIRCUIT, ST_PRESENT,
    ST_UNKNOWN, VLV_FLAG_ASSEMBLY, VLV_FLAG_CAL_RESTORED, VLV_FLAG_EARLY_PENDING,
    VLV_FLAG_FS_BLOCKED, VLV_FLAG_FS_LEASE, VLV_FLAG_NEEDS_REF, VLV_FLAG_RECAL, VLV_FLAG_RETRY,
    VLV_FLAG_SVC_HOLD, VLV_FLAG_UNCALIBRATED,
};
use vdm_stm_core::valve_scheduler::{
    ActionKind, Decision, SchedulerInputs, ValveScheduler, ValveView,
};
use vdm_stm_core::warm_state::{
    is_warm_boot, restore_valve, warm_state_seal, warm_state_valid, WarmState, WarmValve,
    WARM_ASSEMBLY_HOLD, WARM_NEEDS_REFERENCE, WARM_POS_VALID, WARM_RECAL,
};

use core::sync::atomic::Ordering;

use vdm_stm_core::buf_writer::BufWriter;

use crate::hal::{Clock, NoinitStore, System, NOINIT_SIZE};
use crate::motor::{
    CalibState, IsrFlags, MotorShared, ValveSnapshot, CMD_A_CLOSE, CMD_A_CLOSE_END, CMD_A_LEARN,
    CMD_A_OPEN, CMD_A_OPEN_END, CMD_A_TEST, MOVE_KEEP_STATUS, MOVE_REFERENCE,
};

const VALVES: usize = VALVE_COUNT as usize;

/// after x movements a learning cycle is executed
pub const LEARN_AFTER_MOVEMENTS_DEFAULT: u16 = LEARN_MOVEMENTS_DEFAULT;
/// after x seconds a learning cycle is executed
pub const LEARN_AFTER_TIME_DEFAULT: u32 = LEARN_TIME_DEFAULT_S;
pub const NO_OF_MIN_COUNTS: u16 = 3000;
/// marks that no sensor slot is selected
pub const VALVE_SENSOR_UNKNOWN: u32 = 65535;
/// The sensors of the two slots of each valve as the match found them: the index in
/// tempsensors[], VALVE_SENSOR_UNKNOWN without one.
pub type SensorMatch = [(u32, u32); VALVES];
/// rejected_target: no target known
pub const VALVE_NO_TARGET: u8 = NO_REJECTED_TARGET;
/// app_10s_loop calls (~11 s) a service moved valve is left alone
pub const SVMOV_HOLD_10S: u8 = 30;
/// app_10s_loop calls a handed over calibration may take to start
pub const CALIB_START_TICKS: u8 = 2;
/// retest_request: presence test (automatic retry after a short)
pub const RETEST_TEST: u8 = 1;
/// retest_request: stdet, a valve found present calibrates fully
pub const RETEST_DETECT: u8 = 2;

/// max count of usable 1-Wire sensors (hardware.h MAXONEWIRECNT: 2 per valve + 10)
const MAXONEWIRECNT: u8 = 34;
/// the warm state lies at the start of the no-init RAM; with its trailing padding it fills 180
/// bytes (C++ sizeof(vdm::WarmState)), the CRC ends at byte 178
const WARM_BYTES: usize = 178;
const WARM_IMAGE: usize = 180;
/// block B damaged, or the EEPROM could not be read: the failsafe positions of the last run
const CFG_FAILSAFE_LOST: u8 = CFG_SAFETY_CORRUPT + CFG_READ_FAILED;

/// One valve (C++ `struct valve`, `myvalves[]`): sensor slots, learn countdowns and the requests
/// the main loop applies while no valve moves. The valve state machine writes `learn_movements`,
/// `movements` and `svc_hold`, so the array lives in [`MotorShared`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Valve {
    pub sensorindex1: u32,
    pub sensorindex2: u32,
    pub learn_time: u32,
    pub learn_movements: u32,
    pub movements: u32,
    pub statusm: u8,
    /// target changes not executed because the valve is FAILED or BLOCKS
    pub cmd_rejected: u16,
    /// last target that is not a new request (target_rejection::reject_target), VALVE_NO_TARGET
    /// if none
    pub rejected_target: u8,
    /// staln: learn without waiting for a target change
    pub forced_learn: bool,
    /// time trigger: learn at the next target change (after firstchange)
    pub timed_learn: bool,
    /// after svmov the position is left alone (app_10s_loop calls); set by appsetservice,
    /// cleared if the start is refused
    pub svc_hold: u8,
    /// RETEST_*: test the valve again (applied by app_loop while no valve moves)
    pub retest_request: u8,
    /// staop: open fully (applied by app_loop while no valve moves)
    pub open_request: bool,
    /// staop: the lease failsafe does not apply until the next stgtp
    pub assembly_hold: bool,
    /// automatic retry of a failed or blocked valve: calibrate without waiting for a target
    /// change
    pub retry_learn: bool,
    /// second early partial stop in a row: calibrate
    pub early_learn: bool,
    /// counts restored from the EEPROM, no calibration since start-up
    pub calib_restored: bool,
    /// mots[].calib_seq of the calibration record last handed to the EEPROM
    pub stored_seq: u8,
    /// stgtp since the last reference move or calibration
    pub touched: bool,
}

/// gvlvy values 20..25 (C++ `struct valve_v3_info`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ValveV3Info {
    pub flags: u16,
    pub fault: u8,
    pub fs_pct: u8,
    pub drive: u8,
    pub retry_s: u32,
    pub retries: u8,
}

/// Calls of app.cpp into the other glue modules, and the hardware it uses: the clock (temperature
/// hold), the no-init RAM (warm state) and the system reset (`reset`). The motor functions get
/// the motor state the caller holds; the stubs of the tests log the calls as the C++ link-seam
/// stubs do.
pub trait AppEnv: Clock + NoinitStore + System {
    // motor.cpp
    fn valve_idle(&mut self, m: &MotorShared) -> bool;
    /// C++ defaults: force false, flags 0
    fn appsetaction(
        &mut self,
        m: &mut MotorShared,
        cmd: u8,
        valve: u32,
        pos: u8,
        force: bool,
        flags: u8,
    ) -> i16;
    fn appsetservice(
        &mut self,
        m: &mut MotorShared,
        valve: u32,
        dir: u8,
        counts: u16,
        maxma: u8,
    ) -> i16;
    fn appstop(&mut self, m: &mut MotorShared, valve: u32) -> i16;
    fn valve_busy_index(&mut self, m: &MotorShared) -> i32;
    fn valve_get_snapshot(&mut self, m: &MotorShared, valve: u32, out: &mut ValveSnapshot);
    fn motor_set_params(&mut self, m: &mut MotorShared, params: &MotorParams);
    fn motor_set_escalation(&mut self, m: &mut MotorShared, config: &EscalationConfig);
    // eeprom.cpp
    /// `eep_content`: the RAM mirror of the EEPROM
    fn eep_content(&mut self) -> &mut ConfigImage;
    fn eeprom_lease_source(&mut self) -> u8;
    fn eeprom_cfg_flags(&mut self) -> u8;
    fn eeprom_store_calib(&mut self, valve: u8, rec: &CalibRecord);
    fn eeprom_free(&mut self) -> bool;
    // sysstat.cpp
    fn sysstat_safe_mode(&mut self) -> bool;
    fn sysstat_uptime_s(&mut self) -> u32;
    fn sysstat_boot_reason(&mut self) -> BootReason;
    // owDevices.cpp
    /// `noOfDS18Devices`
    fn ds18_count(&mut self) -> u8;
    /// `tempsensors[index].address`
    fn ds18_address(&mut self, index: u8) -> [u8; 8];
    /// `printAddress()` (a debug line of appDebug)
    fn print_address(&mut self, address: &[u8; 8]);
    // terminal.cpp
    fn terminal_manual_active(&mut self) -> bool;
    /// `COMM_DBG` (USART6): the debug lines of appDebug, CR LF included
    fn debug(&mut self, text: &[u8]);
}

/// One debug line: the parts, then CR LF (Arduino `println`).
struct DebugLine(BufWriter<[u8; 64]>);

impl DebugLine {
    fn new(text: &[u8]) -> Self {
        let mut w = BufWriter::<[u8; 64]>::default();
        w.append(text);
        DebugLine(w)
    }

    fn num(mut self, v: u32) -> Self {
        self.0.append_unsigned(v);
        self
    }

    fn text(mut self, text: &[u8]) -> Self {
        self.0.append(text);
        self
    }

    fn send(mut self, env: &mut impl AppEnv) {
        self.0.append(b"\r\n");
        env.debug(self.0.as_bytes());
    }
}

/// `(vdm::StopReason) stopReason` of a move record; the values the firmware writes are 0..7.
fn stop_reason(raw: u8) -> StopReason {
    const REASONS: [StopReason; 8] = [
        StopReason::None,
        StopReason::Target,
        StopReason::EndStop,
        StopReason::EarlyEndStop,
        StopReason::Timeout,
        StopReason::Undercurrent,
        StopReason::SafetyOvercurrent,
        StopReason::Aborted,
    ];
    REASONS
        .get(usize::from(raw))
        .copied()
        .unwrap_or(StopReason::None)
}

/// true if the 1-Wire address matches the EEPROM sensor slot (rom code and crc)
fn app_sensor_matches(slot: &SensorSlot, address: &[u8; 8]) -> bool {
    slot.crc == address[7] && slot.romcode[..] == address[1..7]
}

fn app_faulted(m: &MotorShared, valve: usize) -> bool {
    let status = m.mots[valve].status;
    status == ST_FAILED || status == ST_BLOCKED
}

/// The warm state as it lies in the RAM of the STM32 (the C++ layout, little endian).
fn warm_to_bytes(s: &WarmState) -> [u8; WARM_BYTES] {
    let mut b = [0u8; WARM_BYTES];
    b[0..4].copy_from_slice(&s.magic.to_le_bytes());
    b[4] = s.version;
    b[5] = s.count;
    b[6..8].copy_from_slice(&s.reserved.to_le_bytes());
    for (chunk, w) in b[8..152].as_chunks_mut::<12>().0.iter_mut().zip(&s.valves) {
        chunk[0] = w.actual;
        chunk[1] = w.target;
        chunk[2] = w.status;
        chunk[3] = w.flags;
        chunk[4] = w.retry_attempts;
        chunk[5] = w.retry_scheduled;
        chunk[6..8].copy_from_slice(&w.pad);
        chunk[8..12].copy_from_slice(&w.retry_remaining_s.to_le_bytes());
    }
    b[152..156].copy_from_slice(&s.lease_since_renewal_s.to_le_bytes());
    b[156..160].copy_from_slice(&s.lease_since_client_s.to_le_bytes());
    b[160] = s.lease_client;
    b[161] = s.pad;
    b[162..164].copy_from_slice(&s.lease_timeout_min.to_le_bytes());
    b[164..176].copy_from_slice(&s.failsafe_pct);
    b[176..178].copy_from_slice(&s.crc.to_le_bytes());
    b
}

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn warm_from_bytes(b: &[u8]) -> WarmState {
    let mut s = WarmState {
        magic: u32_at(b, 0),
        version: b[4],
        count: b[5],
        reserved: u16_at(b, 6),
        lease_since_renewal_s: u32_at(b, 152),
        lease_since_client_s: u32_at(b, 156),
        lease_client: b[160],
        pad: b[161],
        lease_timeout_min: u16_at(b, 162),
        crc: u16_at(b, 176),
        ..WarmState::default()
    };
    for (w, chunk) in s.valves.iter_mut().zip(b[8..152].as_chunks::<12>().0) {
        *w = WarmValve {
            actual: chunk[0],
            target: chunk[1],
            status: chunk[2],
            flags: chunk[3],
            retry_attempts: chunk[4],
            retry_scheduled: chunk[5],
            pad: [chunk[6], chunk[7]],
            retry_remaining_s: u32_at(chunk, 8),
        };
    }
    s.failsafe_pct.copy_from_slice(&b[164..176]);
    s
}

/// The state of app.cpp that only the main loop uses (`myvalves[]` and `learning_movements` are
/// in [`MotorShared`], the warm state in the no-init RAM).
pub struct App {
    /// stored learn time (stlnt, gtlnt)
    learning_time: u32,
    /// the countdowns run with it (settings::effective_learn_time)
    learning_time_active: u32,
    reset_request: u32,
    lease: Lease,
    retry: [FaultRetry; VALVES],
    sched: ValveScheduler,
    temp: TempRefresh,
    guard: ProtectionGuard,
    /// active failsafe positions (sfspo, EEPROM block B)
    failsafe: [u8; VALVES],
    /// mots[].move_seq handed to the scheduler
    move_seen: [u8; VALVES],
    /// mots[].trip_seq handed to the protection guard
    trip_seen: [u8; VALVES],
    /// lease timeout and failsafe positions of the run before a warm reset (app_restore): they
    /// stand in for what the EEPROM cannot supply, also at a later re-read of the EEPROM
    /// (app_load_config); a failsafe position set since (sfspo) replaces its copy
    warm_copies: bool,
    warm_lease: u16,
    warm_failsafe: [u8; VALVES],
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    /// The start values of the C++ statics.
    pub fn new() -> Self {
        App {
            learning_time: LEARN_AFTER_TIME_DEFAULT,
            learning_time_active: LEARN_AFTER_TIME_DEFAULT,
            reset_request: 0,
            lease: Lease::default(),
            retry: [FaultRetry::default(); VALVES],
            sched: ValveScheduler::default(),
            temp: TempRefresh::default(),
            guard: ProtectionGuard::default(),
            failsafe: [0; VALVES],
            move_seen: [0; VALVES],
            trip_seen: [0; VALVES],
            warm_copies: false,
            warm_lease: 0,
            warm_failsafe: [0; VALVES],
        }
    }

    fn app_drive(&self, m: &MotorShared, valve: usize) -> Drive {
        drive_target(
            m.mots[valve].target_position,
            self.failsafe[valve],
            m.mots[valve].status,
            self.lease.state() == LeaseState::Expired,
            m.valves[valve].assembly_hold,
        )
    }

    /// Hands a calibration to the valve state machine.
    fn app_start_learn(&mut self, m: &mut MotorShared, env: &mut impl AppEnv, valve: usize) {
        if env.appsetaction(m, CMD_A_LEARN, valve as u32, 0, false, 0) != 0 {
            return;
        }
        m.mots[valve].calib_state = CalibState::InProgress;
        m.mots[valve].calib_time = CALIB_START_TICKS;
        let v = &mut m.valves[valve];
        v.forced_learn = false;
        v.timed_learn = false;
        v.retry_learn = false;
        v.early_learn = false;
        v.svc_hold = 0;
        v.touched = false;
        // the target the calibration positions to; a later different one is new
        v.rejected_target = m.mots[valve].target_position;
    }

    /// A failed or blocked valve is not driven to its target: its position stays (a blocked
    /// valve goes to its failsafe position) and each new target is reported as rejected; a
    /// calibration (staln, the automatic retry) clears the fault. Returns true for a target
    /// change that was counted.
    fn app_reject_target(m: &mut MotorShared, valve: usize) -> bool {
        let target = m.mots[valve].target_position;
        let actual = m.mots[valve].actual_position;
        let v = &mut m.valves[valve];
        reject_target(&mut v.rejected_target, &mut v.cmd_rejected, target, actual)
    }

    /// A calibration of the valve is requested and has not ended yet (staln, time or movement
    /// trigger, automatic retry, early stops, a found valve, a valve without valid counts);
    /// status and calibration as read by the caller.
    pub fn app_learn_pending(
        &self,
        m: &MotorShared,
        valve: u16,
        status: u8,
        calibration: bool,
    ) -> bool {
        let Some(mot) = m.mots.get(usize::from(valve)) else {
            return false;
        };
        let v = &m.valves[usize::from(valve)];
        let pending_cal = !mot.calibrated || mot.recal;
        calibration
            || v.forced_learn
            || v.timed_learn
            || v.retry_learn
            || v.early_learn
            || status == ST_PRESENT
            || ((status == ST_IDLE || status == ST_FULL_OPEN) && pending_cal)
    }

    /// A target request (stgtp) ends the hold of a service move and the assembly hold, also when
    /// the target did not change, and makes a valve without a referenced position or calibration
    /// due.
    pub fn app_target_changed(&self, m: &mut MotorShared, valve: u16) {
        if let Some(v) = m.valves.get_mut(usize::from(valve)) {
            v.svc_hold = 0;
            v.assembly_hold = false;
            v.touched = true;
        }
    }

    /// svmov: 0 accepted, -1 invalid arguments, -2 valve state machine busy or safe mode, -3
    /// calibration pending (the calibration would start right after the move and undo it).
    pub fn app_service_move(
        &self,
        m: &mut MotorShared,
        env: &mut impl AppEnv,
        valve: u16,
        dir: u8,
        counts: u16,
        maxma: u8,
    ) -> i16 {
        let Some(mot) = m.mots.get(usize::from(valve)) else {
            return -1;
        };
        let (status, calibration) = (mot.status, mot.calibration);
        if env.sysstat_safe_mode() {
            return -2;
        }
        if self.app_learn_pending(m, valve, status, calibration) {
            return -3;
        }
        // an accepted move also starts the hold (svc_hold), a start the valve state machine
        // refuses ends it
        env.appsetservice(m, u32::from(valve), dir, counts, maxma)
    }

    pub fn app_setup(&mut self, m: &mut MotorShared, env: &mut impl AppEnv) -> i16 {
        for (x, (mot, v)) in m.mots.iter_mut().zip(m.valves.iter_mut()).enumerate() {
            mot.target_position = 50;
            mot.actual_position = 50;
            mot.status = ST_UNKNOWN;
            mot.calibration = false;
            mot.calib_state = CalibState::Idle;
            v.sensorindex1 = VALVE_SENSOR_UNKNOWN;
            v.sensorindex2 = VALVE_SENSOR_UNKNOWN;
            v.learn_movements = u32::from(LEARN_AFTER_MOVEMENTS_DEFAULT);
            v.movements = 0;
            v.cmd_rejected = 0;
            v.rejected_target = VALVE_NO_TARGET;
            v.forced_learn = false;
            v.timed_learn = false;
            v.svc_hold = 0;
            v.retest_request = 0;
            v.open_request = false;
            v.assembly_hold = false;
            v.retry_learn = false;
            v.early_learn = false;
            v.calib_restored = false;
            v.stored_seq = 0;
            v.touched = false;
            // distribute the learn timing equally over the valve slots
            v.learn_time = LEARN_AFTER_TIME_DEFAULT * (x as u32 + 1) / VALVES as u32;
        }
        self.app_load_config(m, env);
        0
    }

    /// Takes the configuration from the EEPROM mirror (at start-up, and after the EEPROM could be
    /// read again): sensor assignment, learn movements and time, motor parameters, escalation,
    /// lease timeout and failsafe positions. A stored value out of range loads its default, and
    /// the mirror of the learn movements and the motor parameters is corrected.
    pub fn app_load_config(&mut self, m: &mut MotorShared, env: &mut impl AppEnv) {
        let found = self.app_find_sensors(env);
        self.app_load_config_found(m, env, &found);
    }

    /// `app_load_config()` with the sensors matched before by [`App::app_find_sensors`]: the
    /// firmware matches them outside the motor lock and takes the rest under it.
    pub fn app_load_config_found(
        &mut self,
        m: &mut MotorShared,
        env: &mut impl AppEnv,
        found: &SensorMatch,
    ) {
        // match the sensor addresses of the EEPROM with the found sensors
        Self::app_set_sensors(m, found);

        // the range of stlnm (0 = off, 50..65534), so a value set at runtime survives a restart
        let movements = sanitize_learn_movements(env.eep_content().layout.number_of_movements);
        env.eep_content().layout.number_of_movements = movements;
        if u32::from(movements) != m.learning_movements {
            self.app_set_learnmovements(m, movements);
        }
        DebugLine::new(b"learning_movements: ")
            .num(m.learning_movements)
            .send(env);

        // the range table of smotc also applies to the stored values: each field out of range
        // loads its default, and the mirror is corrected so the next write stores valid values
        let layout = env.eep_content().layout;
        let stored = MotorParams {
            low_fac: layout.currentbound_low_fac,
            high_fac: layout.currentbound_high_fac,
            start_on_power: layout.start_on_power,
            min_counts: layout.no_of_min_counts,
            max_retries: layout.max_calib_retries,
        };
        env.motor_set_params(m, &sanitize_motor_params(&stored));
        let escalation = sanitize_escalation(&env.eep_content().escalation);
        env.motor_set_escalation(m, &escalation);

        // learn time (stlnt): the countdowns start again only when it changed
        let learn_time = env.eep_content().learn_time_s;
        if learn_time != self.learning_time {
            self.app_set_learntime(m, learn_time);
        }

        // lease timeout: block A or its copy in block B, else the copy of a warm reset, else
        // the default; failsafe positions: block B, else the copies of a warm reset. The mirror
        // of both is corrected by the eeprom module, not here.
        let mut lease = LEASE_TIMEOUT_DEFAULT_MIN;
        if env.eeprom_lease_source() != LEASE_SOURCE_DEFAULT {
            lease = sanitize_lease_timeout(env.eep_content().lease_timeout_min);
        } else if self.warm_copies {
            lease = self.warm_lease;
        }
        self.lease.set_timeout(lease);
        let warm_failsafe = self.warm_copies && env.eeprom_cfg_flags() & CFG_FAILSAFE_LOST != 0;
        let stored = env.eep_content().failsafe_pct;
        for ((pct, &copy), &stored_pct) in self
            .failsafe
            .iter_mut()
            .zip(&self.warm_failsafe)
            .zip(&stored)
        {
            *pct = if warm_failsafe {
                copy
            } else {
                sanitize_failsafe_pct(stored_pct)
            };
        }
    }

    /// The requests of stdet, staop and the automatic retry change the status (and position) of
    /// a valve; they are applied here, while no valve moves, because the end of a move writes
    /// status and position of its valve.
    fn app_apply_requests(m: &mut MotorShared) {
        for (mot, v) in m.mots.iter_mut().zip(m.valves.iter_mut()) {
            // a valve with a short is not calibrated: each calibration request tests it again
            if mot.status == ST_FAILED
                && mot.fault_reason == ValveFault::Short as u8
                && (v.forced_learn
                    || v.timed_learn
                    || v.retry_learn
                    || v.early_learn
                    || mot.calibration)
            {
                v.forced_learn = false;
                v.timed_learn = false;
                v.retry_learn = false;
                v.early_learn = false;
                mot.calibration = false;
                mot.calib_state = CalibState::Idle;
                v.retest_request = RETEST_TEST;
            }
            if v.retest_request != 0 {
                // stdet: a replaced valve head must not use the old counts
                if v.retest_request == RETEST_DETECT {
                    mot.recal = true;
                }
                v.retest_request = 0;
                v.rejected_target = VALVE_NO_TARGET;
                // fake some position deviation
                mot.actual_position = 0;
                mot.status = ST_UNKNOWN;
            }
            if v.open_request {
                v.open_request = false;
                // a failed or blocked valve is not moved until a calibration clears the fault;
                // its new target 100 is counted as rejected
                if mot.status != ST_FAILED && mot.status != ST_BLOCKED {
                    mot.status = ST_FULL_OPEN;
                }
            }
        }
    }

    /// The results of the valve state machine: the end of a move for the end-stop latch, the
    /// calibration record for the EEPROM, the early-stop request, the trips for the protection
    /// guard.
    fn app_track_valves(&mut self, m: &mut MotorShared, flags: &IsrFlags, env: &mut impl AppEnv) {
        for x in 0..VALVES {
            let valve = x as u8;
            if m.mots[x].move_seq != self.move_seen[x] {
                self.move_seen[x] = m.mots[x].move_seq;
                let mut snap = ValveSnapshot::default();
                env.valve_get_snapshot(m, u32::from(valve), &mut snap);
                self.sched.move_ended(
                    valve,
                    stop_reason(snap.diag.last.stop_reason),
                    snap.diag.last_early,
                    snap.status,
                );
            }
            let mot = m.mots[x];
            if mot.calib_seq != m.valves[x].stored_seq {
                m.valves[x].stored_seq = mot.calib_seq;
                let rec = CalibRecord {
                    opening_count: mot.opening_count as u16,
                    closing_count: mot.closing_count as u16,
                    mean_current: mot.meancurrent as u16,
                    flags: (if mot.calibrated { CALIB_VALID } else { 0 })
                        + (if mot.calib_failed { CALIB_FAILED } else { 0 }),
                };
                env.eeprom_store_calib(valve, &rec);
                m.valves[x].calib_restored = false;
                if !mot.calib_failed {
                    self.sched.clear_latch(valve);
                }
            }
            if m.mots[x].early_learn_due {
                m.mots[x].early_learn_due = false;
                if !app_faulted(m, x) {
                    m.valves[x].early_learn = true;
                }
            }
            if m.mots[x].trip_seq != self.trip_seen[x] {
                self.trip_seen[x] = m.mots[x].trip_seq;
                let uptime = env.sysstat_uptime_s();
                self.guard.on_trip(valve, uptime);
            }
        }

        // short or inrush trips on several valves: the limits are off until the next start, the
        // valves they failed are tested again
        if self.guard.suspended() && !flags.protect_suspended.load(Ordering::SeqCst) {
            flags.protect_suspended.store(true, Ordering::SeqCst);
            for (mot, v) in m.mots.iter_mut().zip(m.valves.iter_mut()) {
                let fault = mot.fault_reason;
                if mot.status == ST_FAILED
                    && (fault == ValveFault::Short as u8 || fault == ValveFault::InrushTrip as u8)
                {
                    mot.status = ST_UNKNOWN;
                    v.rejected_target = VALVE_NO_TARGET;
                }
            }
        }
    }

    fn app_view(&self, m: &MotorShared, x: usize) -> ValveView {
        let drive = self.app_drive(m, x);
        let mot = &m.mots[x];
        let v = &m.valves[x];
        ValveView {
            status: mot.status,
            actual: mot.actual_position,
            target: mot.target_position,
            drive: drive.position,
            blocked_failsafe: drive.source == DriveSource::BlockedFailsafe,
            lease_forced: drive.source == DriveSource::LeaseFailsafe,
            calib_flag: mot.calibration && mot.calib_state == CalibState::Started,
            forced_learn: v.forced_learn,
            timed_learn: v.timed_learn,
            retry_learn: v.retry_learn,
            early_learn: v.early_learn,
            calibrated: mot.calibrated,
            recal: mot.recal,
            needs_reference: mot.needs_reference,
            svc_hold: v.svc_hold != 0,
            touched: v.touched,
        }
    }

    /// Hands the decision of the scheduler to the valve state machine.
    fn app_apply(&mut self, m: &mut MotorShared, env: &mut impl AppEnv, d: &Decision) {
        let x = usize::from(d.valve);
        let cmd = match d.kind {
            ActionKind::Test => {
                DebugLine::new(b"App: valve ")
                    .num(u32::from(d.valve))
                    .text(b" unknown, try to find out...")
                    .send(env);
                if env.appsetaction(m, CMD_A_TEST, u32::from(d.valve), 0, false, 0) == 0 {
                    m.valves[x].rejected_target = m.mots[x].target_position;
                }
                return;
            }
            ActionKind::Learn => {
                DebugLine::new(b"App: learning started for valve ")
                    .num(u32::from(d.valve))
                    .send(env);
                self.app_start_learn(m, env, x);
                return;
            }
            ActionKind::MarkPresent => {
                // a learn request (staln, time or movement trigger, retry, early stops) marks its
                // valve PRESENT here, while no valve moves; the next pass starts the calibration
                m.mots[x].status = ST_PRESENT;
                return;
            }
            ActionKind::OpenEnd => CMD_A_OPEN_END,
            ActionKind::CloseEnd => CMD_A_CLOSE_END,
            ActionKind::Close => CMD_A_CLOSE,
            ActionKind::Open => CMD_A_OPEN,
            ActionKind::None => return,
        };
        let flags = (if d.keep_status { MOVE_KEEP_STATUS } else { 0 })
            + (if d.reference { MOVE_REFERENCE } else { 0 });
        if env.appsetaction(m, cmd, u32::from(d.valve), d.delta, false, flags) != 0 {
            return;
        }
        // the target of this move (a failed or blocked end counts later changes)
        m.valves[x].rejected_target = m.mots[x].target_position;
        m.valves[x].touched = false;
    }

    /// The decisions of the main loop over the valve state machine (10 ms branch).
    pub fn app_loop(
        &mut self,
        m: &mut MotorShared,
        flags: &IsrFlags,
        env: &mut impl AppEnv,
    ) -> i16 {
        self.reset_check(env);

        // a motor output switched on from the debug terminal: no command for the valve machine
        if env.terminal_manual_active() {
            return 0;
        }

        // a due temperature cycle pauses a calibration series between two strokes
        if flags.temp_gap_timeout.load(Ordering::SeqCst) {
            flags.temp_gap_timeout.store(false, Ordering::SeqCst);
            self.temp.hold_timed_out(env.millis());
        }
        flags
            .temp_refresh_request
            .store(self.temp.due(env.millis()), Ordering::SeqCst);

        // if the valve machine is idle search for new tasks; no valve moves until the next
        // command, so the status and position of every valve may be changed here
        if !env.valve_idle(m) {
            return 0;
        }

        self.app_track_valves(m, flags, env);
        Self::app_apply_requests(m);

        let mut views = [ValveView::default(); VALVES];
        for (x, view) in views.iter_mut().enumerate() {
            // a counted rejected target is a target change for the calibrations of the found
            // valves
            if app_faulted(m, x) && Self::app_reject_target(m, x) {
                self.sched.note_change();
            }
            *view = self.app_view(m, x);
        }

        let input = SchedulerInputs {
            safe_mode: env.sysstat_safe_mode(),
            hold_for_temperature: self.temp.hold_commands(env.millis()),
        };
        let d = self.sched.next(&views, &input);
        self.app_apply(m, env, &d);
        0
    }

    /// Every 10th second branch: service holds, the calibration backstop, the learn time and the
    /// movement trigger; `elapsed_s` are the real seconds since the last call.
    pub fn app_10s_loop(
        &mut self,
        m: &mut MotorShared,
        env: &mut impl AppEnv,
        elapsed_s: u32,
    ) -> u8 {
        for (mot, v) in m.mots.iter_mut().zip(m.valves.iter_mut()) {
            // the valve state machine clears the hold of a refused service move
            v.svc_hold = v.svc_hold.saturating_sub(1);

            // backstop for a calibration handed to the valve state machine: its end (learn_end)
            // clears the state; a calibration that did not start, or ended without it, is
            // cleared after CALIB_START_TICKS
            if mot.calib_state == CalibState::InProgress {
                if mot.calib_active {
                    mot.calib_time = CALIB_START_TICKS;
                } else if mot.calib_time > 0 {
                    mot.calib_time -= 1;
                } else {
                    mot.calibration = false;
                    mot.calib_state = CalibState::Idle;
                }
            }
        }

        // learning times, counted in real elapsed seconds
        if self.learning_time_active > 0 {
            for x in 0..VALVES {
                let mut rest = m.valves[x].learn_time;
                let due = countdown(&mut rest, elapsed_s);
                m.valves[x].learn_time = if due { self.learning_time_active } else { rest };
                if !due {
                    continue;
                }
                // a calibration of the valve that runs now (or was just handed over) satisfies the
                // trigger; a failed or blocked valve is calibrated by staln and its automatic
                // retry only
                let mot = &m.mots[x];
                if mot.calib_active
                    || mot.calib_state == CalibState::InProgress
                    || app_faulted(m, x)
                {
                    continue;
                }
                // app_loop marks the valve PRESENT while no valve moves, the next target change
                // starts the calibration
                m.valves[x].timed_learn = true;
                DebugLine::new(b"App: Valve ")
                    .num(x as u32)
                    .text(b" will be learned soon")
                    .send(env);
            }
        }

        // learning movements
        if m.learning_movements > 0 {
            for x in 0..VALVES {
                if !m.mots[x].connected || app_faulted(m, x) {
                    continue;
                }
                if m.valves[x].learn_movements == 0 && m.mots[x].calib_state == CalibState::Idle {
                    let mot = &mut m.mots[x];
                    mot.calibration = true;
                    mot.calib_time = 10;
                    mot.calib_state = CalibState::Started;
                    m.valves[x].movements = 0;
                    m.valves[x].learn_movements = m.learning_movements;
                    // app_loop marks the valve PRESENT while no valve moves and starts the
                    // calibration
                    DebugLine::new(b"App: Valve ")
                        .num(x as u32)
                        .text(b" will be learned soon")
                        .send(env);
                }
            }
        }
        0
    }

    /// Sets the learning movements: after this number of movements a learning cycle is executed.
    pub fn app_set_learnmovements(&mut self, m: &mut MotorShared, movements: u16) -> i16 {
        // update the reload value and every valve
        m.learning_movements = u32::from(movements);
        for v in &mut m.valves {
            v.learn_movements = u32::from(movements);
            v.movements = 0;
        }
        0
    }

    /// The countdowns start again with the learn time, spread equally over the valve slots.
    fn app_learn_reload(&mut self, m: &mut MotorShared, time: u32) {
        self.learning_time_active = time;
        for (x, v) in m.valves.iter_mut().enumerate() {
            // 64 bit product, time * (x + 1) can exceed 32 bit; the result is at most time
            v.learn_time = (u64::from(time) * (x as u64 + 1) / VALVES as u64) as u32;
        }
    }

    /// Sets the learn time (stlnt, EEPROM): after time seconds a learning cycle is executed; 0 is
    /// the ESP's own schedule, honoured while a lease client is present
    /// (settings::effective_learn_time).
    pub fn app_set_learntime(&mut self, m: &mut MotorShared, time: u32) -> i16 {
        self.learning_time = time;
        let client = self.lease.client_seen_within(LEARN_TIME_CLIENT_WINDOW_S);
        self.app_learn_reload(m, effective_learn_time(time, client));
        0
    }

    /// staln: a learning cycle of the valve is executed (255: of every connected valve).
    pub fn app_set_valvelearning(&mut self, m: &mut MotorShared, valve: u16) -> i16 {
        let learn = |mot: &mut crate::motor::ValveMotor, v: &mut Valve, movements: u32| {
            // app_loop marks the valve PRESENT while no valve moves and starts the calibration
            v.forced_learn = true;
            v.svc_hold = 0;
            mot.calibration = true;
            mot.calib_state = CalibState::Started;
            mot.calib_time = 10;
            v.movements = 0;
            v.learn_movements = movements;
        };
        let movements = m.learning_movements;
        if let (Some(mot), Some(v)) = (
            m.mots.get_mut(usize::from(valve)),
            m.valves.get_mut(usize::from(valve)),
        ) {
            learn(mot, v, movements);
            return 0;
        }
        if valve == 255 {
            for (mot, v) in m.mots.iter_mut().zip(m.valves.iter_mut()) {
                if mot.connected {
                    learn(mot, v, movements);
                }
            }
            return 0;
        }
        -1
    }

    /// stdet 255: every valve is tested again; a valve found present calibrates fully
    /// (app_apply_requests).
    pub fn app_scan_valves(&self, m: &mut MotorShared) {
        for v in &mut m.valves {
            v.retest_request = RETEST_DETECT;
            v.svc_hold = 0;
        }
    }

    /// staop: the valve is opened fully (app_loop sets FULLOPEN while no valve moves; a failed or
    /// blocked valve only gets the target and stays where it is until a calibration); the
    /// assembly hold keeps the lease failsafe away from the valve until its next stgtp. 255: every
    /// valve.
    pub fn app_set_valveopen(&self, m: &mut MotorShared, valve: u16) -> i16 {
        let open = |mot: &mut crate::motor::ValveMotor, v: &mut Valve| {
            mot.target_position = 100;
            v.open_request = true;
            v.svc_hold = 0;
            v.assembly_hold = true;
        };
        if let (Some(mot), Some(v)) = (
            m.mots.get_mut(usize::from(valve)),
            m.valves.get_mut(usize::from(valve)),
        ) {
            open(mot, v);
            return 0;
        }
        if valve == 255 {
            for (mot, v) in m.mots.iter_mut().zip(m.valves.iter_mut()) {
                open(mot, v);
            }
            return 0;
        }
        -1
    }

    /// Matches the sensor addresses of the EEPROM with the found sensors and sets the index of
    /// the sensor (the position in tempsensors[], DS18 sensors only: the index space of
    /// gvlvd/gvlon and the ESP's sensor list) into the valve.
    pub fn app_match_sensors(&self, m: &mut MotorShared, env: &mut impl AppEnv) -> i16 {
        let found = self.app_find_sensors(env);
        Self::app_set_sensors(m, &found);
        0
    }

    /// The match of `app_match_sensors()` with its debug lines, without the motor state: the
    /// firmware runs it with TIM1 and TIM2 running (the lines of up to 34 sensors exceed the
    /// transmit ring and wait for USART6) and applies the result under the motor lock
    /// ([`App::app_set_sensors`]). A sensor in a slot of several valves, or several sensors in
    /// one slot: the last match counts, as in C++.
    pub fn app_find_sensors(&self, env: &mut impl AppEnv) -> SensorMatch {
        let count = env.ds18_count().min(MAXONEWIRECNT);
        let mut found_at = [(VALVE_SENSOR_UNKNOWN, VALVE_SENSOR_UNKNOWN); VALVES];
        env.debug(b"Read 1-wire sensor addresses from eeprom\r\n");
        for sensor in 0..count {
            let address = env.ds18_address(sensor);
            let mut found = false;
            env.print_address(&address);
            for (valve, slots) in found_at.iter_mut().enumerate() {
                let layout = &env.eep_content().layout;
                let first = app_sensor_matches(&layout.owsensors1[valve], &address);
                let second = app_sensor_matches(&layout.owsensors2[valve], &address);
                // first sensor of the valve
                if first {
                    DebugLine::new(b" found as 1st sensor at valve: ")
                        .num(valve as u32)
                        .text(b":")
                        .num(u32::from(sensor))
                        .send(env);
                    slots.0 = u32::from(sensor);
                    found = true;
                }
                // second sensor of the valve
                if second {
                    DebugLine::new(b" found as 2nd sensor at valve: ")
                        .num(valve as u32)
                        .send(env);
                    slots.1 = u32::from(sensor);
                    found = true;
                }
            }
            if !found {
                env.debug(b" not found\r\n");
            }
        }
        found_at
    }

    /// The sensor indices of [`App::app_find_sensors`] into the valves.
    pub fn app_set_sensors(m: &mut MotorShared, found: &SensorMatch) {
        for (v, &(first, second)) in m.valves.iter_mut().zip(found) {
            v.sensorindex1 = first;
            v.sensorindex2 = second;
        }
    }

    /// Sets the soft reset request (reset command).
    pub fn reset_stm32(&mut self, env: &mut impl AppEnv) {
        self.reset_request = 1;
        env.debug(b"prepare for soft reset\r\n");
    }

    /// The soft reset waits until the EEPROM is written completely.
    pub fn reset_check(&mut self, env: &mut impl AppEnv) {
        if self.reset_request != 0 && env.eeprom_free() {
            env.debug(b"soft reset now\r\n");
            env.reset();
        }
    }

    /// Every second: the lease, the learn time without a lease client, the automatic retries.
    pub fn app_1s_tick(&mut self, m: &mut MotorShared, elapsed_s: u32) {
        self.lease.advance(elapsed_s);

        let client = self.lease.client_seen_within(LEARN_TIME_CLIENT_WINDOW_S);
        let active = effective_learn_time(self.learning_time, client);
        if active != self.learning_time_active {
            self.app_learn_reload(m, active);
        }

        for x in 0..VALVES {
            let mot = &m.mots[x];
            let v = &m.valves[x];
            // a calibration or presence test of the valve is requested or runs: no retry is
            // counted down
            let busy = v.retest_request != 0
                || v.forced_learn
                || v.retry_learn
                || mot.calibration
                || mot.calib_active
                || mot.calib_state == CalibState::InProgress
                || mot.status == ST_UNKNOWN
                || mot.status == ST_PRESENT;
            let faulted = app_faulted(m, x);
            if self.retry[x].update(faulted, busy, elapsed_s) {
                // a short is tested again, anything else calibrates (without waiting for a
                // target change)
                if m.mots[x].fault_reason == ValveFault::Short as u8 {
                    m.valves[x].retest_request = RETEST_TEST;
                } else {
                    m.valves[x].retry_learn = true;
                }
            }
        }
    }

    /// Start-up, after valve_setup() and before the valve timer runs: the calibration records of
    /// the EEPROM, and after a warm reset the positions, the lease and the retry schedule.
    pub fn app_restore(&mut self, m: &mut MotorShared, env: &mut impl AppEnv) {
        let image = env.read();
        let kept = warm_from_bytes(&image[..WARM_BYTES]);
        let warm = is_warm_boot(env.sysstat_boot_reason()) && warm_state_valid(&kept);

        for x in 0..VALVES {
            let rec = env.eep_content().calib[x];
            let mot = &mut m.mots[x];
            if rec.flags & CALIB_VALID != 0 {
                mot.opening_count = u32::from(rec.opening_count);
                mot.closing_count = u32::from(rec.closing_count);
                mot.deadzone_count = i32::from(rec.closing_count) - i32::from(rec.opening_count);
                mot.scaler = u32::from(rec.opening_count / 100);
                mot.meancurrent = u32::from(rec.mean_current);
                mot.calibrated = true;
                m.valves[x].calib_restored = true;
            }
            // the last calibration ended blocked: the counts are not trusted, a full
            // calibration follows
            if rec.flags & CALIB_FAILED != 0 {
                mot.recal = true;
            }
            m.valves[x].stored_seq = mot.calib_seq;

            if !warm {
                continue;
            }
            let w = &kept.valves[x];
            let r = restore_valve(w, mot.calibrated);
            // a record that does not pass its checks: this valve starts cold (presence test)
            if !r.valid {
                continue;
            }
            mot.status = r.status;
            mot.actual_position = r.actual;
            mot.target_position = r.target;
            mot.needs_reference = r.needs_reference;
            if r.recal {
                mot.recal = true;
            }
            mot.connected = r.status != ST_UNKNOWN && r.status != ST_OPEN_CIRCUIT;
            m.valves[x].assembly_hold = r.assembly_hold;
            m.valves[x].rejected_target = r.target;
            self.retry[x].restore(&RetrySnapshot {
                attempts: w.retry_attempts,
                scheduled: w.retry_scheduled != 0,
                remaining_s: w.retry_remaining_s,
            });
        }

        if !warm {
            return;
        }
        // lease timeout and failsafe positions the EEPROM could not supply: the copies of the
        // last run
        self.warm_copies = true;
        self.warm_lease = sanitize_lease_timeout(kept.lease_timeout_min);
        for (copy, &kept_pct) in self.warm_failsafe.iter_mut().zip(&kept.failsafe_pct) {
            *copy = sanitize_failsafe_pct(kept_pct);
        }
        if env.eeprom_lease_source() == LEASE_SOURCE_DEFAULT {
            self.lease.set_timeout(self.warm_lease);
        }
        if env.eeprom_cfg_flags() & CFG_FAILSAFE_LOST != 0 {
            self.failsafe = self.warm_failsafe;
        }
        self.lease.restore(&LeaseSnapshot {
            since_renewal_s: kept.lease_since_renewal_s,
            since_client_s: kept.lease_since_client_s,
            client: kept.lease_client != 0,
        });
    }

    /// Main loop, 10 ms branch: the record app_restore() reads after a warm reset, written in
    /// every pass.
    pub fn app_warm_save(&self, m: &MotorShared, env: &mut impl AppEnv) {
        let mut ws = WarmState::default();
        // a move or calibration in work: the position of its valve is not valid
        let busy = env.valve_busy_index(m);
        for x in 0..VALVES {
            let mot = &m.mots[x];
            let w = &mut ws.valves[x];
            w.actual = mot.actual_position;
            w.target = mot.target_position;
            w.status = mot.status;
            w.flags = (if busy == x as i32 { 0 } else { WARM_POS_VALID })
                + (if m.valves[x].assembly_hold {
                    WARM_ASSEMBLY_HOLD
                } else {
                    0
                })
                + (if mot.needs_reference {
                    WARM_NEEDS_REFERENCE
                } else {
                    0
                })
                + (if mot.recal { WARM_RECAL } else { 0 });
            let retry = self.retry[x].snapshot();
            w.retry_attempts = retry.attempts;
            w.retry_scheduled = u8::from(retry.scheduled);
            w.retry_remaining_s = retry.remaining_s;
            ws.failsafe_pct[x] = self.failsafe[x];
        }
        let lease = self.lease.snapshot();
        ws.lease_since_renewal_s = lease.since_renewal_s;
        ws.lease_since_client_s = lease.since_client_s;
        ws.lease_client = u8::from(lease.client);
        ws.lease_timeout_min = self.lease.timeout();
        warm_state_seal(&mut ws);
        let mut image = env.read();
        // the C++ copies the whole struct: its trailing padding is 0
        image[..WARM_IMAGE].fill(0);
        image[..WARM_BYTES].copy_from_slice(&warm_to_bytes(&ws));
        env.write(&image);
    }

    /// Called by appsetaction()/appsetservice() in the step that hands the command over: a reset
    /// from here on must not trust the kept position of the valve.
    pub fn app_warm_moving(store: &mut impl NoinitStore, valve: u32) {
        let Ok(x) = usize::try_from(valve) else {
            return;
        };
        let mut image: [u8; NOINIT_SIZE] = store.read();
        let mut ws = warm_from_bytes(&image[..WARM_BYTES]);
        if x >= VALVES || !warm_state_valid(&ws) {
            return;
        }
        ws.valves[x].flags &= !WARM_POS_VALID;
        warm_state_seal(&mut ws);
        image[..WARM_BYTES].copy_from_slice(&warm_to_bytes(&ws));
        store.write(&image);
    }

    pub fn app_lease_poll(&mut self) {
        self.lease.valve_poll();
    }

    pub fn app_lease_command(&mut self) {
        self.lease.lease_command();
    }

    pub fn app_lease_heartbeat(&mut self, alive: bool) {
        self.lease.heartbeat(alive);
    }

    /// slcfg, validated and persisted by the caller
    pub fn app_lease_configure(&mut self, minutes: u16) {
        self.lease.set_timeout(minutes);
    }

    /// 0 off, 1 running, 2 expired
    pub fn app_lease_state(&self) -> u8 {
        self.lease.state() as u8
    }

    pub fn app_lease_remaining_s(&self) -> u32 {
        self.lease.remaining_s()
    }

    pub fn app_lease_client(&self) -> bool {
        self.lease.client_present()
    }

    pub fn app_lease_timeout(&self) -> u16 {
        self.lease.timeout()
    }

    /// bit v: valve v is at its failsafe position because the lease expired
    pub fn app_failsafe_mask(&self, m: &MotorShared) -> u16 {
        let mut mask = 0u16;
        for x in 0..VALVES {
            if self.app_drive(m, x).source == DriveSource::LeaseFailsafe {
                mask += 1 << x;
            }
        }
        mask
    }

    /// sfspo: valve 0..11 or 255; runtime only (the caller validates and persists)
    pub fn app_set_failsafe(&mut self, valve: u16, pct: u8) {
        for x in 0..VALVES {
            if valve == 255 || usize::from(valve) == x {
                self.failsafe[x] = pct;
                // newer than the copy of the last run: a re-read that finds block B damaged
                // keeps it
                self.warm_failsafe[x] = pct;
            }
        }
    }

    pub fn app_failsafe_pct(&self, valve: u16) -> u8 {
        self.failsafe
            .get(usize::from(valve))
            .copied()
            .unwrap_or(FAILSAFE_HOLD)
    }

    /// sstop: stops what runs for the valve (255: whatever runs), cancels the requested
    /// calibrations and leaves the stopped valve where it is (service hold); -1 for an invalid
    /// valve.
    pub fn app_stop(&mut self, m: &mut MotorShared, env: &mut impl AppEnv, valve: u16) -> i16 {
        if usize::from(valve) >= VALVES && valve != 255 {
            return -1;
        }
        let stopped = env.appstop(m, u32::from(valve));
        for (x, (mot, v)) in m.mots.iter_mut().zip(m.valves.iter_mut()).enumerate() {
            if valve != 255 && usize::from(valve) != x {
                continue;
            }
            v.forced_learn = false;
            v.timed_learn = false;
            v.retry_learn = false;
            v.early_learn = false;
            // a running calibration ends through the stop (learn_abort)
            if !mot.calib_active {
                mot.calibration = false;
                mot.calib_state = CalibState::Idle;
            }
            // PRESENT from such a request; a valve that needs its first calibration stays pending
            if mot.status == ST_PRESENT && mot.calibrated && !mot.recal {
                mot.status = ST_IDLE;
            }
        }
        let held = if valve != 255 {
            usize::from(valve)
        } else {
            match usize::try_from(stopped) {
                Ok(x) => x,
                Err(_) => return 0,
            }
        };
        m.valves[held].svc_hold = crate::app::SVMOV_HOLD_10S;
        0
    }

    /// stored learn time (gtlnt)
    pub fn app_get_learntime(&self) -> u32 {
        self.learning_time
    }

    /// owDevices: a temperature cycle completed
    pub fn app_temp_cycle_done(&mut self, env: &mut impl AppEnv) {
        self.temp.cycle_done(env.millis());
    }

    /// seconds since the last complete temperature cycle
    pub fn app_temp_age_s(&self, env: &mut impl AppEnv) -> u32 {
        self.temp.age_s(env.millis())
    }

    /// the short and inrush limits are suspended until the next start
    pub fn app_protect_suspended(&self) -> bool {
        self.guard.suspended()
    }

    /// gvlvy values 20..25.
    pub fn app_get_valve_v3(
        &self,
        m: &MotorShared,
        env: &mut impl AppEnv,
        valve: u16,
        out: &mut ValveV3Info,
    ) {
        let x = usize::from(valve);
        if x >= VALVES {
            *out = ValveV3Info {
                fs_pct: FAILSAFE_HOLD,
                ..ValveV3Info::default()
            };
            return;
        }
        let mut snap = ValveSnapshot::default();
        env.valve_get_snapshot(m, u32::from(valve), &mut snap);
        let drive = self.app_drive(m, x);
        let mot = &m.mots[x];
        let v = &m.valves[x];
        let mut flags = 0u16;
        if drive.source == DriveSource::LeaseFailsafe {
            flags += VLV_FLAG_FS_LEASE;
        }
        if drive.source == DriveSource::BlockedFailsafe {
            flags += VLV_FLAG_FS_BLOCKED;
        }
        if !mot.calibrated {
            flags += VLV_FLAG_UNCALIBRATED;
        }
        if mot.needs_reference {
            flags += VLV_FLAG_NEEDS_REF;
        }
        if mot.recal {
            flags += VLV_FLAG_RECAL;
        }
        if v.calib_restored {
            flags += VLV_FLAG_CAL_RESTORED;
        }
        if self.retry[x].scheduled() {
            flags += VLV_FLAG_RETRY;
        }
        if snap.diag.early_run.count() == 1 {
            flags += VLV_FLAG_EARLY_PENDING;
        }
        if v.assembly_hold {
            flags += VLV_FLAG_ASSEMBLY;
        }
        if v.svc_hold != 0 {
            flags += VLV_FLAG_SVC_HOLD;
        }
        *out = ValveV3Info {
            flags,
            fault: mot.fault_reason,
            fs_pct: self.failsafe[x],
            drive: drive.position,
            retry_s: self.retry[x].remaining_s(),
            retries: self.retry[x].attempts(),
        };
    }
}

#[cfg(test)]
mod stub_env;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_mut;
#[cfg(test)]
mod tests_unit;
#[cfg(test)]
mod tests_v3;
#[cfg(test)]
mod tests_warm;
