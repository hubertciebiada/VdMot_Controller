//! Port of test/native/test_tokenizer.cpp. `t.parse(line)` with the C++ default argument is
//! `t.parse(line, Tokenizer::MAX_ARGS)`.

use super::*;
use std::vec::Vec;

const ALL: u8 = Tokenizer::MAX_ARGS;

#[test]
fn command_with_trailing_space_protocol_v1_style() {
    let line = b"stgtp 3 50 ";
    let mut t = Tokenizer::default();
    assert!(t.parse(line, 5));
    assert_eq!(t.command(), b"stgtp");
    assert!(t.is(b"stgtp"));
    assert_eq!(t.argc(), 2);
    assert_eq!(t.arg(0), b"3");
    assert_eq!(t.arg(1), b"50");
    assert!(!t.too_many_args());
}

#[test]
fn last_token_without_trailing_separator() {
    let mut t = Tokenizer::default();
    assert!(t.parse(b"gvlst", ALL));
    assert!(t.is(b"gvlst"));
    assert_eq!(t.argc(), 0);

    assert!(t.parse(b"gvlvd 11", ALL));
    assert_eq!(t.argc(), 1);
    assert_eq!(t.arg(0), b"11");
}

#[test]
fn separator_runs_tabs_and_leading_blanks() {
    let mut t = Tokenizer::default();
    assert!(t.parse(b" \t stvls\t\t1   a  b \t", ALL));
    assert!(t.is(b"stvls"));
    assert_eq!(t.argc(), 3);
    assert_eq!(t.arg(0), b"1");
    assert_eq!(t.arg(1), b"a");
    assert_eq!(t.arg(2), b"b");
}

#[test]
fn empty_and_blank_lines() {
    let mut t = Tokenizer::default();
    assert!(!t.parse(b"", ALL));
    assert!(t.command().is_empty());
    assert_eq!(t.argc(), 0);
    assert!(!t.parse(b" \t  ", ALL));
    assert!(t.command().is_empty());
    // C++ t.parse(nullptr): a Rust slice is never null; the empty line is the same case
    assert!(!t.parse(b"", ALL));
    assert!(!t.too_many_args());
    assert!(!t.is(b""));
}

#[test]
fn argument_limit() {
    let mut t = Tokenizer::default();
    assert!(t.parse(b"smotc 1 2 3 4 5 ", 5));
    assert_eq!(t.argc(), 5);
    assert_eq!(t.arg(4), b"5");

    assert!(!t.parse(b"smotc 1 2 3 4 5 6", 5));
    assert!(t.too_many_args());
    assert!(t.is(b"smotc"));
    assert_eq!(t.argc(), 5);
    assert_eq!(t.arg(4), b"5");

    assert!(!t.parse(b"gvers x", 0));
    assert!(t.too_many_args());
    assert_eq!(t.argc(), 0);
    assert!(t.parse(b"gvers ", 0));
    assert!(!t.too_many_args());
}

#[test]
fn max_args_is_capped_at_max_args() {
    let mut t = Tokenizer::default();
    assert!(!t.parse(b"c 1 2 3 4 5 6 7 8 9", 200));
    assert!(t.too_many_args());
    assert_eq!(t.argc(), Tokenizer::MAX_ARGS);
    assert_eq!(t.arg(Tokenizer::MAX_ARGS - 1), b"8");

    assert!(t.parse(b"c 1 2 3 4 5 6 7 8", 255));
    assert_eq!(t.argc(), Tokenizer::MAX_ARGS);
}

#[test]
fn missing_arguments_read_as_empty_strings() {
    let mut t = Tokenizer::default();
    assert!(t.parse(b"gvlvd 1", ALL));
    assert!(t.arg(1).is_empty());
    assert!(t.arg(255).is_empty());
}

#[test]
fn exact_command_match() {
    let mut t = Tokenizer::default();
    assert!(t.parse(b"stgtpX 1 2", ALL));
    assert!(!t.is(b"stgtp"));
    assert!(t.is(b"stgtpX"));
    assert!(t.parse(b"stg", ALL));
    assert!(!t.is(b"stgtp"));
    // C++ t.is(nullptr): a Rust slice is never null; the empty name is the nearest case
    assert!(!t.is(b""));
    assert!(!t.is(b"STG"));
}

#[test]
fn state_is_reset_between_lines() {
    let mut t = Tokenizer::default();
    assert!(!t.parse(b"a 1 2 3", 2));
    assert!(t.too_many_args());
    assert!(t.parse(b"b", 2));
    assert!(t.is(b"b"));
    assert_eq!(t.argc(), 0);
    assert!(!t.too_many_args());
}

#[test]
fn long_tokens_are_kept_whole_and_terminated() {
    let long_arg: Vec<u8> = std::vec![b'z'; 200];
    let mut line = b"cmd ".to_vec();
    line.extend_from_slice(&long_arg);
    line.extend_from_slice(b" end");
    let mut t = Tokenizer::default();
    assert!(t.parse(&line, ALL));
    assert_eq!(t.arg(0), &long_arg[..]);
    assert_eq!(t.arg(1), b"end");
}

#[test]
fn checked_integer_arguments() {
    let mut t = Tokenizer::default();
    assert!(t.parse(b"stgtp 11 100 -5 x 65536 300", ALL));
    assert_eq!(t.arg_u32(0, 0, 11), Some(11));
    assert_eq!(t.arg_u32(0, 0, 10), None);
    assert_eq!(t.arg_u32(1, 0, 100), Some(100));
    assert_eq!(t.arg_u32(2, 0, 100), None);
    assert_eq!(t.arg_u32(3, 0, 100), None);
    assert_eq!(t.arg_u32(6, 0, 100), None); // missing

    assert_eq!(t.arg_i32(2, -10, 10), Some(-5));
    assert_eq!(t.arg_i32(2, 0, 10), None);
    assert_eq!(t.arg_i32(9, -10, 10), None);

    assert_eq!(t.arg_u16(4, 0, 65535), None);
    assert_eq!(t.arg_u16(1, 0, 65535), Some(100));
    assert_eq!(t.arg_u16(3, 0, 65535), None);

    assert_eq!(t.arg_u8(5, 0, 255), None);
    assert_eq!(t.arg_u8(0, 0, 255), Some(11));
    assert_eq!(t.arg_u8(1, 0, 99), None);
}

#[test]
fn default_state_before_any_parse() {
    let t = Tokenizer::default();
    assert_eq!(t.argc(), 0);
    assert!(!t.too_many_args());
    assert!(t.command().is_empty());
    assert!(t.arg(0).is_empty());
}

#[test]
fn arguments_of_a_previous_line_are_not_visible() {
    let mut t = Tokenizer::default();
    assert!(t.parse(b"c 1 2 -3", 5));
    assert_eq!(t.argc(), 3);
    assert!(t.parse(b"c 1", 5));
    assert_eq!(t.argc(), 1);
    assert!(t.arg(1).is_empty());
    assert_eq!(t.arg_u32(1, 0, 10), None);
    assert_eq!(t.arg_i32(1, -10, 10), None);
}
