//! Port of the BufWriter case of test/native/test_fuzz.cpp (same seed and iteration counts).

use super::*;
use crate::test_support::{text, Rng};
use std::format;
use std::string::{String, ToString};
use std::vec;

#[test]
fn never_exceeds_its_capacity() {
    let mut rng = Rng::new(0xBADC0DE);
    for round in 0..3000 {
        let capacity = round % 50;
        let mut storage = vec![b'#'; capacity + 1]; // sentinel after the buffer
        {
            let mut w = BufWriter::new(&mut storage[..capacity]);
            let mut model = String::new();
            let mut model_ok = true;
            for _step in 0..30 {
                let op = rng.range_u32(0, 5);
                let v = rng.next_u32();
                let (piece, r) = match op {
                    0 => (v.to_string(), w.append_unsigned(v)),
                    1 => ((v as i32).to_string(), w.append_signed(v as i32)),
                    2 => (format!("{:02x}", v & 0xFF), w.append_hex2(v as u8)),
                    3 => {
                        let piece = "a".repeat((v % 7) as usize);
                        let r = w.append(piece.as_bytes());
                        (piece, r)
                    }
                    4 => ("x".to_string(), w.append_char(b'x')),
                    _ => {
                        let to = (v % 20) as usize;
                        w.truncate(to);
                        if to < model.len() {
                            model.truncate(to);
                        }
                        continue;
                    }
                };
                let fits = capacity > 0 && model.len() + piece.len() < capacity;
                assert_eq!(r, fits, "round {round}");
                if fits {
                    model += &piece;
                } else {
                    model_ok = false;
                }
                assert_eq!(text(w.as_bytes()), model, "round {round}");
                assert_eq!(w.ok(), model_ok, "round {round}");
            }
        }
        assert_eq!(storage[capacity], b'#', "round {round}");
    }
}
