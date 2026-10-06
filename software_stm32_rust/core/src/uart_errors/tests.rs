//! Port of test/native/test_uart_errors.cpp.

use super::*;

fn equal(c: &UartErrorCounters, overrun: u32, framing: u32, noise: u32, dropped: u32) -> bool {
    c.overrun == overrun && c.framing == framing && c.noise == noise && c.dropped == dropped
}

#[test]
fn count_uart_errors_a_byte_without_error_counts_nothing() {
    let mut c = UartErrorCounters::default();
    count_uart_errors(&mut c, 0, false);
    assert!(equal(&c, 0, 0, 0, 0), "{c:?}");
}

#[test]
fn count_uart_errors_each_hal_error_bit_alone() {
    let mut c = UartErrorCounters::default();
    count_uart_errors(&mut c, UART_ERROR_OVERRUN, false);
    assert!(equal(&c, 1, 0, 0, 0), "{c:?}");
    count_uart_errors(&mut c, UART_ERROR_FRAMING, false);
    assert!(equal(&c, 1, 1, 0, 0), "{c:?}");
    count_uart_errors(&mut c, UART_ERROR_NOISE, false);
    assert!(equal(&c, 1, 1, 1, 0), "{c:?}");
    count_uart_errors(&mut c, UART_ERROR_PARITY, false); // parity counts as noise
    assert!(equal(&c, 1, 1, 2, 0), "{c:?}");
}

#[test]
fn count_uart_errors_combined_bits_count_once_each_parity_and_noise_together_once() {
    let mut c = UartErrorCounters::default();
    count_uart_errors(
        &mut c,
        UART_ERROR_OVERRUN | UART_ERROR_FRAMING | UART_ERROR_NOISE | UART_ERROR_PARITY,
        false,
    );
    assert!(equal(&c, 1, 1, 1, 0), "{c:?}");
    count_uart_errors(&mut c, 0xFFFF_FFF0, false); // bits the HAL has no counter for
    assert!(equal(&c, 1, 1, 1, 0), "{c:?}");
}

#[test]
fn count_uart_errors_a_full_ring_counts_a_dropped_byte_also_with_an_error() {
    let mut c = UartErrorCounters::default();
    count_uart_errors(&mut c, 0, true);
    assert!(equal(&c, 0, 0, 0, 1), "{c:?}");
    count_uart_errors(&mut c, UART_ERROR_FRAMING, true);
    assert!(equal(&c, 0, 1, 0, 2), "{c:?}");
}

#[test]
fn count_uart_errors_the_counters_wrap() {
    let mut c = UartErrorCounters {
        overrun: u32::MAX,
        framing: u32::MAX,
        noise: u32::MAX,
        dropped: u32::MAX,
    };
    count_uart_errors(
        &mut c,
        UART_ERROR_OVERRUN | UART_ERROR_FRAMING | UART_ERROR_NOISE,
        true,
    );
    assert!(equal(&c, 0, 0, 0, 0), "{c:?}");
}

#[test]
fn the_bits_of_hal_uart_error_pe_ne_fe_and_ore() {
    assert_eq!(UART_ERROR_PARITY, 0x01);
    assert_eq!(UART_ERROR_NOISE, 0x02);
    assert_eq!(UART_ERROR_FRAMING, 0x04);
    assert_eq!(UART_ERROR_OVERRUN, 0x08);
}
