//! Scheduled calibration: weekday bitmask + local hour:minute -> "calibrate all valves now"
//! decision, once per calendar-day slot, tolerant to missing time, reboots and DST/NTP jumps;
//! and the STM learn-time sync that keeps the STM's own time trigger in line with it (port of
//! `vdm/calib_schedule.h`, DESIGN.md "Calibration schedule"). Hardware-free.

use crate::common::{elapsed_ms, LocalTime};
use crate::config::CalibScheduleConfig;
use crate::link_policy::Outcome;
use crate::stm_codec::{build_get_learn_time, build_set_learn_time, Cmd, Reply, RequestLine};

const MIN_YEAR: u32 = 2000;
const MAX_YEAR: u32 = 9999;

fn is_leap(y: u32) -> bool {
    (y.is_multiple_of(4) && !y.is_multiple_of(100)) || y.is_multiple_of(400)
}

/// Days of month `m` (1..12) in year `y` (C++ `kDays` table; `date_valid` checks `m` first).
fn days_in_month(y: u32, m: u32) -> u32 {
    match m {
        2 if is_leap(y) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

fn date_valid(y: u32, m: u32, d: u32) -> bool {
    (MIN_YEAR..=MAX_YEAR).contains(&y)
        && (1..=12).contains(&m)
        && (1..=days_in_month(y, m)).contains(&d)
}

/// Days since 1970-01-01 of a valid date (H. Hinnant's days_from_civil). Like the C++, it is
/// also evaluated for the fields of any `LocalTime` with `valid` set ([`calib_slot_epoch`]) and
/// any slot key: `/` truncates toward zero as in C++, and no u32 key or u16/u8 fields overflow.
fn days_from_civil(y: u32, m: u32, d: u32) -> i32 {
    let yy = y as i32 - i32::from(m <= 2);
    let era = yy / 400;
    let yoe = yy - era * 400;
    let mp = m as i32 + if m > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + d as i32 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Inverse of [`days_from_civil`], as a yyyymmdd key.
fn key_from_days(z: i32) -> u32 {
    let z = z + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i32::from(m <= 2);
    y as u32 * 10_000 + m as u32 * 100 + d as u32
}

/// 1970-01-01 was a Thursday (4).
fn weekday_from_days(days: i32) -> u8 {
    ((days + 4) % 7) as u8
}

fn key_valid(key: u32) -> bool {
    date_valid(key / 10_000, (key / 100) % 100, key % 100)
}

/// Days since 1970-01-01 of a valid yyyymmdd key.
fn days_from_key(key: u32) -> i32 {
    days_from_civil(key / 10_000, (key / 100) % 100, key % 100)
}

/// Every field in range and the weekday consistent with the date.
fn local_time_valid(t: &LocalTime) -> bool {
    let (y, m, d) = (u32::from(t.year), u32::from(t.month), u32::from(t.mday));
    if !t.valid || !date_valid(y, m, d) {
        return false;
    }
    if t.hour > 23 || t.minute > 59 || t.second > 60 {
        return false;
    }
    t.wday == weekday_from_days(days_from_civil(y, m, d))
}

fn schedule_enabled(cfg: &CalibScheduleConfig) -> bool {
    cfg.day_mask & 0x7F != 0 && cfg.hour <= 23 && cfg.minute <= 59
}

/// Bit `wday` (0..6) of the day mask.
fn day_selected(cfg: &CalibScheduleConfig, wday: u8) -> bool {
    u32::from(cfg.day_mask) & (1 << wday) != 0
}

/// Slot key of a local calendar date: yyyymmdd (e.g. 20260923). 0 when `t` is not valid: valid
/// flag clear, date outside 2000..9999 or impossible (Feb 30), hour/minute/second out of range,
/// or wday not matching the date.
pub fn calib_slot_key(t: &LocalTime) -> u32 {
    if !local_time_valid(t) {
        return 0;
    }
    u32::from(t.year) * 10_000 + u32::from(t.month) * 100 + u32::from(t.mday)
}

/// UTC epoch of local date `slot_key` (a valid yyyymmdd, e.g. from
/// [`CalibScheduler::next_slot`]) at hour:minute, taking the UTC offset of `reference` (its
/// local fields against its epoch). 0 when `slot_key` is 0 or `reference` is not valid. Glue
/// calls it again with the local time at the first result so a DST change before the slot
/// counts. The sum wraps where the C++ would overflow (an epoch near the i64 limits).
pub fn calib_slot_epoch(slot_key: u32, hour: u8, minute: u8, reference: &LocalTime) -> i64 {
    if slot_key == 0 || !reference.valid {
        return 0;
    }
    let days = days_from_key(slot_key)
        - days_from_civil(
            u32::from(reference.year),
            u32::from(reference.month),
            u32::from(reference.mday),
        );
    let secs = (i32::from(hour) - i32::from(reference.hour)) * 3600
        + (i32::from(minute) - i32::from(reference.minute)) * 60
        - i32::from(reference.second);
    reference
        .epoch
        .wrapping_add(i64::from(days) * 86_400)
        .wrapping_add(i64::from(secs))
}

/// [`CalibScheduler::evaluate`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CalibDecision {
    /// nothing to do
    None,
    /// send "staln 255" now; the slot is booked only by on_result(true)
    Fire,
    /// today's slot window passed (or is passing) without valid time; reported once
    SkippedNoTime,
    /// the last Fire got no result within RESULT_TIMEOUT_MS: a failed attempt
    NoResult,
    /// the window of an attempted, unconfirmed slot closed (once per slot)
    Missed,
}

/// Why a scheduled calibration was not confirmed (arg2 of ScheduledCalibrationFailed).
#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CalibFailure {
    #[default]
    None = 0,
    NoReply = 1,
    NotSent = 2,
    NoResult = 3,
    Unsupported = 4,
}

impl CalibFailure {
    pub fn from_raw(v: u8) -> Option<Self> {
        [
            Self::None,
            Self::NoReply,
            Self::NotSent,
            Self::NoResult,
            Self::Unsupported,
        ]
        .get(usize::from(v))
        .copied()
    }
}

/// "none", "no_reply", "not_sent", "no_result", "stm_unsupported".
pub fn calib_failure_name(f: CalibFailure) -> &'static str {
    match f {
        CalibFailure::None => "none",
        CalibFailure::NoReply => "no_reply",
        CalibFailure::NotSent => "not_sent",
        CalibFailure::NoResult => "no_result",
        CalibFailure::Unsupported => "stm_unsupported",
    }
}

/// Rules (DESIGN.md "Calibration schedule"):
///  - A slot exists on local dates whose weekday bit is set in day_mask.
///  - The slot fires when local time is in [hh:mm, hh:mm + grace_minutes) (same local date) and
///    the slot key is newer than the last booked key. A Fire opens an attempt; only
///    on_result(true) books the slot. A failed attempt (on_result(false), or NoResult after
///    RESULT_TIMEOUT_MS) fires again RETRY_MS later while the window is open; a window that
///    closes without success gives Missed once. A spring-forward gap that skips hh:mm still
///    fires at the first minute after it; a fall-back repetition does not fire twice (same date
///    key).
///  - An NTP step backwards to an earlier date never re-fires a booked date (keys only move
///    forward).
///  - The booked key is persisted by glue (restore_last_slot at boot), so an ESP reboot inside
///    the window does not calibrate again.
///  - While time is invalid nothing fires. If time is still invalid when the scheduler is
///    evaluated and the ESP has been up longer than no_time_report_ms, SkippedNoTime is returned
///    once per ESP boot.
///  - day_mask == 0 (bit 7 is ignored) or hour > 23 or minute > 59: never fires. grace_minutes 0
///    is treated as 1.
///  - A local time that calib_slot_key() rejects counts as "no valid time".
///  - Stale bookings: a booked key in the future (clock was wrong) is discarded when a valid time
///    more than 2 days before it is seen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CalibScheduler {
    grace_minutes: u16,
    no_time_report_ms: u32,
    last_slot: u32,
    no_time_reported: bool,
    late_minutes: u16,
    pending: bool,
    attempt_key: u32,
    attempt_at_ms: u32,
    attempts: u8,
    /// no Fire before RETRY_MS after hold_from_ms
    hold_valid: bool,
    hold_from_ms: u32,
    /// the attempted key was confirmed
    booked: bool,
    /// Missed reported for this key
    missed_key: u32,
}

impl Default for CalibScheduler {
    /// C++ `CalibScheduler()`: grace 120 min, no-time report after 1 h of uptime.
    fn default() -> Self {
        Self::new(120, 3_600_000)
    }
}

impl CalibScheduler {
    /// next attempt in the same window
    pub const RETRY_MS: u32 = 600_000;
    pub const RESULT_TIMEOUT_MS: u32 = 60_000;

    /// C++ defaults: grace_minutes 120, no_time_report_ms 3600000 ([`Default`]).
    pub fn new(grace_minutes: u16, no_time_report_ms: u32) -> Self {
        Self {
            grace_minutes: grace_minutes.max(1),
            no_time_report_ms,
            last_slot: 0,
            no_time_reported: false,
            late_minutes: 0,
            pending: false,
            attempt_key: 0,
            attempt_at_ms: 0,
            attempts: 0,
            hold_valid: false,
            hold_from_ms: 0,
            booked: false,
            missed_key: 0,
        }
    }

    /// A key that is not a valid yyyymmdd date is ignored (-> 0).
    pub fn restore_last_slot(&mut self, slot_key: u32) {
        self.last_slot = if key_valid(slot_key) { slot_key } else { 0 };
    }

    pub fn last_slot(&self) -> u32 {
        self.last_slot
    }

    /// Call about every 10 s. `up_ms` is time since ESP boot.
    pub fn evaluate(
        &mut self,
        cfg: &CalibScheduleConfig,
        now: &LocalTime,
        up_ms: u32,
    ) -> CalibDecision {
        let enabled = schedule_enabled(cfg);
        let key = calib_slot_key(now);
        if key == 0 {
            if enabled && !self.no_time_reported && up_ms > self.no_time_report_ms {
                self.no_time_reported = true;
                return CalibDecision::SkippedNoTime;
            }
            return CalibDecision::None;
        }

        if self.pending && elapsed_ms(up_ms, self.attempt_at_ms) >= Self::RESULT_TIMEOUT_MS {
            self.pending = false;
            self.hold_valid = true;
            self.hold_from_ms = up_ms;
            return CalibDecision::NoResult;
        }

        let today = days_from_key(key);
        // last_slot is 0 or a valid key (restore_last_slot / on_result).
        if self.last_slot != 0 && days_from_key(self.last_slot) - today > 2 {
            self.last_slot = 0;
        }

        let now_min = u32::from(now.hour) * 60 + u32::from(now.minute);
        let slot_min = u32::from(cfg.hour) * 60 + u32::from(cfg.minute);
        let in_window = (slot_min..slot_min + u32::from(self.grace_minutes)).contains(&now_min);
        if !self.pending
            && self.attempt_key != 0
            && !self.booked
            && self.missed_key != self.attempt_key
            && (key != self.attempt_key || !in_window)
        {
            self.missed_key = self.attempt_key;
            return CalibDecision::Missed;
        }

        // now.wday is 0..6: calib_slot_key checked it against the date
        if !enabled || !day_selected(cfg, now.wday) || key <= self.last_slot {
            return CalibDecision::None;
        }
        if !in_window || self.pending {
            return CalibDecision::None;
        }
        if self.hold_valid && elapsed_ms(up_ms, self.hold_from_ms) < Self::RETRY_MS {
            return CalibDecision::None;
        }
        self.hold_valid = false;
        if key != self.attempt_key {
            self.attempts = 0;
        }
        self.attempt_key = key;
        self.booked = false;
        self.attempts = self.attempts.saturating_add(1);
        self.pending = true;
        self.attempt_at_ms = up_ms;
        // in the window: 0 <= now_min - slot_min < grace_minutes
        self.late_minutes = (now_min - slot_min) as u16;
        CalibDecision::Fire
    }

    /// Result of the last Fire. ok: the slot is booked (last_slot() = the attempted key),
    /// returns true (the glue persists it). !ok: the next Fire comes RETRY_MS later while the
    /// window is open. Ignored (false) when no attempt is pending.
    pub fn on_result(&mut self, ok: bool, up_ms: u32) -> bool {
        if !self.pending {
            return false;
        }
        self.pending = false;
        if !ok {
            self.hold_valid = true;
            self.hold_from_ms = up_ms;
            return false;
        }
        self.last_slot = self.attempt_key;
        self.booked = true;
        true
    }

    pub fn attempt_pending(&self) -> bool {
        self.pending
    }

    /// Key of the current/last attempt, 0 none.
    pub fn attempt_slot(&self) -> u32 {
        self.attempt_key
    }

    /// Attempts for attempt_slot().
    pub fn attempts(&self) -> u8 {
        self.attempts
    }

    /// Minutes the last firing was late relative to hh:mm (for the event).
    pub fn late_minutes(&self) -> u16 {
        self.late_minutes
    }

    /// yyyymmdd of the next date whose slot will still fire (today while its window is open and
    /// it is not booked yet), looking up to 7 days ahead; 0 when the schedule is off or the time
    /// is not valid. For /api/status.
    pub fn next_slot(&self, cfg: &CalibScheduleConfig, now: &LocalTime) -> u32 {
        let key = calib_slot_key(now);
        if key == 0 || !schedule_enabled(cfg) {
            return 0;
        }
        let today = days_from_key(key);
        let now_min = u32::from(now.hour) * 60 + u32::from(now.minute);
        let slot_min = u32::from(cfg.hour) * 60 + u32::from(cfg.minute);
        for d in 0..=7 {
            let k = key_from_days(today + d);
            if !day_selected(cfg, weekday_from_days(today + d)) || k <= self.last_slot {
                continue;
            }
            if d == 0 && now_min >= slot_min + u32::from(self.grace_minutes) {
                continue;
            }
            return k;
        }
        0
    }
}

// ---------------------------------------------------------------- STM learn time

/// Default STM calibration interval (stlnt) while the ESP schedule is off.
pub const STM_LEARN_TIME_DEFAULT_S: u32 = 604_800;

/// 0 (the STM's own time trigger off) while the ESP schedule is enabled (day_mask & 0x7F != 0,
/// hour <= 23, minute <= 59), else [`STM_LEARN_TIME_DEFAULT_S`].
pub fn stm_learn_time(cfg: &CalibScheduleConfig) -> u32 {
    if schedule_enabled(cfg) {
        0
    } else {
        STM_LEARN_TIME_DEFAULT_S
    }
}

/// C++ `LearnTimeSync::Step`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum LearnTimeSyncStep {
    #[default]
    Read,
    Write,
    Verify,
    Done,
}

/// Keeps the STM's time trigger (stlnt) in line with the ESP schedule. Protocol 3: gtlnt; a
/// value other than the desired one -> stlnt -> gtlnt to verify; equal: nothing until
/// set_desired() or a new session. Protocols 1/2 (no read-back, not persisted by those STMs):
/// desired 0 -> one stlnt 0 per session; desired != 0 -> one stlnt <desired> only after a
/// stlnt 0 went out in this session. Protocol 0: nothing. Failures retry after RETRY_MS.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LearnTimeSync {
    have_desired: bool,
    desired: u32,
    proto: u8,
    step: LearnTimeSyncStep,
    in_flight: bool,
    in_flight_req: RequestLine,
    /// stlnt value in flight
    in_flight_value: u32,
    in_flight_at_ms: u32,
    hold_valid: bool,
    hold_from_ms: u32,
    have_stm: bool,
    stm_value: u32,
    // protocols 1/2, this session
    zero_sent: bool,
    sent_valid: bool,
    sent_value: u32,
}

impl LearnTimeSync {
    pub const RETRY_MS: u32 = 60_000;
    /// no completion -> counts as a failure
    pub const LOST_REQUEST_MS: u32 = 10_000;

    pub fn set_desired(&mut self, seconds: u32) {
        if self.have_desired && seconds == self.desired {
            return;
        }
        self.have_desired = true;
        self.desired = seconds;
        if self.step == LearnTimeSyncStep::Done {
            self.step = LearnTimeSyncStep::Read;
        }
    }

    /// A change starts a new session.
    pub fn set_protocol(&mut self, proto: u8) {
        if proto == self.proto {
            return;
        }
        self.proto = proto;
        self.new_session();
    }

    /// New session.
    pub fn on_stm_reboot(&mut self) {
        self.new_session();
    }

    fn new_session(&mut self) {
        self.step = LearnTimeSyncStep::Read;
        self.in_flight = false;
        self.hold_valid = false;
        self.zero_sent = false;
        self.sent_valid = false;
    }

    fn fail(&mut self, now_ms: u32) {
        self.hold_valid = true;
        self.hold_from_ms = now_ms;
        if self.step != LearnTimeSyncStep::Done {
            self.step = LearnTimeSyncStep::Read;
        }
    }

    /// Next request (Priority::Config), None when nothing is due; one in flight.
    pub fn next(&mut self, now_ms: u32) -> Option<RequestLine> {
        if !self.have_desired || self.proto == 0 {
            return None;
        }
        if self.in_flight {
            if elapsed_ms(now_ms, self.in_flight_at_ms) < Self::LOST_REQUEST_MS {
                return None;
            }
            // C++ onCompletion(copy of the request, Timeout, nullptr): the copy always matches
            self.in_flight = false;
            self.fail(now_ms);
        }
        if self.hold_valid && elapsed_ms(now_ms, self.hold_from_ms) < Self::RETRY_MS {
            return None;
        }
        let out = if self.proto >= 3 {
            match self.step {
                LearnTimeSyncStep::Done => return None,
                LearnTimeSyncStep::Write => {
                    self.in_flight_value = self.desired;
                    build_set_learn_time(self.desired)
                }
                LearnTimeSyncStep::Read | LearnTimeSyncStep::Verify => build_get_learn_time(),
            }
        } else {
            let sent_now = self.sent_valid && self.sent_value == self.desired;
            if sent_now || (self.desired != 0 && !self.zero_sent) {
                return None;
            }
            self.in_flight_value = self.desired;
            build_set_learn_time(self.desired)
        };
        self.in_flight = true;
        self.in_flight_req.clone_from(&out);
        self.in_flight_at_ms = now_ms;
        Some(out)
    }

    /// Completion of a request; ignored unless it is the one in flight (same command and text).
    /// `rep` is the reply (C++ null: none).
    pub fn on_completion(
        &mut self,
        req: &RequestLine,
        o: Outcome,
        rep: Option<&Reply>,
        now_ms: u32,
    ) {
        if !self.in_flight
            || req.cmd != self.in_flight_req.cmd
            || req.text != self.in_flight_req.text
        {
            return;
        }
        self.in_flight = false;
        let rep = match (o, rep) {
            (Outcome::Ok, Some(rep)) => rep,
            _ => {
                self.fail(now_ms);
                return;
            }
        };
        self.hold_valid = false;
        if req.cmd == Cmd::Stlnt {
            if self.in_flight_value == 0 {
                self.zero_sent = true;
            }
            self.sent_valid = true;
            self.sent_value = self.in_flight_value;
            self.step = LearnTimeSyncStep::Verify;
            return;
        }
        self.have_stm = true;
        self.stm_value = rep.learn_time;
        if self.stm_value == self.desired {
            self.step = LearnTimeSyncStep::Done;
        } else if self.step == LearnTimeSyncStep::Verify {
            self.fail(now_ms);
        } else {
            self.step = LearnTimeSyncStep::Write;
        }
    }

    pub fn have_stm_value(&self) -> bool {
        self.have_stm
    }

    pub fn stm_value(&self) -> u32 {
        self.stm_value
    }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_link;
