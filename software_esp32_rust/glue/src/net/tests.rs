//! Port of `test/native/glue/test_net.cpp`: interfaces, state events, WiFi fallback, time,
//! end-to-end reachability, watchdog stages, network trial. Arduino events become the state of
//! the interface fakes (`got_ip`, `link_down`, `link`), `fakes::net().syncTime` the wall clock
//! and an SNTP sync, the sibling fakes the [`NetHost`] fake.
#![allow(clippy::large_stack_frames)]

use super::support::*;
use super::*;
use crate::board;
use crate::testkit::{FakeBoard, Reset};
use vdm_esp_core::common::build_hostname;
use vdm_esp_core::net_trial::NetTrialState;

/// C++ `fakes::net().syncTime(epoch)`: the wall clock is set and lwIP reports the sync.
fn sync_time(rig: &Rig, epoch: i64) {
    rig.dev.wall.set(epoch);
    rig.dev.sntp.sync(epoch as u32);
}

/// C++ `bootWithRecord`: the record and the active config as the previous boot left them,
/// then `begin` with the config on trial.
fn boot_with_record(
    rig: &Rig,
    net: &mut Net<'_, crate::testkit::board::TestPlatform, FakeNetHost>,
    st: NetTrialState,
    prev: &Config,
    on_trial: &mut Config,
) {
    rig.store_record(st, prev, on_trial);
    net.begin(on_trial);
}

fn text(e: &Event) -> &[u8] {
    &e.text
}

#[test]
fn begin_ethernet_with_the_wt32_eth01_phy_wiring() {
    let rig = Rig::new();
    let mut net = rig.net();
    let mut c = config();
    net.begin(&mut c);
    let eth = rig.dev.eth.state();
    assert_eq!(eth.begins.len(), 1);
    // PHY address 1, power (oscillator enable) 16, MDC 23, MDIO 18; LAN8720 and the RMII clock
    // on GPIO0 are fixed in the adapter (GLUE-DESIGN-ESP.md 1.2)
    assert_eq!(
        (
            board::ETH_PHY_ADDR,
            board::ETH_PHY_POWER,
            board::ETH_MDC,
            board::ETH_MDIO
        ),
        (1, 16, 23, 18)
    );
    assert_eq!(eth.begins[0].fixed, None); // DHCP
    drop(eth);
    assert!(rig.dev.wifi.state().begins.is_empty());
    assert!(rig.dev.wifi.state().connects.is_empty());
    let mut host = [0u8; 64];
    let n = build_hostname(b"Heating Floor", &mut host);
    assert_eq!(net.hostname().as_bytes(), &host[..n]);
    assert_eq!(net.hostname(), "Heating-Floor");
    assert_eq!(rig.host.state().net_trial_clears, 0);
    assert!(!rig.shared.trial_info().active);
}

#[test]
fn begin_a_static_address_is_configured_on_ethernet() {
    let rig = Rig::new();
    let mut net = rig.net();
    let mut c = static_config(IP);
    c.net.dns = ip(192, 168, 1, 2);
    net.begin(&mut c);
    let eth = rig.dev.eth.state();
    assert_eq!(eth.begins.len(), 1);
    assert_eq!(
        eth.begins[0].fixed,
        Some(IpInfo {
            ip: IP,
            mask: MASK,
            gateway: GW,
            dns: ip(192, 168, 1, 2),
        })
    );
}

#[test]
fn begin_a_static_address_without_dns_uses_the_gateway_as_dns() {
    let rig = Rig::new();
    let mut net = rig.net();
    let mut c = static_config(IP);
    net.begin(&mut c);
    let eth = rig.dev.eth.state();
    assert_eq!(eth.begins.len(), 1);
    assert_eq!(eth.begins[0].fixed.map(|f| f.dns), Some(GW));
    assert_eq!(c.net.dns, 0); // the stored value stays empty
}

#[test]
fn begin_an_ethernet_driver_that_does_not_start_is_logged() {
    let rig = Rig::new();
    rig.dev.eth.state().begin_ok = false;
    let mut net = rig.net();
    let mut c = config();
    net.begin(&mut c);
    let e = rig.host.first(EventCode::NetDown);
    assert_eq!((e.arg1, e.arg2), (1, 0));
    assert_eq!(text(&e), b"eth init failed");
    assert_eq!(e.severity, event_default_severity(EventCode::NetDown));
    assert_eq!(rig.dev.eth.state().begins[0].fixed, None);
    net.service(0, false);
    assert!(rig.dev.wifi.state().connects.is_empty()); // no SSID: no WiFi to fall back to
}

#[test]
fn begin_sntp_with_the_configured_server_and_the_posix_tz_string() {
    let rig = Rig::new();
    let mut net = rig.net();
    let mut c = config();
    net.begin(&mut c);
    assert_eq!(
        rig.dev.sntp.state().configured,
        vec![Some("pool.ntp.org".to_string())]
    );
    assert_eq!(rig.dev.wall.zones(), vec!["CET-1CEST,M3.5.0,M10.5.0/3"]);
}

#[test]
fn begin_without_an_ntp_server_sntp_is_stopped_and_only_tz_is_set() {
    let rig = Rig::new();
    let mut net = rig.net();
    let mut c = config();
    c.time.ntp_server.clear();
    net.begin(&mut c);
    assert_eq!(rig.dev.sntp.state().configured, vec![None]); // stops a running SNTP
    assert_eq!(rig.dev.wall.zones(), vec!["CET-1CEST,M3.5.0,M10.5.0/3"]);
}

#[test]
fn eth_start_sets_the_host_name_of_the_ethernet_interface() {
    // C++: ETH_START handed the name to the interface; the port takes it with the setup
    let rig = Rig::new();
    let mut net = rig.net();
    let mut c = config();
    net.begin(&mut c);
    assert_eq!(rig.dev.eth.state().begins[0].hostname, net.hostname());
}

#[test]
fn service_ethernet_with_an_address_is_up_net_up_names_the_address() {
    let rig = Rig::new();
    let mut net = rig.net();
    let mut c = config();
    net.begin(&mut c);
    net.service(1000, false);
    assert!(!rig.shared.is_up());
    rig.ethernet_up(ip(192, 168, 1, 7), 0);
    net.service(2000, false);
    assert!(rig.shared.is_up());
    let i = rig.shared.info();
    assert_eq!(i.state, NetState::Ethernet);
    assert_eq!(i.ip, ip(192, 168, 1, 7));
    assert_eq!((i.up_since_ms, i.reconnects), (2000, 1));
    assert_eq!(&i.mac[..], b"A8:03:2A:A1:B2:C3");
    assert_eq!((i.mask, i.rssi), (MASK, 0));
    let e = rig.host.first(EventCode::NetUp);
    assert_eq!(e.arg1, NetState::Ethernet as i32);
    assert_eq!(text(&e), b"192.168.1.7");
    // no mDNS (C++ checked that none was started): the firmware has no mDNS port
    rig.dev.eth.link_down();
    net.service(3000, false);
    assert!(!rig.shared.is_up());
    assert_eq!(
        rig.host.first(EventCode::NetDown).arg1,
        NetState::Ethernet as i32
    );
}

#[test]
fn service_wifi_as_fallback_after_30_s_without_ethernet() {
    let rig = Rig::new();
    let mut net = rig.net();
    let mut c = config();
    copy_string(&mut c.net.ssid, b"home");
    copy_string(&mut c.net.wifi_password, b"secret-pass");
    net.begin(&mut c);
    net.service(0, false);
    net.service(29_999, false);
    assert!(rig.dev.wifi.state().connects.is_empty());
    net.service(30_000, false);
    let w = rig.dev.wifi.state();
    assert_eq!(
        w.connects,
        vec![(b"home".to_vec(), b"secret-pass".to_vec())]
    );
    // the station is built with the host name before its start; nothing goes into the WiFi
    // NVS and Arduino's automatic reconnect does not exist (the adapter, design 1.2)
    assert_eq!(w.begins.len(), 1);
    assert_eq!(w.begins[0].hostname, net.hostname());
    assert_eq!(w.begins[0].fixed, None);
}

#[test]
fn reachability_dhcp_lease_mqtt_sntp_and_lan_http_prove_the_network() {
    let rig = Rig::new();
    let mut net = rig.net();
    let mut c = config();
    net.begin(&mut c);
    rig.ethernet_up(IP, GW);
    rig.dev.pinger.state().default_answer = false;
    rig.tick(&mut net, 1000, false); // the GOT_IP of this DHCP configuration counts as evidence
    let h = rig.shared.health();
    assert!(h.ip_up && h.reachable && h.proven);
    assert!(!h.ping_armed);
    assert_eq!(h.evidence, NetEvidence::DhcpLease);
    assert_eq!(h.evidence_age_s, 0);
    assert!(rig.shared.ota_net_ok());
    rig.tick(&mut net, 5000, true);
    assert_eq!(rig.shared.health().evidence, NetEvidence::Mqtt);
    sync_time(&rig, 1_790_136_000);
    rig.tick(&mut net, 6000, false);
    assert_eq!(rig.shared.health().evidence, NetEvidence::TimeSync);
    rig.shared.note_inbound_http(ip(127, 0, 0, 1));
    rig.shared.note_inbound_http(IP);
    rig.tick(&mut net, 9000, false);
    let h = rig.shared.health();
    assert_eq!(h.evidence, NetEvidence::TimeSync);
    assert_eq!(h.evidence_age_s, 3);
    rig.shared.note_inbound_http(ip(192, 168, 1, 99));
    rig.tick(&mut net, 10_000, false);
    assert_eq!(rig.shared.health().evidence, NetEvidence::InboundHttp);
}

#[test]
fn reachability_a_static_address_is_not_proven_by_its_got_ip_the_gateway_ping_is() {
    let rig = Rig::new();
    let mut net = rig.net();
    let mut c = static_config(IP);
    net.begin(&mut c);
    rig.dev.pinger.state().default_answer = false;
    rig.ethernet_up(IP, GW);
    rig.tick(&mut net, 1000, false);
    let h = rig.shared.health();
    assert!(h.ip_up && h.reachable);
    assert!(!h.proven);
    assert_eq!(h.evidence, NetEvidence::None);
    assert_eq!(h.evidence_age_s, u32::MAX);
    assert!(!rig.shared.ota_net_ok());
    // one echo of 32 B with a 1 s timeout to the gateway: the session of the Pinger port
    assert_eq!(rig.dev.pinger.state().started, vec![GW]);
    rig.dev.pinger.state().default_answer = true;
    rig.tick(&mut net, 61_000, false); // the next probe, answered
    rig.tick(&mut net, 62_000, false);
    let h = rig.shared.health();
    assert!(h.proven && h.ping_armed);
    assert_eq!(h.evidence, NetEvidence::GatewayPing);
    assert!(rig.shared.ota_net_ok());
    // a session per probe, deleted by the pass after its report
    let p = rig.dev.pinger.state();
    assert_eq!(p.started, vec![GW, GW]);
    assert_eq!(p.deletes, 2);
}

#[test]
fn watchdog_gateway_silent_unreachable_interface_restart_esp_restart() {
    let rig = Rig::new();
    let mut net = rig.net();
    let mut c = static_config(IP);
    net.begin(&mut c);
    rig.ethernet_up(IP, GW);
    rig.tick(&mut net, 1000, false); // probe answered
    rig.tick(&mut net, 2000, false); // the reply seen: armed, evidence at 2 s
    assert!(rig.shared.health().ping_armed);
    rig.dev.pinger.state().default_answer = false;
    rig.run(&mut net, 3000, 151_000, 1000);
    assert!(!rig.host.has(EventCode::NetUnreachable));
    rig.tick(&mut net, 152_000, false);
    let lost = rig.host.with_code(EventCode::NetUnreachable);
    assert_eq!(lost.len(), 1);
    assert_eq!(lost[0].arg1, 150);
    assert_eq!(lost[0].arg2, NetEvidence::GatewayPing as i32);
    assert!(!rig.shared.health().reachable);
    assert!(!rig.shared.ota_net_ok());
    rig.run(&mut net, 153_000, 451_000, 1000);
    assert_eq!(rig.dev.eth.state().restarts, 0);
    rig.tick(&mut net, 452_000, false);
    // stop, 200 ms, start: the adapter (Ethernet::restart)
    assert_eq!(rig.dev.eth.state().restarts, 1);
    let ir = rig.host.first(EventCode::NetInterfaceRestart);
    assert_eq!((ir.arg1, ir.arg2), (300, 1));
    assert_eq!(rig.shared.health().iface_restarts, 1);
    rig.run(&mut net, 453_000, 751_000, 1000);
    assert!(rig.host.restarts().is_empty());
    rig.tick(&mut net, 752_000, false);
    assert_eq!(
        rig.host.restarts(),
        vec![RestartRequest {
            reason: 2,
            delay_ms: 1000,
            detail: 10
        }]
    );
}

#[test]
fn watchdog_replies_coming_back_end_the_outage_without_a_restart() {
    let rig = Rig::new();
    let mut net = rig.net();
    let mut c = static_config(IP);
    net.begin(&mut c);
    rig.ethernet_up(IP, GW);
    rig.tick(&mut net, 1000, false);
    rig.tick(&mut net, 2000, false);
    rig.dev.pinger.state().default_answer = false;
    rig.run(&mut net, 3000, 200_000, 1000);
    assert!(rig.host.has(EventCode::NetUnreachable));
    rig.dev.pinger.state().default_answer = true;
    rig.run(&mut net, 201_000, 800_000, 1000);
    let back = rig.host.with_code(EventCode::NetReachable);
    assert_eq!(back.len(), 1);
    assert_eq!(back[0].arg1, 90); // lost at 152 s, the reply of the 241 s probe seen at 242 s
    assert!(rig.host.restarts().is_empty());
    assert_eq!(rig.dev.eth.state().restarts, 0);
}

#[test]
fn watchdog_a_gateway_that_never_answers_leaves_the_ip_only_rule_for_24_h() {
    let rig = Rig::new();
    let mut net = rig.net();
    let mut c = static_config(IP);
    net.begin(&mut c);
    rig.dev.pinger.state().default_answer = false;
    rig.ethernet_up(IP, GW);
    rig.run(&mut net, 1000, 86_400_000, 10_000);
    assert!(rig.host.restarts().is_empty());
    assert_eq!(rig.dev.eth.state().restarts, 0);
    assert!(!rig.host.has(EventCode::NetUnreachable));
    assert!(!rig.shared.health().ping_armed);
}

#[test]
fn watchdog_no_ip_from_boot_interface_restart_only_with_a_driver_handle() {
    let rig = Rig::new();
    let mut net = rig.net();
    let mut c = config();
    net.begin(&mut c);
    rig.dev.eth.state().link = true; // link, no DHCP answer
    rig.run(&mut net, 0, 299_000, 1000);
    assert_eq!(rig.dev.eth.state().restarts, 0);
    rig.tick(&mut net, 300_000, false);
    assert_eq!(rig.dev.eth.state().restarts, 1);
    assert_eq!(rig.host.first(EventCode::NetInterfaceRestart).arg1, 300);
    rig.run(&mut net, 301_000, 600_000, 1000);
    assert_eq!(rig.host.restarts().len(), 1);
    assert_eq!(rig.dev.clock.ms(), 600_000);
    assert_eq!(rig.host.restarts()[0].detail, 10);
}

#[test]
fn watchdog_without_a_driver_handle_and_wifi_nothing_is_restarted_at_stage_1() {
    let rig = Rig::new();
    // no link since boot: the driver has no handle to restart (Ethernet::restart refuses)
    rig.dev.eth.state().restart_ok = false;
    let mut net = rig.net();
    let mut c = config();
    net.begin(&mut c);
    rig.run(&mut net, 0, 300_000, 1000);
    assert_eq!(rig.dev.eth.state().restarts, 1); // asked once, refused
    assert_eq!(rig.dev.wifi.state().reconnects, 0);
    assert!(!rig.host.has(EventCode::NetInterfaceRestart));
    assert_eq!(rig.shared.health().iface_restarts, 1);
    rig.run(&mut net, 301_000, 600_000, 1000);
    assert_eq!(rig.dev.clock.ms(), 600_000);
    assert_eq!(rig.host.restarts().len(), 1);
}

#[test]
fn watchdog_wifi_is_reconnected_at_stage_1() {
    let rig = Rig::new();
    let mut net = rig.net();
    let mut c = wifi_config(b"home");
    net.begin(&mut c);
    rig.run(&mut net, 0, 300_000, 1000);
    assert_eq!(rig.dev.wifi.state().reconnects, 1);
    assert_eq!(rig.host.first(EventCode::NetInterfaceRestart).arg2, 2);
    // Ethernet was never started with the interface WiFi: no restart of its driver
    assert_eq!(rig.dev.eth.state().restarts, 0);
}

#[test]
fn watchdog_0_minutes_disables_both_stages() {
    let rig = Rig::new();
    let mut net = rig.net();
    let mut c = config();
    c.net.reconnect_timeout_min = 0;
    net.begin(&mut c);
    rig.dev.eth.state().link = true;
    rig.run(&mut net, 0, 3_600_000, 10_000);
    assert_eq!(rig.dev.eth.state().restarts, 0);
    assert!(rig.host.restarts().is_empty());
}

#[test]
fn watchdog_the_restart_count_of_an_outage_survives_esp_restart() {
    let board = FakeBoard::new();
    let mut c = config();
    c.net.reconnect_timeout_min = 5;
    {
        let rig = Rig::boot(&board, None);
        let mut net = rig.net();
        net.begin(&mut c.clone());
        rig.run(&mut net, 0, 599_000, 1000);
        assert!(rig.host.restarts().is_empty());
        rig.run(&mut net, 600_000, 600_000, 1000);
        assert_eq!(rig.host.restarts().len(), 1);
        assert_eq!(rig.host.restarts()[0].reason, 2);
        board.reset(Reset::Software);
    }
    // boot 1: one restart in this outage, ESP stage at 5 + 4 x 5 min
    let rig = Rig::boot(&board, None);
    let mut net = rig.net();
    net.begin(&mut c);
    rig.run(&mut net, 0, 1_499_000, 1000);
    assert!(rig.host.restarts().is_empty());
    rig.run(&mut net, 1_500_000, 1_500_000, 1000);
    assert_eq!(rig.host.restarts().len(), 1);
}

#[test]
fn reconfigure_a_tz_change_applies_live_a_station_change_restarts_without_trial() {
    let rig = Rig::new();
    let mut net = rig.net();
    let mut c = config();
    net.begin(&mut c);
    let mut tz = c.clone();
    copy_string(&mut tz.time.tz_posix, b"UTC0");
    net.reconfigure(&tz);
    assert!(rig.host.restarts().is_empty());
    assert_eq!(
        rig.dev.wall.zones(),
        vec!["CET-1CEST,M3.5.0,M10.5.0/3", "UTC0"]
    );
    assert_eq!(rig.dev.sntp.state().configured.len(), 2);
    let mut st = tz.clone();
    copy_string(&mut st.station, b"Other");
    net.reconfigure(&st);
    assert_eq!(
        rig.host.restarts(),
        vec![RestartRequest {
            reason: 0,
            delay_ms: 1500,
            detail: 0
        }]
    );
    let s = rig.host.state();
    assert_eq!(s.net_trial_saves, 0);
    // nothing to revert: net stores no config and touches no record
    assert!(s.applied.is_empty());
    assert_eq!(s.net_trial_clears, 0);
    drop(s);
    assert!(!rig.host.has(EventCode::NetTrialStarted));
    assert!(!rig.host.has(EventCode::NetTrialReverted));
}

#[test]
fn reconfigure_a_station_name_of_full_length_is_kept_whole_for_the_restart_rule() {
    let rig = Rig::new();
    let mut net = rig.net();
    let mut c = config();
    copy_string(&mut c.station, b"abcdefghijklmnopqrst"); // 20 characters
    net.begin(&mut c);
    net.reconfigure(&c);
    assert!(rig.host.restarts().is_empty());
    copy_string(&mut c.station, b"abcdefghijklmnopqrsu"); // the last one differs
    net.reconfigure(&c);
    assert_eq!(rig.host.restarts().len(), 1);
    assert_eq!(rig.host.restarts()[0].reason, 0);
}

#[test]
fn reconfigure_unused_static_fields_and_reconnect_timeout_min_do_not_restart() {
    let rig = Rig::new();
    let mut net = rig.net();
    let mut c = config();
    net.begin(&mut c);
    let mut d = c.clone();
    d.net.ip = NEW_IP;
    d.net.gateway = GW;
    d.net.reconnect_timeout_min = 9;
    copy_string(&mut d.time.ntp_server, b"ntp.example");
    net.reconfigure(&d);
    assert!(rig.host.restarts().is_empty());
    assert_eq!(rig.host.state().net_trial_saves, 0);
    assert_eq!(
        rig.dev.sntp.state().configured.last(),
        Some(&Some("ntp.example".to_string()))
    );
}

#[test]
fn trial_a_new_static_address_is_armed_with_the_old_settings_and_restarts() {
    let rig = Rig::new();
    let mut net = rig.net();
    let mut c = config();
    net.begin(&mut c);
    let n = static_config(NEW_IP);
    net.reconfigure(&n);
    let r = rig.host.stored_record();
    assert_eq!(r.state, NetTrialState::Armed);
    assert!(r.previous.dhcp);
    assert_eq!(r.previous.iface, NetInterface::Auto);
    assert_eq!(r.trial_crc, net_trial_fields_crc(&n.net));
    let e = rig.host.first(EventCode::NetTrialStarted);
    assert_eq!((e.arg1, e.arg2), (120, 0));
    assert_eq!(text(&e), b"192.168.1.50");
    assert_eq!(rig.host.restarts().len(), 1);
    assert_eq!(rig.host.restarts()[0].reason, 0);
}

#[test]
fn trial_two_changes_before_the_restart_keep_the_first_previous_settings() {
    let rig = Rig::new();
    let mut net = rig.net();
    let mut c = config();
    net.begin(&mut c);
    net.reconfigure(&static_config(NEW_IP));
    let b = static_config(ip(192, 168, 1, 60));
    net.reconfigure(&b);
    let r = rig.host.stored_record();
    assert!(r.previous.dhcp);
    assert_eq!(r.trial_crc, net_trial_fields_crc(&b.net));
    assert_eq!(rig.host.state().net_trial_saves, 2);
    assert!(!rig.host.has(EventCode::NetTrialConfirmed));
}

#[test]
fn trial_boot_with_an_armed_record_runs_it_no_confirm_reverts_120_s_after_the_ip() {
    let rig = Rig::new();
    let mut net = rig.net();
    let old = config();
    let mut n = static_config(NEW_IP);
    boot_with_record(&rig, &mut net, NetTrialState::Armed, &old, &mut n);
    assert_eq!(rig.host.stored_record().state, NetTrialState::Running);
    assert_eq!(n.net.ip, NEW_IP); // not reverted
    assert_eq!(
        rig.shared.trial_info(),
        TrialInfo {
            active: true,
            remain_s: 120
        }
    );
    rig.run(&mut net, 1000, 4000, 1000);
    rig.ethernet_up(NEW_IP, GW);
    rig.run(&mut net, 5000, 124_000, 1000);
    assert!(rig.host.restarts().is_empty());
    assert_eq!(rig.shared.trial_info().remain_s, 1);
    let h = rig.shared.health();
    assert!(h.trial_active);
    assert_eq!(h.trial_remaining_s, 1);
    rig.tick(&mut net, 125_000, false);
    assert_eq!(
        rig.host.restarts(),
        vec![RestartRequest {
            reason: 5,
            delay_ms: 1000,
            detail: 0
        }]
    );
    let stored = rig.host.last_applied();
    assert!(stored.net.dhcp);
    assert_eq!(stored.net.iface, NetInterface::Auto);
    assert!(rig.host.state().net_trial.is_empty());
    let e = rig.host.first(EventCode::NetTrialReverted);
    assert_eq!((e.arg1, e.arg2), (1, 0));
    assert_eq!(text(&e), b"dhcp");
    assert!(!rig.shared.trial_info().active);
    assert!(!rig.shared.request_trial_confirm());
}

#[test]
fn trial_no_network_within_120_s_reverts_no_network() {
    let rig = Rig::new();
    let mut net = rig.net();
    let old = static_config(IP);
    let mut n = static_config(NEW_IP);
    boot_with_record(&rig, &mut net, NetTrialState::Armed, &old, &mut n);
    rig.run(&mut net, 1000, 120_000, 1000);
    assert_eq!(rig.host.restarts().len(), 1);
    assert_eq!(rig.dev.clock.ms(), 120_000);
    let e = rig.host.first(EventCode::NetTrialReverted);
    assert_eq!(e.arg1, 2);
    assert_eq!(text(&e), b"192.168.1.20");
    assert_eq!(rig.host.last_applied().net.ip, IP);
}

#[test]
fn trial_a_failed_persist_on_revert_keeps_the_record_for_the_next_boot() {
    let rig = Rig::new();
    let mut net = rig.net();
    let old = static_config(IP);
    let mut n = static_config(NEW_IP);
    boot_with_record(&rig, &mut net, NetTrialState::Armed, &old, &mut n);
    rig.host.state().apply_result = false;
    rig.run(&mut net, 1000, 120_000, 1000);
    assert_eq!(rig.host.restarts().len(), 1);
    assert_eq!(rig.host.restarts()[0].reason, 5);
    assert_eq!(rig.host.stored_record().state, NetTrialState::Running);
    assert_eq!(rig.host.first(EventCode::NetTrialReverted).arg2, -1);
}

#[test]
fn trial_the_revert_stores_the_active_config_with_the_previous_network_settings() {
    let rig = Rig::new();
    let mut net = rig.net();
    let old = static_config(IP);
    let mut n = static_config(NEW_IP);
    boot_with_record(&rig, &mut net, NetTrialState::Armed, &old, &mut n);
    {
        let mut s = rig.host.state();
        copy_string(&mut s.active.station, b"Saved Later"); // a later save stays
        s.active.net.reconnect_timeout_min = 9; // no trial field
    }
    rig.run(&mut net, 1000, 120_000, 1000);
    let s = rig.host.state();
    assert_eq!(s.applied.len(), 1);
    let stored = &s.applied[0];
    assert_eq!(&stored.station[..], b"Saved Later");
    assert_eq!(stored.net.ip, IP);
    assert!(!stored.net.dhcp);
    assert_eq!(stored.net.reconnect_timeout_min, 9);
    assert!(s.net_trial.is_empty());
    assert_eq!(s.get_configs, 1);
    drop(s);
    // the copy lived on the heap for the revert only
    assert_eq!(
        rig.dev.heap.state().granted,
        vec![core::mem::size_of::<Config>()]
    );
}

#[test]
fn trial_without_memory_for_the_config_copy_the_revert_keeps_the_record() {
    let rig = Rig::new();
    let mut net = rig.net();
    let old = static_config(IP);
    let mut n = static_config(NEW_IP);
    boot_with_record(&rig, &mut net, NetTrialState::Armed, &old, &mut n);
    rig.dev.heap.state().fail_all = true;
    rig.run(&mut net, 1000, 120_000, 1000);
    assert_eq!(rig.host.restarts().len(), 1);
    assert_eq!(rig.host.restarts()[0].reason, 5);
    assert!(rig.host.state().applied.is_empty());
    assert_eq!(rig.host.stored_record().state, NetTrialState::Running);
    assert_eq!(rig.host.first(EventCode::NetTrialReverted).arg2, -1);
}

#[test]
fn trial_confirm_ends_it_the_record_is_erased_no_revert() {
    let rig = Rig::new();
    let mut net = rig.net();
    let old = config();
    let mut n = static_config(NEW_IP);
    boot_with_record(&rig, &mut net, NetTrialState::Armed, &old, &mut n);
    rig.ethernet_up(NEW_IP, GW);
    rig.run(&mut net, 1000, 59_000, 1000); // the IP counted from 1 s
    assert!(rig.shared.request_trial_confirm());
    assert!(rig.shared.trial_info().active); // acted on in the next pass
    rig.tick(&mut net, 60_000, false);
    assert!(rig.host.state().net_trial.is_empty());
    let e = rig.host.first(EventCode::NetTrialConfirmed);
    assert_eq!((e.arg1, e.arg2), (59, 0));
    rig.run(&mut net, 61_000, 200_000, 1000);
    assert!(rig.host.restarts().is_empty());
    assert!(!rig.shared.request_trial_confirm());
    assert!(!rig.shared.request_trial_revert());
    assert_eq!(rig.shared.trial_info(), TrialInfo::default());
}

#[test]
fn trial_revert_on_request_reason_4_restarts() {
    let rig = Rig::new();
    let mut net = rig.net();
    let old = config();
    let mut n = static_config(NEW_IP);
    boot_with_record(&rig, &mut net, NetTrialState::Armed, &old, &mut n);
    rig.ethernet_up(NEW_IP, GW);
    rig.run(&mut net, 1000, 59_000, 1000);
    assert!(rig.shared.request_trial_revert());
    rig.tick(&mut net, 60_000, false);
    assert_eq!(rig.host.restarts().len(), 1);
    assert_eq!(rig.host.restarts()[0].reason, 5);
    assert_eq!(rig.host.first(EventCode::NetTrialReverted).arg1, 4);
    assert!(rig.host.last_applied().net.dhcp);
}

#[test]
fn trial_a_network_change_during_a_running_trial_confirms_it_and_arms_a_new_one() {
    let rig = Rig::new();
    let mut net = rig.net();
    let old = config();
    let mut n = static_config(NEW_IP);
    boot_with_record(&rig, &mut net, NetTrialState::Armed, &old, &mut n);
    rig.ethernet_up(NEW_IP, GW);
    rig.run(&mut net, 1000, 31_000, 1000);
    let next = static_config(ip(192, 168, 1, 70));
    net.reconfigure(&next);
    let e = rig.host.first(EventCode::NetTrialConfirmed);
    assert_eq!((e.arg1, e.arg2), (30, 1));
    let r = rig.host.stored_record();
    assert_eq!(r.state, NetTrialState::Armed);
    assert_eq!(r.previous.ip, NEW_IP); // the running settings
    assert!(!r.previous.dhcp);
    assert_eq!(r.trial_crc, net_trial_fields_crc(&next.net));
    assert_eq!(rig.host.restarts().len(), 1);
    assert_eq!(rig.host.restarts()[0].reason, 0);
}

#[test]
fn trial_a_boot_that_finds_a_running_record_reverts_before_the_interfaces_start() {
    let rig = Rig::new();
    let mut net = rig.net();
    let old = static_config(IP);
    let mut n = static_config(NEW_IP);
    let eth = rig.dev.eth.clone();
    rig.host.state().on_apply = Some(Box::new(move || {
        assert!(
            eth.state().begins.is_empty(),
            "applied after the interfaces started"
        );
    }));
    boot_with_record(&rig, &mut net, NetTrialState::Running, &old, &mut n);
    assert_eq!(n.net.ip, IP); // the caller's config is reverted
    let eth = rig.dev.eth.state();
    assert_eq!(eth.begins.len(), 1);
    assert_eq!(eth.begins[0].fixed.map(|f| f.ip), Some(IP));
    drop(eth);
    assert_eq!(rig.host.last_applied().net.ip, IP);
    assert!(rig.host.state().net_trial.is_empty());
    let e = rig.host.first(EventCode::NetTrialReverted);
    assert_eq!((e.arg1, e.arg2), (3, 0));
    assert_eq!(text(&e), b"192.168.1.20");
    assert!(rig.host.restarts().is_empty());
    assert!(!rig.shared.trial_info().active);
    let j = rig.dev.journal.entries();
    assert!(j.iter().any(|e| e == "storage.applyConfig"));
}

#[test]
fn trial_a_running_record_whose_revert_cannot_be_stored_stays() {
    let rig = Rig::new();
    rig.host.state().apply_result = false;
    let mut net = rig.net();
    let old = static_config(IP);
    let mut n = static_config(NEW_IP);
    boot_with_record(&rig, &mut net, NetTrialState::Running, &old, &mut n);
    assert_eq!(n.net.ip, IP);
    assert!(!rig.host.state().net_trial.is_empty());
    assert_eq!(rig.host.first(EventCode::NetTrialReverted).arg2, -1);
}

#[test]
fn trial_a_change_whose_record_cannot_be_stored_is_reverted_at_once_no_restart() {
    let rig = Rig::new();
    let mut net = rig.net();
    let mut c = config();
    net.begin(&mut c);
    rig.host.state().save_net_trial_result = false;
    net.reconfigure(&static_config(NEW_IP));
    let s = rig.host.state();
    assert!(s.net_trial.is_empty());
    assert_eq!(s.net_trial_clears, 1);
    assert_eq!(s.applied.len(), 1);
    assert!(s.applied[0].net.dhcp);
    assert_eq!(s.applied[0].net.iface, NetInterface::Auto);
    drop(s);
    assert!(!rig.host.has(EventCode::NetTrialStarted));
    let e = rig.host.first(EventCode::NetTrialReverted);
    assert_eq!((e.arg1, e.arg2), (5, 0));
    assert_eq!(text(&e), b"dhcp");
    assert!(rig.host.restarts().is_empty());
    assert!(!rig.shared.trial_info().active);
}

#[test]
fn trial_an_unstored_change_keeps_the_restart_of_a_new_station_name_a_failed_revert_is_minus_1() {
    let rig = Rig::new();
    let mut net = rig.net();
    let mut c = static_config(LONG_IP);
    net.begin(&mut c);
    rig.host.state().save_net_trial_result = false;
    rig.host.state().apply_result = false;
    let mut n = static_config(NEW_IP);
    copy_string(&mut n.station, b"Other");
    net.reconfigure(&n);
    let s = rig.host.state();
    assert_eq!(s.applied.len(), 1);
    assert_eq!(s.applied[0].net.ip, LONG_IP);
    assert_eq!(&s.applied[0].station[..], b"Other");
    drop(s);
    let e = rig.host.first(EventCode::NetTrialReverted);
    assert_eq!((e.arg1, e.arg2), (5, -1));
    assert_eq!(text(&e), b"192.168.100.200"); // 15 characters
    assert_eq!(rig.host.restarts().len(), 1);
    assert_eq!(rig.host.restarts()[0].reason, 0);
}

#[test]
fn trial_an_unstored_change_without_memory_for_the_copy_is_minus_1_the_settings_in_use_stay() {
    let rig = Rig::new();
    let mut net = rig.net();
    let mut c = config();
    net.begin(&mut c);
    rig.host.state().save_net_trial_result = false;
    rig.dev.heap.state().next.push_back(false);
    let n = static_config(NEW_IP);
    net.reconfigure(&n);
    assert!(rig.host.state().applied.is_empty());
    assert_eq!(rig.host.state().net_trial_clears, 1);
    let e = rig.host.first(EventCode::NetTrialReverted);
    assert_eq!((e.arg1, e.arg2), (5, -1));
    assert_eq!(text(&e), b"dhcp");
    assert!(rig.host.restarts().is_empty());
    // net goes on with the settings in use: the same config again needs a trial
    rig.host.state().save_net_trial_result = true;
    net.reconfigure(&n);
    assert!(rig.host.has(EventCode::NetTrialStarted));
}

#[test]
fn trial_a_second_change_whose_record_cannot_be_stored_goes_back_to_the_settings_in_use() {
    let rig = Rig::new();
    let mut net = rig.net();
    let mut c = config();
    net.begin(&mut c);
    net.reconfigure(&static_config(NEW_IP)); // armed, restart requested
    assert_eq!(rig.host.stored_record().state, NetTrialState::Armed);
    rig.host.state().save_net_trial_result = false;
    net.reconfigure(&static_config(ip(192, 168, 1, 60)));
    let s = rig.host.state();
    assert!(s.net_trial.is_empty()); // the first change goes as well
    assert_eq!(s.applied.len(), 1);
    assert!(s.applied[0].net.dhcp);
    drop(s);
    assert_eq!(rig.host.with_code(EventCode::NetTrialStarted).len(), 1);
    assert_eq!(rig.host.first(EventCode::NetTrialReverted).arg1, 5);
    assert_eq!(rig.host.restarts().len(), 1); // the one of the first change
}

#[test]
fn trial_an_armed_record_that_cannot_become_running_reverts_at_boot() {
    let rig = Rig::new();
    let mut net = rig.net();
    let old = static_config(IP);
    let mut n = static_config(NEW_IP);
    rig.host.state().save_net_trial_result = false;
    boot_with_record(&rig, &mut net, NetTrialState::Armed, &old, &mut n);
    assert_eq!(n.net.ip, IP); // the caller's config is reverted
    let eth = rig.dev.eth.state();
    assert_eq!(eth.begins.len(), 1);
    assert_eq!(eth.begins[0].fixed.map(|f| f.ip), Some(IP));
    drop(eth);
    assert_eq!(rig.host.last_applied().net.ip, IP);
    assert!(rig.host.state().net_trial.is_empty());
    let e = rig.host.first(EventCode::NetTrialReverted);
    assert_eq!((e.arg1, e.arg2), (5, 0));
    assert_eq!(text(&e), b"192.168.1.20");
    assert!(!rig.shared.trial_info().active);
    assert!(rig.host.restarts().is_empty());
}

#[test]
fn trial_an_armed_record_that_cannot_become_running_and_a_revert_that_cannot_be_stored() {
    let rig = Rig::new();
    let mut net = rig.net();
    let old = static_config(IP);
    let mut n = static_config(NEW_IP);
    rig.host.state().save_net_trial_result = false;
    rig.host.state().apply_result = false;
    boot_with_record(&rig, &mut net, NetTrialState::Armed, &old, &mut n);
    assert_eq!(n.net.ip, IP);
    assert_eq!(rig.host.stored_record().state, NetTrialState::Armed); // the next boot tries again
    assert_eq!(rig.host.first(EventCode::NetTrialReverted).arg2, -1);
    assert!(!rig.shared.trial_info().active);
}

#[test]
fn trial_a_stale_or_undecodable_record_is_erased_without_action() {
    {
        let rig = Rig::new();
        let mut net = rig.net();
        let old = static_config(IP);
        let mut n = static_config(NEW_IP);
        let other = static_config(ip(10, 0, 0, 5));
        let r = NetTrialRecord {
            state: NetTrialState::Running,
            previous: old.net.clone(),
            trial_crc: net_trial_fields_crc(&other.net),
        };
        rig.host.state().net_trial = blob(&r);
        net.begin(&mut n);
        assert_eq!(n.net.ip, NEW_IP);
        let s = rig.host.state();
        assert!(s.net_trial.is_empty());
        assert_eq!(s.net_trial_clears, 1);
        assert!(s.applied.is_empty());
        drop(s);
        assert!(!rig.host.has(EventCode::NetTrialReverted));
    }
    let rig = Rig::new();
    let mut net = rig.net();
    rig.host.state().net_trial = vec![1, 2, 3];
    let mut m = static_config(NEW_IP);
    net.begin(&mut m);
    assert!(rig.host.state().net_trial.is_empty());
    assert_eq!(rig.host.state().net_trial_clears, 1);
    assert!(!rig.shared.trial_info().active);
}

#[test]
fn time_valid_only_after_2020_local_time_follows_tz() {
    let rig = Rig::new();
    let mut net = rig.net();
    let mut c = config();
    copy_string(&mut c.time.tz_posix, b"UTC0");
    net.begin(&mut c);
    assert!(!time_valid(&rig.dev.wall));
    assert!(!local_time(&rig.dev.wall).valid);
    sync_time(&rig, 1_790_136_000); // 2026-09-23 04:00:00 UTC, a Wednesday
    assert!(time_valid(&rig.dev.wall));
    net.service(0, false); // the sync epoch reaches NetShared with the next pass
    assert_eq!(rig.shared.last_sync_epoch(), 1_790_136_000);
    let t = local_time(&rig.dev.wall);
    assert!(t.valid);
    assert_eq!(
        (t.year, t.month, t.mday, t.wday, t.hour),
        (2026, 9, 23, 3, 4)
    );
    assert_eq!(t.epoch, 1_790_136_000);
}

#[test]
fn time_the_first_sync_is_info_later_ones_debug_with_the_clock_step() {
    let rig = Rig::new();
    let mut net = rig.net();
    let mut c = config();
    net.begin(&mut c);
    net.service(1000, false);
    rig.dev.clock.advance_ms(1000);
    sync_time(&rig, 1_790_136_000);
    net.service(2000, false);
    let ev = rig.host.with_code(EventCode::TimeSynced);
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].severity, Severity::Info);
    assert_eq!(ev[0].arg1, 1_790_136_000 - 1); // expected: 1 s after the reference 0
    sync_time(&rig, 1_790_136_010);
    net.service(3000, false);
    let ev = rig.host.with_code(EventCode::TimeSynced);
    assert_eq!(ev.len(), 2);
    assert_eq!(ev[1].severity, Severity::Debug);
    assert_eq!(ev[1].arg1, 9);
}
