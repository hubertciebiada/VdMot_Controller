// The check of the application part (D9): the CRC, the record and the steps.
#![allow(
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::unwrap_used
)]

use std::vec::Vec;

use super::*;

/// Flash from APP_START on; reads are logged, erased (0xFF) outside.
struct Flash {
    app: Vec<u8>,
    reads: Vec<(u32, usize)>,
}

impl Flash {
    fn new(app: &[u8]) -> Flash {
        Flash {
            app: app.to_vec(),
            reads: Vec::new(),
        }
    }
}

impl FlashRead for Flash {
    fn flash_read(&mut self, addr: u32, out: &mut [u8]) {
        self.reads.push((addr, out.len()));
        for (i, b) in out.iter_mut().enumerate() {
            let at = (addr as usize + i).checked_sub(APP_START as usize);
            *b = at.and_then(|a| self.app.get(a)).copied().unwrap_or(0xFF);
        }
    }
}

fn app(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i * 13 + 5) as u8).collect()
}

#[test]
fn crc32_is_the_zlib_crc() {
    // the check value of CRC-32/ISO-HDLC
    assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    assert_eq!(crc32(b""), 0);
    assert_eq!(crc32(&[0]), 0xD202_EF8D);
    assert_eq!(crc32(&[0xFF; 4]), 0xFFFF_FFFF);
    // in pieces as in one
    let data = app(100);
    let reg = crc32_update(crc32_update(!0, &data[..37]), &data[37..]);
    assert_eq!(!reg, crc32(&data));
}

#[test]
fn the_record_holds_magic_start_length_and_crc() {
    let data = app(300);
    assert_eq!(
        record(&data),
        [APP_CHECK_MAGIC, 0x0800_4000, 300, crc32(&data)]
    );
    assert_eq!(APP_CHECK_MAGIC.to_le_bytes(), *b"VDAC");
    assert_eq!(APP_CHECK_MAGIC, 0x4341_4456);
    assert_eq!(APP_START, 0x0800_4000);
    assert_eq!(APP_MAX_LEN, 128 * 1024 - 16 * 1024);
    // behind the 64 bytes of the ID block, inside sector 0
    assert_eq!(RECORD_ADDR, crate::id_block::ID_BLOCK_ADDR + 64);
    assert_eq!(RECORD_WORDS * 4, 16);
}

#[test]
fn a_matching_application_part_passes() {
    let len = 125 * STEP_BYTES + 3;
    let data = app(len);
    let mut flash = Flash::new(&data);
    assert!(AppCheck::new(record(&data)).finish(&mut flash));
    // every byte once, in steps of STEP_BYTES and the rest, in address order
    let total: usize = flash.reads.iter().map(|&(_, n)| n).sum();
    assert_eq!(total, len);
    let mut at = APP_START;
    for &(addr, n) in &flash.reads {
        assert_eq!(addr, at);
        at += n as u32;
    }
    assert_eq!(flash.reads.len(), 126);
    assert!(flash.reads[..125].iter().all(|&(_, n)| n == STEP_BYTES));
    assert_eq!(
        flash.reads.last(),
        Some(&(APP_START + 125 * STEP_BYTES as u32, 3))
    );
}

#[test]
fn one_changed_or_erased_byte_fails() {
    let data = app(1000);
    let rec = record(&data);
    for at in [0usize, 1, 500, 998, 999] {
        let mut other = data.clone();
        other[at] ^= 0x01;
        assert!(!AppCheck::new(rec).finish(&mut Flash::new(&other)), "{at}");
    }
    // erased behind byte 600 (a flash that stopped there)
    assert!(!AppCheck::new(rec).finish(&mut Flash::new(&data[..600])));
    // all erased
    assert!(!AppCheck::new(rec).finish(&mut Flash::new(&[])));
}

#[test]
fn a_record_that_is_not_patched_or_out_of_range_fails_without_reading() {
    let data = app(64);
    let good = record(&data);
    let bad = [
        // the placeholder of an image that was not patched
        [!0, !0, !0, !0],
        [0, 0, 0, 0],
        [APP_CHECK_MAGIC ^ 1, good[1], good[2], good[3]],
        [good[0], APP_START + 4, good[2], good[3]],
        [good[0], APP_START - 0x4000, good[2], good[3]],
        [good[0], good[1], APP_MAX_LEN + 1, good[3]],
        [good[0], good[1], u32::MAX, good[3]],
    ];
    for rec in bad {
        let mut flash = Flash::new(&data);
        assert!(!AppCheck::new(rec).finish(&mut flash), "{rec:x?}");
        assert!(flash.reads.is_empty(), "{rec:x?}");
    }
    // a record of the right form with another CRC reads the part and fails
    let mut flash = Flash::new(&data);
    let other = [good[0], good[1], good[2], good[3] ^ 0x8000_0000];
    assert!(!AppCheck::new(other).finish(&mut flash));
    assert_eq!(flash.reads.len(), 64 / STEP_BYTES);
    // one byte more or less than the part
    for len in [63, 65] {
        let rec = [good[0], good[1], len, good[3]];
        assert!(!AppCheck::new(rec).finish(&mut Flash::new(&data)), "{len}");
    }
    // the longest part is fine
    let long = app(APP_MAX_LEN as usize);
    assert!(AppCheck::new(record(&long)).finish(&mut Flash::new(&long)));
}

#[test]
fn an_empty_application_part_needs_the_crc_of_nothing() {
    assert!(AppCheck::new([APP_CHECK_MAGIC, APP_START, 0, 0]).finish(&mut Flash::new(&[])));
    assert!(!AppCheck::new([APP_CHECK_MAGIC, APP_START, 0, 1]).finish(&mut Flash::new(&[])));
}

#[test]
fn steps_advance_one_chunk_each_and_stop_at_the_result() {
    let s = STEP_BYTES as u32;
    let data = app(2 * STEP_BYTES + 5);
    let mut flash = Flash::new(&data);
    let mut check = AppCheck::new(record(&data));
    check.step(&mut flash);
    assert_eq!(flash.reads, [(APP_START, STEP_BYTES)]);
    check.step(&mut flash);
    check.step(&mut flash);
    assert_eq!(flash.reads.len(), 3);
    assert_eq!(flash.reads[1], (APP_START + s, STEP_BYTES));
    assert_eq!(flash.reads[2], (APP_START + 2 * s, 5));
    // the step after the last byte compares; later steps read nothing
    check.step(&mut flash);
    check.step(&mut flash);
    assert_eq!(flash.reads.len(), 3);
    assert!(check.finish(&mut flash));
    assert_eq!(flash.reads.len(), 3);
    // finish() on a check that is done reads nothing either
    let mut failed = AppCheck::new([0; RECORD_WORDS]);
    failed.step(&mut flash);
    assert!(!failed.finish(&mut flash));
    assert_eq!(flash.reads.len(), 3);
}
