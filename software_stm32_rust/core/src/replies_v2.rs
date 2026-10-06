//! Reply formatters of the protocol v2 commands (see PROTOCOL_V2.md).
//! All replies are one line of space separated integers without a trailing
//! space; the caller appends CR LF. Each formatter writes all or nothing.

use crate::buf_writer::{BufWriter, Storage};
use crate::calibration::EscalationConfig;
use crate::motor_params::{
    HIGH_FAC_RANGE, LOW_FAC_RANGE, MAX_RETRIES_RANGE, MIN_COUNTS_RANGE, START_ON_POWER_RANGE,
};
use crate::move_classifier::MoveResult;
use crate::profile_recorder::{ProfileRecorder, PROFILE_SAMPLES};

pub const PROTOCOL_VERSION: u8 = 3;

/// One reply line: the command, then " value" per call. done() returns false and
/// removes the whole line when anything did not fit.
pub struct ReplyLine<'w, B: Storage> {
    out: &'w mut BufWriter<B>,
    start: usize,
    ok: bool,
}

impl<'w, B: Storage> ReplyLine<'w, B> {
    pub fn new(out: &'w mut BufWriter<B>, cmd: &[u8]) -> Self {
        let start = out.length();
        let ok = out.append(cmd);
        ReplyLine { out, start, ok }
    }

    pub fn u(&mut self, v: u32) -> &mut Self {
        self.ok = self.ok && self.out.append_char(b' ') && self.out.append_unsigned(v);
        self
    }

    pub fn s(&mut self, v: i32) -> &mut Self {
        self.ok = self.ok && self.out.append_char(b' ') && self.out.append_signed(v);
        self
    }

    pub fn text(&mut self, t: &[u8]) -> &mut Self {
        self.ok = self.ok && self.out.append_char(b' ') && self.out.append(t);
        self
    }

    pub fn done(&mut self) -> bool {
        if !self.ok {
            self.out.truncate(self.start);
        }
        self.ok
    }
}

// gvlvx calState bits

/// 0 idle, 1 requested, 2 running
pub const CAL_STATE_MASK: u8 = 0x03;
pub const CAL_STATE_IDLE: u8 = 0;
pub const CAL_STATE_REQUESTED: u8 = 1;
pub const CAL_STATE_RUNNING: u8 = 2;
/// early end stop since the last good calibration
pub const CAL_FLAG_EARLY_STOP: u8 = 0x04;
/// the last calibration did not succeed
pub const CAL_FLAG_LAST_FAILED: u8 = 0x08;

/// running wins over requested
pub fn compose_cal_state(
    running: bool,
    requested: bool,
    early_warn: bool,
    last_failed: bool,
) -> u8 {
    let mut v = if running {
        CAL_STATE_RUNNING
    } else if requested {
        CAL_STATE_REQUESTED
    } else {
        CAL_STATE_IDLE
    };
    if early_warn {
        v |= CAL_FLAG_EARLY_STOP;
    }
    if last_failed {
        v |= CAL_FLAG_LAST_FAILED;
    }
    v
}

/// gvlvx status in the gvlvd encoding: valve status, bit 7 (0x80) the
/// calibration flag of the valve, as in gvlvd. It is set only by staln and the
/// movement trigger (until that calibration ends); calibrations started by the
/// time trigger or by the first target change of a found valve run without it.
/// Whether a calibration is requested or running is calState & 3.
pub const STATUS_CALIBRATION_BIT: u8 = 0x80;

pub fn encode_valve_status(status: u8, calibration: bool) -> u8 {
    if calibration {
        status | STATUS_CALIBRATION_BIT
    } else {
        status
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ValveExtReply {
    pub index: u8,
    pub status: u8,
    pub position: u8,
    pub target: u8,
    /// mA
    pub mean_current: u16,
    pub opening_count: u32,
    pub closing_count: u32,
    pub deadzone_count: i32,
    pub calib_retries: u8,
    pub movements: u32,
    pub cal_state: u8,
    pub early_stops: u16,
    pub cmd_rejected: u16,
    pub last: MoveResult,
}

/// "gvlvx" + 20 numbers of up to 11 characters, separated by spaces.
pub const VALVE_EXT_REPLY_MAX_LEN: usize = 5 + 20 * 12;

pub fn format_valve_ext<B: Storage>(out: &mut BufWriter<B>, r: &ValveExtReply) -> bool {
    let mut line = ReplyLine::new(out, b"gvlvx");
    append_valve_ext_fields(&mut line, r).done()
}

/// the values of gvlvx, also the first ones of gvlvy
pub fn append_valve_ext_fields<'l, 'w, B: Storage>(
    line: &'l mut ReplyLine<'w, B>,
    r: &ValveExtReply,
) -> &'l mut ReplyLine<'w, B> {
    line.u(u32::from(r.index))
        .u(u32::from(r.status))
        .u(u32::from(r.position))
        .u(u32::from(r.target))
        .u(u32::from(r.mean_current))
        .u(r.opening_count)
        .u(r.closing_count)
        .s(r.deadzone_count)
        .u(u32::from(r.calib_retries))
        .u(r.movements)
        .u(u32::from(r.cal_state))
        .u(u32::from(r.early_stops))
        .u(u32::from(r.cmd_rejected))
        .u(u32::from(r.last.dir))
        .u(u32::from(r.last.requested_counts))
        .u(u32::from(r.last.counted_counts))
        .u(u32::from(r.last.stop_reason))
        .u(u32::from(r.last.peak_current))
        .u(r.last.duration_ms)
}

/// "gprof idx n c1:m1 ... cn:mn": up to 32 pairs of 5-digit numbers.
pub const PROFILE_REPLY_MAX_LEN: usize = 5 + 4 + 3 + PROFILE_SAMPLES as usize * 12;

pub fn format_profile<B: Storage>(
    out: &mut BufWriter<B>,
    index: u8,
    profile: &ProfileRecorder,
) -> bool {
    let start = out.length();
    let mut ok = ReplyLine::new(out, b"gprof")
        .u(u32::from(index))
        .u(u32::from(profile.size()))
        .done();
    for i in 0..profile.size() {
        let p = profile.at(i);
        ok = ok
            && out.append_char(b' ')
            && out.append_unsigned(u32::from(p.count))
            && out.append_char(b':')
            && out.append_unsigned(u32::from(p.current));
    }
    if !ok {
        out.truncate(start);
    }
    ok
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StatReply {
    pub uptime_seconds: u32,
    pub resets: u32,
    pub boot_reason: u8,
    pub rx_overflow: u32,
    pub parse_errors: u32,
    pub eeprom_state: u8,
}

// gstat eepState values
pub const EEP_STATE_OK: u8 = 0;
pub const EEP_STATE_PENDING: u8 = 1;
pub const EEP_STATE_WRITE_FAILED: u8 = 2;
pub const EEP_STATE_READ_FAILED: u8 = 3;

/// v1 "eepst x": 1 only when the configuration is stored. While a write is
/// pending, and while writing fails or is disabled after a failed read, it is 0:
/// the legacy ESP then does not take the configuration as saved (it waits up to
/// 60 s and restarts, which resets the STM), a v2 ESP reads the cause from gstat.
pub fn eepst_saved(eep_state: u8) -> u8 {
    u8::from(eep_state == EEP_STATE_OK)
}

pub const STAT_REPLY_MAX_LEN: usize = 5 + 6 * 12;

pub fn format_stat<B: Storage>(out: &mut BufWriter<B>, r: &StatReply) -> bool {
    let mut line = ReplyLine::new(out, b"gstat");
    append_stat_fields(&mut line, r).done()
}

/// the values of gstat, also the first ones of gstax
pub fn append_stat_fields<'l, 'w, B: Storage>(
    line: &'l mut ReplyLine<'w, B>,
    r: &StatReply,
) -> &'l mut ReplyLine<'w, B> {
    line.u(r.uptime_seconds)
        .u(r.resets)
        .u(u32::from(r.boot_reason))
        .u(r.rx_overflow)
        .u(r.parse_errors)
        .u(u32::from(r.eeprom_state))
}

/// "gcalx enable stepPct maxmA"
pub fn format_escalation<B: Storage>(out: &mut BufWriter<B>, c: &EscalationConfig) -> bool {
    ReplyLine::new(out, b"gcalx")
        .u(u32::from(c.enable))
        .u(u32::from(c.step_pct))
        .u(u32::from(c.max_ma))
        .done()
}

/// "gmotx lowMin lowMax highMin highMax sopMin sopMax minCntMin minCntMax retrMin retrMax"
pub const MOTOR_LIMITS_REPLY_MAX_LEN: usize = 5 + 10 * 6;

pub fn format_motor_limits<B: Storage>(out: &mut BufWriter<B>) -> bool {
    let mut line = ReplyLine::new(out, b"gmotx");
    for r in [
        LOW_FAC_RANGE,
        HIGH_FAC_RANGE,
        START_ON_POWER_RANGE,
        MIN_COUNTS_RANGE,
        MAX_RETRIES_RANGE,
    ] {
        line.u(u32::from(r.min)).u(u32::from(r.max));
    }
    line.done()
}

/// "gproto 3"
pub fn format_protocol_version<B: Storage>(out: &mut BufWriter<B>) -> bool {
    ReplyLine::new(out, b"gproto")
        .u(u32::from(PROTOCOL_VERSION))
        .done()
}

/// "<cmd> ok" / "<cmd> err"
pub fn format_result<B: Storage>(out: &mut BufWriter<B>, cmd: &[u8], ok: bool) -> bool {
    let result: &[u8] = if ok { b"ok" } else { b"err" };
    ReplyLine::new(out, cmd).text(result).done()
}

/// "<cmd> <index> ok" / "<cmd> <index> err <code>"
/// (index -1 when the request had no valid index)
pub fn format_indexed_result<B: Storage>(
    out: &mut BufWriter<B>,
    cmd: &[u8],
    index: i32,
    error_code: u8,
) -> bool {
    let mut line = ReplyLine::new(out, cmd);
    line.s(index);
    if error_code == 0 {
        return line.text(b"ok").done();
    }
    line.text(b"err").u(u32::from(error_code)).done()
}

#[cfg(test)]
mod tests;
