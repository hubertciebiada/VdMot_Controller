//! ESP-side model of the STM: per-valve state built from replies, desired targets with
//! acknowledged + verified delivery, derived health flags, and the 1-Wire sensor readings (port
//! of `vdm/valve_model.h`). Hardware-free. Owned by the stm_link task; other tasks only see
//! copies (snapshots).

use crate::common::{
    crc_valid, elapsed_ms, is_zero, OneWireId, ALL_VALVES, TEMP_POWER_ON, TEMP_READ_ERROR,
    TEMP_SLOT_COUNT, TEMP_UNASSIGNED, VAD_FAILED, VALVE_COUNT, VOLT_SLOT_COUNT,
};
use crate::failsafe::{FailsafeKind, FAILSAFE_HOLD};
use crate::stm_codec::{
    resolve_temp_slot, Cmd, MoveDir, MoveResult, OneWireList, RequestLine, StopReason, TargetReply,
    TempData, ValveData, ValveEx, ValveSensors, ValveStates, ValveStatus, VoltData,
    STM_FLAG_ASSEMBLY, STM_FLAG_FS_BLOCKED, STM_FLAG_FS_LEASE,
};

/// Valves (the arrays of the model).
const N: usize = VALVE_COUNT as usize;

const STATUS_TEXT: [&str; 10] = [
    "",
    "idle",
    "opens",
    "closes",
    "failed",
    "unknown",
    "no valve",
    "full open",
    "connected",
    "blocked",
];
const STATUS_KEY: [&str; 10] = [
    "nodata",
    "idle",
    "opening",
    "closing",
    "failed",
    "unknown",
    "novalve",
    "fullopen",
    "connected",
    "blocked",
];

const STATUS_OPENING: u8 = ValveStatus::Opening as u8;
const STATUS_CLOSING: u8 = ValveStatus::Closing as u8;
const STATUS_FAILED: u8 = ValveStatus::Failed as u8;
const STATUS_NO_VALVE: u8 = ValveStatus::NoValve as u8;
const STATUS_BLOCKED: u8 = ValveStatus::Blocked as u8;

/// Legacy text for a status value (MQTT plain-text payloads): 0 "", 1 "idle", 2 "opens",
/// 3 "closes", 4 "failed", 5 "unknown", 6 "no valve", 7 "full open", 8 "connected",
/// 9 "blocked", >= 10 "".
pub fn valve_status_text(status: u8) -> &'static str {
    STATUS_TEXT.get(usize::from(status)).copied().unwrap_or("")
}

/// Stable machine name for the new API: "nodata", "idle", "opening", "closing", "failed",
/// "unknown", "novalve", "fullopen", "connected", "blocked", "invalid" (>= 10).
pub fn valve_status_key(status: u8) -> &'static str {
    STATUS_KEY
        .get(usize::from(status))
        .copied()
        .unwrap_or("invalid")
}

/// Who set the desired target (event log, API; the numbers are stored by target_store).
#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TargetSource {
    #[default]
    None = 0,
    /// adopted from the STM at (re)sync (gtgtp/gvlvx), ESP boot
    Stm = 1,
    /// HTTP /api/valves/{n}/target
    Web = 2,
    /// MQTT valves/<V>/target
    Mqtt = 3,
    /// from the RTC/NVS copy at ESP boot
    Restored = 4,
    /// staop: 100 %, held until the next web/MQTT target
    Assembly = 5,
}

impl TargetSource {
    pub fn from_raw(v: u8) -> Option<Self> {
        [
            Self::None,
            Self::Stm,
            Self::Web,
            Self::Mqtt,
            Self::Restored,
            Self::Assembly,
        ]
        .get(usize::from(v))
        .copied()
    }
}

/// "none", "stm", "web", "mqtt", "restored", "assembly".
pub fn target_source_name(s: TargetSource) -> &'static str {
    match s {
        TargetSource::None => "none",
        TargetSource::Stm => "stm",
        TargetSource::Web => "web",
        TargetSource::Mqtt => "mqtt",
        TargetSource::Restored => "restored",
        TargetSource::Assembly => "assembly",
    }
}

/// Delivery state of the desired target (see DESIGN.md "Target delivery").
#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TargetSync {
    /// no desired target and STM target not read yet
    #[default]
    Unknown = 0,
    /// STM target verified equal to desired
    Synced = 1,
    /// desired differs / needs (re)push; waits for backoff and !calibrating
    Pending = 2,
    /// stgtp handed out, waiting for its ack
    AwaitAck = 3,
    /// acked; waiting for a read-back (gtgtp v1 / gvlvx v2)
    AwaitVerify = 4,
    /// max_push_attempts reached; retried after failed_retry_ms
    Failed = 5,
}

impl TargetSync {
    pub fn from_raw(v: u8) -> Option<Self> {
        [
            Self::Unknown,
            Self::Synced,
            Self::Pending,
            Self::AwaitAck,
            Self::AwaitVerify,
            Self::Failed,
        ]
        .get(usize::from(v))
        .copied()
    }
}

/// "unknown", "synced", "pending", "await_ack", "await_verify", "failed".
pub fn target_sync_name(s: TargetSync) -> &'static str {
    match s {
        TargetSync::Unknown => "unknown",
        TargetSync::Synced => "synced",
        TargetSync::Pending => "pending",
        TargetSync::AwaitAck => "await_ack",
        TargetSync::AwaitVerify => "await_verify",
        TargetSync::Failed => "failed",
    }
}

// Health flags (C++ `enum HealthFlag : uint16_t`, a bitmask). Derived on every update; see
// DESIGN.md "Health".
/// status 9
pub const HEALTH_BLOCKED: u16 = 0x0001;
/// status 4
pub const HEALTH_FAILED: u16 = 0x0002;
/// status 6 on an ACTIVE valve
pub const HEALTH_NO_VALVE: u16 = 0x0004;
/// calib_retries > 0 (last calibration needed retries)
pub const HEALTH_CALIB_RETRIES: u16 = 0x0008;
/// early_stops increased since ESP boot (v2)
pub const HEALTH_EARLY_STOP: u16 = 0x0010;
/// cmd_rejected increased since ESP boot (v2)
pub const HEALTH_CMD_REJECTED: u16 = 0x0020;
/// active valve: no data for stale_ms
pub const HEALTH_STALE: u16 = 0x0040;
/// TargetSync::Failed, and while that target is retried
pub const HEALTH_TARGET_UNCONFIRMED: u16 = 0x0080;
/// an assigned sensor reports a sentinel
pub const HEALTH_TEMP_FAILED: u16 = 0x0100;
/// at its lease failsafe position (STM or ESP emulation)
pub const HEALTH_FAILSAFE: u16 = 0x0200;
/// calibration stroke close to min_counts
pub const HEALTH_STROKE_SHORT: u16 = 0x0400;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ValveState {
    /// at least one gvlvd/gvlvx/gvlst applied
    pub known: bool,
    /// now_ms of the last gvlvd/gvlvx
    pub last_seen_ms: u32,
    /// ValveStatus value (0 = no data yet)
    pub status: u8,
    pub calibrating: bool,
    /// % estimated by the STM
    pub position: u8,
    /// mA
    pub mean_current: u16,
    /// raw 0.1 C incl. sentinels
    pub temp1: i16,
    pub temp2: i16,
    pub moves: u32,
    pub open_count: u32,
    pub close_count: u32,
    pub dead_zone: i32,
    pub calib_retries: u8,
    // v2 only (has_extended)
    pub has_extended: bool,
    /// CAL_STATE_* phase
    pub cal_state: u8,
    /// CAL_FLAG_* bits
    pub cal_flags: u8,
    pub early_stops: u32,
    pub cmd_rejected: u32,
    /// first value seen after ESP boot / STM reboot
    pub early_stops_at_boot: u32,
    pub cmd_rejected_at_boot: u32,
    pub last_move: MoveResult,
    /// +1 each time last_move changes
    pub move_seq: u32,
    // target delivery
    pub desired_valid: bool,
    /// 0..100
    pub desired: u8,
    pub source: TargetSource,
    pub stm_target_known: bool,
    pub stm_target: u8,
    pub sync: TargetSync,
    pub push_attempts: u8,
    pub last_push_ms: u32,
    /// sensor assignment reported by the STM (gvlon), resolved to config slots
    pub sensor_id: [OneWireId; 2],
    /// 1-based config slot, 0 = none/unknown
    pub sensor_slot: [u8; 2],
    /// HEALTH_* bits
    pub health: u16,
    /// +1 on every change of any field above except the last_seen_ms/last_push_ms timestamps
    /// (a poll that returns the same data is not a change).
    pub revision: u32,
    // protocol 3 (gvlvy)
    pub has_v3: bool,
    /// STM_FLAG_*
    pub stm_flags: u16,
    /// ValveFault
    pub fault: u8,
    /// Failsafe position that applies: gvlvy (protocol 3) or the ESP config (protocols 1/2,
    /// FAILSAFE_HOLD for inactive valves).
    pub fs_pct: u8,
    /// target the STM drives to
    pub drive: u8,
    /// s to the next automatic calibration retry
    pub retry_s: u32,
    /// automatic retries since the fault began
    pub retries: u8,
    /// retries rose since the last calibration end
    pub auto_retry: bool,
    // ESP failsafe emulation (protocols 1/2): fs_target is pushed instead of desired.
    pub fs_override: bool,
    pub fs_target: u8,
    /// push the desired target once even if the read-back equals it
    pub force_push: bool,
}

impl ValveState {
    /// A valve nothing is known about (C++ `ValveState{}`).
    pub const EMPTY: Self = Self {
        known: false,
        last_seen_ms: 0,
        status: 0,
        calibrating: false,
        position: 0,
        mean_current: 0,
        temp1: TEMP_UNASSIGNED,
        temp2: TEMP_UNASSIGNED,
        moves: 0,
        open_count: 0,
        close_count: 0,
        dead_zone: 0,
        calib_retries: 0,
        has_extended: false,
        cal_state: 0,
        cal_flags: 0,
        early_stops: 0,
        cmd_rejected: 0,
        early_stops_at_boot: 0,
        cmd_rejected_at_boot: 0,
        last_move: MoveResult {
            dir: MoveDir::Open,
            requested_counts: 0,
            counted_counts: 0,
            stop: StopReason::None,
            peak_current: 0,
            duration_ms: 0,
        },
        move_seq: 0,
        desired_valid: false,
        desired: 0,
        source: TargetSource::None,
        stm_target_known: false,
        stm_target: 0,
        sync: TargetSync::Unknown,
        push_attempts: 0,
        last_push_ms: 0,
        sensor_id: [OneWireId { b: [0; 8] }; 2],
        sensor_slot: [0; 2],
        health: 0,
        revision: 0,
        has_v3: false,
        stm_flags: 0,
        fault: 0,
        fs_pct: FAILSAFE_HOLD,
        drive: 0,
        retry_s: 0,
        retries: 0,
        auto_retry: false,
        fs_override: false,
        fs_target: 0,
        force_push: false,
    };
}

impl Default for ValveState {
    fn default() -> Self {
        Self::EMPTY
    }
}

// Field groups for change detection (publishing, events; C++ `enum ValveChange : uint32_t`).
/// status or calibrating
pub const CHANGE_STATUS: u32 = 0x0001;
pub const CHANGE_POSITION: u32 = 0x0002;
/// desired
pub const CHANGE_TARGET: u32 = 0x0004;
pub const CHANGE_MEAN_CURRENT: u32 = 0x0008;
pub const CHANGE_TEMP1: u32 = 0x0010;
pub const CHANGE_TEMP2: u32 = 0x0020;
/// moves, open_count, close_count, dead_zone
pub const CHANGE_COUNTERS: u32 = 0x0040;
pub const CHANGE_CALIB_RETRIES: u32 = 0x0080;
/// cal_state, cal_flags, early_stops, cmd_rejected
pub const CHANGE_EXTENDED: u32 = 0x0100;
/// move_seq
pub const CHANGE_LAST_MOVE: u32 = 0x0200;
pub const CHANGE_SYNC: u32 = 0x0400;
pub const CHANGE_SENSORS: u32 = 0x0800;
pub const CHANGE_HEALTH: u32 = 0x1000;
pub const CHANGE_KNOWN: u32 = 0x2000;
/// has_v3, stm_flags, fault, fs_pct, drive, retries, auto_retry, fs_override, fs_target
pub const CHANGE_FAILSAFE: u32 = 0x4000;

/// Bitmask of groups that differ between two snapshots of one valve. The groups are added in
/// increasing bit order, so `+` is the `|` of the C++.
pub fn diff_valve(a: &ValveState, b: &ValveState) -> u32 {
    let mut m = 0;
    if a.status != b.status || a.calibrating != b.calibrating {
        m += CHANGE_STATUS;
    }
    if a.position != b.position {
        m += CHANGE_POSITION;
    }
    if a.desired_valid != b.desired_valid || a.desired != b.desired {
        m += CHANGE_TARGET;
    }
    if a.mean_current != b.mean_current {
        m += CHANGE_MEAN_CURRENT;
    }
    if a.temp1 != b.temp1 {
        m += CHANGE_TEMP1;
    }
    if a.temp2 != b.temp2 {
        m += CHANGE_TEMP2;
    }
    if a.moves != b.moves
        || a.open_count != b.open_count
        || a.close_count != b.close_count
        || a.dead_zone != b.dead_zone
    {
        m += CHANGE_COUNTERS;
    }
    if a.calib_retries != b.calib_retries {
        m += CHANGE_CALIB_RETRIES;
    }
    if a.cal_state != b.cal_state
        || a.cal_flags != b.cal_flags
        || a.early_stops != b.early_stops
        || a.cmd_rejected != b.cmd_rejected
    {
        m += CHANGE_EXTENDED;
    }
    if a.move_seq != b.move_seq {
        m += CHANGE_LAST_MOVE;
    }
    if a.sync != b.sync || a.stm_target_known != b.stm_target_known || a.stm_target != b.stm_target
    {
        m += CHANGE_SYNC;
    }
    if a.sensor_id != b.sensor_id || a.sensor_slot != b.sensor_slot {
        m += CHANGE_SENSORS;
    }
    if a.health != b.health {
        m += CHANGE_HEALTH;
    }
    if a.known != b.known {
        m += CHANGE_KNOWN;
    }
    if a.has_v3 != b.has_v3
        || a.stm_flags != b.stm_flags
        || a.fault != b.fault
        || a.fs_pct != b.fs_pct
        || a.drive != b.drive
        || a.retries != b.retries
        || a.auto_retry != b.auto_retry
        || a.fs_override != b.fs_override
        || a.fs_target != b.fs_target
    {
        m += CHANGE_FAILSAFE;
    }
    m
}

/// Every field that counts for [`ValveState::revision`]; the timestamps last_seen_ms and
/// last_push_ms and the revision itself are excluded.
fn same_state(a: &ValveState, b: &ValveState) -> bool {
    diff_valve(a, b) == 0
        && a.source == b.source
        && a.push_attempts == b.push_attempts
        && a.has_extended == b.has_extended
        && a.early_stops_at_boot == b.early_stops_at_boot
        && a.cmd_rejected_at_boot == b.cmd_rejected_at_boot
        && a.last_move == b.last_move
}

/// Failsafe state of a valve for the API and MQTT: Blocked when the STM reports it at its
/// failsafe position because it is blocked (protocol 3), Lease when it is at its lease failsafe
/// position (STM flag, or the ESP emulation overrides its target), else None.
pub fn failsafe_kind(v: &ValveState) -> FailsafeKind {
    if v.has_v3 && (v.stm_flags & STM_FLAG_FS_BLOCKED) != 0 {
        return FailsafeKind::Blocked;
    }
    if v.fs_override || (v.has_v3 && (v.stm_flags & STM_FLAG_FS_LEASE) != 0) {
        return FailsafeKind::Lease;
    }
    FailsafeKind::None
}

/// `failsafe_kind(v) == FailsafeKind::Lease`.
pub fn valve_at_failsafe(v: &ValveState) -> bool {
    failsafe_kind(v) == FailsafeKind::Lease
}

/// Calibration stroke close to the minimum: min_counts, open_count and close_count known (> 0)
/// and min(open_count, close_count) < 1.2 x min_counts.
pub fn stroke_near_minimum(open_count: u32, close_count: u32, min_counts: u16) -> bool {
    if min_counts == 0 || open_count == 0 || close_count == 0 {
        return false;
    }
    let stroke = u64::from(open_count.min(close_count));
    stroke * 5 < u64::from(min_counts) * 6
}

fn temp_failed(raw: i16) -> bool {
    raw != TEMP_UNASSIGNED && !temp_raw_valid(raw)
}

fn addressed(valve_or_all: u8, i: usize) -> bool {
    valve_or_all == ALL_VALVES || usize::from(valve_or_all) == i
}

/// gvlvd/gvlvx carry no protocol 3 fields; fs_pct stays (ESP config on 1/2).
fn clear_v3(v: &mut ValveState) {
    v.has_v3 = false;
    v.stm_flags = 0;
    v.fault = 0;
    v.drive = 0;
    v.retry_s = 0;
    v.retries = 0;
    v.auto_retry = false;
}

/// The value pushed to and verified on the STM: fs_target under the failsafe override, else
/// desired.
fn pushed(v: &ValveState) -> u8 {
    if v.fs_override {
        v.fs_target
    } else {
        v.desired
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ValveModelParams {
    /// active valve without data -> HEALTH_STALE
    pub stale_ms: u32,
    /// stgtp attempts before TargetSync::Failed
    pub max_push_attempts: u8,
    /// min spacing between stgtp to one valve
    pub push_retry_ms: u32,
    /// Failed -> Pending again after 5 min
    pub failed_retry_ms: u32,
}

impl Default for ValveModelParams {
    fn default() -> Self {
        Self {
            stale_ms: 60_000,
            max_push_attempts: 5,
            push_retry_ms: 2000,
            failed_retry_ms: 300_000,
        }
    }
}

/// Per-valve bookkeeping of the model that is not part of [`ValveState`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Track {
    /// last gvlvd/gvlvx, or first tick while active
    stale_ref_ms: u32,
    stale_ref_valid: bool,
    /// latched until the next valve data
    stale: bool,
    /// false: the next gvlvx sets the v2 counter baselines
    baselined: bool,
    /// last_push_ms is meaningful
    pushed_once: bool,
    /// A Failed delivery that is being retried keeps HEALTH_TARGET_UNCONFIRMED until a
    /// read-back confirms it, so a stuck valve does not flap the flag.
    keep_unconfirmed: bool,
    /// AwaitAck waits for a staop result
    assembly_pending: bool,
    /// restored target: no push before a read-back
    read_back_first: bool,
}

fn mark_synced(v: &mut ValveState, t: &mut Track) {
    v.sync = TargetSync::Synced;
    v.push_attempts = 0;
    t.keep_unconfirmed = false;
}

static EMPTY_VALVE: ValveState = ValveState::EMPTY;

#[derive(Clone, Debug)]
pub struct ValveModel {
    params: ValveModelParams,
    v: [ValveState; N],
    t: [Track; N],
    active: u16,
    push_cursor: usize,
    hold_while_calibrating: bool,
    assembly_via_staop: bool,
    min_counts: u16,
    desired_rev: u32,
}

impl Default for ValveModel {
    fn default() -> Self {
        Self::new(ValveModelParams::default())
    }
}

impl ValveModel {
    pub fn new(params: ValveModelParams) -> Self {
        Self {
            params,
            v: [ValveState::EMPTY; N],
            t: [Track::default(); N],
            active: 0,
            push_cursor: 0,
            hold_while_calibrating: true,
            assembly_via_staop: false,
            min_counts: 0,
            desired_rev: 0,
        }
    }

    fn is_active(&self, i: usize) -> bool {
        (self.active >> i) & 1 != 0
    }

    /// Recomputes the health of valve `i` and counts the revisions (C++ `commit`).
    fn commit(&mut self, i: usize, before: &ValveState) {
        self.update_health(i);
        let v = &mut self.v[i];
        if before.desired_valid != v.desired_valid
            || before.desired != v.desired
            || before.source != v.source
        {
            self.desired_rev = self.desired_rev.wrapping_add(1);
        }
        if !same_state(before, v) {
            v.revision = v.revision.wrapping_add(1);
        }
    }

    /// The flags are added in increasing bit order, so `+` is the `|` of the C++.
    fn update_health(&mut self, i: usize) {
        let active = self.is_active(i);
        let t = self.t[i];
        let min_counts = self.min_counts;
        let v = &mut self.v[i];
        let mut h = 0;
        if v.status == STATUS_BLOCKED {
            h += HEALTH_BLOCKED;
        }
        if v.status == STATUS_FAILED {
            h += HEALTH_FAILED;
        }
        if active {
            if v.status == STATUS_NO_VALVE {
                h += HEALTH_NO_VALVE;
            }
            if v.calib_retries > 0 {
                h += HEALTH_CALIB_RETRIES;
            }
            if v.has_extended && v.early_stops > v.early_stops_at_boot {
                h += HEALTH_EARLY_STOP;
            }
            if v.has_extended && v.cmd_rejected > v.cmd_rejected_at_boot {
                h += HEALTH_CMD_REJECTED;
            }
            if t.stale {
                h += HEALTH_STALE;
            }
            if v.sync == TargetSync::Failed || (t.keep_unconfirmed && v.sync != TargetSync::Synced)
            {
                h += HEALTH_TARGET_UNCONFIRMED;
            }
            if temp_failed(v.temp1) || temp_failed(v.temp2) {
                h += HEALTH_TEMP_FAILED;
            }
            if valve_at_failsafe(v) {
                h += HEALTH_FAILSAFE;
            }
            if stroke_near_minimum(v.open_count, v.close_count, min_counts) {
                h += HEALTH_STROKE_SHORT;
            }
        }
        v.health = h;
    }

    /// Active valves (config). Inactive valves keep their data but never get targets and never
    /// raise health flags other than Blocked/Failed. Bits above valve 11 are ignored.
    pub fn set_active_mask(&mut self, mask: u16) {
        for i in 0..N {
            let now = (mask >> i) & 1 != 0;
            if self.is_active(i) == now {
                continue;
            }
            let before = self.v[i];
            // The bit differs: toggling sets it to `now`. Staleness of a newly activated valve
            // is measured from the next tick.
            self.active ^= 1 << i;
            self.t[i].stale_ref_valid = false;
            self.t[i].stale = false;
            self.commit(i, &before);
        }
    }

    pub fn active_mask(&self) -> u16 {
        self.active
    }

    /// User/MQTT command. Returns false (no change) for valve >= 12, pos > 100 or an inactive
    /// valve. Same value as desired -> true without a re-push, except that a Failed delivery is
    /// re-armed (Pending, attempts reset). A new value equal to the known STM target (no stgtp
    /// in flight) is Synced at once; otherwise it goes Pending.
    pub fn set_desired_target(
        &mut self,
        valve: u8,
        pos: u8,
        src: TargetSource,
        _now_ms: u32,
    ) -> bool {
        let i = usize::from(valve);
        if i >= N || pos > 100 || !self.is_active(i) {
            return false;
        }
        let before = self.v[i];
        // The same value from a web/MQTT command ends an assembly: the stgtp ends the STM's
        // assembly hold.
        let ends_assembly =
            before.source == TargetSource::Assembly && src != TargetSource::Assembly;
        let (v, t) = (&mut self.v[i], &mut self.t[i]);
        if before.desired_valid && before.desired == pos && !ends_assembly {
            // Same value: only a Failed delivery is re-armed (explicit retry).
            if before.sync == TargetSync::Failed {
                v.source = src;
                v.sync = TargetSync::Pending;
                v.push_attempts = 0;
                t.keep_unconfirmed = false;
            }
        } else {
            let pushed_before = pushed(v);
            let in_flight = matches!(v.sync, TargetSync::AwaitAck | TargetSync::AwaitVerify);
            v.desired_valid = true;
            v.desired = pos;
            v.source = src;
            t.assembly_pending = false;
            t.read_back_first = false;
            if ends_assembly {
                v.force_push = true;
            }
            // Under the failsafe override the STM keeps getting fs_target: the new desired
            // value waits without touching the delivery.
            if !(v.fs_override && before.desired_valid && pushed(v) == pushed_before) {
                v.push_attempts = 0;
                t.keep_unconfirmed = false;
                v.sync = if !in_flight
                    && !v.force_push
                    && v.stm_target_known
                    && v.stm_target == pushed(v)
                {
                    TargetSync::Synced
                } else {
                    TargetSync::Pending
                };
            }
        }
        self.commit(i, &before);
        true
    }

    fn mark_seen(&mut self, i: usize, now_ms: u32) {
        self.v[i].known = true;
        self.v[i].last_seen_ms = now_ms;
        let t = &mut self.t[i];
        t.stale_ref_ms = now_ms;
        t.stale_ref_valid = true;
        t.stale = false;
    }

    /// gvlvd reply. An out-of-range valve is ignored (the codec already validated). Updates
    /// health and revision.
    pub fn apply_valve_data(&mut self, d: &ValveData, now_ms: u32) {
        let i = usize::from(d.valve);
        let Some(&before) = self.v.get(i) else {
            return;
        };
        self.mark_seen(i, now_ms);
        let v = &mut self.v[i];
        v.status = d.status;
        v.calibrating = d.calibrating;
        v.position = d.position;
        v.mean_current = d.mean_current;
        v.temp1 = d.temp1;
        v.temp2 = d.temp2;
        v.moves = d.moves;
        v.open_count = d.open_count;
        v.close_count = d.close_count;
        v.dead_zone = d.dead_zone;
        v.calib_retries = d.calib_retries;
        clear_v3(v);
        self.commit(i, &before);
    }

    /// gvlvx/gvlvy reply, also a target read-back. Out-of-range valve or target: ignored.
    pub fn apply_valve_ex(&mut self, d: &ValveEx, now_ms: u32) {
        let i = usize::from(d.valve);
        let Some(&before) = self.v.get(i) else {
            return;
        };
        if d.target > 100 {
            return;
        }
        self.mark_seen(i, now_ms);
        let (v, t) = (&mut self.v[i], &mut self.t[i]);
        v.status = d.status;
        v.calibrating = d.calibrating;
        v.position = d.position;
        v.mean_current = d.mean_current;
        v.open_count = d.open_count;
        v.close_count = d.close_count;
        v.dead_zone = d.dead_zone;
        v.calib_retries = d.calib_retries;
        v.moves = d.moves;
        v.has_extended = true;
        v.cal_state = d.cal_state;
        v.cal_flags = d.cal_flags;
        v.early_stops = d.early_stops;
        v.cmd_rejected = d.cmd_rejected;
        // A counter below its baseline means an STM reboot went unnoticed.
        if !t.baselined
            || d.early_stops < v.early_stops_at_boot
            || d.cmd_rejected < v.cmd_rejected_at_boot
        {
            v.early_stops_at_boot = d.early_stops;
            v.cmd_rejected_at_boot = d.cmd_rejected;
            t.baselined = true;
        }
        if v.last_move != d.last_move {
            v.last_move = d.last_move;
            v.move_seq = v.move_seq.wrapping_add(1);
        }
        if d.v3 {
            v.has_v3 = true;
            v.stm_flags = d.flags;
            v.fault = d.fault;
            v.fs_pct = d.fs_pct;
            v.drive = d.drive;
            v.retry_s = d.retry_s;
            // More automatic retries than before: the next calibration start is one.
            if d.retries > v.retries {
                v.auto_retry = true;
            }
            v.retries = d.retries;
            if d.retries == 0 || (before.calibrating && !d.calibrating) {
                v.auto_retry = false;
            }
        } else {
            clear_v3(v);
        }
        let hold_ok =
            !(d.v3 && v.source == TargetSource::Assembly && (d.flags & STM_FLAG_ASSEMBLY) == 0);
        self.apply_read_back(i, d.target, hold_ok);
        self.commit(i, &before);
    }

    /// gvlst reply: only fills the status of valves that are not known yet (no timestamp: it
    /// is no proof of fresh data).
    pub fn apply_valve_states(&mut self, s: &ValveStates, _now_ms: u32) {
        for (i, &status) in s.status.iter().enumerate() {
            let before = self.v[i];
            if before.known {
                continue;
            }
            self.v[i].known = true;
            self.v[i].status = status & 0x7F;
            self.commit(i, &before);
        }
    }

    /// gtgtp read-back. If no desired target exists yet (ESP boot), the STM target is adopted
    /// as desired with source Stm -> Synced. Out-of-range valve or target: ignored.
    pub fn apply_target(&mut self, t: &TargetReply, _now_ms: u32) {
        let i = usize::from(t.valve);
        let Some(&before) = self.v.get(i) else {
            return;
        };
        if t.target > 100 {
            return;
        }
        self.apply_read_back(i, t.target, true);
        self.commit(i, &before);
    }

    /// `hold_ok` false: a protocol 3 read-back of an Assembly valve without the STM's Assembly
    /// flag (the hold is gone even when the target matches).
    fn apply_read_back(&mut self, i: usize, target: u8, hold_ok: bool) {
        let (v, t) = (&mut self.v[i], &mut self.t[i]);
        v.stm_target_known = true;
        v.stm_target = target;
        t.read_back_first = false;
        if !v.desired_valid {
            v.desired_valid = true;
            v.desired = target;
            v.source = TargetSource::Stm;
            mark_synced(v, t);
            return;
        }
        // A forced push (after an STM reboot) goes out although the value matches.
        let equal = target == pushed(v) && hold_ok && !v.force_push;
        match v.sync {
            // The read-back may predate the stgtp in flight; the ack decides.
            TargetSync::AwaitAck => {}
            TargetSync::Failed => {
                if equal {
                    mark_synced(v, t);
                }
            }
            TargetSync::Unknown
            | TargetSync::Synced
            | TargetSync::Pending
            | TargetSync::AwaitVerify => {
                if equal {
                    mark_synced(v, t);
                } else {
                    v.sync = TargetSync::Pending;
                }
            }
        }
    }

    /// gvlon reply; ids resolved to config slots with [`resolve_temp_slot`] against `slot_ids`
    /// (`slot_ids[i]` for slot i + 1; empty: no slot table, every slot 0).
    pub fn apply_valve_sensors(&mut self, s: &ValveSensors, slot_ids: &[OneWireId]) {
        let valves = if s.is_list {
            0..N
        } else {
            let i = usize::from(s.valve);
            if i >= N {
                return;
            }
            i..i + 1
        };
        for i in valves {
            let before = self.v[i];
            let ids = s.ids[i];
            self.v[i].sensor_id = ids;
            self.v[i].sensor_slot = ids.map(|id| resolve_temp_slot(&id, slot_ids));
            self.commit(i, &before);
        }
    }

    /// Protocol v2 polls gvlvx, which has no temperatures, instead of gvlvd: temp1/temp2 come
    /// from the gvlon assignment and the goned readings. Per sensor: zero id or a bad CRC ->
    /// TEMP_UNASSIGNED (nothing assigned); on the bus and fresh ([`SensorModel::temp_fresh`]) ->
    /// the raw reading (sentinels included); on the bus, read before but no longer fresh ->
    /// TEMP_READ_ERROR; never read or not on the bus -> TEMP_READ_ERROR once the sensors are
    /// `settled` (link up, re-sync and sensor grace over), else TEMP_UNASSIGNED.
    pub fn apply_sensor_temps(
        &mut self,
        sensors: &SensorModel,
        now_ms: u32,
        max_age_ms: u32,
        settled: bool,
    ) {
        for i in 0..N {
            let before = self.v[i];
            let raw = before.sensor_id.map(|id| {
                if is_zero(&id) || !crc_valid(&id) {
                    return TEMP_UNASSIGNED;
                }
                match sensors.find_temp(&id) {
                    Some(bus) if sensors.temp_fresh(bus, now_ms, max_age_ms) => {
                        sensors.temp(bus).raw
                    }
                    // read before, too old now
                    Some(bus) if sensors.temp(bus).seen => TEMP_READ_ERROR,
                    _ if settled => TEMP_READ_ERROR,
                    _ => TEMP_UNASSIGNED,
                }
            });
            if raw == [before.temp1, before.temp2] {
                continue;
            }
            [self.v[i].temp1, self.v[i].temp2] = raw;
            self.commit(i, &before);
        }
    }

    /// Whether a calibrating valve gets no stgtp (default true). STM 1.x acks a target during a
    /// calibration but drops it; protocol v2 takes it and ends the calibration at the newest
    /// target.
    pub fn set_hold_targets_while_calibrating(&mut self, hold: bool) {
        self.hold_while_calibrating = hold;
    }

    /// Shared selection of next_target_push/next_assembly_push.
    fn next_delivery(&mut self, now_ms: u32, assembly: bool) -> Option<u8> {
        let cursor = self.push_cursor;
        for i in (cursor..N).chain(0..cursor) {
            let (v, t) = (&self.v[i], &self.t[i]);
            if !self.is_active(i) || !(v.known || v.stm_target_known) || !v.desired_valid {
                continue;
            }
            if t.read_back_first {
                continue;
            }
            let via_staop = self.assembly_via_staop && v.source == TargetSource::Assembly;
            if via_staop != assembly {
                continue;
            }
            if v.calibrating && self.hold_while_calibrating {
                continue;
            }
            let since_push = elapsed_ms(now_ms, v.last_push_ms);
            let rearm = v.sync == TargetSync::Failed;
            if rearm {
                if since_push < self.params.failed_retry_ms {
                    continue;
                }
            } else if v.sync != TargetSync::Pending {
                continue;
            }
            if t.pushed_once && since_push < self.params.push_retry_ms {
                continue;
            }
            let before = *v;
            let (v, t) = (&mut self.v[i], &mut self.t[i]);
            if rearm {
                v.push_attempts = 0;
                t.keep_unconfirmed = true;
            }
            v.sync = TargetSync::AwaitAck;
            v.push_attempts = v.push_attempts.saturating_add(1);
            v.last_push_ms = now_ms;
            v.stm_target_known = false; // uncertain until the read-back
            v.force_push = false;
            t.assembly_pending = assembly;
            t.pushed_once = true;
            self.push_cursor = (i + 1) % N;
            self.commit(i, &before);
            return u8::try_from(i).ok();
        }
        None
    }

    /// Target delivery driven by the stm_link task: the next stgtp to send, `(valve, pos)`. A
    /// valve in Pending (or Failed past failed_retry_ms), active, known (valve data or a target
    /// read-back), not calibrating (only while held, see
    /// [`set_hold_targets_while_calibrating`](Self::set_hold_targets_while_calibrating)), not
    /// waiting for the first read-back of a restored target, push_retry_ms since the last push.
    /// Round robin over valves. Moves that valve to AwaitAck and counts the attempt; `pos` is
    /// [`push_target`](Self::push_target)`(valve)`. A force_push valve is pushed although its
    /// read-back equals the target (once, after an STM reboot).
    pub fn next_target_push(&mut self, now_ms: u32) -> Option<(u8, u8)> {
        let valve = self.next_delivery(now_ms, false)?;
        Some((valve, self.push_target(valve)))
    }

    /// The stgtp handed out by next_target_push() could not be queued: back to Pending, the
    /// attempt is not counted; the next try waits push_retry_ms.
    pub fn on_target_push_dropped(&mut self, valve: u8, _now_ms: u32) {
        let i = usize::from(valve);
        let Some(&before) = self.v.get(i) else {
            return;
        };
        if before.sync != TargetSync::AwaitAck {
            return;
        }
        let v = &mut self.v[i];
        v.sync = TargetSync::Pending;
        v.push_attempts = v.push_attempts.saturating_sub(1);
        self.t[i].assembly_pending = false;
        self.commit(i, &before);
    }

    /// -> AwaitVerify (only from AwaitAck, and not while a staop result is awaited).
    pub fn on_target_ack(&mut self, valve: u8, _now_ms: u32) {
        let i = usize::from(valve);
        let Some(&before) = self.v.get(i) else {
            return;
        };
        // the staop result decides
        if before.sync != TargetSync::AwaitAck || self.t[i].assembly_pending {
            return;
        }
        self.v[i].sync = TargetSync::AwaitVerify;
        self.commit(i, &before);
    }

    /// -> Pending, or Failed after max_push_attempts (only from AwaitAck, and not while a staop
    /// result is awaited).
    pub fn on_target_timeout(&mut self, valve: u8, _now_ms: u32) {
        let i = usize::from(valve);
        let Some(&before) = self.v.get(i) else {
            return;
        };
        if before.sync != TargetSync::AwaitAck || self.t[i].assembly_pending {
            return;
        }
        self.v[i].sync = self.after_failed_attempt(before.push_attempts);
        self.commit(i, &before);
    }

    fn after_failed_attempt(&self, push_attempts: u8) -> TargetSync {
        if push_attempts >= self.params.max_push_attempts {
            TargetSync::Failed
        } else {
            TargetSync::Pending
        }
    }

    /// Valve whose read-back is due (AwaitVerify), the lowest one. The caller enqueues gtgtp
    /// (v1) or gvlvx (v2); the reply lands in apply_target/apply_valve_ex.
    pub fn next_verify(&self) -> Option<u8> {
        (0..VALVE_COUNT)
            .zip(&self.v)
            .find(|(_, v)| v.sync == TargetSync::AwaitVerify)
            .map(|(i, _)| i)
    }

    /// STM rebooted or was reset/re-flashed: every valve with a desired target goes to Pending
    /// with force_push (pushed once even when the read-back equals it), STM targets become
    /// unknown, v2 counters re-baseline. Valves without a desired target adopt the STM value
    /// again.
    pub fn on_stm_rebooted(&mut self, _now_ms: u32) {
        for i in 0..N {
            let before = self.v[i];
            let v = &mut self.v[i];
            v.stm_target_known = false;
            v.stm_target = 0;
            v.push_attempts = 0;
            v.sync = if v.desired_valid {
                TargetSync::Pending
            } else {
                TargetSync::Unknown
            };
            v.force_push = v.desired_valid;
            let t = &mut self.t[i];
            t.keep_unconfirmed = false;
            t.assembly_pending = false;
            t.baselined = false;
            self.commit(i, &before);
        }
    }

    /// Assembly (staop) was queued for `valve_or_all`: every addressed active valve gets desired
    /// 100, source Assembly, sync AwaitAck until the staop result (no stgtp is pushed: it would
    /// end the STM's assembly hold). The ESP failsafe emulation never overrides such a valve.
    pub fn set_assembly(&mut self, valve_or_all: u8, _now_ms: u32) {
        for i in 0..N {
            if !addressed(valve_or_all, i) || !self.is_active(i) {
                continue;
            }
            let before = self.v[i];
            let v = &mut self.v[i];
            v.desired_valid = true;
            v.desired = 100;
            v.source = TargetSource::Assembly;
            v.sync = TargetSync::AwaitAck;
            v.push_attempts = 0;
            v.stm_target_known = false;
            v.force_push = false;
            v.fs_override = false;
            v.fs_target = 0;
            let t = &mut self.t[i];
            t.keep_unconfirmed = false;
            t.read_back_first = false;
            t.assembly_pending = true;
            self.commit(i, &before);
        }
    }

    fn assembly_result(&mut self, valve_or_all: u8, ok: bool) {
        for i in 0..N {
            if !addressed(valve_or_all, i) || !self.t[i].assembly_pending {
                continue;
            }
            let before = self.v[i];
            self.t[i].assembly_pending = false;
            self.v[i].sync = if ok {
                TargetSync::AwaitVerify
            } else {
                self.after_failed_attempt(before.push_attempts)
            };
            self.commit(i, &before);
        }
    }

    /// The staop for `valve_or_all` was acknowledged: AwaitAck -> AwaitVerify.
    pub fn on_assembly_ack(&mut self, valve_or_all: u8, _now_ms: u32) {
        self.assembly_result(valve_or_all, true);
    }

    /// No answer / rejected: Pending (a stgtp 100, or a staop again, follows), Failed after
    /// max_push_attempts like a target push.
    pub fn on_assembly_failed(&mut self, valve_or_all: u8, _now_ms: u32) {
        self.assembly_result(valve_or_all, false);
    }

    /// Protocol >= 2: a Pending valve with source Assembly is delivered with a staop
    /// ([`next_assembly_push`](Self::next_assembly_push)) instead of stgtp 100, so the STM keeps
    /// its assembly hold; a protocol 3 read-back counts only with the Assembly flag. Off
    /// (default): stgtp 100.
    pub fn set_assembly_via_staop(&mut self, on: bool) {
        self.assembly_via_staop = on;
    }

    /// Like next_target_push for the staop deliveries; the valve waits for the staop result
    /// (on_assembly_ack/on_assembly_failed).
    pub fn next_assembly_push(&mut self, now_ms: u32) -> Option<u8> {
        self.next_delivery(now_ms, true)
    }

    /// Desired target restored at ESP boot (RTC/NVS copy). Active valves only; source Restored
    /// (Assembly stays Assembly); sync Pending and no push before the first read-back (equal ->
    /// Synced without a push). False for an invalid valve/pos or an inactive valve.
    pub fn restore_desired(&mut self, valve: u8, pos: u8, src: TargetSource) -> bool {
        let i = usize::from(valve);
        if i >= N || pos > 100 || !self.is_active(i) {
            return false;
        }
        let before = self.v[i];
        let v = &mut self.v[i];
        v.desired_valid = true;
        v.desired = pos;
        v.source = if src == TargetSource::Assembly {
            TargetSource::Assembly
        } else {
            TargetSource::Restored
        };
        v.sync = TargetSync::Pending;
        v.push_attempts = 0;
        let t = &mut self.t[i];
        t.keep_unconfirmed = false;
        t.read_back_first = true;
        self.commit(i, &before);
        true
    }

    /// +1 whenever desired_valid/desired/source of any valve changes.
    pub fn desired_revision(&self) -> u32 {
        self.desired_rev
    }

    /// ESP failsafe emulation (protocols 1/2): for valves in `mask` that are active, have a
    /// desired target, are not in assembly and have pct <= 100, the value pushed to and
    /// verified on the STM is `pct[v]` instead of desired (fs_override); desired and source
    /// never change. A valve whose pushed value changes goes Pending (attempts 0), or Synced
    /// when the known STM target already equals it. mask 0 ends every override. Valves without
    /// protocol 3 data take fs_pct from pct.
    pub fn set_failsafe_drive(&mut self, mask: u16, pct: &[u8; N]) {
        for (i, &p) in pct.iter().enumerate() {
            let before = self.v[i];
            let on = (mask >> i) & 1 != 0
                && self.is_active(i)
                && before.desired_valid
                && before.source != TargetSource::Assembly
                && p <= 100;
            let v = &mut self.v[i];
            v.fs_override = on;
            v.fs_target = if on { p } else { 0 };
            if !v.has_v3 {
                v.fs_pct = p;
            }
            let now_pushed = pushed(v);
            if v.desired_valid && now_pushed != pushed(&before) {
                v.push_attempts = 0;
                v.sync = if v.stm_target_known && v.stm_target == now_pushed {
                    TargetSync::Synced
                } else {
                    TargetSync::Pending
                };
                self.t[i].keep_unconfirmed = false;
            }
            self.commit(i, &before);
        }
    }

    /// fs_override ? fs_target : desired; 0 out of range.
    pub fn push_target(&self, valve: u8) -> u8 {
        self.v.get(usize::from(valve)).map_or(0, pushed)
    }

    /// From gmotc; 0 = unknown (no stroke check).
    pub fn set_min_counts(&mut self, min_counts: u16) {
        self.min_counts = min_counts;
        for i in 0..N {
            let before = self.v[i];
            self.commit(i, &before);
        }
    }

    pub fn min_counts(&self) -> u16 {
        self.min_counts
    }

    /// STM firmware unsupported: every valve known = false, STM data back to defaults; desired
    /// targets, sources and the failsafe override stay.
    pub fn forget_stm_data(&mut self) {
        for i in 0..N {
            let before = self.v[i];
            self.v[i] = ValveState {
                desired_valid: before.desired_valid,
                desired: before.desired,
                source: before.source,
                sync: if before.desired_valid {
                    TargetSync::Pending
                } else {
                    TargetSync::Unknown
                },
                fs_override: before.fs_override,
                fs_target: before.fs_target,
                fs_pct: before.fs_pct,
                move_seq: before.move_seq,
                revision: before.revision,
                ..ValveState::EMPTY
            };
            let t = &mut self.t[i];
            t.stale = false;
            t.stale_ref_valid = false;
            t.baselined = false;
            t.assembly_pending = false;
            t.keep_unconfirmed = false;
            self.commit(i, &before);
        }
    }

    /// Staleness evaluation; call about once per second.
    pub fn tick(&mut self, now_ms: u32) {
        for i in 0..N {
            if !self.is_active(i) {
                continue;
            }
            let before = self.v[i];
            let t = &mut self.t[i];
            if !t.stale_ref_valid {
                t.stale_ref_valid = true;
                t.stale_ref_ms = now_ms;
            }
            // Latched: a counter wrap after 49 days of silence cannot clear it.
            if elapsed_ms(now_ms, t.stale_ref_ms) >= self.params.stale_ms {
                t.stale = true;
            }
            self.commit(i, &before);
        }
    }

    /// i >= 12 returns a static empty state.
    pub fn valve(&self, i: u8) -> &ValveState {
        self.v.get(usize::from(i)).unwrap_or(&EMPTY_VALVE)
    }

    pub fn any_calibrating(&self) -> bool {
        self.v.iter().any(|v| v.calibrating)
    }

    /// Valve needs fast polling: moving (opening/closing), calibrating, or a delivery in
    /// progress (Pending, AwaitAck, AwaitVerify). A Failed delivery waits failed_retry_ms and
    /// does not need fast polling.
    pub fn is_busy(&self, i: u8) -> bool {
        let v = self.valve(i); // out of range: the empty state, never busy
        v.calibrating
            || v.status == STATUS_OPENING
            || v.status == STATUS_CLOSING
            || matches!(
                v.sync,
                TargetSync::Pending | TargetSync::AwaitAck | TargetSync::AwaitVerify
            )
    }
}

// ---------------------------------------------------------------- sensors

/// Consecutive failed readings (goned 0, a sentinel or out-of-range value) before a reading is
/// failed; a single failure keeps the previous reading.
pub const SENSOR_FAIL_DEBOUNCE: u8 = 2;
/// gstax tempAgeS above this: every temperature reading is stale (60 s STM refresh + 120 s move
/// timeout + margin).
pub const STM_TEMP_MAX_AGE_S: u32 = 200;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TempReading {
    /// from goned (authoritative) or gonec list
    pub id: OneWireId,
    pub raw: i16,
    /// at least one goned for this bus index
    pub seen: bool,
    pub last_seen_ms: u32,
    /// consecutive failed readings (saturating)
    pub fail_streak: u8,
}

impl TempReading {
    /// No sensor known at the bus index (C++ `TempReading{}`).
    pub const EMPTY: Self = Self {
        id: OneWireId { b: [0; 8] },
        raw: TEMP_UNASSIGNED,
        seen: false,
        last_seen_ms: 0,
        fail_streak: 0,
    };
}

impl Default for TempReading {
    fn default() -> Self {
        Self::EMPTY
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VoltReading {
    pub id: OneWireId,
    /// 10 mV
    pub vad: i32,
    pub seen: bool,
    pub last_seen_ms: u32,
    /// consecutive failed readings (saturating)
    pub fail_streak: u8,
}

impl VoltReading {
    /// No sensor known at the bus index (C++ `VoltReading{}`).
    pub const EMPTY: Self = Self {
        id: OneWireId { b: [0; 8] },
        vad: VAD_FAILED,
        seen: false,
        last_seen_ms: 0,
        fail_streak: 0,
    };
}

impl Default for VoltReading {
    fn default() -> Self {
        Self::EMPTY
    }
}

/// Raw temperature is a real reading (not -500, -1270 or 850) and within -550..1250 (DS18B20
/// range).
pub fn temp_raw_valid(raw: i16) -> bool {
    !matches!(raw, TEMP_UNASSIGNED | TEMP_READ_ERROR | TEMP_POWER_ON)
        && (-550..=1250).contains(&raw)
}

/// vad > VAD_FAILED.
pub fn vad_valid(vad: i32) -> bool {
    vad > VAD_FAILED
}

/// A reading at one bus index, for the list logic both kinds share (the C++ template).
trait BusReading: Copy {
    const EMPTY: Self;
    fn id(&self) -> &OneWireId;
    fn set_id(&mut self, id: OneWireId);
}

impl BusReading for TempReading {
    const EMPTY: Self = TempReading::EMPTY;
    fn id(&self) -> &OneWireId {
        &self.id
    }
    fn set_id(&mut self, id: OneWireId) {
        self.id = id;
    }
}

impl BusReading for VoltReading {
    const EMPTY: Self = VoltReading::EMPTY;
    fn id(&self) -> &OneWireId {
        &self.id
    }
    fn set_id(&mut self, id: OneWireId) {
        self.id = id;
    }
}

/// Shared list logic of apply_temp_list/apply_volt_list: the count is clamped to the bus maximum
/// (the length of `readings`).
fn apply_list<R: BusReading>(readings: &mut [R], count: &mut u8, l: &OneWireList) -> bool {
    let n = l.count.min(u8::try_from(readings.len()).unwrap_or(u8::MAX));
    let changed = n != *count;
    let (listed, beyond) = readings.split_at_mut(usize::from(n));
    beyond.fill(R::EMPTY);
    *count = n;
    if l.has_list {
        for (r, id) in listed.iter_mut().zip(&l.ids) {
            if r.id() != id {
                // another sensor now sits at this bus index
                *r = R::EMPTY;
                r.set_id(*id);
            }
        }
    }
    changed
}

/// Bus index of the sensor with this id among the first `count` readings.
fn find_reading<R: BusReading>(readings: &[R], count: u8, id: &OneWireId) -> Option<u8> {
    if is_zero(id) {
        return None;
    }
    (0..count)
        .zip(readings)
        .find(|(_, r)| r.id() == id)
        .map(|(i, _)| i)
}

static EMPTY_TEMP: TempReading = TempReading::EMPTY;
static EMPTY_VOLT: VoltReading = VoltReading::EMPTY;

/// Bus-indexed sensor readings (index = STM bus index, NOT config slot). Config slots are
/// resolved by id at render time, so a sensor keeps its slot when the bus order changes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SensorModel {
    temps: [TempReading; TEMP_SLOT_COUNT as usize],
    volts: [VoltReading; VOLT_SLOT_COUNT as usize],
    temp_count: u8,
    volt_count: u8,
    have_stm_age: bool,
    stm_age_s: u32,
    stm_age_at_ms: u32,
}

impl Default for SensorModel {
    fn default() -> Self {
        Self {
            temps: [TempReading::EMPTY; TEMP_SLOT_COUNT as usize],
            volts: [VoltReading::EMPTY; VOLT_SLOT_COUNT as usize],
            temp_count: 0,
            volt_count: 0,
            have_stm_age: false,
            stm_age_s: 0,
            stm_age_at_ms: 0,
        }
    }
}

impl SensorModel {
    /// gonec. A count change clears readings beyond the new count and returns true (caller
    /// requests the id list). A list reply replaces ids.
    pub fn apply_temp_list(&mut self, l: &OneWireList, _now_ms: u32) -> bool {
        apply_list(&mut self.temps, &mut self.temp_count, l)
    }

    /// gowvc, like [`apply_temp_list`](Self::apply_temp_list).
    pub fn apply_volt_list(&mut self, l: &OneWireList, _now_ms: u32) -> bool {
        apply_list(&mut self.volts, &mut self.volt_count, l)
    }

    /// goned for the bus index that was requested (the reply carries no index). A good reading
    /// (valid form, [`temp_raw_valid`]) stores it and resets fail_streak. A failed one (invalid
    /// form "goned 0", sentinel or out of range) increments fail_streak (saturating); below
    /// [`SENSOR_FAIL_DEBOUNCE`] the previous reading is kept, from then on it is applied: the
    /// invalid form marks the index as not seen, a sentinel is stored as the reading.
    pub fn apply_temp_data(&mut self, bus_index: u8, d: &TempData, now_ms: u32) {
        let Some(r) = self.temps.get_mut(usize::from(bus_index)) else {
            return;
        };
        if d.valid && temp_raw_valid(d.value) {
            r.fail_streak = 0;
        } else {
            r.fail_streak = r.fail_streak.saturating_add(1);
            if r.fail_streak < SENSOR_FAIL_DEBOUNCE {
                return; // one failure is held
            }
            if !d.valid {
                r.seen = false;
                r.raw = TEMP_UNASSIGNED;
                return;
            }
        }
        r.id = d.id;
        r.raw = d.value;
        r.seen = true;
        r.last_seen_ms = now_ms;
    }

    /// gowvd, like [`apply_temp_data`](Self::apply_temp_data) with [`vad_valid`].
    pub fn apply_volt_data(&mut self, bus_index: u8, d: &VoltData, now_ms: u32) {
        let Some(r) = self.volts.get_mut(usize::from(bus_index)) else {
            return;
        };
        if d.valid && vad_valid(d.vad) {
            r.fail_streak = 0;
        } else {
            r.fail_streak = r.fail_streak.saturating_add(1);
            if r.fail_streak < SENSOR_FAIL_DEBOUNCE {
                return; // one failure is held
            }
            if !d.valid {
                r.seen = false;
                r.vad = VAD_FAILED;
                return;
            }
        }
        r.id = d.id;
        r.vad = d.vad;
        r.seen = true;
        r.last_seen_ms = now_ms;
    }

    /// A late reply that answered no request: applied to the bus index of its id (true), false
    /// when the id is not on the list (nothing changes).
    pub fn apply_stray_temp_data(&mut self, d: &TempData, now_ms: u32) -> bool {
        let bus = if d.valid { self.find_temp(&d.id) } else { None };
        let Some(bus) = bus else {
            return false;
        };
        self.apply_temp_data(bus, d, now_ms);
        true
    }

    /// Like [`apply_stray_temp_data`](Self::apply_stray_temp_data) for gowvd.
    pub fn apply_stray_volt_data(&mut self, d: &VoltData, now_ms: u32) -> bool {
        let bus = if d.valid { self.find_volt(&d.id) } else { None };
        let Some(bus) = bus else {
            return false;
        };
        self.apply_volt_data(bus, d, now_ms);
        true
    }

    /// gstax tempAgeS (protocol 3): seconds since the STM's last complete temperature cycle,
    /// reported at `now_ms`.
    pub fn set_stm_temp_age(&mut self, age_s: u32, now_ms: u32) {
        self.have_stm_age = true;
        self.stm_age_s = age_s;
        self.stm_age_at_ms = now_ms;
    }

    /// After stons: forget everything until the next list.
    pub fn clear(&mut self) {
        self.temps.fill(TempReading::EMPTY);
        self.volts.fill(VoltReading::EMPTY);
        self.temp_count = 0;
        self.volt_count = 0;
    }

    pub fn temp_count(&self) -> u8 {
        self.temp_count
    }

    pub fn volt_count(&self) -> u8 {
        self.volt_count
    }

    /// Out of range -> a static empty reading.
    pub fn temp(&self, bus_index: u8) -> &TempReading {
        self.temps
            .get(usize::from(bus_index))
            .unwrap_or(&EMPTY_TEMP)
    }

    /// Out of range -> a static empty reading.
    pub fn volt(&self, bus_index: u8) -> &VoltReading {
        self.volts
            .get(usize::from(bus_index))
            .unwrap_or(&EMPTY_VOLT)
    }

    /// Bus index of the sensor with this id (C++ -1: None; a zero id is never found).
    pub fn find_temp(&self, id: &OneWireId) -> Option<u8> {
        find_reading(&self.temps, self.temp_count, id)
    }

    /// Bus index of the voltage sensor with this id.
    pub fn find_volt(&self, id: &OneWireId) -> Option<u8> {
        find_reading(&self.volts, self.volt_count, id)
    }

    /// Reading older than `max_age_ms` counts as stale for display/publishing; so does every
    /// reading while the STM's temperature age (reported age + whole seconds since the report)
    /// is above [`STM_TEMP_MAX_AGE_S`].
    pub fn temp_fresh(&self, bus_index: u8, now_ms: u32, max_age_ms: u32) -> bool {
        let Some(r) = self.temps.get(usize::from(bus_index)) else {
            return false;
        };
        if !r.seen || elapsed_ms(now_ms, r.last_seen_ms) > max_age_ms {
            return false;
        }
        !self.have_stm_age
            || u64::from(self.stm_age_s) + u64::from(elapsed_ms(now_ms, self.stm_age_at_ms) / 1000)
                <= u64::from(STM_TEMP_MAX_AGE_S)
    }
}

/// goned/gowvd request: sets `r.expect` to the id the SensorModel knows at bus index `r.arg`
/// (zero when unknown, then any id matches). Other commands are left alone. The index is the
/// low byte of `arg`, like the C++ `static_cast<uint8_t>`.
pub fn expect_sensor(r: &mut RequestLine, s: &SensorModel) {
    match r.cmd {
        Cmd::Goned => r.expect = s.temp(r.arg as u8).id,
        Cmd::Gowvd => r.expect = s.volt(r.arg as u8).id,
        _ => {}
    }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_link;
#[cfg(test)]
mod tests_mut;
