// New cases (design §7.3, no C++ counterpart: the C++ suites run against a fake HardwareSerial):
// the HAL error-code rule per interrupt, the 1023-byte rings, the drop counting, the order of
// the transmitter and the blocking writes.

use std::cell::{Cell, RefCell};
use std::vec;
use std::vec::Vec;

use super::*;
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

/// A USART for the writer: the interrupt runs at once when `isr` is set (it drains the ring
/// into `wire`), otherwise only a masked writer sends; SR as set by the test.
struct TestUsart<'a> {
    port: &'a Port,
    wire: &'a RefCell<Vec<u8>>,
    starts: &'a Cell<u32>,
    isr: bool,
    masked: bool,
    sr: &'a Cell<u32>,
}

impl UsartTx for TestUsart<'_> {
    fn start(&self) {
        self.starts.set(self.starts.get() + 1);
        if self.isr {
            while let Tx::Send(b) = self.port.on_irq(SR_TXE, 0) {
                self.wire.borrow_mut().push(b);
            }
        }
    }

    fn masked(&self) -> bool {
        self.masked
    }

    fn sr(&self) -> u32 {
        self.sr.get()
    }

    fn send(&self, byte: u8) {
        self.wire.borrow_mut().push(byte);
    }
}

#[test]
fn write_queues_the_bytes_and_starts_the_interrupt_once_per_write() {
    let port = Port::new();
    let wire = RefCell::new(Vec::new());
    let starts = Cell::new(0);
    let sr = Cell::new(SR_TXE + SR_TC);
    let mut s = PortSerial {
        port: &port,
        usart: TestUsart {
            port: &port,
            wire: &wire,
            starts: &starts,
            isr: false,
            masked: false,
            sr: &sr,
        },
    };
    s.write(b"hello");
    assert_eq!(port.tx_pending(), 5);
    assert_eq!(starts.get(), 1);
    // nothing written: no start
    s.write(b"");
    assert_eq!(starts.get(), 1);
    assert!(wire.borrow().is_empty());
}

#[test]
fn write_of_more_than_the_ring_waits_for_the_interrupt() {
    let port = Port::new();
    let wire = RefCell::new(Vec::new());
    let starts = Cell::new(0);
    let sr = Cell::new(SR_TXE + SR_TC);
    let mut s = PortSerial {
        port: &port,
        usart: TestUsart {
            port: &port,
            wire: &wire,
            starts: &starts,
            isr: true,
            masked: false,
            sr: &sr,
        },
    };
    let text: Vec<u8> = (0..3000u32).map(|i| b'a' + (i % 26) as u8).collect();
    s.write(&text);
    assert_eq!(*wire.borrow(), text);
    // the ring filled twice: started for room, then once at the end
    assert_eq!(starts.get(), 3);
}

#[test]
fn write_with_the_interrupt_masked_sends_from_the_writer_when_txe_is_set() {
    let port = Port::new();
    let wire = RefCell::new(Vec::new());
    let starts = Cell::new(0);
    let sr = Cell::new(SR_TXE);
    let mut s = PortSerial {
        port: &port,
        usart: TestUsart {
            port: &port,
            wire: &wire,
            starts: &starts,
            isr: false,
            masked: true,
            sr: &sr,
        },
    };
    let text = vec![b'm'; 1100];
    s.write(&text);
    // 1100 - 1023 bytes had to go out before the rest fitted
    assert_eq!(wire.borrow().len(), 1100 - (RING_SIZE - 1));
    assert_eq!(port.tx_pending(), RING_SIZE - 1);
}

#[test]
fn write_unmasked_never_sends_from_the_writer_and_masked_not_without_txe() {
    let port = Port::new();
    let wire = RefCell::new(Vec::new());
    let starts = Cell::new(0);
    let sr = Cell::new(SR_TXE + SR_TC);
    let unmasked = PortSerial {
        port: &port,
        usart: TestUsart {
            port: &port,
            wire: &wire,
            starts: &starts,
            isr: false,
            masked: false,
            sr: &sr,
        },
    };
    assert!(port.push_tx(1));
    unmasked.poll();
    assert!(wire.borrow().is_empty());
    sr.set(0);
    let masked = PortSerial {
        port: &port,
        usart: TestUsart {
            port: &port,
            wire: &wire,
            starts: &starts,
            isr: false,
            masked: true,
            sr: &sr,
        },
    };
    masked.poll();
    assert!(wire.borrow().is_empty());
    sr.set(SR_TXE);
    masked.poll();
    assert_eq!(*wire.borrow(), vec![1]);
    // an empty ring sends nothing
    masked.poll();
    assert_eq!(wire.borrow().len(), 1);
}

#[test]
fn flush_waits_for_the_empty_ring_and_tc() {
    let port = Port::new();
    let wire = RefCell::new(Vec::new());
    let starts = Cell::new(0);
    let sr = Cell::new(SR_TXE + SR_TC);
    let mut s = PortSerial {
        port: &port,
        usart: TestUsart {
            port: &port,
            wire: &wire,
            starts: &starts,
            isr: true,
            masked: false,
            sr: &sr,
        },
    };
    // empty ring and TC: returns at once
    s.flush();
    assert_eq!(starts.get(), 0);
    assert!(port.push_tx(b'x'));
    s.flush();
    assert_eq!(*wire.borrow(), b"x".to_vec());
    assert_eq!(starts.get(), 1);
}

#[test]
fn flush_with_the_interrupt_masked_drains_the_ring_itself() {
    let port = Port::new();
    let wire = RefCell::new(Vec::new());
    let starts = Cell::new(0);
    let sr = Cell::new(SR_TXE + SR_TC);
    let mut s = PortSerial {
        port: &port,
        usart: TestUsart {
            port: &port,
            wire: &wire,
            starts: &starts,
            isr: false,
            masked: true,
            sr: &sr,
        },
    };
    s.write(b"abc");
    s.flush();
    assert_eq!(*wire.borrow(), b"abc".to_vec());
    assert_eq!(port.tx_pending(), 0);
}

#[test]
fn serial_reads_what_the_interrupt_received() {
    let port = Port::new();
    let wire = RefCell::new(Vec::new());
    let starts = Cell::new(0);
    let sr = Cell::new(SR_TXE + SR_TC);
    let mut s = PortSerial {
        port: &port,
        usart: TestUsart {
            port: &port,
            wire: &wire,
            starts: &starts,
            isr: false,
            masked: false,
            sr: &sr,
        },
    };
    receive(&port, b"ok");
    assert_eq!(s.available(), 2);
    assert_eq!(s.read(), Some(b'o'));
    assert_eq!(s.read(), Some(b'k'));
    assert_eq!(s.read(), None);
}
