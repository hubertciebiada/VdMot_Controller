// No C++ counterpart: the warm state in the no-init RAM as bytes. The C++ keeps `vdm::WarmState`
// there as a struct (178 bytes + 2 bytes of trailing padding, design §3.2); the Rust image must be
// byte-identical so that a warm reset across a C++ <-> Rust flash keeps every valve.
use vdm_stm_core::legacy_layout::crc16_ccitt;
use vdm_stm_core::valve_codes::{ST_BLOCKED, ST_IDLE};
use vdm_stm_core::warm_state::{warm_state_seal, warm_state_valid, WarmState, WARM_POS_VALID};

use crate::hal::NoinitStore;
use crate::test_support::fake_board::FakeNoinit;

use super::stub_env::AppBench;
use super::{warm_from_bytes, warm_to_bytes, App};

fn sample() -> WarmState {
    let mut ws = WarmState::default();
    for (x, w) in ws.valves.iter_mut().enumerate() {
        let x = x as u8;
        w.actual = 10 + x;
        w.target = 20 + x;
        w.status = ST_IDLE;
        w.flags = WARM_POS_VALID;
        w.retry_attempts = x;
        w.retry_scheduled = x % 2;
        w.retry_remaining_s = 0x0102_0304 + u32::from(x);
    }
    ws.valves[3].status = ST_BLOCKED;
    ws.lease_since_renewal_s = 0x1122_3344;
    ws.lease_since_client_s = 0x5566_7788;
    ws.lease_client = 1;
    ws.lease_timeout_min = 0x0A0B;
    for (x, pct) in ws.failsafe_pct.iter_mut().enumerate() {
        *pct = 40 + x as u8;
    }
    warm_state_seal(&mut ws);
    ws
}

#[test]
fn the_bytes_follow_the_cpp_layout_and_carry_the_crc_of_the_core() {
    let ws = sample();
    let b = warm_to_bytes(&ws);
    // "VDWS", version 1, 12 valves, reserved 0
    assert_eq!(b[0..8], [0x53, 0x57, 0x44, 0x56, 1, 12, 0, 0]);
    // valve 0 at 8, valve 11 at 140: actual, target, status, flags, attempts, scheduled, pad, rest
    assert_eq!(
        b[8..20],
        [10, 20, ST_IDLE, WARM_POS_VALID, 0, 0, 0, 0, 4, 3, 2, 1]
    );
    assert_eq!(
        b[140..152],
        [21, 31, ST_IDLE, WARM_POS_VALID, 11, 1, 0, 0, 15, 3, 2, 1]
    );
    // the status of valve 3: 8 + 3 x 12 + 2
    assert_eq!(b[46], ST_BLOCKED);
    assert_eq!(b[152..156], [0x44, 0x33, 0x22, 0x11]);
    assert_eq!(b[156..160], [0x88, 0x77, 0x66, 0x55]);
    assert_eq!(b[160..164], [1, 0, 0x0B, 0x0A]);
    assert_eq!(b[164], 40);
    assert_eq!(b[175], 51);
    assert_eq!(b[176..178], ws.crc.to_le_bytes());
    // the CRC of the core covers exactly these 176 bytes
    assert_eq!(crc16_ccitt(&b[..176]), ws.crc);
}

#[test]
fn the_bytes_read_back_to_the_same_state() {
    let ws = sample();
    let back = warm_from_bytes(&warm_to_bytes(&ws));
    assert_eq!(back, ws);
    assert!(warm_state_valid(&back));
}

#[test]
fn every_byte_of_the_record_is_read_back_reserved_and_pad_included() {
    // the CRC covers the reserved and pad bytes too (the C++ reads the whole struct)
    let mut ws = sample();
    ws.reserved = 0x0102;
    ws.pad = 7;
    ws.crc = crc16_ccitt(&warm_to_bytes(&ws)[..176]);
    let back = warm_from_bytes(&warm_to_bytes(&ws));
    assert_eq!(back, ws);
    assert!(warm_state_valid(&back));
}

#[test]
fn app_warm_save_writes_the_padding_as_0_and_keeps_the_cells_behind_the_warm_state() {
    let mut b = AppBench::new();
    b.app_setup();
    // the reset guard and the reset counter of the sysstat module follow at 180
    b.env.noinit.bytes[178] = 0x77;
    b.env.noinit.bytes[179] = 0x77;
    b.env.noinit.bytes[180..212].fill(0x5A);
    b.app_warm_save();
    let image = b.env.noinit.read();
    assert!(warm_state_valid(&warm_from_bytes(&image[..178])));
    assert_eq!(image[178..180], [0, 0]);
    assert!(image[180..212].iter().all(|&x| x == 0x5A));
}

#[test]
fn app_warm_moving_changes_only_the_flag_of_its_valve_and_the_crc() {
    let mut store = FakeNoinit::default();
    let ws = sample();
    store.bytes[..178].copy_from_slice(&warm_to_bytes(&ws));
    store.bytes[178] = 0x77;
    let before = store.bytes;
    App::app_warm_moving(&mut store, 2);
    let after = store.bytes;
    let moved = warm_from_bytes(&after[..178]);
    assert!(warm_state_valid(&moved));
    assert_eq!(moved.valves[2].flags, 0);
    let mut want = ws;
    want.valves[2].flags = 0;
    warm_state_seal(&mut want);
    assert_eq!(moved, want);
    // the padding and the cells behind it are not touched
    assert_eq!(after[178..], before[178..]);
}
