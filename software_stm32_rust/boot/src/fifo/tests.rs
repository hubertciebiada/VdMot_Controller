// The RX ring of the boot window: the STM32duino head/tail rule (1023 of 1024 slots), drops
// when full, 8-byte blocks only when 8 bytes wait.
#![allow(clippy::indexing_slicing, clippy::arithmetic_side_effects, clippy::unwrap_used)]

use super::*;

#[test]
fn holds_1023_bytes_and_drops_the_1024th() {
    let mut f = Fifo::new();
    for i in 0..1023u32 {
        assert!(f.push(i as u8), "{i}");
    }
    assert_eq!(f.len(), 1023);
    assert!(!f.push(0xEE));
    assert!(!f.push(0xEF));
    assert_eq!(f.dropped(), 2);
    assert_eq!(f.len(), 1023);
    for i in 0..1023u32 {
        assert_eq!(f.pop(), Some(i as u8));
    }
    assert_eq!(f.pop(), None);
    assert!(f.is_empty());
}

#[test]
fn keeps_the_order_across_the_wrap_of_the_ring() {
    let mut f = Fifo::new();
    for round in 0..5u32 {
        for i in 0..700u32 {
            assert!(f.push((i + round) as u8));
        }
        assert_eq!(f.len(), 700);
        for i in 0..700u32 {
            assert_eq!(f.pop(), Some((i + round) as u8));
        }
        assert_eq!(f.len(), 0);
    }
    assert_eq!(f.dropped(), 0);
}

#[test]
fn take_block_needs_8_bytes_and_takes_exactly_8() {
    let mut f = Fifo::new();
    for &b in b"DEADBEE" {
        f.push(b);
    }
    assert_eq!(f.take_block(), None);
    assert_eq!(f.len(), 7);
    f.push(b'F');
    f.push(b'\n');
    assert_eq!(f.take_block(), Some(*b"DEADBEEF"));
    assert_eq!(f.len(), 1);
    assert_eq!(f.pop(), Some(b'\n'));
}

#[test]
fn take_block_across_the_wrap() {
    let mut f = Fifo::new();
    for _ in 0..1020 {
        f.push(0);
        f.pop();
    }
    for &b in b"ABCDEFGH" {
        f.push(b);
    }
    assert_eq!(f.take_block(), Some(*b"ABCDEFGH"));
}

#[test]
fn clear_drops_what_waits() {
    let mut f = Fifo::new();
    for &b in b"xxxxxxxxxx" {
        f.push(b);
    }
    f.clear();
    assert!(f.is_empty());
    assert_eq!(f.len(), 0);
    assert_eq!(f.take_block(), None);
    assert!(f.push(b'a'));
    assert_eq!(f.pop(), Some(b'a'));
}

#[test]
fn default_is_empty() {
    let f = Fifo::default();
    assert!(f.is_empty());
    assert_eq!(f.dropped(), 0);
    assert_eq!(FIFO_SLOTS, 1024);
    assert_eq!(BLOCK_LEN, 8);
}
