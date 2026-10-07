//! Tests of `app` (C++ `test_app.cpp`; the cases on the queue, the snapshot, the profile store,
//! the save state and the calibration info test `AppShared` in `shared/tests.rs`): boot
//! sequence, factory reset pin, tasks, app task, resources, heap guard and /api/health. The
//! "NRST released first" case of the retired `test_main.cpp` is the first entry of the boot
//! sequence.
// host test code: the stack rule of the glue (design 2.4) is for the device
#![allow(clippy::large_stack_frames)]

use std::sync::{Arc, Mutex};

use super::rig::{FakeThreads, Rig};
use super::*;
use crate::port::HeapStats;
use crate::testkit::{lock as tlock, run, Ended, FakeBoard, FakeClock, FakeWatchdog, Reset};
use vdm_esp_core::common::copy_string;
use vdm_esp_core::stm_flasher::FlashPhase;
use vdm_esp_core::stm_types::StmSnapshot;

fn rig_after(reset: Reset) -> Rig {
    let board = FakeBoard::new();
    board.reset(reset);
    let dev = board.boot();
    let host = super::rig::FakeHost::new(dev.journal.clone());
    Rig {
        dev,
        shared: AppShared::new(),
        host,
    }
}

#[test]
fn setup_the_boot_sequence() {
    let rig = Rig::new();
    rig.journal_sleeps();
    let mut app = rig.app();
    rig.setup(&mut app);
    // C++ also had serial0.begin (the firmware's console), logger.begin (main creates the ring
    // and the sinks), pinMode 2 (the pin adapter), esp_task_wdt_init and the tasks (main,
    // spawn_tasks: tests below)
    let order = [
        "stm_link.begin",
        "delay 2",
        "storage.beginFs",
        "storage.loadConfig",
        "storage.setActiveConfig",
        "logger.log boot",
        "logger.configure",
        "stm_service.begin",
        "net.begin",
        "ota.begin",
        "mqtt.begin",
    ];
    let j = &rig.dev.journal;
    let mut at = 0;
    for (i, step) in order.iter().enumerate() {
        let found = j
            .find(step, if i == 0 { 0 } else { at + 1 })
            .unwrap_or_else(|| panic!("{step} missing or out of order: {:?}", j.entries()));
        at = found;
    }
    // test_main.cpp: the STM leaves reset before anything else happens
    assert_eq!(j.entries()[0], "stm_link.begin");
    let h = rig.host.s();
    assert!(h.factory_latch_sets.is_empty());
    assert_eq!(h.factory_resets, 0);
    assert_eq!(h.ota_begins, 1);
    assert_eq!(h.stm_service_begins, 1);
    // the log sinks follow the loaded config
    assert_eq!(h.configures, vec![(0, 0, 514, true, b"VdMot".to_vec())]);
}

#[test]
fn setup_the_log_sinks_follow_the_loaded_config() {
    let rig = Rig::new();
    {
        let mut h = rig.host.s();
        h.loaded.syslog.level = 2;
        h.loaded.syslog.server = 0x0A00_000A;
        h.loaded.syslog.port = 1514;
        h.loaded.persist_log = false;
        copy_string(&mut h.loaded.station, b"Boiler");
    }
    let mut app = rig.app();
    rig.setup(&mut app);
    let h = rig.host.s();
    assert_eq!(
        h.configures,
        vec![(2, 0x0A00_000A, 1514, false, b"Boiler".to_vec())]
    );
    assert_eq!(&h.active.station[..], b"Boiler");
    assert_eq!(h.revision, 1);
}

#[test]
fn setup_tasks_of_the_binding_table() {
    // first-build sizes of GLUE-DESIGN-ESP.md 2.1 (C++ 6656, 7168, 7168)
    assert_eq!(
        TASKS,
        [
            TaskSpec {
                name: "stm",
                stack_bytes: 8192,
                priority: 5,
                core: 1
            },
            TaskSpec {
                name: "app",
                stack_bytes: 9216,
                priority: 3,
                core: 1
            },
            TaskSpec {
                name: "mqtt",
                stack_bytes: 9216,
                priority: 2,
                core: 1
            },
        ]
    );
    assert_eq!(TASK_WDT_TIMEOUT_S, 30);
    assert_eq!(HTTPD_STACK_BYTES, 10_240);
    assert_eq!(
        (PASS_DELAY_MS, SECOND_MS, RESOURCES_MS),
        (100, 1000, 10_000)
    );
}

/// A task that records its start and passes.
struct Probe {
    name: &'static str,
    log: Arc<Mutex<Vec<String>>>,
}

impl Task<FakeWatchdog> for Probe {
    fn start(&mut self, watchdog: FakeWatchdog) {
        watchdog.feed();
        tlock(&self.log).push(format!("start {}", self.name));
    }
    fn pass(&mut self) -> u32 {
        tlock(&self.log).push(format!("pass {}", self.name));
        7
    }
}

type Body<'a> = Box<dyn FnOnce(FakeWatchdog) + Send + 'a>;

#[derive(Default)]
struct FakeSpawner<'a> {
    specs: Vec<TaskSpec>,
    bodies: Vec<Body<'a>>,
}

impl<'a> Spawner<'a, FakeWatchdog> for FakeSpawner<'a> {
    fn spawn(&mut self, spec: &TaskSpec, body: Body<'a>) {
        self.specs.push(*spec);
        self.bodies.push(body);
    }
}

#[test]
fn setup_the_threads_start_in_the_order_of_the_table_each_with_its_task() {
    let clock = FakeClock::default();
    let log = Arc::new(Mutex::new(Vec::new()));
    let probe = |name| Probe {
        name,
        log: log.clone(),
    };
    let mut sp = FakeSpawner::default();
    spawn_tasks(&mut sp, &clock, probe("stm"), probe("app"), probe("mqtt"));
    assert_eq!(sp.specs, TASKS);
    for body in sp.bodies {
        let wd = FakeWatchdog::default();
        clock.stop_after_sleeps(2);
        assert_eq!(run(|| body(wd.clone())), Ended::Stopped);
        assert_eq!(wd.feeds(), 1);
    }
    let want: Vec<String> = ["stm", "app", "mqtt"]
        .iter()
        .flat_map(|n| {
            [
                format!("start {n}"),
                format!("pass {n}"),
                format!("pass {n}"),
            ]
        })
        .collect();
    assert_eq!(*tlock(&log), want);
    assert_eq!(clock.sleeps(), vec![7; 6]);
}

#[test]
fn setup_the_boot_event_carries_the_reset_reason_the_boot_count_and_the_version() {
    let rig = rig_after(Reset::Software);
    rig.host.s().boot_count = 41;
    let mut app = rig.app();
    rig.setup(&mut app);
    let e = rig.host.first(EventCode::Boot);
    assert_eq!(e.arg1, 3); // ESP_RST_SW
    assert_eq!(e.arg2, 42);
    assert_eq!(e.severity, Severity::Info);
    assert_eq!(e.valve, NO_VALVE);
    assert_eq!(&e.text[..], firmware_version().as_bytes());
}

#[test]
fn setup_a_panic_reset_makes_the_boot_event_a_warning() {
    let rig = rig_after(Reset::Panic);
    let mut app = rig.app();
    rig.setup(&mut app);
    let e = rig.host.first(EventCode::Boot);
    assert_eq!(e.arg1, 4);
    assert_eq!(e.severity, Severity::Warning);
}

#[test]
fn setup_gpio2_held_low_for_5_s_resets_the_configuration_and_sets_the_latch() {
    let rig = Rig::new();
    rig.journal_sleeps();
    rig.dev.gpio.set_input(2, |_| true);
    let mut app = rig.app();
    rig.setup(&mut app);
    assert_eq!(rig.host.s().factory_resets, 1);
    assert_eq!(rig.host.s().factory_latch_sets, vec![true]);
    let now = rig.dev.clock.ms();
    assert!((5002..5100).contains(&now), "{now}");
    let j = &rig.dev.journal;
    let settle = j.find("delay 2", 0).expect("settle");
    assert!(j.find("delay 50", settle).is_some());
    assert!(j.find("storage.beginFs", 0) < j.find("storage.loadConfig", 0));
    let e = rig.host.first(EventCode::ConfigSaved);
    assert_eq!(&e.text[..], b"factory");
    assert_eq!((e.arg1, e.arg2), (1, 0)); // the revision of the active config
    assert!(!rig.host.has(EventCode::FactoryResetSkipped));
}

#[test]
fn setup_a_failed_factory_reset_is_logged_with_minus_1_and_sets_no_latch() {
    let rig = Rig::new();
    rig.dev.gpio.set_input(2, |now| now < 30_000);
    rig.host.s().factory_reset_result = false;
    let mut app = rig.app();
    rig.setup(&mut app);
    assert_eq!(rig.host.s().factory_resets, 1);
    assert_eq!(rig.host.first(EventCode::ConfigSaved).arg2, -1);
    // the next boot with the jumper tries again
    assert!(rig.host.s().factory_latch_sets.is_empty());
    // not latched: the pin going high at run time clears nothing
    rig.run_app(&mut app, 300);
    assert!(rig.dev.clock.ms() > 30_000);
    assert!(rig.host.s().factory_latch_sets.is_empty());
}

#[test]
fn setup_gpio2_still_set_after_a_pin_reset_settings_kept_no_wait() {
    let rig = Rig::new();
    rig.dev.gpio.set_input(2, |_| true);
    rig.host.s().factory_latched = true;
    let mut app = rig.app();
    rig.setup(&mut app);
    assert_eq!(rig.host.s().factory_resets, 0);
    assert!(rig.host.s().factory_latch_sets.is_empty());
    assert!(rig.dev.clock.ms() < 100);
    let e = rig.host.first(EventCode::FactoryResetSkipped);
    assert_eq!(e.severity, Severity::Warning);
    assert!(!rig.host.has(EventCode::ConfigSaved));
}

#[test]
fn setup_gpio2_released_at_boot_clears_the_latch() {
    let rig = Rig::new();
    rig.host.s().factory_latched = true;
    let mut app = rig.app();
    rig.setup(&mut app);
    assert_eq!(rig.host.s().factory_latch_sets, vec![false]);
    assert_eq!(rig.host.s().factory_resets, 0);
    assert!(!rig.host.has(EventCode::FactoryResetSkipped));
    // cleared at boot: nothing more to clear at run time
    rig.run_app(&mut app, 30);
    assert_eq!(rig.host.s().factory_latch_sets, vec![false]);
}

#[test]
fn setup_gpio2_low_for_3_s_then_high_does_not_reset() {
    let rig = Rig::new();
    rig.dev.gpio.set_input(2, |now| now < 3000);
    let mut app = rig.app();
    rig.setup(&mut app);
    assert_eq!(rig.host.s().factory_resets, 0);
    assert!(rig.host.s().factory_latch_sets.is_empty());
    assert!(!rig.host.has(EventCode::ConfigSaved));
    let now = rig.dev.clock.ms();
    assert!((3000..3100).contains(&now), "{now}");
}

#[test]
fn task_the_latch_is_cleared_once_when_the_pin_goes_high_at_run_time() {
    let rig = Rig::new();
    rig.dev.gpio.set_input(2, |now| now < 30_000);
    rig.host.s().factory_latched = true;
    let mut app = rig.app();
    rig.setup(&mut app);
    rig.run_app(&mut app, 250); // 25 s: pin still low
    assert!(rig.host.s().factory_latch_sets.is_empty());
    rig.run_app(&mut app, 100); // past 30 s
    assert_eq!(rig.host.s().factory_latch_sets, vec![false]);
    rig.run_app(&mut app, 30);
    assert_eq!(rig.host.s().factory_latch_sets, vec![false]);
}

#[test]
fn task_a_factory_reset_at_boot_is_latched_until_the_pin_goes_high() {
    // Rust addition: the latch of a reset done at this boot
    let rig = Rig::new();
    rig.dev.gpio.set_input(2, |now| now < 20_000);
    let mut app = rig.app();
    rig.setup(&mut app);
    assert_eq!(rig.host.s().factory_latch_sets, vec![true]);
    rig.run_app(&mut app, 100);
    assert_eq!(rig.host.s().factory_latch_sets, vec![true]);
    rig.run_app(&mut app, 200);
    assert_eq!(rig.host.s().factory_latch_sets, vec![true, false]);
}

#[test]
fn task_without_a_latch_the_pin_is_not_watched_at_run_time() {
    let rig = Rig::new();
    let mut app = rig.app();
    rig.setup(&mut app);
    rig.run_app(&mut app, 30);
    assert!(rig.host.s().factory_latch_sets.is_empty());
}

#[test]
fn setup_a_formatted_and_a_failed_file_system_are_logged() {
    let rig = Rig::new();
    rig.host.s().formatted = true;
    rig.host.s().begin_fs_result = false;
    let mut app = rig.app();
    rig.setup(&mut app);
    let ev = rig.host.with_code(EventCode::FsFormatted);
    assert_eq!(ev.len(), 2);
    assert_eq!(ev[0].arg1, 0);
    assert_eq!(ev[1].arg1, -1);
}

#[test]
fn setup_a_formatted_file_system_alone() {
    // Rust addition: each of the two events stands on its own
    let rig = Rig::new();
    rig.host.s().formatted = true;
    let mut app = rig.app();
    rig.setup(&mut app);
    let ev = rig.host.with_code(EventCode::FsFormatted);
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].arg1, 0);
}

#[test]
fn setup_net_begin_may_change_the_configuration_a_reverted_network_trial() {
    let rig = Rig::new();
    rig.host.s().net_on_begin = Some(Box::new(|c: &mut Config| c.net.ip = 0x0A00_000A));
    let mut app = rig.app();
    rig.setup(&mut app);
    let h = rig.host.s();
    assert_eq!(
        h.net_begun_with.as_ref().map(|c| c.net.ip),
        Some(0x0A00_000A)
    );
}

#[test]
fn read_health_version_uptime_heap_net_ota_and_log() {
    let rig = Rig::new();
    rig.dev.clock.advance_ms(12_345);
    rig.dev.system.state().heap = HeapStats {
        free: 1000,
        min_free: 900,
        largest: 800,
    };
    let mut parts = HealthParts::default();
    parts.net.reachable = true;
    parts.net.iface_restarts = 3;
    parts.ota.pending = true;
    parts.ota.remaining_s = 77;
    parts.log.flushes = 12;
    // whatever `out` held is replaced
    let mut h = HealthSnapshot {
        task_count: 5,
        uptime_s: 9,
        ..HealthSnapshot::default()
    };
    h.tasks[0].stack_bytes = 1;
    read_health(&rig.dev.clock, &rig.dev.system, &rig.shared, parts, &mut h);
    assert_eq!(h.tasks[0], TaskStackInfo::default());
    assert_eq!(h.version, firmware_version().as_bytes());
    assert_eq!(h.uptime_s, 12);
    assert_eq!((h.free_heap, h.min_free_heap), (1000, 900));
    assert_eq!(h.largest_free_block, 800);
    assert_eq!(h.min_largest_free_block, 0); // no sample yet
    assert_eq!(h.task_count, 0); // no task started yet
    assert!(h.net.reachable);
    assert_eq!(h.net.iface_restarts, 3);
    assert!(h.ota.pending);
    assert_eq!(h.ota.remaining_s, 77);
    assert_eq!(h.log.flushes, 12);
    assert_eq!(rig.dev.clock.now_ms(), 12_345);
}

#[test]
fn read_health_our_tasks_at_once_library_tasks_once_found_by_a_resource_sample() {
    let rig = Rig::new();
    let mut app = rig.app();
    rig.setup(&mut app);
    rig.stack("stm", 2100);
    rig.stack("app", 3200);
    rig.stack("mqtt", 4100);
    rig.stack("tiT", 1500); // exists, not found yet
    let tasks = |rig: &Rig| {
        let mut h = HealthSnapshot::default();
        let parts = HealthParts::default();
        read_health(&rig.dev.clock, &rig.dev.system, &rig.shared, parts, &mut h);
        let n = usize::from(h.task_count);
        h.tasks[..n]
            .iter()
            .map(|t| {
                (
                    String::from_utf8_lossy(t.name).into_owned(),
                    t.stack_bytes,
                    t.min_free_bytes,
                )
            })
            .collect::<Vec<_>>()
    };
    let ours = vec![
        ("stm".to_string(), 8192, 2100),
        ("app".to_string(), 9216, 3200),
        ("mqtt".to_string(), 9216, 4100),
    ];
    assert_eq!(tasks(&rig), ours);
    rig.run_app(&mut app, 101); // one sample after 10 s
    let mut want = ours.clone();
    want.push(("tiT".to_string(), 3072, 1500));
    assert_eq!(tasks(&rig), want);
    let mut h = HealthSnapshot::default();
    read_health(
        &rig.dev.clock,
        &rig.dev.system,
        &rig.shared,
        HealthParts::default(),
        &mut h,
    );
    assert_eq!(h.min_largest_free_block, 110_000);
    // listed in the order of the table, not in the order found
    rig.stack("httpd", 9000);
    rig.stack("sys_evt", 1100);
    assert_eq!(tasks(&rig).len(), 4);
    rig.run_app(&mut app, 101);
    let mut want = ours;
    want.push(("httpd".to_string(), 10_240, 9000));
    want.push(("sys_evt".to_string(), 3072, 1100));
    want.push(("tiT".to_string(), 3072, 1500));
    assert_eq!(tasks(&rig), want);
    assert!(!rig.host.has(EventCode::StackLow));
}

#[test]
fn task_once_a_second_the_network_the_web_server_ota_and_the_schedule() {
    let rig = Rig::new();
    let mut app = rig.app();
    rig.setup(&mut app);
    {
        let mut h = rig.host.s();
        h.net_up = true;
        h.ota_net_ok = true;
        h.mqtt_connected = true;
    }
    let t0 = rig.dev.clock.ms() as u32; // setup waited 2 ms for GPIO2
    rig.run_app(&mut app, 11); // 1.1 s
    let h = rig.host.s();
    assert_eq!(h.net_services, vec![(t0 + 1000, true)]);
    assert_eq!(h.web_begins, 1);
    assert_eq!(h.ota_services, vec![(t0 + 1000, true, false, true)]);
    assert_eq!(h.stm_service_services, vec![t0 + 1000]);
    assert_eq!(h.logger_services.len(), 11);
    assert!(h.logger_services[0]);
    assert_eq!(h.storage_services, 11);
    assert_eq!(h.service_restarts.len(), 11);
    assert_eq!(h.service_restarts[0], (t0, true, false));
    drop(h);
    assert_eq!(rig.dev.clock.sleeps().last(), Some(&100));
    assert_eq!(rig.dev.watchdog.feeds(), 11);
    let j = &rig.dev.journal;
    let net = j.find(&format!("net.service {}", t0 + 1000), 0);
    let ota = j.find(&format!("ota.service {}", t0 + 1000), 0);
    assert!(net.is_some() && net < ota);
}

#[test]
fn task_the_link_state_goes_to_ota_the_web_server_starts_only_with_the_network() {
    let rig = Rig::new();
    let mut app = rig.app();
    rig.setup(&mut app);
    let mut s = Box::<StmSnapshot>::default();
    s.link = LinkState::Up;
    rig.shared.publish_stm_snapshot(&s);
    rig.run_app(&mut app, 11);
    let h = rig.host.s();
    assert_eq!(h.web_begins, 0);
    assert_eq!(h.ota_services.len(), 1);
    assert!(h.ota_services[0].2);
    assert!(!h.ota_services[0].3);
    assert!(h.service_restarts[0].2);
    assert!(!h.net_services[0].1);
}

#[test]
fn task_a_started_web_server_is_not_begun_again() {
    // Rust addition: the start is asked for only while the server does not run
    let rig = Rig::new();
    let mut app = rig.app();
    rig.setup(&mut app);
    rig.host.s().net_up = true;
    rig.run_app(&mut app, 31);
    assert_eq!(rig.host.s().web_begins, 1);
    assert_eq!(rig.host.s().ota_services.len(), 3);
    assert!(rig.host.s().ota_services.iter().all(|s| s.3));
}

#[test]
fn task_a_new_config_revision_reconfigures_the_logger_and_the_network() {
    let rig = Rig::new();
    let mut app = rig.app();
    rig.setup(&mut app);
    {
        let mut h = rig.host.s();
        copy_string(&mut h.active.station, b"Boiler");
        h.revision += 1; // C++ storage::applyConfig()
    }
    rig.run_app(&mut app, 1);
    assert_eq!(rig.host.s().reconfigures, vec![b"Boiler".to_vec()]);
    assert_eq!(
        rig.host.s().configures.last().map(|c| c.4.clone()),
        Some(b"Boiler".to_vec())
    );
    rig.run_app(&mut app, 1);
    assert_eq!(rig.host.s().reconfigures.len(), 1);
}

#[test]
fn task_without_memory_for_its_copy_a_config_change_is_applied_by_the_next_pass() {
    let rig = Rig::new();
    let mut app = rig.app();
    rig.setup(&mut app);
    // the boot config is a boot block (C++ counted its allocation): nothing asked of the gate
    assert!(rig.dev.heap.state().granted.is_empty());
    {
        let mut h = rig.host.s();
        copy_string(&mut h.active.station, b"Boiler");
        h.revision += 1;
    }
    let configures = rig.host.s().configures.len();
    rig.dev.heap.state().next.push_back(false);
    rig.run_app(&mut app, 1);
    assert!(rig.host.s().reconfigures.is_empty());
    assert_eq!(rig.host.s().configures.len(), configures);
    rig.run_app(&mut app, 1);
    assert_eq!(rig.host.s().reconfigures, vec![b"Boiler".to_vec()]);
    assert_eq!(
        rig.host.s().configures.last().map(|c| c.4.clone()),
        Some(b"Boiler".to_vec())
    );
    let heap = rig.dev.heap.state();
    assert_eq!(heap.refused, vec![core::mem::size_of::<Config>()]);
    assert_eq!(heap.granted, vec![core::mem::size_of::<Config>()]);
}

#[test]
fn task_resources_are_sampled_every_10_s_not_before() {
    let rig = Rig::new();
    let mut app = rig.app();
    rig.setup(&mut app);
    rig.dev.system.state().heap = HeapStats {
        free: 29_000,
        min_free: 20_000,
        largest: 4000,
    };
    rig.run_app(&mut app, 100); // 9.9 s
    assert!(!rig.host.has(EventCode::LowHeap));
    rig.run_app(&mut app, 101);
    let low = rig.host.with_code(EventCode::LowHeap);
    assert_eq!(low.len(), 1);
    assert_eq!((low[0].arg1, low[0].arg2), (29_000, 20_000));
    let frag = rig.host.with_code(EventCode::HeapFragmented);
    assert_eq!(frag.len(), 1);
    assert_eq!((frag[0].arg1, frag[0].arg2), (4000, 29_000));
    rig.run_app(&mut app, 201); // two more samples: repeated only after an hour
    assert_eq!(rig.host.with_code(EventCode::LowHeap).len(), 1);
}

#[test]
fn task_a_low_stack_high_water_mark_is_reported_once_per_task() {
    let rig = Rig::new();
    let mut app = rig.app();
    rig.setup(&mut app);
    rig.stack("stm", 1023); // just below 8192 / 8
    rig.stack("app", 5000);
    rig.stack("mqtt", 5000);
    rig.stack("httpd", HTTPD_STACK_BYTES / 8 - 1); // just below the threshold
    rig.stack("sys_evt", 511); // below the 512 B floor (3072 / 8 is less)
    rig.stack("tiT", 512); // at the floor: not low
    rig.run_app(&mut app, 101);
    rig.run_app(&mut app, 101);
    let ev = rig.host.with_code(EventCode::StackLow);
    assert_eq!(ev.len(), 3);
    assert_eq!(&ev[0].text[..], b"stm");
    assert_eq!((ev[0].arg1, ev[0].arg2), (1023, 8192));
    assert_eq!(&ev[1].text[..], b"httpd");
    assert_eq!(
        (ev[1].arg1, ev[1].arg2),
        ((HTTPD_STACK_BYTES / 8 - 1) as i32, HTTPD_STACK_BYTES as i32)
    );
    assert_eq!(&ev[2].text[..], b"sys_evt");
    assert_eq!((ev[2].arg1, ev[2].arg2), (511, 3072));
}

#[test]
fn task_the_heap_guard_samples_every_second_from_10_min_on_and_restarts_once() {
    let rig = Rig::new();
    let mut app = rig.app();
    rig.setup(&mut app);
    rig.dev.system.state().heap = HeapStats {
        free: 12 * 1024 - 1,
        min_free: 0,
        largest: 6000,
    };
    rig.dev.clock.advance_ms(599_000);
    // samples at 599.002 s (not armed yet), then 600.002 s (the 60 s start) .. 659.002 s
    rig.run_app(&mut app, 610);
    assert!(rig.host.s().restart_requests.is_empty());
    assert!(!rig.host.has(EventCode::HeapCritical));
    rig.run_app(&mut app, 1); // 660.002 s
    assert_eq!(rig.host.s().restart_requests, vec![(6, 1000)]);
    let ev = rig.host.with_code(EventCode::HeapCritical);
    assert_eq!(ev.len(), 1);
    assert_eq!((ev[0].arg1, ev[0].arg2), (12 * 1024 - 1, 6000));
    assert_eq!(ev[0].severity, Severity::Error);
    let j = &rig.dev.journal;
    let logged = j.find("logger.log heap_critical", 0);
    assert!(logged.is_some());
    assert!(logged < j.find("ota.requestRestart 6 1000", 0));
    // the restart is pending from now on: asked once
    rig.run_app(&mut app, 700);
    assert_eq!(rig.host.s().restart_requests.len(), 1);
    assert_eq!(rig.host.with_code(EventCode::HeapCritical).len(), 1);
}

#[test]
fn task_no_heap_guard_restart_during_an_esp_upload_the_60_s_start_after_it() {
    let rig = Rig::new();
    let mut app = rig.app();
    rig.setup(&mut app);
    rig.dev.system.state().heap.free = 1000;
    rig.host.s().upload_active = true;
    rig.dev.clock.advance_ms(599_000);
    rig.run_app(&mut app, 700); // up to 668.902 s
    assert!(rig.host.s().restart_requests.is_empty());
    rig.host.s().upload_active = false;
    rig.run_app(&mut app, 600); // samples 669.002 s (the 60 s start) .. 728.002 s
    assert!(rig.host.s().restart_requests.is_empty());
    rig.run_app(&mut app, 1);
    assert_eq!(rig.host.s().restart_requests, vec![(6, 1000)]);
}

#[test]
fn task_no_heap_guard_restart_during_an_stm_flash_the_60_s_start_after_it() {
    let rig = Rig::new();
    let mut app = rig.app();
    rig.setup(&mut app);
    rig.dev.system.state().heap.free = 1000;
    rig.shared.mark_stm_flash_active();
    rig.dev.clock.advance_ms(599_000);
    rig.run_app(&mut app, 700);
    assert!(rig.host.s().restart_requests.is_empty());
    let mut s = Box::<StmSnapshot>::default();
    s.flash.phase = FlashPhase::Done;
    rig.shared.publish_stm_snapshot(&s);
    rig.run_app(&mut app, 600);
    assert!(rig.host.s().restart_requests.is_empty());
    rig.run_app(&mut app, 1);
    assert_eq!(rig.host.s().restart_requests.len(), 1);
    assert!(rig.host.has(EventCode::HeapCritical));
}

#[test]
fn task_no_heap_guard_restart_while_another_restart_is_pending() {
    let rig = Rig::new();
    let mut app = rig.app();
    rig.setup(&mut app);
    rig.dev.system.state().heap.free = 1000;
    rig.host.s().restart_pending = true;
    rig.dev.clock.advance_ms(599_000);
    rig.run_app(&mut app, 700);
    assert!(rig.host.s().restart_requests.is_empty());
    assert!(!rig.host.has(EventCode::HeapCritical));
}

#[test]
fn task_the_heap_guard_ignores_a_heap_at_the_threshold() {
    // Rust addition: 12 KB free is not low
    let rig = Rig::new();
    let mut app = rig.app();
    rig.setup(&mut app);
    rig.dev.system.state().heap.free = 12 * 1024;
    rig.dev.clock.advance_ms(599_000);
    rig.run_app(&mut app, 1300);
    assert!(rig.host.s().restart_requests.is_empty());
}

#[test]
fn boot_deadline_restarts_a_boot_that_never_reached_the_app_thread() {
    // Rust addition (GLUE-DESIGN-ESP.md 6.3): setup returned, the app thread did not run
    let rig = Rig::new();
    let mut app = rig.app();
    rig.setup(&mut app);
    assert!(!rig.shared.app_running());
    assert_eq!(
        run(|| boot_deadline(&rig.shared, &rig.dev.system)),
        Ended::Reset(Reset::Software)
    );
    assert_eq!(rig.dev.journal.of("esp_restart").len(), 1);
}

#[test]
fn boot_deadline_is_disarmed_by_the_first_pass_of_the_app_thread() {
    let rig = Rig::new();
    let mut app = rig.app();
    rig.setup(&mut app);
    rig.run_app(&mut app, 1);
    assert!(rig.shared.app_running());
    assert_eq!(
        run(|| boot_deadline(&rig.shared, &rig.dev.system)),
        Ended::Returned(())
    );
    assert!(rig.dev.journal.of("esp_restart").is_empty());
}

#[test]
fn rtc_layout_one_record_after_the_other() {
    let table = [
        (RTC_BOOT_GUARD, crate::boot_guard::MIRROR_LEN),
        (RTC_NET_WATCHDOG, crate::net::RTC_RECORD_LEN),
        (RTC_TARGETS, crate::stm_service::RTC_TARGETS_LEN),
        (RTC_LEASE, crate::stm_service::RTC_LEASE_LEN),
        (RTC_HA_STATUS, crate::mqtt_client::HA_STATUS_RECORD_LEN),
    ];
    assert_eq!(
        table,
        [(0, 28), (28, 8), (36, 46), (82, 14), (96, 8)],
        "boot guard 0..28, net watchdog 28..36, targets 36..82, lease 82..96, HA status 96..104"
    );
    assert_eq!(RTC_LEN, 104);
    assert_eq!(RTC_STM_SERVICE.targets, RTC_TARGETS);
    assert_eq!(RTC_STM_SERVICE.lease, RTC_LEASE);
    // the fake RTC block holds the layout
    const { assert!(RTC_LEN <= crate::testkit::system::RTC_SIZE) };
}

#[test]
fn monitored_tasks_ours_first_then_the_library_tasks() {
    assert_eq!(
        MONITORED,
        [
            ("stm", 8192),
            ("app", 9216),
            ("mqtt", 9216),
            ("httpd", 10_240),
            ("sys_evt", 3072),
            ("tiT", 3072)
        ]
    );
    assert_eq!(OWN_TASKS, 3);
}

#[test]
fn setup_with_the_thread_modules_of_the_rig() {
    // the fake thread modules note their begins (C++ sib::stmLink / sib::mqtt)
    let rig = Rig::new();
    let mut t = FakeThreads(rig.dev.journal.clone());
    t.stm_link_begin();
    t.mqtt_begin();
    assert_eq!(
        rig.dev.journal.entries(),
        vec!["stm_link.begin", "mqtt.begin"]
    );
}
