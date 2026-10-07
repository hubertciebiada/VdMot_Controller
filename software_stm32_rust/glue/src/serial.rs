//! `HardwareSerial` of the STM32 core as the C++ firmware uses it (framework-arduinoststm32 2.x
//! `HardwareSerial.cpp`, `uart.c` and the F4 HAL's `HAL_UART_IRQHandler`), docs/rust/
//! GLUE-DESIGN-STM.md §4.1: a receive ring and a transmit ring of 1024 slots each (1023 bytes
//! usable: a byte that would make the head reach the tail is dropped), filled and drained by
//! the USART interrupt. The firmware's interrupt handler only reads USART_SR and USART_DR and
//! hands them to [`Port::on_irq`]; what happens with them is decided here.
//!
//! Per receive interrupt the error flags of SR become the HAL error code of that byte (PE, NE,
//! FE, ORE), the byte is stored all the same, and the counters of gstax count the code and a
//! byte that finds the ring full (`comm_rx_irq` of communication.cpp, core `count_uart_errors`).
//! An interrupt with error flags but without RXNE stores and counts nothing (the HAL clears the
//! flag in its error callback; the core's receive callback does not run).
//!
//! A port lives in a firmware static and is shared between its interrupt (producer of the
//! receive ring, consumer of the transmit ring) and the main loop (the other ends): one
//! producer and one consumer per ring, so atomics suffice (no unsafe in the glue).

use core::sync::atomic::{AtomicU16, AtomicU32, AtomicU8, Ordering};

use vdm_stm_core::uart_errors::{count_uart_errors, UartErrorCounters};

use crate::hal::Serial;

/// `SERIAL_RX_BUFFER_SIZE`, `SERIAL_TX_BUFFER_SIZE` of the C++ release envs.
pub const RING_SIZE: usize = 1024;

// USART_SR (RM0368 / RM0383 §19.6.1)
pub const SR_PE: u32 = 1 << 0;
pub const SR_FE: u32 = 1 << 1;
pub const SR_NE: u32 = 1 << 2;
pub const SR_ORE: u32 = 1 << 3;
pub const SR_RXNE: u32 = 1 << 5;
pub const SR_TC: u32 = 1 << 6;
pub const SR_TXE: u32 = 1 << 7;

/// `HAL_UART_ERROR_*` (stm32f4xx_hal_uart.h); core `UART_ERROR_*` has the same values.
pub const HAL_UART_ERROR_PE: u32 = 0x01;
pub const HAL_UART_ERROR_NE: u32 = 0x02;
pub const HAL_UART_ERROR_FE: u32 = 0x04;
pub const HAL_UART_ERROR_ORE: u32 = 0x08;

/// What the interrupt handler does with the transmitter after [`Port::on_irq`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tx {
    /// TXE is not set: nothing
    None,
    /// write this byte to DR
    Send(u8),
    /// the transmit ring is empty: TXE interrupt off
    Idle,
}

/// One ring of the core's `serial_t`: `head` is written by the producer, `tail` by the
/// consumer, one slot stays free.
struct Ring {
    buf: [AtomicU8; RING_SIZE],
    head: AtomicU16,
    tail: AtomicU16,
}

/// The index after `i` (wraps at RING_SIZE).
fn next(i: u16) -> u16 {
    let n = usize::from(i) + 1;
    if n == RING_SIZE {
        0
    } else {
        n as u16
    }
}

impl Ring {
    const fn new() -> Self {
        Ring {
            buf: [const { AtomicU8::new(0) }; RING_SIZE],
            head: AtomicU16::new(0),
            tail: AtomicU16::new(0),
        }
    }

    /// producer: no room for one more byte
    fn full(&self) -> bool {
        next(self.head.load(Ordering::Acquire)) == self.tail.load(Ordering::Acquire)
    }

    /// producer: false when the ring is full (the byte is dropped)
    fn push(&self, byte: u8) -> bool {
        let head = self.head.load(Ordering::Acquire);
        let n = next(head);
        if n == self.tail.load(Ordering::Acquire) {
            return false;
        }
        self.buf[usize::from(head)].store(byte, Ordering::Relaxed);
        self.head.store(n, Ordering::Release);
        true
    }

    /// consumer
    fn pop(&self) -> Option<u8> {
        let tail = self.tail.load(Ordering::Acquire);
        if tail == self.head.load(Ordering::Acquire) {
            return None;
        }
        let byte = self.buf[usize::from(tail)].load(Ordering::Relaxed);
        self.tail.store(next(tail), Ordering::Release);
        Some(byte)
    }

    /// bytes in the ring (`(SIZE + head - tail) % SIZE`)
    fn len(&self) -> usize {
        let head = usize::from(self.head.load(Ordering::Acquire));
        let tail = usize::from(self.tail.load(Ordering::Acquire));
        (RING_SIZE + head - tail) % RING_SIZE
    }
}

/// One USART: its rings and the receive error counters.
pub struct Port {
    rx: Ring,
    tx: Ring,
    overrun: AtomicU32,
    framing: AtomicU32,
    noise: AtomicU32,
    dropped: AtomicU32,
}

impl Default for Port {
    fn default() -> Self {
        Self::new()
    }
}

/// The HAL error code of a receive interrupt from its SR.
pub fn hal_error_code(sr: u32) -> u32 {
    let mut code = 0;
    if sr & SR_PE != 0 {
        code += HAL_UART_ERROR_PE;
    }
    if sr & SR_NE != 0 {
        code += HAL_UART_ERROR_NE;
    }
    if sr & SR_FE != 0 {
        code += HAL_UART_ERROR_FE;
    }
    if sr & SR_ORE != 0 {
        code += HAL_UART_ERROR_ORE;
    }
    code
}

impl Port {
    pub const fn new() -> Self {
        Port {
            rx: Ring::new(),
            tx: Ring::new(),
            overrun: AtomicU32::new(0),
            framing: AtomicU32::new(0),
            noise: AtomicU32::new(0),
            dropped: AtomicU32::new(0),
        }
    }

    /// The USART interrupt: `sr` is USART_SR as read first, `dr` USART_DR as read after it
    /// (only meaningful with RXNE). Returns what the handler does with the transmitter.
    pub fn on_irq(&self, sr: u32, dr: u8) -> Tx {
        if sr & SR_RXNE != 0 {
            let full = self.rx.full();
            // only this interrupt writes the counters
            let mut c = self.errors();
            count_uart_errors(&mut c, hal_error_code(sr), full);
            self.overrun.store(c.overrun, Ordering::Relaxed);
            self.framing.store(c.framing, Ordering::Relaxed);
            self.noise.store(c.noise, Ordering::Relaxed);
            self.dropped.store(c.dropped, Ordering::Relaxed);
            self.rx.push(dr);
        }
        if sr & SR_TXE == 0 {
            return Tx::None;
        }
        match self.tx.pop() {
            Some(byte) => Tx::Send(byte),
            None => Tx::Idle,
        }
    }

    /// The receive error counters (gstax uartOre, uartFe, uartNe, rxDropped).
    pub fn errors(&self) -> UartErrorCounters {
        UartErrorCounters {
            overrun: self.overrun.load(Ordering::Relaxed),
            framing: self.framing.load(Ordering::Relaxed),
            noise: self.noise.load(Ordering::Relaxed),
            dropped: self.dropped.load(Ordering::Relaxed),
        }
    }

    /// Bytes received and not read yet.
    pub fn available(&self) -> usize {
        self.rx.len()
    }

    pub fn read(&self) -> Option<u8> {
        self.rx.pop()
    }

    /// Queues a byte for the transmitter; false when the ring is full.
    pub fn push_tx(&self, byte: u8) -> bool {
        self.tx.push(byte)
    }

    /// The next byte to send (the interrupt, or a writer that cannot wait for it).
    pub fn pop_tx(&self) -> Option<u8> {
        self.tx.pop()
    }

    /// Bytes queued for the transmitter.
    pub fn tx_pending(&self) -> usize {
        self.tx.len()
    }
}

/// What a writer needs of the USART besides the rings (the firmware's registers).
pub trait UsartTx {
    /// TXE interrupt on: the interrupt sends what the ring holds.
    fn start(&self);
    /// The USART interrupt cannot run here (interrupts masked at its priority): nobody else
    /// drains the transmit ring.
    fn masked(&self) -> bool;
    /// USART_SR
    fn sr(&self) -> u32;
    /// writes USART_DR
    fn send(&self, byte: u8);
}

/// A port with its USART as a [`Serial`]: a handle the modules may copy.
#[derive(Clone, Copy)]
pub struct PortSerial<'a, U> {
    pub port: &'a Port,
    pub usart: U,
}

impl<U: UsartTx> PortSerial<'_, U> {
    /// While the interrupt cannot run, one byte goes out from here when the transmitter takes
    /// it (STM32duino would wait for ever, until the IWDG resets).
    fn poll(&self) {
        if self.usart.masked() && self.usart.sr() & SR_TXE != 0 {
            if let Some(byte) = self.port.pop_tx() {
                self.usart.send(byte);
            }
        }
    }
}

impl<U: UsartTx> Serial for PortSerial<'_, U> {
    fn available(&self) -> usize {
        self.port.available()
    }

    fn read(&mut self) -> Option<u8> {
        self.port.read()
    }

    /// Blocks while the transmit ring is full.
    fn write(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        for &byte in bytes {
            while !self.port.push_tx(byte) {
                self.usart.start();
                self.poll();
            }
        }
        self.usart.start();
    }

    /// The transmit ring empty and the last byte out of the shift register (TC).
    fn flush(&mut self) {
        while self.port.tx_pending() != 0 || self.usart.sr() & SR_TC == 0 {
            self.usart.start();
            self.poll();
        }
    }
}

#[cfg(test)]
mod tests;
