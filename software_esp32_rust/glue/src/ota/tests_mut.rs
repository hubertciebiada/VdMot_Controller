//! Port of `test/native/glue/test_ota__mut.cpp`: the timing and the partial reads of the
//! loopback self-check, the validator seconds in /api/health, the restart delay after an upload,
//! the restart reasons that confirm an image on trial, the first upload error of a boot.
#![allow(clippy::large_stack_frames)]

use super::support::*;
use super::update::error_text;
use super::*;
use crate::testkit::board::APP_B;
use crate::testkit::ota::SLOT_ADDR;
use crate::testkit::{run, Ended, Reset};

/// C++ `pendingImage(first, ip)`: an image on trial (no STM link required), the network up at
/// `ip`; the loopback server answers `first` at once and the `later` parts at their times.
fn pending(first: &[u8], later: Vec<(u64, Vec<u8>)>, at: u32) -> Rig {
    let rig = Rig::pending(first, false);
    rig.host.state().net_ip = at;
    rig.loopback(first, later);
    rig
}

/// The self-check of `service` at fake time 0; its result.
fn self_check_at_zero(rig: &Rig) -> bool {
    let mut svc = rig.begun();
    svc.service(0, true, true, true);
    rig.shared.health().http_ok
}

/// A restart through the STM save gate: requested, saved, esp_restart.
fn restart_through_gate(rig: &Rig, svc: &mut OtaService<'_, TestPlatform, FakeOtaHost>) {
    let now = rig.dev.clock.now_ms();
    svc.service_restart(now, true, true);
    rig.host.state().save_state = StmSaveState::Saved;
    assert_eq!(
        run(|| svc.service_restart(now, true, true)),
        Ended::Reset(Reset::Software)
    );
}

use crate::testkit::board::TestPlatform;

fn ok_200() -> Vec<u8> {
    b"HTTP/1.1 200 OK\r\n".to_vec()
}

// ---------------------------------------------------------------- self-check timing

#[test]
fn self_check_a_silent_server_is_polled_every_10_ms_for_3_s() {
    let rig = pending(b"", vec![(100_000, ok_200())], DEVICE_IP);
    assert!(!self_check_at_zero(&rig));
    assert_eq!(rig.dev.clock.sleeps(), vec![10; 300]);
    assert_eq!(rig.dev.clock.ms(), 3000);
    assert_eq!(rig.requests().len(), 1);
}

#[test]
fn self_check_an_answer_at_3000_ms_is_too_late() {
    let rig = pending(b"", vec![(3000, ok_200())], DEVICE_IP);
    assert!(!self_check_at_zero(&rig));
    assert_eq!(rig.dev.clock.ms(), 3000);
}

#[test]
fn self_check_an_answer_at_2999_ms_still_counts() {
    let rig = pending(b"", vec![(2999, ok_200())], DEVICE_IP);
    // one extra 9 ms on the first wait: the loop looks at 0, 19, 29, ..., 2999 ms
    let clock = rig.dev.clock.clone();
    let mut shifted = false;
    rig.dev.clock.on_sleep(move |_| {
        if !shifted {
            clock.advance_ms(9);
        }
        shifted = true;
    });
    assert!(self_check_at_zero(&rig));
    assert_eq!(rig.dev.clock.ms(), 2999);
}

// ---------------------------------------------------------------- self-check reads

#[test]
fn self_check_a_status_line_in_two_parts_the_last_one_a_single_byte() {
    let rig = pending(b"HTTP/1.1 20", vec![(10, b"0".to_vec())], DEVICE_IP);
    assert!(self_check_at_zero(&rig));
    assert_eq!(rig.dev.clock.sleeps(), vec![10]);
}

#[test]
fn self_check_the_second_part_is_read_only_up_to_the_12_status_bytes() {
    let later = b"0 OK\r\nContent-Length: 2\r\n\r\n{}".to_vec();
    let rig = pending(b"HTTP/1.1 20", vec![(10, later)], DEVICE_IP);
    assert!(self_check_at_zero(&rig));
    // the rest stays unread on the connection
    let wire = rig.dev.tcp.wire(0);
    assert!(crate::testkit::lock(&wire).pending() > 0);
}

#[test]
fn self_check_only_the_first_12_bytes_of_the_status_line_are_looked_at() {
    let rig = pending(b"HTTP/1.1 200OK\r\n", Vec::new(), DEVICE_IP);
    assert!(self_check_at_zero(&rig));
}

#[test]
fn self_check_the_host_header_carries_a_15_character_address_whole() {
    let rig = pending(&ok_200(), Vec::new(), ip(192, 168, 100, 200));
    assert!(self_check_at_zero(&rig));
    assert_eq!(
        rig.requests(),
        vec!["GET /api/health HTTP/1.1\r\nHost: 192.168.100.200\r\nConnection: close\r\n\r\n"]
    );
}

// ---------------------------------------------------------------- health seconds

#[test]
fn health_healthy_and_remaining_time_are_whole_seconds_rounded_down() {
    let rig = pending(&ok_200(), Vec::new(), DEVICE_IP);
    let mut svc = rig.begun();
    svc.service(0, true, true, true);
    svc.service(1998, true, true, true);
    assert_eq!(rig.shared.health().healthy_for_s, 1);
    svc.service(2500, true, true, true);
    assert_eq!(rig.shared.health().remaining_s, 897); // 897.5 s left
}

// ---------------------------------------------------------------- restart

#[test]
fn upload_the_restart_after_a_committed_image_is_due_at_exactly_1000_ms() {
    let rig = Rig::confirmed();
    let mut svc = rig.begun();
    let mut up = rig.upload();
    assert!(up.upload_begin(1, b""));
    assert!(up.upload_write(&[0xE9]));
    assert!(up.upload_end(true));
    svc.service_restart(999, true, true);
    assert_eq!(rig.host.state().save_requests, 0);
    svc.service_restart(1000, true, true);
    assert_eq!(rig.host.state().save_requests, 1);
}

#[test]
fn service_restart_an_upload_restart_without_an_upload_of_this_boot_stores_0() {
    let rig = Rig::confirmed();
    let mut svc = rig.begun();
    rig.request_restart(1, 0, 0);
    restart_through_gate(&rig, &mut svc);
    assert_eq!(rig.host.state().ota_stm_sets, vec![false]);
}

#[test]
fn service_restart_a_factory_reset_confirms_an_image_on_trial_first() {
    let rig = pending(b"", Vec::new(), DEVICE_IP);
    let mut svc = rig.begun();
    rig.request_restart(3, 0, 0);
    restart_through_gate(&rig, &mut svc);
    assert_eq!(rig.dev.ota.knobs().mark_valids, 1);
    assert_eq!(rig.confirmed_app(), Some(APP_B.0));
}

#[test]
fn service_restart_a_requested_rollback_restart_is_a_warning_and_confirms_nothing() {
    let rig = pending(b"", Vec::new(), DEVICE_IP);
    let mut svc = rig.begun();
    rig.request_restart(4, 0, 0);
    assert_eq!(
        rig.host.first(EventCode::RebootRequested).severity,
        Severity::Warning
    );
    restart_through_gate(&rig, &mut svc);
    assert_eq!(rig.dev.ota.knobs().mark_valids, 0);
    assert_eq!(rig.dev.ota.knobs().set_boots, vec![SLOT_ADDR[0]]);
}

// ---------------------------------------------------------------- upload errors

#[test]
fn upload_end_without_any_upload_in_this_boot_the_error_is_no_upload() {
    let rig = Rig::confirmed();
    let mut up = rig.upload();
    assert!(!up.upload_end(true));
    assert_eq!(up.upload_error(), "no upload");
}

#[test]
fn upload_error_every_update_text_is_kept_whole() {
    // C++ copied the library's String into 48 bytes (47 characters kept); the Rust errors are
    // static texts, the longest of them 31 characters
    let longest = (0..=13).map(|c| error_text(c).len()).max();
    assert_eq!(longest, Some(31));
    let rig = Rig::confirmed();
    rig.dev.ota.knobs().finish_err = Some(crate::port::EspErr::OTA_VALIDATE_FAILED);
    let mut up = rig.upload();
    assert!(up.upload_begin(1, b""));
    assert!(up.upload_write(&[0xE9]));
    assert!(!up.upload_end(true));
    assert_eq!(up.upload_error(), "Could Not Activate The Firmware");
}

// ---------------------------------------------------------------- Rust only

/// A server whose connection first reports two reads that copied nothing.
struct CopiesNothing;

impl crate::testkit::net::TcpPeer for CopiesNothing {
    fn on_connect(&mut self, wire: &mut crate::testkit::net::Wire, _now_ms: u64) {
        wire.empty_reads = 2;
    }
    fn on_data(&mut self, wire: &mut crate::testkit::net::Wire, _data: &[u8], now_ms: u64) {
        wire.send_at(now_ms, b"HTTP/1.1 200 OK\r\n");
        wire.close_at(now_ms);
    }
}

#[test]
fn self_check_an_adapter_that_copies_nothing_waits_like_an_empty_read() {
    let rig = pending(b"", Vec::new(), DEVICE_IP);
    rig.dev
        .tcp
        .listen("127.0.0.1", 80, || Box::new(CopiesNothing));
    let mut svc = rig.begun();
    // each read that copied nothing waits 10 ms instead of spinning
    svc.service(0, true, true, true);
    assert!(rig.shared.health().http_ok);
    assert_eq!(rig.dev.clock.sleeps(), vec![10, 10]);
}
