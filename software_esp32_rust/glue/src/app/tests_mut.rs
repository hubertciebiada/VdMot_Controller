//! Edge cases of `app` (C++ `test_app__mut.cpp`; the snapshot accessors before the first
//! snapshot and the queue that never waits test `AppShared` in `shared/tests.rs`): the exact
//! second and resource periods.
// host test code: the stack rule of the glue (design 2.4) is for the device
#![allow(clippy::large_stack_frames)]

use super::rig::{Rig, TestApp};
use crate::port::HeapStats;
use vdm_esp_core::event_log::EventCode;

/// setup(), then the clock moved so that the first app task pass runs at `ms`.
fn setup_first_pass_at<'a>(rig: &'a Rig, ms: u64) -> TestApp<'a> {
    let mut app = rig.app();
    rig.setup(&mut app);
    let now = rig.dev.clock.ms();
    assert!(now <= ms);
    rig.dev.clock.advance_ms(ms - now);
    app
}

#[test]
fn task_the_once_a_second_work_runs_at_1000_ms_of_uptime() {
    let rig = Rig::new();
    let mut app = setup_first_pass_at(&rig, 1000);
    rig.run_app(&mut app, 1);
    assert_eq!(rig.host.s().net_services.len(), 1);
}

#[test]
fn task_the_once_a_second_work_does_not_run_at_999_ms() {
    let rig = Rig::new();
    let mut app = setup_first_pass_at(&rig, 999);
    rig.run_app(&mut app, 1);
    assert!(rig.host.s().net_services.is_empty());
}

#[test]
fn task_resources_are_sampled_10000_ms_after_the_start_not_1_ms_earlier() {
    let rig = Rig::new();
    let mut app = rig.app();
    rig.setup(&mut app);
    rig.dev.system.state().heap = HeapStats {
        free: 29_000,
        min_free: 20_000,
        largest: 4000,
    };
    // every pass 101 ms apart: the 100th pass runs 9999 ms after the start
    let clock = rig.dev.clock.clone();
    rig.dev.clock.on_sleep(move |_| clock.advance_ms(1));
    rig.run_app(&mut app, 100);
    assert!(!rig.host.has(EventCode::LowHeap));
}
