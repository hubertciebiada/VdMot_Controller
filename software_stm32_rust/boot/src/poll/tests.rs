// New cases (the C++ waits for these flags inside the HAL, with HAL_GetTick timeouts): the
// bound of the boot stage's polls.
#![allow(clippy::arithmetic_side_effects)]

use super::*;

/// Polls until the flag shows at `ready_at` (1 = the first poll; 0: never); the number of polls.
fn polls(ready_at: u32) -> u32 {
    let mut n = 0u32;
    spin_until(|| {
        n += 1;
        n == ready_at
    });
    n
}

#[test]
fn a_flag_that_is_set_ends_the_wait_at_the_first_poll() {
    assert_eq!(polls(1), 1);
}

#[test]
fn the_wait_ends_at_the_poll_that_sees_the_flag() {
    assert_eq!(polls(2), 2);
    assert_eq!(polls(1000), 1000);
}

#[test]
fn a_flag_that_comes_at_the_last_poll_is_still_seen() {
    assert_eq!(polls(SPIN_LIMIT), SPIN_LIMIT);
}

#[test]
fn a_flag_that_never_comes_ends_the_wait_after_the_limit() {
    assert_eq!(polls(0), SPIN_LIMIT);
    assert_eq!(polls(SPIN_LIMIT + 1), SPIN_LIMIT);
    assert_eq!(SPIN_LIMIT, 200_000);
}
