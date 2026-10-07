//! Port of `test/native/glue/test_net_edges.cpp`: boundaries and state transitions of the
//! interfaces, the WiFi back-off, the clock, the second-granular event arguments and the RTC
//! restart count; then the port forms (the WiFi retry of D§16, a station that cannot be built,
//! the RTC record bounds, the clamped clock step).
#![allow(clippy::large_stack_frames)]

use super::support::*;
use super::*;
use crate::testkit::{FakeBoard, Reset};
use vdm_esp_core::net_trial::NetTrialState;

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

fn info(addr: u32, gateway: u32) -> IpInfo {
    IpInfo {
        ip: addr,
        mask: MASK,
        gateway,
        dns: 0,
    }
}

// ---------------------------------------------------------------- interfaces

#[test]
fn a_static_address_needs_the_ethernet_link() {
    let rig = Rig::new();
    let mut net = rig.net();
    let mut c = static_config(IP);
    net.begin(&mut c);
    rig.dev.eth.state().info = info(IP, 0); // the driver has the address, the link is not there
    net.service(1000, false);
    assert!(!rig.shared.is_up());
    rig.dev.eth.state().link = true; // CONNECTED
    net.service(2000, false);
    assert!(rig.shared.is_up());
    rig.dev.eth.link_down(); // DISCONNECTED
    net.service(3000, false);
    assert!(!rig.shared.is_up());
    rig.dev.eth.state().link = true;
    net.service(4000, false);
    assert!(rig.shared.is_up());
    rig.dev.eth.link_down(); // STOP
    net.service(5000, false);
    assert!(!rig.shared.is_up());
}

#[test]
fn with_dhcp_the_link_is_up_only_with_a_lease_also_after_a_link_loss() {
    let rig = Rig::new();
    let mut net = rig.net();
    let mut c = config();
    net.begin(&mut c);
    rig.dev.eth.state().info = info(IP, 0);
    rig.dev.eth.state().link = true;
    net.service(1000, false);
    assert!(!rig.shared.is_up());
    rig.dev.eth.got_ip(info(IP, 0));
    net.service(2000, false);
    assert!(rig.shared.is_up());
    rig.dev.eth.link_down();
    rig.dev.eth.state().link = true;
    net.service(3000, false);
    assert!(!rig.shared.is_up()); // the old lease does not count
    rig.dev.eth.got_ip(info(IP, 0));
    net.service(4000, false);
    assert!(rig.shared.is_up());
    rig.dev.eth.link_down(); // STOP
    rig.dev.eth.state().link = true;
    net.service(5000, false);
    assert!(!rig.shared.is_up());
}

#[test]
fn an_interface_without_an_address_is_down() {
    let rig = Rig::new();
    let mut net = rig.net();
    let mut c = config();
    net.begin(&mut c);
    rig.ethernet_up(0, 0);
    net.service(1000, false);
    assert!(!rig.shared.is_up());
    assert_eq!(rig.shared.info().state, NetState::Down);
    assert!(!rig.host.has(EventCode::NetUp));
}

#[test]
fn a_new_address_on_the_same_interface_is_a_new_net_up_down_is_no_reconnect() {
    let rig = Rig::new();
    let mut net = rig.net();
    let mut c = config();
    net.begin(&mut c);
    rig.ethernet_up(IP, 0);
    net.service(1000, false);
    net.service(2000, false);
    assert_eq!(rig.shared.info().reconnects, 1);
    assert_eq!(rig.shared.info().up_since_ms, 1000);
    rig.dev.eth.got_ip(info(LONG_IP, 0));
    net.service(3000, false);
    let i = rig.shared.info();
    assert_eq!(i.ip, LONG_IP);
    assert_eq!((i.reconnects, i.up_since_ms), (2, 3000));
    let up = rig.host.with_code(EventCode::NetUp);
    assert_eq!(up.len(), 2);
    assert_eq!(&up[1].text[..], b"192.168.100.200");
    assert_eq!(up[1].arg2, 0);
    rig.dev.eth.link_down();
    net.service(4000, false);
    let i = rig.shared.info();
    assert_eq!(i.state, NetState::Down);
    assert_eq!((i.reconnects, i.up_since_ms), (2, 4000));
}

#[test]
fn wifi_with_its_address_rssi_mac_and_a_dhcp_lease_as_evidence() {
    let rig = Rig::new();
    let mut net = rig.net();
    let mut c = wifi_config(b"home");
    net.begin(&mut c);
    rig.dev.wifi.state().rssi = -60;
    rig.dev.wifi.got_ip(info(LONG_IP, 0));
    rig.tick(&mut net, 1000, false);
    let i = rig.shared.info();
    assert_eq!(i.state, NetState::Wifi);
    assert_eq!((i.ip, i.mask, i.rssi), (LONG_IP, MASK, -60));
    assert_eq!(&i.mac[..], b"24:0A:C4:12:34:56");
    let e = rig.host.first(EventCode::NetUp);
    assert_eq!((e.arg1, e.arg2), (NetState::Wifi as i32, 0));
    assert_eq!(&e.text[..], b"192.168.100.200");
    assert_eq!(rig.shared.health().evidence, NetEvidence::DhcpLease);
    rig.dev.wifi.state().rssi = -128;
    rig.tick(&mut net, 2000, false);
    assert_eq!(rig.shared.info().rssi, -128);
    rig.dev.wifi.state().rssi = 1; // no valid reading
    rig.tick(&mut net, 3000, false);
    assert_eq!(rig.shared.info().rssi, 0);
    rig.dev.wifi.state().rssi = 5;
    rig.tick(&mut net, 4000, false);
    assert_eq!(rig.shared.info().rssi, 0);
    // STA_DISCONNECTED (not requested), STA_LOST_IP, STA_STOP
    let downs: [fn(&Rig); 3] = [
        |r| r.dev.wifi.drop_station(),
        |r| r.dev.wifi.state().up = false,
        |r| r.dev.wifi.state().up = false,
    ];
    let mut t = 5000;
    for down in downs {
        down(&rig);
        rig.tick(&mut net, t, false);
        assert_eq!(rig.shared.info().state, NetState::Down);
        rig.dev.wifi.got_ip(info(LONG_IP, 0));
        rig.tick(&mut net, t + 1000, false);
        assert_eq!(rig.shared.info().state, NetState::Wifi);
        t += 2000;
    }
}

#[test]
fn a_one_character_ssid_is_a_wifi_network() {
    let rig = Rig::new();
    let mut net = rig.net();
    let mut c = wifi_config(b"a");
    net.begin(&mut c);
    assert_eq!(rig.dev.wifi.state().connects, vec![(b"a".to_vec(), vec![])]);
}

#[test]
fn a_static_address_on_wifi_is_configured_on_the_wifi_interface() {
    let rig = Rig::new();
    let mut net = rig.net();
    let mut c = wifi_config(b"home");
    c.net.dhcp = false;
    c.net.ip = IP;
    c.net.gateway = GW;
    c.net.mask = MASK;
    net.begin(&mut c);
    assert!(rig.dev.eth.state().begins.is_empty());
    let w = rig.dev.wifi.state();
    assert_eq!(w.begins.len(), 1);
    assert_eq!(
        w.begins[0].fixed,
        Some(IpInfo {
            ip: IP,
            mask: MASK,
            gateway: GW,
            dns: GW
        })
    );
}

#[test]
fn wifi_retries_back_off_from_5_s_doubling_up_to_60_s() {
    let rig = Rig::new();
    let mut net = rig.net();
    let mut c = wifi_config(b"home");
    net.begin(&mut c);
    assert_eq!(rig.dev.wifi.state().connects.len(), 1);
    let steps: [(u32, usize); 13] = [
        (0, 2),
        (4999, 2),
        (5000, 3),
        (14_999, 3),
        (15_000, 4),
        (34_999, 4),
        (35_000, 5),
        (74_999, 5),
        (75_000, 6),
        (134_999, 6),
        (135_000, 7),
        (194_999, 7),
        (195_000, 8),
    ];
    for (t, n) in steps {
        net.service(t, false);
        assert_eq!(rig.dev.wifi.state().connects.len(), n, "t = {t}");
    }
    assert_eq!(rig.dev.wifi.state().begins.len(), 1); // built once
}

#[test]
fn auto_fallback_to_wifi_and_back_to_ethernet() {
    let rig = Rig::new();
    let mut net = rig.net();
    let mut c = config();
    copy_string(&mut c.net.ssid, b"home");
    net.begin(&mut c);
    net.service(0, false);
    net.service(30_000, false);
    assert_eq!(rig.dev.wifi.state().connects.len(), 1);
    net.service(31_000, false);
    assert_eq!(rig.dev.wifi.state().stops, 0); // WiFi stays while Ethernet is down
    rig.dev.wifi.got_ip(info(NEW_IP, 0));
    net.service(32_000, false);
    assert_eq!(rig.shared.info().state, NetState::Wifi);
    rig.ethernet_up(IP, 0);
    net.service(40_000, false);
    assert_eq!(rig.shared.info().state, NetState::Ethernet);
    // the station is stopped and its driver freed (WIFI_OFF)
    assert_eq!(rig.dev.wifi.state().stops, 1);

    // Ethernet lost 60 s later: down (WiFi is off), WiFi again 30 s after the loss
    rig.dev.eth.link_down();
    net.service(100_000, false);
    assert_eq!(rig.shared.info().state, NetState::Down);
    net.service(129_999, false);
    assert_eq!(rig.dev.wifi.state().connects.len(), 1);
    net.service(130_000, false);
    assert_eq!(rig.dev.wifi.state().connects.len(), 2);
    // STA, OFF, STA: built, stopped, built again
    assert_eq!(rig.dev.wifi.state().begins.len(), 2);
    assert_eq!(rig.dev.wifi.state().stops, 1);
}

#[test]
fn the_auto_fallback_counts_from_the_first_pass_without_ethernet() {
    let rig = Rig::new();
    let mut net = rig.net();
    let mut c = config();
    copy_string(&mut c.net.ssid, b"home");
    net.begin(&mut c);
    net.service(10_000, false);
    net.service(39_999, false);
    assert!(rig.dev.wifi.state().connects.is_empty());
    net.service(40_000, false);
    assert_eq!(rig.dev.wifi.state().connects.len(), 1);
}

#[test]
fn auto_with_an_ethernet_driver_that_failed_starts_wifi_at_once() {
    let rig = Rig::new();
    rig.dev.eth.state().begin_ok = false;
    let mut net = rig.net();
    let mut c = config();
    copy_string(&mut c.net.ssid, b"home");
    net.begin(&mut c);
    let e = rig.host.first(EventCode::NetDown);
    assert_eq!((e.arg1, e.arg2), (1, 0));
    assert!(rig.dev.wifi.state().connects.is_empty());
    net.service(0, false);
    assert_eq!(rig.dev.wifi.state().connects.len(), 1);
}

#[test]
fn a_station_name_of_the_maximum_length_is_the_whole_host_name() {
    let rig = Rig::new();
    let mut net = rig.net();
    let mut c = config();
    copy_string(&mut c.station, b"ABCDEFGHIJKLMNOPQRST");
    assert_eq!(c.station.len(), STATION_NAME_MAX);
    net.begin(&mut c);
    assert_eq!(net.hostname(), "ABCDEFGHIJKLMNOPQRST");
}

// ---------------------------------------------------------------- time

#[test]
fn time_valid_from_2020_01_01_00_00_00_utc() {
    let rig = Rig::new();
    let mut net = rig.net();
    let mut c = config();
    copy_string(&mut c.time.tz_posix, b"UTC0");
    net.begin(&mut c);
    rig.dev.wall.set(1_577_836_799);
    assert!(!time_valid(&rig.dev.wall));
    assert!(!local_time(&rig.dev.wall).valid);
    rig.dev.wall.set(1_577_836_800);
    assert!(time_valid(&rig.dev.wall));
    let t = local_time(&rig.dev.wall);
    assert!(t.valid);
    assert_eq!((t.year, t.epoch), (2020, 1_577_836_800));
    assert_eq!(MIN_VALID_EPOCH, 1_577_836_800);
}

#[test]
fn time_the_last_sync_epoch_is_0_before_a_sync() {
    // C++ also called the SNTP callback with a null time and a negative one (0 both): the
    // adapter's callback gets a Duration since 1970, never null or negative
    let rig = Rig::new();
    let mut net = rig.net();
    let mut c = config();
    net.begin(&mut c);
    assert_eq!(rig.shared.last_sync_epoch(), 0);
    rig.dev.sntp.sync(1);
    net.service(1000, false);
    assert_eq!(rig.shared.last_sync_epoch(), 1);
    rig.dev.sntp.sync(1_790_136_000);
    assert_eq!(rig.shared.last_sync_epoch(), 1); // published by the next pass
    net.service(2000, false);
    assert_eq!(rig.shared.last_sync_epoch(), 1_790_136_000);
}

#[test]
fn time_without_an_ntp_server_the_tz_string_replaces_the_environments() {
    let rig = Rig::new();
    rig.dev.wall.set_time_zone("EST5");
    let mut net = rig.net();
    let mut c = config();
    c.time.ntp_server.clear();
    copy_string(&mut c.time.tz_posix, b"UTC0");
    net.begin(&mut c);
    assert_eq!(rig.dev.wall.zones(), vec!["EST5", "UTC0"]);
    rig.dev.wall.set(1_790_136_000);
    assert_eq!(local_time(&rig.dev.wall).hour, 4);
}

#[test]
fn time_a_clock_that_kept_time_for_999_s_steps_by_0() {
    let rig = Rig::new();
    let mut net = rig.net();
    let mut c = config();
    net.begin(&mut c);
    rig.dev.clock.set_ms(1000);
    rig.dev.wall.set(1_790_136_000);
    rig.dev.sntp.sync(1_790_136_000);
    net.service(1000, false);
    rig.dev.clock.set_ms(1_000_000);
    rig.dev.wall.set(1_790_136_999);
    rig.dev.sntp.sync(1_790_136_999);
    net.service(1_000_000, false);
    let ev = rig.host.with_code(EventCode::TimeSynced);
    assert_eq!(ev.len(), 2);
    assert_eq!(ev[1].arg1, 0);
}

#[test]
fn reconfigure_the_same_time_settings_are_not_applied_again() {
    let rig = Rig::new();
    let mut net = rig.net();
    let mut c = config();
    net.begin(&mut c);
    assert_eq!(rig.dev.wall.zones().len(), 1);
    net.reconfigure(&c);
    assert_eq!(rig.dev.wall.zones().len(), 1);
    assert_eq!(rig.dev.sntp.state().configured.len(), 1);
    assert!(rig.host.restarts().is_empty());
}

// ---------------------------------------------------------------- reachability

#[test]
fn reachability_a_new_gateway_gets_a_new_ping_session() {
    let rig = Rig::new();
    let mut net = rig.net();
    let mut c = config();
    net.begin(&mut c);
    rig.ethernet_up(IP, GW);
    rig.tick(&mut net, 1000, false);
    assert_eq!(rig.dev.pinger.state().started.len(), 1);
    let gw2 = ip(192, 168, 1, 254);
    rig.dev.eth.got_ip(info(IP, gw2)); // lease renewed with another gateway
    rig.tick(&mut net, 2000, false);
    let p = rig.dev.pinger.state();
    // the first session went before the second started (one session at a time)
    assert_eq!(p.started, vec![GW, gw2]);
    assert_eq!(p.deletes, 1);
}

#[test]
fn reachability_a_probe_due_while_the_previous_one_runs_waits_for_its_report() {
    let rig = Rig::new();
    let mut net = rig.net();
    let mut c = config();
    net.begin(&mut c);
    rig.ethernet_up(IP, GW);
    rig.service(&mut net, 1000); // the probe is sent, the ping task has not answered yet
    assert_eq!(rig.dev.pinger.state().started.len(), 1);
    let gw2 = ip(192, 168, 1, 254);
    rig.dev.eth.got_ip(info(IP, gw2)); // the new gateway makes the next probe due
    rig.service(&mut net, 2000);
    assert_eq!(rig.dev.pinger.state().started.len(), 1); // no second session meanwhile
    assert_eq!(rig.dev.pinger.state().deletes, 0);
    rig.dev.pinger.step(); // the reply of the first probe
    rig.tick(&mut net, 3000, false);
    let p = rig.dev.pinger.state();
    assert_eq!(p.started, vec![GW, gw2]);
    assert_eq!(p.deletes, 1);
    drop(p);
    // the old reply counts
    assert_eq!(rig.shared.health().evidence, NetEvidence::GatewayPing);
}

#[test]
fn reachability_a_probe_that_times_out_goes_in_the_next_pass_like_one_with_a_reply() {
    let rig = Rig::new();
    let mut net = rig.net();
    let mut c = config();
    net.begin(&mut c);
    rig.ethernet_up(IP, GW);
    rig.dev.pinger.state().answers.push_back(false);
    rig.tick(&mut net, 1000, false); // the probe times out
    assert_eq!(rig.dev.pinger.state().started.len(), 1);
    assert_eq!(rig.dev.pinger.state().deletes, 0);
    rig.tick(&mut net, 2000, false);
    assert_eq!(rig.dev.pinger.state().deletes, 1);
    // a timeout is no evidence
    assert_eq!(rig.shared.health().evidence, NetEvidence::DhcpLease);
}

#[test]
fn reachability_a_session_without_a_report_is_dropped_after_10_s() {
    let rig = Rig::new();
    let mut net = rig.net();
    let mut c = config();
    net.begin(&mut c);
    rig.ethernet_up(IP, GW);
    for t in (1000..=10_000).step_by(1000) {
        rig.service(&mut net, t); // the ping task never reports
    }
    rig.service(&mut net, 10_999);
    assert_eq!(rig.dev.pinger.state().started.len(), 1);
    assert_eq!(rig.dev.pinger.state().deletes, 0);
    rig.service(&mut net, 11_000);
    assert_eq!(rig.dev.pinger.state().deletes, 1);
    rig.tick(&mut net, 60_000, false);
    assert_eq!(rig.dev.pinger.state().started.len(), 1); // the next probe keeps its cadence
    rig.tick(&mut net, 61_000, false);
    assert_eq!(rig.dev.pinger.state().started.len(), 2);
    assert_eq!(PING_SESSION_MAX_MS, 10_000);
}

#[test]
fn reachability_a_session_that_does_not_start_goes_at_once_the_next_probe_tries_again() {
    let rig = Rig::new();
    let mut net = rig.net();
    let mut c = config();
    net.begin(&mut c);
    rig.ethernet_up(IP, GW);
    rig.dev.pinger.state().start_ok = false;
    rig.tick(&mut net, 1000, false);
    assert_eq!(rig.dev.pinger.state().started, vec![GW]);
    assert_eq!(rig.dev.pinger.state().deletes, 1);
    rig.tick(&mut net, 2000, false);
    assert_eq!(rig.dev.pinger.state().started.len(), 1); // the failed probe counts as sent
    rig.dev.pinger.state().start_ok = true;
    rig.tick(&mut net, 61_000, false);
    assert_eq!(rig.dev.pinger.state().started.len(), 2);
    rig.tick(&mut net, 62_000, false);
    assert_eq!(rig.shared.health().evidence, NetEvidence::GatewayPing);
    assert_eq!(rig.dev.pinger.state().deletes, 2);
}

#[test]
fn reachability_a_session_that_cannot_be_created_is_tried_again_at_the_next_probe() {
    // C++ told a session that could not be created (nothing to delete) from one that did not
    // start; the port has one start call, and a failed one is deleted (no-op without a session)
    let rig = Rig::new();
    let mut net = rig.net();
    let mut c = config();
    net.begin(&mut c);
    rig.ethernet_up(IP, GW);
    rig.dev.pinger.state().start_ok = false;
    rig.tick(&mut net, 1000, false);
    rig.tick(&mut net, 2000, false);
    assert_eq!(rig.dev.pinger.state().started.len(), 1);
    rig.dev.pinger.state().start_ok = true;
    rig.tick(&mut net, 60_000, false);
    assert_eq!(rig.dev.pinger.state().started.len(), 1);
    rig.tick(&mut net, 61_000, false);
    assert_eq!(rig.dev.pinger.state().started.len(), 2);
    rig.tick(&mut net, 62_000, false);
    assert_eq!(rig.shared.health().evidence, NetEvidence::GatewayPing);
}

#[test]
fn reachability_the_evidence_age_in_whole_seconds() {
    let rig = Rig::new();
    let mut net = rig.net();
    let mut c = config();
    net.begin(&mut c);
    rig.ethernet_up(IP, 0);
    rig.tick(&mut net, 1000, false); // the DHCP lease
    rig.tick(&mut net, 2998, false);
    assert_eq!(rig.shared.health().evidence_age_s, 1);
}

#[test]
fn reachability_no_lease_evidence_without_a_got_ip() {
    let rig = Rig::new();
    let mut net = rig.net();
    let mut c = static_config(IP);
    net.begin(&mut c);
    rig.dev.eth.state().info = info(IP, 0);
    rig.dev.eth.state().link = true; // CONNECTED
    let mut d = c.clone(); // DHCP saved, the restart not yet done
    d.net.dhcp = true;
    net.reconfigure(&d);
    rig.tick(&mut net, 1000, false);
    assert!(rig.shared.health().ip_up);
    assert_eq!(rig.shared.health().evidence, NetEvidence::None);
}

#[test]
fn reachability_lost_and_regained_name_whole_seconds() {
    let rig = Rig::new();
    let mut net = rig.net();
    let mut c = static_config(IP);
    net.begin(&mut c);
    rig.ethernet_up(IP, GW);
    rig.tick(&mut net, 1000, false);
    rig.tick(&mut net, 2000, false); // armed, evidence at 2 s
    rig.dev.pinger.state().default_answer = false;
    rig.tick(&mut net, 152_850, false);
    let lost = rig.host.first(EventCode::NetUnreachable);
    assert_eq!(lost.arg1, 150);
    assert_eq!(lost.arg2, NetEvidence::GatewayPing as i32);
    rig.tick(&mut net, 243_800, true); // MQTT back 90.95 s later
    assert_eq!(rig.host.first(EventCode::NetReachable).arg1, 90);
}

#[test]
fn reachability_lost_without_evidence_since_the_ip_came_back_has_no_age() {
    let rig = Rig::new();
    let mut net = rig.net();
    let mut c = static_config(IP);
    net.begin(&mut c);
    rig.ethernet_up(IP, GW);
    rig.tick(&mut net, 1000, false);
    rig.tick(&mut net, 2000, false); // armed
    rig.dev.pinger.state().default_answer = false;
    rig.dev.eth.link_down();
    rig.tick(&mut net, 3000, false);
    rig.dev.eth.state().link = true;
    rig.tick(&mut net, 4000, false); // up again with the same gateway: armed, no evidence
    rig.tick(&mut net, 153_000, false);
    assert!(!rig.host.has(EventCode::NetUnreachable));
    rig.tick(&mut net, 154_000, false);
    let lost = rig.host.first(EventCode::NetUnreachable);
    assert_eq!(lost.arg1, -1);
    assert_eq!(lost.arg2, NetEvidence::None as i32);
}

// ---------------------------------------------------------------- watchdog

#[test]
fn watchdog_the_interface_restart_names_whole_seconds() {
    // C++ also counted the 200 ms between esp_eth_stop and esp_eth_start: the adapter's wait
    let rig = Rig::new();
    let mut net = rig.net();
    let mut c = config();
    net.begin(&mut c);
    rig.dev.eth.state().link = true; // link, no lease
    rig.tick(&mut net, 0, false);
    rig.tick(&mut net, 300_700, false);
    assert_eq!(rig.dev.eth.state().restarts, 1);
    assert_eq!(rig.host.first(EventCode::NetInterfaceRestart).arg1, 300);
}

#[test]
fn watchdog_the_esp_restart_names_whole_minutes_of_the_outage() {
    let rig = Rig::new();
    let mut net = rig.net();
    let mut c = config();
    net.begin(&mut c);
    rig.tick(&mut net, 0, false);
    rig.tick(&mut net, 659_990, false);
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
fn watchdog_a_restart_count_without_the_rtc_check_word_is_ignored() {
    let board = FakeBoard::new();
    let mut c = config();
    c.net.reconnect_timeout_min = 5;
    {
        let rig = Rig::boot(&board, None);
        let mut net = rig.net();
        net.begin(&mut c.clone());
        rig.run(&mut net, 0, 600_000, 1000);
        assert_eq!(rig.host.restarts().len(), 1);
        assert_eq!(rig.rtc_record(), (MAGIC, 1));
        rig.set_rtc_record(0, 1); // count 1 stays, the check word is gone
        board.reset(Reset::Software);
    }
    let rig = Rig::boot(&board, None);
    let mut net = rig.net();
    net.begin(&mut c);
    rig.run(&mut net, 0, 600_000, 1000);
    assert_eq!(rig.host.restarts().len(), 1);
    assert_eq!(rig.dev.clock.ms(), 600_000); // 5 + 5 min: no earlier restart counted
}

#[test]
fn watchdog_255_restarts_in_the_rtc_word_are_taken_the_wait_is_capped() {
    let board = FakeBoard::new();
    let mut c = config();
    c.net.reconnect_timeout_min = 5;
    {
        let rig = Rig::boot(&board, None);
        let mut net = rig.net();
        net.begin(&mut c.clone());
        rig.run(&mut net, 0, 600_000, 1000);
        assert_eq!(rig.host.restarts().len(), 1);
        rig.set_rtc_record(MAGIC, 255);
        board.reset(Reset::Software);
    }
    let rig = Rig::boot(&board, None);
    let mut net = rig.net();
    net.begin(&mut c);
    rig.run(&mut net, 0, 900_000, 10_000);
    assert!(rig.host.restarts().is_empty()); // 5 min + 24 h
    assert_eq!(rig.shared.health().iface_restarts, 1);
    assert_eq!(rig.rtc_record(), (MAGIC, 255));
}

// ---------------------------------------------------------------- trial

#[test]
fn trial_the_remaining_time_in_whole_seconds() {
    let rig = Rig::new();
    let mut net = rig.net();
    let old = config();
    let mut n = static_config(NEW_IP);
    boot_with_record(&rig, &mut net, NetTrialState::Armed, &old, &mut n);
    rig.tick(&mut net, 120, false);
    assert_eq!(rig.shared.health().trial_remaining_s, 119);
    assert_eq!(rig.shared.trial_info().remain_s, 119);
}

#[test]
fn trial_confirm_names_the_whole_seconds_with_the_network() {
    let rig = Rig::new();
    let mut net = rig.net();
    let old = config();
    let mut n = static_config(NEW_IP);
    boot_with_record(&rig, &mut net, NetTrialState::Armed, &old, &mut n);
    rig.ethernet_up(NEW_IP, GW);
    rig.run(&mut net, 1000, 59_000, 1000); // the IP counted from 1 s
    assert!(rig.shared.request_trial_confirm());
    rig.tick(&mut net, 60_940, false);
    assert_eq!(rig.host.first(EventCode::NetTrialConfirmed).arg1, 59);
}

#[test]
fn trial_a_change_during_the_trial_names_the_whole_seconds_it_ran() {
    let rig = Rig::new();
    let mut net = rig.net();
    let old = config();
    let mut n = static_config(NEW_IP);
    boot_with_record(&rig, &mut net, NetTrialState::Armed, &old, &mut n);
    rig.ethernet_up(NEW_IP, GW);
    rig.run(&mut net, 1000, 31_000, 1000);
    rig.dev.clock.set_ms(31_970);
    net.reconfigure(&static_config(ip(192, 168, 1, 70)));
    let e = rig.host.first(EventCode::NetTrialConfirmed);
    assert_eq!((e.arg1, e.arg2), (30, 1));
}

#[test]
fn trial_the_longest_address_in_net_trial_started() {
    let rig = Rig::new();
    let mut net = rig.net();
    let mut c = config();
    net.begin(&mut c);
    net.reconfigure(&static_config(LONG_IP));
    let e = rig.host.first(EventCode::NetTrialStarted);
    assert_eq!((e.arg1, e.arg2), (120, 0));
    assert_eq!(&e.text[..], b"192.168.100.200");
}

#[test]
fn trial_a_revert_after_the_window_names_the_longest_previous_address() {
    let rig = Rig::new();
    let mut net = rig.net();
    let old = static_config(LONG_IP);
    let mut n = static_config(NEW_IP);
    boot_with_record(&rig, &mut net, NetTrialState::Armed, &old, &mut n);
    rig.run(&mut net, 1000, 120_000, 1000);
    assert_eq!(rig.host.restarts().len(), 1);
    assert_eq!(
        &rig.host.first(EventCode::NetTrialReverted).text[..],
        b"192.168.100.200"
    );
}

#[test]
fn trial_a_revert_at_boot_names_the_longest_previous_address() {
    let rig = Rig::new();
    let mut net = rig.net();
    let old = static_config(LONG_IP);
    let mut n = static_config(NEW_IP);
    boot_with_record(&rig, &mut net, NetTrialState::Running, &old, &mut n);
    assert_eq!(
        &rig.host.first(EventCode::NetTrialReverted).text[..],
        b"192.168.100.200"
    );
}

// ---------------------------------------------------------------- port forms (Rust only)

#[test]
fn wifi_one_retry_after_the_first_unrequested_disconnect_since_boot() {
    let rig = Rig::new();
    let mut net = rig.net();
    let mut c = wifi_config(b"home");
    net.begin(&mut c);
    rig.dev.wifi.got_ip(info(IP, 0));
    rig.tick(&mut net, 1000, false);
    assert_eq!(rig.dev.wifi.state().reconnects, 0);
    rig.dev.wifi.drop_station();
    rig.tick(&mut net, 2000, false);
    assert_eq!(rig.dev.wifi.state().reconnects, 1);
    rig.dev.wifi.got_ip(info(IP, 0));
    rig.tick(&mut net, 3000, false);
    rig.dev.wifi.drop_station();
    rig.tick(&mut net, 4000, false);
    assert_eq!(rig.dev.wifi.state().reconnects, 1); // once per boot
    assert_eq!(rig.dev.wifi.state().unrequested_disconnects, 0); // every report taken
}

#[test]
fn wifi_a_station_that_cannot_be_built_is_built_again_with_the_next_connect() {
    let rig = Rig::new();
    rig.dev.wifi.state().begin_ok = false;
    let mut net = rig.net();
    let mut c = wifi_config(b"home");
    net.begin(&mut c);
    assert_eq!(rig.dev.wifi.state().begins.len(), 1);
    assert!(rig.dev.wifi.state().connects.is_empty());
    net.service(0, false);
    assert_eq!(rig.dev.wifi.state().begins.len(), 2);
    assert!(rig.dev.wifi.state().connects.is_empty());
    rig.dev.wifi.state().begin_ok = true;
    net.service(5000, false);
    assert_eq!(rig.dev.wifi.state().begins.len(), 3);
    assert_eq!(rig.dev.wifi.state().connects.len(), 1);
    // a station that never started is not stopped, nor restarted by the watchdog
    net.service(10_000, false);
    assert_eq!(rig.dev.wifi.state().begins.len(), 3);
}

#[test]
fn watchdog_a_restart_count_above_255_in_the_rtc_record_is_ignored() {
    let board = FakeBoard::new();
    board.reset(Reset::Software); // a warm first boot: the RTC record is taken as it is
    let rig = Rig::boot(&board, None);
    rig.set_rtc_record(MAGIC, 256);
    let mut net = rig.net();
    let mut c = config();
    net.begin(&mut c);
    rig.run(&mut net, 0, 600_000, 1000);
    assert_eq!(rig.dev.clock.ms(), 600_000); // 5 + 5 min: no earlier restart counted
    assert_eq!(rig.rtc_record(), (MAGIC, 1));
}

#[test]
fn watchdog_the_rtc_record_is_written_on_changes_only_and_cleared_by_reachability() {
    let board = FakeBoard::new();
    let mut c = config();
    {
        let rig = Rig::boot(&board, None); // power-on: the RTC block is garbage
        let mut net = rig.net();
        net.begin(&mut c);
        rig.run(&mut net, 0, 599_000, 1000);
        assert_eq!(rig.rtc_record(), (0xA5A5_A5A5, 0xA5A5_A5A5)); // count 0: nothing stored
        rig.run(&mut net, 600_000, 600_000, 1000);
        assert_eq!(rig.rtc_record(), (MAGIC, 1));
        board.reset(Reset::Software);
    }
    let rig = Rig::boot(&board, None);
    let mut net = rig.net();
    net.begin(&mut c);
    rig.ethernet_up(IP, 0);
    rig.tick(&mut net, 1000, false);
    assert_eq!(rig.rtc_record(), (MAGIC, 0)); // reachable: the outage is over
}

#[test]
fn time_the_step_of_a_sync_is_clamped_to_int32() {
    let rig = Rig::new();
    let mut net = rig.net();
    let mut c = config();
    net.begin(&mut c);
    rig.dev.wall.set(5_000_000_000);
    rig.dev.sntp.sync(0);
    net.service(0, false);
    rig.dev.wall.set(0);
    rig.dev.sntp.sync(0);
    net.service(0, false);
    let ev = rig.host.with_code(EventCode::TimeSynced);
    assert_eq!(ev.len(), 2);
    assert_eq!((ev[0].arg1, ev[1].arg1), (i32::MAX, i32::MIN));
}

#[test]
fn shared_requests_and_the_inbound_filter() {
    let shared = NetShared::new();
    assert!(!shared.is_up());
    assert!(!shared.request_trial_confirm());
    assert!(!shared.request_trial_revert());
    assert!(!shared.confirm.load(Ordering::SeqCst));
    assert!(!shared.revert.load(Ordering::SeqCst));
    shared.note_inbound_http(ip(127, 1, 2, 3)); // the whole loopback network
    shared.note_inbound_http(0); // own address before an address is known
    assert_eq!(shared.inbound.load(Ordering::SeqCst), 0);
    shared.note_inbound_http(ip(128, 0, 0, 1));
    shared.note_inbound_http(ip(126, 0, 0, 1));
    assert_eq!(shared.inbound.load(Ordering::SeqCst), 2);
    lock(&shared.health).trial_active = true;
    assert!(shared.request_trial_confirm());
    assert!(shared.confirm.load(Ordering::SeqCst));
    assert!(!shared.revert.load(Ordering::SeqCst));
    assert!(shared.request_trial_revert());
    assert!(shared.revert.load(Ordering::SeqCst));
    let mut h = shared.health();
    h.reachable = true;
    *lock(&shared.health) = h;
    assert!(!shared.ota_net_ok()); // reachable, not proven
    h.proven = true;
    h.reachable = false;
    *lock(&shared.health) = h;
    assert!(!shared.ota_net_ok());
    h.reachable = true;
    *lock(&shared.health) = h;
    assert!(shared.ota_net_ok());
}

#[test]
fn constants_of_the_cpp_glue() {
    assert_eq!(
        (WIFI_FALLBACK_MS, WIFI_BACKOFF_MIN_MS, WIFI_BACKOFF_MAX_MS),
        (30_000, 5000, 60_000)
    );
    assert_eq!(RTC_MAGIC.to_be_bytes(), *b"VNWD");
    assert_eq!(RTC_RECORD_LEN, 8);
    assert_eq!(
        format_mac([0, 0x0F, 0xF0, 0xFF, 0x12, 0xAB]).as_slice(),
        b"00:0F:F0:FF:12:AB"
    );
}
