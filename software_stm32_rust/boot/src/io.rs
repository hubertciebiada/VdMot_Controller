//! The registers of the boot stage as traits. `firmware/src/boot_hw.rs` implements them on the
//! PAC (polled, no interrupts); the tests implement them with a fake board that keeps time in
//! microseconds (`test_support`). Every call is one register access (a flag read, a write) or
//! a fixed set-up sequence without a branch; every decision of the boot stage, every wait and
//! its bound stays in this crate.

use crate::app_check::RECORD_WORDS;
use crate::capture::NOINIT_LEN;
use crate::id_block::BootId;

/// The flash of the image (the check of the application part, D9).
pub trait FlashRead {
    /// The bytes from `addr` on (volatile reads: the image as flashed, not as linked).
    fn flash_read(&mut self, addr: u32, out: &mut [u8]);
}

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
    /// RCC_CFGR.SW = HSE: SYSCLK from the ready HSE without PLL (the switch takes a few cycles).
    fn sysclk_select_hse(&mut self);
    /// RCC_CFGR.SWS = HSE: the switch is done.
    fn sysclk_is_hse(&mut self) -> bool;
}

/// The boot window (`BootSetup`, `BootLoop`): USART1 115200 8E1 polled, the LED on PC13
/// (low = on). `crate::uart` decides when the data register is read and written.
pub trait BootIo: ClockIo {
    /// PC13 as push-pull output (`pinMode(LED, OUTPUT)`).
    fn led_begin(&mut self);
    fn set_led(&mut self, high: bool);
    /// The output latch of the LED pin (`digitalRead` of an output).
    fn led(&self) -> bool;
    /// USART1 115200 8E1 with this BRR (PCLK2 = SYSCLK during the boot stage), receiver and
    /// transmitter on, PA9 TX / PA10 RX in AF7.
    fn uart_begin(&mut self, brr: u16);
    /// USART1_SR.RXNE: a received byte waits in DR (a byte with a parity, framing or noise
    /// error, or one ahead of an overrun, sets it too).
    fn uart_rx_ready(&mut self) -> bool;
    /// Reads USART1_DR; after the SR read of [`BootIo::uart_rx_ready`] it clears RXNE and the
    /// error flags.
    fn uart_read(&mut self) -> u8;
    /// USART1_SR.TXE: DR takes the next byte.
    fn uart_tx_empty(&mut self) -> bool;
    /// USART1_SR.TC: the last byte has left the shift register.
    fn uart_tx_complete(&mut self) -> bool;
    /// Writes USART1_DR.
    fn uart_write(&mut self, byte: u8);
    /// USART1 back to its reset state (RCC reset pulse).
    fn uart_end(&mut self);
}

/// The independent watchdog (`crate::watchdog`).
pub trait IwdgIo {
    /// Writes IWDG_KR.
    fn iwdg_key(&mut self, key: u16);
    /// Writes IWDG_PR.
    fn iwdg_prescaler(&mut self, pr: u32);
    /// Writes IWDG_RLR.
    fn iwdg_reload_value(&mut self, rlr: u32);
    /// IWDG_SR.PVU or RVU: a written prescaler or reload value is still on its way into the
    /// LSI domain.
    fn iwdg_updating(&mut self) -> bool;
}

/// The jump into the ROM bootloader (`crate::jump`): the clocks back to their reset state, the
/// system memory at address 0.
pub trait JumpIo: ClockIo {
    /// RCC_CR.HSION = 1
    fn hsi_on(&mut self);
    /// RCC_CR.HSIRDY
    fn hsi_ready(&mut self) -> bool;
    /// RCC_CFGR = 0: SYSCLK from HSI, every prescaler 1.
    fn cfgr_reset(&mut self);
    /// RCC_CFGR.SWS = HSI: the switch is done.
    fn sysclk_is_hsi(&mut self) -> bool;
    /// RCC_CR: HSEON, HSEBYP, CSSON, PLLON and PLLI2SON = 0.
    fn oscillators_off(&mut self);
    /// PRIMASK = 1 (`__disable_irq`).
    fn irq_disable(&mut self);
    /// RCC_APB2ENR.SYSCFGEN = 1, read back (the clock is on two cycles later).
    fn syscfg_on(&mut self);
    /// SYSCFG_MEMRMP = 01: the system memory at address 0.
    fn system_memory_at_0(&mut self);
    /// MSP and PC from the ROM bootloader's vector table at 0x1FFF0000 (AN2606).
    fn bootload(&mut self) -> !;
}

/// Everything else the boot stage touches before the window, and the watchdog at its end.
pub trait BootHw: BootIo + FlashRead + IwdgIo {
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
    /// The record of the application part in sector 0 (`crate::app_check`), as flashed.
    fn app_record(&mut self) -> [u32; RECORD_WORDS];
}
