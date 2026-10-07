//! C++ `test_mqtt_client__gate.cpp`: the memory of a discovery run and its start. The automatic
//! runs wait for settled STM inputs (DiscoveryGate, at most 120 s), a manual request runs at
//! once; the context and the buffers of a run, and the copy of a profile, live on the heap only
//! while they are used, and a run without memory waits for the next pass.
// host test code: the stack rule of the glue (design 2.4) is for the device
#![allow(clippy::large_stack_frames, clippy::large_stack_arrays)]

use super::rig::{s, Rig};
use super::*;

/// MQTT with HA discovery on broker.lan.
fn use_ha(rig: &Rig) {
    rig.use_mqtt(MqttMode::MqttHa);
}

/// The STM link up (protocol 3), its inputs not settled yet.
fn link_up(rig: &Rig) {
    rig.snap(|s| {
        s.link = LinkState::Up;
        s.proto = 3;
        s.valves[0].known = true;
    });
    rig.publish_snap();
}

fn runs(rig: &Rig) -> usize {
    rig.events(EventCode::HaDiscoverySent).len()
}

/// Heap blocks of discovery runs: the context and the payload + list buffer of each run.
fn run_blocks(rig: &Rig) -> (usize, usize) {
    let h = rig.dev.heap.state();
    let ctx = core::mem::size_of::<DiscoveryContext>();
    let bytes = DISCOVERY_PAYLOAD_MAX + 1 + LIST_BUFFER;
    (
        h.granted.iter().filter(|&&a| a == ctx).count(),
        h.granted.iter().filter(|&&a| a == bytes).count(),
    )
}

/// One pass every DISCOVERY_PACE_MS (20 ms).
const PASSES_PER_SECOND: usize = 50;

// ---------------------------------------------------------------- the gate

#[test]
fn gate_a_connect_before_the_stm_settled_waits_then_one_run() {
    let rig = Rig::new();
    let mut c = rig.client();
    use_ha(&rig);
    link_up(&rig);
    c.begin();
    rig.run(&mut c, 40);
    assert!(rig.session_up());
    assert!(!rig.shared.status().discovery_running);
    assert_eq!(rig.count_prefix("homeassistant/", 0), 0);
    // the STM start-up: sensor assignments come in step by step (each changes the inputs)
    for slot in 1..=3 {
        rig.snap(|s| s.valves[0].sensor_slot[0] = slot);
        rig.publish_snap();
        rig.run(&mut c, 5 * PASSES_PER_SECOND);
    }
    assert_eq!(rig.count_prefix("homeassistant/", 0), 0);
    assert_eq!(runs(&rig), 0);
    rig.snap(|s| s.sensors_settled = true);
    rig.publish_snap();
    rig.run(&mut c, 1);
    assert!(rig.shared.status().discovery_running);
    rig.run(&mut c, 1000);
    assert_eq!(runs(&rig), 1);
    assert!(!rig.shared.status().discovery_running);
    // nothing changed since: no further run
    rig.publish_snap();
    rig.run(&mut c, 500);
    assert_eq!(runs(&rig), 1);
    // a change of the inputs after the STM settled runs at once (through the gate, not held)
    rig.snap(|s| {
        copy_string(&mut s.version.hw, b"C2");
    });
    rig.publish_snap();
    rig.run(&mut c, 1);
    assert!(rig.shared.status().discovery_running);
}

#[test]
fn gate_inputs_that_change_and_change_back_during_a_run_start_no_other() {
    let rig = Rig::new();
    let mut c = rig.client();
    use_ha(&rig);
    link_up(&rig);
    rig.snap(|s| s.sensors_settled = true);
    rig.publish_snap();
    c.begin();
    rig.run(&mut c, 5);
    assert!(rig.shared.status().discovery_running);
    rig.snap(|s| {
        copy_string(&mut s.version.hw, b"C2");
    });
    rig.publish_snap();
    rig.run(&mut c, 5);
    assert!(rig.shared.status().discovery_running);
    rig.snap(|s| s.version.hw.clear());
    rig.publish_snap();
    rig.run(&mut c, 1000);
    assert_eq!(runs(&rig), 1);
    assert!(!rig.shared.status().discovery_running);
}

#[test]
fn gate_without_a_settling_stm_the_run_starts_120_s_after_the_connect() {
    let rig = Rig::new();
    let mut c = rig.client();
    use_ha(&rig);
    link_up(&rig);
    c.begin();
    rig.run(&mut c, 1); // the connect
    assert!(rig.session_up());
    let wait = DiscoveryGate::MAX_WAIT_MS as usize / 1000 * PASSES_PER_SECOND;
    rig.run(&mut c, wait - 10);
    assert_eq!(rig.count_prefix("homeassistant/", 0), 0);
    assert!(!rig.shared.status().discovery_running);
    rig.run(&mut c, 20);
    assert!(rig.shared.status().discovery_running);
    rig.run(&mut c, 1000);
    assert_eq!(runs(&rig), 1);
}

#[test]
fn gate_a_manual_request_runs_at_once_and_answers_the_waiting_run() {
    let rig = Rig::new();
    let mut c = rig.client();
    use_ha(&rig);
    link_up(&rig);
    c.begin();
    rig.run(&mut c, 40);
    assert!(rig.session_up());
    rig.shared.request_discovery(DiscoveryAction::Delete);
    rig.run(&mut c, 1);
    assert!(rig.shared.status().discovery_running);
    rig.run(&mut c, 1000);
    assert_eq!(runs(&rig), 1);
    // the waiting automatic run is gone: nothing publishes the configs the user deleted
    let before = rig.published_len();
    rig.run(
        &mut c,
        DiscoveryGate::MAX_WAIT_MS as usize / 1000 * PASSES_PER_SECOND,
    );
    assert_eq!(runs(&rig), 1);
    for p in &rig.published()[before..] {
        assert!(!p.topic.starts_with(b"homeassistant/"), "{}", s(&p.topic));
    }
}

#[test]
fn gate_a_session_that_ends_drops_the_waiting_run_the_next_one_asks() {
    let rig = Rig::new();
    let mut c = rig.client();
    use_ha(&rig);
    link_up(&rig);
    c.begin();
    rig.run(&mut c, 1); // the connect
    assert_eq!(rig.connects(), 1);
    rig.run(&mut c, 60 * PASSES_PER_SECOND);
    rig.broker.drop_connection();
    rig.run(&mut c, 100); // the reconnect after the back-off
    assert_eq!(rig.connects(), 2);
    // 120 s after the first connect: the wait of the first session is gone with it
    rig.run(&mut c, 60 * PASSES_PER_SECOND);
    assert!(!rig.shared.status().discovery_running);
    assert_eq!(runs(&rig), 0);
    // 120 s after the second connect (about 60 s after the first)
    rig.run(&mut c, 55 * PASSES_PER_SECOND);
    assert!(!rig.shared.status().discovery_running);
    assert_eq!(runs(&rig), 0);
    rig.run(&mut c, 5 * PASSES_PER_SECOND);
    assert!(rig.shared.status().discovery_running || runs(&rig) == 1);
}

#[test]
fn gate_the_cleanup_of_mode_1_runs_at_once_without_memory_later() {
    let rig = Rig::new();
    let mut c = rig.client();
    use_ha(&rig);
    rig.cfg(|c| c.mqtt.mode = MqttMode::Mqtt);
    rig.host.state().ha_cleanup_done = false;
    link_up(&rig);
    c.begin();
    rig.run(&mut c, 1); // the connect: the cleanup starts with it
    assert!(rig.shared.status().discovery_running);
    rig.run(&mut c, 1000);
    assert_eq!(rig.host.state().ha_cleanup_marks, 1);
    assert_eq!(runs(&rig), 1);
    // the next boot, no memory at the connect: the gate takes the cleanup
    rig.host.state().ha_cleanup_done = false;
    rig.broker.drop_connection();
    rig.dev.heap.state().fail_all = true;
    rig.run(&mut c, 100);
    assert_eq!(rig.connects(), 2);
    assert!(!rig.shared.status().discovery_running);
    rig.dev.heap.state().fail_all = false;
    rig.run(&mut c, 1);
    assert!(!rig.shared.status().discovery_running); // the STM inputs are not settled
    rig.snap(|s| s.sensors_settled = true);
    rig.publish_snap();
    rig.run(&mut c, 1);
    assert!(rig.shared.status().discovery_running);
    rig.run(&mut c, 1000);
    assert_eq!(rig.host.state().ha_cleanup_marks, 2);
}

// ---------------------------------------------------------------- memory

#[test]
fn discovery_a_run_takes_its_context_and_buffers_from_the_heap_for_the_run() {
    // C++ "one block a run" (context and payload in one block): the Rust run takes two, the
    // context and one block of payload and list buffer
    let rig = Rig::new();
    let mut c = rig.client();
    use_ha(&rig);
    link_up(&rig);
    rig.snap(|s| s.sensors_settled = true);
    rig.publish_snap();
    c.begin();
    assert_eq!(run_blocks(&rig), (0, 0)); // nothing at boot
    rig.run(&mut c, 1000);
    assert_eq!(runs(&rig), 1);
    assert_eq!(run_blocks(&rig), (1, 1));
    assert!(c.disc.is_none()); // freed when the run ended
    rig.shared.request_discovery(DiscoveryAction::Publish);
    rig.run(&mut c, 1000);
    assert_eq!(runs(&rig), 2);
    assert_eq!(run_blocks(&rig), (2, 2));
    // a request during a run restarts it in the blocks it has
    rig.shared.request_discovery(DiscoveryAction::Publish);
    rig.run(&mut c, 5);
    assert!(rig.shared.status().discovery_running);
    rig.shared.request_discovery(DiscoveryAction::Publish);
    rig.run(&mut c, 1000);
    assert_eq!(runs(&rig), 3);
    assert_eq!(run_blocks(&rig), (3, 3));
}

#[test]
fn discovery_without_memory_a_run_waits_for_the_next_pass_automatic_or_manual() {
    let rig = Rig::new();
    let mut c = rig.client();
    use_ha(&rig);
    link_up(&rig);
    rig.snap(|s| s.sensors_settled = true);
    rig.publish_snap();
    c.begin();
    rig.dev.heap.state().fail_all = true;
    rig.run(&mut c, 40);
    assert!(rig.session_up());
    assert!(!rig.shared.status().discovery_running);
    assert_eq!(rig.count_prefix("homeassistant/", 0), 0);
    rig.dev.heap.state().fail_all = false;
    rig.run(&mut c, 1);
    assert!(rig.shared.status().discovery_running);
    rig.run(&mut c, 1000);
    assert_eq!(runs(&rig), 1);
    rig.dev.heap.state().fail_all = true;
    rig.shared.request_discovery(DiscoveryAction::Delete);
    rig.run(&mut c, 50);
    assert!(!rig.shared.status().discovery_running);
    assert_eq!(runs(&rig), 1);
    rig.dev.heap.state().fail_all = false;
    rig.run(&mut c, 1);
    assert!(rig.shared.status().discovery_running);
    rig.run(&mut c, 1000);
    assert_eq!(runs(&rig), 2);
}

#[test]
fn discovery_ha_back_online_without_memory_for_the_run_gets_it_later() {
    let rig = Rig::new();
    let mut c = rig.client();
    use_ha(&rig);
    link_up(&rig);
    rig.snap(|s| s.sensors_settled = true);
    rig.publish_snap();
    c.begin();
    rig.run(&mut c, 1000);
    assert_eq!(runs(&rig), 1);
    rig.deliver("homeassistant/status", "offline");
    rig.run(&mut c, 2);
    rig.dev.heap.state().fail_all = true;
    rig.deliver("homeassistant/status", "online");
    rig.run(&mut c, 2);
    assert_eq!(rig.shared.regulator_state().ha, HaStatus::Online);
    assert!(!rig.shared.status().discovery_running);
    rig.dev.heap.state().fail_all = false;
    rig.run(&mut c, 1);
    assert!(rig.shared.status().discovery_running);
    rig.run(&mut c, 1000);
    assert_eq!(runs(&rig), 2);
}

#[test]
fn diag_the_largest_profile_fits_its_json_buffer() {
    let rig = Rig::new();
    let mut c = rig.client();
    use_ha(&rig);
    rig.cfg(|c| {
        c.mqtt.mode = MqttMode::Mqtt;
        c.valves[11].active = true;
    });
    link_up(&rig);
    rig.snap(|s| {
        s.valves[11].known = true;
        s.valves[11].has_extended = true;
    });
    rig.publish_snap();
    c.begin();
    rig.run(&mut c, 40);
    assert!(rig.session_up());
    {
        let mut h = rig.host.state();
        let p = &mut h.profiles[11];
        p.valve = 11;
        p.count = PROFILE_MAX_SAMPLES;
        for x in p.samples.iter_mut() {
            x.count = u32::MAX;
            x.current = u16::MAX;
        }
        h.snap.profile_seq[11] += 1;
    }
    rig.publish_snap();
    rig.run(&mut c, 5);
    let m = rig
        .broker
        .state()
        .published_to(b"VdMot/diag/valves/12/profile");
    assert_eq!(m.len(), 1);
    assert_eq!(m[0].payload.len(), 643);
    assert!(
        s(&m[0].payload).starts_with("{\"valve\":12,\"count\":32,\"samples\":[[4294967295,65535],")
    );
}
