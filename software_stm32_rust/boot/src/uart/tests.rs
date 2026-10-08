// New cases (the C++ window reads a ring its RX interrupt fills and sends through
// HardwareSerial): the register sequences of the polled USART1 against a model of SR and DR.
#![allow(
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::panic
)]

use std::vec;
use std::vec::Vec;

use super::*;
use crate::io::ClockIo;
use crate::poll::SPIN_LIMIT;

/// What the boot stage did with USART1, in order (a flag read with the value it gave).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    RxReady(bool),
    Read(u8),
    TxEmpty(bool),
    TxComplete(bool),
    Write(u8),
}

/// SR and DR: RXNE while a received byte waits; TXE and TC from the poll given on (1: the
/// first; 0: never).
struct Usart {
    ops: Vec<Op>,
    dr: Option<u8>,
    txe_at: u32,
    tc_at: u32,
    txe_polls: u32,
    tc_polls: u32,
}

impl Usart {
    fn new() -> Self {
        Usart {
            ops: Vec::new(),
            dr: None,
            txe_at: 1,
            tc_at: 1,
            txe_polls: 0,
            tc_polls: 0,
        }
    }

    fn count(&self, pred: impl Fn(&Op) -> bool) -> usize {
        self.ops.iter().filter(|o| pred(o)).count()
    }
}

impl ClockIo for Usart {
    fn tick_start(&mut self, _reload: u32) {}
    fn tick(&mut self) -> bool {
        false
    }
    fn tick_stop(&mut self) {}
    fn hse_on(&mut self) {}
    fn hse_ready(&mut self) -> bool {
        false
    }
    fn hse_off(&mut self) {}
    fn sysclk_select_hse(&mut self) {}
    fn sysclk_is_hse(&mut self) -> bool {
        false
    }
}

impl BootIo for Usart {
    fn led_begin(&mut self) {}
    fn set_led(&mut self, _high: bool) {}
    fn led(&self) -> bool {
        false
    }
    fn uart_begin(&mut self, _brr: u16) {}

    fn uart_rx_ready(&mut self) -> bool {
        let ready = self.dr.is_some();
        self.ops.push(Op::RxReady(ready));
        ready
    }

    fn uart_read(&mut self) -> u8 {
        let Some(byte) = self.dr.take() else {
            panic!("DR read without RXNE");
        };
        self.ops.push(Op::Read(byte));
        byte
    }

    fn uart_tx_empty(&mut self) -> bool {
        self.txe_polls += 1;
        let empty = self.txe_polls == self.txe_at;
        self.ops.push(Op::TxEmpty(empty));
        empty
    }

    fn uart_tx_complete(&mut self) -> bool {
        self.tc_polls += 1;
        let complete = self.tc_polls == self.tc_at;
        self.ops.push(Op::TxComplete(complete));
        complete
    }

    fn uart_write(&mut self, byte: u8) {
        self.ops.push(Op::Write(byte));
        self.txe_polls = 0;
    }

    fn uart_end(&mut self) {}
}

// ---- receive

#[test]
fn rx_without_rxne_reads_only_sr() {
    let mut u = Usart::new();
    assert_eq!(rx(&mut u), None);
    assert_eq!(u.ops, vec![Op::RxReady(false)]);
}

#[test]
fn rx_with_rxne_reads_sr_then_dr_and_returns_the_byte_once() {
    let mut u = Usart::new();
    u.dr = Some(b'D');
    assert_eq!(rx(&mut u), Some(b'D'));
    assert_eq!(u.ops, vec![Op::RxReady(true), Op::Read(b'D')]);
    assert_eq!(rx(&mut u), None);
    assert_eq!(u.ops[2..], [Op::RxReady(false)]);
}

// ---- transmit

#[test]
fn tx_with_txe_set_writes_dr_after_one_poll() {
    let mut u = Usart::new();
    tx(&mut u, b'B');
    assert_eq!(u.ops, vec![Op::TxEmpty(true), Op::Write(b'B')]);
}

#[test]
fn tx_waits_for_txe_before_it_writes_dr() {
    let mut u = Usart::new();
    u.txe_at = 3;
    tx(&mut u, b'E');
    assert_eq!(
        u.ops,
        vec![
            Op::TxEmpty(false),
            Op::TxEmpty(false),
            Op::TxEmpty(true),
            Op::Write(b'E')
        ]
    );
}

#[test]
fn tx_writes_after_the_bound_when_txe_never_comes() {
    let mut u = Usart::new();
    u.txe_at = 0;
    tx(&mut u, b'X');
    assert_eq!(
        u.count(|o| matches!(o, Op::TxEmpty(_))),
        SPIN_LIMIT as usize
    );
    assert_eq!(u.ops.last(), Some(&Op::Write(b'X')));
    assert_eq!(u.ops.len(), SPIN_LIMIT as usize + 1);
}

#[test]
fn flush_waits_for_tc_and_writes_nothing() {
    let mut u = Usart::new();
    u.tc_at = 2;
    flush(&mut u);
    assert_eq!(u.ops, vec![Op::TxComplete(false), Op::TxComplete(true)]);
}

#[test]
fn flush_ends_after_the_bound_when_tc_never_comes() {
    let mut u = Usart::new();
    u.tc_at = 0;
    flush(&mut u);
    assert_eq!(u.ops.len(), SPIN_LIMIT as usize);
    assert!(u.ops.iter().all(|o| *o == Op::TxComplete(false)));
}

#[test]
fn a_reply_waits_for_txe_before_every_byte_then_for_tc() {
    let mut u = Usart::new();
    for &b in b"OK" {
        tx(&mut u, b);
    }
    flush(&mut u);
    assert_eq!(
        u.ops,
        vec![
            Op::TxEmpty(true),
            Op::Write(b'O'),
            Op::TxEmpty(true),
            Op::Write(b'K'),
            Op::TxComplete(true)
        ]
    );
}
