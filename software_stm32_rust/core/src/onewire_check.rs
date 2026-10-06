//! Integrity checks for data read from 1-Wire devices. Hardware-free.

/// Dallas/Maxim CRC-8 (polynomial x^8 + x^5 + x^4 + 1), as used by 1-Wire devices.
pub fn crc8(data: &[u8]) -> u8 {
    let mut crc: u8 = 0;
    for &b in data {
        let mut byte = b;
        for _bit in 0..8 {
            let mix = (crc ^ byte) & 0x01 != 0;
            crc >>= 1;
            if mix {
                crc ^= 0x8C;
            }
            byte >>= 1;
        }
    }
    crc
}

/// A scratchpad page as read from a DS2438: 8 data bytes followed by their CRC.
/// Rejects a CRC mismatch and an all-zero read, which has a valid CRC but is
/// what a bus held low returns.
pub fn is_valid_scratchpad(page: &[u8; 9]) -> bool {
    let all_zero = page.iter().all(|&b| b == 0);
    !all_zero && crc8(&page[..8]) == page[8]
}

#[cfg(test)]
mod tests;
