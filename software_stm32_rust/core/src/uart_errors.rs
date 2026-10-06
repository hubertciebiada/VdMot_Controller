//! Counters of the receive errors of the ESP UART (gstax uartOre, uartFe, uartNe, rxDropped).
//! Hardware-free: the glue passes the HAL error code the UART interrupt set for a received byte
//! and whether the receive ring had no room for it.

// the HAL_UART_ERROR_* bits (the glue static_asserts them)
pub const UART_ERROR_PARITY: u32 = 0x01;
pub const UART_ERROR_NOISE: u32 = 0x02;
pub const UART_ERROR_FRAMING: u32 = 0x04;
pub const UART_ERROR_OVERRUN: u32 = 0x08;

/// since start-up; they wrap, consumers use differences
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UartErrorCounters {
    pub overrun: u32,
    pub framing: u32,
    /// noise and parity errors
    pub noise: u32,
    /// bytes that found the receive ring full
    pub dropped: u32,
}

/// one received byte: every error bit of hal_error_code counts once (parity counts as noise)
pub fn count_uart_errors(c: &mut UartErrorCounters, hal_error_code: u32, ring_full: bool) {
    if hal_error_code & UART_ERROR_OVERRUN != 0 {
        c.overrun = c.overrun.wrapping_add(1);
    }
    if hal_error_code & UART_ERROR_FRAMING != 0 {
        c.framing = c.framing.wrapping_add(1);
    }
    if hal_error_code & (UART_ERROR_NOISE | UART_ERROR_PARITY) != 0 {
        c.noise = c.noise.wrapping_add(1);
    }
    if ring_full {
        c.dropped = c.dropped.wrapping_add(1);
    }
}
