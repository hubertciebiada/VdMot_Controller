//! Port of test/native/test_net_policy.cpp: NetReachability, NetWatchdog, network evidence
//! names.

use super::*;
use NetReachabilityChange as C;
use NetWatchdogAction as A;

const GW: u32 = 0x0101_A8C0; // 192.168.1.1

#[test]
fn net_evidence_name_cases() {
    assert_eq!(net_evidence_name(NetEvidence::None), "none");
    assert_eq!(net_evidence_name(NetEvidence::GatewayPing), "ping");
    assert_eq!(net_evidence_name(NetEvidence::Mqtt), "mqtt");
    assert_eq!(net_evidence_name(NetEvidence::TimeSync), "ntp");
    assert_eq!(net_evidence_name(NetEvidence::InboundHttp), "http");
    assert_eq!(net_evidence_name(NetEvidence::DhcpLease), "dhcp");
    // C++ netEvidenceName(static_cast<NetEvidence>(6)) == "unknown": refused by from_raw.
    assert_eq!(NetEvidence::from_raw(6), None);
    assert_eq!(NetEvidence::DhcpLease as u8, 5);
    // Rust addition: from_raw is the inverse of the discriminants.
    for v in 0..=5u8 {
        assert_eq!(NetEvidence::from_raw(v).map(|e| e as u8), Some(v));
    }
}

#[test]
fn reachability_ip_down_then_up_probe_ping_reply_staleness_lost_regained() {
    let mut r = NetReachability::default();
    assert!(!r.ip_up());
    r.update(false, GW, 0);
    assert!(!r.reachable(0));
    assert!(!r.probe_due(0));
    assert!(!r.proven());
    assert_eq!(r.change(0), C::None);
    r.update(true, GW, 1000);
    assert!(r.ip_up());
    assert!(r.reachable(1000));
    assert!(!r.armed());
    assert!(!r.proven());
    assert_eq!(r.last_evidence(), NetEvidence::None);
    assert_eq!(r.evidence_age_ms(1000), u32::MAX);
    assert!(r.probe_due(1000));
    assert_eq!(r.change(1000), C::None);
    r.on_probe_sent(1000);
    assert!(!r.probe_due(1000));
    assert!(!r.probe_due(60999));
    assert!(r.probe_due(61000));
    r.on_evidence(NetEvidence::GatewayPing, 1200);
    assert!(r.armed());
    assert!(r.proven());
    assert_eq!(r.last_evidence(), NetEvidence::GatewayPing);
    assert_eq!(r.evidence_age_ms(1300), 100);
    assert!(r.reachable(1200 + 149_999));
    assert_eq!(r.change(1200 + 149_999), C::None);
    assert!(!r.reachable(1200 + 150_000));
    assert_eq!(r.change(1200 + 150_000), C::Lost);
    assert_eq!(r.change(1200 + 151_000), C::None);
    assert_eq!(r.lost_for_ms(1200 + 151_000), 1000);
    assert!(r.proven()); // evidence since the IP came up, only stale
    r.on_evidence(NetEvidence::Mqtt, 200_000);
    assert_eq!(r.last_evidence(), NetEvidence::Mqtt);
    assert!(r.armed());
    assert_eq!(r.change(200_000), C::Regained);
    assert_eq!(r.lost_for_ms(200_000), 200_000 - 151_200);
    assert_eq!(r.change(201_000), C::None);
}

#[test]
fn reachability_never_armed_the_ip_alone_is_reachable_for_ever() {
    let mut r = NetReachability::default();
    r.update(true, GW, 0);
    r.on_evidence(NetEvidence::Mqtt, 10);
    assert!(!r.armed());
    assert!(r.proven());
    assert!(r.reachable(4_000_000_000));
    assert_eq!(r.change(4_000_000_000), C::None);
    assert_eq!(r.lost_for_ms(4_000_000_000), 0);
}

#[test]
fn reachability_a_gateway_change_disarms_and_makes_the_probe_due() {
    let mut r = NetReachability::default();
    r.update(true, GW, 0);
    r.on_probe_sent(0);
    r.on_evidence(NetEvidence::GatewayPing, 100);
    assert!(r.armed());
    assert!(!r.reachable(150_100));
    r.update(true, GW + 1, 150_100);
    assert!(!r.armed());
    assert!(r.reachable(150_100));
    assert!(r.probe_due(150_100));
    assert!(r.proven()); // the evidence stays
    r.on_probe_sent(150_100);
    r.update(true, 0, 150_200); // to no gateway
    assert!(!r.probe_due(150_200));
    assert!(!r.probe_due(400_000));
    assert!(r.reachable(400_000));
}

#[test]
fn reachability_an_ip_loss_clears_the_evidence_and_cancels_a_lost() {
    let mut r = NetReachability::default();
    r.update(true, GW, 0);
    r.on_evidence(NetEvidence::GatewayPing, 0);
    assert_eq!(r.change(0), C::None);
    assert_eq!(r.change(150_000), C::Lost);
    r.update(false, 0, 160_000);
    assert!(!r.proven());
    assert_eq!(r.last_evidence(), NetEvidence::None);
    assert_eq!(r.evidence_age_ms(160_000), u32::MAX);
    assert_eq!(r.lost_for_ms(160_000), 0);
    assert_eq!(r.change(160_000), C::None); // the IP loss is NetDown's business
    assert!(r.armed()); // same gateway: stays armed
    r.on_evidence(NetEvidence::Mqtt, 161_000); // ignored while down
    assert_eq!(r.last_evidence(), NetEvidence::None);
    r.update(true, GW, 170_000);
    assert!(r.armed());
    assert!(r.probe_due(170_000));
    assert!(r.reachable(170_000)); // the IP coming up starts the clock
    assert_eq!(r.change(170_000), C::None); // no Regained: the IP loss cancelled the Lost
    assert!(r.reachable(170_000 + 149_999));
    assert!(!r.reachable(170_000 + 150_000));
    assert_eq!(r.change(170_000 + 150_000), C::Lost);
}

#[test]
fn reachability_none_evidence_is_ignored() {
    let mut r = NetReachability::default();
    r.update(true, GW, 0);
    r.on_evidence(NetEvidence::None, 5);
    assert!(!r.proven());
    assert_eq!(r.evidence_age_ms(5), u32::MAX);
    r.on_evidence(NetEvidence::DhcpLease, 6);
    assert!(r.proven());
    assert!(!r.armed());
    assert_eq!(r.evidence_age_ms(10), 4);
}

#[test]
fn watchdog_interface_restart_after_minutes_esp_restart_after_another_wait() {
    let mut w = NetWatchdog::default();
    w.configure(5);
    assert_eq!(w.update(false, 1000), A::None); // boot counts as the start of the outage
    assert_eq!(w.outage_ms(1000), 0);
    assert_eq!(w.update(false, 1000 + 299_999), A::None);
    assert_eq!(w.outage_ms(1000 + 299_999), 299_999);
    assert_eq!(w.update(false, 1000 + 300_000), A::RestartInterface);
    assert_eq!(w.interface_restarts(), 1);
    assert_eq!(w.update(false, 1000 + 300_001), A::None); // once
    assert_eq!(w.update(false, 1000 + 599_999), A::None);
    assert_eq!(w.restarts_in_outage(), 0);
    assert_eq!(w.update(false, 1000 + 600_000), A::RestartEsp);
    assert_eq!(w.restarts_in_outage(), 1);
    assert_eq!(w.update(false, 1000 + 600_001), A::None);
    assert_eq!(w.update(false, 4_000_000_000), A::None);
    assert_eq!(w.interface_restarts(), 1);
    assert_eq!(w.update(true, 4_000_001_000), A::None);
    assert_eq!(w.restarts_in_outage(), 0);
    assert_eq!(w.outage_ms(4_000_001_000), 0);
    // Re-armed: a new outage runs both stages again.
    assert_eq!(w.update(false, 4_000_002_000), A::None);
    assert_eq!(
        w.update(false, 4_000_002_000 + 300_000),
        A::RestartInterface
    );
    assert_eq!(w.interface_restarts(), 2);
    assert_eq!(w.update(false, 4_000_002_000 + 600_000), A::RestartEsp);
}

#[test]
fn watchdog_one_earlier_restart_interface_at_5_min_esp_at_25_min() {
    let mut w = NetWatchdog::default();
    w.configure(5);
    w.set_restarts_in_outage(1);
    assert_eq!(w.update(false, 0), A::None);
    assert_eq!(w.update(false, 299_999), A::None);
    assert_eq!(w.update(false, 300_000), A::RestartInterface);
    assert_eq!(w.update(false, 1_499_999), A::None);
    assert_eq!(w.update(false, 1_500_000), A::RestartEsp);
    assert_eq!(w.restarts_in_outage(), 2);
}

#[test]
fn watchdog_0_disables_both_actions() {
    let mut off = NetWatchdog::default();
    assert_eq!(off.update(false, 0), A::None);
    assert_eq!(off.update(false, 4_000_000_000), A::None);
    assert_eq!(off.interface_restarts(), 0);
    assert_eq!(off.outage_ms(4_000_000_000), 4_000_000_000);
}

#[test]
fn watchdog_configure_re_arms_after_an_action_enabled_later_counts_the_outage() {
    let mut late = NetWatchdog::default();
    assert_eq!(late.update(false, 0), A::None);
    assert_eq!(late.update(false, 200_000), A::None);
    late.configure(2);
    assert_eq!(late.update(false, 200_001), A::RestartInterface);
    assert_eq!(late.update(false, 239_999), A::None);
    assert_eq!(late.update(false, 240_000), A::RestartEsp);
    late.configure(2); // re-armed, the grown wait (2 min * 4) applies to the ESP stage
    assert_eq!(late.update(false, 240_001), A::RestartInterface);
    assert_eq!(late.update(false, 599_999), A::None);
    assert_eq!(late.update(false, 600_000), A::RestartEsp);
    late.configure(0);
    assert_eq!(late.update(false, 999_999_999), A::None);
}

#[test]
fn watchdog_up_at_boot_lost_later_up_again_resets_the_timer() {
    let mut u = NetWatchdog::default();
    u.configure(1);
    assert_eq!(u.update(true, 0), A::None);
    assert_eq!(u.update(false, 10_000), A::None);
    assert_eq!(u.update(true, 69_000), A::None);
    assert_eq!(u.update(false, 70_000), A::None);
    assert_eq!(u.update(false, 129_999), A::None);
    assert_eq!(u.update(false, 130_000), A::RestartInterface);
    assert_eq!(u.update(false, 189_999), A::None);
    assert_eq!(u.update(false, 190_000), A::RestartEsp);
}

#[test]
fn watchdog_maximum_setting() {
    let mut max = NetWatchdog::default();
    max.configure(255);
    assert_eq!(max.update(false, 0), A::None);
    assert_eq!(max.update(false, 255 * 60_000 - 1), A::None);
    assert_eq!(max.update(false, 255 * 60_000), A::RestartInterface);
    assert_eq!(max.update(false, 510 * 60_000 - 1), A::None);
    assert_eq!(max.update(false, 510 * 60_000), A::RestartEsp);
}

#[test]
fn watchdog_wait_ms_grows_with_every_restart_of_one_outage() {
    // Router off for hours: ESP restarts 5, 20, 80, 320, 1280 min after the interface restart,
    // then every 24 h (each one also resets the STM).
    let expect_min: [u32; 7] = [5, 20, 80, 320, 1280, 1440, 1440];
    let mut kept = 0u8; // what the glue keeps in RTC memory across restarts
    for m in expect_min {
        let mut w = NetWatchdog::default(); // a fresh boot
        w.configure(5);
        w.set_restarts_in_outage(kept);
        assert_eq!(w.wait_ms(), m * 60_000, "m {m}");
        assert_eq!(w.update(false, 700), A::None);
        assert_eq!(w.update(false, 700 + 300_000), A::RestartInterface);
        assert_eq!(w.update(false, 700 + 300_000 + m * 60_000 - 1), A::None);
        assert_eq!(w.update(false, 700 + 300_000 + m * 60_000), A::RestartEsp);
        kept = w.restarts_in_outage();
    }
    assert_eq!(kept, 7);

    let mut big = NetWatchdog::default();
    big.configure(240);
    big.set_restarts_in_outage(255);
    assert_eq!(big.wait_ms(), 1440 * 60_000);
    assert_eq!(big.update(false, 0), A::None);
    assert_eq!(big.update(false, 1680 * 60_000), A::RestartEsp);
    assert_eq!(big.restarts_in_outage(), 255);
    let mut one = NetWatchdog::default();
    one.configure(1);
    one.set_restarts_in_outage(1);
    assert_eq!(one.wait_ms(), 4 * 60_000);
    one.configure(0);
    assert_eq!(one.wait_ms(), 0);
}

#[test]
fn watchdog_interface_restarts_saturate() {
    let mut w = NetWatchdog::default();
    w.configure(1);
    let mut t: u32 = 0;
    for _ in 0..65536u32 {
        w.update(false, t);
        w.update(false, t + 60_000);
        w.update(true, t + 61_000);
        t += 62_000;
    }
    assert_eq!(w.interface_restarts(), 65535);
}

#[test]
fn watchdog_constants_and_default() {
    // Rust addition: the constants and the default state (down since boot, nothing fired).
    assert_eq!(NetWatchdog::GROWTH, 4);
    assert_eq!(NetWatchdog::MAX_WAIT_MIN, 1440);
    assert_eq!(NetReachability::STALE_MS, 150_000);
    assert_eq!(NetReachability::PROBE_INTERVAL_MS, 60_000);
    let w = NetWatchdog::default();
    assert_eq!(w.restarts_in_outage(), 0);
    assert_eq!(w.interface_restarts(), 0);
    assert_eq!(w.wait_ms(), 0);
    assert_eq!(w.outage_ms(123), 0); // not started
}
