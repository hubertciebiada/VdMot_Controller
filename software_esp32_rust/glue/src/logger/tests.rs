//! `test_logger.cpp`: RAM log, console mirror, file sink (flush policy, rotation, gap lines,
//! write failures), syslog, statistics; then the new Rust cases (an open reader blocking the
//! rotation, a failed reopen after a rotation, events logged during a write, two boots in one
//! file, refused datagrams, the constants).
#![allow(clippy::large_stack_frames, clippy::large_stack_arrays)]

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use super::*;
use crate::port::FsEntry;
use crate::testkit::fs::FakeFile;
use crate::testkit::{Device, FakeBoard, FakeClock, FakeConsole, FakeFs, FakeUdp, FakeWall, Reset};
use vdm_esp_core::event_log::event_code_name;

/// The C++ sibling `storage::fsReady()` (true unless a case takes it away).
#[derive(Default)]
pub(super) struct Host {
    pub(super) fs_down: AtomicBool,
}

impl LoggerHost for Host {
    fn fs_ready(&self) -> bool {
        !self.fs_down.load(Ordering::Relaxed)
    }
}

pub(super) type TestLogger<'a> = Logger<'a, FakeClock, FakeWall, FakeConsole>;
pub(super) type TestSinks<'a, F = FakeFs> =
    LogSinks<'a, FakeClock, FakeWall, FakeConsole, F, FakeUdp, &'a Host>;

pub(super) fn logger<'a>(dev: &Device, shared: &'a LoggerShared) -> TestLogger<'a> {
    Logger::new(
        shared,
        dev.clock.clone(),
        dev.wall.clone(),
        dev.console.clone(),
    )
}

pub(super) fn sinks<'a>(dev: &Device, lg: &'a TestLogger<'a>, host: &'a Host) -> TestSinks<'a> {
    LogSinks::new(lg, dev.fs.clone(), dev.udp.clone(), host)
}

/// IPv4 in lwIP order (the C++ `IPAddress(a, b, c, d)`).
pub(super) fn ip(a: u8, b: u8, c: u8, d: u8) -> u32 {
    u32::from_le_bytes([a, b, c, d])
}

pub(super) fn line_of(e: &Event) -> Vec<u8> {
    let mut line = [0u8; 160];
    let n = format_event_line(e, &mut line);
    line[..n].to_vec()
}

/// Every event in the ring, oldest first.
pub(super) fn all(shared: &LoggerShared) -> Vec<Event> {
    let mut out = vec![Event::default(); EVENT_CAPACITY];
    let mut next = 0;
    let n = shared.read_since(0, &mut out, &mut next);
    out.truncate(n);
    out
}

/// The file lines of the events in the ring with seq in [from, to] (Info and above).
pub(super) fn lines_of(shared: &LoggerShared, from: u32, to: u32) -> Vec<u8> {
    let mut s = Vec::new();
    for e in all(shared) {
        if e.seq >= from && e.seq <= to && e.severity != Severity::Debug {
            s.extend_from_slice(&line_of(&e));
            s.push(b'\n');
        }
    }
    s
}

pub(super) fn mount_fs(dev: &Device) {
    assert!(dev.fs.mount());
    assert!(dev.fs.mkdir("/log"));
}

pub(super) fn log_file(dev: &Device) -> Vec<u8> {
    dev.fs.read(LOG_FILE).unwrap_or_default()
}

pub(super) fn count_lines(s: &[u8]) -> usize {
    s.iter().filter(|&&c| c == b'\n').count()
}

pub(super) fn info(lg: &TestLogger<'_>, n: i32) {
    for i in 0..n {
        lg.log(EventCode::NetUp, NO_VALVE, i, 0, b"");
    }
}

#[test]
fn log_seq_from_1_uptime_and_wall_clock_filled_the_line_goes_to_the_console() {
    let board = FakeBoard::new();
    let dev = board.boot();
    let shared = LoggerShared::default();
    let lg = logger(&dev, &shared);
    dev.clock.set_ms(65000);
    dev.wall.set(1_790_136_000);
    assert_eq!(lg.log(EventCode::NetUp, NO_VALVE, 1, 0, b"10.0.0.2"), 1);
    assert_eq!(
        lg.log_sev(EventCode::Boot, Severity::Warning, 3, -1, 2, b""),
        2
    );
    let ev = all(&shared);
    assert_eq!(ev.len(), 2);
    assert_eq!(ev[0].uptime_s, 65);
    assert_eq!(ev[0].epoch, 1_790_136_000);
    assert_eq!(ev[0].severity, event_default_severity(EventCode::NetUp));
    assert_eq!(ev[0].text.as_slice(), b"10.0.0.2");
    assert_eq!(ev[1].severity, Severity::Warning);
    assert_eq!((ev[1].valve, ev[1].arg1, ev[1].arg2), (3, -1, 2));
    assert_eq!(shared.last_seq(), 2);
    assert_eq!(lg.shared().last_seq(), 2);
    assert_eq!(dev.console.lines(), vec![line_of(&ev[0]), line_of(&ev[1])]);
    assert!(dev.console.lines()[0].starts_with(b"#1 "));
}

#[test]
fn log_works_before_begin_the_epoch_is_0_before_2020() {
    let board = FakeBoard::new();
    let dev = board.boot();
    let shared = LoggerShared::new();
    let lg = logger(&dev, &shared);
    dev.wall.set(1_577_836_799); // 2019-12-31T23:59:59Z
    lg.log(EventCode::Boot, NO_VALVE, 0, 0, b"");
    dev.wall.set(1_577_836_800);
    lg.log(EventCode::Boot, NO_VALVE, 0, 0, b"");
    let ev = all(&shared);
    assert_eq!(ev.len(), 2);
    assert_eq!(ev[0].epoch, 0);
    assert_eq!(ev[1].epoch, 1_577_836_800);
}

#[test]
fn read_filter_first_and_last_seq() {
    let board = FakeBoard::new();
    let dev = board.boot();
    let shared = LoggerShared::new();
    let lg = logger(&dev, &shared);
    for i in 0..5u8 {
        lg.log(EventCode::Boot, i, 0, 0, b"");
    }
    let f = EventFilter {
        since_seq: 2,
        ..EventFilter::default()
    };
    let mut out = vec![Event::default(); 10];
    let r = shared.read(&f, &mut out);
    assert_eq!(r.count, 3);
    assert_eq!(out[0].seq, 3);
    assert_eq!(r.first_seq, 1);
    assert_eq!(r.last_seq, 5);
    assert_eq!(r.dropped, 0);
    assert_eq!(r.next_since, 5);
}

#[test]
fn file_info_events_wait_in_ram_for_the_5_min_flush_then_one_append() {
    let board = FakeBoard::new();
    let dev = board.boot();
    mount_fs(&dev);
    let shared = LoggerShared::new();
    let lg = logger(&dev, &shared);
    let host = Host::default();
    let mut sk = sinks(&dev, &lg, &host);
    assert!(!shared.stats(0).flushed);
    for i in 0..20u64 {
        dev.clock.set_ms(i * 10000);
        info(&lg, 1);
        sk.service(false);
    }
    dev.clock.set_ms(299_999);
    sk.service(false);
    assert_eq!(dev.fs.knobs().opens, 0);
    assert!(!dev.fs.exists(LOG_FILE));
    let s = shared.stats(299_999);
    assert!(s.persist);
    assert_eq!(s.backlog, 20);
    assert_eq!(s.flushes, 0);
    dev.clock.set_ms(300_000);
    sk.service(false);
    assert_eq!(dev.fs.knobs().write_opens, 1);
    assert_eq!(dev.fs.knobs().writes, 20);
    assert_eq!(log_file(&dev), lines_of(&shared, 1, 20));
    let s = shared.stats(305_999);
    assert_eq!(s.backlog, 0);
    assert_eq!(s.flushes, 1);
    assert!(s.flushed);
    assert_eq!(s.last_flush_age_s, 5);
    assert_eq!(s.failures, 0);
    assert_eq!(s.lost, 0);
    sk.service(false); // nothing new
    assert_eq!(dev.fs.knobs().write_opens, 1);
}

#[test]
fn file_a_warning_3_s_after_an_attempt_is_written_10_s_after_that_attempt() {
    let board = FakeBoard::new();
    let dev = board.boot();
    mount_fs(&dev);
    let shared = LoggerShared::new();
    let lg = logger(&dev, &shared);
    let host = Host::default();
    let mut sk = sinks(&dev, &lg, &host);
    dev.clock.set_ms(1000);
    info(&lg, 1);
    shared.request_flush();
    sk.service(false);
    assert_eq!(count_lines(&log_file(&dev)), 1);
    dev.clock.set_ms(4000);
    lg.log_sev(EventCode::NetDown, Severity::Warning, NO_VALVE, 1, 0, b"");
    dev.clock.set_ms(10_999);
    sk.service(false);
    assert_eq!(count_lines(&log_file(&dev)), 1);
    dev.clock.set_ms(11_000);
    sk.service(false);
    assert_eq!(log_file(&dev), lines_of(&shared, 1, 2));
}

#[test]
fn file_a_warning_10_s_after_the_last_attempt_is_written_in_the_next_pass() {
    let board = FakeBoard::new();
    let dev = board.boot();
    mount_fs(&dev);
    let shared = LoggerShared::new();
    let lg = logger(&dev, &shared);
    let host = Host::default();
    let mut sk = sinks(&dev, &lg, &host);
    dev.clock.set_ms(9999);
    lg.log_sev(EventCode::NetDown, Severity::Warning, NO_VALVE, 1, 0, b"");
    sk.service(false);
    assert!(!dev.fs.exists(LOG_FILE));
    dev.clock.set_ms(10_000);
    sk.service(false);
    assert_eq!(log_file(&dev), lines_of(&shared, 1, 1));
}

#[test]
fn file_256_waiting_events_are_written_at_once_255_are_not() {
    let board = FakeBoard::new();
    let dev = board.boot();
    mount_fs(&dev);
    let shared = LoggerShared::new();
    let lg = logger(&dev, &shared);
    let host = Host::default();
    let mut sk = sinks(&dev, &lg, &host);
    info(&lg, 255);
    sk.service(false);
    assert_eq!(dev.fs.knobs().opens, 0);
    info(&lg, 1);
    sk.service(false);
    assert_eq!(count_lines(&log_file(&dev)), 256);
    assert_eq!(log_file(&dev), lines_of(&shared, 1, 256));
}

#[test]
fn file_request_flush_writes_the_backlog_at_the_next_pass() {
    let board = FakeBoard::new();
    let dev = board.boot();
    mount_fs(&dev);
    let shared = LoggerShared::new();
    let lg = logger(&dev, &shared);
    let host = Host::default();
    let mut sk = sinks(&dev, &lg, &host);
    info(&lg, 2);
    sk.service(false);
    assert_eq!(dev.fs.knobs().opens, 0);
    shared.request_flush();
    assert_eq!(dev.fs.knobs().opens, 0);
    sk.service(false);
    assert_eq!(log_file(&dev), lines_of(&shared, 1, 2));
    info(&lg, 1);
    sk.service(false); // the request was used up
    assert_eq!(count_lines(&log_file(&dev)), 2);
}

#[test]
fn file_debug_events_stay_out_of_the_file_but_reach_the_console_and_syslog() {
    let board = FakeBoard::new();
    let dev = board.boot();
    mount_fs(&dev);
    let shared = LoggerShared::new();
    let lg = logger(&dev, &shared);
    let host = Host::default();
    let mut sk = sinks(&dev, &lg, &host);
    shared.configure(3, ip(192, 168, 1, 9), 514, true, b"VdMot");
    lg.log_sev(EventCode::TimeSynced, Severity::Debug, NO_VALVE, 5, 0, b"");
    shared.request_flush();
    sk.service(true);
    assert_eq!(dev.fs.knobs().opens, 0); // a Debug-only backlog opens nothing
    assert_eq!(dev.console.lines().len(), 1);
    let sent = dev.udp.sent();
    assert_eq!(sent.len(), 1);
    let name = event_code_name(EventCode::TimeSynced).as_bytes();
    assert!(sent[0].2.windows(name.len()).any(|w| w == name));
    assert_eq!(shared.stats(0).backlog, 0);
    info(&lg, 1);
    shared.request_flush();
    sk.service(true);
    assert_eq!(log_file(&dev), lines_of(&shared, 2, 2));
}

#[test]
fn file_nothing_without_persistence_or_without_a_file_system() {
    let board = FakeBoard::new();
    let dev = board.boot();
    mount_fs(&dev);
    let shared = LoggerShared::new();
    let lg = logger(&dev, &shared);
    let host = Host::default();
    let mut sk = sinks(&dev, &lg, &host);
    shared.configure(0, 0, 514, false, b"VdMot");
    info(&lg, 1);
    sk.flush();
    shared.request_flush();
    sk.service(false);
    assert!(!dev.fs.exists(LOG_FILE));
    assert!(!shared.stats(0).persist);
    assert_eq!(shared.stats(0).backlog, 0);
    shared.configure(0, 0, 514, true, b"VdMot");
    host.fs_down.store(true, Ordering::Relaxed);
    info(&lg, 1);
    sk.flush();
    assert!(!dev.fs.exists(LOG_FILE));
    assert!(shared.stats(0).persist);
    assert_eq!(shared.stats(0).backlog, 0);
    assert_eq!(shared.stats(0).flushes, 0);
    // The events of that time are not written later.
    host.fs_down.store(false, Ordering::Relaxed);
    info(&lg, 1);
    sk.flush();
    assert_eq!(log_file(&dev), lines_of(&shared, 3, 3));
}

#[test]
fn flush_writes_the_whole_backlog_now_nothing_when_there_is_none() {
    let board = FakeBoard::new();
    let dev = board.boot();
    mount_fs(&dev);
    let shared = LoggerShared::new();
    let lg = logger(&dev, &shared);
    let host = Host::default();
    let mut sk = sinks(&dev, &lg, &host);
    sk.flush();
    assert_eq!(shared.stats(0).flushes, 0);
    info(&lg, 300);
    sk.flush();
    assert_eq!(count_lines(&log_file(&dev)), 300);
    assert_eq!(shared.stats(0).flushes, 1);
}

#[test]
fn file_the_file_rotates_to_events_1_log_at_64_kib() {
    let board = FakeBoard::new();
    let dev = board.boot();
    mount_fs(&dev);
    let shared = LoggerShared::new();
    let lg = logger(&dev, &shared);
    let host = Host::default();
    let mut sk = sinks(&dev, &lg, &host);
    info(&lg, 1);
    let mut line = line_of(&all(&shared)[0]);
    line.push(b'\n');
    let old = vec![b'o'; 64 * 1024 - line.len() + 1];
    dev.fs.put(LOG_FILE, &old);
    dev.fs.put(LOG_FILE_OLD, b"old");
    sk.flush();
    assert_eq!(dev.fs.read(LOG_FILE_OLD).unwrap(), old);
    assert_eq!(log_file(&dev), line);
    // C++: both opens of the log file got the 512 B stdio buffer (storage::kFileBufferSize)
    // before any I/O. No Rust form: the port has no stdio layer. The two opens are counted.
    assert_eq!(dev.fs.knobs().write_opens, 2);
    assert_eq!(dev.fs.knobs().renames, 1);
}

#[test]
fn file_a_line_that_just_fits_is_appended_without_a_rotation() {
    let board = FakeBoard::new();
    let dev = board.boot();
    mount_fs(&dev);
    let shared = LoggerShared::new();
    let lg = logger(&dev, &shared);
    let host = Host::default();
    let mut sk = sinks(&dev, &lg, &host);
    info(&lg, 1);
    let mut line = line_of(&all(&shared)[0]);
    line.push(b'\n');
    let old = vec![b'o'; 64 * 1024 - line.len()];
    dev.fs.put(LOG_FILE, &old);
    sk.flush();
    let mut want = old.clone();
    want.extend_from_slice(&line);
    assert_eq!(log_file(&dev), want);
    assert_eq!(dev.fs.knobs().renames, 0);
}

#[test]
fn file_a_blocked_rotation_grows_the_file_up_to_72_kib_then_stops() {
    let board = FakeBoard::new();
    let dev = board.boot();
    mount_fs(&dev);
    let shared = LoggerShared::new();
    let lg = logger(&dev, &shared);
    let host = Host::default();
    let mut sk = sinks(&dev, &lg, &host);
    let old = vec![b'o'; 64 * 1024 - 10];
    dev.fs.put(LOG_FILE, &old);
    dev.fs.fail("rename", LOG_FILE, 1000);
    info(&lg, 200);
    sk.flush();
    let grown = log_file(&dev);
    assert!(grown.len() > 64 * 1024);
    assert!(grown.len() <= 72 * 1024);
    assert!(grown.starts_with(&old));
    let written = count_lines(&grown[old.len()..]);
    assert!(written > 100);
    assert!(written < 200);
    assert_eq!(&grown[old.len()..], lines_of(&shared, 1, written as u32));
    assert!(!dev.fs.exists(LOG_FILE_OLD));
    let report = all(&shared).pop().unwrap();
    assert_eq!(report.code, EventCode::LogWriteFailed);
    assert_eq!(report.arg1, 4);
    assert_eq!(report.arg2, 0);
    assert_eq!(shared.stats(0).failures, 1);
    assert_eq!(shared.stats(0).backlog, 201 - written as u32);
    // The reader is gone: the next attempt rotates and writes the rest.
    dev.fs.knobs().failures.clear();
    sk.flush();
    assert_eq!(dev.fs.read(LOG_FILE_OLD).unwrap(), grown);
    assert_eq!(log_file(&dev), lines_of(&shared, written as u32 + 1, 201));
    assert_eq!(shared.stats(0).backlog, 0);
}

#[test]
fn file_an_open_failure_keeps_the_backlog_reports_once_and_retries_after_10_s() {
    let board = FakeBoard::new();
    let dev = board.boot();
    mount_fs(&dev);
    let shared = LoggerShared::new();
    let lg = logger(&dev, &shared);
    let host = Host::default();
    let mut sk = sinks(&dev, &lg, &host);
    dev.clock.set_ms(1000);
    info(&lg, 2);
    dev.fs.fail("open", LOG_FILE, 2);
    shared.request_flush();
    sk.service(false);
    let ev = all(&shared);
    assert_eq!(ev.len(), 3);
    assert_eq!(ev[2].code, EventCode::LogWriteFailed);
    assert_eq!(ev[2].arg1, 1);
    assert_eq!(ev[2].severity, Severity::Warning);
    shared.request_flush();
    dev.clock.set_ms(10_999);
    sk.service(false);
    assert_eq!(dev.fs.knobs().opens, 1); // back-off
    dev.clock.set_ms(11_000);
    sk.service(false); // fails again, not reported again within the hour
    assert_eq!(dev.fs.knobs().opens, 2);
    assert_eq!(all(&shared).len(), 3);
    assert_eq!(shared.stats(11_000).failures, 2);
    dev.clock.set_ms(21_000);
    sk.service(false); // the Warning is due after the back-off
    assert_eq!(log_file(&dev), lines_of(&shared, 1, 3));
    assert_eq!(shared.stats(21_000).failures, 2);
    assert_eq!(shared.stats(21_000).flushes, 3);
}

#[test]
fn file_events_lost_in_the_ring_become_one_gap_line() {
    let board = FakeBoard::new();
    let dev = board.boot();
    mount_fs(&dev);
    let shared = LoggerShared::new();
    let lg = logger(&dev, &shared);
    let host = Host::default();
    let mut sk = sinks(&dev, &lg, &host);
    dev.clock.set_ms(1000);
    dev.fs.fail("open", LOG_FILE, 1000);
    info(&lg, 300);
    sk.service(false); // due (256 waiting), the open fails
    assert_eq!(all(&shared).last().unwrap().code, EventCode::LogWriteFailed);
    info(&lg, 299); // 600 events: the ring (512) lost the oldest 88
    dev.fs.knobs().failures.clear();
    dev.clock.set_ms(11_000);
    sk.service(false);
    let f = all(&shared)[0].seq;
    assert_eq!(f, 89);
    let mut want = format!("#1-{} gap: {} events not written\n", f - 1, f - 1).into_bytes();
    want.extend_from_slice(&lines_of(&shared, f, 600));
    assert_eq!(log_file(&dev), want);
    assert_eq!(shared.stats(11_000).lost, f - 1);
    assert_eq!(shared.stats(11_000).backlog, 0);
}

// ---------------------------------------------------------------- a LittleFS with hooks

type Hook<'h> = Box<dyn FnMut(&str) + Send + 'h>;

/// The fake LittleFS with hooks: after every open of a file and before every write to one (the
/// C++ `fakes::fs().onWrite`: another task running meanwhile).
#[derive(Clone)]
pub(super) struct HookFs<'h> {
    inner: FakeFs,
    on_open: Arc<Mutex<Option<Hook<'h>>>>,
    on_write: Arc<Mutex<Option<Hook<'h>>>>,
}

pub(super) struct HookFile<'h> {
    inner: FakeFile,
    path: String,
    on_write: Arc<Mutex<Option<Hook<'h>>>>,
}

impl<'h> HookFs<'h> {
    pub(super) fn new(inner: FakeFs) -> Self {
        HookFs {
            inner,
            on_open: Arc::default(),
            on_write: Arc::default(),
        }
    }
    pub(super) fn on_open(&self, hook: impl FnMut(&str) + Send + 'h) {
        *self.on_open.lock().unwrap() = Some(Box::new(hook));
    }
    pub(super) fn on_write(&self, hook: impl FnMut(&str) + Send + 'h) {
        *self.on_write.lock().unwrap() = Some(Box::new(hook));
    }
}

impl<'h> Fs for HookFs<'h> {
    type File = HookFile<'h>;
    fn mount(&self) -> bool {
        self.inner.mount()
    }
    fn format(&self) -> bool {
        self.inner.format()
    }
    fn open(&self, path: &str, mode: OpenMode) -> Option<HookFile<'h>> {
        let f = self.inner.open(path, mode)?;
        if let Some(h) = self.on_open.lock().unwrap().as_mut() {
            h(path);
        }
        Some(HookFile {
            inner: f,
            path: path.to_string(),
            on_write: self.on_write.clone(),
        })
    }
    fn exists(&self, path: &str) -> bool {
        self.inner.exists(path)
    }
    fn mkdir(&self, path: &str) -> bool {
        self.inner.mkdir(path)
    }
    fn remove(&self, path: &str) -> bool {
        self.inner.remove(path)
    }
    fn rename(&self, from: &str, to: &str) -> bool {
        self.inner.rename(from, to)
    }
    fn list(&self, dir: &str, visit: &mut dyn FnMut(&FsEntry) -> bool) {
        self.inner.list(dir, visit)
    }
    fn usage(&self) -> (u32, u32) {
        self.inner.usage()
    }
}

impl FsFile for HookFile<'_> {
    fn read(&mut self, out: &mut [u8]) -> usize {
        self.inner.read(out)
    }
    fn write(&mut self, data: &[u8]) -> usize {
        if let Some(h) = self.on_write.lock().unwrap().as_mut() {
            h(&self.path);
        }
        self.inner.write(data)
    }
    fn seek(&mut self, pos: u32) -> bool {
        self.inner.seek(pos)
    }
    fn size(&self) -> u32 {
        self.inner.size()
    }
}

fn hook_sinks<'a>(
    dev: &Device,
    lg: &'a TestLogger<'a>,
    host: &'a Host,
    fs: &HookFs<'a>,
) -> TestSinks<'a, HookFs<'a>> {
    LogSinks::new(lg, fs.clone(), dev.udp.clone(), host)
}

#[test]
fn file_events_the_ring_drops_while_the_file_is_written_become_a_gap_line() {
    let board = FakeBoard::new();
    let dev = board.boot();
    mount_fs(&dev);
    let shared = LoggerShared::new();
    let lg = logger(&dev, &shared);
    let host = Host::default();
    let burst = AtomicBool::new(false);
    let fs = HookFs::new(dev.fs.clone());
    let mut sk = hook_sinks(&dev, &lg, &host, &fs);
    info(&lg, 300); // 256 waiting: due
    let before = all(&shared);
    let mut first4 = Vec::new();
    for e in &before[..4] {
        first4.extend_from_slice(&line_of(e));
        first4.push(b'\n');
    }
    fs.on_write(|path| {
        if path != LOG_FILE || burst.swap(true, Ordering::Relaxed) {
            return;
        }
        info(&lg, 600); // another task meanwhile: the ring (512) drops seq 1..388
    });
    sk.service(false);
    assert!(burst.load(Ordering::Relaxed));
    assert_eq!(all(&shared)[0].seq, 389);
    // lines 1..4 were read before the drop, the next batch starts at 389
    let mut head = first4.clone();
    head.extend_from_slice(b"#5-388 gap: 384 events not written\n");
    head.extend_from_slice(&lines_of(&shared, 389, 392));
    assert_eq!(log_file(&dev), head);
    assert_eq!(shared.stats(0).lost, 384);
    sk.flush();
    let mut all_lines = head.clone();
    all_lines.extend_from_slice(&lines_of(&shared, 393, 900));
    assert_eq!(log_file(&dev), all_lines);
    assert_eq!(shared.stats(0).lost, 384);
}

#[test]
fn file_a_short_write_keeps_the_cursor_at_the_last_complete_line() {
    let board = FakeBoard::new();
    let dev = board.boot();
    mount_fs(&dev);
    let shared = LoggerShared::new();
    let lg = logger(&dev, &shared);
    let host = Host::default();
    let mut sk = sinks(&dev, &lg, &host);
    info(&lg, 8);
    let ev = all(&shared);
    let four: usize = ev[..4].iter().map(|e| line_of(e).len() + 1).sum();
    // Lines 1-4 fill the file's block exactly and there is no free block for line 5.
    let old = vec![b'o'; 4096 - four];
    dev.fs.put(LOG_FILE, &old);
    let used = dev.fs.usage().1;
    dev.fs.knobs().total_bytes = used;
    sk.flush();
    let mut want = old.clone();
    want.extend_from_slice(&lines_of(&shared, 1, 4));
    assert_eq!(log_file(&dev), want);
    assert_eq!(shared.stats(0).backlog, 5); // lines 5-8 and the LogWriteFailed report
    let last = all(&shared).pop().unwrap();
    assert_eq!(last.code, EventCode::LogWriteFailed);
    assert_eq!(last.arg1, 2);
    dev.fs.knobs().total_bytes = 0x17_0000;
    sk.flush();
    let mut want = old.clone();
    want.extend_from_slice(&lines_of(&shared, 1, 9));
    assert_eq!(log_file(&dev), want);
}

#[test]
fn syslog_level_1_sends_warnings_as_rfc_5424_to_the_server() {
    let board = FakeBoard::new();
    let dev = board.boot();
    let shared = LoggerShared::new();
    let lg = logger(&dev, &shared);
    let host = Host::default();
    let mut sk = sinks(&dev, &lg, &host);
    shared.configure(1, ip(192, 168, 1, 9), 1514, true, b"Heating Floor");
    lg.log(EventCode::Boot, NO_VALVE, 0, 0, b""); // Info: not sent at level 1
    lg.log_sev(EventCode::NetDown, Severity::Warning, NO_VALVE, 1, 0, b"");
    sk.service(false);
    assert!(dev.udp.sent().is_empty()); // network down: not sent, not kept
    lg.log_sev(EventCode::NetDown, Severity::Warning, NO_VALVE, 2, 0, b"");
    sk.service(true);
    let sent = dev.udp.sent();
    assert_eq!(sent.len(), 1);
    let (pip, port, data) = &sent[0];
    assert_eq!(*pip, ip(192, 168, 1, 9));
    assert_eq!(*port, 1514);
    let mut host_name = [0u8; 64];
    let hn = build_hostname(b"Heating Floor", &mut host_name);
    let e = all(&shared).pop().unwrap();
    let mut msg = [0u8; 120];
    let m = format_event_message(&e, &mut msg);
    let mut want = [0u8; 240];
    let w = format_syslog(&e, &msg[..m], &host_name[..hn], &mut want);
    assert_eq!(data.as_slice(), &want[..w]);
}

#[test]
fn syslog_at_most_32_events_per_pass_no_server_or_port_nothing() {
    let board = FakeBoard::new();
    let dev = board.boot();
    let shared = LoggerShared::new();
    let lg = logger(&dev, &shared);
    let host = Host::default();
    let mut sk = sinks(&dev, &lg, &host);
    shared.configure(3, ip(192, 168, 1, 9), 514, true, b"VdMot");
    info(&lg, 40);
    sk.service(true);
    assert_eq!(dev.udp.sent().len(), 32);
    sk.service(true);
    assert_eq!(dev.udp.sent().len(), 40);
    shared.configure(3, 0, 514, true, b"VdMot");
    info(&lg, 1);
    sk.service(true);
    shared.configure(3, ip(192, 168, 1, 9), 0, true, b"VdMot");
    info(&lg, 1);
    sk.service(true);
    shared.configure(0, ip(192, 168, 1, 9), 514, true, b"VdMot");
    info(&lg, 1);
    sk.service(true);
    assert_eq!(dev.udp.sent().len(), 40);
}

// ---------------------------------------------------------------- Rust (new)

#[test]
fn constants_are_the_cpp_ones() {
    assert_eq!(EVENT_CAPACITY, 512); // the native tests' VDM_EVENT_CAPACITY
    assert_eq!(LOG_FILE, "/log/events.log");
    assert_eq!(LOG_FILE_OLD, "/log/events.1.log");
    assert_eq!(LOG_FILE_MAX, 65_536);
    assert_eq!(LOG_FILE_SLACK, 8192);
    assert_eq!(PENDING_LINES, 32);
    assert_eq!(EPOCH_VALID_FROM, 1_577_836_800);
    assert_eq!(BATCH, 4);
}

#[test]
fn file_a_reader_that_holds_the_log_file_blocks_the_rotation_until_it_closes() {
    // design 4.6: a download of /api/log holds the file open, esp_littlefs refuses the rename
    let board = FakeBoard::new();
    let dev = board.boot();
    mount_fs(&dev);
    let shared = LoggerShared::new();
    let lg = logger(&dev, &shared);
    let host = Host::default();
    let mut sk = sinks(&dev, &lg, &host);
    let old = vec![b'o'; 64 * 1024 - 10];
    dev.fs.put(LOG_FILE, &old);
    let reader = dev.fs.open(LOG_FILE, OpenMode::Read).unwrap();
    info(&lg, 3);
    sk.flush();
    let grown = log_file(&dev);
    assert_eq!(&grown[old.len()..], lines_of(&shared, 1, 3));
    assert!(!dev.fs.exists(LOG_FILE_OLD));
    assert_eq!(dev.fs.knobs().renames, 0);
    drop(reader);
    info(&lg, 1);
    sk.flush();
    assert_eq!(dev.fs.read(LOG_FILE_OLD).unwrap(), grown);
    assert_eq!(log_file(&dev), lines_of(&shared, 4, 4));
}

#[test]
fn file_a_log_file_that_cannot_be_opened_after_its_rotation_is_reported_as_step_3() {
    let board = FakeBoard::new();
    let dev = board.boot();
    mount_fs(&dev);
    let shared = LoggerShared::new();
    let lg = logger(&dev, &shared);
    let host = Host::default();
    let fs = HookFs::new(dev.fs.clone());
    let mut sk = hook_sinks(&dev, &lg, &host, &fs);
    dev.fs.put(LOG_FILE, &vec![b'o'; 64 * 1024]);
    let inner = dev.fs.clone();
    let opens = AtomicU32::new(0);
    fs.on_open(move |_| {
        if opens.fetch_add(1, Ordering::Relaxed) == 0 {
            inner.fail("open", LOG_FILE, 1); // the open after the rename fails
        }
    });
    info(&lg, 2);
    sk.flush();
    assert!(dev.fs.exists(LOG_FILE_OLD));
    assert!(!dev.fs.exists(LOG_FILE));
    let last = all(&shared).pop().unwrap();
    assert_eq!(last.code, EventCode::LogWriteFailed);
    assert_eq!(last.arg1, 3);
    assert_eq!(shared.stats(0).backlog, 3);
}

#[test]
fn file_events_logged_while_the_file_is_written_wait_for_the_next_attempt() {
    let board = FakeBoard::new();
    let dev = board.boot();
    mount_fs(&dev);
    let shared = LoggerShared::new();
    let lg = logger(&dev, &shared);
    let host = Host::default();
    let once = AtomicBool::new(false);
    let fs = HookFs::new(dev.fs.clone());
    let mut sk = hook_sinks(&dev, &lg, &host, &fs);
    info(&lg, 4); // one batch, the last seq at the start is 4
    fs.on_write(|_| {
        if !once.swap(true, Ordering::Relaxed) {
            info(&lg, 1);
        }
    });
    sk.flush();
    assert_eq!(log_file(&dev), lines_of(&shared, 1, 4));
    assert_eq!(shared.stats(0).backlog, 1);
    sk.flush();
    assert_eq!(log_file(&dev), lines_of(&shared, 1, 5));
}

#[test]
fn file_two_boots_write_into_one_file_the_second_appends() {
    let board = FakeBoard::new();
    let first = {
        let dev = board.boot();
        mount_fs(&dev);
        let shared = LoggerShared::new();
        let lg = logger(&dev, &shared);
        let host = Host::default();
        let mut sk = sinks(&dev, &lg, &host);
        info(&lg, 2);
        sk.flush();
        lines_of(&shared, 1, 2)
    };
    board.reset(Reset::Software);
    let dev = board.boot();
    assert!(dev.fs.mount());
    let shared = LoggerShared::new();
    let lg = logger(&dev, &shared);
    let host = Host::default();
    let mut sk = sinks(&dev, &lg, &host);
    assert_eq!(lg.log(EventCode::Boot, NO_VALVE, 3, 1, b""), 1); // a new ring per boot
    sk.flush();
    let mut want = first;
    want.extend_from_slice(&lines_of(&shared, 1, 1));
    assert_eq!(log_file(&dev), want);
}

#[test]
fn syslog_a_datagram_the_stack_refuses_is_lost_the_cursor_moves_on() {
    let board = FakeBoard::new();
    let dev = board.boot();
    let shared = LoggerShared::new();
    let lg = logger(&dev, &shared);
    let host = Host::default();
    let mut sk = sinks(&dev, &lg, &host);
    shared.configure(2, ip(10, 0, 0, 1), 514, true, b"VdMot");
    dev.udp.set_fail(true);
    info(&lg, 2);
    sk.service(true);
    dev.udp.set_fail(false);
    sk.service(true);
    assert!(dev.udp.sent().is_empty());
    info(&lg, 1);
    sk.service(true);
    assert_eq!(dev.udp.sent().len(), 1);
    assert_eq!(dev.udp.sent()[0].0, ip(10, 0, 0, 1));
}
