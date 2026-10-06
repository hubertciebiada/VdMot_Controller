//! Decides which STM request to issue next when no user/config request is waiting: re-sync
//! sequence, one-shot reads, and periodic polling with a per-category cadence (port of
//! `vdm/poll_planner.h`). Hardware-free; no queue of its own (the caller enqueues the returned
//! line at Priority::Poll, or Config for re-sync).

use crate::common::{elapsed_ms, Text, ALL_VALVES, TEMP_SLOT_COUNT, VALVE_COUNT, VOLT_SLOT_COUNT};
use crate::stm_codec::{
    build_get_breakaway, build_get_hw_id, build_get_learn_movements, build_get_motor_chars,
    build_get_proto, build_get_status, build_get_status_v3, build_get_target, build_get_version,
    build_match_sensors, build_profile, build_temp_count, build_temp_data, build_temp_list,
    build_valve_data, build_valve_ex, build_valve_ex_v3, build_valve_sensors, build_valve_states,
    build_volt_count, build_volt_data, build_volt_list, RequestLine,
};
use crate::version::{format_version, is_revamped, stm_support, StmSupport, Version};

/// Binding cadence (DESIGN.md "Poll cadence"). All values are the period of one item, e.g.
/// every active valve is read every `valve_active_ms`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PollCadence {
    /// valve moving/calibrating or target not synced
    pub valve_busy_ms: u16,
    /// active valve, idle
    pub valve_active_ms: u16,
    /// inactive valve (still shown on the dashboard)
    pub valve_inactive_ms: u16,
    /// each DS18 bus index (goned)
    pub temp_data_ms: u16,
    /// each DS2438 bus index (gowvd)
    pub volt_data_ms: u16,
    /// gonec / gowvc count check
    pub sensor_count_ms: u16,
    /// gstat (v2 only)
    pub status_ms: u16,
    /// gvers re-read (both protocols)
    pub version_ms: u32,
    /// gvers while the STM firmware is too old (the only poll)
    pub unsupported_version_ms: u16,
}

impl Default for PollCadence {
    fn default() -> Self {
        Self {
            valve_busy_ms: 500,
            valve_active_ms: 2000,
            valve_inactive_ms: 30000,
            temp_data_ms: 10000,
            volt_data_ms: 10000,
            sensor_count_ms: 30000,
            status_ms: 10000,
            version_ms: 300_000,
            unsupported_version_ms: 30000,
        }
    }
}

/// Re-sync steps in the order they are issued. v2-only steps are skipped on a v1 STM; Proto is
/// always first and its timeout means "v1". Proto and Version go out alone: nothing else is sent
/// before the version is known.
#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ResyncStep {
    /// gproto
    Proto = 0,
    /// gvers
    Version = 1,
    /// ghwin
    HwId = 2,
    /// gmotc
    MotorChars = 3,
    /// gtlnm
    LearnMovements = 4,
    /// gcalx (v2)
    Breakaway = 5,
    /// gonec 255
    TempList = 6,
    /// gowvc 255
    VoltList = 7,
    /// gvlon 255
    ValveSensors = 8,
    /// gvlst (v1 only; v2 uses gvlvx)
    ValveStates = 9,
    /// gtgtp 0..11 (v1) / gvlvx 0..11 (v2) / gvlvy 0..11 (v3): adopt STM targets
    Targets = 10,
    #[default]
    Done = 11,
}

impl ResyncStep {
    pub fn from_raw(v: u8) -> Option<Self> {
        [
            Self::Proto,
            Self::Version,
            Self::HwId,
            Self::MotorChars,
            Self::LearnMovements,
            Self::Breakaway,
            Self::TempList,
            Self::VoltList,
            Self::ValveSensors,
            Self::ValveStates,
            Self::Targets,
            Self::Done,
        ]
        .get(usize::from(v))
        .copied()
    }

    /// The step after this one; Done stays Done.
    fn following(self) -> Self {
        Self::from_raw(self as u8 + 1).unwrap_or(Self::Done)
    }
}

// One-shot items: bit i of `pending`/`inflight`.
const ITEM_TARGET: u8 = 0; // 0..11
const ITEM_PROFILE: u8 = ITEM_TARGET + VALVE_COUNT; // 12..23
const ITEM_TEMP_LIST: u8 = ITEM_PROFILE + VALVE_COUNT;
const ITEM_VOLT_LIST: u8 = ITEM_TEMP_LIST + 1;
const ITEM_VALVE_SENSORS: u8 = ITEM_VOLT_LIST + 1;
const ITEM_MOTOR_CHARS: u8 = ITEM_VALVE_SENSORS + 1;
const ITEM_LEARN_MOVEMENTS: u8 = ITEM_MOTOR_CHARS + 1;
const ITEM_BREAKAWAY: u8 = ITEM_LEARN_MOVEMENTS + 1;
/// 30: gproto (probe)
const ITEM_PROBE: u8 = ITEM_BREAKAWAY + 1;
/// 31
const ITEM_STATUS: u8 = ITEM_PROBE + 1;
/// 32: masns
const ITEM_MATCH_SENSORS: u8 = ITEM_STATUS + 1;
const ITEM_COUNT: u8 = ITEM_MATCH_SENSORS + 1;
/// The items the re-sync sequence reads anyway: the target read-backs (bits 0..11), the lists,
/// the motor parameters, the probe, the status and masns (bits 24..32).
const RESYNC_COVERED: u64 = 0x1_FF00_0FFF;
/// The v2-only items: the profiles (bits 12..23), the breakaway (29) and the status (31).
const V2_ITEMS: u64 = 0xA0FF_F000;

const fn bit(item: u8) -> u64 {
    1 << item
}

/// A hold makes an item ineligible until `hold_for` ms after `held_at` (C++
/// `PollPlanner::Hold`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct PollPlannerHold {
    held_at: u32,
    hold_for: u16,
}

impl PollPlannerHold {
    fn expired(&self, now_ms: u32) -> bool {
        elapsed_ms(now_ms, self.held_at) >= u32::from(self.hold_for)
    }

    fn set(&mut self, now_ms: u32, ms: u16) {
        self.held_at = now_ms;
        self.hold_for = ms;
    }
}

/// A periodic item of [`PollPlanner::next`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Periodic {
    Valve(u8),
    Status,
    Temp,
    Volt,
    TempCount,
    VoltCount,
    Version,
}

/// Same command, valve and argument.
fn same_request(a: &RequestLine, b: &RequestLine) -> bool {
    a.cmd == b.cmd && a.valve == b.valve && a.arg == b.arg
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PollPlanner {
    cadence: PollCadence,
    proto: u8,
    support: StmSupport,
    /// last gvers (format_version), "" none
    version_text: Text<31>,
    /// Valve i active / busy (C++ `activeMask_`, `busyMask_`: bits 0..11).
    active: [bool; VALVE_COUNT as usize],
    busy: [bool; VALVE_COUNT as usize],
    temp_count: u8,
    volt_count: u8,

    step: ResyncStep,
    /// Targets step: next valve
    step_valve: u8,
    step_inflight: bool,
    step_hold: PollPlannerHold,
    last_was_resync: bool,

    pending: u64,
    inflight: u64,
    item_hold: [PollPlannerHold; ITEM_COUNT as usize],

    primed: bool,
    valve_last_ms: [u32; VALVE_COUNT as usize],
    temp_index: u8,
    volt_index: u8,
    temp_last_ms: u32,
    volt_last_ms: u32,
    temp_count_last_ms: u32,
    volt_count_last_ms: u32,
    status_last_ms: u32,
    version_last_ms: u32,
}

impl Default for PollPlanner {
    fn default() -> Self {
        Self::new(PollCadence::default())
    }
}

impl PollPlanner {
    pub const SCAN_MATCH_DELAY_MS: u16 = 5000;
    pub const LOST_REQUEST_MS: u16 = 10000;

    pub fn new(cadence: PollCadence) -> Self {
        Self {
            cadence,
            proto: 0,
            support: StmSupport::Unknown,
            version_text: Text::new(),
            active: [false; VALVE_COUNT as usize],
            busy: [false; VALVE_COUNT as usize],
            temp_count: 0,
            volt_count: 0,
            step: ResyncStep::Done,
            step_valve: 0,
            step_inflight: false,
            step_hold: PollPlannerHold::default(),
            last_was_resync: false,
            pending: 0,
            inflight: 0,
            item_hold: [PollPlannerHold::default(); ITEM_COUNT as usize],
            primed: false,
            valve_last_ms: [0; VALVE_COUNT as usize],
            temp_index: 0,
            volt_index: 0,
            temp_last_ms: 0,
            volt_last_ms: 0,
            temp_count_last_ms: 0,
            volt_count_last_ms: 0,
            status_last_ms: 0,
            version_last_ms: 0,
        }
    }

    // ------------------------------------------------------------ inputs

    /// 0 = unknown (probe pending), 1, 2, 3 (higher values count as 3). Selects gvlvd / gvlvx /
    /// gvlvy and gstat / gstax, enables gcalx/gprof from 2; below 2 pending v2-only one-shots
    /// are dropped.
    pub fn set_protocol(&mut self, proto: u8) {
        self.proto = proto.min(3);
        if self.proto < 2 {
            self.pending &= !V2_ITEMS;
            self.inflight &= !V2_ITEMS;
        }
    }

    pub fn protocol(&self) -> u8 {
        self.proto
    }

    /// Bit i = valve i active.
    pub fn set_active_mask(&mut self, mask: u16) {
        for (v, a) in (0u16..).zip(self.active.iter_mut()) {
            *a = (mask >> v) & 1 != 0;
        }
    }

    /// From ValveModel; valves out of range are ignored.
    pub fn set_valve_busy(&mut self, valve: u8, busy: bool) {
        if let Some(b) = self.busy.get_mut(usize::from(valve)) {
            *b = busy;
        }
    }

    /// From gonec/gowvc (clamped to 34/8).
    pub fn set_sensor_counts(&mut self, temps: u8, volts: u8) {
        self.temp_count = temps.min(TEMP_SLOT_COUNT);
        self.volt_count = volts.min(VOLT_SLOT_COUNT);
        if self.temp_index >= self.temp_count {
            self.temp_index = 0;
        }
        if self.volt_index >= self.volt_count {
            self.volt_index = 0;
        }
    }

    /// Starts the full re-sync sequence (ESP boot, STM reboot detected, link recovered, after
    /// flashing). The Proto and Version steps go out alone; after them periodic polling of
    /// valves continues interleaved: when a re-sync step and another item are both due they
    /// alternate, so at most every second request is a re-sync step while other work is waiting
    /// (with nothing else due, steps go back to back). support() is Unknown until the next
    /// gvers. The protocol goes back to 0 (the STM may have been re-flashed; v1 commands work
    /// on both, so nothing v2-only is sent until `gproto` answers). Pending one-shots the
    /// sequence re-reads anyway (lists, motor params, targets) and v2-only ones (profiles) are
    /// dropped.
    pub fn request_resync(&mut self) {
        self.step = ResyncStep::Proto; // advance_step() zeroes step_valve before Targets
        self.step_inflight = false;
        self.step_hold = PollPlannerHold::default();
        self.last_was_resync = false;
        self.pending &= !RESYNC_COVERED;
        self.inflight &= !RESYNC_COVERED;
        self.support = StmSupport::Unknown;
        self.set_protocol(0);
    }

    pub fn resync_active(&self) -> bool {
        self.step != ResyncStep::Done
    }

    pub fn resync_step(&self) -> ResyncStep {
        self.step
    }

    /// True when the request last returned by next() was a re-sync step (the caller queues
    /// those at Priority::Config, everything else at Poll).
    pub fn last_was_resync(&self) -> bool {
        self.last_was_resync
    }

    fn set_pending(&mut self, item: u8) {
        // An unsupported STM gets gvers and target read-backs (gtgtp) only.
        if self.support == StmSupport::TooOld && item >= ITEM_PROFILE {
            return;
        }
        let b = bit(item);
        if self.pending & b != 0 {
            return; // coalesced
        }
        self.pending += b;
        self.inflight &= !b;
        self.item_hold[usize::from(item)] = PollPlannerHold::default();
    }

    // One-shot reads, each coalesced (a pending one is not queued twice). While support() is
    // TooOld every request_*() except request_target() is ignored.

    /// gonec 255 (count changed / after stons)
    pub fn request_temp_list(&mut self) {
        self.set_pending(ITEM_TEMP_LIST);
    }

    /// gowvc 255
    pub fn request_volt_list(&mut self) {
        self.set_pending(ITEM_VOLT_LIST);
    }

    /// gvlon 255 (after stvls+masns)
    pub fn request_valve_sensors(&mut self) {
        self.set_pending(ITEM_VALVE_SENSORS);
    }

    /// gmotc + gtlnm (+ gcalx on v2)
    pub fn request_motor_params(&mut self) {
        self.set_pending(ITEM_MOTOR_CHARS);
        self.set_pending(ITEM_LEARN_MOVEMENTS);
        if self.proto >= 2 {
            self.set_pending(ITEM_BREAKAWAY);
        }
    }

    /// gtgtp (v1) / gvlvx (v2) read-back
    pub fn request_target(&mut self, valve: u8) {
        if valve < VALVE_COUNT {
            self.set_pending(ITEM_TARGET + valve);
        }
    }

    /// gprof (v2 only; ignored on v1)
    pub fn request_profile(&mut self, valve: u8) {
        if valve < VALVE_COUNT && self.proto >= 2 {
            self.set_pending(ITEM_PROFILE + valve);
        }
    }

    /// gstax (3) / gstat (2); ignored on 0/1
    pub fn request_status(&mut self) {
        if self.proto >= 2 {
            self.set_pending(ITEM_STATUS);
        }
    }

    /// masns, not handed out before now_ms + SCAN_MATCH_DELAY_MS (a legacy STM blocks its main
    /// loop during the 1-Wire search after stons).
    pub fn request_match_sensors(&mut self, now_ms: u32) {
        if self.pending & bit(ITEM_MATCH_SENSORS) != 0 {
            return; // coalesced, keeps its delay
        }
        self.set_pending(ITEM_MATCH_SENSORS);
        self.item_hold[usize::from(ITEM_MATCH_SENSORS)].set(now_ms, Self::SCAN_MATCH_DELAY_MS);
    }

    // ------------------------------------------------------------ helpers

    fn valve_request(&self, valve: u8) -> Option<RequestLine> {
        if self.proto >= 3 {
            build_valve_ex_v3(valve)
        } else if self.proto == 2 {
            build_valve_ex(valve)
        } else {
            build_valve_data(valve)
        }
    }

    fn read_back_request(&self, valve: u8) -> Option<RequestLine> {
        if self.proto >= 3 {
            build_valve_ex_v3(valve)
        } else if self.proto == 2 {
            build_valve_ex(valve)
        } else {
            build_get_target(valve)
        }
    }

    fn status_request(&self) -> RequestLine {
        if self.proto >= 3 {
            build_get_status_v3()
        } else {
            build_get_status()
        }
    }

    fn build_item(&self, item: u8) -> Option<RequestLine> {
        if item < ITEM_PROFILE {
            return self.read_back_request(item); // ITEM_TARGET == 0: the item is the valve
        }
        if item < ITEM_TEMP_LIST {
            return build_profile(item - ITEM_PROFILE);
        }
        Some(match item {
            ITEM_TEMP_LIST => build_temp_list(),
            ITEM_VOLT_LIST => build_volt_list(),
            ITEM_VALVE_SENSORS => build_valve_sensors(ALL_VALVES)?,
            ITEM_MOTOR_CHARS => build_get_motor_chars(),
            ITEM_LEARN_MOVEMENTS => build_get_learn_movements(),
            ITEM_BREAKAWAY => build_get_breakaway(),
            ITEM_PROBE => build_get_proto(),
            ITEM_STATUS => self.status_request(),
            _ => build_match_sensors(), // ITEM_MATCH_SENSORS
        })
    }

    fn build_step(&self) -> Option<RequestLine> {
        Some(match self.step {
            ResyncStep::Proto => build_get_proto(),
            ResyncStep::Version => build_get_version(),
            ResyncStep::HwId => build_get_hw_id(),
            ResyncStep::MotorChars => build_get_motor_chars(),
            ResyncStep::LearnMovements => build_get_learn_movements(),
            ResyncStep::Breakaway => build_get_breakaway(),
            ResyncStep::TempList => build_temp_list(),
            ResyncStep::VoltList => build_volt_list(),
            ResyncStep::ValveSensors => build_valve_sensors(ALL_VALVES)?,
            ResyncStep::ValveStates => build_valve_states(),
            // Targets (never called for Done)
            ResyncStep::Targets | ResyncStep::Done => self.read_back_request(self.step_valve)?,
        })
    }

    fn advance_step(&mut self) {
        self.step_inflight = false;
        self.step_hold = PollPlannerHold::default();
        if self.step == ResyncStep::Targets {
            self.step_valve += 1;
            if self.step_valve < VALVE_COUNT {
                return;
            }
        }
        self.step = self.step.following();
        self.step_valve = 0;
    }

    fn skip_steps_for_protocol(&mut self) {
        while (self.step == ResyncStep::Breakaway && self.proto < 2)
            || (self.step == ResyncStep::ValveStates && self.proto >= 2)
        {
            self.advance_step();
        }
    }

    fn prime(&mut self, now_ms: u32) {
        self.primed = true;
        // Valves, gstat and sensor data are due at once; the re-sync sequence already reads the
        // counts/lists and the version.
        let inactive = now_ms.wrapping_sub(u32::from(self.cadence.valve_inactive_ms));
        self.valve_last_ms = [inactive; VALVE_COUNT as usize];
        self.status_last_ms = now_ms.wrapping_sub(u32::from(self.cadence.status_ms));
        self.temp_last_ms = now_ms.wrapping_sub(u32::from(self.cadence.temp_data_ms));
        self.volt_last_ms = now_ms.wrapping_sub(u32::from(self.cadence.volt_data_ms));
        self.temp_count_last_ms = now_ms;
        self.volt_count_last_ms = now_ms;
        self.version_last_ms = now_ms;
    }

    // ------------------------------------------------------------ next

    fn next_one_shot(&mut self, now_ms: u32) -> Option<RequestLine> {
        // Lowest item index first: that is the one-shot priority order.
        for i in 0..ITEM_COUNT {
            if self.pending & bit(i) == 0 || !self.item_hold[usize::from(i)].expired(now_ms) {
                continue;
            }
            let r = self.build_item(i); // cannot fail for an item index
            self.inflight |= bit(i);
            self.item_hold[usize::from(i)].set(now_ms, Self::LOST_REQUEST_MS);
            return r;
        }
        None
    }

    /// 0 busy, 1 active, 2 inactive; busy wins over the active flag.
    fn valve_class(&self, v: u8) -> u8 {
        let v = usize::from(v);
        if self.busy[v] {
            0
        } else if self.active[v] {
            1
        } else {
            2
        }
    }

    /// The valves of a class with the time of their last read.
    fn valves_of_class(&self, class: u8) -> impl Iterator<Item = (u8, u32)> + '_ {
        (0u8..)
            .zip(self.valve_last_ms)
            .filter(move |&(v, _)| self.valve_class(v) == class)
    }

    fn next_periodic(&mut self, now_ms: u32) -> Option<RequestLine> {
        if self.support == StmSupport::TooOld {
            // Only the version is polled, until an update makes the STM supported.
            if elapsed_ms(now_ms, self.version_last_ms)
                < u32::from(self.cadence.unsupported_version_ms)
            {
                return None;
            }
            self.version_last_ms = now_ms;
            return Some(build_get_version());
        }

        // The most overdue item and how much it is overdue.
        let mut best: Option<(Periodic, u32)> = None;
        // elapsed_ms() wraps after 49 days; for an item that was ineligible that long (no
        // sensors, v1 STM) this costs at most one period, never a stall.
        let mut consider = |id: Periodic, last: u32, period: u32| {
            let e = elapsed_ms(now_ms, last);
            // strict: ties keep the earlier
            if e >= period && best.is_none_or(|(_, overdue)| e - period > overdue) {
                best = Some((id, e - period));
            }
        };
        let c = self.cadence;
        // Candidates in tie-break order.
        for (v, last) in self.valves_of_class(0) {
            consider(Periodic::Valve(v), last, u32::from(c.valve_busy_ms));
        }
        for (v, last) in self.valves_of_class(1) {
            consider(Periodic::Valve(v), last, u32::from(c.valve_active_ms));
        }
        if self.proto >= 2 {
            consider(
                Periodic::Status,
                self.status_last_ms,
                u32::from(c.status_ms),
            );
        }
        if self.temp_count > 0 {
            let period = c.temp_data_ms / u16::from(self.temp_count);
            consider(Periodic::Temp, self.temp_last_ms, u32::from(period));
        }
        if self.volt_count > 0 {
            let period = c.volt_data_ms / u16::from(self.volt_count);
            consider(Periodic::Volt, self.volt_last_ms, u32::from(period));
        }
        let count_period = u32::from(c.sensor_count_ms);
        consider(Periodic::TempCount, self.temp_count_last_ms, count_period);
        consider(Periodic::VoltCount, self.volt_count_last_ms, count_period);
        for (v, last) in self.valves_of_class(2) {
            consider(Periodic::Valve(v), last, u32::from(c.valve_inactive_ms));
        }
        consider(Periodic::Version, self.version_last_ms, c.version_ms);

        match best?.0 {
            Periodic::Status => {
                self.status_last_ms = now_ms;
                Some(self.status_request())
            }
            Periodic::Temp => {
                self.temp_last_ms = now_ms;
                let r = build_temp_data(self.temp_index);
                self.temp_index = (self.temp_index + 1) % self.temp_count;
                r
            }
            Periodic::Volt => {
                self.volt_last_ms = now_ms;
                let r = build_volt_data(self.volt_index);
                self.volt_index = (self.volt_index + 1) % self.volt_count;
                r
            }
            Periodic::TempCount => {
                self.temp_count_last_ms = now_ms;
                Some(build_temp_count())
            }
            Periodic::VoltCount => {
                self.volt_count_last_ms = now_ms;
                Some(build_volt_count())
            }
            Periodic::Version => {
                self.version_last_ms = now_ms;
                Some(build_get_version())
            }
            Periodic::Valve(v) => {
                self.valve_last_ms[usize::from(v)] = now_ms;
                self.valve_request(v)
            }
        }
    }

    /// Produces the next request, or None when nothing is due. Priority: re-sync step > target
    /// read-backs > profiles > sensor/param one-shots > most overdue periodic item (busy
    /// valves, active valves, gstat, goned, gowvd, counts, inactive valves, gvers; ties in that
    /// order). Marks the item as handed out. A handed-out step or one-shot whose result never
    /// arrives (request evicted or dropped by an STM reset) is handed out again after
    /// LOST_REQUEST_MS.
    pub fn next(&mut self, now_ms: u32) -> Option<RequestLine> {
        if !self.primed {
            self.prime(now_ms);
        }

        self.skip_steps_for_protocol();
        let step_ready = self.step != ResyncStep::Done && self.step_hold.expired(now_ms);
        // Nothing else goes out before the protocol and the version are known.
        let exclusive = self.step == ResyncStep::Proto || self.step == ResyncStep::Version;
        if exclusive && !step_ready {
            return None;
        }
        if !exclusive && !(step_ready && !self.last_was_resync) {
            if let Some(r) = self.next_one_shot(now_ms) {
                self.last_was_resync = false;
                return Some(r);
            }
            if let Some(r) = self.next_periodic(now_ms) {
                self.last_was_resync = false;
                return Some(r);
            }
            if !step_ready {
                return None;
            }
        }
        let r = self.build_step(); // cannot fail while a step is active
        self.step_inflight = true;
        self.step_hold.set(now_ms, Self::LOST_REQUEST_MS);
        self.last_was_resync = true;
        r
    }

    // ------------------------------------------------------------ results

    /// A parsed gvers reply (re-sync step or periodic read):
    ///  - support() := stm_support(v). TooOld: the re-sync ends and every pending one-shot is
    ///    dropped; from then on next() hands out target read-backs (gtgtp) and gvers every
    ///    unsupported_version_ms only.
    ///  - Supported after TooOld, or a version text different from the last one while no
    ///    re-sync runs: request_resync().
    ///  - Protocol 1 with a revamped version (the probe may have hit the STM's start-up window):
    ///    a gproto probe one-shot; its success with protocol 2+ restarts the re-sync, a timeout
    ///    leaves protocol 1.
    pub fn on_version(&mut self, v: &Version) {
        let mut buf = [0u8; 32];
        let len = format_version(v, &mut buf);
        let text = buf.get(..len).unwrap_or_default();
        let changed = len > 0 && !self.version_text.is_empty() && text != &self.version_text[..];
        if len > 0 {
            self.version_text.clear();
            let _ = self.version_text.extend_from_slice(text);
        }
        let before = self.support;
        self.support = stm_support(v);
        if self.support == StmSupport::TooOld {
            self.step = ResyncStep::Done;
            self.step_inflight = false;
            self.last_was_resync = false;
            self.pending = 0;
            self.inflight = 0;
            return;
        }
        if (self.support == StmSupport::Supported && before == StmSupport::TooOld)
            || (changed && !self.resync_active())
        {
            self.request_resync();
            return;
        }
        // A revamped STM that answered no probe (it may have been starting up): probe again
        // with every gvers.
        if self.proto == 1 && is_revamped(v) {
            self.set_pending(ITEM_PROBE);
        }
    }

    pub fn support(&self) -> StmSupport {
        self.support
    }

    /// Result of a request produced by next(). Re-sync steps and one-shots are repeated until
    /// they succeed (a failed step is retried after the other due items, never more than once
    /// per valve_active_ms). A Proto timeout sets protocol 1 (the caller applies a `gproto`
    /// reply with set_protocol() before reporting it here). Periodic items are simply due again
    /// after their period. Results of requests the planner did not hand out are ignored.
    pub fn on_result(&mut self, request: &RequestLine, ok: bool, now_ms: u32) {
        if self.step_inflight
            && self
                .build_step()
                .is_some_and(|expected| same_request(&expected, request))
        {
            if ok {
                self.advance_step();
            } else if self.step == ResyncStep::Proto {
                if self.proto == 0 {
                    self.proto = 1; // silent STM: protocol v1
                }
                self.advance_step();
            } else {
                self.step_inflight = false;
                self.step_hold.set(now_ms, self.cadence.valve_active_ms);
            }
        }

        for i in 0..ITEM_COUNT {
            let b = bit(i);
            if self.inflight & b == 0
                || !self
                    .build_item(i)
                    .is_some_and(|expected| same_request(&expected, request))
            {
                continue;
            }
            self.inflight &= !b;
            if !ok {
                if i == ITEM_PROBE {
                    self.pending &= !b; // no answer: protocol 1 stays, the next gvers probes again
                } else {
                    self.item_hold[usize::from(i)].set(now_ms, self.cadence.valve_active_ms);
                }
                continue;
            }
            self.pending &= !b;
            // The re-probe found protocol 2+: re-sync in that protocol.
            if i == ITEM_PROBE && self.proto >= 2 {
                self.request_resync();
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests;
