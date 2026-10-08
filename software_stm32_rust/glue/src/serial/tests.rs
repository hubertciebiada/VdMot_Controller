// New cases (design §7.3, no C++ counterpart: the C++ suites run against a fake HardwareSerial):
// the HAL error-code rule per interrupt, the register sequence of the interrupt, the 1023-byte
// rings, the drop counting, the order of the transmitter and the blocking writes.

use std::cell::{Cell, RefCell};
use std::vec;
use std::vec::Vec;

use super::*;
use crate::irq::{PRIO_EXTI, PRIO_MOTOR};
use crate::test_support::io_fakes::FakeSerial;
use vdm_stm_core::uart_errors::{
    UART_ERROR_FRAMING, UART_ERROR_NOISE, UART_ERROR_OVERRUN, UART_ERROR_PARITY,
};

fn receive(port: &Port, bytes: &[u8]) {
    for &b in bytes {
        port.on_irq(SR_RXNE, b);
    }
}

fn drain(port: &Port) -> Vec<u8> {
    let mut out = Vec::new();
    while let Some(b) = port.read() {
        out.push(b);
    }
    out
}

#[test]
fn hal_error_bits_are_the_cores_and_one_per_flag() {
    assert_eq!(HAL_UART_ERROR_PE, UART_ERROR_PARITY);
    assert_eq!(HAL_UART_ERROR_NE, UART_ERROR_NOISE);
    assert_eq!(HAL_UART_ERROR_FE, UART_ERROR_FRAMING);
    assert_eq!(HAL_UART_ERROR_ORE, UART_ERROR_OVERRUN);
    assert_eq!(hal_error_code(0), 0);
    assert_eq!(hal_error_code(SR_PE), 0x01);
    assert_eq!(hal_error_code(SR_NE), 0x02);
    assert_eq!(hal_error_code(SR_FE), 0x04);
    assert_eq!(hal_error_code(SR_ORE), 0x08);
    assert_eq!(
        hal_error_code(SR_PE + SR_NE + SR_FE + SR_ORE + SR_RXNE + SR_TXE),
        0x0F
    );
    // the register bits (RM0368 §19.6.1)
    assert_eq!(
        [SR_PE, SR_FE, SR_NE, SR_ORE, SR_RXNE, SR_TC, SR_TXE],
        [0x01, 0x02, 0x04, 0x08, 0x20, 0x40, 0x80]
    );
}

#[test]
fn rx_bytes_come_out_in_order_across_the_wrap_of_the_ring() {
    let port = Port::new();
    let mut sent = Vec::new();
    let mut got = Vec::new();
    for round in 0..3u8 {
        let chunk = vec![b'a' + round; 700];
        receive(&port, &chunk);
        sent.extend_from_slice(&chunk);
        assert_eq!(port.available(), 700);
        got.extend(drain(&port));
    }
    assert_eq!(got, sent);
    assert_eq!(port.read(), None);
    assert_eq!(port.available(), 0);
    assert_eq!(port.errors(), UartErrorCounters::default());
}

#[test]
fn rx_the_ring_holds_1023_bytes_a_byte_that_finds_it_full_is_dropped_and_counted() {
    let port = Port::new();
    let bytes: Vec<u8> = (0..1030u32).map(|i| i as u8).collect();
    receive(&port, &bytes);
    assert_eq!(port.available(), RING_SIZE - 1);
    assert_eq!(port.errors().dropped, 7);
    // the first 1023 are kept, the later ones lost
    assert_eq!(drain(&port), bytes[..RING_SIZE - 1].to_vec());
    // room again
    receive(&port, b"z");
    assert_eq!(port.read(), Some(b'z'));
    assert_eq!(port.errors().dropped, 7);
}

#[test]
fn rx_the_same_counts_and_drops_as_the_fake_hardware_serial_of_the_c_plus_plus_suites() {
    let port = Port::new();
    let mut fake = FakeSerial::new();
    let bytes = vec![b'q'; 1500];
    receive(&port, &bytes);
    fake.inject(&bytes);
    assert_eq!(port.available(), fake.available());
    assert_eq!(port.errors(), fake.errors);
}

#[test]
fn rx_an_error_of_the_interrupt_is_counted_and_its_byte_stored() {
    let port = Port::new();
    port.on_irq(SR_RXNE + SR_FE, b'a');
    port.on_irq(SR_RXNE + SR_NE, b'b');
    port.on_irq(SR_RXNE + SR_PE, b'c');
    port.on_irq(SR_RXNE + SR_ORE, b'd');
    port.on_irq(SR_RXNE + SR_ORE + SR_FE + SR_NE, b'e');
    port.on_irq(SR_RXNE, b'f');
    let e = port.errors();
    assert_eq!(e.framing, 2);
    // parity counts as noise
    assert_eq!(e.noise, 3);
    assert_eq!(e.overrun, 2);
    assert_eq!(e.dropped, 0);
    assert_eq!(drain(&port), b"abcdef".to_vec());
}

#[test]
fn rx_error_flags_without_rxne_store_and_count_nothing() {
    // the HAL's error callback clears the flag, the core's receive callback does not run
    let port = Port::new();
    assert_eq!(port.on_irq(SR_FE + SR_NE + SR_PE + SR_ORE, b'x'), Tx::None);
    assert_eq!(port.available(), 0);
    assert_eq!(port.errors(), UartErrorCounters::default());
}

#[test]
fn rx_a_full_ring_counts_the_drop_and_the_error_of_the_same_byte() {
    let port = Port::new();
    receive(&port, &vec![0u8; RING_SIZE - 1]);
    port.on_irq(SR_RXNE + SR_ORE, 1);
    let e = port.errors();
    assert_eq!((e.dropped, e.overrun), (1, 1));
    assert_eq!(port.available(), RING_SIZE - 1);
}

#[test]
fn tx_the_interrupt_sends_the_queued_bytes_in_order_then_is_idle() {
    let port = Port::new();
    assert_eq!(port.on_irq(SR_TXE, 0), Tx::Idle);
    assert!(port.push_tx(b'1'));
    assert!(port.push_tx(b'2'));
    assert_eq!(port.tx_pending(), 2);
    // TXE not set (the shift register is busy): nothing
    assert_eq!(port.on_irq(0, 0), Tx::None);
    assert_eq!(port.on_irq(SR_TXE, 0), Tx::Send(b'1'));
    // a receive interrupt with TXE set also sends
    assert_eq!(port.on_irq(SR_RXNE + SR_TXE + SR_TC, b'r'), Tx::Send(b'2'));
    assert_eq!(port.on_irq(SR_TXE, 0), Tx::Idle);
    assert_eq!(port.tx_pending(), 0);
    assert_eq!(port.read(), Some(b'r'));
}

#[test]
fn tx_the_ring_takes_1023_bytes() {
    let port = Port::new();
    for i in 0..RING_SIZE - 1 {
        assert!(port.push_tx(i as u8), "byte {i}");
    }
    assert!(!port.push_tx(0xEE));
    assert_eq!(port.tx_pending(), RING_SIZE - 1);
    assert_eq!(port.pop_tx(), Some(0));
    assert!(port.push_tx(0xEE));
    let mut last = 0;
    while let Some(b) = port.pop_tx() {
        last = b;
    }
    assert_eq!(last, 0xEE);
    assert_eq!(port.pop_tx(), None);
}

// ---- the interrupt on the registers (the firmware's USART1 and USART6 handlers)

/// A register access of the interrupt, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Reg {
    Sr,
    ReadDr,
    WriteDr(u8),
    TxeIe(bool),
}

/// USART_SR and USART_DR as the interrupt finds them.
struct Regs {
    sr: u32,
    dr: u8,
    ops: RefCell<Vec<Reg>>,
}

impl UsartRegs for Regs {
    fn sr(&self) -> u32 {
        self.ops.borrow_mut().push(Reg::Sr);
        self.sr
    }

    fn read_dr(&self) -> u8 {
        self.ops.borrow_mut().push(Reg::ReadDr);
        self.dr
    }

    fn write_dr(&self, byte: u8) {
        self.ops.borrow_mut().push(Reg::WriteDr(byte));
    }

    fn set_txeie(&self, on: bool) {
        self.ops.borrow_mut().push(Reg::TxeIe(on));
    }
}

/// One interrupt with this SR and DR; its register accesses.
fn irq(port: &Port, sr: u32, dr: u8) -> Vec<Reg> {
    let regs = Regs {
        sr,
        dr,
        ops: RefCell::new(Vec::new()),
    };
    port.irq(&regs);
    regs.ops.take()
}

#[test]
fn irq_a_received_byte_reads_sr_then_dr_and_is_stored() {
    let port = Port::new();
    assert_eq!(irq(&port, SR_RXNE, b'a'), vec![Reg::Sr, Reg::ReadDr]);
    assert_eq!(port.read(), Some(b'a'));
    assert_eq!(port.errors(), UartErrorCounters::default());
}

#[test]
fn irq_a_byte_with_an_error_is_stored_and_counted() {
    let port = Port::new();
    assert_eq!(
        irq(&port, SR_RXNE + SR_FE, b'b'),
        vec![Reg::Sr, Reg::ReadDr]
    );
    assert_eq!(port.read(), Some(b'b'));
    assert_eq!(port.errors().framing, 1);
}

#[test]
fn irq_each_error_flag_alone_is_cleared_by_a_dr_read_that_stores_nothing() {
    for flag in [SR_ORE, SR_NE, SR_FE, SR_PE] {
        let port = Port::new();
        assert_eq!(
            irq(&port, flag, b'x'),
            vec![Reg::Sr, Reg::ReadDr],
            "{flag:#x}"
        );
        assert_eq!(port.available(), 0);
        assert_eq!(port.errors(), UartErrorCounters::default());
    }
}

#[test]
fn irq_without_a_byte_or_an_error_never_reads_dr() {
    // a DR read here could take a byte that arrives after the SR read, unstored
    let port = Port::new();
    assert_eq!(irq(&port, 0, b'x'), vec![Reg::Sr]);
    assert_eq!(irq(&port, SR_TC, b'x'), vec![Reg::Sr]);
    assert_eq!(
        irq(&port, SR_TXE + SR_TC, b'x'),
        vec![Reg::Sr, Reg::TxeIe(false)]
    );
    assert_eq!(port.available(), 0);
}

#[test]
fn irq_txe_sends_the_next_byte_then_turns_its_interrupt_off_when_the_ring_is_empty() {
    let port = Port::new();
    assert!(port.push_tx(b'1'));
    assert!(port.push_tx(b'2'));
    assert_eq!(irq(&port, SR_TXE, 0), vec![Reg::Sr, Reg::WriteDr(b'1')]);
    assert_eq!(
        irq(&port, SR_TXE + SR_TC, 0),
        vec![Reg::Sr, Reg::WriteDr(b'2')]
    );
    assert_eq!(irq(&port, SR_TXE, 0), vec![Reg::Sr, Reg::TxeIe(false)]);
    assert_eq!(port.tx_pending(), 0);
}

#[test]
fn irq_a_byte_in_and_a_byte_out_in_one_interrupt() {
    let port = Port::new();
    assert!(port.push_tx(b'o'));
    assert_eq!(
        irq(&port, SR_RXNE + SR_TXE, b'i'),
        vec![Reg::Sr, Reg::ReadDr, Reg::WriteDr(b'o')]
    );
    assert_eq!(
        irq(&port, SR_RXNE + SR_ORE + SR_TXE, b'j'),
        vec![Reg::Sr, Reg::ReadDr, Reg::TxeIe(false)]
    );
    assert_eq!(drain(&port), b"ij".to_vec());
    assert_eq!(port.errors().overrun, 1);
}

#[test]
fn irq_while_txe_is_clear_the_transmitter_is_left_alone() {
    let port = Port::new();
    assert!(port.push_tx(b'w'));
    assert_eq!(irq(&port, SR_RXNE, b'r'), vec![Reg::Sr, Reg::ReadDr]);
    assert_eq!(port.tx_pending(), 1);
}

// ---- the writer

/// A USART for the writer: TXEIE on lets the interrupt run at once when `isr` is set (it drains
/// the ring onto `wire` through [`Port::irq`]); otherwise only a writer that finds the
/// interrupt blocked sends. SR, PRIMASK and BASEPRI as set by the test; while `tc_clear` counts
/// down, the SR reads find TC clear (the last byte still in the shift register).
struct TestUsart<'a> {
    port: &'a Port,
    wire: &'a RefCell<Vec<u8>>,
    starts: &'a Cell<u32>,
    isr: bool,
    primask: bool,
    basepri: u8,
    sr: &'a Cell<u32>,
    tc_clear: &'a Cell<u32>,
    txeie: Cell<bool>,
}

impl UsartRegs for TestUsart<'_> {
    fn sr(&self) -> u32 {
        let left = self.tc_clear.get();
        if left > 0 {
            self.tc_clear.set(left - 1);
            return self.sr.get() & !SR_TC;
        }
        self.sr.get()
    }

    fn read_dr(&self) -> u8 {
        0
    }

    fn write_dr(&self, byte: u8) {
        self.wire.borrow_mut().push(byte);
    }

    fn set_txeie(&self, on: bool) {
        self.txeie.set(on);
        if on {
            self.starts.set(self.starts.get() + 1);
            while self.isr && self.txeie.get() {
                self.port.irq(self);
            }
        }
    }
}

impl Masks for TestUsart<'_> {
    fn primask(&self) -> bool {
        self.primask
    }

    fn basepri(&self) -> u8 {
        self.basepri
    }
}

/// What a writer test works with.
struct Rig {
    port: Port,
    wire: RefCell<Vec<u8>>,
    starts: Cell<u32>,
    sr: Cell<u32>,
    tc_clear: Cell<u32>,
}

impl Rig {
    fn new(sr: u32) -> Self {
        Rig {
            port: Port::new(),
            wire: RefCell::new(Vec::new()),
            starts: Cell::new(0),
            sr: Cell::new(sr),
            tc_clear: Cell::new(0),
        }
    }

    /// A writer whose interrupt runs (`isr`) or not, with the masks of its context.
    fn serial(&self, isr: bool, primask: bool, basepri: u8) -> PortSerial<'_, TestUsart<'_>> {
        PortSerial {
            port: &self.port,
            usart: TestUsart {
                port: &self.port,
                wire: &self.wire,
                starts: &self.starts,
                isr,
                primask,
                basepri,
                sr: &self.sr,
                tc_clear: &self.tc_clear,
                txeie: Cell::new(false),
            },
        }
    }

    fn wire(&self) -> Vec<u8> {
        self.wire.borrow().clone()
    }
}

#[test]
fn write_queues_the_bytes_and_starts_the_interrupt_once_per_write() {
    let rig = Rig::new(SR_TXE + SR_TC);
    let mut s = rig.serial(false, false, 0);
    s.write(b"hello");
    assert_eq!(rig.port.tx_pending(), 5);
    assert_eq!(rig.starts.get(), 1);
    assert!(s.usart.txeie.get());
    // nothing written: no start
    s.write(b"");
    assert_eq!(rig.starts.get(), 1);
    assert!(rig.wire().is_empty());
}

#[test]
fn write_of_more_than_the_ring_waits_for_the_interrupt() {
    let rig = Rig::new(SR_TXE + SR_TC);
    let mut s = rig.serial(true, false, 0);
    let text: Vec<u8> = (0..3000u32).map(|i| b'a' + (i % 26) as u8).collect();
    s.write(&text);
    assert_eq!(rig.wire(), text);
    // the ring filled twice: started for room, then once at the end
    assert_eq!(rig.starts.get(), 3);
    // the interrupt turned itself off with the ring empty
    assert!(!s.usart.txeie.get());
}

#[test]
fn write_with_primask_set_sends_from_the_writer_when_txe_is_set() {
    let rig = Rig::new(SR_TXE);
    let mut s = rig.serial(false, true, 0);
    let text = vec![b'm'; 1100];
    s.write(&text);
    // 1100 - 1023 bytes had to go out before the rest fitted
    assert_eq!(rig.wire().len(), 1100 - (RING_SIZE - 1));
    assert_eq!(rig.port.tx_pending(), RING_SIZE - 1);
}

#[test]
fn write_with_basepri_at_the_usart_level_sends_from_the_writer() {
    let rig = Rig::new(SR_TXE);
    let mut s = rig.serial(false, false, PRIO_USART);
    s.write(&vec![b'u'; RING_SIZE + 9]);
    assert_eq!(rig.wire().len(), 10);
}

#[test]
fn poll_sends_only_while_the_usart_interrupt_is_blocked_and_txe_is_set() {
    let rig = Rig::new(SR_TXE + SR_TC);
    assert!(rig.port.push_tx(1));
    // nothing masked, or BASEPRI of the motor lock (TIM1, TIM2) or at EXTI4: the interrupt
    // runs, so the writer leaves the ring to it
    for basepri in [0, PRIO_MOTOR, PRIO_EXTI] {
        rig.serial(false, false, basepri).poll();
        assert!(rig.wire().is_empty(), "{basepri:#x}");
    }
    // blocked, but the transmitter is busy
    rig.sr.set(0);
    rig.serial(false, true, 0).poll();
    assert!(rig.wire().is_empty());
    rig.sr.set(SR_TXE);
    rig.serial(false, true, 0).poll();
    assert_eq!(rig.wire(), vec![1]);
    // an empty ring sends nothing
    rig.serial(false, true, 0).poll();
    assert_eq!(rig.wire().len(), 1);
}

#[test]
fn flush_waits_for_the_empty_ring_and_tc() {
    let rig = Rig::new(SR_TXE + SR_TC);
    let mut s = rig.serial(true, false, 0);
    // empty ring and TC: returns at once
    s.flush();
    assert_eq!(rig.starts.get(), 0);
    assert!(rig.port.push_tx(b'x'));
    s.flush();
    assert_eq!(rig.wire(), b"x".to_vec());
    assert_eq!(rig.starts.get(), 1);
}

#[test]
fn flush_with_the_ring_empty_waits_until_tc_is_set() {
    let rig = Rig::new(SR_TXE + SR_TC);
    let mut s = rig.serial(true, false, 0);
    // the last byte is still in the shift register for the next three SR reads
    rig.tc_clear.set(3);
    s.flush();
    assert_eq!(rig.tc_clear.get(), 0);
    assert!(rig.starts.get() >= 1);
    // the same with the interrupt blocked and a byte left in the ring
    let mut s = rig.serial(false, true, 0);
    assert!(rig.port.push_tx(b'z'));
    rig.tc_clear.set(4);
    s.flush();
    assert_eq!(rig.wire(), b"z".to_vec());
    assert_eq!(rig.tc_clear.get(), 0);
}

#[test]
fn flush_with_the_interrupt_blocked_drains_the_ring_itself() {
    let rig = Rig::new(SR_TXE + SR_TC);
    let mut s = rig.serial(false, true, 0);
    s.write(b"abc");
    s.flush();
    assert_eq!(rig.wire(), b"abc".to_vec());
    assert_eq!(rig.port.tx_pending(), 0);
}

#[test]
fn serial_reads_what_the_interrupt_received() {
    let rig = Rig::new(SR_TXE + SR_TC);
    let mut s = rig.serial(false, false, 0);
    receive(&rig.port, b"ok");
    assert_eq!(s.available(), 2);
    assert_eq!(s.read(), Some(b'o'));
    assert_eq!(s.read(), Some(b'k'));
    assert_eq!(s.read(), None);
}
