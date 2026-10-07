//! `test_logger__mut.cpp`: buffer limits of the longest lines, host name length, statistics,
//! the file handle and the backlog after a written cursor.
#![allow(clippy::large_stack_frames, clippy::large_stack_arrays)]

use super::tests::{all, info, ip, logger, mount_fs, sinks, Host};
use super::*;
use crate::testkit::FakeBoard;

/// `kEventTextMax` characters.
const LONG_TEXT: &[u8] = b"abcdefghijklmnopqrstuvw";

/// The event code whose message is the longest with the widest arguments and text.
fn longest_code() -> EventCode {
    let mut best = EventCode::Boot;
    let mut best_len = 0;
    for c in 0..1000u16 {
        let Some(code) = EventCode::from_raw(c) else {
            continue;
        };
        let e = make_event(code, Severity::Warning, 11, i32::MIN, i32::MIN, LONG_TEXT);
        let mut msg = [0u8; 512];
        let n = format_event_message(&e, &mut msg);
        if n > best_len {
            best_len = n;
            best = code;
        }
    }
    best
}

#[test]
fn the_longest_event_is_cut_to_the_console_file_and_syslog_buffers() {
    let board = FakeBoard::new();
    let dev = board.boot();
    mount_fs(&dev);
    let shared = LoggerShared::new();
    let lg = logger(&dev, &shared);
    let host = Host::default();
    let mut sk = sinks(&dev, &lg, &host);
    shared.configure(3, ip(192, 168, 1, 9), 514, true, b"VdMot");
    dev.wall.set(1_790_136_000);
    let code = longest_code();
    lg.log_sev(code, Severity::Warning, 11, i32::MIN, i32::MIN, LONG_TEXT);
    sk.service(true);
    sk.flush();
    let ev = all(&shared);
    assert_eq!(ev.len(), 1);
    let e = &ev[0];
    let mut line = [0u8; 160];
    let n = format_event_line(e, &mut line);
    let mut full = [0u8; 512];
    let full_len = format_event_line(e, &mut full);
    assert!(full_len > 160, "longest line {full_len}"); // the buffers below matter
    assert_eq!(dev.console.lines(), vec![line[..n].to_vec()]);
    let mut file = line[..n].to_vec();
    file.push(b'\n');
    assert_eq!(dev.fs.read(LOG_FILE).unwrap(), file);
    let mut full_msg = [0u8; 512];
    assert!(format_event_message(e, &mut full_msg) < 120 - 1); // never the limit
    let mut msg = [0u8; 120];
    let m = format_event_message(e, &mut msg);
    let mut host_name = [0u8; 64];
    let hn = build_hostname(b"VdMot", &mut host_name);
    let mut want = [0u8; 240];
    let w = format_syslog(e, &msg[..m], &host_name[..hn], &mut want);
    assert!(w < 240 - 1); // the packet buffer is never the limit
    let sent = dev.udp.sent();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].2, want[..w].to_vec());
}

#[test]
fn syslog_the_host_name_is_cut_to_the_station_name_length() {
    let board = FakeBoard::new();
    let dev = board.boot();
    let shared = LoggerShared::new();
    let lg = logger(&dev, &shared);
    let host = Host::default();
    let mut sk = sinks(&dev, &lg, &host);
    shared.configure(
        1,
        ip(192, 168, 1, 9),
        514,
        true,
        b"abcdefghijklmnopqrstuvwxyz",
    );
    lg.log_sev(EventCode::NetDown, Severity::Warning, NO_VALVE, 1, 0, b"");
    sk.service(true);
    let sent = dev.udp.sent();
    assert_eq!(sent.len(), 1);
    let e = all(&shared).pop().unwrap();
    let mut msg = [0u8; 120];
    let m = format_event_message(&e, &mut msg);
    let mut want = [0u8; 240];
    let w = format_syslog(&e, &msg[..m], b"abcdefghijklmnopqrst", &mut want);
    assert_eq!(sent[0].2, want[..w].to_vec());
}

#[test]
fn file_the_file_is_closed_after_the_attempt_the_backlog_counts_from_the_cursor() {
    let board = FakeBoard::new();
    let dev = board.boot();
    mount_fs(&dev);
    let shared = LoggerShared::new();
    let lg = logger(&dev, &shared);
    let host = Host::default();
    let mut sk = sinks(&dev, &lg, &host);
    dev.clock.set_ms(0);
    assert_eq!(shared.stats(5000).last_flush_age_s, 0); // never flushed
    info(&lg, 300);
    sk.flush();
    assert_eq!(dev.fs.open_handles(), 0);
    assert_eq!(dev.fs.knobs().writes, 300);
    assert_eq!(shared.stats(1_000_000).last_flush_age_s, 1000);
    assert_eq!(shared.stats(999_999).last_flush_age_s, 999);
    info(&lg, 1);
    sk.service(false); // 1 Info waiting: not due
    assert_eq!(dev.fs.knobs().writes, 300);
    assert_eq!(shared.stats(0).backlog, 1);
}
