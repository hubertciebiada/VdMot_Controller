//! Log sinks: when the RAM log is written to the file (flash wear), which events go to the file
//! and to syslog, rotation steps, gap lines and the syslog packet (port of `vdm/log_sink.h`).
//! Hardware-free.

use core::fmt::Write;

use crate::common::{c_str, elapsed_ms, fmt_trunc, TextBuf};
use crate::event_log::{event_code_name, format_utc_timestamp, syslog_severity, Event, Severity};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LogFlushParams {
    /// every 5 min
    pub period_ms: u32,
    /// Warning+ waits at most this long after the last attempt; also the back-off after a
    /// failed attempt
    pub urgent_gap_ms: u32,
    /// the logger sets half its RAM ring (its event capacity)
    pub backlog_high: u32,
    /// LogWriteFailed at most hourly
    pub failure_report_ms: u32,
}

impl Default for LogFlushParams {
    fn default() -> Self {
        Self {
            period_ms: 300_000,
            urgent_gap_ms: 10_000,
            backlog_high: 256,
            failure_report_ms: 3_600_000,
        }
    }
}

/// The RAM ring is the buffer: the file cursor lags behind it and the backlog is written only
/// when [`due`](Self::due) says so.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LogFlushPolicy {
    p: LogFlushParams,
    /// the last attempt, begin() before the first
    last_ms: u32,
    last_failed: bool,
    attempts: u32,
    failures: u32,
    reported: bool,
    report_ms: u32,
}

impl LogFlushPolicy {
    /// C++ default for p: `LogFlushParams()` ([`Default`]).
    pub fn new(p: LogFlushParams) -> Self {
        Self {
            p,
            ..Self::default()
        }
    }

    /// Boot: the periodic timer starts here.
    pub fn begin(&mut self, now_ms: u32) {
        self.last_ms = now_ms;
        self.last_failed = false;
    }

    /// `backlog` = events after the file cursor (all severities); `urgent` = a Warning+ event is
    /// among them; `requested` = logger::requestFlush() or a forced flush.
    ///  backlog 0 -> false; last attempt failed and < urgent_gap_ms ago -> false;
    ///  requested or backlog >= backlog_high -> true; urgent and >= urgent_gap_ms since the last
    ///  attempt -> true; >= period_ms since the last attempt -> true.
    pub fn due(&self, backlog: u32, urgent: bool, requested: bool, now_ms: u32) -> bool {
        if backlog == 0 {
            return false;
        }
        let since = elapsed_ms(now_ms, self.last_ms);
        if self.last_failed && since < self.p.urgent_gap_ms {
            return false;
        }
        if requested || backlog >= self.p.backlog_high {
            return true;
        }
        if urgent && since >= self.p.urgent_gap_ms {
            return true;
        }
        since >= self.p.period_ms
    }

    /// An attempt ended (`ok`: every line it tried was written). Returns true when a
    /// LogWriteFailed event is due (first failure, then at most every failure_report_ms).
    pub fn on_attempt(&mut self, ok: bool, now_ms: u32) -> bool {
        self.attempts = self.attempts.wrapping_add(1);
        self.last_ms = now_ms;
        self.last_failed = !ok;
        if ok {
            return false;
        }
        self.failures = self.failures.wrapping_add(1);
        if self.reported && elapsed_ms(now_ms, self.report_ms) < self.p.failure_report_ms {
            return false;
        }
        self.reported = true;
        self.report_ms = now_ms;
        true
    }

    pub fn attempts(&self) -> u32 {
        self.attempts
    }

    pub fn failures(&self) -> u32 {
        self.failures
    }

    pub fn attempted(&self) -> bool {
        self.attempts != 0
    }

    pub fn last_attempt_ms(&self) -> u32 {
        self.last_ms
    }
}

/// Info and above go to the file; Debug stays in RAM, serial and syslog.
pub fn file_wants_severity(s: Severity) -> bool {
    s >= Severity::Info
}

/// Syslog level 1 Warning+, 2 Info+, 3 everything, else nothing.
pub fn syslog_wants(level: u8, s: Severity) -> bool {
    match level {
        1 => s >= Severity::Warning,
        2 => s >= Severity::Info,
        3 => true,
        _ => false,
    }
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogFileStep {
    Append = 0,
    Rotate = 1,
    Defer = 2,
}

///  file_size + line_len <= max_bytes -> Append; !rotation_blocked -> Rotate;
///  file_size + line_len <= max_bytes + slack_bytes -> Append (a reader holds a file); else
///  Defer.
pub fn log_file_step(
    file_size: usize,
    line_len: usize,
    rotation_blocked: bool,
    max_bytes: usize,
    slack_bytes: usize,
) -> LogFileStep {
    // size_t arithmetic, wrapping like the C++
    let after = file_size.wrapping_add(line_len);
    if after <= max_bytes {
        return LogFileStep::Append;
    }
    if !rotation_blocked {
        return LogFileStep::Rotate;
    }
    if after <= max_bytes.wrapping_add(slack_bytes) {
        LogFileStep::Append
    } else {
        LogFileStep::Defer
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LogGap {
    pub from: u32,
    pub to: u32,
}

/// Events between the file cursor and the oldest event still in the ring. first_seq 0 (empty
/// ring) or first_seq <= cursor + 1 -> None (the C++ false).
pub fn detect_log_gap(cursor: u32, first_seq: u32) -> Option<LogGap> {
    // uint32 arithmetic, wrapping like the C++
    let next = cursor.wrapping_add(1);
    if first_seq == 0 || first_seq <= next {
        return None;
    }
    Some(LogGap {
        from: next,
        to: first_seq - 1,
    })
}

/// "#<from>-<to> gap: <n> events not written"; returns the length (truncated to fit).
pub fn format_log_gap_line(g: &LogGap, out: &mut [u8]) -> usize {
    let count = g.to.wrapping_sub(g.from).wrapping_add(1);
    fmt_trunc(
        out,
        format_args!("#{}-{} gap: {} events not written", g.from, g.to, count),
    )
}

/// RFC 5424 packet: "<PRI>1 TIMESTAMP HOSTNAME vdmot - <code name> - <msg>", facility local0,
/// TIMESTAMP "-" before the clock is set, HOSTNAME "-" when empty. `msg` and `host` are C
/// strings. Returns the length (truncated to fit), 0 for an empty `out`.
pub fn format_syslog(e: &Event, msg: &[u8], host: &[u8], out: &mut [u8]) -> usize {
    let mut ts = [0u8; 26];
    let ts: &[u8] = if e.epoch != 0 {
        // "...T08:13:40+00:00" -> "...T08:13:40Z"
        format_utc_timestamp(e.epoch, &mut ts);
        ts[19] = b'Z';
        &ts[..20]
    } else {
        b"-"
    };
    let host = match c_str(host) {
        [] => b"-".as_slice(),
        h => h,
    };
    let pri = 16 * 8 + u32::from(syslog_severity(e.severity));
    let mut w = TextBuf::new(out);
    let _ = write!(w, "<{pri}>1 ");
    w.push_bytes(ts);
    w.push(b' ');
    w.push_bytes(host);
    w.push_bytes(b" vdmot - ");
    w.push_bytes(event_code_name(e.code).as_bytes());
    w.push_bytes(b" - ");
    w.push_bytes(c_str(msg));
    w.len()
}

#[cfg(test)]
mod tests;
