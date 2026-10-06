//! The registers of the boot stage as traits. `firmware/src/boot_hw.rs` implements them on the
//! PAC (polled, no interrupts); the tests implement them with a fake board that keeps time in
//! microseconds (`test_support`). Every decision of the boot stage stays in this crate.

use crate::capture::NOINIT_LEN;
use crate::id_block::BootId;

/// SysTick and the HSE oscillator.
pub trait ClockIo {
    /// SysTick on the processor clock without interrupt, counting from `reload` down; the
    /// count flag of a previous run is cleared.
    fn tick_start(&mut self, reload: u32);
    /// SysTick COUNTFLAG: true once per elapsed period (the read clears it).
    fn tick(&mut self) -> bool;
    /// SysTick off and back to its reset values.
    fn tick_stop(&mut self);
    /// RCC_CR.HSEON = 1
    fn hse_on(&mut self);
    /// RCC_CR.HSERDY
    fn hse_ready(&mut self) -> bool;
    /// RCC_CR.HSEON = 0
    fn hse_off(&mut self);
    /// SYSCLK from the ready HSE without PLL (RCC_CFGR.SW = HSE).
    fn sysclk_hse(&mut self);
}

/// The boot window (`BootSetup`, `BootLoop`): USART1 115200 8E1 polled, the LED on PC13
/// (low = on).
pub trait BootIo: ClockIo {
    /// PC13 as push-pull output (`pinMode(LED, OUTPUT)`).
    fn led_begin(&mut self);
    fn set_led(&mut self, high: bool);
    /// The output latch of the LED pin (`digitalRead` of an output).
    fn led(&self) -> bool;
    /// USART1 115200 8E1 with this BRR (PCLK2 = SYSCLK during the boot stage), receiver and
    /// transmitter on, PA9 TX / PA10 RX in AF7.
    fn uart_begin(&mut self, brr: u16);
    /// The received byte, if USART1 holds one (RXNE; a byte with a parity, framing or noise
    /// error is returned too, as the C++ ring stores it).
    fn rx(&mut self) -> Option<u8>;
    /// Sends one byte (waits for TXE).
    fn tx(&mut self, byte: u8);
    /// Waits until the last byte has left the shift register (TC): `Serial1.flush()`.
    fn flush(&mut self);
    /// USART1 back to its reset state (RCC reset pulse).
    fn uart_end(&mut self);
}

/// Everything else the boot stage touches before the window.
pub trait BootHw: BootIo {
    /// RCC_CSR as found after the reset.
    fn reset_flags(&mut self) -> u32;
    /// RCC_CSR.RMVF = 1: the flags accumulate until cleared.
    fn clear_reset_flags(&mut self);
    /// The 212 bytes of the C++ 2.1.7 `.noinit` cells (volatile copy).
    fn noinit(&mut self) -> [u8; NOINIT_LEN];
    fn set_noinit(&mut self, cells: &[u8; NOINIT_LEN]);
    /// `valve_pins_safe`: PB9 latch high, then open drain (valve PSU off); ENA0..5 (PA5, PA6,
    /// PA7, PB0, PA15, PB3) outputs low.
    fn outputs_safe(&mut self);
    /// The handshake pattern and reply from the ID block in flash.
    fn boot_id(&mut self) -> BootId;
}
