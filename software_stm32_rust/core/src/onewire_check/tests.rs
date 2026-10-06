//! Port of test/native/test_onewire_check.cpp.

use super::*;
use crate::test_support::Rng;

/// Bitwise reference from Maxim application note 27.
fn reference_crc8(data: &[u8]) -> u8 {
    let mut crc: u8 = 0;
    for &d in data {
        for bit in 0..8 {
            let input = (d >> bit) & 1;
            let fb = (crc ^ input) & 1;
            crc >>= 1;
            if fb != 0 {
                crc ^= 0x8C;
            }
        }
    }
    crc
}

#[test]
fn crc8_maxim_an27_rom_example() {
    let rom: [u8; 7] = [0x02, 0x1C, 0xB8, 0x01, 0x00, 0x00, 0x00];
    assert_eq!(crc8(&rom), 0xA2);
}

#[test]
fn crc8_empty_input_and_single_bytes() {
    // C++ crc8(nullptr, 0): a Rust slice is never null, the empty slice is the nearest case
    assert_eq!(crc8(&[]), 0);
    assert_eq!(crc8(&[0x01]), 0x5E);
    assert_eq!(crc8(&[0xFF]), 0x35);
}

#[test]
fn crc8_matches_the_reference_on_random_data() {
    // rng() % 17 and static_cast<uint8_t>(rng()) on the raw output: the same data as the C++
    let mut rng = Rng::new(20_260_924);
    let mut buf = [0u8; 16];
    for round in 0..2000 {
        let len = (rng.next_u32() % (buf.len() as u32 + 1)) as usize;
        for b in &mut buf[..len] {
            *b = rng.next_u32() as u8;
        }
        assert_eq!(
            crc8(&buf[..len]),
            reference_crc8(&buf[..len]),
            "round {round}"
        );
    }
}

#[test]
fn is_valid_scratchpad_accepts_a_page_with_a_matching_crc() {
    let mut page: [u8; 9] = [0x0F, 0x80, 0x19, 0xF4, 0x01, 0x00, 0x00, 0x00, 0x00];
    page[8] = crc8(&page[..8]);
    assert!(is_valid_scratchpad(&page));
}

#[test]
fn is_valid_scratchpad_rejects_a_crc_mismatch_in_data_or_crc_byte() {
    let mut page: [u8; 9] = [0x0F, 0x80, 0x19, 0xF4, 0x01, 0x00, 0x00, 0x00, 0x00];
    page[8] = crc8(&page[..8]);
    for i in 0..9 {
        let mut bad = page;
        bad[i] ^= 0x01;
        assert!(!is_valid_scratchpad(&bad), "byte {i}");
    }
}

#[test]
fn is_valid_scratchpad_rejects_the_all_zero_read_of_a_bus_held_low() {
    let zeros = [0u8; 9];
    assert_eq!(crc8(&zeros[..8]), 0); // the CRC alone would accept it
    assert!(!is_valid_scratchpad(&zeros));
}

#[test]
fn is_valid_scratchpad_a_zero_crc_byte_is_fine_when_the_data_is_not_all_zero() {
    // find data whose CRC is 0 but which is not all zero
    let mut page = [0u8; 9];
    let mut found = false;
    for v in 1..65536u32 {
        page[0] = v as u8;
        page[1] = (v >> 8) as u8;
        found = crc8(&page[..8]) == 0;
        if found {
            break;
        }
    }
    assert!(found);
    assert!(is_valid_scratchpad(&page));
}

#[test]
fn is_valid_scratchpad_an_all_0xff_read_no_device_fails_the_crc() {
    let ones = [0xFFu8; 9];
    assert!(!is_valid_scratchpad(&ones));
}
