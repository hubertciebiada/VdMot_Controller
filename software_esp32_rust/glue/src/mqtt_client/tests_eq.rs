//! C++ `test_mqtt_client__eq.cpp`: inputs that are rare but real: a publish that fails as the last
//! one of the full publish, valve profiles whose CRC-32 hits the values the diag comparison
//! stores for "no profile" or for the last profile (every CRC value is reachable: the samples are
//! STM data), and a valve whose first diag pass after the connect runs out of budget.
// host test code: the stack rule of the glue (design 2.4) is for the device
#![allow(clippy::large_stack_frames, clippy::large_stack_arrays)]

use super::rig::Rig;
use super::tests::topic_of;
use super::*;

/// The bytes of a profile as the C++ laid it out (valve, count, 2 bytes of padding, per sample
/// count, current, 2 bytes of padding; little endian), the padding zero.
fn layout(p: &Profile) -> Vec<u8> {
    let mut b = vec![p.valve, p.count, 0, 0];
    for s in &p.samples {
        b.extend_from_slice(&s.count.to_le_bytes());
        b.extend_from_slice(&s.current.to_le_bytes());
        b.extend_from_slice(&[0, 0]);
    }
    b
}

/// CRC-32 over the whole C++ object with zero padding: what the diag comparison hashes.
fn crc_of(p: &Profile) -> u32 {
    let b = layout(p);
    assert_eq!(b.len(), 260);
    let c = crc32(&b, 0);
    assert_eq!(profile_crc(p), c);
    c
}

/// CRC-32 is affine in the message bits, so the 32 bits of one sample count reach every value:
/// solves for them over GF(2).
fn force_crc(p: &mut Profile, sample: usize, target: u32) {
    p.samples[sample].count = 0;
    let base = crc_of(p);
    let mut basis = [0u32; 32]; // basis[k]: a CRC change whose highest set bit is k
    let mut combo = [0u32; 32]; // the count bits that make it
    for j in 0..32 {
        p.samples[sample].count ^= 1 << j;
        let mut v = crc_of(p) ^ base;
        p.samples[sample].count ^= 1 << j;
        let mut c = 1u32 << j;
        for k in (0..32).rev() {
            if v == 0 {
                break;
            }
            if (v >> k) & 1 == 0 {
                continue;
            }
            if basis[k] == 0 {
                basis[k] = v;
                combo[k] = c;
                v = 0;
            } else {
                v ^= basis[k];
                c ^= combo[k];
            }
        }
    }
    let mut v = base ^ target;
    let mut x = 0u32;
    for k in (0..32).rev() {
        if (v >> k) & 1 == 0 {
            continue;
        }
        assert_ne!(basis[k], 0);
        v ^= basis[k];
        x ^= combo[k];
    }
    p.samples[sample].count = x;
    assert_eq!(crc_of(p), target);
}

/// `n` samples (at least 3), currents from `current`; the count of the third one is solved for
/// `crc`.
fn set_profile(p: &mut Profile, valve: u8, crc: u32, current: u16, n: u8) {
    *p = Profile::default();
    p.valve = valve;
    p.count = n;
    for i in 0..n {
        p.samples[usize::from(i)].count = 100 * (u32::from(i) + 1);
        p.samples[usize::from(i)].current = current + u16::from(i);
    }
    force_crc(p, 2, crc);
}

/// A gprof reply: the next snapshot counts it.
fn new_profile(rig: &Rig, v: usize) {
    rig.snap(|s| s.profile_seq[v] += 1);
    rig.publish_snap();
}

fn profile_json(p: &Profile) -> String {
    let samples: Vec<String> = p.samples[..usize::from(p.count)]
        .iter()
        .map(|s| format!("[{},{}]", s.count, s.current))
        .collect();
    format!(
        "{{\"valve\":{},\"count\":{},\"samples\":[{}]}}",
        p.valve + 1,
        p.count,
        samples.join(",")
    )
}

fn profile(rig: &Rig, v: usize, f: impl FnOnce(&mut Profile)) -> Profile {
    let mut h = rig.host.state();
    f(&mut h.profiles[v]);
    h.profiles[v]
}

// ---------------------------------------------------------------- stm/status and failsafe

#[test]
fn a_failsafe_publish_that_fails_at_the_end_of_the_full_publish_is_retried() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.link_up();
    let failsafe = topic_of(&rig.host.state().cfg, Topic::Failsafe);
    rig.script.get().fail_topic = Some(failsafe.as_bytes().to_vec());
    c.begin();
    // the first session, 2 s of back-off, the second session
    rig.run(&mut c, 150);
    // the full publish ends with stm/status (sent) and failsafe (failed: the session is
    // dropped); the on-change check of the same pass finds failsafe unpublished and sends both
    // again, on the dead session
    assert_eq!(rig.shared.status().publish_failures, 3);
    assert_eq!(rig.connects(), 2);
    assert_eq!(rig.payloads(&failsafe), vec!["0"]);
}

// ---------------------------------------------------------------- profiles

#[test]
fn a_profile_after_an_empty_one_goes_out_also_with_the_crc_1() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.link_up();
    rig.snap(|s| s.valves[0].has_extended = true);
    profile(&rig, 0, |p| *p = Profile::default());
    new_profile(&rig, 0); // an empty profile known at the connect
    rig.settle(&mut c, 40);
    assert!(rig.payloads("VdMot/diag/valves/1/profile").is_empty());
    let p = profile(&rig, 0, |p| set_profile(p, 0, 1, 40, 3));
    new_profile(&rig, 0);
    rig.run(&mut c, 5);
    assert_eq!(
        rig.payloads("VdMot/diag/valves/1/profile"),
        vec![profile_json(&p)]
    );
}

#[test]
fn an_empty_profile_is_stored_as_crc_0_not_as_the_crc_of_its_bytes() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.link_up();
    rig.snap(|s| s.valves[0].has_extended = true);
    let empty = profile(&rig, 0, |p| *p = Profile::default());
    let empty_bytes = crc_of(&empty);
    assert_ne!(empty_bytes, 0);
    new_profile(&rig, 0);
    rig.settle(&mut c, 40);
    let p = profile(&rig, 0, |p| set_profile(p, 0, empty_bytes, 40, 3));
    new_profile(&rig, 0);
    rig.run(&mut c, 5);
    assert_eq!(
        rig.payloads("VdMot/diag/valves/1/profile"),
        vec![profile_json(&p)]
    );
}

#[test]
fn a_valve_whose_first_pass_ran_out_of_budget_keeps_its_known_profile_back() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.cfg(|c| c.valves[1].active = true);
    rig.link_up();
    rig.snap(|s| {
        s.valves[0].has_extended = true;
        s.valves[1].has_extended = true;
    });
    profile(&rig, 0, |p| *p = Profile::default());
    profile(&rig, 1, |p| set_profile(p, 1, 1, 40, 3)); // known at the connect, CRC-32 1
    new_profile(&rig, 0);
    new_profile(&rig, 1);
    rig.settle(&mut c, 40);
    // first diag pass: protocol and link take two of the four messages, the last-move checks
    // of valves 1 and 2 the others; valve 2 stores the CRC of its profile all the same, so no
    // later pass takes that profile for a new one
    assert!(rig.payloads("VdMot/diag/valves/2/profile").is_empty());
    let p = profile(&rig, 1, |p| set_profile(p, 1, 2, 40, 3));
    new_profile(&rig, 1);
    rig.run(&mut c, 5);
    assert_eq!(
        rig.payloads("VdMot/diag/valves/2/profile"),
        vec![profile_json(&p)]
    );
}

#[test]
fn the_profile_crc_takes_the_fields_only() {
    // C++ "the profile CRC-32 takes the padding for zeros, whatever it holds": the C++ filled the
    // padding bytes with 0xa5; a Rust profile has no padding the code could read, so the same
    // fields again stand for that step
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.link_up();
    rig.snap(|s| s.valves[0].has_extended = true);
    let known = profile(&rig, 0, |p| set_profile(p, 0, 0x5eed_0001, 40, 3)); // known at connect
    new_profile(&rig, 0);
    rig.settle(&mut c, 40);
    new_profile(&rig, 0); // the same fields: not new
    rig.run(&mut c, 5);
    assert!(rig.payloads("VdMot/diag/valves/1/profile").is_empty());
    // other fields, the sample count among them, with the CRC-32 of the known profile: the
    // comparison sees the same profile
    let other = profile(&rig, 0, |p| set_profile(p, 0, 0x5eed_0001, 50, 4));
    assert_ne!(profile_json(&other), profile_json(&known));
    new_profile(&rig, 0);
    rig.run(&mut c, 5);
    assert!(rig.payloads("VdMot/diag/valves/1/profile").is_empty());
    // another CRC-32: a new profile, published once
    let p = profile(&rig, 0, |p| set_profile(p, 0, 0x5eed_0002, 50, 4));
    new_profile(&rig, 0);
    rig.run(&mut c, 5);
    assert_eq!(
        rig.payloads("VdMot/diag/valves/1/profile"),
        vec![profile_json(&p)]
    );
}
