// New cases (the C++ sets the pin modes through the HAL's GPIO_Init, one pin at a time): the
// 2-bit fields of the pin sets the boot stage configures (firmware/src/boot_hw.rs).

use super::*;

#[test]
fn no_pin_no_field() {
    assert_eq!(fields2(0, 0b01), (0, 0));
}

#[test]
fn the_first_and_the_last_pin_of_a_port() {
    assert_eq!(fields2(1 << 0, 0b10), (0b11, 0b10));
    assert_eq!(fields2(1 << 15, 0b01), (0xC000_0000, 0x4000_0000));
}

#[test]
fn every_pin_of_a_port() {
    assert_eq!(fields2(0xFFFF, 0b01), (0xFFFF_FFFF, 0x5555_5555));
    assert_eq!(fields2(0xFFFF, 0b11), (0xFFFF_FFFF, 0xFFFF_FFFF));
}

#[test]
fn bits_above_pin_15_are_no_pins() {
    assert_eq!(fields2(0xFFFF_0000, 0b01), (0, 0));
    assert_eq!(fields2(0x0001_0001, 0b01), (0b11, 0b01));
}

#[test]
fn the_pins_of_the_boot_stage() {
    // LED PC13, output
    assert_eq!(fields2(1 << 13, 0b01), (0x0C00_0000, 0x0400_0000));
    // valve PSU PB9, output
    assert_eq!(fields2(1 << 9, 0b01), (0x000C_0000, 0x0004_0000));
    // ENA0..2, ENA4: PA5, PA6, PA7, PA15, outputs
    let ena_a = (1 << 5) | (1 << 6) | (1 << 7) | (1 << 15);
    assert_eq!(fields2(ena_a, 0b01), (0xC000_FC00, 0x4000_5400));
    // ENA3, ENA5: PB0, PB3, outputs
    assert_eq!(
        fields2((1 << 0) | (1 << 3), 0b01),
        (0x0000_00C3, 0x0000_0041)
    );
    // USART1 PA9, PA10: alternate function, high speed, pull-up
    let usart1 = (1 << 9) | (1 << 10);
    assert_eq!(fields2(usart1, 0b10), (0x003C_0000, 0x0028_0000));
    assert_eq!(fields2(usart1, 0b11), (0x003C_0000, 0x003C_0000));
    assert_eq!(fields2(usart1, 0b01), (0x003C_0000, 0x0014_0000));
}

#[test]
fn the_fields_are_known_when_the_firmware_is_compiled() {
    const LED: (u32, u32) = fields2(1 << 13, 0b01);
    assert_eq!(LED, (0x0C00_0000, 0x0400_0000));
}
