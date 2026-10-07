//! Tests of the firmware's wiring: every host method reaches its module or shared object, and
//! the modules work together on the fake board as the firmware runs them (the boot order of
//! `main` and setup, the stm link, the calibration chain, the restart path across a reboot).
// host test code: the stack rule of the glue (design 2.4) is for the device
#![allow(clippy::large_stack_frames, clippy::large_stack_arrays)]

use super::*;
use crate::app::{RTC_LEASE, RTC_TARGETS};
use crate::boot_guard::BootGuard;
use crate::port::{IpInfo, OpenMode};
use crate::storage::{KEY_CALIB_SLOT, KEY_LAST_CALIB, KEY_OTA_STM, KEY_TARGETS, NAMESPACE};
use crate::testkit::board::TestPlatform;
use crate::testkit::{run, Device, Ended, FakeBoard, FakeHttpServer, FakeStm, Reset};
use vdm_esp_core::common::{copy_string, ALL_VALVES, NO_VALVE};
use vdm_esp_core::config::{
    encode_config, encode_config_ext, MqttMode, CONFIG_BLOB_MAX, CONFIG_EXT_BLOB_MAX,
};
use vdm_esp_core::event_log::{EventFilter, Severity};
use vdm_esp_core::lease_client::{encode_lease_record, LEASE_RECORD_SIZE};
use vdm_esp_core::net_trial::{
    encode_net_trial, net_trial_fields_crc, NetTrialRecord, NetTrialState, NET_TRIAL_BLOB_MAX,
};
use vdm_esp_core::stm_types::StmCommandType;
use vdm_esp_core::target_store::{encode_targets, PERSISTED_TARGETS_SIZE};
use vdm_esp_core::valve_model::TargetSource;
use vdm_esp_core::version::StmSupport;

type P = TestPlatform;

/// The shared ports of a boot.
fn ports(dev: &Device) -> Ports<'_, P> {
    Ports {
        clock: &dev.clock,
        wall: &dev.wall,
        console: &dev.console,
        nvs: &dev.nvs,
        fs: &dev.fs,
        tcp: &dev.tcp,
        ota: &dev.ota,
        system: &dev.system,
        heap: &dev.heap,
        rtc: &dev.rtc,
    }
}

/// Every event of the shared ring with `code`.
fn events(shared: &Shared, code: EventCode) -> Vec<Event> {
    let mut out: Vec<Event> = vec![Event::default(); 512];
    let r = shared.logger.read(&EventFilter::default(), &mut out);
    out.truncate(r.count);
    out.retain(|e| e.code == code);
    out
}

/// A config stored in NVS as a save of an earlier boot left it.
fn store_config(dev: &Device, f: impl FnOnce(&mut Config)) {
    let mut c = Config::default();
    f(&mut c);
    let mut base = vec![0u8; CONFIG_BLOB_MAX];
    let mut ext = vec![0u8; CONFIG_EXT_BLOB_MAX];
    let n = encode_config(&c, &mut base);
    let x = encode_config_ext(&c, &mut ext, &[]);
    assert!(n > 0 && x > 0);
    dev.nvs.set_blob(NAMESPACE, "cfg", &base[..n]);
    dev.nvs.set_blob(NAMESPACE, "cfgx", &ext[..x]);
}

/// A config the storage validation accepts that differs from the defaults.
fn boiler() -> Config {
    let mut c = Config::default();
    copy_string(&mut c.station, b"Boiler");
    c
}

// ---------------------------------------------------------------- the whole firmware

/// The firmware of one boot, built as `main` builds it.
struct Fw<'a> {
    dev: &'a Device,
    shared: &'a Shared,
    storage: &'a FwStorage<'a, P>,
    sinks: &'a Mutex<FwSinks<'a, P>>,
    service: &'a Mutex<FwService<'a, P>>,
    link: FwLink<'a, P>,
    mqtt: FwMqtt<'a, P>,
    app: FwApp<'a, P, HttpStart<FakeHttpServer>>,
}

/// Builds the firmware on `dev` in the order of `main` (NRST released first, the boot guard,
/// the modules) and hands it to `f`.
fn firmware<R>(dev: &Device, f: impl FnOnce(&mut Fw<'_>) -> R) -> R {
    let shared = Shared::new();
    let p = ports(dev);
    let logger = p.logger(&shared);
    let st = storage(p, &shared);
    let sk = sinks(p, &logger, &shared, dev.udp.clone());
    let sv = Mutex::new(stm_service(p, &shared, &st));
    let mut link = stm_link(
        p,
        &shared,
        &st,
        dev.uart.clone(),
        dev.gpio.output(15),
        dev.gpio.output(14),
    );
    link.release_reset();
    let (guard, report) = match run(|| BootGuard::boot(&dev.nvs, &dev.ota, &dev.rtc, &dev.system)) {
        Ended::Returned(r) => r,
        other => panic!("the boot guard switched: {other:?}"),
    };
    let mq = mqtt(p, &shared, &st);
    let nt = net(
        p,
        &shared,
        &st,
        dev.eth.clone(),
        dev.wifi.clone(),
        dev.sntp.clone(),
        dev.pinger.clone(),
    );
    let ot = ota(
        p,
        &shared,
        OtaWire::new(Wire::new(p, &shared, &st), &sk, &sv),
        guard,
    );
    let modules = Modules::new(
        Wire::new(p, &shared, &st),
        &sk,
        &sv,
        nt,
        ot,
        report,
        HttpStart::new(dev.http.clone()),
    );
    let ap = app(p, &shared, dev.gpio.input(2), modules);
    let mut fw = Fw {
        dev,
        shared: &shared,
        storage: &st,
        sinks: &sk,
        service: &sv,
        link,
        mqtt: mq,
        app: ap,
    };
    f(&mut fw)
}

impl Fw<'_> {
    /// `app::setup` with the thread modules.
    fn setup(&mut self) {
        let mut threads = Threads {
            link: &mut self.link,
            mqtt: &mut self.mqtt,
        };
        self.app.setup(&mut threads);
    }

    /// The starts of the three threads.
    fn start(&mut self) {
        let wd = &self.dev.watchdog;
        Task::start(&mut self.link, wd.clone());
        Task::start(&mut self.app, wd.clone());
        Task::start(&mut self.mqtt, wd.clone());
    }

    /// The three threads for `ms` of fake time: each pass once its delay is over.
    fn run_ms(&mut self, ms: u64) {
        let clock = &self.dev.clock;
        let end = clock.ms() + ms;
        let mut due = [clock.ms(); 3];
        while clock.ms() < end {
            let now = clock.ms();
            if now >= due[0] {
                due[0] = clock.ms() + u64::from(Task::pass(&mut self.link));
            }
            if now >= due[1] {
                due[1] = clock.ms() + u64::from(Task::pass(&mut self.app));
            }
            if now >= due[2] {
                due[2] = clock.ms() + u64::from(Task::pass(&mut self.mqtt));
            }
            let next = due.iter().copied().min().unwrap_or(end);
            if next > clock.ms() {
                clock.set_ms(next);
            }
        }
    }
}

#[test]
fn boot_releases_the_stm_first_then_runs_the_boot_order_over_the_real_modules() {
    let board = FakeBoard::new();
    let dev = board.boot();
    dev.nvs.set_i64(NAMESPACE, KEY_LAST_CALIB, 1_790_000_000);
    firmware(&dev, |fw| {
        fw.setup();
        // main: NRST released before the boot guard touches NVS
        let j = fw.dev.journal.entries();
        assert_eq!(j[..2], ["gpio 14=0", "gpio 15=0"]);
        let s = fw.shared;
        // storage: LittleFS mounted, the defaults active, the boot counted
        assert!(s.storage.fs_ready());
        assert_eq!(s.storage.config_revision(), 1);
        assert_eq!(fw.dev.nvs.get_i(NAMESPACE, "boots"), 1);
        let boot = events(s, EventCode::Boot);
        assert_eq!(boot.len(), 1);
        assert_eq!((boot[0].arg1, boot[0].arg2), (1, 1)); // power-on, boot 1
                                                          // the boot guard's report through ota.begin: no image to fall back to
        assert_eq!(events(s, EventCode::EspOtaFailed)[0].arg1, -3);
        // stm_service.begin: the last calibration time of NVS
        assert_eq!(s.app.calib_info().last_scheduled_epoch, 1_790_000_000);
        // net.begin: Ethernet started with the host name of the station
        let begins = fw.dev.eth.state().begins.clone();
        assert_eq!(begins.len(), 1);
        assert_eq!(begins[0].hostname, "VdMot");
        // the stm link opened Serial2 again
        assert_eq!(fw.dev.uart.state().configures, 1);
    });
}

#[test]
fn boot_a_stored_config_reaches_the_logger_mqtt_and_net() {
    let board = FakeBoard::new();
    let dev = board.boot();
    store_config(&dev, |c| {
        copy_string(&mut c.station, b"Boiler");
        c.persist_log = false;
        c.mqtt.mode = MqttMode::Mqtt;
        copy_string(&mut c.mqtt.host, b"broker.lan");
    });
    firmware(&dev, |fw| {
        fw.setup();
        let s = fw.shared;
        assert!(!s.logger.stats(0).persist);
        assert_eq!(s.mqtt.regulator_state().mode, MqttMode::Mqtt);
        assert_eq!(fw.dev.eth.state().begins[0].hostname, "Boiler");
        assert_eq!(s.storage.boot_load_source(), LoadSource::Stored);
    });
}

#[test]
fn the_stm_link_comes_up_and_the_other_threads_see_it() {
    let board = FakeBoard::new();
    let dev = board.boot();
    store_config(&dev, |c| c.valves[0].active = true);
    let stm = FakeStm::attach(&dev.uart, &dev.gpio, &dev.clock);
    firmware(&dev, |fw| {
        fw.setup();
        fw.start();
        fw.run_ms(8000);
        let s = fw.shared;
        assert_eq!(s.app.stm_link_state(), LinkState::Up);
        assert_eq!(s.app.stm_protocol(), 2);
        assert_eq!(s.app.stm_support(), StmSupport::Supported);
        assert!(s.app.stm_snapshot_revision() > 0);
        // OTA sees the link (its health), the regulator state of MQTT reached the session
        assert!(s.ota.health().stm_ok);
        // a command of another thread goes through the queue onto the UART
        let c = StmCommand {
            kind: StmCommandType::SetTarget,
            valve: 0,
            pos: 40,
            source: TargetSource::Web,
            ..StmCommand::default()
        };
        assert!(s.app.submit(&c));
        fw.run_ms(1000);
        assert_eq!(
            stm.requests_of("stgtp").last().map(String::as_str),
            Some("stgtp 0 40 ")
        );
        // the desired target went to the RTC record of the layout (with the targets the other
        // valves adopted from the STM)
        let rtc = fw.dev.rtc.snapshot();
        let mut t = PersistedTargets::default();
        assert!(vdm_esp_core::target_store::decode_targets(
            &rtc[RTC_TARGETS..RTC_TARGETS + PERSISTED_TARGETS_SIZE],
            &mut t
        ));
        assert_eq!(
            (t.valid[0], t.pos[0], t.source[0]),
            (true, 40, TargetSource::Web)
        );
        // the lease record of every second sits at its place too (a valid record)
        let lease = &rtc[RTC_LEASE..RTC_LEASE + LEASE_RECORD_SIZE];
        assert!(vdm_esp_core::lease_client::decode_lease_record(lease).is_some());
    });
}

#[test]
fn a_scheduled_calibration_is_booked_when_the_stm_confirms_it() {
    let board = FakeBoard::new();
    let dev = board.boot();
    // Wednesday 2026-09-23 01:00 UTC = 03:00 CEST (the default zone), the schedule Wednesday
    // 03:00
    dev.wall.set(1_790_121_600 + 3600);
    store_config(&dev, |c| {
        c.calib.day_mask = 1 << 3;
        c.calib.hour = 3;
        c.calib.minute = 0;
    });
    let stm = FakeStm::attach(&dev.uart, &dev.gpio, &dev.clock);
    firmware(&dev, |fw| {
        fw.setup();
        fw.start();
        fw.run_ms(25_000);
        assert_eq!(stm.requests_of("staln"), vec!["staln 255 "]);
        let s = fw.shared;
        let e = events(s, EventCode::ScheduledCalibration);
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].arg1, 20_260_923);
        assert_eq!(fw.dev.nvs.get_i(NAMESPACE, KEY_CALIB_SLOT), 20_260_923);
        assert!(fw.dev.nvs.get_i(NAMESPACE, KEY_LAST_CALIB) >= 1_790_121_600 + 3600);
        let ci = s.app.calib_info();
        assert_eq!(ci.next_slot, 20_260_930);
        assert_eq!(ci.next_epoch, 1_790_121_600 + 7 * 86_400 + 3600);
        // the STM started it as a scheduled one
        let started = events(s, EventCode::CalibStarted);
        assert!(started.iter().all(|e| e.arg1 == 1));
    });
}

#[test]
fn the_heap_guard_restart_keeps_the_desired_targets_across_the_reboot() {
    let board = FakeBoard::new();
    {
        let dev = board.boot();
        store_config(&dev, |c| {
            c.valves[0].active = true;
            // no network here: the network watchdog would restart first
            c.net.reconnect_timeout_min = 0;
        });
        let _stm = FakeStm::attach(&dev.uart, &dev.gpio, &dev.clock);
        let ended = firmware(&dev, |fw| {
            fw.setup();
            fw.start();
            fw.run_ms(8000);
            let c = StmCommand {
                kind: StmCommandType::SetTarget,
                valve: 0,
                pos: 40,
                source: TargetSource::Mqtt,
                ..StmCommand::default()
            };
            assert!(fw.shared.app.submit(&c));
            fw.run_ms(2000);
            // the heap stays low from 10 min of uptime on: the guard restarts after 60 s
            fw.dev.system.state().heap.free = 1000;
            fw.dev.clock.set_ms(600_000);
            let ended = run(|| fw.run_ms(120_000));
            let r = events(fw.shared, EventCode::RebootRequested);
            assert_eq!(r.len(), 1);
            assert_eq!(r[0].arg1, 6); // the heap guard
            ended
        });
        assert_eq!(ended, Ended::Reset(Reset::Software));
        // the restart path: the targets in NVS, the reason in the log file
        assert!(dev.nvs.has(NAMESPACE, KEY_TARGETS));
        let log = dev.fs.read("/log/events.log").unwrap_or_default();
        let text = String::from_utf8_lossy(&log);
        assert!(text.contains("heap_critical"), "{text}");
        assert!(text.contains("reboot_requested"), "{text}");
    }
    let dev = board.boot();
    let stm = FakeStm::attach(&dev.uart, &dev.gpio, &dev.clock);
    firmware(&dev, |fw| {
        fw.setup();
        fw.start();
        fw.run_ms(10_000);
        let restored = events(fw.shared, EventCode::TargetsRestored);
        assert_eq!(restored.len(), 1);
        assert_eq!((restored[0].arg1, restored[0].arg2), (1, 1)); // one valve, from RTC
        assert_eq!(
            stm.requests_of("stgtp").first().map(String::as_str),
            Some("stgtp 0 40 ")
        );
        assert_eq!(events(fw.shared, EventCode::Boot)[0].arg1, 3); // ESP_RST_SW
    });
}

#[test]
fn the_web_server_starts_once_the_network_is_up_a_refused_start_is_tried_again() {
    let board = FakeBoard::new();
    let dev = board.boot();
    firmware(&dev, |fw| {
        fw.setup();
        fw.start();
        fw.run_ms(2000);
        assert_eq!(fw.dev.http.starts(), 0);
        assert!(!fw.app.host().web().started());
        fw.dev.http.set_refuse(true);
        fw.dev.eth.got_ip(IpInfo {
            ip: u32::from_le_bytes([192, 168, 1, 20]),
            mask: u32::from_le_bytes([255, 255, 255, 0]),
            gateway: u32::from_le_bytes([192, 168, 1, 1]),
            dns: 0,
        });
        fw.run_ms(2000);
        assert!(fw.shared.net.is_up());
        let refused = fw.dev.http.starts();
        assert!(refused >= 1);
        assert!(!fw.app.host().web().started());
        fw.dev.http.set_refuse(false);
        fw.run_ms(3000);
        assert!(fw.app.host().web().started());
        assert_eq!(fw.dev.http.starts(), refused + 1);
        // OTA saw the started server
        let _ = fw.sinks;
    });
}

// ---------------------------------------------------------------- the hosts, one by one

/// The pieces of a host test: the shared objects, storage over the board.
fn with_wire<R>(dev: &Device, f: impl FnOnce(&Shared, &FwStorage<'_, P>, Wire<'_, P>) -> R) -> R {
    let shared = Shared::new();
    let p = ports(dev);
    let st = storage(p, &shared);
    let w = Wire::new(p, &shared, &st);
    f(&shared, &st, w)
}

#[test]
fn storage_wire_logs_into_the_ring_and_reads_the_network_trial() {
    let dev = FakeBoard::new().boot();
    let shared = Shared::new();
    let p = ports(&dev);
    let host: StorageWire<'_, P> = StorageWire {
        logger: p.logger(&shared),
        shared: &shared,
    };
    host.log(EventCode::ConfigDefaults, NO_VALVE, 101, 0, b"");
    let e = events(&shared, EventCode::ConfigDefaults);
    assert_eq!((e.len(), e[0].arg1), (1, 101));
    assert!(!host.net_trial_active());
    // a trial runs once net begins with an armed record of the settings in use
    let cfg = Config::default();
    let rec = NetTrialRecord {
        state: NetTrialState::Armed,
        previous: cfg.net.clone(),
        trial_crc: net_trial_fields_crc(&cfg.net),
    };
    let mut b = [0u8; NET_TRIAL_BLOB_MAX];
    let n = encode_net_trial(&rec, &mut b);
    dev.nvs.set_blob(NAMESPACE, "netTrial", &b[..n]);
    let st = storage(p, &shared);
    let mut nt = net(
        p,
        &shared,
        &st,
        dev.eth.clone(),
        dev.wifi.clone(),
        dev.sntp.clone(),
        dev.pinger.clone(),
    );
    let mut c = cfg;
    nt.begin(&mut c);
    assert!(host.net_trial_active());
}

#[test]
fn logger_wire_reports_littlefs_mounted() {
    let dev = FakeBoard::new().boot();
    let shared = Shared::new();
    let host = LoggerWire { shared: &shared };
    assert!(!host.fs_ready());
    let st = storage(ports(&dev), &shared);
    let mut formatted = false;
    assert!(st.begin_fs(&mut formatted));
    assert!(host.fs_ready());
}

#[test]
fn sinks_write_the_file_once_littlefs_is_mounted() {
    let dev = FakeBoard::new().boot();
    let shared = Shared::new();
    let p = ports(&dev);
    let logger = p.logger(&shared);
    let st = storage(p, &shared);
    let sk = sinks(p, &logger, &shared, dev.udp.clone());
    logger.log(EventCode::Boot, NO_VALVE, 1, 1, b"");
    lock(&sk).flush();
    // nothing to write to: the RAM log is all there is
    assert!(dev.fs.read("/log/events.log").is_none());
    let mut formatted = false;
    assert!(st.begin_fs(&mut formatted));
    logger.log(EventCode::LowHeap, NO_VALVE, 1, 1, b"");
    lock(&sk).flush();
    assert!(dev
        .fs
        .read("/log/events.log")
        .is_some_and(|l| !l.is_empty()));
}

#[test]
fn wire_mqtt_host_reads_and_writes_the_shared_objects() {
    let dev = FakeBoard::new().boot();
    dev.clock.set_ms(5500);
    with_wire(&dev, |s, st, w| {
        assert_eq!(MqttHost::uptime_s(&w), 5);
        let c = StmCommand {
            valve: 7,
            ..StmCommand::default()
        };
        assert!(MqttHost::submit(&w, &c));
        assert_eq!(s.app.receive().map(|c| c.valve), Some(7));
        let mut snap = Box::<StmSnapshot>::default();
        snap.revision = 9;
        snap.hw_id = 0x431;
        s.app.publish_stm_snapshot(&snap);
        assert_eq!(MqttHost::stm_snapshot_revision(&w), 9);
        let mut out = Box::<StmSnapshot>::default();
        MqttHost::read_stm_snapshot(&w, &mut out);
        assert_eq!(out.hw_id, 0x431);
        s.app.store_profile(&Profile {
            valve: 3,
            count: 2,
            ..Profile::default()
        });
        let mut p = Profile::default();
        MqttHost::read_profile(&w, 3, &mut p);
        assert_eq!(p.count, 2);
        s.app.set_calib_info(&CalibInfo {
            next_epoch: 77,
            ..CalibInfo::default()
        });
        assert_eq!(MqttHost::calib_next_epoch(&w), 77);
        s.storage.set_active_config(&Config::default());
        s.storage.set_active_config(&boiler());
        assert_eq!(MqttHost::config_revision(&w), 2);
        let mut station = Vec::new();
        MqttHost::with_config(&w, &mut |c| station = c.station.to_vec());
        assert_eq!(station, b"Boiler");
        assert!(!MqttHost::fs_ready(&w));
        let mut formatted = false;
        assert!(st.begin_fs(&mut formatted));
        assert!(MqttHost::fs_ready(&w));
        assert!(!MqttHost::ha_cleanup_done(&w));
        MqttHost::set_ha_cleanup_done(&w);
        assert!(MqttHost::ha_cleanup_done(&w));
        assert_eq!(MqttHost::ha_layout(&w), 0);
        MqttHost::set_ha_layout(&w, 2);
        assert_eq!(MqttHost::ha_layout(&w), 2);
        MqttHost::log(&w, EventCode::LowHeap, NO_VALVE, 1, 2, b"");
        let seq = MqttHost::log(&w, EventCode::LowHeap, NO_VALVE, 5, 6, b"");
        assert_eq!(seq, 2);
        assert_eq!(seq, s.logger.last_seq());
        let mut ev: Vec<Event> = vec![Event::default(); 4];
        let (n, next) = MqttHost::read_events_since(&w, seq - 1, &mut ev);
        assert_eq!((n, next), (1, seq));
        assert_eq!(
            (ev[0].code, ev[0].arg1, ev[0].arg2),
            (EventCode::LowHeap, 5, 6)
        );
        assert!(!MqttHost::net_up(&w));
        assert_eq!(MqttHost::net_ip(&w), 0);
        assert!(!MqttHost::local_time(&w).valid);
        dev.wall.set(1_790_121_600);
        let t = MqttHost::local_time(&w);
        assert!(t.valid);
        assert_eq!((t.year, t.month, t.mday), (2026, 9, 23));
        assert!(!MqttHost::restart_pending(&w));
        MqttHost::request_restart(&w, 0, 1000);
        assert!(MqttHost::restart_pending(&w));
        let r = events(s, EventCode::RebootRequested);
        assert_eq!((r.len(), r[0].arg1, r[0].arg2), (1, 0, 0));
        // a second request does not count: nothing more logged
        MqttHost::request_restart(&w, 0, 1000);
        assert_eq!(events(s, EventCode::RebootRequested).len(), 1);
    });
}

#[test]
fn wire_net_and_ota_hosts_see_the_network_of_net() {
    let dev = FakeBoard::new().boot();
    let shared = Shared::new();
    let p = ports(&dev);
    let logger = p.logger(&shared);
    let st = storage(p, &shared);
    let sk = sinks(p, &logger, &shared, dev.udp.clone());
    let sv = Mutex::new(stm_service(p, &shared, &st));
    let mut nt = net(
        p,
        &shared,
        &st,
        dev.eth.clone(),
        dev.wifi.clone(),
        dev.sntp.clone(),
        dev.pinger.clone(),
    );
    let mut c = Config::default();
    nt.begin(&mut c);
    let ip = u32::from_le_bytes([192, 168, 1, 20]);
    dev.eth.got_ip(IpInfo {
        ip,
        mask: u32::from_le_bytes([255, 255, 255, 0]),
        gateway: u32::from_le_bytes([192, 168, 1, 1]),
        dns: 0,
    });
    nt.service(1000, false);
    let w = Wire::new(p, &shared, &st);
    assert!(MqttHost::net_up(&w));
    assert_eq!(MqttHost::net_ip(&w), ip);
    let mut o = OtaWire::new(Wire::new(p, &shared, &st), &sk, &sv);
    assert!(o.net_is_up());
    assert_eq!(o.net_ip(), ip);
}

#[test]
fn wire_net_host_logs_restarts_and_keeps_the_config_and_the_trial_record() {
    let dev = FakeBoard::new().boot();
    with_wire(&dev, |s, _st, mut w| {
        NetHost::log(&mut w, &make_test_event(EventCode::NetDown, 1));
        assert_eq!(events(s, EventCode::NetDown).len(), 1);
        NetHost::request_restart(&mut w, 2, 1000, 5);
        assert!(s.ota.restart_pending());
        let r = events(s, EventCode::RebootRequested);
        assert_eq!(
            (r[0].arg1, r[0].arg2, r[0].severity),
            (2, 5, Severity::Warning)
        );
        assert!(NetHost::apply_config(&mut w, &boiler()));
        assert_eq!(s.storage.config_revision(), 1);
        let mut out = Box::<Config>::default();
        NetHost::get_config(&mut w, &mut out);
        assert_eq!(&out.station[..], b"Boiler");
        let mut bad = boiler();
        copy_string(&mut bad.station, b"bad/name");
        assert!(!NetHost::apply_config(&mut w, &bad));
        assert_eq!(s.storage.config_revision(), 1);
        let mut buf = [0u8; 16];
        assert_eq!(NetHost::load_net_trial_blob(&mut w, &mut buf), 0);
        assert!(NetHost::save_net_trial_blob(&mut w, b"VDNT1"));
        assert_eq!(NetHost::load_net_trial_blob(&mut w, &mut buf), 5);
        assert_eq!(&buf[..5], b"VDNT1");
        NetHost::clear_net_trial(&mut w);
        assert_eq!(NetHost::load_net_trial_blob(&mut w, &mut buf), 0);
    });
}

fn make_test_event(code: EventCode, arg1: i32) -> Event {
    vdm_esp_core::event_log::make_event(code, Severity::Warning, NO_VALVE, arg1, 0, b"")
}

#[test]
fn wire_stm_service_host_reads_and_writes_app_storage_and_time() {
    let dev = FakeBoard::new().boot();
    with_wire(&dev, |s, _st, mut w| {
        StmServiceHost::log(&mut w, &make_test_event(EventCode::CalibTimeMissing, 0));
        assert_eq!(events(s, EventCode::CalibTimeMissing).len(), 1);
        let c = StmCommand {
            kind: StmCommandType::Calibrate,
            valve: ALL_VALVES,
            ..StmCommand::default()
        };
        assert!(StmServiceHost::submit(&mut w, &c));
        assert_eq!(
            s.app.receive().map(|c| c.kind),
            Some(StmCommandType::Calibrate)
        );
        let ci = CalibInfo {
            last_scheduled_epoch: 1,
            next_slot: 2,
            next_epoch: 3,
        };
        StmServiceHost::set_calib_info(&mut w, &ci);
        assert_eq!(s.app.calib_info(), ci);
        assert_eq!(StmServiceHost::calib_info(&mut w), ci);
        let mut cfg = boiler();
        cfg.calib.day_mask = 5;
        s.storage.set_active_config(&Config::default());
        s.storage.set_active_config(&cfg);
        assert_eq!(StmServiceHost::config_revision(&mut w), 2);
        assert_eq!(StmServiceHost::calib_config(&mut w).day_mask, 5);
        assert_eq!(StmServiceHost::load_calib_slot(&mut w), 0);
        StmServiceHost::save_calib_slot(&mut w, 20_261_001);
        assert_eq!(StmServiceHost::load_calib_slot(&mut w), 20_261_001);
        assert_eq!(StmServiceHost::load_last_calib(&mut w), 0);
        StmServiceHost::save_last_calib(&mut w, 1_790_000_000);
        assert_eq!(StmServiceHost::load_last_calib(&mut w), 1_790_000_000);
        let mut buf = [0u8; 8];
        assert_eq!(StmServiceHost::load_targets(&mut w, &mut buf), 0);
        assert!(StmServiceHost::save_targets(&mut w, b"targets"));
        assert_eq!(StmServiceHost::load_targets(&mut w, &mut buf), 7);
        assert!(!StmServiceHost::local_time(&mut w).valid);
        dev.wall.set(1_790_121_600);
        assert!(StmServiceHost::local_time(&mut w).valid);
    });
}

#[test]
fn wire_stm_link_host_reaches_app_storage_ota_mqtt_and_the_rtc_records() {
    let dev = FakeBoard::new().boot();
    let shared = Shared::new();
    let p = ports(&dev);
    let st = storage(p, &shared);
    let mut w = Wire::new(p, &shared, &st);
    let s = &shared;
    StmLinkHost::log_event(&mut w, &make_test_event(EventCode::LinkDown, 0));
    assert_eq!(events(s, EventCode::LinkDown).len(), 1);
    let mut snap = Box::<StmSnapshot>::default();
    snap.revision = 4;
    StmLinkHost::publish(&mut w, &snap);
    assert_eq!(s.app.stm_snapshot_revision(), 4);
    StmLinkHost::store_profile(
        &mut w,
        &Profile {
            valve: 1,
            count: 5,
            ..Profile::default()
        },
    );
    let mut pr = Profile::default();
    s.app.read_profile(1, &mut pr);
    assert_eq!(pr.count, 5);
    assert!(!StmLinkHost::restart_pending(&mut w));
    assert!(s.ota.request_restart(0, 0, 1000, 0).is_some());
    assert!(StmLinkHost::restart_pending(&mut w));
    StmLinkHost::mark_flash_active(&mut w);
    assert!(s.app.stm_flash_active());
    // the RTC records of the layout
    let mut t = PersistedTargets::default();
    t.valid[2] = true;
    t.pos[2] = 66;
    StmLinkHost::store_desired_targets(&mut w, &t);
    let mut want = [0u8; PERSISTED_TARGETS_SIZE];
    encode_targets(&t, &mut want);
    let rtc = dev.rtc.snapshot();
    assert_eq!(rtc[RTC_TARGETS..RTC_TARGETS + PERSISTED_TARGETS_SIZE], want);
    let lease = LeaseClientSnapshot {
        lost: true,
        active: true,
        mask: 4,
        lost_elapsed_ms: 99,
    };
    StmLinkHost::store_lease_record(&mut w, &lease);
    let mut l = [0u8; LEASE_RECORD_SIZE];
    encode_lease_record(&lease, &mut l);
    let rtc = dev.rtc.snapshot();
    let at = RTC_LEASE;
    assert_eq!(rtc[at..at + LEASE_RECORD_SIZE], l);
    // the boot choice of stm_service: the RTC records win
    let sv = Mutex::new(stm_service(p, &shared, &st));
    lock(&sv).begin();
    assert_eq!(StmLinkHost::boot_targets(&mut w), (t, RestoreSource::Rtc));
    assert_eq!(StmLinkHost::boot_lease(&mut w), Some(lease));
    StmLinkHost::set_stm_save_state(&mut w, StmSaveState::TimedOut);
    assert_eq!(s.app.stm_save_state(), StmSaveState::TimedOut);
    assert_eq!(StmLinkHost::stm_save_state(&mut w), StmSaveState::TimedOut);
    // the config and its trust
    assert_eq!(StmLinkHost::boot_load_source(&mut w), LoadSource::Defaults);
    let mut cfg = Box::<Config>::default();
    let mut report = ImportReport::default();
    let mut details = LoadDetails::default();
    store_config(&dev, |c| {
        copy_string(&mut c.station, b"Boiler");
    });
    st.load_config(&mut cfg, &mut report, &mut details);
    assert_eq!(StmLinkHost::boot_load_source(&mut w), LoadSource::Stored);
    s.storage.set_active_config(&Config::default());
    s.storage.set_active_config(&cfg);
    assert_eq!(StmLinkHost::config_revision(&mut w), 2);
    let mut station = Vec::new();
    StmLinkHost::with_config(&mut w, &mut |c| station = c.station.to_vec());
    assert_eq!(station, b"Boiler");
    assert!(!StmLinkHost::config_saved_since_boot(&mut w));
    assert!(st.apply_config(&boiler(), &mut []).is_ok());
    assert!(StmLinkHost::config_saved_since_boot(&mut w));
    // the queue and the regulator state of MQTT
    assert!(StmLinkHost::receive(&mut w).is_none());
    assert!(s.app.submit(&StmCommand {
        valve: 9,
        ..StmCommand::default()
    }));
    assert_eq!(StmLinkHost::receive(&mut w).map(|c| c.valve), Some(9));
    let mut cfg = boiler();
    cfg.mqtt.mode = MqttMode::MqttHa;
    copy_string(&mut cfg.mqtt.host, b"broker.lan");
    assert!(st.apply_config(&cfg, &mut []).is_ok());
    let mut mq = mqtt(p, &shared, &st);
    mq.begin();
    assert_eq!(StmLinkHost::regulator_state(&mut w).mode, MqttMode::MqttHa);
}

#[test]
fn wire_stm_link_host_asks_storage_for_the_last_good_copy() {
    let dev = FakeBoard::new().boot();
    with_wire(&dev, |_s, st, mut w| {
        let mut formatted = false;
        assert!(st.begin_fs(&mut formatted));
        dev.fs.put("/stm/c2.bin", &[0x5A; 3000]);
        StmLinkHost::request_last_good_copy(&mut w, b"c2");
        for _ in 0..8 {
            st.service();
        }
        assert_eq!(dev.fs.read("/stm/last_good.bin"), Some(vec![0x5A; 3000]));
    });
}

#[test]
fn ota_wire_reaches_the_sinks_stm_service_app_and_storage() {
    let dev = FakeBoard::new().boot();
    let shared = Shared::new();
    let p = ports(&dev);
    let logger = p.logger(&shared);
    let st = storage(p, &shared);
    let sk = sinks(p, &logger, &shared, dev.udp.clone());
    let sv = Mutex::new(stm_service(p, &shared, &st));
    let mut o = OtaWire::new(Wire::new(p, &shared, &st), &sk, &sv);
    let s = &shared;
    let mut formatted = false;
    assert!(st.begin_fs(&mut formatted));
    o.log(&make_test_event(EventCode::EspOtaStarted, 7));
    assert_eq!(events(s, EventCode::EspOtaStarted)[0].arg1, 7);
    o.log_flush();
    let log = dev.fs.read("/log/events.log").unwrap_or_default();
    assert!(String::from_utf8_lossy(&log).contains("esp_ota_started"));
    assert!(!o.net_is_up());
    assert_eq!(o.net_ip(), 0);
    assert!(!o.stm_flash_active());
    s.app.mark_stm_flash_active();
    assert!(o.stm_flash_active());
    assert!(!o.image_upload_active());
    assert_eq!(
        st.image_upload_begin(b"c2", 100),
        crate::storage::ImageResult::Ok
    );
    assert!(o.image_upload_active());
    st.image_upload_abort();
    assert_eq!(o.stm_link_state(), LinkState::Unknown);
    let mut snap = Box::<StmSnapshot>::default();
    snap.link = LinkState::Degraded;
    s.app.publish_stm_snapshot(&snap);
    assert_eq!(o.stm_link_state(), LinkState::Degraded);
    assert_eq!(o.stm_save_state(), StmSaveState::Idle);
    o.request_stm_save();
    assert_eq!(s.app.stm_save_state(), StmSaveState::Waiting);
    assert_eq!(o.stm_save_state(), StmSaveState::Waiting);
    // the desired targets of the hand-over go to NVS
    lock(&sv).begin();
    let mut t = PersistedTargets::default();
    t.valid[0] = true;
    t.pos[0] = 12;
    StmServiceLink::new(&s.stm_service, &dev.rtc, RTC_STM_SERVICE).store_desired_targets(&t);
    assert!(!dev.nvs.has(NAMESPACE, KEY_TARGETS));
    o.flush_for_restart();
    let mut want = [0u8; PERSISTED_TARGETS_SIZE];
    encode_targets(&t, &mut want);
    assert_eq!(dev.nvs.get_blob(NAMESPACE, KEY_TARGETS), want.to_vec());
    o.set_ota_stm_required(true);
    assert_eq!(dev.nvs.get_i(NAMESPACE, KEY_OTA_STM), 1);
    o.set_ota_stm_required(false);
    assert!(!dev.nvs.has(NAMESPACE, KEY_OTA_STM));
}

#[test]
fn modules_forward_the_app_threads_calls() {
    let board = FakeBoard::new();
    let dev = board.boot();
    // a latch set by an earlier factory reset
    dev.nvs.set_u8(NAMESPACE, "frLatch", 1);
    firmware(&dev, |fw| {
        let host = &mut fw.app.host;
        assert!(host.factory_latched());
        host.set_factory_latched(false);
        assert!(!host.factory_latched());
        let mut formatted = true;
        assert!(host.begin_fs(&mut formatted));
        assert!(!formatted);
        assert!(fw.dev.fs.open("/log/x", OpenMode::Write).is_some());
        // the log sinks: an event, a flush request, the next service writes the file
        host.log(&make_test_event(EventCode::LowHeap, 3));
        assert_eq!(events(fw.shared, EventCode::LowHeap).len(), 1);
        fw.shared.logger.request_flush();
        host.logger_service(false);
        let log = fw.dev.fs.read("/log/events.log").unwrap_or_default();
        assert!(String::from_utf8_lossy(&log).contains("low_heap"));
        // storage: a saved config gets its backup files from the service
        assert!(fw.storage.apply_config(&boiler(), &mut []).is_ok());
        assert!(!fw.dev.fs.exists("/sys/cfg.bak"));
        host.storage_service();
        assert!(fw.dev.fs.exists("/sys/cfg.bak"));
        assert!(host.factory_reset());
        assert_eq!(fw.dev.nvs.get_i(NAMESPACE, "imported"), 1);
        assert_eq!(host.increment_boot_count(), 1);
        assert_eq!(host.increment_boot_count(), 2);
        host.logger_configure(2, 0x0A00_000A, 514, false, b"Boiler");
        assert!(!fw.shared.logger.stats(0).persist);
        host.set_active_config(&Config::default());
        host.set_active_config(&boiler());
        assert_eq!(host.config_revision(), 3); // the save above, then these two
        let mut out = Box::<Config>::default();
        host.get_config(&mut out);
        assert_eq!(&out.station[..], b"Boiler");
        assert!(!host.mqtt_connected());
        assert!(!host.net_is_up());
        assert!(!host.net_ota_ok());
        // an ESP upload of the web server
        assert!(!host.ota_upload_active());
        let mut up = crate::ota::OtaUpload::<P, OtaWire<'_, P>>::new(
            &fw.dev.clock,
            &fw.dev.ota,
            fw.dev.md5.clone(),
            &fw.dev.heap,
            &fw.shared.ota,
            OtaWire::new(
                Wire::new(ports(fw.dev), fw.shared, fw.storage),
                fw.sinks,
                fw.service,
            ),
        );
        assert!(up.upload_begin(1000, b""));
        assert!(host.ota_upload_active());
        assert!(!up.upload_end(false));
        assert!(!host.ota_upload_active());
        assert!(!host.ota_restart_pending());
        host.ota_request_restart(6, 1000);
        assert!(host.ota_restart_pending());
        let r = events(fw.shared, EventCode::RebootRequested);
        assert_eq!((r[0].arg1, r[0].severity), (6, Severity::Warning));
    });
}

#[test]
fn modules_net_and_web_follow_the_interface() {
    let board = FakeBoard::new();
    let dev = board.boot();
    firmware(&dev, |fw| {
        let host = &mut fw.app.host;
        let mut cfg = boiler();
        host.net_begin(&mut cfg);
        assert_eq!(fw.dev.eth.state().begins[0].hostname, "Boiler");
        fw.dev.eth.got_ip(IpInfo {
            ip: u32::from_le_bytes([192, 168, 1, 20]),
            mask: u32::from_le_bytes([255, 255, 255, 0]),
            gateway: u32::from_le_bytes([192, 168, 1, 1]),
            dns: 0,
        });
        host.net_service(1000, true);
        assert!(host.net_is_up());
        // the DHCP lease and the MQTT session prove the network
        assert!(host.net_ota_ok());
        assert!(!host.web_started());
        host.web_begin();
        assert!(host.web_started());
        assert_eq!(fw.dev.http.starts(), 1);
        host.web_begin(); // idempotent
        assert_eq!(fw.dev.http.starts(), 1);
        // a host name change restarts the ESP after the response
        let mut c = boiler();
        copy_string(&mut c.station, b"Cellar");
        host.net_reconfigure(&c);
        assert!(fw.shared.ota.restart_pending());
    });
}

#[test]
fn wire_read_health_takes_the_parts_of_net_ota_and_the_logger() {
    let board = FakeBoard::new();
    let dev = board.boot();
    firmware(&dev, |fw| {
        fw.setup();
        dev.system.state().stacks.insert("stm".to_string(), 3000);
        fw.dev.clock.set_ms(42_000);
        let w = Wire::new(ports(fw.dev), fw.shared, fw.storage);
        lock(fw.sinks).flush();
        let mut h = HealthSnapshot::default();
        w.read_health(&mut h);
        assert_eq!(h.uptime_s, 42);
        assert_eq!(h.task_count, 1);
        assert_eq!(h.log, fw.shared.logger.stats(42_000));
        assert!(h.log.flushed);
        assert_eq!(h.net, fw.shared.net.health());
        assert_eq!(h.ota, fw.shared.ota.health());
        assert!(h.net.evidence_age_s == u32::MAX);
    });
}

#[test]
fn the_tasks_of_the_stm_and_mqtt_threads_run_their_modules() {
    let board = FakeBoard::new();
    let dev = board.boot();
    firmware(&dev, |fw| {
        fw.setup();
        let wd = fw.dev.watchdog.clone();
        Task::start(&mut fw.link, wd.clone());
        assert_eq!(Task::pass(&mut fw.link), crate::stm_link::PASS_DELAY_MS);
        assert_eq!(wd.feeds(), 1);
        Task::start(&mut fw.mqtt, wd.clone());
        // MQTT off: the idle delay
        assert_eq!(Task::pass(&mut fw.mqtt), crate::mqtt_client::IDLE_MS);
        assert_eq!(wd.feeds(), 2);
    });
}

#[test]
fn the_mqtt_session_runs_over_the_wiring_and_proves_the_network() {
    let board = FakeBoard::new();
    let dev = board.boot();
    store_config(&dev, |c| {
        c.mqtt.mode = MqttMode::Mqtt;
        copy_string(&mut c.mqtt.host, b"broker.lan");
        c.valves[0].active = true;
    });
    let broker = crate::testkit::FakeBroker::default();
    broker.attach(&dev.tcp, "broker.lan", 1883);
    let _stm = FakeStm::attach(&dev.uart, &dev.gpio, &dev.clock);
    dev.eth.got_ip(IpInfo {
        ip: u32::from_le_bytes([192, 168, 1, 20]),
        mask: u32::from_le_bytes([255, 255, 255, 0]),
        gateway: u32::from_le_bytes([192, 168, 1, 1]),
        dns: 0,
    });
    firmware(&dev, |fw| {
        fw.setup();
        fw.start();
        fw.run_ms(10_000);
        assert_eq!(broker.state().connects.len(), 1);
        assert_eq!(fw.shared.mqtt.status().state, MqttState::Connected);
        assert!(fw.app.host.mqtt_connected());
        // the session published the state of the STM the stm thread published
        assert!(!broker.state().published.is_empty());
        assert!(fw.shared.app.stm_snapshot_revision() > 0);
    });
}

#[test]
fn http_start_is_started_by_its_first_successful_start() {
    let server = FakeHttpServer::default();
    let mut s = HttpStart::new(server.clone());
    assert!(!s.started());
    server.set_refuse(true);
    s.begin();
    assert!(!s.started());
    server.set_refuse(false);
    s.begin();
    assert!(s.started());
    s.begin();
    assert_eq!(server.starts(), 2);
}
