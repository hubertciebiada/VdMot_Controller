// New cases (no C++ counterpart: the C++ uses the STM32 core's twi.c): the register sequence of
// the master against a model of the I2C v1 peripheral that only allows the RM0368 §18.3.3
// order, with a slave at 0x50 that NACKs on request, a bus that stays busy, lost arbitration,
// bus errors and flags that never come.

use super::*;
use crate::i2c_bus::{I2cLine, LineMode};
use std::cell::{Cell, RefCell};
use std::vec;
use std::vec::Vec;

/// What the master did, in order (SR1 polls are only counted).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    Sr2,
    Clear(u16),
    Start(bool),
    Stop,
    Ack(bool),
    Dr(u8),
    ReadDr(u8),
    End,
    Begin,
    Line,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum St {
    Idle,
    /// START requested, SB pending
    Started,
    /// the address byte is out: ADDR (or AF) pending
    Address,
    Writing,
    Reading,
}

const SLAVE: u8 = 0x50;
const RX: [u8; 4] = [0x11, 0x22, 0x33, 0x44];

/// The peripheral with its slave.
struct Hw {
    ops: RefCell<Vec<Op>>,
    polls: Cell<u32>,
    st: Cell<St>,
    /// R/W bit of the address byte
    read: Cell<bool>,
    addr_flag: Cell<bool>,
    sr1_seen: Cell<bool>,
    af: Cell<bool>,
    fault: Cell<u16>,
    fault_fired: Cell<bool>,
    written: Cell<usize>,
    taken: Cell<usize>,
    violations: RefCell<Vec<&'static str>>,
    // ---- behaviour
    /// SR2 reads that still see BUSY
    busy_reads: Cell<u32>,
    no_sb: bool,
    nack_addr: bool,
    nack_data_at: Option<usize>,
    /// SR1 shows these flags (ARLO, BERR) from the first poll in this state on
    fault_in: Option<(St, u16)>,
    /// RXNE stops after this many bytes
    rx_stall_after: Option<usize>,
}

impl Hw {
    fn new() -> Self {
        Hw {
            ops: RefCell::new(Vec::new()),
            polls: Cell::new(0),
            st: Cell::new(St::Idle),
            read: Cell::new(false),
            addr_flag: Cell::new(false),
            sr1_seen: Cell::new(false),
            af: Cell::new(false),
            fault: Cell::new(0),
            fault_fired: Cell::new(false),
            written: Cell::new(0),
            taken: Cell::new(0),
            violations: RefCell::new(Vec::new()),
            busy_reads: Cell::new(0),
            no_sb: false,
            nack_addr: false,
            nack_data_at: None,
            fault_in: None,
            rx_stall_after: None,
        }
    }

    fn op(&self, op: Op) {
        self.ops.borrow_mut().push(op);
    }

    fn violation(&self, what: &'static str) {
        self.violations.borrow_mut().push(what);
    }

    fn ops(&self) -> Vec<Op> {
        self.ops.borrow().clone()
    }

    fn rx_ready(&self) -> bool {
        let taken = self.taken.get();
        taken < RX.len() && self.rx_stall_after.is_none_or(|n| taken < n)
    }

    fn assert_clean(&self) {
        assert_eq!(*self.violations.borrow(), Vec::<&str>::new());
    }
}

impl I2cRegs for &Hw {
    fn sr1(&self) -> u16 {
        self.polls.set(self.polls.get() + 1);
        let st = self.st.get();
        if let Some((at, flags)) = self.fault_in {
            if st == at && !self.fault_fired.get() {
                self.fault_fired.set(true);
                self.fault.set(flags);
            }
        }
        let mut f = self.fault.get();
        if st == St::Started && !self.no_sb {
            f |= SR1_SB;
        }
        if st == St::Address && self.addr_flag.get() {
            f |= SR1_ADDR;
            self.sr1_seen.set(true);
        }
        if self.af.get() {
            f |= SR1_AF;
        }
        if st == St::Writing {
            f |= SR1_TXE;
            if self.written.get() > 0 {
                f |= SR1_BTF;
            }
        }
        if st == St::Reading && self.rx_ready() {
            f |= SR1_RXNE;
        }
        f
    }

    fn sr2(&self) -> u16 {
        self.op(Op::Sr2);
        if self.busy_reads.get() > 0 {
            self.busy_reads.set(self.busy_reads.get() - 1);
            return SR2_BUSY;
        }
        if self.st.get() == St::Address && self.addr_flag.get() && self.sr1_seen.get() {
            self.addr_flag.set(false);
            self.st.set(if self.read.get() {
                St::Reading
            } else {
                St::Writing
            });
        }
        0
    }

    fn clear_sr1(&self, flags: u16) {
        self.op(Op::Clear(flags));
        if flags & SR1_AF != 0 {
            self.af.set(false);
        }
        self.fault.set(self.fault.get() & !flags);
    }

    fn start(&self, ack: bool) {
        self.op(Op::Start(ack));
        if self.st.get() != St::Idle {
            self.violation("START during a transfer");
        }
        self.st.set(St::Started);
    }

    fn stop(&self) {
        self.op(Op::Stop);
        // a receiver still gets the byte on the way after STOP
        if self.st.get() != St::Reading {
            self.st.set(St::Idle);
        }
    }

    fn set_ack(&self, on: bool) {
        self.op(Op::Ack(on));
    }

    fn write_dr(&self, byte: u8) {
        self.op(Op::Dr(byte));
        match self.st.get() {
            St::Started if !self.no_sb => {
                self.read.set(byte & 1 != 0);
                self.st.set(St::Address);
                if byte >> 1 == SLAVE && !self.nack_addr {
                    self.addr_flag.set(true);
                } else {
                    self.af.set(true);
                }
            }
            St::Writing if !self.af.get() => {
                if self.nack_data_at == Some(self.written.get()) {
                    self.af.set(true);
                }
                self.written.set(self.written.get() + 1);
            }
            _ => self.violation("DR written out of turn"),
        }
    }

    fn read_dr(&self) -> u8 {
        if self.st.get() != St::Reading || !self.rx_ready() {
            self.violation("DR read without RXNE");
            self.op(Op::ReadDr(0));
            return 0;
        }
        let b = RX[self.taken.get()];
        self.taken.set(self.taken.get() + 1);
        self.op(Op::ReadDr(b));
        b
    }
}

impl RecoveryPins for &Hw {
    fn mode(&mut self, _line: I2cLine, _mode: LineMode) {
        self.op(Op::Line);
    }

    fn write(&mut self, _line: I2cLine, _high: bool) {
        self.op(Op::Line);
    }

    fn read(&mut self, _line: I2cLine) -> bool {
        true
    }
}

impl Wire for &Hw {
    fn end(&mut self) {
        self.op(Op::End);
    }

    fn begin(&mut self) {
        self.op(Op::Begin);
    }
}

/// Every millis() call is one millisecond later: a wait polls once per millisecond.
struct Ms(Cell<u32>);

impl Ms {
    fn new() -> Self {
        Ms(Cell::new(1000))
    }
}

impl Clock for &Ms {
    fn millis(&self) -> u32 {
        let t = self.0.get();
        self.0.set(t.wrapping_add(1));
        t
    }

    fn micros(&self) -> u32 {
        self.0.get().wrapping_mul(1000)
    }

    fn delay_ms(&self, ms: u32) {
        self.0.set(self.0.get().wrapping_add(ms));
    }

    fn delay_us(&self, _us: u32) {}
}

fn master<'a>(hw: &'a Hw, ms: &'a Ms) -> I2cV1<&'a Hw, &'a Ms> {
    I2cV1::new(hw, ms)
}

// ---- writes

#[test]
fn a_write_sends_the_address_then_each_byte_on_txe_waits_for_btf_and_stops() {
    let hw = Hw::new();
    let ms = Ms::new();
    let mut m = master(&hw, &ms);
    assert_eq!(m.write(SLAVE, &[0x00, 0x07, 0x5A]), WIRE_OK);
    assert_eq!(
        hw.ops(),
        vec![
            Op::Sr2,
            Op::Start(false),
            Op::Dr(0xA0),
            Op::Sr2,
            Op::Dr(0x00),
            Op::Dr(0x07),
            Op::Dr(0x5A),
            Op::Stop
        ]
    );
    assert_eq!(hw.written.get(), 3);
    assert_eq!(hw.st.get(), St::Idle);
    // SB, ADDR, TXE before each byte, BTF
    assert_eq!(hw.polls.get(), 6);
    hw.assert_clean();
}

#[test]
fn a_nack_of_the_last_byte_is_seen_by_the_wait_for_btf() {
    let mut hw = Hw::new();
    hw.nack_data_at = Some(2);
    let ms = Ms::new();
    let mut m = master(&hw, &ms);
    assert_eq!(m.write(SLAVE, &[7, 8, 9]), WIRE_NACK);
    let ops = hw.ops();
    assert_eq!(
        ops[ops.len() - 3..],
        [Op::Dr(9), Op::Clear(SR1_AF), Op::Stop]
    );
    hw.assert_clean();
}

#[test]
fn a_probe_without_bytes_stops_after_the_address_without_waiting_for_btf() {
    let hw = Hw::new();
    let ms = Ms::new();
    let mut m = master(&hw, &ms);
    assert_eq!(m.write(SLAVE, &[]), WIRE_OK);
    assert_eq!(
        hw.ops(),
        vec![Op::Sr2, Op::Start(false), Op::Dr(0xA0), Op::Sr2, Op::Stop]
    );
    // SB, ADDR: one poll each
    assert_eq!(hw.polls.get(), 2);
    hw.assert_clean();
}

#[test]
fn an_address_nack_clears_af_sends_stop_and_gives_2() {
    let mut hw = Hw::new();
    hw.nack_addr = true;
    let ms = Ms::new();
    let mut m = master(&hw, &ms);
    assert_eq!(m.write(SLAVE, &[1, 2]), WIRE_NACK);
    assert_eq!(
        hw.ops(),
        vec![
            Op::Sr2,
            Op::Start(false),
            Op::Dr(0xA0),
            Op::Clear(SR1_AF),
            Op::Stop
        ]
    );
    assert!(!hw.af.get());
    hw.assert_clean();
}

#[test]
fn an_absent_slave_is_a_nack() {
    let hw = Hw::new();
    let ms = Ms::new();
    let mut m = master(&hw, &ms);
    assert_eq!(m.write(0x51, &[]), WIRE_NACK);
    assert!(hw.ops().contains(&Op::Dr(0xA2)));
}

#[test]
fn a_data_nack_ends_the_write_after_that_byte_with_2() {
    let mut hw = Hw::new();
    hw.nack_data_at = Some(1);
    let ms = Ms::new();
    let mut m = master(&hw, &ms);
    assert_eq!(m.write(SLAVE, &[7, 8, 9]), WIRE_NACK);
    let ops = hw.ops();
    assert_eq!(
        ops[ops.len() - 4..],
        [Op::Dr(7), Op::Dr(8), Op::Clear(SR1_AF), Op::Stop]
    );
    hw.assert_clean();
}

#[test]
fn a_missing_start_bit_times_out_after_more_than_100_ms_with_a_stop() {
    let mut hw = Hw::new();
    hw.no_sb = true;
    let ms = Ms::new();
    let mut m = master(&hw, &ms);
    assert_eq!(m.write(SLAVE, &[1]), WIRE_TIMEOUT);
    assert_eq!(hw.ops(), vec![Op::Sr2, Op::Start(false), Op::Stop]);
    // one poll per millisecond: the wait gives up when 101 ms have passed
    assert_eq!(hw.polls.get(), 101);
}

#[test]
fn arbitration_lost_gives_4_with_arlo_and_berr_cleared_and_a_stop() {
    let mut hw = Hw::new();
    hw.fault_in = Some((St::Writing, SR1_ARLO));
    let ms = Ms::new();
    let mut m = master(&hw, &ms);
    assert_eq!(m.write(SLAVE, &[1]), WIRE_ERROR);
    let ops = hw.ops();
    assert_eq!(
        ops[ops.len() - 2..],
        [Op::Clear(SR1_ARLO | SR1_BERR), Op::Stop]
    );
    assert_eq!(hw.fault.get(), 0);
    assert_eq!(hw.written.get(), 0);
}

#[test]
fn a_bus_error_gives_4() {
    let mut hw = Hw::new();
    hw.fault_in = Some((St::Started, SR1_BERR));
    let ms = Ms::new();
    let mut m = master(&hw, &ms);
    assert_eq!(m.write(SLAVE, &[]), WIRE_ERROR);
    assert_eq!(
        hw.ops(),
        vec![
            Op::Sr2,
            Op::Start(false),
            Op::Clear(SR1_ARLO | SR1_BERR),
            Op::Stop
        ]
    );
}

#[test]
fn a_nack_wins_over_a_bus_error_of_the_same_poll() {
    let mut hw = Hw::new();
    hw.nack_addr = true;
    hw.fault_in = Some((St::Address, SR1_BERR));
    let ms = Ms::new();
    let mut m = master(&hw, &ms);
    assert_eq!(m.write(SLAVE, &[]), WIRE_NACK);
}

#[test]
fn a_bus_busy_for_more_than_100_ms_gives_4_without_a_start() {
    let hw = Hw::new();
    hw.busy_reads.set(u32::MAX);
    let ms = Ms::new();
    let mut m = master(&hw, &ms);
    assert_eq!(m.write(SLAVE, &[1]), WIRE_ERROR);
    let ops = hw.ops();
    assert!(ops.iter().all(|o| *o == Op::Sr2), "{ops:?}");
    // one SR2 read per millisecond: the wait gives up when 101 ms have passed
    assert_eq!(ops.len(), 101);
}

#[test]
fn a_bus_busy_for_100_ms_is_waited_for() {
    let hw = Hw::new();
    hw.busy_reads.set(100);
    let ms = Ms::new();
    let mut m = master(&hw, &ms);
    assert_eq!(m.write(SLAVE, &[]), WIRE_OK);
    assert_eq!(
        hw.ops()[100..],
        [Op::Sr2, Op::Start(false), Op::Dr(0xA0), Op::Sr2, Op::Stop]
    );
}

#[test]
fn a_start_bit_after_100_ms_of_polls_is_waited_for() {
    // SB shows from the 101st poll on: still in time
    struct LateSb<'a>(&'a Hw, Cell<u32>);
    let hw = Hw::new();
    let ms = Ms::new();
    let late = LateSb(&hw, Cell::new(0));
    impl I2cRegs for &LateSb<'_> {
        fn sr1(&self) -> u16 {
            self.1.set(self.1.get() + 1);
            if self.1.get() <= 100 {
                0
            } else {
                (&self.0).sr1()
            }
        }
        fn sr2(&self) -> u16 {
            (&self.0).sr2()
        }
        fn clear_sr1(&self, flags: u16) {
            (&self.0).clear_sr1(flags)
        }
        fn start(&self, ack: bool) {
            (&self.0).start(ack)
        }
        fn stop(&self) {
            (&self.0).stop()
        }
        fn set_ack(&self, on: bool) {
            (&self.0).set_ack(on)
        }
        fn write_dr(&self, byte: u8) {
            (&self.0).write_dr(byte)
        }
        fn read_dr(&self) -> u8 {
            (&self.0).read_dr()
        }
    }
    let m = I2cV1::new(&late, &ms);
    assert_eq!(m.write_bytes(SLAVE, &[]), Ok(()));
    hw.assert_clean();
}

// ---- reads

#[test]
fn a_read_of_one_byte_nacks_it_before_addr_is_cleared_then_stops() {
    let hw = Hw::new();
    let ms = Ms::new();
    let mut m = master(&hw, &ms);
    let mut buf = [0u8; 1];
    assert_eq!(m.read(SLAVE, &mut buf), 1);
    assert_eq!(buf, [0x11]);
    assert_eq!(
        hw.ops(),
        vec![
            Op::Sr2,
            Op::Start(true),
            Op::Dr(0xA1),
            Op::Ack(false),
            Op::Sr2,
            Op::Ack(false),
            Op::Stop,
            Op::ReadDr(0x11)
        ]
    );
    hw.assert_clean();
}

#[test]
fn a_read_of_three_bytes_acks_two_then_nack_and_stop_before_the_last() {
    let hw = Hw::new();
    let ms = Ms::new();
    let mut m = master(&hw, &ms);
    let mut buf = [0u8; 3];
    assert_eq!(m.read(SLAVE, &mut buf), 3);
    assert_eq!(buf, [0x11, 0x22, 0x33]);
    assert_eq!(
        hw.ops(),
        vec![
            Op::Sr2,
            Op::Start(true),
            Op::Dr(0xA1),
            Op::Sr2,
            Op::ReadDr(0x11),
            Op::ReadDr(0x22),
            Op::Ack(false),
            Op::Stop,
            Op::ReadDr(0x33)
        ]
    );
    hw.assert_clean();
}

#[test]
fn a_read_into_nothing_touches_nothing() {
    let hw = Hw::new();
    let ms = Ms::new();
    let mut m = master(&hw, &ms);
    assert_eq!(m.read(SLAVE, &mut []), 0);
    assert_eq!(hw.ops(), Vec::<Op>::new());
}

#[test]
fn a_read_with_an_address_nack_gives_0() {
    let mut hw = Hw::new();
    hw.nack_addr = true;
    let ms = Ms::new();
    let mut m = master(&hw, &ms);
    let mut buf = [0u8; 2];
    assert_eq!(m.read(SLAVE, &mut buf), 0);
    assert_eq!(
        hw.ops(),
        vec![
            Op::Sr2,
            Op::Start(true),
            Op::Dr(0xA1),
            Op::Clear(SR1_AF),
            Op::Stop
        ]
    );
}

#[test]
fn a_read_whose_bytes_stop_coming_times_out_with_a_stop_and_gives_0() {
    let mut hw = Hw::new();
    hw.rx_stall_after = Some(1);
    let ms = Ms::new();
    let mut m = master(&hw, &ms);
    let mut buf = [0u8; 3];
    assert_eq!(m.read(SLAVE, &mut buf), 0);
    let ops = hw.ops();
    assert_eq!(ops[ops.len() - 2..], [Op::ReadDr(0x11), Op::Stop]);
    hw.assert_clean();
}

#[test]
fn the_last_byte_that_never_comes_times_out_after_the_stop() {
    let mut hw = Hw::new();
    hw.rx_stall_after = Some(2);
    let ms = Ms::new();
    let mut m = master(&hw, &ms);
    let mut buf = [0u8; 3];
    assert_eq!(m.read(SLAVE, &mut buf), 0);
    let ops = hw.ops();
    assert_eq!(
        ops[ops.len() - 5..],
        [
            Op::ReadDr(0x11),
            Op::ReadDr(0x22),
            Op::Ack(false),
            Op::Stop,
            Op::Stop
        ]
    );
}

// ---- restart

#[test]
fn restart_stops_the_peripheral_frees_the_bus_and_starts_it_again() {
    let hw = Hw::new();
    let ms = Ms::new();
    let mut m = master(&hw, &ms);
    m.restart();
    let ops = hw.ops();
    assert_eq!(ops.first(), Some(&Op::End));
    assert_eq!(ops.last(), Some(&Op::Begin));
    assert!(ops[1..ops.len() - 1].iter().all(|o| *o == Op::Line));
    assert!(ops.len() > 2);
}
