//! Types shared between the STM task and the other tasks: the commands into the STM task and
//! the STM snapshot it publishes (DESIGN.md "Task model"; port of `vdm/stm_types.h`).
//! Hardware-free.

use crate::common::{OneWireId, Text, NO_VALVE, TEMP_SLOT_COUNT, VALVE_COUNT, VOLT_SLOT_COUNT};
use crate::failsafe::LeaseStatus;
use crate::link_policy::{LinkState, LinkStats};
use crate::stm_codec::{Breakaway, MotorChars, MoveDir, StmStatus};
use crate::stm_flasher::FlashStatus;
use crate::valve_model::{TargetSource, TempReading, ValveState, VoltReading};
use crate::version::{StmSupport, Version};

// ---------------------------------------------------------------- commands

/// Commands into the STM task (web and MQTT never touch the UART or the model directly).
/// Validation happens before submit(): the STM task only re-checks invariants the core builders
/// enforce anyway. The numbers are the C++ declaration order.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StmCommandType {
    /// valve, pos, source
    SetTarget = 0,
    /// valve or ALL_VALVES
    Calibrate = 1,
    /// valve or ALL_VALVES
    Assembly = 2,
    /// stdet 255
    Detect = 3,
    /// stons, then lists
    ScanSensors = 4,
    /// valve, ids[2]; followed by masns + gvlon 255
    SetValveSensors = 5,
    /// motor / learn_movements / breakaway (v2), each if its has_* flag
    SetMotorSettings = 6,
    /// valve, dir, counts, max_ma (v2)
    ServiceMove = 7,
    /// valve (v2)
    RequestProfile = 8,
    /// NRST pulse by the user
    ResetStm = 9,
    /// image, blank, force, board
    StartFlash = 10,
    AbortFlash = 11,
    /// active mask / sensor slots changed: re-read config
    ConfigChanged = 12,
    /// valve or ALL_VALVES: sstop (protocol 3)
    StopValve = 13,
    /// ssafe 0 (protocol 3)
    LeaveSafeMode = 14,
}

/// One command into the STM task; only the fields of its [`StmCommandType`] are read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StmCommand {
    /// C++ `type` (a Rust keyword); default ConfigChanged
    pub kind: StmCommandType,
    /// default NO_VALVE
    pub valve: u8,
    pub pos: u8,
    pub source: TargetSource,
    pub ids: [OneWireId; 2],
    /// SetMotorSettings: one command, so a request is queued completely or not at all even
    /// while other tasks submit concurrently.
    pub has_motor: bool,
    pub motor: MotorChars,
    pub has_learn_movements: bool,
    pub learn_movements: u16,
    pub has_breakaway: bool,
    pub breakaway: Breakaway,
    pub dir: MoveDir,
    pub counts: u16,
    /// C++ `maxmA`
    pub max_ma: u8,
    /// LittleFS name below /stm/ (C++ `char[32]`)
    pub image: Text<31>,
    pub blank: bool,
    pub force: bool,
    /// Calibrate: fired by the calibration schedule
    pub scheduled: bool,
    /// StartFlash: board revision chosen by the user ("C1", "C2"), "" none (C++ `char[4]`)
    pub board: Text<3>,
    /// Calibrate (scheduled): attempt number of the slot
    pub attempt: u16,
}

impl Default for StmCommand {
    fn default() -> Self {
        Self {
            kind: StmCommandType::ConfigChanged,
            valve: NO_VALVE,
            pos: 0,
            source: TargetSource::None,
            ids: [OneWireId::default(); 2],
            has_motor: false,
            motor: MotorChars::default(),
            has_learn_movements: false,
            learn_movements: 0,
            has_breakaway: false,
            breakaway: Breakaway::default(),
            dir: MoveDir::Open,
            counts: 0,
            max_ma: 0,
            image: Text::new(),
            blank: false,
            force: false,
            scheduled: false,
            board: Text::new(),
            attempt: 0,
        }
    }
}

/// State of the STM EEPROM save before an ESP restart (the restart also resets the STM when
/// jumper X20 is fitted).
#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StmSaveState {
    #[default]
    Idle = 0,
    /// a restart asked for the save
    Waiting = 1,
    /// the STM reported no pending EEPROM write
    Saved = 2,
    /// link not up or flashing: nothing to wait for
    Unavailable = 3,
    TimedOut = 4,
}

impl StmSaveState {
    pub fn from_raw(v: u8) -> Option<Self> {
        [
            Self::Idle,
            Self::Waiting,
            Self::Saved,
            Self::Unavailable,
            Self::TimedOut,
        ]
        .get(usize::from(v))
        .copied()
    }
}

// ---------------------------------------------------------------- snapshot

/// Everything other tasks may know about the STM, published by the STM task after every change
/// (copy under a mutex, never references into the model). About 3 KB: keep it off the stacks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StmSnapshot {
    pub revision: u32,
    pub taken_ms: u32,
    pub valves: [ValveState; VALVE_COUNT as usize],
    pub temps: [TempReading; TEMP_SLOT_COUNT as usize],
    pub temp_count: u8,
    pub volts: [VoltReading; VOLT_SLOT_COUNT as usize],
    pub volt_count: u8,
    pub link: LinkState,
    pub link_stats: LinkStats,
    pub proto: u8,
    pub version: Version,
    pub build: u32,
    pub hw_id: u16,
    /// default true
    pub compatible: bool,
    pub have_motor: bool,
    pub motor: MotorChars,
    pub learn_movements: u16,
    pub have_breakaway: bool,
    pub breakaway: Breakaway,
    pub have_status: bool,
    pub status: StmStatus,
    /// Link Up, re-sync finished and the 30 s sensor grace over: valve sensor assignments and
    /// temperatures reflect the STM (HA first-run cleanup).
    pub sensors_settled: bool,
    pub line_overflows: u32,
    pub line_malformed: u32,
    pub flash: FlashStatus,
    /// C++ `char[32]`
    pub flash_image: Text<31>,
    /// gprof replies per valve since boot. The profiles themselves (3 KB) are not part of the
    /// snapshot: the session hands each one to the port (StmSessionPort::storeProfile), and a
    /// reader copies one profile when its count here changed.
    pub profile_seq: [u32; VALVE_COUNT as usize],
    pub support: StmSupport,
    pub lease: LeaseStatus,
    /// gtlnt (protocol 3) read
    pub have_learn_time: bool,
    pub learn_time_s: u32,
    /// a flash waits for the STM EEPROM
    pub flash_pending: bool,
}

impl Default for StmSnapshot {
    fn default() -> Self {
        Self {
            revision: 0,
            taken_ms: 0,
            valves: [ValveState::EMPTY; VALVE_COUNT as usize],
            temps: [TempReading::EMPTY; TEMP_SLOT_COUNT as usize],
            temp_count: 0,
            volts: [VoltReading::EMPTY; VOLT_SLOT_COUNT as usize],
            volt_count: 0,
            link: LinkState::Unknown,
            link_stats: LinkStats::default(),
            proto: 0,
            version: Version::default(),
            build: 0,
            hw_id: 0,
            compatible: true,
            have_motor: false,
            motor: MotorChars::default(),
            learn_movements: 0,
            have_breakaway: false,
            breakaway: Breakaway::default(),
            have_status: false,
            status: StmStatus::default(),
            sensors_settled: false,
            line_overflows: 0,
            line_malformed: 0,
            flash: FlashStatus::default(),
            flash_image: Text::new(),
            profile_seq: [0; VALVE_COUNT as usize],
            support: StmSupport::Unknown,
            lease: LeaseStatus::default(),
            have_learn_time: false,
            learn_time_s: 0,
            flash_pending: false,
        }
    }
}

#[cfg(test)]
mod tests;
