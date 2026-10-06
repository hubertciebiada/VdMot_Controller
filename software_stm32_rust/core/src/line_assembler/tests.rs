//! Port of test/native/test_line_assembler.cpp.

use super::*;
use crate::test_support::text;
use std::string::String;
use std::vec::Vec;

/// Feeds the whole input, collecting every completed line.
fn feed_all<B: Storage>(la: &mut LineAssembler<B>, input: &[u8]) -> Vec<String> {
    let mut lines = Vec::new();
    let mut pos = 0;
    while pos < input.len() {
        pos += la.feed(&input[pos..]);
        if la.has_line() {
            lines.push(text(la.line()));
            la.release();
        }
    }
    lines
}

fn lines(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| String::from(*s)).collect()
}

#[test]
fn lf_cr_and_crlf_terminate_lines() {
    let mut la = StaticLineAssembler::<32>::default();
    assert_eq!(feed_all(&mut la, b"gvlst \n"), lines(&["gvlst "]));
    assert_eq!(feed_all(&mut la, b"gvers \r"), lines(&["gvers "]));
    assert_eq!(
        feed_all(&mut la, b"stgtp 1 50 \r\n"),
        lines(&["stgtp 1 50 "])
    );
    assert_eq!(
        feed_all(&mut la, b"a\n\rb\r\n\r\nc\n"),
        lines(&["a", "b", "c"])
    );
    assert_eq!(la.overflow_count(), 0);
    assert_eq!(la.malformed_count(), 0);
}

#[test]
fn empty_lines_and_bare_terminators_are_ignored() {
    let mut la = StaticLineAssembler::<8>::default();
    assert!(feed_all(&mut la, b"\n\r\r\n\n").is_empty());
    assert!(!la.has_line());
    assert_eq!(la.length(), 0);
    assert!(la.line().is_empty());
}

#[test]
fn a_line_is_not_delivered_before_its_terminator() {
    let mut la = StaticLineAssembler::<16>::default();
    assert_eq!(la.feed(b"gver"), 4);
    assert!(!la.has_line());
    assert_eq!(la.line(), b"gver");
    assert_eq!(la.length(), 4);
    assert!(la.take_line().is_none());
    assert_eq!(la.feed(b"s\n"), 2);
    assert!(la.has_line());
    assert_eq!(la.line(), b"gvers");
    assert!(la.take_line().is_some());
    assert_eq!(la.take_line().map(|l| text(l)), Some(String::from("gvers")));
}

#[test]
fn bytes_after_the_terminator_stay_with_the_caller() {
    let mut la = StaticLineAssembler::<32>::default();
    let input = b"gvlst \ngvers \nrest";
    let len = input.len();
    let mut used = la.feed(input);
    assert_eq!(used, 7);
    assert!(la.has_line());
    assert_eq!(la.line(), b"gvlst ");

    // While a line is pending nothing more is consumed.
    assert_eq!(la.feed(&input[used..]), 0);
    assert!(!la.push(b'x'));
    assert_eq!(la.line(), b"gvlst ");

    la.release();
    used += la.feed(&input[used..]);
    assert_eq!(used, 14);
    assert!(la.has_line());
    assert_eq!(la.line(), b"gvers ");
    la.release();
    used += la.feed(&input[used..]);
    assert_eq!(used, len);
    assert!(!la.has_line());
    assert_eq!(la.line(), b"rest");
}

#[test]
fn push_reports_consumption() {
    let mut la = StaticLineAssembler::<8>::default();
    assert!(la.push(b'a'));
    assert!(la.push(b'\n'));
    assert!(la.has_line());
    assert!(!la.push(b'b'));
    la.release();
    assert!(la.push(b'b'));
    assert_eq!(la.line(), b"b");
}

#[test]
fn capacity_boundary() {
    let mut la = StaticLineAssembler::<6>::default(); // at most 5 characters
    assert_eq!(feed_all(&mut la, b"12345\n"), lines(&["12345"]));
    assert_eq!(la.overflow_count(), 0);

    assert!(feed_all(&mut la, b"123456\n").is_empty());
    assert_eq!(la.overflow_count(), 1);

    // The oversize line is dropped up to its terminator, the next is intact.
    assert_eq!(feed_all(&mut la, b"1234567890abc\nok\n"), lines(&["ok"]));
    assert_eq!(la.overflow_count(), 2);

    // CRLF after an oversize line does not create an extra line.
    assert_eq!(feed_all(&mut la, b"abcdefgh\r\nxy\r\n"), lines(&["xy"]));
    assert_eq!(la.overflow_count(), 3);
}

#[test]
fn an_endless_line_is_counted_once_and_keeps_memory_bounded() {
    let mut la = StaticLineAssembler::<4>::default();
    let noise = std::vec![b'x'; 10000];
    assert!(feed_all(&mut la, &noise).is_empty());
    assert_eq!(la.overflow_count(), 1);
    assert_eq!(la.length(), 0);
    assert_eq!(feed_all(&mut la, b"\nab\n"), lines(&["ab"]));
    assert_eq!(la.overflow_count(), 1);
}

#[test]
fn lines_with_control_or_non_ascii_bytes_are_dropped() {
    let mut la = StaticLineAssembler::<32>::default();
    assert!(feed_all(&mut la, b"gv\0ers\n").is_empty());
    assert_eq!(la.malformed_count(), 1);
    assert!(feed_all(&mut la, b"\x7fgvers\n").is_empty());
    assert!(feed_all(&mut la, b"gvers\x80\n").is_empty());
    assert!(feed_all(&mut la, b"\x1b[A\n").is_empty());
    assert_eq!(la.malformed_count(), 4);
    assert_eq!(la.overflow_count(), 0);
    // TAB and every printable character are accepted.
    assert_eq!(feed_all(&mut la, b"a\tb ~\n"), lines(&["a\tb ~"]));
    assert_eq!(
        feed_all(&mut la, b" !/09:@AZ[`az{~\n"),
        lines(&[" !/09:@AZ[`az{~"])
    );
    assert_eq!(la.malformed_count(), 4);
}

#[test]
fn malformed_then_oversize_in_one_line_counts_once() {
    let mut la = StaticLineAssembler::<4>::default();
    assert!(feed_all(&mut la, b"\x01xxxxxxxx\n").is_empty());
    assert_eq!(la.malformed_count(), 1);
    assert_eq!(la.overflow_count(), 0);
    assert!(feed_all(&mut la, b"xxxx\x01\n").is_empty());
    assert_eq!(la.malformed_count(), 1);
    assert_eq!(la.overflow_count(), 1);
}

#[test]
fn reset_drops_partial_line_and_discard_state() {
    let mut la = StaticLineAssembler::<4>::default();
    assert_eq!(la.feed(b"abcdef"), 6); // now discarding
    la.reset();
    assert_eq!(feed_all(&mut la, b"ok\n"), lines(&["ok"]));
    assert_eq!(la.feed(b"par"), 3);
    la.reset();
    assert_eq!(la.length(), 0);
    assert_eq!(feed_all(&mut la, b"z\n"), lines(&["z"]));
    assert_eq!(la.feed(b"x\n"), 2);
    la.reset();
    assert!(!la.has_line());
}

#[test]
fn degenerate_storage_drops_every_line_as_overflow() {
    let mut one = [0u8; 1];
    let mut tiny = LineAssembler::new(&mut one[..]);
    assert!(feed_all(&mut tiny, b"a\n\nb\n").is_empty());
    assert_eq!(tiny.overflow_count(), 2);
    assert!(tiny.line().is_empty());

    // the C++ null buffer is storage of length 0
    let mut none = LineAssembler::new(&mut [0u8; 0][..]);
    assert!(feed_all(&mut none, b"abc\n").is_empty());
    assert_eq!(none.overflow_count(), 1);
    assert!(none.line().is_empty());
    // C++ feed(nullptr, 5): a Rust slice is never null; empty data consumes nothing
    assert_eq!(none.feed(b""), 0);

    let mut two = [0u8; 2];
    let mut smallest = LineAssembler::new(&mut two[..]);
    assert_eq!(feed_all(&mut smallest, b"a\nbc\nd\n"), lines(&["a", "d"]));
    assert_eq!(smallest.overflow_count(), 1);
}

#[test]
fn every_dropped_line_is_counted() {
    let mut la = StaticLineAssembler::<2>::default();
    for _ in 0..1000 {
        la.feed(b"xx\n");
        la.feed(b"\x01\n");
    }
    assert_eq!(la.overflow_count(), 1000);
    assert_eq!(la.malformed_count(), 1000);
}

#[test]
fn expire_drops_a_stale_partial_line_sender_went_away() {
    let mut la = StaticLineAssembler::<32>::default();
    assert!(!la.partial());
    assert!(!la.expire(1000, 0, 100)); // nothing pending

    // "stgtp 3" from an ESP that reset before sending the terminator
    la.feed(b"stgtp 3");
    assert!(la.partial());
    assert!(!la.expire(1100, 1000, 100)); // exactly the timeout: kept
    assert!(la.partial());
    assert!(la.expire(1101, 1000, 100));
    assert!(!la.partial());
    assert_eq!(la.expired_count(), 1);
    assert_eq!(la.malformed_count(), 0);
    assert_eq!(la.overflow_count(), 0);

    // the first request after the restart is not glued to the stale bytes
    assert_eq!(feed_all(&mut la, b"gproto\n"), lines(&["gproto"]));
    assert_eq!(la.expired_count(), 1);
}

#[test]
fn expire_works_across_the_millisecond_clock_wrap() {
    let mut la = StaticLineAssembler::<32>::default();
    la.feed(b"gvl");
    assert!(!la.expire(40, 0xFFFF_FFF0, 100)); // 56 ms after the last byte
    assert!(la.partial());
    assert!(la.expire(120, 0xFFFF_FFF0, 100)); // 136 ms
    assert_eq!(la.expired_count(), 1);
}

#[test]
fn expire_leaves_a_complete_line_alone() {
    let mut la = StaticLineAssembler::<32>::default();
    assert_eq!(la.feed(b"gvers\nxy"), 6);
    assert!(la.has_line());
    assert!(!la.partial());
    assert!(!la.expire(10000, 0, 100));
    assert!(la.has_line());
    assert_eq!(la.line(), b"gvers");
    assert_eq!(la.expired_count(), 0);
}

#[test]
fn expire_ends_the_discarding_of_an_unterminated_overlong_line() {
    let mut la = StaticLineAssembler::<8>::default();
    la.feed(b"0123456789"); // overflow, discarding until a terminator
    assert_eq!(la.overflow_count(), 1);
    assert!(la.partial());
    assert!(la.expire(200, 0, 100));
    assert_eq!(la.expired_count(), 0); // already counted as overflow
    assert_eq!(feed_all(&mut la, b"gvers\n"), lines(&["gvers"]));

    let mut bad = StaticLineAssembler::<8>::default();
    bad.feed(b"ab\x01"); // malformed, discarding
    assert_eq!(bad.malformed_count(), 1);
    assert!(bad.expire(200, 0, 100));
    assert_eq!(bad.expired_count(), 0);
    assert_eq!(feed_all(&mut bad, b"x\n"), lines(&["x"]));
}
