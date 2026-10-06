//! Port of test/native/test_legacy_layout.cpp.

use super::*;

type Image = [u8; LEGACY_IMAGE_SIZE];

fn fill_slot(s: &mut SensorSlot, family: u8, n: u8) {
    s.familycode = family;
    for (k, b) in s.romcode.iter_mut().enumerate() {
        // static_cast<uint8_t>(n * 11 + k * 37 + 1)
        *b = n
            .wrapping_mul(11)
            .wrapping_add(k as u8 * 37)
            .wrapping_add(1);
    }
    s.crc = 0xC0 ^ n;
}

/// The 8 bytes of a slot in the order of the C++ struct (address order); the C++ test reaches
/// them with memset and reinterpret_cast.
fn slot_bytes(s: &SensorSlot) -> [u8; 8] {
    let r = s.romcode;
    [s.familycode, r[0], r[1], r[2], r[3], r[4], r[5], s.crc]
}

fn slot_from_bytes(b: [u8; 8]) -> SensorSlot {
    SensorSlot {
        familycode: b[0],
        romcode: [b[1], b[2], b[3], b[4], b[5], b[6]],
        crc: b[7],
    }
}

/// memset(&l, b, sizeof(l)): every byte of every field is `b`.
fn layout_filled(b: u8) -> LegacyLayout {
    let slot = slot_from_bytes([b; 8]);
    let w = u16::from_le_bytes([b, b]);
    LegacyLayout {
        b_slave: b,
        descr: [b; 25],
        one_wire_cfg: [b; 3],
        currentbound_low_fac: b,
        currentbound_high_fac: b,
        number_of_movements: w,
        owsensors1: [slot; VALVE_COUNT as usize],
        owsensors2: [slot; VALVE_COUNT as usize],
        owsensors: [slot; EXTRA_SENSOR_SLOTS as usize],
        start_on_power: b,
        no_of_min_counts: w,
        max_calib_retries: b,
    }
}

/// A different value in every field and in every byte of a slot, so any change
/// of the byte order shows in the golden image.
fn sample_layout() -> LegacyLayout {
    // memset(&l, 0, sizeof(l)), then the fields
    let mut l = LegacyLayout {
        b_slave: 0x5A,
        one_wire_cfg: [0x11, 0x22, 0x33],
        currentbound_low_fac: 17,
        currentbound_high_fac: 23,
        number_of_movements: 0x1234,
        start_on_power: 42,
        no_of_min_counts: 0xABCD,
        max_calib_retries: 2,
        ..LegacyLayout::default()
    };
    for (i, c) in l.descr.iter_mut().enumerate() {
        *c = b'A' + i as u8;
    }
    for (s, slot) in l.owsensors1.iter_mut().enumerate() {
        fill_slot(slot, 0x28, s as u8);
    }
    for (s, slot) in l.owsensors2.iter_mut().enumerate() {
        fill_slot(slot, 0x10, 12 + s as u8);
    }
    for (s, slot) in l.owsensors.iter_mut().enumerate() {
        fill_slot(slot, 0x22, 24 + s as u8);
    }
    l
}

/// sample_layout() as the 58632d6 eeprom_write_layout() wrote it at 0x0007
/// (generated once with that writer against a fake I2C_eeprom)
const GOLDEN: Image = [
    0x5A, 0x41, 0x42, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4A, 0x4B, 0x4C, 0x4D, 0x4E, 0x4F,
    0x50, 0x51, 0x52, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x11, 0x22, 0x33, 0x11, 0x17, 0x34,
    0x12, 0x28, 0xBA, 0x95, 0x70, 0x4B, 0x26, 0x01, 0xC0, 0x28, 0xC5, 0xA0, 0x7B, 0x56, 0x31, 0x0C,
    0xC1, 0x28, 0xD0, 0xAB, 0x86, 0x61, 0x3C, 0x17, 0xC2, 0x28, 0xDB, 0xB6, 0x91, 0x6C, 0x47, 0x22,
    0xC3, 0x28, 0xE6, 0xC1, 0x9C, 0x77, 0x52, 0x2D, 0xC4, 0x28, 0xF1, 0xCC, 0xA7, 0x82, 0x5D, 0x38,
    0xC5, 0x28, 0xFC, 0xD7, 0xB2, 0x8D, 0x68, 0x43, 0xC6, 0x28, 0x07, 0xE2, 0xBD, 0x98, 0x73, 0x4E,
    0xC7, 0x28, 0x12, 0xED, 0xC8, 0xA3, 0x7E, 0x59, 0xC8, 0x28, 0x1D, 0xF8, 0xD3, 0xAE, 0x89, 0x64,
    0xC9, 0x28, 0x28, 0x03, 0xDE, 0xB9, 0x94, 0x6F, 0xCA, 0x28, 0x33, 0x0E, 0xE9, 0xC4, 0x9F, 0x7A,
    0xCB, 0x10, 0x3E, 0x19, 0xF4, 0xCF, 0xAA, 0x85, 0xCC, 0x10, 0x49, 0x24, 0xFF, 0xDA, 0xB5, 0x90,
    0xCD, 0x10, 0x54, 0x2F, 0x0A, 0xE5, 0xC0, 0x9B, 0xCE, 0x10, 0x5F, 0x3A, 0x15, 0xF0, 0xCB, 0xA6,
    0xCF, 0x10, 0x6A, 0x45, 0x20, 0xFB, 0xD6, 0xB1, 0xD0, 0x10, 0x75, 0x50, 0x2B, 0x06, 0xE1, 0xBC,
    0xD1, 0x10, 0x80, 0x5B, 0x36, 0x11, 0xEC, 0xC7, 0xD2, 0x10, 0x8B, 0x66, 0x41, 0x1C, 0xF7, 0xD2,
    0xD3, 0x10, 0x96, 0x71, 0x4C, 0x27, 0x02, 0xDD, 0xD4, 0x10, 0xA1, 0x7C, 0x57, 0x32, 0x0D, 0xE8,
    0xD5, 0x10, 0xAC, 0x87, 0x62, 0x3D, 0x18, 0xF3, 0xD6, 0x10, 0xB7, 0x92, 0x6D, 0x48, 0x23, 0xFE,
    0xD7, 0x22, 0xC2, 0x9D, 0x78, 0x53, 0x2E, 0x09, 0xD8, 0x22, 0xCD, 0xA8, 0x83, 0x5E, 0x39, 0x14,
    0xD9, 0x22, 0xD8, 0xB3, 0x8E, 0x69, 0x44, 0x1F, 0xDA, 0x22, 0xE3, 0xBE, 0x99, 0x74, 0x4F, 0x2A,
    0xDB, 0x22, 0xEE, 0xC9, 0xA4, 0x7F, 0x5A, 0x35, 0xDC, 0x22, 0xF9, 0xD4, 0xAF, 0x8A, 0x65, 0x40,
    0xDD, 0x22, 0x04, 0xDF, 0xBA, 0x95, 0x70, 0x4B, 0xDE, 0x22, 0x0F, 0xEA, 0xC5, 0xA0, 0x7B, 0x56,
    0xDF, 0x22, 0x1A, 0xF5, 0xD0, 0xAB, 0x86, 0x61, 0xE0, 0x22, 0x25, 0x00, 0xDB, 0xB6, 0x91, 0x6C,
    0xE1, 0x2A, 0xCD, 0xAB, 0x02,
];

/// a DS18B20 address with its CRC (28 FF 4C 7A 61 16 04 1A)
fn real_slot() -> SensorSlot {
    SensorSlot {
        familycode: 0x28,
        romcode: [0xFF, 0x4C, 0x7A, 0x61, 0x16, 0x04],
        crc: 0x1A,
    }
}

#[test]
fn sizes_of_the_1_x_image() {
    assert_eq!(LEGACY_IMAGE_SIZE, 309);
    assert_eq!(VALVE_COUNT, 12);
    assert_eq!(EXTRA_SENSOR_SLOTS, 10);
    assert_eq!(core::mem::size_of::<SensorSlot>(), 8);
}

#[test]
fn encode_legacy_layout_byte_order_of_the_58632d6_writer() {
    let mut out: Image = [0xEE; LEGACY_IMAGE_SIZE];
    encode_legacy_layout(&sample_layout(), &mut out);
    for (i, (o, g)) in out.iter().zip(GOLDEN.iter()).enumerate() {
        assert_eq!(o, g, "byte {i}");
    }
}

#[test]
fn decode_legacy_layout_the_golden_image_gives_back_every_field() {
    let mut l = layout_filled(0xAA);
    decode_legacy_layout(&GOLDEN, &mut l);
    // sameLayout(): every field (the derived PartialEq)
    assert_eq!(l, sample_layout());
    // spot checks of the byte order: rom code reversed, 16-bit values little endian
    assert_eq!(l.owsensors1[0].romcode[5], 0xBA);
    assert_eq!(l.owsensors1[0].romcode[0], 0x01);
    assert_eq!(l.number_of_movements, 0x1234);
    assert_eq!(l.no_of_min_counts, 0xABCD);
    assert_eq!(l.max_calib_retries, 2);
}

#[test]
fn decode_legacy_layout_encode_legacy_layout_x_eq_x() {
    let mut input = sample_layout();
    input.b_slave = 0;
    input.descr[24] = 0;
    input.number_of_movements = 0xFFFE;
    input.no_of_min_counts = 0x0100;
    input.start_on_power = 100;
    input.max_calib_retries = 0;
    fill_slot(&mut input.owsensors2[11], 0x28, 200);
    fill_slot(&mut input.owsensors[9], 0x26, 201);
    let mut raw: Image = [0; LEGACY_IMAGE_SIZE];
    encode_legacy_layout(&input, &mut raw);
    let mut out = layout_filled(0x55);
    decode_legacy_layout(&raw, &mut out);
    assert_eq!(input, out);

    // and back: the bytes survive a decode/encode round trip, an erased chip included
    let mut again: Image = [0; LEGACY_IMAGE_SIZE];
    encode_legacy_layout(&out, &mut again);
    assert_eq!(raw, again);
    let erased: Image = [0xFF; LEGACY_IMAGE_SIZE];
    decode_legacy_layout(&erased, &mut out);
    assert_eq!(out.number_of_movements, 0xFFFF);
    assert_eq!(out.no_of_min_counts, 0xFFFF);
    assert_eq!(out.max_calib_retries, 0xFF);
    encode_legacy_layout(&out, &mut again);
    assert_eq!(erased, again);
}

#[test]
fn crc16_ccitt_crc_16_ccitt_false() {
    let check = *b"123456789";
    assert_eq!(crc16_ccitt(&check), 0x29B1);
    assert_eq!(crc16_ccitt(&check[..0]), 0xFFFF);
    let zero: [u8; 1] = [0x00];
    assert_eq!(crc16_ccitt(&zero), 0xE1F0);
    let erased: Image = [0xFF; LEGACY_IMAGE_SIZE];
    assert_eq!(crc16_ccitt(&erased), 0xA238);
}

#[test]
fn crc16_ccitt_every_single_bit_change_of_the_layout_changes_the_crc() {
    let good = crc16_ccitt(&GOLDEN);
    for i in 0..LEGACY_IMAGE_SIZE {
        for bit in 0..8 {
            let mut raw = GOLDEN;
            raw[i] ^= 1 << bit;
            assert_ne!(crc16_ccitt(&raw), good, "byte {i} bit {bit}");
        }
    }
}

#[test]
fn sensor_slot_valid_never_assigned_cleared_and_real_addresses() {
    let mut s = slot_from_bytes([0xFF; 8]);
    assert!(sensor_slot_valid(&s)); // erased: never assigned
    s = slot_from_bytes([0x00; 8]);
    assert!(sensor_slot_valid(&s)); // cleared assignment: CRC 0 of zeros
    assert!(sensor_slot_valid(&real_slot()));
}

#[test]
fn sensor_slot_valid_one_flipped_bit_anywhere_is_invalid() {
    // C++ flips the bytes of the struct through reinterpret_cast: here the same bytes through
    // slot_bytes()/slot_from_bytes()
    let good = real_slot();
    for i in 0..core::mem::size_of::<SensorSlot>() {
        for bit in 0..8 {
            let mut bytes = slot_bytes(&good);
            bytes[i] ^= 1 << bit;
            let s = slot_from_bytes(bytes);
            assert!(!sensor_slot_valid(&s), "byte {i} bit {bit}");
        }
    }
    // and in a slot that is all 0xFF but one byte
    for i in 0..core::mem::size_of::<SensorSlot>() {
        let mut bytes = [0xFFu8; 8];
        bytes[i] = 0xFE;
        assert!(!sensor_slot_valid(&slot_from_bytes(bytes)), "byte {i}");
    }
}
