//! Port of test/native/test_line_assembler.cpp: CR/LF framing, bounds, overflow/malformed
//! accounting. The C++ assembler over caller storage (null, capacity 0, 1, 2) is the
//! LineAssembler<N> with N = 0, 1, 2; Rust owns the storage, so "never writes past the buffer"
//! holds by construction.

use super::*;
use crate::test_support::Lcg;
use std::string::String;
use std::vec;
use std::vec::Vec;

/// Feeds everything, collecting completed lines (the way the glue does it). Rust addition: it
/// fails at once instead of looping for ever when the assembler makes no progress.
fn feed_all<const N: usize>(la: &mut LineAssembler<N>, data: &[u8]) -> Vec<String> {
    let mut lines = Vec::new();
    let mut off = 0;
    while off < data.len() {
        let used = la.feed(&data[off..]);
        off += used;
        if la.has_line() {
            lines.push(String::from_utf8(la.line().to_vec()).expect("printable ASCII"));
            la.release();
            assert!(!la.has_line(), "release() keeps the line");
        } else {
            assert!(used > 0, "feed() consumed nothing and has no line");
        }
    }
    lines
}

fn strings(lines: &[&str]) -> Vec<String> {
    lines.iter().map(|s| String::from(*s)).collect()
}

#[test]
fn initial_state() {
    let la = LineAssembler::<8>::new();
    assert!(!la.has_line());
    assert_eq!(la.length(), 0);
    assert_eq!(la.line(), b"");
    assert_eq!(la.overflow_count(), 0);
    assert_eq!(la.malformed_count(), 0);
    let d = LineAssembler::<8>::default();
    assert!(!d.has_line());
    assert_eq!(d.length(), 0);
}

#[test]
fn cr_lf_and_crlf_each_end_exactly_one_line() {
    let mut la = LineAssembler::<64>::new();
    assert_eq!(feed_all(&mut la, b"abc\r"), strings(&["abc"]));
    assert_eq!(feed_all(&mut la, b"def\n"), strings(&["def"]));
    assert_eq!(feed_all(&mut la, b"ghi\r\n"), strings(&["ghi"]));
    assert_eq!(
        feed_all(&mut la, b"a\r\nb\rc\nd\r\n"),
        strings(&["a", "b", "c", "d"])
    );
    // LF CR is two terminators around nothing: still one line.
    assert_eq!(feed_all(&mut la, b"e\n\r"), strings(&["e"]));
    assert_eq!(la.overflow_count(), 0);
    assert_eq!(la.malformed_count(), 0);
}

#[test]
fn crlf_split_across_feed_calls_counts_once() {
    let mut la = LineAssembler::<64>::new();
    assert_eq!(feed_all(&mut la, b"gvlvd 1\r"), strings(&["gvlvd 1"]));
    assert_eq!(feed_all(&mut la, b"\ngvlvd 2\r"), strings(&["gvlvd 2"]));
    assert!(feed_all(&mut la, b"\n").is_empty());
    assert!(!la.has_line());
}

#[test]
fn empty_lines_are_ignored() {
    let mut la = LineAssembler::<16>::new();
    assert!(feed_all(&mut la, b"\r\n\r\n\n\n\r\r").is_empty());
    assert_eq!(feed_all(&mut la, b"\r\nx\r\n\r\n"), strings(&["x"]));
}

#[test]
fn push_semantics_and_hold_of_the_pending_line() {
    let mut la = LineAssembler::<16>::new();
    assert!(la.push(b'a'));
    assert_eq!(la.length(), 1);
    assert_eq!(la.line(), b"a"); // partial line visible
    assert!(!la.has_line());
    assert!(la.push(b'\r'));
    assert!(la.has_line());
    // Pending line: nothing more is consumed, the line stays intact.
    assert!(!la.push(b'b'));
    assert!(!la.push(b'\n'));
    assert_eq!(la.line(), b"a");
    assert_eq!(la.length(), 1);
    la.release();
    assert!(!la.has_line());
    assert_eq!(la.length(), 0);
    assert_eq!(la.line(), b"");
    assert!(la.push(b'b'));
    assert!(la.push(b'\n'));
    assert_eq!(la.line(), b"b");
}

#[test]
fn feed_stops_after_a_complete_line_and_keeps_the_rest() {
    let mut la = LineAssembler::<32>::new();
    let data = b"one\r\ntwo\r\n";
    let used = la.feed(data);
    assert_eq!(used, 4); // "one\r"
    assert!(la.has_line());
    assert_eq!(la.line(), b"one");
    assert_eq!(la.feed(&data[used..]), 0); // held
    la.release();
    let used2 = la.feed(&data[used..]);
    assert_eq!(used2, 5); // "\ntwo\r"
    assert_eq!(la.line(), b"two");
    la.release();
    assert_eq!(la.feed(&data[used + used2..used + used2 + 1]), 1); // final "\n" swallowed
    assert!(!la.has_line());
}

#[test]
fn feed_edge_cases() {
    let mut la = LineAssembler::<8>::new();
    // C++ feed(nullptr, 5) == 0: no Rust form; an empty slice consumes nothing either.
    assert_eq!(la.feed(&b"abc"[..0]), 0);
    assert_eq!(la.length(), 0);
    assert_eq!(la.feed(b"abc"), 3);
    assert!(!la.has_line());
    assert_eq!(la.length(), 3);
}

#[test]
fn capacity_boundary() {
    let mut la = LineAssembler::<5>::new(); // 4 chars max
    assert_eq!(feed_all(&mut la, b"abcd\r"), strings(&["abcd"]));
    assert_eq!(la.overflow_count(), 0);
    assert!(feed_all(&mut la, b"abcde\r").is_empty());
    assert_eq!(la.overflow_count(), 1);
    // Discarding continues up to the terminator, the next line is clean.
    assert_eq!(feed_all(&mut la, b"abcdefghij\r\nxy\r\n"), strings(&["xy"]));
    assert_eq!(la.overflow_count(), 2);
    assert_eq!(la.malformed_count(), 0);
}

#[test]
fn overflow_counted_once_per_line_even_without_terminator() {
    let mut la = LineAssembler::<4>::new();
    let long_line = vec![b'x'; 1000];
    assert!(feed_all(&mut la, &long_line).is_empty());
    assert_eq!(la.overflow_count(), 1);
    assert_eq!(la.length(), 0);
    assert_eq!(la.line(), b"");
    assert_eq!(feed_all(&mut la, b"\rok\r"), strings(&["ok"]));
    assert_eq!(la.overflow_count(), 1);
}

#[test]
fn stm_max_line_len_line_fits_one_more_does_not() {
    let mut la = LineAssembler::<{ STM_MAX_LINE_LEN + 1 }>::new();
    let exact = vec![b'a'; STM_MAX_LINE_LEN];
    let mut line = exact.clone();
    line.extend_from_slice(b"\r\n");
    let want = String::from_utf8(exact.clone()).expect("ASCII");
    assert_eq!(feed_all(&mut la, &line), [want]);
    assert_eq!(la.overflow_count(), 0);
    let mut longer = exact;
    longer.extend_from_slice(b"b\r\n");
    assert!(feed_all(&mut la, &longer).is_empty());
    assert_eq!(la.overflow_count(), 1);
}

#[test]
fn non_printable_bytes_drop_the_line_as_malformed() {
    let mut la = LineAssembler::<32>::new();
    assert_eq!(feed_all(&mut la, b"ab\x01cd\r\nok\r\n"), strings(&["ok"]));
    assert_eq!(la.malformed_count(), 1);
    assert!(feed_all(&mut la, b"a\x7f\r").is_empty());
    assert_eq!(la.malformed_count(), 2);
    assert!(feed_all(&mut la, b"\x80\xff\xfe\r").is_empty());
    assert_eq!(la.malformed_count(), 3); // once per line
    assert!(feed_all(&mut la, b"\x1f\r").is_empty());
    assert_eq!(la.malformed_count(), 4);
    assert!(feed_all(&mut la, b"a\0b\r").is_empty());
    assert_eq!(la.malformed_count(), 5);
    assert_eq!(la.overflow_count(), 0);
}

#[test]
fn printable_range_and_tab_are_accepted() {
    let mut la = LineAssembler::<128>::new();
    let mut all: Vec<u8> = (0x20..=0x7E).collect();
    all.push(b'\t');
    let want = String::from_utf8(all.clone()).expect("ASCII");
    all.push(b'\n');
    assert_eq!(feed_all(&mut la, &all), [want]);
    assert_eq!(la.malformed_count(), 0);
}

#[test]
fn malformed_then_overflow_in_the_same_line_counts_once() {
    let mut la = LineAssembler::<4>::new();
    assert!(feed_all(&mut la, b"\x01xxxxxxxx\r").is_empty());
    assert_eq!(la.malformed_count(), 1);
    assert_eq!(la.overflow_count(), 0);
    assert!(feed_all(&mut la, b"xxxxx\x01\r").is_empty());
    assert_eq!(la.malformed_count(), 1);
    assert_eq!(la.overflow_count(), 1);
}

#[test]
fn reset_drops_partial_line_pending_line_and_discard_state() {
    let mut la = LineAssembler::<4>::new();
    assert_eq!(la.feed(b"abcdef"), 6); // overflowing -> discarding
    assert_eq!(la.overflow_count(), 1);
    la.reset();
    assert_eq!(feed_all(&mut la, b"xy\r"), strings(&["xy"])); // not discarded

    assert_eq!(la.feed(b"ab"), 2);
    la.reset();
    assert_eq!(la.length(), 0);
    assert_eq!(feed_all(&mut la, b"c\r"), strings(&["c"]));

    assert_eq!(la.feed(b"d\r"), 2);
    assert!(la.has_line());
    la.reset();
    assert!(!la.has_line());
    assert_eq!(la.length(), 0);
    assert_eq!(la.line(), b"");
    // Counters survive reset.
    assert_eq!(la.overflow_count(), 1);
}

#[test]
fn degenerate_capacities() {
    // C++ null storage with capacity 100: no storage at all.
    let mut null = LineAssembler::<0>::new();
    assert_eq!(null.line(), b"");
    assert!(feed_all(&mut null, b"abc\r\n").is_empty());
    assert_eq!(null.overflow_count(), 1);
    assert_eq!(null.length(), 0);
    null.release();
    null.reset();
    assert_eq!(null.line(), b"");

    // capacity 0
    let mut zero = LineAssembler::<0>::new();
    assert!(feed_all(&mut zero, b"a\r").is_empty());
    assert_eq!(zero.overflow_count(), 1);
    assert_eq!(zero.line(), b"");

    // capacity 1
    let mut one = LineAssembler::<1>::new();
    assert!(feed_all(&mut one, b"a\rb\r").is_empty());
    assert_eq!(one.overflow_count(), 2);
    assert!(feed_all(&mut one, b"\r\n").is_empty());
    assert_eq!(one.overflow_count(), 2); // empty lines are not overflows

    // capacity 2
    let mut two = LineAssembler::<2>::new();
    assert_eq!(feed_all(&mut two, b"a\r"), strings(&["a"]));
    assert!(feed_all(&mut two, b"ab\r").is_empty());
    assert_eq!(two.overflow_count(), 1);
}

#[test]
fn buffer_is_never_overrun() {
    // The C++ checks the bytes around a caller buffer; Rust owns the storage. What is left:
    // an overlong line is consumed and dropped, a line of capacity - 1 chars is complete.
    let mut la = LineAssembler::<8>::new();
    assert_eq!(la.feed(b"12345678901234"), 14);
    assert_eq!(la.overflow_count(), 1);
    la.reset();
    assert_eq!(la.feed(b"1234567\r"), 8);
    assert_eq!(la.line().len(), 7);
    assert_eq!(la.line(), b"1234567");
}

#[test]
fn fixed_seed_fuzz_keeps_invariants() {
    // C++: seed = 0x1234567; seed = seed * 1664525u + 1013904223u; rnd() = seed >> 8.
    let mut lcg = Lcg::numerical_recipes(0x123_4567);
    let mut rnd = move || lcg.next_state() >> 8;
    let mut la = LineAssembler::<17>::new();
    let mut lines = 0usize;
    for _ in 0..20000 {
        let mut chunk = [0u8; 64];
        let n = rnd() as usize % chunk.len();
        for byte in chunk.iter_mut().take(n) {
            let r = rnd() % 100;
            // Mostly printable, some terminators, a few junk bytes.
            *byte = if r < 80 {
                (0x20 + rnd() % 95) as u8
            } else if r < 95 {
                if r & 1 != 0 {
                    b'\r'
                } else {
                    b'\n'
                }
            } else {
                (rnd() & 0xFF) as u8
            };
        }
        let mut off = 0;
        while off < n {
            off += la.feed(&chunk[off..n]);
            assert!(la.length() <= 16);
            assert_eq!(la.line().len(), la.length());
            if la.has_line() {
                assert!(la.length() > 0);
                assert!(la
                    .line()
                    .iter()
                    .all(|&c| c == b'\t' || (0x20..0x7F).contains(&c)));
                lines += 1;
                la.release();
                assert!(!la.has_line()); // Rust addition: no endless loop under a mutant
            } else {
                assert_eq!(off, n);
            }
        }
    }
    assert!(lines > 1000);
    assert!(la.overflow_count() > 0);
    assert!(la.malformed_count() > 0);
}
