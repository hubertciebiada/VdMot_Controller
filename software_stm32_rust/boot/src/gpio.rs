//! The register fields of the boot stage's pins (the valve outputs of `valve_pins_safe`, the
//! LED, USART1's PA9 and PA10): MODER, OSPEEDR and PUPDR hold two bits per pin.

/// The 2-bit fields of the pins in `pins` (bit n = pin n of the port): the mask of their fields
/// and the 2-bit `value` in each of them.
pub const fn fields2(pins: u32, value: u32) -> (u32, u32) {
    let mut mask = 0u32;
    let mut set = 0u32;
    let mut p = 0u32;
    while p < 16 {
        if pins & 1u32.wrapping_shl(p) != 0 {
            let at = p.wrapping_mul(2);
            mask |= 0b11u32.wrapping_shl(at);
            set |= value.wrapping_shl(at);
        }
        p = p.wrapping_add(1);
    }
    (mask, set)
}

#[cfg(test)]
mod tests;
