//! Port of the LineAssembler case of test/native/test_fuzz.cpp (same seed and iteration
//! count).

use super::*;
use crate::test_support::{random_bytes, random_protocol_text, Rng};
use std::vec::Vec;

/// The reference's own copy of the line character rule.
fn ref_line_char(c: u8) -> bool {
    c == b'\t' || (0x20..0x7F).contains(&c)
}

/// Reference line splitter used to cross-check the assembler.
struct Reference {
    lines: Vec<Vec<u8>>,
    overflows: u32,
    malformed: u32,
}

fn reference_split(input: &[u8], capacity: usize) -> Reference {
    let mut r = Reference {
        lines: Vec::new(),
        overflows: 0,
        malformed: 0,
    };
    let mut cur = Vec::new();
    let mut bad = false;
    let mut over = false;
    for &c in input {
        if c == b'\r' || c == b'\n' {
            if !bad && !over && !cur.is_empty() {
                r.lines.push(cur.clone());
            }
            cur.clear();
            bad = false;
            over = false;
            continue;
        }
        if bad || over {
            continue;
        }
        if !ref_line_char(c) {
            bad = true;
            r.malformed += 1;
        } else if cur.len() + 1 >= capacity {
            over = true;
            r.overflows += 1;
        } else {
            cur.push(c);
        }
    }
    r
}

#[test]
fn line_assembler_matches_a_reference_splitter_on_random_bytes() {
    let mut rng = Rng::new(0x5eed1234);
    for round in 0..2000usize {
        let capacity = 2 + round % 40;
        let input = if round % 2 == 1 {
            random_bytes(&mut rng, 300)
        } else {
            random_protocol_text(&mut rng, 300)
        };
        let mut storage = std::vec![b'#'; capacity + 1]; // sentinel after the buffer
        let mut lines = Vec::new();
        let (overflows, malformed);
        {
            let mut la = LineAssembler::new(&mut storage[..capacity]);
            let mut pos = 0;
            while pos < input.len() {
                let n = rng.range_usize(1, 17).min(input.len() - pos);
                let used = la.feed(&input[pos..pos + n]);
                assert!(used <= n);
                pos += used;
                assert!(la.length() < capacity);
                assert_eq!(la.line().len(), la.length());
                if la.has_line() {
                    assert!(la.length() > 0);
                    lines.push(la.line().to_vec());
                    la.release();
                }
            }
            overflows = la.overflow_count();
            malformed = la.malformed_count();
        }
        assert_eq!(storage[capacity], b'#');

        let r = reference_split(&input, capacity);
        assert_eq!(lines, r.lines, "round {round}");
        assert_eq!(overflows, r.overflows, "round {round}");
        assert_eq!(malformed, r.malformed, "round {round}");
        for l in &lines {
            assert!(l.iter().all(|&c| ref_line_char(c)));
        }
    }
}
