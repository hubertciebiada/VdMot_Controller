//! Values published over MQTT that are derived from the config and the STM snapshot (port of
//! `vdm/mqtt_values.h`). Hardware-free.
//!
//! The C++ arrays with a count become slices (`&[ValveState]`, `&[TempReading]`); the C++ null
//! array is the empty slice where the count is a parameter. A valve snapshot of exactly
//! [`VALVE_COUNT`] entries that the C++ may pass as null is an `Option<&[ValveState; 12]>`.
//! The -1 of a missing bus index is None.

use crate::common::{
    c_str, elapsed_ms, fmt_fit, is_zero, LocalTime, OneWireId, TextBuf, TEMP_SLOT_COUNT,
    VALVE_COUNT, VOLT_SLOT_COUNT,
};
use crate::config::{crc32, item_segment, Config, ItemKind};
use crate::link_policy::LinkState;
use crate::valve_model::{
    temp_raw_valid, vad_valid, TargetSource, TargetSync, TempReading, ValveState, VoltReading,
    CHANGE_CALIB_RETRIES, CHANGE_COUNTERS, CHANGE_FAILSAFE, CHANGE_HEALTH, CHANGE_KNOWN,
    CHANGE_MEAN_CURRENT, CHANGE_POSITION, CHANGE_SENSORS, CHANGE_STATUS, CHANGE_SYNC,
    CHANGE_TARGET, CHANGE_TEMP1, CHANGE_TEMP2, HEALTH_BLOCKED, HEALTH_FAILED, HEALTH_NO_VALVE,
    HEALTH_STALE, HEALTH_TARGET_UNCONFIRMED, HEALTH_TEMP_FAILED,
};

/// Valves (the snapshot arrays).
const N: usize = VALVE_COUNT as usize;

/// A full valve snapshot (C++ `const ValveState*` of [`VALVE_COUNT`] entries).
pub type ValveSnapshot = [ValveState; N];

/// System-wide conditions besides the link and the valves.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SystemFlags {
    /// the STM runs in safe mode
    pub safe_mode: bool,
    /// valves are at their failsafe positions (lease or ESP emulation)
    pub failsafe: bool,
}

/// The health flags that make common/state an error.
const HEALTH_ERROR: u16 = HEALTH_BLOCKED + HEALTH_FAILED;

/// Legacy MQTT common/state value (0 ok, 1 info, 2 error), derived:
///  2 when the link is Down, the STM is in safe mode, or an ACTIVE valve has
///    [`HEALTH_BLOCKED`] or [`HEALTH_FAILED`];
///  1 when the link is Unknown/Degraded/Booting, the failsafe is active, or
///    an active valve has any other health flag;
///  0 otherwise (Suspended during flashing counts as 1).
///
/// `valves` is the C++ array with its count (the first [`VALVE_COUNT`] entries count; the C++
/// null array is the empty slice). C++ default for `f`: `SystemFlags::default()`.
pub fn system_state(
    link: LinkState,
    valves: &[ValveState],
    active_mask: u16,
    f: SystemFlags,
) -> u8 {
    let mut info = link != LinkState::Up || f.failsafe;
    if link == LinkState::Down || f.safe_mode {
        return 2;
    }
    for (i, v) in valves.iter().take(N).enumerate() {
        if (active_mask >> i) & 1 == 0 {
            continue;
        }
        if v.health & HEALTH_ERROR != 0 {
            return 2;
        }
        if v.health != 0 {
            info = true;
        }
    }
    u8::from(info)
}

/// Bit v set for every active valve of the config.
pub fn active_valve_mask(c: &Config) -> u16 {
    let mut m = 0;
    for (i, v) in c.valves.iter().enumerate() {
        if v.active {
            // one bit per valve, added once: the `|` of the C++
            m += 1 << i;
        }
    }
    m
}

/// Temperature of config slot `slot1` (1-based) with its offset from the bus readings: the
/// reading with the slot's id that was seen, is valid and not older than `stale_ms`. None =
/// "failed" (also for an empty slot). `bus` is the C++ array with its count.
pub fn slot_temp_tenths(
    c: &Config,
    bus: &[TempReading],
    slot1: u8,
    now_ms: u32,
    stale_ms: u32,
) -> Option<i32> {
    let s = usize::from(slot1)
        .checked_sub(1)
        .and_then(|i| c.temps.get(i))?;
    let r = bus.get(usize::from(find_temp_bus(bus, &s.id)?))?;
    if !r.seen || !temp_raw_valid(r.raw) || elapsed_ms(now_ms, r.last_seen_ms) > stale_ms {
        return None;
    }
    Some(i32::from(r.raw) + i32::from(s.offset))
}

/// Value of volt slot `i` (0-based) in its unit ((vad / 100 + offset) * factor), same rules.
pub fn slot_volt_value(
    c: &Config,
    bus: &[VoltReading],
    i: u8,
    now_ms: u32,
    stale_ms: u32,
) -> Option<f64> {
    let s = c.volts.get(usize::from(i))?;
    let r = bus.get(usize::from(find_volt_bus(bus, &s.id)?))?;
    if !r.seen || !vad_valid(r.vad) || elapsed_ms(now_ms, r.last_seen_ms) > stale_ms {
        return None;
    }
    Some((f64::from(r.vad) / 100.0 + f64::from(s.offset)) * f64::from(s.factor))
}

/// The STM assigned temp slot `slot1` (1-based) to a valve (sensor 1 or 2). C++ null valves:
/// None (false).
pub fn temp_assigned_to_valve(valves: Option<&ValveSnapshot>, slot1: u8) -> bool {
    if slot1 == 0 {
        return false;
    }
    valves.is_some_and(|vs| vs.iter().any(|v| v.sensor_slot.contains(&slot1)))
}

/// temps/<T>/… of slot `i`: active, id set, and all_temps or not assigned to a valve.
pub fn temp_published(c: &Config, valves: Option<&ValveSnapshot>, i: u8) -> bool {
    c.temps.get(usize::from(i)).is_some_and(|s| {
        s.active
            && !is_zero(&s.id)
            && (c.mqtt.all_temps || !temp_assigned_to_valve(valves, i.wrapping_add(1)))
    })
}

/// E23: sensors/<S>/… for every configured volt slot (id set), active or not.
pub fn volt_published_mqtt(c: &Config, i: u8) -> bool {
    c.volts.get(usize::from(i)).is_some_and(|s| !is_zero(&s.id))
}

/// Discovery announces a volt slot when it is active and has an id.
pub fn volt_announced(c: &Config, i: u8) -> bool {
    volt_published_mqtt(c, i) && c.volts.get(usize::from(i)).is_some_and(|s| s.active)
}

/// 0-based index of the first of the first `max` readings whose id is `id` (not zero).
fn find_bus<'a>(ids: impl Iterator<Item = &'a OneWireId>, max: u8, id: &OneWireId) -> Option<u8> {
    if is_zero(id) {
        return None;
    }
    let b = ids.take(usize::from(max)).position(|r| r == id)?;
    u8::try_from(b).ok()
}

/// E22: 0-based STM bus index of the reading with this id among the readings (at most
/// [`TEMP_SLOT_COUNT`]), None (C++ -1) when absent or the id is zero.
pub fn find_temp_bus(bus: &[TempReading], id: &OneWireId) -> Option<u8> {
    find_bus(bus.iter().map(|r| &r.id), TEMP_SLOT_COUNT, id)
}

/// [`find_temp_bus`] for the volt readings (at most [`VOLT_SLOT_COUNT`]).
pub fn find_volt_bus(bus: &[VoltReading], id: &OneWireId) -> Option<u8> {
    find_bus(bus.iter().map(|r| &r.id), VOLT_SLOT_COUNT, id)
}

/// Topic segment of temp or volt slot `idx0`: item_segment() when the slot has a topic override
/// or a name; otherwise bus_index + 1 (legacy: unnamed sensors are published under their STM
/// bus number). 0 for an unnamed slot without a bus index (C++ -1), for a valve or an index out
/// of range, or when it does not fit.
pub fn sensor_topic_segment(
    c: &Config,
    kind: ItemKind,
    idx0: u8,
    bus_index: Option<u8>,
    out: &mut [u8],
) -> usize {
    let i = usize::from(idx0);
    let texts = match kind {
        ItemKind::Temp => c.temps.get(i).map(|s| (&s.name, &s.topic)),
        ItemKind::Volt => c.volts.get(i).map(|s| (&s.name, &s.topic)),
        ItemKind::Valve => None,
    };
    let Some((name, topic)) = texts else {
        return 0;
    };
    if !c_str(name).is_empty() || !c_str(topic).is_empty() {
        return item_segment(c, kind, idx0, out);
    }
    let Some(b) = bus_index else {
        return 0;
    };
    fmt_fit(out, format_args!("{}", u32::from(b) + 1))
}

/// W5: value of valves/<V>/target. Not separate: the desired target (the topic doubles as
/// command topic). Separate: the STM's target as read back (stm_target while stm_target_known),
/// nothing while a restored target is not synced yet, or the desired target while the ESP
/// emulates the failsafe of this valve (fs_override: the STM then holds the failsafe position).
/// None = nothing to publish now.
pub fn published_target(v: &ValveState, separate: bool) -> Option<u8> {
    if !separate || v.fs_override {
        return v.desired_valid.then_some(v.desired);
    }
    if v.source == TargetSource::Restored && v.sync != TargetSync::Synced {
        return None;
    }
    v.stm_target_known.then_some(v.stm_target)
}

/// The health flags of [`valve_problem`] (added in bit order: the `|` of the C++).
pub const PROBLEM_MASK: u16 = HEALTH_BLOCKED
    + HEALTH_FAILED
    + HEALTH_NO_VALVE
    + HEALTH_STALE
    + HEALTH_TARGET_UNCONFIRMED
    + HEALTH_TEMP_FAILED;

/// health & [`PROBLEM_MASK`].
pub fn valve_problem(v: &ValveState) -> bool {
    v.health & PROBLEM_MASK != 0
}

/// Up or Degraded.
pub fn stm_online(s: LinkState) -> bool {
    matches!(s, LinkState::Up | LinkState::Degraded)
}

/// The diff_valve() groups whose change publishes the compat topics of a valve on change (the
/// extended counters and the last move are diag topics). Added in bit order: the `|` of the
/// C++.
pub const VALVE_COMPAT_MASK: u32 = CHANGE_STATUS
    + CHANGE_POSITION
    + CHANGE_TARGET
    + CHANGE_MEAN_CURRENT
    + CHANGE_TEMP1
    + CHANGE_TEMP2
    + CHANGE_COUNTERS
    + CHANGE_CALIB_RETRIES
    + CHANGE_SYNC
    + CHANGE_SENSORS
    + CHANGE_HEALTH
    + CHANGE_KNOWN
    + CHANGE_FAILSAFE;

/// CRC-32 over exactly the fields of [`VALVE_COMPAT_MASK`], field by field (padding never
/// counts). The MQTT task keeps it per valve instead of a copy of the last published ValveState
/// (4 B instead of 136): equal fields give equal keys, and a change of any of them changes the
/// key unless the CRC-32 collides (2^-32; that change then goes out with the next full publish).
///
/// The bytes are the C++ ones (little-endian fields, bool 0/1, enums as u8), in the order of
/// the diff_valve() groups, so the key equals the one of the C++ firmware.
pub fn valve_compat_key(v: &ValveState) -> u32 {
    // kChangeStatus, kChangePosition, kChangeTarget
    let mut c = crc32(
        &[
            v.status,
            u8::from(v.calibrating),
            v.position,
            u8::from(v.desired_valid),
            v.desired,
        ],
        0,
    );
    c = crc32(&v.mean_current.to_le_bytes(), c); // kChangeMeanCurrent
    c = crc32(&v.temp1.to_le_bytes(), c); // kChangeTemp1
    c = crc32(&v.temp2.to_le_bytes(), c); // kChangeTemp2
    c = crc32(&v.moves.to_le_bytes(), c); // kChangeCounters
    c = crc32(&v.open_count.to_le_bytes(), c);
    c = crc32(&v.close_count.to_le_bytes(), c);
    c = crc32(&v.dead_zone.to_le_bytes(), c);
    // kChangeCalibRetries, kChangeSync
    c = crc32(
        &[
            v.calib_retries,
            v.sync as u8,
            u8::from(v.stm_target_known),
            v.stm_target,
        ],
        c,
    );
    c = crc32(&v.sensor_id[0].b, c); // kChangeSensors
    c = crc32(&v.sensor_id[1].b, c);
    c = crc32(&v.sensor_slot, c);
    c = crc32(&v.health.to_le_bytes(), c); // kChangeHealth
    c = crc32(&[u8::from(v.known), u8::from(v.has_v3)], c); // kChangeKnown, kChangeFailsafe
    c = crc32(&v.stm_flags.to_le_bytes(), c);
    crc32(
        &[
            v.fault,
            v.fs_pct,
            v.drive,
            v.retries,
            u8::from(v.auto_retry),
            u8::from(v.fs_override),
            v.fs_target,
        ],
        c,
    )
}

/// Display name of valve `idx0` for new HA entity names: the configured name as it is (UTF-8,
/// spaces; read up to a NUL, the C++ null name is the empty one), or "Valve <n>" when empty.
/// Returns the length (0 when it does not fit).
pub fn valve_display_name(config_name: &[u8], idx0: u8, out: &mut [u8]) -> usize {
    let name = c_str(config_name);
    if name.is_empty() {
        return fmt_fit(out, format_args!("Valve {}", u32::from(idx0) + 1));
    }
    let mut w = TextBuf::new(out);
    w.push_bytes(name);
    w.fit()
}

/// Bit `i` of `mask`; false past bit 15.
fn bit(mask: u16, i: u8) -> bool {
    mask.checked_shr(u32::from(i)).is_some_and(|m| m & 1 != 0)
}

/// End of the last calibration per valve (legacy calibration/date), observed from the
/// snapshots: a valve whose `calibrating` falls from true to false ended one. The first
/// observation only learns the state.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CalibEndTracker {
    observed: bool,
    calibrating: u16,
    ended: u16,
    dirty: u16,
    end: [LocalTime; N],
}

impl CalibEndTracker {
    /// Returns the mask of valves that ended a calibration now (stamped `now`). C++ null
    /// valves: None (0, nothing learned).
    pub fn observe(&mut self, valves: Option<&ValveSnapshot>, now: &LocalTime) -> u16 {
        let Some(valves) = valves else {
            return 0;
        };
        let mut cal = 0;
        for (i, v) in valves.iter().enumerate() {
            if v.calibrating {
                cal += 1 << i;
            }
        }
        let ended_now = if self.observed {
            self.calibrating & !cal
        } else {
            0
        };
        for (i, end) in self.end.iter_mut().enumerate() {
            if (ended_now >> i) & 1 != 0 {
                *end = *now;
            }
        }
        self.ended |= ended_now;
        self.dirty |= ended_now;
        self.calibrating = cal;
        self.observed = true;
        ended_now
    }

    pub fn ended(&self, valve: u8) -> bool {
        bit(self.ended, valve)
    }

    /// The end of `valve`'s last calibration; an out-of-range valve reads entry 0 (C++).
    pub fn end(&self, valve: u8) -> &LocalTime {
        self.end.get(usize::from(valve)).unwrap_or(&self.end[0])
    }

    /// Ended since the last [`clear_dirty`](Self::clear_dirty).
    pub fn dirty(&self, valve: u8) -> bool {
        bit(self.dirty, valve)
    }

    pub fn clear_dirty(&mut self, valve: u8) {
        if let Some(b) = 1u16.checked_shl(u32::from(valve)) {
            self.dirty &= !b;
        }
    }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_mut;
