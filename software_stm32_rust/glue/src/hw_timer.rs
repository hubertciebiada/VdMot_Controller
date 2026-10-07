//! The periods of the control timers (docs/rust/GLUE-DESIGN-STM.md §2.4): STM32_TimerInterrupt
//! 1.3.0 `attachInterruptInterval(us)` sets the STM32 core's `HardwareTimer::setOverflow(us,
//! MICROSEC_FORMAT)`, which computes prescaler and auto-reload for a 16-bit counter (also for
//! the 32-bit TIM2). The firmware writes the two values into TIM1 (1 ms) and TIM2 (10 ms).

/// The register values of one period: the counter runs `(psc + 1) x (arr + 1)` timer clocks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Overflow {
    /// TIMx_PSC
    pub psc: u32,
    /// TIMx_ARR
    pub arr: u32,
}

/// `HardwareTimer::setOverflow(period_us, MICROSEC_FORMAT)` at `timer_clk_hz`: the clock in
/// whole MHz, `cyc = us x MHz`, `factor = cyc / 65536 + 1`, `PSC = factor - 1`,
/// `ARR = cyc / factor - 1` (not below 0).
pub fn overflow(timer_clk_hz: u32, period_us: u32) -> Overflow {
    let cycles = period_us.wrapping_mul(timer_clk_hz / 1_000_000);
    let factor = cycles / 0x1_0000 + 1;
    Overflow {
        psc: factor - 1,
        arr: (cycles / factor).saturating_sub(1),
    }
}

#[cfg(test)]
mod tests;
