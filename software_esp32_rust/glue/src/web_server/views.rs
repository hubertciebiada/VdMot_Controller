//! The documents of the GET handlers (C++ `handleStatus`, `buildValveViews`,
//! `buildSensorViews`, `handleEvents`, `handleImages`, `handleHealth`, `handleLog`,
//! `handleStatic`, ...): views of the snapshot and the config copy, the lists of storage and
//! the logger, the import report, the log download and the dashboard files.

use vdm_esp_core::common::{elapsed_ms, fmt_fit, VOLT_SLOT_COUNT};
use vdm_esp_core::config::ValveConfig;
use vdm_esp_core::event_log::parse_severity;
use vdm_esp_core::file_manager::{write_files_json, FileEntry};
use vdm_esp_core::json_api::{
    write_events_json, write_flash_status_json, write_health_json, write_motor_json,
    write_profile_json, write_sensors_json, write_status_json, write_valves_json, SensorView,
    StatusSnapshot, ValveView,
};
use vdm_esp_core::legacy_http::{
    write_legacy_temps_json, write_legacy_valves_json, write_legacy_volts_json,
};
use vdm_esp_core::valve_model::{temp_raw_valid, vad_valid, TempReading, ValveState, VoltReading};
use vdm_esp_core::version::firmware_version;

use super::*;
use crate::http_parse::has_query_param;
use crate::logger::{EVENT_CAPACITY, LOG_FILE, LOG_FILE_OLD};
use crate::net::local_time;
use crate::port::{FsFile, OpenMode, System, WallClock};
use crate::storage::{ImageEntry, IMAGE_SLOTS, IMPORT_REPORT_FILE};

/// Events read for one answer: the ring holds at most [`EVENT_CAPACITY`] events (32 in the
/// firmware), so no read fills more.
const EVENTS_BUFFER: usize = if (MAX_EVENTS_PER_RESPONSE as usize) < EVENT_CAPACITY {
    MAX_EVENTS_PER_RESPONSE as usize
} else {
    EVENT_CAPACITY
};
/// Sensor views: every temperature and voltage slot, and as many bus sensors without one.
const SENSOR_VIEWS: usize = 2 * (TEMP_SLOT_COUNT as usize + VOLT_SLOT_COUNT as usize);
/// The download name of the config export.
const CONFIG_EXPORT: &str = "attachment; filename=\"vdmot-config.json\"";
/// The download name of the log.
const LOG_DOWNLOAD: &str = "attachment; filename=\"vdmot-events.log\"";

/// One image of GET /api/stm/images and of an upload's answer (C++ `writeImage`): the CRC only
/// when known, version and check only once the image was scanned, the board tag when it has
/// one.
pub(super) fn write_image(jw: &mut JsonWriter<'_>, e: &ImageEntry, crc_known: bool) {
    jw.begin_object();
    jw.kv("name", c_str(&e.name));
    jw.kv("size", e.size);
    jw.key("crc32");
    if crc_known {
        let mut buf = [0u8; 12];
        let n = fmt_fit(&mut buf, format_args!("0x{:08x}", e.crc));
        jw.value(buf.get(..n).unwrap_or_default());
    } else {
        jw.null_value();
    }
    jw.key("version");
    let version = c_str(&e.version);
    if e.scanned && !version.is_empty() {
        jw.value(version);
    } else {
        jw.null_value();
    }
    jw.key("check");
    if e.scanned {
        jw.value(flash_error_name(e.check));
    } else {
        jw.null_value();
    }
    jw.key("hw");
    let hw = c_str(&e.hw_tag);
    if hw.is_empty() {
        jw.null_value();
    } else {
        jw.value(hw);
    }
    jw.end_object();
}

/// A reading is valid when seen, of a valid value, and not older than [`SENSOR_STALE_MS`].
fn fresh(seen: bool, value_ok: bool, now: u32, last_seen_ms: u32) -> bool {
    seen && value_ok && elapsed_ms(now, last_seen_ms) <= SENSOR_STALE_MS
}

/// Seconds since the reading was seen, 0 when never seen.
fn age_s(seen: bool, now: u32, last_seen_ms: u32) -> u32 {
    if seen {
        elapsed_ms(now, last_seen_ms) / 1000
    } else {
        0
    }
}

/// The milli value of a voltage reading: (vad / 100 + offset) x factor, clamped to +-2e9.
fn volt_milli(vad: i32, offset: f32, factor: f32) -> i32 {
    let milli = (f64::from(vad) / 100.0 + f64::from(offset)) * f64::from(factor) * 1000.0;
    milli.clamp(-2e9, 2e9) as i32
}

/// The query of GET /api/events into `f`: `since`, `limit` 1..50 (default 50), `valve` 1..12,
/// `minSeverity`; the limit, or `None` when a bad value was answered (400).
fn event_query(req: &mut dyn HttpRequest, f: &mut EventFilter) -> Option<u32> {
    let mut limit = MAX_EVENTS_PER_RESPONSE;
    let mut valve = 0;
    if !query_uint(req, b"since", u32::MAX, &mut f.since_seq)
        || !query_uint(req, b"limit", MAX_EVENTS_PER_RESPONSE, &mut limit)
        || limit == 0
        || !query_uint(req, b"valve", u32::from(VALVE_COUNT), &mut valve)
    {
        send_error(req, 400, "bad_request", b"since/limit/valve");
        return None;
    }
    if has_query_param(split_target(req.target()).1, b"valve") {
        if valve == 0 {
            send_error(req, 400, "bad_request", b"valve");
            return None;
        }
        f.valve = valve as u8 - 1;
    }
    let mut sev = [0u8; QUERY_MAX];
    if let Some(v) = query(req, b"minSeverity", &mut sev) {
        let Some(s) = parse_severity(v) else {
            send_error(req, 400, "bad_request", b"minSeverity");
            return None;
        };
        f.min_severity = s;
    }
    Some(limit)
}

/// The STM members of the status document from the snapshot.
fn status_stm(s: &mut StatusSnapshot<'_>, snap: &StmSnapshot) {
    s.link = snap.link;
    s.link_stats = snap.link_stats;
    s.stm_proto = snap.proto;
    s.stm_version = snap.version.clone();
    s.stm_build = snap.build;
    s.stm_hw_id = snap.hw_id;
    s.stm_compatible = snap.compatible;
    s.have_stm_status = snap.have_status;
    s.stm_status = snap.status;
    s.esp_line_overflows = snap.line_overflows;
    s.esp_line_malformed = snap.line_malformed;
    s.calibration_active = snap.valves.iter().any(|v| v.calibrating);
    s.stm_support = snap.support;
    s.lease = snap.lease;
    s.have_learn_time = snap.have_learn_time;
    s.learn_time_s = snap.learn_time_s;
}

/// Sensor position `k` of a valve's view: the configured temperature slot `slot` (1-based, 0 =
/// none) with its name, and the reading `raw` of the valve with the slot's offset.
fn valve_sensor<'v>(v: &mut ValveView<'v>, k: usize, slot: u8, raw: i16, cfg: &'v Config) {
    let Some(t) = slot
        .checked_sub(1)
        .and_then(|s| cfg.temps.get(usize::from(s)))
    else {
        return;
    };
    v.sensor_slot[k] = slot;
    v.sensor_name[k] = &t.name;
    v.sensor_valid[k] = temp_raw_valid(raw);
    v.sensor_tenths[k] = i32::from(raw) + i32::from(t.offset);
}

/// The views of the configured temperature slots (empty ones skipped): the reading of the bus
/// sensor with the slot's id, and the valve that uses the slot.
fn temp_slot_views<'v>(
    views: &mut Vec<SensorView<'v>>,
    snap: &StmSnapshot,
    cfg: &'v Config,
    bus: &[TempReading],
    now: u32,
) {
    for (slot, c) in (1u8..).zip(cfg.temps.iter()) {
        if is_zero(&c.id) && !c.active && c_str(&c.name).is_empty() {
            continue;
        }
        let mut v = SensorView {
            slot,
            name: &c.name,
            active: c.active,
            id: c.id,
            ..SensorView::default()
        };
        if let Some(r) = bus.iter().find(|r| !is_zero(&c.id) && r.id == c.id) {
            v.on_bus = true;
            v.raw = i32::from(r.raw);
            v.value = i32::from(r.raw) + i32::from(c.offset);
            v.valid = fresh(r.seen, temp_raw_valid(r.raw), now, r.last_seen_ms);
            v.age_s = age_s(r.seen, now, r.last_seen_ms);
        }
        if let Some(k) = snap
            .valves
            .iter()
            .position(|st| st.sensor_slot.contains(&slot))
        {
            v.valve = k as u8;
        }
        views.push(v);
    }
}

/// The views of the temperature sensors on the bus without a slot.
fn bus_temp_views(views: &mut Vec<SensorView<'_>>, cfg: &Config, bus: &[TempReading], now: u32) {
    for r in bus {
        if is_zero(&r.id) || cfg.temps.iter().any(|c| c.id == r.id) {
            continue;
        }
        views.push(SensorView {
            on_bus: true,
            id: r.id,
            raw: i32::from(r.raw),
            value: i32::from(r.raw),
            valid: fresh(r.seen, temp_raw_valid(r.raw), now, r.last_seen_ms),
            age_s: age_s(r.seen, now, r.last_seen_ms),
            ..SensorView::default()
        });
    }
}

/// The views of the configured voltage slots (empty ones skipped) with the reading of the bus
/// sensor with the slot's id.
fn volt_slot_views<'v>(
    views: &mut Vec<SensorView<'v>>,
    cfg: &'v Config,
    bus: &[VoltReading],
    now: u32,
) {
    for (slot, c) in (1u8..).zip(cfg.volts.iter()) {
        if is_zero(&c.id) && !c.active && c_str(&c.name).is_empty() {
            continue;
        }
        let mut v = SensorView {
            slot,
            name: &c.name,
            active: c.active,
            id: c.id,
            unit: &c.unit,
            ..SensorView::default()
        };
        if let Some(r) = bus.iter().find(|r| !is_zero(&c.id) && r.id == c.id) {
            v.on_bus = true;
            v.raw = r.vad;
            v.value = volt_milli(r.vad, c.offset, c.factor);
            v.valid = fresh(r.seen, vad_valid(r.vad), now, r.last_seen_ms);
            v.age_s = age_s(r.seen, now, r.last_seen_ms);
        }
        views.push(v);
    }
}

/// The views of the voltage sensors on the bus without a slot: no offset, factor or unit.
fn bus_volt_views(views: &mut Vec<SensorView<'_>>, cfg: &Config, bus: &[VoltReading], now: u32) {
    for r in bus {
        if is_zero(&r.id) || cfg.volts.iter().any(|c| c.id == r.id) {
            continue;
        }
        views.push(SensorView {
            on_bus: true,
            id: r.id,
            raw: r.vad,
            age_s: age_s(r.seen, now, r.last_seen_ms),
            ..SensorView::default()
        });
    }
}

impl<'a, P, N, F, G, S, O, H> Web<'a, P, N, F, G, S, O, H>
where
    P: Platform,
    N: Nvs,
    F: Fs,
    G: HeapGate,
    S: StorageHost,
    O: OtaHost,
    H: WebHost,
{
    /// A JSON document built by `build` into the response buffer (C++ `sendDocument`): 500
    /// `internal` "document too large" when it does not fit, 503 without the buffer.
    fn document(
        &self,
        req: &mut dyn HttpRequest,
        response: &mut Option<Vec<u8>>,
        attachment: Option<&str>,
        build: impl FnOnce(&mut JsonWriter<'_>) -> bool,
    ) {
        let Some(buf) = response_buf(response, self.ports.gate) else {
            return out_of_memory(req);
        };
        let mut jw = JsonWriter::new(buf);
        if !build(&mut jw) || !jw.complete() {
            return send_error(req, 500, "internal", b"document too large");
        }
        send_document(req, 200, jw.as_bytes(), attachment);
    }

    // ------------------------------------------------------------ status
    //
    // The status document is filled in parts (C++ `handleStatus`): one function for all of it
    // would exceed the stack frame limit.

    /// GET /api/status.
    pub(super) fn status(&self, req: &mut dyn HttpRequest, w: &mut WorkSet) {
        self.host.read_stm_snapshot(&mut w.snap);
        let mut s = StatusSnapshot::default();
        self.status_esp(&mut s);
        self.status_net(&mut s);
        status_stm(&mut s, &w.snap);
        self.status_config(&mut s, &w.cfg);
        self.document(req, &mut w.response, None, |jw| write_status_json(jw, &s));
    }

    /// The firmware, the chip, the heap, the image and the counters.
    fn status_esp(&self, s: &mut StatusSnapshot<'_>) {
        let heap = self.ports.system.heap();
        s.esp_version = Some(firmware_version().as_bytes());
        s.build_epoch = BUILD_EPOCH;
        s.uptime_s = self.ports.clock.uptime_s();
        s.reset_reason = self.ports.system.reset_reason();
        s.boot_count = self.storage.shared().boot_count();
        s.free_heap = heap.free;
        s.min_free_heap = heap.min_free;
        s.largest_free_block = heap.largest;
        s.sketch_size = self.ports.ota.running_image_size();
        s.sketch_space = self.ports.ota.running().size;
        s.last_event_seq = self.host.last_event_seq();
        s.import_report = self.storage.has_import_report();
    }

    /// The time, the network and MQTT.
    fn status_net(&self, s: &mut StatusSnapshot<'_>) {
        let local = local_time(self.ports.wall);
        s.time_valid = local.valid;
        s.epoch = local.epoch;
        s.local = local;
        s.last_sync_epoch = self.host.last_sync_epoch();
        let ni = self.host.net_info();
        s.net = ni.state;
        s.ip = ni.ip;
        s.mask = ni.mask;
        s.gateway = ni.gateway;
        s.dns = ni.dns;
        s.mac = ni.mac;
        s.wifi_rssi = ni.rssi;
        let trial = self.host.net_trial();
        s.net_trial_active = trial.active;
        s.net_trial_remain_s = trial.remain_s;
        let ms = self.host.mqtt_status();
        s.mqtt = ms.state;
        s.mqtt_rc = ms.rc;
        s.mqtt_reconnects = ms.reconnects;
        s.mqtt_publish_failures = ms.publish_failures;
        s.mqtt_client_id = ms.client_id;
        s.mqtt_ha_status = ms.ha_status;
    }

    /// The station, the config load and the calibration schedule.
    fn status_config<'c>(&self, s: &mut StatusSnapshot<'c>, cfg: &'c Config) {
        let mut hostname = [0u8; STATION_NAME_MAX + 1];
        let n = build_hostname(&cfg.station, &mut hostname);
        s.hostname = Text::from_slice(hostname.get(..n).unwrap_or_default()).unwrap_or_default();
        s.station = &cfg.station;
        let shared = self.storage.shared();
        let details = shared.boot_load_details();
        s.config_source = shared.boot_load_source() as u8;
        s.config_repairs = details.info.repairs.mask;
        s.config_newer_schema = details.info.decode.newer_schema;
        let ci = self.host.calib_info();
        s.last_scheduled_calib_epoch = ci.last_scheduled_epoch;
        s.next_calib_slot = ci.next_slot;
        s.next_calib_epoch = ci.next_epoch;
        let next_local = if ci.next_epoch > 0 {
            self.ports.wall.local_time(ci.next_epoch)
        } else {
            None
        };
        s.next_calib_local = next_local.map_or_else(LocalTime::default, |t| LocalTime {
            valid: true,
            epoch: ci.next_epoch,
            ..t
        });
    }

    // ------------------------------------------------------------ valves and sensors

    /// The views of the 12 valves with their sensors resolved (C++ `buildValveViews`); `None`
    /// without memory for them.
    fn valve_views<'v>(
        &self,
        snap: &'v StmSnapshot,
        cfg: &'v Config,
    ) -> Option<Vec<ValveView<'v>>> {
        let mut views = scratch(self.ports.gate, usize::from(VALVE_COUNT))?;
        for (i, (st, vc)) in (0u8..).zip(snap.valves.iter().zip(cfg.valves.iter())) {
            views.push(self.valve_view(i, st, vc, cfg));
        }
        Some(views)
    }

    /// The view of valve `i`: its state and config, its sensors, the end of its last
    /// calibration.
    fn valve_view<'v>(
        &self,
        i: u8,
        st: &'v ValveState,
        vc: &'v ValveConfig,
        cfg: &'v Config,
    ) -> ValveView<'v> {
        let mut v = ValveView {
            state: Some(st),
            config: Some(vc),
            ..ValveView::default()
        };
        valve_sensor(&mut v, 0, st.sensor_slot[0], st.temp1, cfg);
        valve_sensor(&mut v, 1, st.sensor_slot[1], st.temp2, cfg);
        v.calibration_end = self
            .host
            .calibration_end(i)
            .map_or_else(LocalTime::default, |t| LocalTime { valid: true, ..t });
        v
    }

    /// GET /api/valves.
    pub(super) fn valves(&self, req: &mut dyn HttpRequest, w: &mut WorkSet) {
        self.host.read_stm_snapshot(&mut w.snap);
        let Some(views) = self.valve_views(&w.snap, &w.cfg) else {
            return out_of_memory(req);
        };
        let now = self.ports.clock.now_ms();
        self.document(req, &mut w.response, None, |jw| {
            write_valves_json(jw, &views, now)
        });
    }

    /// GET /valves (legacy).
    pub(super) fn legacy_valves(&self, req: &mut dyn HttpRequest, w: &mut WorkSet) {
        self.host.read_stm_snapshot(&mut w.snap);
        let Some(views) = self.valve_views(&w.snap, &w.cfg) else {
            return out_of_memory(req);
        };
        self.document(req, &mut w.response, None, |jw| {
            write_legacy_valves_json(jw, &views)
        });
    }

    /// The temperature views (configured slots, then bus sensors without a slot) followed by
    /// the voltage views, and the number of temperature views (C++ `buildSensorViews`); `None`
    /// without memory for them.
    fn sensor_views<'v>(
        &self,
        snap: &'v StmSnapshot,
        cfg: &'v Config,
    ) -> Option<(Vec<SensorView<'v>>, usize)> {
        let mut views = scratch(self.ports.gate, SENSOR_VIEWS)?;
        let now = self.ports.clock.now_ms();
        let bus_temps = snap
            .temps
            .get(..usize::from(snap.temp_count.min(TEMP_SLOT_COUNT)))
            .unwrap_or_default();
        let bus_volts = snap
            .volts
            .get(..usize::from(snap.volt_count.min(VOLT_SLOT_COUNT)))
            .unwrap_or_default();
        temp_slot_views(&mut views, snap, cfg, bus_temps, now);
        bus_temp_views(&mut views, cfg, bus_temps, now);
        let temps = views.len();
        volt_slot_views(&mut views, cfg, bus_volts, now);
        bus_volt_views(&mut views, cfg, bus_volts, now);
        Some((views, temps))
    }

    /// GET /api/sensors.
    pub(super) fn sensors(&self, req: &mut dyn HttpRequest, w: &mut WorkSet) {
        self.host.read_stm_snapshot(&mut w.snap);
        let Some((views, temps)) = self.sensor_views(&w.snap, &w.cfg) else {
            return out_of_memory(req);
        };
        let (t, v) = views.split_at(temps.min(views.len()));
        self.document(req, &mut w.response, None, |jw| {
            write_sensors_json(jw, t, v)
        });
    }

    /// GET /temps and /volts (legacy).
    pub(super) fn legacy_sensors(&self, req: &mut dyn HttpRequest, w: &mut WorkSet, temps: bool) {
        self.host.read_stm_snapshot(&mut w.snap);
        let Some((views, n)) = self.sensor_views(&w.snap, &w.cfg) else {
            return out_of_memory(req);
        };
        let (t, v) = views.split_at(n.min(views.len()));
        let all = w.cfg.mqtt.all_temps;
        self.document(req, &mut w.response, None, |jw| {
            if temps {
                write_legacy_temps_json(jw, t, all)
            } else {
                write_legacy_volts_json(jw, v)
            }
        });
    }

    // ------------------------------------------------------------ events

    /// GET /api/events?since=&limit=&valve=&minSeverity=: a count that does not fit the
    /// response buffer is halved until it does.
    pub(super) fn events(&self, req: &mut dyn HttpRequest, w: &mut WorkSet) {
        let mut f = EventFilter::default();
        let Some(limit) = event_query(req, &mut f) else {
            return;
        };
        let Some(mut events) = scratch(self.ports.gate, EVENTS_BUFFER) else {
            return out_of_memory(req);
        };
        events.resize(EVENTS_BUFFER, Event::default());
        let Some(buf) = response_buf(&mut w.response, self.ports.gate) else {
            return out_of_memory(req);
        };
        let mut max = limit as usize;
        while max > 0 {
            let out = events.get_mut(..max.min(EVENTS_BUFFER)).unwrap_or_default();
            if self.events_fit(req, &f, out, buf) {
                return;
            }
            max /= 2;
        }
        send_error(req, 500, "internal", b"events");
    }

    /// The events of `f` read into `out` and answered when their document fits `buf`; false
    /// when it does not (nothing answered).
    fn events_fit(
        &self,
        req: &mut dyn HttpRequest,
        f: &EventFilter,
        out: &mut [Event],
        buf: &mut [u8],
    ) -> bool {
        let r = self.host.read_events(f, out);
        let mut jw = JsonWriter::new(buf);
        let read = out.get(..r.count).unwrap_or_default();
        if !write_events_json(
            &mut jw,
            read,
            r.first_seq,
            r.last_seq,
            r.next_since,
            r.dropped,
        ) || !jw.complete()
        {
            return false;
        }
        send_document(req, 200, jw.as_bytes(), None);
        true
    }

    // ------------------------------------------------------------ STM documents

    /// GET /api/valves/{n}/profile: 404 without one.
    pub(super) fn profile(&self, req: &mut dyn HttpRequest, w: &mut WorkSet, valve: u8) {
        let mut p = Profile::default();
        self.host.read_profile(valve, &mut p);
        if p.count == 0 {
            return send_error(req, 404, "not_found", b"no profile");
        }
        self.document(req, &mut w.response, None, |jw| write_profile_json(jw, &p));
    }

    /// GET /api/stm/motor.
    pub(super) fn motor(&self, req: &mut dyn HttpRequest, w: &mut WorkSet) {
        self.host.read_stm_snapshot(&mut w.snap);
        let s = &*w.snap;
        self.document(req, &mut w.response, None, |jw| {
            write_motor_json(
                jw,
                &s.motor,
                s.learn_movements,
                s.have_breakaway.then_some(&s.breakaway),
                s.have_motor,
            )
        });
    }

    /// GET /api/stm/flash.
    pub(super) fn flash_status(&self, req: &mut dyn HttpRequest, w: &mut WorkSet) {
        self.host.read_stm_snapshot(&mut w.snap);
        let s = &*w.snap;
        let image = c_str(&s.flash_image);
        self.document(req, &mut w.response, None, |jw| {
            write_flash_status_json(
                jw,
                &s.flash,
                (!image.is_empty()).then_some(image),
                s.flash_pending,
            )
        });
    }

    /// GET /api/stm/images.
    pub(super) fn images(&self, req: &mut dyn HttpRequest, w: &mut WorkSet) {
        let mut list: [ImageEntry; IMAGE_SLOTS] = Default::default();
        let n = self.storage.list_images(&mut list);
        self.document(req, &mut w.response, None, |jw| {
            jw.begin_array();
            for e in list.iter().take(n) {
                write_image(jw, e, e.scanned);
            }
            jw.end_array();
            jw.ok()
        });
    }

    /// GET /api/config and /api/config/export (a download).
    pub(super) fn config_get(&self, req: &mut dyn HttpRequest, w: &mut WorkSet, export: bool) {
        let cfg = &*w.cfg;
        self.document(
            req,
            &mut w.response,
            export.then_some(CONFIG_EXPORT),
            |jw| write_config_json(jw, cfg, None),
        );
    }

    // ------------------------------------------------------------ files

    /// GET /api/files: at most 32 files, `truncated` when there are more.
    pub(super) fn files(&self, req: &mut dyn HttpRequest, w: &mut WorkSet) {
        let Some(mut files) = scratch(self.ports.gate, MAX_FILES_PER_RESPONSE) else {
            return out_of_memory(req);
        };
        files.resize(MAX_FILES_PER_RESPONSE, FileEntry::default());
        let mut truncated = false;
        let n = self.storage.list_files(&mut files, &mut truncated);
        let total = self.storage.fs_total();
        let used = self.storage.fs_used();
        let listed = files.get(..n).unwrap_or_default();
        self.document(req, &mut w.response, None, |jw| {
            write_files_json(jw, listed, total, used, truncated)
        });
    }

    /// GET /api/import-report: the file as it is, read whole into the response buffer.
    pub(super) fn import_report(&self, req: &mut dyn HttpRequest, w: &mut WorkSet) {
        if !self.storage.has_import_report() {
            return send_error(req, 404, "not_found", b"no import report");
        }
        let Some(mut f) = self.ports.fs.open(IMPORT_REPORT_FILE, OpenMode::Read) else {
            return send_error(req, 404, "not_found", b"no import report");
        };
        let Some(buf) = response_buf(&mut w.response, self.ports.gate) else {
            return out_of_memory(req);
        };
        let n = f.read(buf);
        let whole = n == f.size() as usize;
        drop(f);
        if n == 0 || !whole {
            return send_error(req, 500, "io_error", IMPORT_REPORT_FILE.as_bytes());
        }
        send_document(req, 200, buf.get(..n).unwrap_or_default(), None);
    }

    // ------------------------------------------------------------ health and log

    /// GET /api/health: the document of the app in the first 1024 bytes of the response buffer
    /// (C++ a 1 KB text outside the slots).
    pub(super) fn health(&self, req: &mut dyn HttpRequest, w: &mut WorkSet) {
        let mut h = HealthSnapshot::default();
        self.host.read_health(&mut h);
        let Some(buf) = response_buf(&mut w.response, self.ports.gate) else {
            return out_of_memory(req);
        };
        let mut jw = JsonWriter::new(buf.get_mut(..HEALTH_BUF_SIZE).unwrap_or_default());
        if !write_health_json(&mut jw, &h) {
            return send_error(req, 500, "internal", b"health");
        }
        send_document(req, 200, jw.as_bytes(), None);
    }

    /// GET /api/log: the log is flushed first (asynchronously, by the app task), then the
    /// previous and the current file go out chunked through the response buffer.
    pub(super) fn log_download(&self, req: &mut dyn HttpRequest, w: &mut WorkSet) {
        if !self.storage.fs_ready() {
            return send_error(req, 503, "unavailable", b"no file system");
        }
        let Some(buf) = response_buf(&mut w.response, self.ports.gate) else {
            return out_of_memory(req);
        };
        self.host.request_log_flush();
        if !req.begin_chunked(
            200,
            "text/plain; charset=utf-8",
            &[("Content-Disposition", LOG_DOWNLOAD)],
        ) {
            return;
        }
        for path in [LOG_FILE_OLD, LOG_FILE] {
            let Some(mut f) = self.ports.fs.open(path, OpenMode::Read) else {
                continue;
            };
            loop {
                let n = f.read(buf);
                if n == 0 {
                    break;
                }
                if !req.chunk(buf.get(..n).unwrap_or_default()) {
                    return;
                }
            }
        }
        req.chunk(&[]);
    }

    // ------------------------------------------------------------ dashboard

    /// A dashboard file ("/" is "/index.html"): 304 when the client has it (`If-None-Match`
    /// equals its ETag), else the gzip bytes; 404 for any other path.
    pub(super) fn static_asset(&self, req: &mut dyn HttpRequest, path: &[u8]) {
        let wanted = if path == b"/" {
            b"/index.html".as_slice()
        } else {
            c_str(path)
        };
        let Some(a) = self.assets.iter().find(|a| a.path.as_bytes() == wanted) else {
            return send_error(req, 404, "not_found", path);
        };
        send_asset(req, a);
    }
}

/// A dashboard file: 304 when the client has it (`If-None-Match` equals its ETag), else the
/// gzip bytes.
fn send_asset(req: &mut dyn HttpRequest, a: &Asset) {
    let mut etag = [0u8; 16];
    if header(req, "If-None-Match", &mut etag) == Some(a.etag.as_bytes()) {
        return send(req, 304, "", b"");
    }
    req.respond(
        200,
        a.content_type,
        &[
            ("Content-Encoding", "gzip"),
            ("ETag", a.etag),
            ("Cache-Control", "no-cache"),
        ],
        a.data,
    );
}
