//! HTTP routes of the legacy firmware: the aliases /valves, /temps, /volts and /setvalve with
//! their legacy JSON documents, and the 410 table of the legacy paths that have a replacement
//! in /api (port of `vdm/legacy_http.h`). Hardware-free.

use crate::common::{c_str, format_one_wire_id, is_zero, VALVE_COUNT};
use crate::json_api::{HttpMethod, SensorView, ValveView};
use crate::json_writer::JsonWriter;
use crate::stm_codec::ValveStatus;

#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LegacyRoute {
    #[default]
    None = 0,
    Valves = 1,
    Temps = 2,
    Volts = 3,
    SetValve = 4,
    Gone = 5,
    MethodNotAllowed = 6,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LegacyMatch {
    pub route: LegacyRoute,
    /// Gone: the "detail" of the 410 answer
    pub replacement: &'static str,
}

impl Default for LegacyMatch {
    fn default() -> Self {
        Self {
            route: LegacyRoute::None,
            replacement: "",
        }
    }
}

const CMD_REPLACEMENT: &str = "/api/system/reboot, /api/valves/calibrate, /api/valves/assembly, \
     /api/valves/detect, /api/sensors/scan, /api/mqtt/reconnect, /api/mqtt/discovery";

/// The aliases and the method each answers.
const ALIASES: [(&[u8], LegacyRoute, HttpMethod); 4] = [
    (b"/valves", LegacyRoute::Valves, HttpMethod::Get),
    (b"/temps", LegacyRoute::Temps, HttpMethod::Get),
    (b"/volts", LegacyRoute::Volts, HttpMethod::Get),
    (b"/setvalve", LegacyRoute::SetValve, HttpMethod::Post),
];

/// The 410 table: legacy path and its replacement.
#[rustfmt::skip]
const GONE: [(&[u8], &str); 27] = [
    (b"/netinfo", "/api/status"),
    (b"/sysinfo", "/api/status"),
    (b"/sysdyninfo", "/api/status"),
    (b"/update/identity", "/api/status"),
    (b"/netconfig", "/api/config"),
    (b"/protconfig", "/api/config"),
    (b"/valvesconfig", "/api/config"),
    (b"/tempsconfig", "/api/config"),
    (b"/voltsconfig", "/api/config"),
    (b"/sysconfig", "/api/config"),
    (b"/sysLogCfg", "/api/config"),
    (b"/motorconfig", "/api/stm/motor"),
    (b"/tempsensorsid", "/api/sensors"),
    (b"/voltsensorsid", "/api/sensors"),
    (b"/fsdir", "/api/files"),
    (b"/fupload", "/api/stm/images"),
    (b"/stmupdate", "/#maintenance"),
    (b"/stmupdstatus", "/api/stm/flash"),
    (b"/stmdoupdate", "/api/stm/flash"),
    (b"/update", "/api/ota/esp"),
    (b"/cmd", CMD_REPLACEMENT),
    (b"/valvesctrlconfig", "removed: PI control"),
    (b"/msgconfig", "removed: messenger"),
    (b"/testPO", "removed: messenger"),
    (b"/testEmail", "removed: messenger"),
    (b"/ssidinfo", "removed: WiFi scan"),
    (b"/auth", "removed: web login"),
];

/// Exact, case-sensitive path (no query). MethodNotAllowed: an alias path with another method
/// (GET for /valves, /temps, /volts; POST for /setvalve). Gone: any method on a path of the
/// 410 table.
pub fn match_legacy_route(m: HttpMethod, path: &[u8]) -> LegacyMatch {
    if let Some(&(_, route, method)) = ALIASES.iter().find(|a| a.0 == path) {
        return LegacyMatch {
            route: if m == method {
                route
            } else {
                LegacyRoute::MethodNotAllowed
            },
            replacement: "",
        };
    }
    match GONE.iter().find(|g| g.0 == path) {
        Some(&(_, replacement)) => LegacyMatch {
            route: LegacyRoute::Gone,
            replacement,
        },
        None => LegacyMatch::default(),
    }
}

/// A temperature: tenths with one decimal, or "failed".
fn write_temp(jw: &mut JsonWriter<'_>, valid: bool, tenths: i32) {
    if valid {
        jw.fixed(tenths, 1);
    } else {
        jw.value("failed");
    }
}

/// The keys of the two sensor positions.
const SENSOR_KEYS: [(&str, &str); 2] = [("tIdxName1", "temp1"), ("tIdxName2", "temp2")];

/// `{"valves":[{"idx":n,"name":"..","state":s,"pos":..,"meanCur":..,
///  "targetPos":t,"link":0,"moves":..,"oc":..,"cc":..,"dc":..,"cr":..,
///  ["tIdxName1":"..","temp1":21.5|"failed",]["tIdxName2":..,"temp2":..,]
///  "controlActive":0[,"calibration":1]},...]}`
///
/// One entry per valve whose status is neither 0 (no data) nor 6 (no valve); a view without
/// state or config is skipped; idx = position in `views` + 1. targetPos = desired when valid,
/// else the STM target when known, else pos. A sensor position k is listed when
/// sensor_slot[k] != 0. "calibration" only while calibrating. Returns jw.ok().
pub fn write_legacy_valves_json(jw: &mut JsonWriter<'_>, views: &[ValveView<'_>]) -> bool {
    jw.begin_object();
    jw.key("valves");
    jw.begin_array();
    for (idx, v) in (1u32..).zip(views) {
        let (Some(st), Some(cfg)) = (v.state, v.config) else {
            continue;
        };
        if st.status == 0 || st.status == ValveStatus::NoValve as u8 {
            continue;
        }
        jw.begin_object();
        jw.kv("idx", idx);
        jw.kv("name", c_str(&cfg.name));
        jw.kv("state", st.status);
        jw.kv("pos", st.position);
        jw.kv("meanCur", st.mean_current);
        let target = if st.desired_valid {
            st.desired
        } else if st.stm_target_known {
            st.stm_target
        } else {
            st.position
        };
        jw.kv("targetPos", target);
        jw.kv("link", 0);
        jw.kv("moves", st.moves);
        jw.kv("oc", st.open_count);
        jw.kv("cc", st.close_count);
        jw.kv("dc", st.dead_zone);
        jw.kv("cr", st.calib_retries);
        for (k, (name_key, temp_key)) in SENSOR_KEYS.into_iter().enumerate() {
            if v.sensor_slot[k] == 0 {
                continue;
            }
            jw.kv(name_key, c_str(v.sensor_name[k]));
            jw.key(temp_key);
            write_temp(jw, v.sensor_valid[k], v.sensor_tenths[k]);
        }
        jw.kv("controlActive", 0);
        if st.calibrating {
            jw.kv("calibration", 1);
        }
        jw.end_object();
    }
    jw.end_array();
    jw.end_object();
    jw.ok()
}

fn id_value(jw: &mut JsonWriter<'_>, s: &SensorView<'_>) {
    let mut tmp = [0u8; 24];
    let n = format_one_wire_id(&s.id, &mut tmp);
    jw.kv("id", tmp.get(..n).unwrap_or_default());
}

/// Configured, active slot with an id.
fn listed(s: &SensorView<'_>) -> bool {
    s.slot != 0 && s.active && !is_zero(&s.id)
}

/// `[{"id":"28-..","name":"..","temp":21.5|"failed"},...]` for configured (slot != 0), active
/// slots with an id that no valve uses (every one with `all_temps`). Returns jw.ok().
pub fn write_legacy_temps_json(
    jw: &mut JsonWriter<'_>,
    temps: &[SensorView<'_>],
    all_temps: bool,
) -> bool {
    jw.begin_array();
    for s in temps {
        if !listed(s) || (s.valve < VALVE_COUNT && !all_temps) {
            continue;
        }
        jw.begin_object();
        id_value(jw, s);
        jw.kv("name", c_str(s.name));
        jw.key("temp");
        write_temp(jw, s.valid, s.value);
        jw.end_object();
    }
    jw.end_array();
    jw.ok()
}

/// `[{"id":"..","name":"..","unit":"..","value":12.080|"failed"},...]` for configured, active
/// slots with an id; value = SensorView::value (milli-units) with 3 decimals. Returns jw.ok().
pub fn write_legacy_volts_json(jw: &mut JsonWriter<'_>, volts: &[SensorView<'_>]) -> bool {
    jw.begin_array();
    for s in volts.iter().filter(|s| listed(s)) {
        jw.begin_object();
        id_value(jw, s);
        jw.kv("name", c_str(s.name));
        jw.kv("unit", c_str(s.unit));
        jw.key("value");
        if s.valid {
            jw.fixed(s.value, 3);
        } else {
            jw.value("failed");
        }
        jw.end_object();
    }
    jw.end_array();
    jw.ok()
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_mut;
