//! USART1 of the boot window, polled (B2: the boot stage runs without interrupts; the C++
//! `BootLoop` reads the ring its RX interrupt fills): when the data register is read and
//! written, over the flags of [`BootIo`].

use crate::io::BootIo;
use crate::poll::spin_until;

/// The received byte, if USART1 holds one: DR is read only when SR shows RXNE (a DR read
/// without it would return the last byte again). A byte with a parity, framing or noise error,
/// or the one ahead of an overrun, comes with RXNE and is returned as the C++ ring stores it;
/// the SR read and this DR read clear the error flags.
pub fn rx<I: BootIo>(io: &mut I) -> Option<u8> {
    if io.uart_rx_ready() {
        Some(io.uart_read())
    } else {
        None
    }
}

/// Sends one byte: waits for TXE (bounded), then writes DR.
pub fn tx<I: BootIo>(io: &mut I, byte: u8) {
    spin_until(|| io.uart_tx_empty());
    io.uart_write(byte);
}

/// `Serial1.flush()`: waits (bounded) until TC shows that the last byte has left the shift
/// register.
pub fn flush<I: BootIo>(io: &mut I) {
    spin_until(|| io.uart_tx_complete());
}

#[cfg(test)]
mod tests;
