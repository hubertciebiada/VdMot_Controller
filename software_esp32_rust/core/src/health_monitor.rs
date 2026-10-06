//! Health decisions: turns model/link changes into Events (with per-condition dedup) (port of
//! `vdm/health_monitor.h`). Hardware-free.

use core::cmp::Ordering;

use crate::common::{elapsed_ms, NO_VALVE, TEMP_SLOT_COUNT, VALVE_COUNT};
use crate::event_log::{event_default_severity, make_event, Event, EventCode};
use crate::failsafe::FAILSAFE_HOLD;
use crate::link_policy::LinkState;
use crate::stm_codec::{
    StmStatus, ValveStatus, CAL_FLAG_LAST_FAILED, STM_FLAG_FS_BLOCKED, STM_SYS_PROTECT_SUSPENDED,
};
use crate::valve_model::{
    TargetSource, ValveModelParams, ValveState, HEALTH_STALE, HEALTH_STROKE_SHORT,
    HEALTH_TARGET_UNCONFIRMED,
};

/// Maximum events one call may produce (callers size their arrays with it).
pub const MAX_EVENTS_PER_UPDATE: usize = 8;

const STATUS_IDLE: u8 = ValveStatus::Idle as u8;
const STATUS_FAILED: u8 = ValveStatus::Failed as u8;
const STATUS_NO_VALVE: u8 = ValveStatus::NoValve as u8;
const STATUS_BLOCKED: u8 = ValveStatus::Blocked as u8;
const COUNTER_EVENT_INTERVAL_MS: u32 = 600_000;
/// Disjoint bits: `+` is the C++ `|`.
const RECOVERABLE_FLAGS: u16 = HEALTH_STALE + HEALTH_TARGET_UNCONFIRMED;
/// arg2 of ValveBlocked/CalibFailed (failsafe position) and ValveFailed (fault) when the
/// snapshot has neither: the message leaves it out.
const ARG_NONE: i32 = -1;
/// an older repair is not re-reported after a restart
const CFG_EVENT_MAX_UPTIME_S: u32 = 600;

/// Position the STM drives a blocked valve to (protocol 3), else [`ARG_NONE`].
fn blocked_position(v: &ValveState) -> i32 {
    if !v.has_v3 || (v.stm_flags & STM_FLAG_FS_BLOCKED) == 0 || v.fs_pct == FAILSAFE_HOLD {
        return ARG_NONE;
    }
    i32::from(v.fs_pct)
}

/// Bounded event sink for one call: at most [`MAX_EVENTS_PER_UPDATE`] events, never more than
/// the caller's slice holds (C++ `maxOut`; a null `out` is the empty slice).
struct Sink<'a> {
    out: &'a mut [Event],
    n: usize,
}

impl<'a> Sink<'a> {
    fn new(out: &'a mut [Event]) -> Self {
        let max = out.len().min(MAX_EVENTS_PER_UPDATE);
        let (out, _) = out.split_at_mut(max);
        Self { out, n: 0 }
    }

    fn add(&mut self, code: EventCode, valve: u8, a1: i32, a2: i32) {
        if let Some(slot) = self.out.get_mut(self.n) {
            *slot = make_event(code, event_default_severity(code), valve, a1, a2, b"");
            self.n += 1;
        }
    }
}

/// Status value of a "bad" condition (Blocked/Failed/NoValve), 0 otherwise. Only called for
/// known snapshots.
fn bad_status(v: &ValveState, active: bool) -> u8 {
    if v.status == STATUS_BLOCKED || v.status == STATUS_FAILED {
        return v.status;
    }
    if v.status == STATUS_NO_VALVE && active {
        return v.status;
    }
    0
}

/// The event of a bad status (`bad_status() != 0`).
fn add_bad(sink: &mut Sink<'_>, valve: u8, status: u8, v: &ValveState) {
    let retries = i32::from(v.calib_retries);
    match status {
        STATUS_BLOCKED => sink.add(EventCode::ValveBlocked, valve, retries, blocked_position(v)),
        STATUS_FAILED => {
            let fault = if v.has_v3 {
                i32::from(v.fault)
            } else {
                ARG_NONE
            };
            sink.add(EventCode::ValveFailed, valve, retries, fault);
        }
        _ => sink.add(EventCode::ValveNoValve, valve, retries, 0),
    }
}

fn rose(before: &ValveState, after: &ValveState, flag: u16) -> bool {
    (after.health & flag) != 0 && (before.health & flag) == 0
}

fn add_target_set(sink: &mut Sink<'_>, valve: u8, before: &ValveState, after: &ValveState) {
    if !after.desired_valid {
        return;
    }
    if !matches!(
        after.source,
        TargetSource::Web | TargetSource::Mqtt | TargetSource::Assembly
    ) {
        return;
    }
    if before.desired_valid && before.desired == after.desired {
        return;
    }
    sink.add(
        EventCode::TargetSet,
        valve,
        i32::from(after.desired),
        after.source as i32,
    );
}

/// Events of a known valve between two snapshots, except TargetSet and ValveStateChanged.
/// Returns true when a status-related event covered the status change.
fn add_transitions(
    sink: &mut Sink<'_>,
    valve: u8,
    before: &ValveState,
    after: &ValveState,
    active: bool,
    min_counts: u16,
) -> bool {
    let prev_bad = bad_status(before, active);
    let cur_bad = bad_status(after, active);

    // Calibration outcome first: it replaces the status event it implies.
    let mut calib_ok = false;
    let mut calib_failed = false;
    if !before.calibrating && after.calibrating {
        let arg = if after.auto_retry { 2 } else { 0 };
        sink.add(EventCode::CalibStarted, valve, arg, 0);
    } else if before.calibrating && !after.calibrating {
        // v2 reports the outcome itself (calState bit 3); 1.x only by the status.
        let failed = after.status == STATUS_BLOCKED
            || (after.has_extended && (after.cal_flags & CAL_FLAG_LAST_FAILED) != 0);
        if failed {
            sink.add(
                EventCode::CalibFailed,
                valve,
                i32::from(after.calib_retries),
                blocked_position(after),
            );
            calib_failed = true;
        } else if after.status == STATUS_IDLE {
            // The C++ static_cast<int32_t> of the uint32_t counts.
            sink.add(
                EventCode::CalibOk,
                valve,
                after.open_count as i32,
                after.close_count as i32,
            );
            calib_ok = true;
        }
    }
    if (before.calibrating || after.calibrating)
        && !calib_failed
        && after.calib_retries > before.calib_retries
    {
        sink.add(
            EventCode::CalibRetry,
            valve,
            i32::from(after.calib_retries),
            0,
        );
    }

    let mut covered = calib_ok || calib_failed;
    if cur_bad != prev_bad {
        if cur_bad != 0 {
            if !(calib_failed && cur_bad == STATUS_BLOCKED) {
                add_bad(sink, valve, cur_bad, after);
            }
        } else if !covered {
            sink.add(EventCode::ValveRecovered, valve, i32::from(prev_bad), 0);
        }
        covered = true;
    }

    if before.has_extended && after.has_extended {
        if after.early_stops > before.early_stops {
            sink.add(
                EventCode::EarlyStop,
                valve,
                after.early_stops as i32,
                after.last_move.stop as i32,
            );
        }
        if after.cmd_rejected > before.cmd_rejected {
            sink.add(EventCode::CmdRejected, valve, after.cmd_rejected as i32, 0);
        }
    }
    if rose(before, after, HEALTH_TARGET_UNCONFIRMED) {
        sink.add(
            EventCode::TargetNotConfirmed,
            valve,
            i32::from(after.desired),
            i32::from(after.push_attempts),
        );
    }
    if rose(before, after, HEALTH_STALE) {
        let stale_s = ValveModelParams::default().stale_ms / 1000;
        sink.add(EventCode::ValveStale, valve, stale_s as i32, 0);
    }
    if rose(before, after, HEALTH_STROKE_SHORT) {
        let stroke = after.open_count.min(after.close_count);
        sink.add(
            EventCode::CalibStrokeShort,
            valve,
            stroke as i32,
            i32::from(min_counts),
        );
    }
    let cleared = before.health & !after.health & RECOVERABLE_FLAGS;
    if cleared != 0 {
        sink.add(EventCode::ValveRecovered, valve, 0, i32::from(cleared));
    }
    covered
}

/// Rate-limited "total increased" tracking of one counter on one side (C++
/// `HealthMonitor::CounterTrack`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct HealthMonitorCounterTrack {
    baselined: bool,
    /// last total reported (or the baseline)
    total: u32,
    /// last_event_ms is meaningful
    reported: bool,
    last_event_ms: u32,
}

impl HealthMonitorCounterTrack {
    /// C++ `counterIncreased`: the first total (or one below the last: the counter restarted)
    /// is the baseline; an increase reports at most once per [`COUNTER_EVENT_INTERVAL_MS`]
    /// with the latest total, a rate-limited increase stays pending.
    fn increased(&mut self, total: u32, now_ms: u32) -> bool {
        if !self.baselined {
            self.baselined = true;
            self.total = total;
            return false;
        }
        match total.cmp(&self.total) {
            Ordering::Less => {
                self.total = total;
                false
            }
            Ordering::Equal => false,
            Ordering::Greater => {
                if self.reported
                    && elapsed_ms(now_ms, self.last_event_ms) < COUNTER_EVENT_INTERVAL_MS
                {
                    return false;
                }
                self.total = total;
                self.reported = true;
                self.last_event_ms = now_ms;
                true
            }
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HealthMonitor {
    rx_overflow: [HealthMonitorCounterTrack; 2],
    parse_err: [HealthMonitorCounterTrack; 2],
    uart: HealthMonitorCounterTrack,
    min_counts: u16,
}

impl HealthMonitor {
    /// Compares two snapshots of one valve and writes events to `out` (at most `out.len()`,
    /// never more than [`MAX_EVENTS_PER_UPDATE`]); returns their number. Rules (DESIGN.md
    /// "Health"):
    ///  - status transitions into Blocked/Failed/NoValve(active only) raise
    ///    ValveBlocked/ValveFailed/ValveNoValve once; leaving them raises ValveRecovered;
    ///  - calibrating false->true: CalibStarted; true->false: CalibOk when the new status is
    ///    Idle, CalibFailed when Blocked; calib_retries increase during calibration: CalibRetry;
    ///  - ValveBlocked and CalibFailed carry arg2 = the failsafe position the STM drives the
    ///    blocked valve to (protocol 3 FS_BLOCKED flag, fs_pct not hold), else -1; ValveFailed
    ///    arg2 = the fault (protocol 3), else -1;
    ///  - CalibStarted arg1 2 when the calibration is an automatic retry (auto_retry), else 0;
    ///  - HEALTH_STROKE_SHORT set: CalibStrokeShort (arg1 min(open_count, close_count), arg2
    ///    [`set_min_counts`](Self::set_min_counts));
    ///  - early_stops / cmd_rejected increase: EarlyStop / CmdRejected;
    ///  - HEALTH_TARGET_UNCONFIRMED set (sync -> Failed): TargetNotConfirmed; HEALTH_STALE set:
    ///    ValveStale (arg1 = the model's default staleness threshold in s); either flag
    ///    clearing: ValveRecovered with arg1 0 and arg2 = the cleared flag bits;
    ///  - desired target change by the web, MQTT or an assembly: TargetSet (Info); adopting
    ///    the STM's target (source Stm) and restoring one (Restored) are silent;
    ///  - any other status change: ValveStateChanged (Debug).
    ///
    /// A calibration outcome replaces the status event it implies: CalibFailed suppresses
    /// ValveBlocked, and CalibOk/CalibFailed suppress ValveRecovered. Nothing is emitted for the
    /// first snapshot of a valve (known false->true) except Blocked/Failed/NoValve if the valve
    /// starts in that state (and TargetSet). When more than `out` holds arise, the most severe
    /// kinds (status, calibration) come first.
    pub fn on_valve(
        &self,
        valve: u8,
        before: &ValveState,
        after: &ValveState,
        active: bool,
        out: &mut [Event],
    ) -> usize {
        if valve >= VALVE_COUNT {
            return 0;
        }
        let mut sink = Sink::new(out);
        let mut status_changed = false;
        if !before.known && after.known {
            let bad = bad_status(after, active);
            if bad != 0 {
                add_bad(&mut sink, valve, bad, after);
            }
        } else if after.known {
            status_changed =
                !add_transitions(&mut sink, valve, before, after, active, self.min_counts)
                    && before.status != after.status;
        }
        add_target_set(&mut sink, valve, before, after);
        if status_changed {
            sink.add(
                EventCode::ValveStateChanged,
                valve,
                i32::from(before.status),
                i32::from(after.status),
            );
        }
        sink.n
    }

    /// min_counts of the STM (gmotc), arg2 of CalibStrokeShort.
    pub fn set_min_counts(&mut self, min_counts: u16) {
        self.min_counts = min_counts;
    }

    /// Protocol 3 system events from two gstax statuses; `before` None (or not protocol 3): the
    /// first status since the ESP booted or the STM rebooted. Only when `after.v3`:
    ///  - safe_mode 0 -> 1, or 1 on the first status: StmSafeMode (arg1 wdg_resets); 1 -> 0:
    ///    StmSafeModeEnded;
    ///  - cfg_events increased, or > 0 on the first status while the STM uptime is below 600 s
    ///    (an old repair is not reported again after a restart): StmConfigRepaired (arg1
    ///    cfg_flags, arg2 cfg_events);
    ///  - uart_ore + uart_fe + uart_ne + rx_dropped increased (the first status is the baseline,
    ///    a lower total re-baselines silently, at most one event per 10 min with the latest
    ///    totals): StmUartErrors (arg1 ore + fe + ne, arg2 rx_dropped);
    ///  - sys_flags PROTECT_SUSPENDED set, not set before (or on the first status):
    ///    StmProtectionSuspended.
    pub fn on_stm_status(
        &mut self,
        before: Option<&StmStatus>,
        after: &StmStatus,
        now_ms: u32,
        out: &mut [Event],
    ) -> usize {
        if !after.v3 {
            return 0;
        }
        let mut sink = Sink::new(out);
        let prev = before.filter(|b| b.v3);
        let prev_safe = prev.is_some_and(|p| p.safe_mode);
        if after.safe_mode && !prev_safe {
            sink.add(
                EventCode::StmSafeMode,
                NO_VALVE,
                i32::from(after.wdg_resets),
                0,
            );
        } else if !after.safe_mode && prev_safe {
            sink.add(EventCode::StmSafeModeEnded, NO_VALVE, 0, 0);
        }
        let repaired = match prev {
            None => after.cfg_events > 0 && after.uptime_s < CFG_EVENT_MAX_UPTIME_S,
            Some(p) => after.cfg_events > p.cfg_events,
        };
        if repaired {
            sink.add(
                EventCode::StmConfigRepaired,
                NO_VALVE,
                i32::from(after.cfg_flags),
                after.cfg_events as i32,
            );
        }
        // The C++ uint32_t sums wrap (totals the STM cannot reach).
        let line_errors = after
            .uart_ore
            .wrapping_add(after.uart_fe)
            .wrapping_add(after.uart_ne);
        if prev.is_none() {
            self.uart.baselined = false;
        }
        if self
            .uart
            .increased(line_errors.wrapping_add(after.rx_dropped), now_ms)
        {
            sink.add(
                EventCode::StmUartErrors,
                NO_VALVE,
                line_errors as i32,
                after.rx_dropped as i32,
            );
        }
        let suspended = |s: &StmStatus| (s.sys_flags & STM_SYS_PROTECT_SUSPENDED) != 0;
        if suspended(after) && !prev.is_some_and(suspended) {
            sink.add(EventCode::StmProtectionSuspended, NO_VALVE, 0, 0);
        }
        sink.n
    }

    /// LinkUp / LinkDegraded / LinkDown on state transitions (Booting and Suspended are silent:
    /// the reset/flash events describe them).
    pub fn on_link(
        &self,
        before: LinkState,
        after: LinkState,
        consecutive_timeouts: u8,
        out: &mut [Event],
    ) -> usize {
        if before == after {
            return 0;
        }
        let mut sink = Sink::new(out);
        let timeouts = i32::from(consecutive_timeouts);
        match after {
            LinkState::Up => sink.add(EventCode::LinkUp, NO_VALVE, 0, 0),
            LinkState::Degraded => sink.add(EventCode::LinkDegraded, NO_VALVE, timeouts, 0),
            LinkState::Down => sink.add(EventCode::LinkDown, NO_VALVE, timeouts, 0),
            LinkState::Unknown | LinkState::Booting | LinkState::Suspended => {}
        }
        sink.n
    }

    /// STM counters (gstat, or the ESP line assembler): StmRxOverflow / StmParseErrors when the
    /// total increased, at most once per 10 min per counter and side (the latest total is
    /// reported). side 0 = ESP (counts from 0 at ESP boot), 1 = STM: its first observation
    /// after ESP boot is only the baseline. A total below the last one (STM reboot)
    /// re-baselines silently. side > 1 is ignored.
    pub fn on_stm_counters(
        &mut self,
        rx_overflow_total: u32,
        parse_err_total: u32,
        side: u8,
        now_ms: u32,
        out: &mut [Event],
    ) -> usize {
        let s = usize::from(side);
        let (Some(rx), Some(pe)) = (self.rx_overflow.get_mut(s), self.parse_err.get_mut(s)) else {
            return 0;
        };
        let mut sink = Sink::new(out);
        if side == 0 {
            // The ESP counters start at 0 with the ESP: that is their baseline.
            rx.baselined = true;
            pe.baselined = true;
        }
        if rx.increased(rx_overflow_total, now_ms) {
            sink.add(
                EventCode::StmRxOverflow,
                NO_VALVE,
                rx_overflow_total as i32,
                i32::from(side),
            );
        }
        if pe.increased(parse_err_total, now_ms) {
            sink.add(
                EventCode::StmParseErrors,
                NO_VALVE,
                parse_err_total as i32,
                i32::from(side),
            );
        }
        sink.n
    }

    /// Temperature sensor in config slot (1-based, 1..34) went invalid / valid again.
    /// `was_valid`/`is_valid` from temp_raw_valid(); first observation (`was_known` false) is
    /// silent. TempSensorFailed carries arg2 = raw; the caller may put the sensor id into the
    /// event text.
    pub fn on_temp_sensor(
        &self,
        slot: u8,
        was_known: bool,
        was_valid: bool,
        is_valid: bool,
        raw: i16,
        out: &mut [Event],
    ) -> usize {
        if slot == 0 || slot > TEMP_SLOT_COUNT || !was_known || was_valid == is_valid {
            return 0;
        }
        let mut sink = Sink::new(out);
        if is_valid {
            sink.add(EventCode::TempSensorRecovered, NO_VALVE, i32::from(slot), 0);
        } else {
            sink.add(
                EventCode::TempSensorFailed,
                NO_VALVE,
                i32::from(slot),
                i32::from(raw),
            );
        }
        sink.n
    }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_link;
