//! The bounded wait for a hardware flag (B2: nothing in the boot stage waits without a bound).

/// Polls of a flag that settles within a few clock cycles (the switch of SYSCLK, HSI on),
/// within one character at 115200 Bd (TXE, TC: 95 us) or within a few LSI cycles (the IWDG
/// values in the LSI domain, well below 1 ms): far above all of them on any clock here, as a
/// poll takes at least a few cycles and 200,000 of them several milliseconds even at 96 MHz.
pub const SPIN_LIMIT: u32 = 200_000;

/// Polls `done` until it reports true, at most [`SPIN_LIMIT`] times; the caller goes on either
/// way.
pub fn spin_until(mut done: impl FnMut() -> bool) {
    for _ in 0..SPIN_LIMIT {
        if done() {
            return;
        }
    }
}

#[cfg(test)]
mod tests;
