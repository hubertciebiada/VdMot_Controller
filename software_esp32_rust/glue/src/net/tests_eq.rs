//! Port of `test/native/glue/test_net__eq.cpp`: a switch of the interface at run time applies at
//! the restart that follows; the service passes before it run with the new settings.
#![allow(clippy::large_stack_frames)]

use super::support::*;
use super::*;

#[test]
fn wifi_lost_after_a_switch_from_wifi_to_auto_reconnects_at_once_before_the_restart() {
    let rig = Rig::new();
    let mut net = rig.net();
    let mut c = wifi_config(b"home");
    net.begin(&mut c); // Ethernet is not started with the interface WiFi
    assert_eq!(rig.dev.wifi.state().connects.len(), 1);
    rig.dev.wifi.got_ip(IpInfo {
        ip: IP,
        mask: MASK,
        gateway: 0,
        dns: 0,
    });
    rig.tick(&mut net, 1000, false);
    assert_eq!(rig.shared.info().state, NetState::Wifi);
    let mut auto = c.clone();
    auto.net.iface = NetInterface::Auto;
    net.reconfigure(&auto);
    let r = rig.host.restarts();
    assert_eq!(r.len(), 1); // the new interface needs a restart
    assert_eq!(r[0].delay_ms, 1500);
    rig.dev.wifi.drop_station();
    rig.tick(&mut net, 2000, false);
    // without a started Ethernet driver auto wants WiFi at once, not after 30 s
    assert_eq!(rig.dev.wifi.state().connects.len(), 2);
    assert!(rig.dev.eth.state().begins.is_empty());
    // and the disconnect was the first unrequested one since boot: Arduino's one retry
    assert_eq!(rig.dev.wifi.state().reconnects, 1);
}
