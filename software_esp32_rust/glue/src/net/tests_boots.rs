//! Multi-boot scenarios of net (new): the network trial across the restart that starts it and
//! the boot after it, and the watchdog's restart count in RTC memory across software restarts
//! and a power cycle. The host carries what storage keeps in NVS (the active config and the
//! trial record) from boot to boot; the board keeps the RTC block.
#![allow(clippy::large_stack_frames)]

use super::support::*;
use super::*;
use crate::testkit::{FakeBoard, Reset};
use vdm_esp_core::net_trial::NetTrialState;

/// The config storage holds (what app::setup hands to `begin`).
fn stored(rig: &Rig) -> Config {
    (*rig.host.state().active).clone()
}

/// Boot 1: DHCP in use, the user saves the static address NEW_IP (storage applies it, then
/// the app thread reconfigures net), the restart path ends with esp_restart. Returns the host
/// that holds the storage.
fn save_static_address(board: &FakeBoard) -> FakeNetHost {
    let rig = Rig::boot(board, None);
    *rig.host.state().active = config();
    let mut cfg = stored(&rig);
    let mut net = rig.net();
    net.begin(&mut cfg);
    rig.ethernet_up(IP, GW);
    rig.tick(&mut net, 1000, false);
    let n = static_config(NEW_IP);
    assert!(NetHost::apply_config(&mut rig.host.clone(), &n));
    net.reconfigure(&n);
    assert_eq!(
        rig.host.restarts(),
        vec![RestartRequest {
            reason: 0,
            delay_ms: 1500,
            detail: 0
        }]
    );
    assert_eq!(rig.host.stored_record().state, NetTrialState::Armed);
    rig.board.reset(Reset::Software);
    rig.host.clone()
}

/// Boot 2: the saved address runs on trial; `secs` passes with the network up from 1 s.
fn run_trial(board: &FakeBoard, storage: &FakeNetHost, until_ms: u64) -> Rig {
    let rig = Rig::boot(board, Some(storage));
    let mut cfg = stored(&rig);
    {
        let mut net = rig.net();
        net.begin(&mut cfg);
        assert_eq!(cfg.net.ip, NEW_IP);
        assert!(rig.shared.trial_info().active);
        assert_eq!(rig.host.stored_record().state, NetTrialState::Running);
        rig.ethernet_up(NEW_IP, GW);
        rig.run(&mut net, 1000, until_ms, 1000);
    }
    rig
}

#[test]
fn a_saved_address_runs_on_trial_after_the_restart_and_goes_back_without_a_confirmation() {
    let board = FakeBoard::new();
    let storage = save_static_address(&board);
    let rig = run_trial(&board, &storage, 200_000);
    // the network came up at 1 s: the window ends 120 s later
    assert_eq!(rig.dev.clock.ms(), 121_000);
    assert_eq!(
        rig.host.restarts(),
        vec![RestartRequest {
            reason: 5,
            delay_ms: 1000,
            detail: 0
        }]
    );
    assert_eq!(rig.host.first(EventCode::NetTrialReverted).arg1, 1);
    assert!(rig.host.state().net_trial.is_empty());
    rig.board.reset(Reset::Software);
    let storage = rig.host.clone();
    drop(rig);
    // boot 3: the previous settings are in use, no trial
    let rig = Rig::boot(&board, Some(&storage));
    let mut cfg = stored(&rig);
    let mut net = rig.net();
    net.begin(&mut cfg);
    assert!(cfg.net.dhcp);
    assert_eq!(cfg.net.iface, NetInterface::Auto);
    assert_eq!(rig.dev.eth.state().begins[0].fixed, None);
    assert!(!rig.shared.trial_info().active);
    assert!(rig.host.state().events.is_empty());
    assert_eq!(rig.host.state().net_trial_clears, 0);
}

#[test]
fn a_reset_during_the_trial_reverts_at_the_next_boot_before_the_interfaces_start() {
    let board = FakeBoard::new();
    let storage = save_static_address(&board);
    let rig = run_trial(&board, &storage, 30_000);
    assert!(rig.host.restarts().is_empty());
    rig.board.reset(Reset::Panic); // a crash in the middle of the trial
    let storage = rig.host.clone();
    drop(rig);
    let rig = Rig::boot(&board, Some(&storage));
    let mut cfg = stored(&rig);
    let mut net = rig.net();
    net.begin(&mut cfg);
    assert!(cfg.net.dhcp); // reverted before the interfaces started
    assert_eq!(rig.dev.eth.state().begins[0].fixed, None);
    let e = rig.host.first(EventCode::NetTrialReverted);
    assert_eq!((e.arg1, e.arg2), (3, 0));
    assert_eq!(&e.text[..], b"dhcp");
    assert!(rig.host.restarts().is_empty()); // no extra restart
    assert!(rig.host.state().net_trial.is_empty());
    assert!(stored(&rig).net.dhcp);
    assert!(!rig.shared.trial_info().active);
}

#[test]
fn a_confirmed_trial_keeps_the_new_settings_across_the_next_boot() {
    let board = FakeBoard::new();
    let storage = save_static_address(&board);
    let rig = Rig::boot(&board, Some(&storage));
    let mut cfg = stored(&rig);
    {
        let mut net = rig.net();
        net.begin(&mut cfg);
        rig.ethernet_up(NEW_IP, GW);
        rig.run(&mut net, 1000, 30_000, 1000);
        assert!(rig.shared.request_trial_confirm()); // POST /api/system/network/confirm
        rig.run(&mut net, 31_000, 300_000, 1000);
    }
    assert!(rig.host.restarts().is_empty());
    assert_eq!(rig.host.first(EventCode::NetTrialConfirmed).arg1, 30);
    assert!(rig.host.state().net_trial.is_empty());
    rig.board.reset(Reset::Software);
    let storage = rig.host.clone();
    drop(rig);
    let rig = Rig::boot(&board, Some(&storage));
    let mut cfg = stored(&rig);
    let mut net = rig.net();
    net.begin(&mut cfg);
    assert_eq!(cfg.net.ip, NEW_IP);
    assert_eq!(
        rig.dev.eth.state().begins[0].fixed.map(|f| f.ip),
        Some(NEW_IP)
    );
    assert!(!rig.shared.trial_info().active);
    assert!(rig.host.state().events.is_empty());
}

#[test]
fn the_watchdog_wait_grows_per_restart_of_one_outage_and_a_power_cycle_starts_over() {
    let board = FakeBoard::new();
    // no network at all: each boot ends with the watchdog's ESP restart
    for (minutes, count) in [(10, 1), (25, 2), (85, 3)] {
        let rig = Rig::boot(&board, None);
        let mut cfg = config();
        let mut net = rig.net();
        net.begin(&mut cfg);
        rig.run(&mut net, 0, 6_000_000, 10_000);
        assert_eq!(rig.dev.clock.ms(), minutes * 60_000);
        let r = rig.host.restarts();
        assert_eq!(r.len(), 1);
        assert_eq!((r[0].reason, r[0].detail), (2, minutes as i32));
        assert_eq!(rig.rtc_record(), (MAGIC, count));
        rig.board.reset(Reset::Software);
    }
    board.reset(Reset::PowerOn);
    let rig = Rig::boot(&board, None);
    let mut cfg = config();
    let mut net = rig.net();
    net.begin(&mut cfg);
    rig.run(&mut net, 0, 6_000_000, 10_000);
    assert_eq!(rig.dev.clock.ms(), 600_000);
    assert_eq!(rig.rtc_record(), (MAGIC, 1));
}
