//! Step 6a of the boot stage (docs/rust/GLUE-DESIGN-STM.md §5.2), `JumpToBootloader` of the C++
//! (`src/boot_jump.cpp`): the clocks back to their reset state (`HAL_RCC_DeInit`: HSI on and
//! SYSCLK, CFGR 0, HSE, CSS and the PLLs off), SysTick off and back to its reset values,
//! interrupts off, the system memory at address 0, then MSP and PC from the ROM bootloader's
//! vector table. VTOR stays 0: with MEMRMP = 01 address 0 is the system memory (R2: only
//! hardware proves the ROM bootloader start, §5.9).
//!
//! The two waits for a clock flag are bounded (B2); after the bound the sequence goes on (the
//! C++ ignores a timeout of `HAL_RCC_DeInit` as well and jumps).

use crate::io::JumpIo;
use crate::poll::spin_until;

/// The jump into the ROM bootloader; never returns.
pub fn jump<J: JumpIo>(io: &mut J) -> ! {
    io.hsi_on();
    spin_until(|| io.hsi_ready());
    io.cfgr_reset();
    spin_until(|| io.sysclk_is_hsi());
    io.oscillators_off();
    io.tick_stop();
    io.irq_disable();
    io.syscfg_on();
    io.system_memory_at_0();
    io.bootload()
}

#[cfg(test)]
mod tests;
