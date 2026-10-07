//! Logging (C++ `logger.cpp`, `logger.h`): structured events in a RAM ring (core [`EventLog`]),
//! mirrored to the console, written to a rotating file log on LittleFS and sent to an optional
//! UDP syslog server (RFC 5424).
//!
//! The C++ module kept two groups of file statics, and they are three parts here:
//! - [`LoggerShared`]: what the C++ guarded by the log mutex (the ring, the sink configuration,
//!   the flush request, the statistics), the data the logger publishes for the other tasks.
//!   `main` creates it; readers (web, MQTT) use it directly.
//! - [`Logger`]: `log()` and `logSev()` with the ports they need (uptime, wall clock, console),
//!   shared by every thread (any task logs).
//! - [`LogSinks`]: the app-task part (the C++ "app task only" statics): the syslog and file
//!   cursors, the flush policy, LittleFS and the UDP socket. Slow I/O happens only here.
//!
//! No lock is held while another is taken, and none during file or socket I/O.

use std::sync::{Mutex, MutexGuard, PoisonError};

use vdm_esp_core::common::{build_hostname, elapsed_ms, Text, NO_VALVE, STATION_NAME_MAX};
use vdm_esp_core::event_log::{
    event_default_severity, format_event_line, format_event_message, make_event, Event, EventCode,
    EventFilter, EventLog, Severity,
};
use vdm_esp_core::json_api::LogHealthInfo;
use vdm_esp_core::log_sink::{
    detect_log_gap, file_wants_severity, format_log_gap_line, format_syslog, log_file_step,
    syslog_wants, LogFileStep, LogFlushParams, LogFlushPolicy, LogGap,
};

use crate::port::{Clock, Console, Fs, FsFile, OpenMode, Udp, WallClock};

/// Events of the RAM ring, which is also the buffer of the file sink: the file is written when
/// half of it waits (the app task looks every 100 ms). 48 B per event on the device: the firmware
/// keeps 32 (`VDM_EVENT_CAPACITY` of platformio.ini; with 512 the WT32-ETH01 had 2 KB of heap
/// left once the network was up and Ethernet dropped every frame). The file keeps the history;
/// the glue tests use 512, as the C++ native tests did.
#[cfg(not(test))]
pub const EVENT_CAPACITY: usize = 32;
/// Events of the RAM ring under the glue tests (the C++ native build's 512).
#[cfg(test)]
pub const EVENT_CAPACITY: usize = 512;
const _: () = assert!(
    EVENT_CAPACITY >= 32,
    "the file sink needs a backlog of at least 16 events"
);

/// The log file (binding): up to [`LOG_FILE_MAX`] bytes, then renamed to [`LOG_FILE_OLD`]
/// (replacing it), so at most 2 x 64 KB on flash.
pub const LOG_FILE: &str = "/log/events.log";
/// The previous log file.
pub const LOG_FILE_OLD: &str = "/log/events.1.log";
/// Size at which the log file rotates.
pub const LOG_FILE_MAX: usize = 64 * 1024;
/// Growth past [`LOG_FILE_MAX`] while a reader (`GET /api/log`) blocks the rotation.
pub const LOG_FILE_SLACK: usize = 8192;
/// Syslog follows the ring with its own cursor, at most this many events per pass. The file
/// cursor moves per line, only after the line was written; events the ring overwrote before
/// they reached the file become one gap line (`#<from>-<to> gap: ...`, `stats().lost`).
pub const PENDING_LINES: usize = 32;

/// Before SNTP the clock counts from 1970: an epoch before 2020-01-01 is reported as 0.
const EPOCH_VALID_FROM: i64 = 1_577_836_800;
/// Events per ring read of the sinks.
const BATCH: usize = 4;

/// What the logger asks of the other glue modules (C++ sibling calls). The firmware wiring
/// implements it, the tests fake it.
pub trait LoggerHost {
    /// `storage::fsReady()`: LittleFS is mounted.
    fn fs_ready(&self) -> bool;
}

impl<T: LoggerHost + ?Sized> LoggerHost for &T {
    fn fs_ready(&self) -> bool {
        (**self).fs_ready()
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // a panic aborts the firmware, so a poisoned lock exists only in a failing test
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The C++ statics guarded by the log mutex.
struct LogState {
    ring: Box<EventLog<EVENT_CAPACITY>>,
    syslog_level: u8,
    syslog_server: u32,
    syslog_port: u16,
    persist: bool,
    /// syslog HOSTNAME: the station name made a host name
    hostname: Text<STATION_NAME_MAX>,
    /// the last Warning+ event
    urgent_seq: u32,
    flush_requested: bool,
    /// `backlog` is computed by [`LoggerShared::stats`]
    stats: LogHealthInfo,
    stats_cursor: u32,
    last_attempt_ms: u32,
}

/// A copy of the ring for the API ([`LoggerShared::read`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LogRead {
    /// Events copied.
    pub count: usize,
    /// The `since_seq` of the next read (core [`EventLog::read`]).
    pub next_since: u32,
    /// Oldest event in the ring, 0 when empty.
    pub first_seq: u32,
    /// Newest event in the ring, 0 when empty.
    pub last_seq: u32,
    /// Events the ring overwrote.
    pub dropped: u32,
}

/// The data the logger publishes: the RAM ring, the sink configuration, the flush request and
/// the statistics (one mutex, the C++ log mutex). `main` creates it at boot (the ring is a boot
/// allocation, D§9).
pub struct LoggerShared {
    state: Mutex<LogState>,
}

impl Default for LoggerShared {
    fn default() -> Self {
        Self::new()
    }
}

impl LoggerShared {
    /// An empty ring; syslog off (level 0, port 514, host name "VdMot"), the file sink on.
    pub fn new() -> Self {
        let mut hostname = Text::new();
        let _ = hostname.extend_from_slice(b"VdMot");
        LoggerShared {
            state: Mutex::new(LogState {
                ring: Box::default(),
                syslog_level: 0,
                syslog_server: 0,
                syslog_port: 514,
                persist: true,
                hostname,
                urgent_seq: 0,
                flush_requested: false,
                stats: LogHealthInfo::default(),
                stats_cursor: 0,
                last_attempt_ms: 0,
            }),
        }
    }

    /// Filtered copy for the API (core [`EventLog::read`]) with the ring's bounds.
    pub fn read(&self, f: &EventFilter, out: &mut [Event]) -> LogRead {
        let st = lock(&self.state);
        let mut next_since = 0;
        let count = st.ring.read(f, out, &mut next_since);
        LogRead {
            count,
            next_since,
            first_seq: st.ring.first_seq(),
            last_seq: st.ring.last_seq(),
            dropped: st.ring.dropped(),
        }
    }

    /// Seq of the newest event, 0 when the ring is empty.
    pub fn last_seq(&self) -> u32 {
        lock(&self.state).ring.last_seq()
    }

    /// Events after `since_seq`, oldest first (the MQTT task calls this with its own cursor).
    /// Returns the count; `next_since` as core [`EventLog::read`].
    pub fn read_since(&self, since_seq: u32, out: &mut [Event], next_since: &mut u32) -> usize {
        let f = EventFilter {
            since_seq,
            ..EventFilter::default()
        };
        lock(&self.state).ring.read(&f, out, next_since)
    }

    /// Sink configuration (from the config): syslog level, server and port, the file on or off,
    /// and the syslog HOSTNAME field (the station name made a host name by core
    /// `build_hostname`, at most [`STATION_NAME_MAX`] characters).
    pub fn configure(
        &self,
        syslog_level: u8,
        syslog_server: u32,
        syslog_port: u16,
        persist: bool,
        hostname: &[u8],
    ) {
        let mut host = [0u8; STATION_NAME_MAX + 1];
        let n = build_hostname(hostname, &mut host);
        let mut st = lock(&self.state);
        st.syslog_level = syslog_level;
        st.syslog_server = syslog_server;
        st.syslog_port = syslog_port;
        st.persist = persist;
        st.hostname.clear();
        let _ = st
            .hostname
            .extend_from_slice(host.get(..n).unwrap_or_default());
    }

    /// Any task: the next [`LogSinks::service`] writes the whole backlog (`GET /api/log`).
    pub fn request_flush(&self) {
        lock(&self.state).flush_requested = true;
    }

    /// Sink statistics for `/api/health`.
    pub fn stats(&self, now_ms: u32) -> LogHealthInfo {
        let st = lock(&self.state);
        let mut s = st.stats;
        s.persist = st.persist;
        s.backlog = st.ring.last_seq().wrapping_sub(st.stats_cursor);
        s.last_flush_age_s = if s.flushed {
            elapsed_ms(now_ms, st.last_attempt_ms) / 1000
        } else {
            0
        };
        s
    }
}

/// The epoch an event records: 0 while the clock is not set.
fn event_epoch(epoch: i64) -> u32 {
    if epoch >= EPOCH_VALID_FROM {
        // the C++ static_cast<uint32_t>(tv_sec)
        epoch as u32
    } else {
        0
    }
}

/// Records events (any thread, also before the network is up). One per firmware, shared by
/// every task (`&'static`).
pub struct Logger<'a, C, W, K> {
    shared: &'a LoggerShared,
    clock: C,
    wall: W,
    console: K,
}

impl<'a, C: Clock, W: WallClock, K: Console> Logger<'a, C, W, K> {
    /// The logger over its published data and the ports of `log()`.
    pub fn new(shared: &'a LoggerShared, clock: C, wall: W, console: K) -> Self {
        Logger {
            shared,
            clock,
            wall,
            console,
        }
    }

    /// The published data (ring, statistics).
    pub fn shared(&self) -> &'a LoggerShared {
        self.shared
    }

    /// Records `e` with uptime and epoch filled (seq assigned by the ring), mirrors its line to
    /// the console and returns the seq. A Warning+ event makes the file sink urgent.
    pub fn log_event(&self, e: &Event) -> u32 {
        let mut ev = e.clone();
        ev.uptime_s = self.clock.uptime_s();
        ev.epoch = event_epoch(self.wall.epoch());
        let seq = {
            let mut st = lock(&self.shared.state);
            let seq = st.ring.append(&ev);
            if ev.severity >= Severity::Warning {
                st.urgent_seq = seq;
            }
            seq
        };
        ev.seq = seq;
        let mut line = [0u8; 160];
        let n = format_event_line(&ev, &mut line);
        self.console.line(line.get(..n).unwrap_or_default());
        seq
    }

    /// An event with the default severity of `code` (C++ `log(code, valve, arg1, arg2,
    /// text)`; C++ defaults: valve [`NO_VALVE`], arg1 0, arg2 0, text ""). `text` is a C string,
    /// cut to 23 characters.
    pub fn log(&self, code: EventCode, valve: u8, arg1: i32, arg2: i32, text: &[u8]) -> u32 {
        self.log_event(&make_event(
            code,
            event_default_severity(code),
            valve,
            arg1,
            arg2,
            text,
        ))
    }

    /// The same with an explicit severity (codes whose severity depends on the arguments).
    pub fn log_sev(
        &self,
        code: EventCode,
        sev: Severity,
        valve: u8,
        arg1: i32,
        arg2: i32,
        text: &[u8],
    ) -> u32 {
        self.log_event(&make_event(code, sev, valve, arg1, arg2, text))
    }
}

/// Outcome of one line of a flush attempt (LogWriteFailed arg1).
#[repr(i32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    Ok = 0,
    /// the log file could not be opened
    Open = 1,
    /// a short or failed write
    Write = 2,
    /// the log file could not be opened again after a rotation
    Rotate = 3,
    /// a reader blocks the rotation and the slack is used up
    Size = 4,
}

/// The log file during one flush attempt.
struct FileSink<T> {
    file: Option<T>,
    rotation_blocked: bool,
}

/// The app-task part of the logger: syslog and the file sink (C++ `service`, `flush` and the
/// "app task only" statics). [`LogSinks::new`] is the C++ `begin()`: the flush policy starts.
pub struct LogSinks<'a, C, W, K, F, U, H> {
    logger: &'a Logger<'a, C, W, K>,
    fs: F,
    udp: U,
    host: H,
    /// syslog: every event is looked at once
    syslog_cursor: u32,
    /// file: moved per line, only after it was written
    file_cursor: u32,
    policy: LogFlushPolicy,
}

impl<'a, C: Clock, W: WallClock, K: Console, F: Fs, U: Udp, H: LoggerHost>
    LogSinks<'a, C, W, K, F, U, H>
{
    /// The sinks of `logger`; the periodic flush timer starts now.
    pub fn new(logger: &'a Logger<'a, C, W, K>, fs: F, udp: U, host: H) -> Self {
        let mut policy = LogFlushPolicy::new(LogFlushParams {
            backlog_high: (EVENT_CAPACITY / 2) as u32,
            ..LogFlushParams::default()
        });
        policy.begin(logger.clock.now_ms());
        LogSinks {
            logger,
            fs,
            udp,
            host,
            syslog_cursor: 0,
            file_cursor: 0,
            policy,
        }
    }

    /// App task, every pass: syslog (when the network is up) and the file backlog when the
    /// flush policy says so (every 5 min, 10 s after a Warning+, at once for half the ring or a
    /// request). Debug events never go to the file.
    pub fn service(&mut self, net_up: bool) {
        self.service_syslog(net_up);
        self.service_file(false);
    }

    /// App task, restart path: writes the whole file backlog now.
    pub fn flush(&mut self) {
        self.service_file(true);
    }

    /// Syslog: at most [`PENDING_LINES`] events per pass, sent while the network is up; the
    /// cursor always advances (no backlog is kept for syslog).
    fn service_syslog(&mut self, net_up: bool) {
        let (level, server, port, host) = {
            let st = lock(&self.logger.shared.state);
            (
                st.syslog_level,
                st.syslog_server,
                st.syslog_port,
                st.hostname.clone(),
            )
        };
        // level 0 (off) and unknown levels: syslog_wants() takes nothing
        let mut batch: [Event; BATCH] = Default::default();
        for _ in 0..PENDING_LINES / BATCH {
            let mut next = self.syslog_cursor;
            let n = self
                .logger
                .shared
                .read_since(self.syslog_cursor, &mut batch, &mut next);
            if n == 0 {
                break;
            }
            for e in batch.iter().take(n) {
                if net_up && syslog_wants(level, e.severity) {
                    self.send_syslog(e, server, port, &host);
                }
            }
            self.syslog_cursor = next;
        }
    }

    fn send_syslog(&mut self, e: &Event, server: u32, port: u16, host: &[u8]) {
        if server == 0 || port == 0 {
            return;
        }
        let mut msg = [0u8; 120];
        let m = format_event_message(e, &mut msg);
        let mut pkt = [0u8; 240];
        let len = format_syslog(e, msg.get(..m).unwrap_or_default(), host, &mut pkt);
        // a datagram the stack cannot take is lost, as with WiFiUDP
        self.udp
            .send_to(server, port, pkt.get(..len).unwrap_or_default());
    }

    /// File sink of one pass: `force` writes the backlog now (restart path).
    fn service_file(&mut self, force: bool) {
        let (persist, requested, last, urgent_seq) = {
            let mut st = lock(&self.logger.shared.state);
            let requested = st.flush_requested || force;
            st.flush_requested = false;
            (st.persist, requested, st.ring.last_seq(), st.urgent_seq)
        };
        if !persist || !self.host.fs_ready() {
            // nothing to write to: the RAM log is all there is
            self.file_cursor = last;
            lock(&self.logger.shared.state).stats_cursor = last;
            return;
        }
        let backlog = last.wrapping_sub(self.file_cursor);
        let now = self.logger.clock.now_ms();
        let due = if force {
            backlog != 0
        } else {
            self.policy
                .due(backlog, urgent_seq > self.file_cursor, requested, now)
        };
        if due {
            self.write_backlog(now);
        }
    }

    /// Appends one line (with its '\n'), opening the file lazily and rotating it as needed.
    /// [`Step::Ok`] only when the whole line was written.
    fn append_line(&mut self, sink: &mut FileSink<F::File>, line: &[u8]) -> Step {
        let mut file = match sink.file.take() {
            Some(f) => f,
            None => match self.fs.open(LOG_FILE, OpenMode::Append) {
                Some(f) => f,
                None => return Step::Open,
            },
        };
        loop {
            let step = log_file_step(
                file.size() as usize,
                line.len(),
                sink.rotation_blocked,
                LOG_FILE_MAX,
                LOG_FILE_SLACK,
            );
            match step {
                LogFileStep::Append => break,
                LogFileStep::Defer => {
                    sink.file = Some(file);
                    return Step::Size;
                }
                LogFileStep::Rotate => {
                    // The rename replaces the old file. LittleFS refuses to rename an open
                    // file (a reader of /api/log): then the file may grow by the slack and the
                    // rotation is tried again at the next attempt.
                    drop(file);
                    sink.rotation_blocked = !self.fs.rename(LOG_FILE, LOG_FILE_OLD);
                    file = match self.fs.open(LOG_FILE, OpenMode::Append) {
                        Some(f) => f,
                        None => return Step::Rotate,
                    };
                }
            }
        }
        let written = file.write(line);
        sink.file = Some(file);
        if written == line.len() {
            Step::Ok
        } else {
            Step::Write
        }
    }

    /// The gap line of `gap` (with its '\n').
    fn append_gap_line(&mut self, sink: &mut FileSink<F::File>, gap: &LogGap) -> Step {
        let mut line = [0u8; 64];
        let len = format_log_gap_line(gap, &mut line[..63]);
        line[len] = b'\n';
        self.append_line(sink, &line[..=len])
    }

    /// The file line of `e` (with its '\n'), at most 159 characters before it.
    fn append_event_line(&mut self, sink: &mut FileSink<F::File>, e: &Event) -> Step {
        let mut line = [0u8; 161];
        let len = format_event_line(e, &mut line[..160]);
        line[len] = b'\n';
        self.append_line(sink, &line[..=len])
    }

    /// One batch read from the ring: its gap line and its lines; the events lost before it are
    /// added to `lost`.
    fn write_batch(
        &mut self,
        sink: &mut FileSink<F::File>,
        batch: &[Event],
        lost: &mut u32,
    ) -> Step {
        let first = batch.first().map_or(0, |e| e.seq);
        if let Some(gap) = detect_log_gap(self.file_cursor, first) {
            let step = self.append_gap_line(sink, &gap);
            if step != Step::Ok {
                return step;
            }
            *lost = lost.wrapping_add(gap.to.wrapping_sub(gap.from).wrapping_add(1));
            self.file_cursor = gap.to;
        }
        for e in batch {
            if file_wants_severity(e.severity) {
                let step = self.append_event_line(sink, e);
                if step != Step::Ok {
                    return step;
                }
            }
            self.file_cursor = e.seq;
        }
        Step::Ok
    }

    /// Writes the file backlog (events after the file cursor up to the last seq at the start).
    /// One open per attempt, nothing opened for a Debug-only backlog. Events the ring dropped
    /// before they were written become a gap line, checked per batch: the ring also drops
    /// events while the file is written.
    fn write_backlog(&mut self, now_ms: u32) {
        let upper = self.logger.shared.last_seq();
        let mut sink = FileSink {
            file: None,
            rotation_blocked: false,
        };
        let mut failed = Step::Ok;
        let mut lost: u32 = 0;
        let mut batch: [Event; BATCH] = Default::default();
        while failed == Step::Ok && self.file_cursor < upper {
            let mut next = self.file_cursor;
            let n = self
                .logger
                .shared
                .read_since(self.file_cursor, &mut batch, &mut next);
            if n == 0 {
                break;
            }
            failed = self.write_batch(&mut sink, batch.get(..n).unwrap_or_default(), &mut lost);
        }
        // LittleFS commits on close
        drop(sink.file.take());
        let report = self.policy.on_attempt(failed == Step::Ok, now_ms);
        let lost_total = {
            let mut st = lock(&self.logger.shared.state);
            st.stats.lost = st.stats.lost.wrapping_add(lost);
            st.stats.flushes = self.policy.attempts();
            st.stats.failures = self.policy.failures();
            st.stats.flushed = true;
            st.last_attempt_ms = now_ms;
            st.stats_cursor = self.file_cursor;
            st.stats.lost
        };
        if report {
            // the C++ static_cast<int32_t>
            self.logger.log(
                EventCode::LogWriteFailed,
                NO_VALVE,
                failed as i32,
                lost_total as i32,
                b"",
            );
        }
    }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_mut;
