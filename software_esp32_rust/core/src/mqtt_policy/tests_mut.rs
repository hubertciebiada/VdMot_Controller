//! Port of test/native/test_mqtt_policy__mut.cpp: repeated drops, client id buffers, echo reset,
//! latch bounds.

use super::*;
use crate::test_support::assert_text;

#[test]
fn reconnect_pacer_a_second_drop_without_a_new_connect_is_not_another_failure() {
    let mut p = ReconnectPacer::new(2000, 60000);
    p.on_connected(0);
    p.on_dropped(100);
    assert_eq!(p.delay_ms(), 4000);
    p.on_dropped(200);
    assert_eq!(p.delay_ms(), 4000);
    assert!(p.due(2100));
}

#[test]
fn build_mqtt_client_id_small_buffers() {
    let mac = [0x24, 0x0a, 0xc4, 0xa1, 0xb2, 0xc3];
    let mut out = [b'X'; 4];
    assert_eq!(build_mqtt_client_id(b"VdMot", &mac, &mut out[..0]), 0);
    assert_eq!(out[0], b'X');
    assert_eq!(build_mqtt_client_id(b"VdMot", &mac, &mut out[..1]), 0);
    // C++ out[0] == '\0': the 0 result; the byte past the capacity stays
    assert_eq!(out[1], b'X');
}

#[test]
fn build_mqtt_client_id_the_host_part_is_cut_to_16_chars() {
    let mac = [0x24, 0x0a, 0xc4, 0xa1, 0xb2, 0xc3];
    let mut out = [0u8; 32];
    assert_eq!(
        build_mqtt_client_id(b"abcdefghijklmnopqrstuvwxyz", &mac, &mut out),
        23
    );
    assert_text(&out[..23], "abcdefghijklmnop-a1b2c3");
    assert_eq!(build_mqtt_client_id(b"abcdefghijklmno", &mac, &mut out), 22);
    assert_text(&out[..22], "abcdefghijklmno-a1b2c3");
}

#[test]
fn echo_filter_reset_forgets_every_published_value() {
    let mut f = EchoFilter::default();
    f.published(0, 5);
    f.published(11, 7);
    assert!(f.is_echo(0, 5));
    assert!(f.is_echo(11, 7));
    f.reset();
    assert!(!f.is_echo(0, 5));
    assert!(!f.is_echo(11, 7));
}

#[test]
fn target_latch_out_of_range_valves_are_ignored() {
    let mut l = TargetLatch::default();
    l.set(VALVE_COUNT, 40);
    assert_eq!(l.next(0), None);
    assert!(!l.pending(VALVE_COUNT));
    l.set(0, 30);
    l.clear(VALVE_COUNT);
    assert_eq!(l.next(5), Some((0, 30)));
}
