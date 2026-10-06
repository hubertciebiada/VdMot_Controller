//! Port of the Tokenizer case of test/native/test_fuzz.cpp (same seed and iteration count).

use super::*;
use crate::test_support::{random_protocol_text, Rng};

#[test]
fn tokenizer_invariants_on_random_lines() {
    let mut rng = Rng::new(0xC0FFEE);
    for round in 0..5000u32 {
        let mut line = random_protocol_text(&mut rng, 80);
        for c in &mut line {
            if *c == 0 {
                *c = b' ';
            }
        }
        let max_args = (round % 10) as u8;

        let mut t = Tokenizer::default();
        let ok = t.parse(&line, max_args);
        let limit = max_args.min(Tokenizer::MAX_ARGS);
        assert!(t.argc() <= limit);
        assert_eq!(ok, !t.too_many_args() && !t.command().is_empty());

        // Count tokens independently.
        let mut tokens = 0usize;
        let mut in_token = false;
        for &c in &line {
            let sep = c == b' ' || c == b'\t';
            if !sep && !in_token {
                tokens += 1;
            }
            in_token = !sep;
        }
        if tokens == 0 {
            assert!(!ok);
            assert_eq!(t.argc(), 0);
            continue;
        }
        assert_eq!(t.too_many_args(), tokens - 1 > usize::from(limit));
        assert_eq!(usize::from(t.argc()), (tokens - 1).min(usize::from(limit)));

        let base = line.as_ptr() as usize;
        let mut all = std::vec![t.command()];
        for i in 0..t.argc() {
            all.push(t.arg(i));
        }
        for token in all {
            // a non-empty part of the line without separators
            let start = token.as_ptr() as usize;
            assert!(start >= base && start + token.len() <= base + line.len());
            assert!(!token.is_empty());
            assert!(!token.contains(&b' '));
            assert!(!token.contains(&b'\t'));
        }
    }
}
