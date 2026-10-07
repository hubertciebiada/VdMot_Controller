//! Port of test/native/test_net_trial.cpp: record codec, boot rule, trial window, field
//! helpers. C++ cases with a null pointer and the damaged struct (an SSID or password array
//! without its NUL) have no Rust form; they are named where they stood.

use super::*;
use crate::common::copy_string;
use crate::test_support::assert_text;
use std::vec;
use std::vec::Vec;

fn static_net() -> NetConfig {
    NetConfig {
        iface: NetInterface::Ethernet,
        dhcp: false,
        ip: 0x3201_A8C0,      // 192.168.1.50
        mask: 0x00FF_FFFF,    // 255.255.255.0
        gateway: 0x0101_A8C0, // 192.168.1.1
        dns: 0x0201_A8C0,
        reconnect_timeout_min: 7,
        ..NetConfig::default()
    }
}

fn wifi_net() -> NetConfig {
    let mut n = NetConfig {
        iface: NetInterface::Wifi,
        dhcp: true,
        ..NetConfig::default()
    };
    assert!(copy_string(&mut n.ssid, &[b'S'; 32]));
    assert!(copy_string(&mut n.wifi_password, &[b'p'; 64]));
    n
}

fn add_u32(v: &mut Vec<u8>, x: u32) {
    v.extend_from_slice(&x.to_le_bytes());
}

fn encode(r: &NetTrialRecord) -> Vec<u8> {
    let mut v = vec![0u8; NET_TRIAL_BLOB_MAX];
    let n = encode_net_trial(r, &mut v);
    v.truncate(n);
    v
}

/// A blob with its CRC fixed after a change of the bytes before it.
fn recrc(mut v: Vec<u8>) -> Vec<u8> {
    let n = v.len();
    let c = crc32(&v[..n - 4], 0);
    v[n - 4..].copy_from_slice(&c.to_le_bytes());
    v
}

fn decodes(v: &[u8]) -> bool {
    let mut r = NetTrialRecord::default();
    decode_net_trial(v, &mut r)
}

fn same_trial_fields(a: &NetConfig, b: &NetConfig) -> bool {
    a.iface == b.iface
        && a.dhcp == b.dhcp
        && a.ip == b.ip
        && a.mask == b.mask
        && a.gateway == b.gateway
        && a.dns == b.dns
        && a.ssid == b.ssid
        && a.wifi_password == b.wifi_password
}

#[test]
fn encode_the_exact_layout_of_a_static_record() {
    let mut r = NetTrialRecord {
        state: NetTrialState::Running,
        previous: static_net(),
        trial_crc: 0x1122_3344,
    };
    copy_string(&mut r.previous.ssid, b"ab");
    copy_string(&mut r.previous.wifi_password, b"xyz");
    let mut want = vec![b'V', b'D', b'N', b'T', 1, 2, 1, 0];
    add_u32(&mut want, 0x3201_A8C0);
    add_u32(&mut want, 0x00FF_FFFF);
    add_u32(&mut want, 0x0101_A8C0);
    add_u32(&mut want, 0x0201_A8C0);
    want.extend_from_slice(b"\x02ab\x03xyz");
    add_u32(&mut want, 0x1122_3344);
    let crc = crc32(&want, 0);
    add_u32(&mut want, crc);
    assert_eq!(encode(&r), want);
    // The fields CRC covers exactly the bytes from iface to the password.
    assert_eq!(
        net_trial_fields_crc(&r.previous),
        crc32(&want[6..want.len() - 8], 0)
    );
}

#[test]
fn encode_decode_round_trips() {
    let input = NetTrialRecord {
        previous: static_net(),
        trial_crc: 0xDEAD_BEEF,
        ..NetTrialRecord::default()
    };
    let mut b = encode(&input);
    assert_eq!(b.len(), 34);
    let mut out = NetTrialRecord::default();
    out.previous.reconnect_timeout_min = 42;
    assert!(decode_net_trial(&b, &mut out));
    assert_eq!(out.state, NetTrialState::Armed);
    assert!(same_trial_fields(&out.previous, &input.previous));
    assert_eq!(out.previous.reconnect_timeout_min, 42); // not a trial field
    assert_eq!(out.trial_crc, 0xDEAD_BEEF);

    let mut dhcp = NetTrialRecord {
        state: NetTrialState::Running,
        ..NetTrialRecord::default()
    };
    dhcp.previous.iface = NetInterface::Auto;
    b = encode(&dhcp);
    let mut out2 = NetTrialRecord {
        previous: static_net(),
        ..NetTrialRecord::default()
    };
    copy_string(&mut out2.previous.ssid, b"old");
    assert!(decode_net_trial(&b, &mut out2));
    assert_eq!(out2.state, NetTrialState::Running);
    assert!(same_trial_fields(&out2.previous, &dhcp.previous));

    let wifi = NetTrialRecord {
        previous: wifi_net(),
        ..NetTrialRecord::default()
    };
    b = encode(&wifi);
    assert_eq!(b.len(), 130);
    let mut out3 = NetTrialRecord::default();
    assert!(decode_net_trial(&b, &mut out3));
    assert!(same_trial_fields(&out3.previous, &wifi.previous));
    assert_eq!(out3.previous.ssid.len(), 32);
    assert_eq!(out3.previous.wifi_password.len(), 64);
}

#[test]
fn encode_capacity() {
    let r = NetTrialRecord {
        previous: wifi_net(),
        ..NetTrialRecord::default()
    };
    let mut buf = [0u8; NET_TRIAL_BLOB_MAX];
    assert_eq!(encode_net_trial(&r, &mut buf[..129]), 0);
    assert_eq!(encode_net_trial(&r, &mut buf[..130]), 130);
    // C++ encodeNetTrial(r, nullptr, 130): no Rust form.
    assert_eq!(NET_TRIAL_BLOB_MAX, 136);
    // The longest record and the NUL TextBuf keeps fit the encoder's 136-byte buffer.
    assert_eq!(FIXED_BYTES + SSID_MAX + SECRET_MAX, 130);
}

#[test]
fn decode_rejects() {
    let mut r = NetTrialRecord {
        previous: static_net(),
        ..NetTrialRecord::default()
    };
    copy_string(&mut r.previous.ssid, b"net");
    copy_string(&mut r.previous.wifi_password, b"password");
    let good = encode(&r);
    assert!(decodes(&good));
    let mut out = NetTrialRecord {
        trial_crc: 77,
        ..NetTrialRecord::default()
    };
    // C++ decodeNetTrial(nullptr, ..): no Rust form.
    for n in 0..good.len() {
        assert!(!decode_net_trial(&good[..n], &mut out), "{n}");
    }
    assert_eq!(out.trial_crc, 77); // unchanged
    let mut v = good.clone();
    v[0] = b'X';
    assert!(!decodes(&recrc(v)));
    v = good.clone();
    v[3] = b'X';
    assert!(!decodes(&recrc(v)));
    v = good.clone();
    v[4] = 2; // version
    assert!(!decodes(&recrc(v)));
    v = good.clone();
    v[4] = 0;
    assert!(!decodes(&recrc(v)));
    for st in [0, 3] {
        v = good.clone();
        v[5] = st;
        assert!(!decodes(&recrc(v)));
    }
    v = good.clone();
    v[6] = 3; // iface
    assert!(!decodes(&recrc(v.clone())));
    v[6] = 2;
    assert!(decodes(&recrc(v)));
    v = good.clone();
    v[7] = 2; // dhcp
    assert!(!decodes(&recrc(v.clone())));
    v[7] = 1;
    assert!(decodes(&recrc(v)));
    v = good.clone();
    v.push(0); // trailing byte
    assert!(!decodes(&recrc(v)));
    v = good.clone();
    v.pop();
    assert!(!decodes(&v));
    // Every single-bit flip of a valid blob.
    for i in 0..good.len() {
        for bit in 0..8 {
            v = good.clone();
            v[i] ^= 1 << bit;
            assert!(!decodes(&v), "{i}:{bit}");
        }
    }
}

#[test]
fn decode_ssid_and_password_length_limits() {
    let r = NetTrialRecord {
        previous: wifi_net(),
        ..NetTrialRecord::default()
    };
    let good = encode(&r);
    // ssid length 33: one more byte in the blob (still consistent), rejected.
    let mut v = good.clone();
    v[24] = 33;
    v.insert(25, b'S');
    assert!(!decodes(&recrc(v)));
    // password length 65.
    v = good.clone();
    v[25 + 32] = 65;
    v.insert(26 + 32, b'p');
    assert!(!decodes(&recrc(v)));
    // A length byte pointing past the end.
    v = good.clone();
    v[24] = 32;
    v.resize(34 + 10, 0);
    v[24] = 30;
    assert!(!decodes(&recrc(v)));
    // A shorter ssid with the password length adjusted: consistent and accepted.
    let mut s = NetTrialRecord {
        previous: static_net(),
        ..NetTrialRecord::default()
    };
    copy_string(&mut s.previous.ssid, b"a");
    let one = encode(&s);
    assert_eq!(one.len(), 35);
    assert!(decodes(&one));
}

#[test]
fn decode_a_password_length_at_or_past_the_blob_end_is_not_read() {
    let r = NetTrialRecord {
        previous: static_net(),
        ..NetTrialRecord::default()
    };
    let good = encode(&r); // 34 bytes, both lengths 0
    assert_eq!(good.len(), 34);
    // An ssid length that puts the password length at the blob end, or past it: rejected
    // without a read beyond the blob.
    for ssid in [9, 10, 32] {
        let mut v = good.clone();
        v[24] = ssid;
        assert!(!decodes(&recrc(v)), "{ssid}");
    }
    // One byte short of the end: the password length is read, the total does not add up.
    let mut v = good.clone();
    v[24] = 8;
    assert!(!decodes(&recrc(v)));
}

#[test]
fn encode_full_length_ssid_and_password() {
    // C++ "an ssid or password without its terminator ends at the field size" damages the
    // arrays (no NUL); a Rust text member holds at most its capacity, so the full-length fields
    // are the case: 32 + 64 bytes of text, 130 bytes of record.
    let r = NetTrialRecord {
        previous: wifi_net(),
        ..NetTrialRecord::default()
    };
    let b = encode(&r);
    assert_eq!(b.len(), 130);
    assert_eq!(&b[25..57], &[b'S'; 32]);
    assert_eq!(b[57], 64);
    assert_eq!(&b[58..122], &[b'p'; 64]);
}

#[test]
fn net_trial_fields_crc_changes_with_every_trial_field_not_with_reconnect_timeout_min() {
    let base = static_net();
    let c = net_trial_fields_crc(&base);
    let mut n = base.clone();
    n.reconnect_timeout_min = 0;
    assert_eq!(net_trial_fields_crc(&n), c);
    n = base.clone();
    n.iface = NetInterface::Auto;
    assert_ne!(net_trial_fields_crc(&n), c);
    n = base.clone();
    n.dhcp = true;
    assert_ne!(net_trial_fields_crc(&n), c);
    n = base.clone();
    n.ip ^= 1;
    assert_ne!(net_trial_fields_crc(&n), c);
    n = base.clone();
    n.mask ^= 0x8000_0000;
    assert_ne!(net_trial_fields_crc(&n), c);
    n = base.clone();
    n.gateway ^= 0x0001_0000;
    assert_ne!(net_trial_fields_crc(&n), c);
    n = base.clone();
    n.dns ^= 0x0000_0100;
    assert_ne!(net_trial_fields_crc(&n), c);
    n = base.clone();
    copy_string(&mut n.ssid, b"x");
    assert_ne!(net_trial_fields_crc(&n), c);
    n = base.clone();
    copy_string(&mut n.wifi_password, b"x");
    let pw = net_trial_fields_crc(&n);
    assert_ne!(pw, c);
    // "x" as ssid and as password are different settings.
    let mut s = base.clone();
    copy_string(&mut s.ssid, b"x");
    assert_ne!(net_trial_fields_crc(&s), pw);
}

#[test]
fn net_trial_at_boot_rule() {
    let cur = static_net();
    assert_eq!(net_trial_at_boot(None, &cur), NetTrialBoot::None);
    let mut r = NetTrialRecord::default();
    r.previous.dhcp = true;
    r.trial_crc = net_trial_fields_crc(&cur).wrapping_add(1);
    assert_eq!(net_trial_at_boot(Some(&r), &cur), NetTrialBoot::Stale);
    r.state = NetTrialState::Running;
    assert_eq!(net_trial_at_boot(Some(&r), &cur), NetTrialBoot::Stale);
    r.trial_crc = net_trial_fields_crc(&cur);
    assert_eq!(net_trial_at_boot(Some(&r), &cur), NetTrialBoot::RevertNow);
    r.state = NetTrialState::Armed;
    assert_eq!(net_trial_at_boot(Some(&r), &cur), NetTrialBoot::Start);
}

#[test]
fn apply_net_trial_fields_copies_the_8_fields_and_keeps_reconnect_timeout_min() {
    let mut dst = NetConfig {
        reconnect_timeout_min: 3,
        ..NetConfig::default()
    };
    let p2 = NetConfig {
        ip: 1,
        mask: 2,
        gateway: 3,
        dns: 4,
        dhcp: false,
        reconnect_timeout_min: 99,
        ..wifi_net()
    };
    apply_net_trial_fields(&mut dst, &p2);
    assert!(same_trial_fields(&dst, &p2));
    assert_eq!(dst.reconnect_timeout_min, 3);
}

#[test]
fn format_net_address_texts() {
    let mut out = [0u8; 32];
    let n = format_net_address(&static_net(), &mut out);
    assert_eq!(n, 12);
    assert_text(&out[..n], "192.168.1.50");
    let n = format_net_address(&NetConfig::default(), &mut out);
    assert_eq!(n, 4);
    assert_text(&out[..n], "dhcp");
    let n = format_net_address(&static_net(), &mut out[..5]);
    assert_eq!(n, 4);
    assert_text(&out[..n], "192.");
    assert_eq!(format_net_address(&static_net(), &mut out[..1]), 0);
    out[0] = b'q';
    assert_eq!(format_net_address(&static_net(), &mut out[..0]), 0);
    assert_eq!(out[0], b'q');
    // C++ formatNetAddress(.., nullptr, 10): no Rust form.
}

#[test]
fn no_network_within_the_window_reverts_no_network_at_120_s() {
    let mut t = NetTrial::default();
    assert!(!t.active());
    assert_eq!(t.update(false, 0), NetTrialDecision::None);
    t.start(0);
    assert!(t.active());
    assert_eq!(t.reason(), NetTrialRevert::NotConfirmed);
    assert_eq!(t.remaining_ms(0), 120_000);
    assert_eq!(t.remaining_ms(119_999), 1);
    assert_eq!(t.up_for_ms(50_000), 0);
    assert_eq!(t.update(false, 119_999), NetTrialDecision::None);
    assert_eq!(t.update(false, 120_000), NetTrialDecision::Revert);
    assert_eq!(t.reason(), NetTrialRevert::NoNetwork);
    assert!(!t.active());
    assert_eq!(t.remaining_ms(120_000), 0);
    assert_eq!(t.update(false, 130_000), NetTrialDecision::None); // once
    t.start(200_000);
    assert_eq!(t.reason(), NetTrialRevert::NotConfirmed);
}

#[test]
fn the_window_counts_from_the_first_network_a_later_ip_loss_does_not_reset_it() {
    let mut t = NetTrial::default();
    t.start(0);
    assert_eq!(t.update(false, 29_000), NetTrialDecision::None);
    assert_eq!(t.update(true, 30_000), NetTrialDecision::None);
    assert_eq!(t.remaining_ms(30_000), 120_000);
    assert_eq!(t.up_for_ms(30_000), 0);
    assert_eq!(t.up_for_ms(40_000), 10_000);
    assert_eq!(t.update(false, 100_000), NetTrialDecision::None);
    assert_eq!(t.update(true, 110_000), NetTrialDecision::None);
    assert_eq!(t.remaining_ms(110_000), 40_000);
    assert_eq!(t.update(false, 149_999), NetTrialDecision::None);
    assert_eq!(t.remaining_ms(149_999), 1);
    assert_eq!(t.update(false, 150_000), NetTrialDecision::Revert);
    assert_eq!(t.reason(), NetTrialRevert::NotConfirmed);
    assert_eq!(t.remaining_ms(150_000), 0);
}

#[test]
fn confirm_ends_the_trial_once() {
    let mut t = NetTrial::default();
    assert!(!t.confirm());
    t.start(1000);
    t.update(true, 2000);
    assert!(t.confirm());
    assert!(!t.active());
    assert!(!t.confirm());
    assert_eq!(t.update(true, 500_000), NetTrialDecision::None);
    assert_eq!(t.remaining_ms(3000), 0);
    assert_eq!(t.up_for_ms(62_000), 60_000);
}

#[test]
fn start_near_the_32_bit_wrap_and_a_custom_window() {
    let mut t = NetTrial::default();
    let s: u32 = 0xFFFF_F000;
    t.start(s);
    assert_eq!(t.remaining_ms(s.wrapping_add(1000)), 119_000);
    assert_eq!(
        t.update(false, s.wrapping_add(119_999)),
        NetTrialDecision::None
    );
    assert_eq!(
        t.update(false, s.wrapping_add(120_000)),
        NetTrialDecision::Revert
    );
    let mut w = NetTrial::new(10);
    w.start(5);
    assert_eq!(w.update(true, 6), NetTrialDecision::None);
    assert_eq!(w.update(true, 15), NetTrialDecision::None);
    assert_eq!(w.update(true, 16), NetTrialDecision::Revert);
    assert_eq!(NET_TRIAL_WINDOW_MS, 120_000);
}

#[test]
fn net_trial_revert_and_state_numbers() {
    assert_eq!(NetTrialRevert::NotConfirmed as u8, 1);
    assert_eq!(NetTrialRevert::NoNetwork as u8, 2);
    assert_eq!(NetTrialRevert::Interrupted as u8, 3);
    assert_eq!(NetTrialRevert::User as u8, 4);
    assert_eq!(NetTrialState::Armed as u8, 1);
    assert_eq!(NetTrialState::Running as u8, 2);
    // Rust: the conversions from the raw numbers.
    for v in 1..=5 {
        assert_eq!(NetTrialRevert::from_raw(v).map(|r| r as u8), Some(v));
    }
    assert_eq!(NetTrialRevert::from_raw(0), None);
    assert_eq!(NetTrialRevert::from_raw(6), None);
    assert_eq!(NetTrialRevert::default(), NetTrialRevert::NotConfirmed);
    for v in 1..=2 {
        assert_eq!(NetTrialState::from_raw(v).map(|s| s as u8), Some(v));
    }
    assert_eq!(NetTrialState::from_raw(0), None);
    assert_eq!(NetTrialState::from_raw(3), None);
    for v in 0..=3 {
        assert_eq!(NetTrialBoot::from_raw(v).map(|b| b as u8), Some(v));
    }
    assert_eq!(NetTrialBoot::from_raw(4), None);
}

#[test]
fn decode_a_nul_inside_a_stored_text_ends_it() {
    // Rust: the C++ copies the stored bytes into the char arrays, so a NUL inside the SSID or
    // the password ends the text there (found by the differential check).
    let r = NetTrialRecord {
        previous: static_net(),
        ..NetTrialRecord::default()
    };
    let mut v = encode(&r);
    v[24] = 4;
    v.splice(25..25, *b"ab\0c");
    v[29] = 3;
    v.splice(30..30, *b"\0xy");
    let v = recrc(v);
    let mut out = NetTrialRecord::default();
    assert!(decode_net_trial(&v, &mut out));
    assert_text(&out.previous.ssid, "ab");
    assert!(out.previous.wifi_password.is_empty());
    // The fields CRC sees the texts as the C++ does.
    let mut c = r.previous.clone();
    copy_string(&mut c.ssid, b"ab");
    assert_eq!(
        net_trial_fields_crc(&out.previous),
        net_trial_fields_crc(&c)
    );
}
