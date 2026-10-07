//! The UART fake of the I/O suites (communication, eeprom, ow_devices, print, terminal): a
//! receive ring filled byte by byte through the RX interrupt of communication.cpp and the
//! transmitted bytes, as `fake_board.cpp` keeps them for `HardwareSerial`. The board (time,
//! pins, the I2C lines) is [`super::fake_board::FakeBoard`].

use std::collections::VecDeque;

use crate::communication::comm_rx_irq;
use crate::hal::Serial;
use vdm_stm_core::uart_errors::UartErrorCounters;

/// Receive ring size of the firmware (SERIAL_RX_BUFFER_SIZE): one slot stays free.
pub const RX_RING_SIZE: usize = 1024;

/// A UART: the receive ring as the RX interrupt fills it, the transmitted bytes.
#[derive(Default)]
pub struct FakeSerial {
    rx: VecDeque<u8>,
    pub tx: Vec<u8>,
    pub read_calls: u32,
    pub flushes: u32,
    /// HAL error code of the next received byte
    pub next_error: u32,
    /// the counters of the USART1 RX interrupt (communication::comm_rx_irq)
    pub errors: UartErrorCounters,
}

impl FakeSerial {
    pub fn new() -> Self {
        FakeSerial::default()
    }

    /// Bytes arrive one by one through the RX interrupt: its error code is counted and a byte
    /// that finds the ring full is dropped.
    pub fn inject(&mut self, bytes: &[u8]) {
        for &b in bytes {
            let ring_full = self.rx.len() == RX_RING_SIZE - 1;
            comm_rx_irq(&mut self.errors, self.next_error, ring_full);
            self.next_error = 0;
            if !ring_full {
                self.rx.push_back(b);
            }
        }
    }

    pub fn inject_str(&mut self, s: &str) {
        self.inject(s.as_bytes());
    }

    /// The bytes written since the last take, as text.
    pub fn take_tx(&mut self) -> String {
        let out = String::from_utf8_lossy(&self.tx).into_owned();
        self.tx.clear();
        out
    }
}

impl Serial for FakeSerial {
    fn available(&self) -> usize {
        self.rx.len()
    }

    fn read(&mut self) -> Option<u8> {
        self.read_calls += 1;
        self.rx.pop_front()
    }

    fn write(&mut self, bytes: &[u8]) {
        self.tx.extend_from_slice(bytes);
    }

    fn flush(&mut self) {
        self.flushes += 1;
    }
}
