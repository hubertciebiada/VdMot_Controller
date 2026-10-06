//! STM application protocol codec (ESP side): builds request lines and parses reply lines into
//! typed structs (port of `vdm/stm_codec.h`). Hardware-free, no state.
//!
//! Wire format (protocol v1 = STM 1.4.x, v2 = revamped STM 2.0, v3 = STM 2.1):
//! ```text
//!   request : "<cmd>" { " " <arg> } " " "\r\n"  -- EVERY token, including the last, is
//!             followed by one space (the v1 STM tokenizer needs it); numbers are non-negative
//!             decimal; max 5 args; line <= 63 chars.
//!   reply   : "<cmd>" { " " <arg> } [" "] and CR/LF (one line per request).
//! ```
//! Replies are matched to requests by their 5-char command (see [`reply_matches`]).
//!
//! Builders that validate arguments return `Option<RequestLine>`: `None` is the C++ `false`
//! with `out = RequestLine{}` (`unwrap_or_default()` gives that line). Builders without an
//! argument to validate cannot fail and return the [`RequestLine`].

use core::fmt::Write as _;

use crate::common::{
    crc_valid, format_one_wire_id, is_zero, parse_int, parse_one_wire_id, parse_uint, OneWireId,
    Text, ALL_VALVES, NO_VALVE, ONE_WIRE_ID_TEXT_LEN, TEMP_SLOT_COUNT, TEMP_UNASSIGNED, VAD_FAILED,
    VALVE_COUNT, VOLT_SLOT_COUNT,
};
use crate::failsafe::{
    failsafe_pct_valid, lease_timeout_valid, LeaseState, FAILSAFE_HOLD, FAILSAFE_TIMEOUT_MAX_MIN,
};
use crate::line_assembler::STM_MAX_LINE_LEN;
use crate::version::{parse_version, Version};

/// A command of the protocol (C++ `enum class Cmd : uint8_t`; the numbers are external).
#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Cmd {
    #[default]
    None = 0,
    // protocol v1
    /// set target: `stgtp <valve> <pos>`
    Stgtp = 1,
    /// get target: `gtgtp <valve>`
    Gtgtp = 2,
    /// valve data: `gvlvd <valve>`
    Gvlvd = 3,
    /// all valve states: `gvlst`
    Gvlst = 4,
    /// DS18 count / list: `gonec` | `gonec 255`
    Gonec = 5,
    /// DS18 data by bus idx: `goned <idx>`
    Goned = 6,
    /// valve sensor ids: `gvlon <valve|255>`
    Gvlon = 7,
    /// DS2438 count / list: `gowvc` | `gowvc 255`
    Gowvc = 8,
    /// DS2438 data: `gowvd <idx>`
    Gowvd = 9,
    /// new 1-Wire search: `stons`
    Stons = 10,
    /// set valve sensor ids: `stvls <valve> <id1> <id2>`
    Stvls = 11,
    /// re-match sensor ids: `masns`
    Masns = 12,
    /// assembly (open fully): `staop <valve|255>`
    Staop = 13,
    /// calibrate: `staln <valve|255>`
    Staln = 14,
    /// re-detect valves: `stdet 255`
    Stdet = 15,
    /// set learn movements: `stlnm <n>`
    Stlnm = 16,
    /// get learn movements: `gtlnm`
    Gtlnm = 17,
    /// set motor chars: `smotc <low> <high> <sop> <minCnt> <maxRetr>`
    Smotc = 18,
    /// get motor chars: `gmotc`
    Gmotc = 19,
    /// version: `gvers`
    Gvers = 20,
    /// MCU device id: `ghwin`
    Ghwin = 21,
    /// EEPROM write state: `eepst`
    Eepst = 22,
    /// soft reset (EEPROM-safe): `reset`
    Reset = 23,
    // protocol v2 (revamped STM only; a v1 STM stays silent)
    /// protocol version: `gproto`
    Gproto = 24,
    /// extended valve data: `gvlvx <valve>`
    Gvlvx = 25,
    /// current profile: `gprof <valve>`
    Gprof = 26,
    /// service move: `svmov <valve> <dir> <counts> <maxmA>`
    Svmov = 27,
    /// set breakaway: `scalx <enable> <stepPct> <maxmA>`
    Scalx = 28,
    /// get breakaway: `gcalx`
    Gcalx = 29,
    /// STM health: `gstat`
    Gstat = 30,
    // protocol v3 (STM 2.1: gproto answers 3)
    /// valve data v3: `gvlvy <valve>`
    Gvlvy = 31,
    /// STM health v3: `gstax`
    Gstax = 32,
    /// lease heartbeat: `slhbt <0|1>`
    Slhbt = 33,
    /// set lease timeout: `slcfg <min>`
    Slcfg = 34,
    /// set failsafe pos.: `sfspo <valve|255> <pct|255>`
    Sfspo = 35,
    /// get lease config: `glcfg`
    Glcfg = 36,
    /// stop a move: `sstop <valve|255>`
    Sstop = 37,
    /// get learn time: `gtlnt`
    Gtlnt = 38,
    /// leave safe mode: `ssafe 0`
    Ssafe = 39,
    // protocol v1 command, appended (the numbers are external)
    /// set learn time: `stlnt <seconds>`
    Stlnt = 40,
}

/// Commands, including None.
pub const CMD_COUNT: u8 = 41;

/// Indexed by the command number.
const CMDS: [Cmd; CMD_COUNT as usize] = [
    Cmd::None,
    Cmd::Stgtp,
    Cmd::Gtgtp,
    Cmd::Gvlvd,
    Cmd::Gvlst,
    Cmd::Gonec,
    Cmd::Goned,
    Cmd::Gvlon,
    Cmd::Gowvc,
    Cmd::Gowvd,
    Cmd::Stons,
    Cmd::Stvls,
    Cmd::Masns,
    Cmd::Staop,
    Cmd::Staln,
    Cmd::Stdet,
    Cmd::Stlnm,
    Cmd::Gtlnm,
    Cmd::Smotc,
    Cmd::Gmotc,
    Cmd::Gvers,
    Cmd::Ghwin,
    Cmd::Eepst,
    Cmd::Reset,
    Cmd::Gproto,
    Cmd::Gvlvx,
    Cmd::Gprof,
    Cmd::Svmov,
    Cmd::Scalx,
    Cmd::Gcalx,
    Cmd::Gstat,
    Cmd::Gvlvy,
    Cmd::Gstax,
    Cmd::Slhbt,
    Cmd::Slcfg,
    Cmd::Sfspo,
    Cmd::Glcfg,
    Cmd::Sstop,
    Cmd::Gtlnt,
    Cmd::Ssafe,
    Cmd::Stlnt,
];

/// Indexed by the command number.
const CMD_NAMES: [&str; CMD_COUNT as usize] = [
    "", "stgtp", "gtgtp", "gvlvd", "gvlst", "gonec", "goned", "gvlon", "gowvc", "gowvd", "stons",
    "stvls", "masns", "staop", "staln", "stdet", "stlnm", "gtlnm", "smotc", "gmotc", "gvers",
    "ghwin", "eepst", "reset", "gproto", "gvlvx", "gprof", "svmov", "scalx", "gcalx", "gstat",
    "gvlvy", "gstax", "slhbt", "slcfg", "sfspo", "glcfg", "sstop", "gtlnt", "ssafe", "stlnt",
];

impl Cmd {
    pub fn from_raw(v: u8) -> Option<Self> {
        CMDS.get(usize::from(v)).copied()
    }
}

/// "stgtp" etc.; "" for None.
pub fn cmd_name(c: Cmd) -> &'static str {
    CMD_NAMES.get(usize::from(c as u8)).copied().unwrap_or("")
}

/// Exact match of all of `s` against the 40 names; None if unknown.
pub fn cmd_from_name(s: &[u8]) -> Cmd {
    CMDS.iter()
        .zip(CMD_NAMES)
        .find(|(_, name)| name.as_bytes() == s)
        .map_or(Cmd::None, |(c, _)| *c)
}

/// Lowest protocol that answers the command: 1 for the v1 commands and Stlnt, 2 for
/// Gproto..Gstat, 3 for Gvlvy..Ssafe; 0 for None.
pub fn cmd_min_protocol(c: Cmd) -> u8 {
    match c {
        Cmd::None => 0,
        Cmd::Stlnt => 1,
        _ if c as u8 >= Cmd::Gvlvy as u8 => 3,
        _ if c as u8 >= Cmd::Gproto as u8 => 2,
        _ => 1,
    }
}

/// True for the commands a v1 STM does not answer: `cmd_min_protocol(c) >= 2`.
pub fn cmd_is_v2(c: Cmd) -> bool {
    cmd_min_protocol(c) >= 2
}

/// True when sending the same request twice has the same effect as once (all get*, stgtp,
/// stvls, stlnm, smotc, scalx, stlnt and every v3 command: a repeated heartbeat, stop or
/// safe-mode exit changes nothing). Only these are retried by LinkPolicy. Actions (staln,
/// staop, stdet, stons, masns, svmov, reset) are never retried automatically.
pub fn cmd_is_idempotent(c: Cmd) -> bool {
    !matches!(
        c,
        Cmd::None
            | Cmd::Stons
            | Cmd::Masns
            | Cmd::Staop
            | Cmd::Staln
            | Cmd::Stdet
            | Cmd::Reset
            | Cmd::Svmov
    )
}

// ---------------------------------------------------------------- requests

/// Longest request in chars, "\r\n" included (the C++ `text` holds it and a NUL).
pub const REQUEST_MAX_LEN: usize = 63;

/// One encoded request. `valve` is the valve the request is about (0..11), [`ALL_VALVES`] for
/// "255" requests, or [`NO_VALVE`]. `arg` is the first numeric argument when there is one that
/// is not a valve (sensor bus index, count; slhbt: alive, slcfg: minutes, sfspo: pct), else 0;
/// LinkPolicy uses (cmd, valve, arg) to match replies.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequestLine {
    /// The line, "\r\n" included (C++ `text` and `len`); empty: no request.
    pub text: Text<REQUEST_MAX_LEN>,
    pub cmd: Cmd,
    pub valve: u8,
    pub arg: u16,
    /// May stay unanswered by design (gproto on a v1 STM): its timeouts never count toward a
    /// link failure. Set only by [`build_get_proto`].
    pub probe: bool,
    /// goned/gowvd: id expected at bus index `arg`; zero = unknown, any id matches (see
    /// [`reply_matches`]).
    pub expect: OneWireId,
}

impl Default for RequestLine {
    fn default() -> Self {
        Self {
            text: Text::new(),
            cmd: Cmd::None,
            valve: NO_VALVE,
            arg: 0,
            probe: false,
            expect: OneWireId::default(),
        }
    }
}

/// Motor characteristics (STM EEPROM), as in gmotc/smotc. `low_factor`/`high_factor`: end-stop
/// current threshold in tenths x mean current.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MotorChars {
    pub low_factor: u8,
    pub high_factor: u8,
    /// % assumed and targeted after STM boot
    pub start_on_power: u8,
    /// min pulses of a full stroke (calibration)
    pub min_counts: u16,
    /// extra calibration attempts before BLOCKED
    pub max_calib_retries: u8,
    /// parse only: fields present in gmotc (3..5)
    pub field_count: u8,
}

impl Default for MotorChars {
    fn default() -> Self {
        Self {
            low_factor: 17,
            high_factor: 17,
            start_on_power: 30,
            min_counts: 3000,
            max_calib_retries: 2,
            field_count: 5,
        }
    }
}

/// Values smotc may send: those the v1 STM keeps across a reboot as well (its boot acceptance):
/// factors 10..40, start_on_power 0..100, min_counts 0..60000, max_calib_retries 0..2.
pub fn motor_chars_valid(m: &MotorChars) -> bool {
    (10..=40).contains(&m.low_factor)
        && (10..=40).contains(&m.high_factor)
        && m.start_on_power <= 100
        && m.min_counts <= 60000
        && m.max_calib_retries <= 2
}

/// Breakaway escalation (v2, scalx/gcalx).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Breakaway {
    pub enable: bool,
    /// 0..100
    pub step_pct: u8,
    /// 20..60
    pub max_ma: u8,
}

impl Default for Breakaway {
    fn default() -> Self {
        Self {
            enable: false,
            step_pct: 0,
            max_ma: 60,
        }
    }
}

pub fn breakaway_valid(b: &Breakaway) -> bool {
    b.step_pct <= 100 && (20..=60).contains(&b.max_ma)
}

/// learnAfterMovements accepted by stlnm: 0 (disabled) or 50..65534.
pub fn learn_movements_valid(n: u32) -> bool {
    n == 0 || (50..=65534).contains(&n)
}

/// Direction of a service move.
#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MoveDir {
    #[default]
    Open = 0,
    Close = 1,
}

impl MoveDir {
    pub fn from_raw(v: u8) -> Option<Self> {
        [Self::Open, Self::Close].get(usize::from(v)).copied()
    }
}

/// Appends `s` and the space that follows every token. The longest request (stvls with two
/// ids) has 59 chars, so nothing is ever cut: the C++ overflow guard cannot fire.
fn push_token(text: &mut Text<REQUEST_MAX_LEN>, s: &[u8]) {
    let _ = text.extend_from_slice(s);
    let _ = text.push(b' ');
}

fn push_number(text: &mut Text<REQUEST_MAX_LEN>, v: u32) {
    let _ = write!(text, "{v} ");
}

fn push_id(text: &mut Text<REQUEST_MAX_LEN>, id: &OneWireId) {
    let mut buf = [0u8; ONE_WIRE_ID_TEXT_LEN + 1];
    let n = format_one_wire_id(id, &mut buf);
    push_token(text, buf.get(..n).unwrap_or_default());
}

fn push_end(text: &mut Text<REQUEST_MAX_LEN>) {
    let _ = text.extend_from_slice(b"\r\n");
}

/// "<cmd> <n0> <n1> ... \r\n". `valve` and `arg` only fill the metadata.
fn build_numeric(cmd: Cmd, valve: u8, arg: u16, nums: &[u32]) -> RequestLine {
    let mut out = RequestLine {
        cmd,
        valve,
        arg,
        ..RequestLine::default()
    };
    push_token(&mut out.text, cmd_name(cmd).as_bytes());
    for &n in nums {
        push_number(&mut out.text, n);
    }
    push_end(&mut out.text);
    out
}

fn build_bare(cmd: Cmd) -> RequestLine {
    build_numeric(cmd, NO_VALVE, 0, &[])
}

fn build_valve_arg(cmd: Cmd, valve: u8) -> Option<RequestLine> {
    (valve < VALVE_COUNT).then(|| build_numeric(cmd, valve, 0, &[u32::from(valve)]))
}

fn build_valve_or_all(cmd: Cmd, valve: u8) -> Option<RequestLine> {
    (valve < VALVE_COUNT || valve == ALL_VALVES)
        .then(|| build_numeric(cmd, valve, 0, &[u32::from(valve)]))
}

fn build_index_arg(cmd: Cmd, index: u8, max_index: u8) -> Option<RequestLine> {
    (index <= max_index)
        .then(|| build_numeric(cmd, NO_VALVE, u16::from(index), &[u32::from(index)]))
}

fn build_list_request(cmd: Cmd) -> RequestLine {
    build_numeric(
        cmd,
        NO_VALVE,
        u16::from(ALL_VALVES),
        &[u32::from(ALL_VALVES)],
    )
}

/// `stgtp <valve> <pos>`: valve 0..11, pos 0..100.
pub fn build_set_target(valve: u8, pos: u8) -> Option<RequestLine> {
    (valve < VALVE_COUNT && pos <= 100).then(|| {
        build_numeric(
            Cmd::Stgtp,
            valve,
            u16::from(pos),
            &[u32::from(valve), u32::from(pos)],
        )
    })
}

/// `gtgtp <valve>`: 0..11.
pub fn build_get_target(valve: u8) -> Option<RequestLine> {
    build_valve_arg(Cmd::Gtgtp, valve)
}

/// `gvlvd <valve>`: 0..11.
pub fn build_valve_data(valve: u8) -> Option<RequestLine> {
    build_valve_arg(Cmd::Gvlvd, valve)
}

/// "gvlst "
pub fn build_valve_states() -> RequestLine {
    build_bare(Cmd::Gvlst)
}

/// "gonec "
pub fn build_temp_count() -> RequestLine {
    build_bare(Cmd::Gonec)
}

/// "gonec 255 "
pub fn build_temp_list() -> RequestLine {
    build_list_request(Cmd::Gonec)
}

/// `goned <bus index>`: 0..33.
pub fn build_temp_data(bus_index: u8) -> Option<RequestLine> {
    build_index_arg(Cmd::Goned, bus_index, TEMP_SLOT_COUNT - 1)
}

/// `gvlon <valve>`: 0..11 or [`ALL_VALVES`].
pub fn build_valve_sensors(valve_or_all: u8) -> Option<RequestLine> {
    build_valve_or_all(Cmd::Gvlon, valve_or_all)
}

/// "gowvc "
pub fn build_volt_count() -> RequestLine {
    build_bare(Cmd::Gowvc)
}

/// "gowvc 255 "
pub fn build_volt_list() -> RequestLine {
    build_list_request(Cmd::Gowvc)
}

/// `gowvd <bus index>`: 0..7.
pub fn build_volt_data(bus_index: u8) -> Option<RequestLine> {
    build_index_arg(Cmd::Gowvd, bus_index, VOLT_SLOT_COUNT - 1)
}

/// "stons "
pub fn build_scan_one_wire() -> RequestLine {
    build_bare(Cmd::Stons)
}

/// A zero id or one that passes the CRC check.
fn sensor_id_ok(id: &OneWireId) -> bool {
    is_zero(id) || crc_valid(id)
}

/// `stvls <valve> <id1> <id2>`. Zero ids unassign. Non-zero ids must pass `crc_valid()` (the
/// STM ignores bad CRCs silently, so they are rejected here).
pub fn build_set_valve_sensors(valve: u8, s1: &OneWireId, s2: &OneWireId) -> Option<RequestLine> {
    if valve >= VALVE_COUNT || !sensor_id_ok(s1) || !sensor_id_ok(s2) {
        return None;
    }
    let mut out = RequestLine {
        cmd: Cmd::Stvls,
        valve,
        ..RequestLine::default()
    };
    push_token(&mut out.text, cmd_name(Cmd::Stvls).as_bytes());
    push_number(&mut out.text, u32::from(valve));
    push_id(&mut out.text, s1);
    push_id(&mut out.text, s2);
    push_end(&mut out.text);
    Some(out)
}

/// "masns "
pub fn build_match_sensors() -> RequestLine {
    build_bare(Cmd::Masns)
}

/// `staop <valve|255>`
pub fn build_assembly(valve_or_all: u8) -> Option<RequestLine> {
    build_valve_or_all(Cmd::Staop, valve_or_all)
}

/// `staln <valve|255>`
pub fn build_calibrate(valve_or_all: u8) -> Option<RequestLine> {
    build_valve_or_all(Cmd::Staln, valve_or_all)
}

/// "stdet 255 "
pub fn build_detect() -> RequestLine {
    build_numeric(Cmd::Stdet, ALL_VALVES, 0, &[u32::from(ALL_VALVES)])
}

/// `stlnm <n>`: [`learn_movements_valid`].
pub fn build_set_learn_movements(n: u32) -> Option<RequestLine> {
    learn_movements_valid(n).then(|| build_numeric(Cmd::Stlnm, NO_VALVE, n as u16, &[n]))
}

/// "gtlnm "
pub fn build_get_learn_movements() -> RequestLine {
    build_bare(Cmd::Gtlnm)
}

/// `smotc <low> <high> <sop> <minCnt> <maxRetr>`: [`motor_chars_valid`], always 5 args.
pub fn build_set_motor_chars(m: &MotorChars) -> Option<RequestLine> {
    motor_chars_valid(m).then(|| {
        build_numeric(
            Cmd::Smotc,
            NO_VALVE,
            u16::from(m.low_factor),
            &[
                u32::from(m.low_factor),
                u32::from(m.high_factor),
                u32::from(m.start_on_power),
                u32::from(m.min_counts),
                u32::from(m.max_calib_retries),
            ],
        )
    })
}

/// "gmotc "
pub fn build_get_motor_chars() -> RequestLine {
    build_bare(Cmd::Gmotc)
}

/// "gvers "
pub fn build_get_version() -> RequestLine {
    build_bare(Cmd::Gvers)
}

/// "ghwin "
pub fn build_get_hw_id() -> RequestLine {
    build_bare(Cmd::Ghwin)
}

/// "eepst "
pub fn build_eeprom_state() -> RequestLine {
    build_bare(Cmd::Eepst)
}

/// "reset "
pub fn build_soft_reset() -> RequestLine {
    build_bare(Cmd::Reset)
}

/// "gproto ". A v1 STM never answers it: the request is a probe.
pub fn build_get_proto() -> RequestLine {
    RequestLine {
        probe: true,
        ..build_bare(Cmd::Gproto)
    }
}

/// `gvlvx <valve>`: 0..11 (v2).
pub fn build_valve_ex(valve: u8) -> Option<RequestLine> {
    build_valve_arg(Cmd::Gvlvx, valve)
}

/// `gprof <valve>`: 0..11 (v2).
pub fn build_profile(valve: u8) -> Option<RequestLine> {
    build_valve_arg(Cmd::Gprof, valve)
}

/// `svmov <valve> <dir> <counts> <maxmA>`: counts 1..10000, max_ma 5..60 (v2).
pub fn build_service_move(valve: u8, dir: MoveDir, counts: u16, max_ma: u8) -> Option<RequestLine> {
    // C++ also refuses a dir above 1, which a MoveDir cannot hold.
    if valve >= VALVE_COUNT || !(1..=10000).contains(&counts) || !(5..=60).contains(&max_ma) {
        return None;
    }
    let d = dir as u8;
    Some(build_numeric(
        Cmd::Svmov,
        valve,
        u16::from(d),
        &[
            u32::from(valve),
            u32::from(d),
            u32::from(counts),
            u32::from(max_ma),
        ],
    ))
}

/// `scalx <enable> <stepPct> <maxmA>`: [`breakaway_valid`] (v2).
pub fn build_set_breakaway(b: &Breakaway) -> Option<RequestLine> {
    breakaway_valid(b).then(|| {
        build_numeric(
            Cmd::Scalx,
            NO_VALVE,
            u16::from(b.enable),
            &[
                u32::from(b.enable),
                u32::from(b.step_pct),
                u32::from(b.max_ma),
            ],
        )
    })
}

/// "gcalx " (v2)
pub fn build_get_breakaway() -> RequestLine {
    build_bare(Cmd::Gcalx)
}

/// "gstat " (v2)
pub fn build_get_status() -> RequestLine {
    build_bare(Cmd::Gstat)
}

/// `gvlvy <valve>`: 0..11 (v3).
pub fn build_valve_ex_v3(valve: u8) -> Option<RequestLine> {
    build_valve_arg(Cmd::Gvlvy, valve)
}

/// "gstax " (v3)
pub fn build_get_status_v3() -> RequestLine {
    build_bare(Cmd::Gstax)
}

/// "slhbt <0|1> " (v3)
pub fn build_heartbeat(alive: bool) -> RequestLine {
    build_numeric(Cmd::Slhbt, NO_VALVE, u16::from(alive), &[u32::from(alive)])
}

/// `slcfg <minutes>`: [`lease_timeout_valid`] (v3).
pub fn build_set_lease_timeout(minutes: u32) -> Option<RequestLine> {
    lease_timeout_valid(minutes)
        .then(|| build_numeric(Cmd::Slcfg, NO_VALVE, minutes as u16, &[minutes]))
}

/// `sfspo <valve|255> <pct|255>`: 0..11 or [`ALL_VALVES`], [`failsafe_pct_valid`] (v3).
pub fn build_set_failsafe(valve_or_all: u8, pct: u8) -> Option<RequestLine> {
    ((valve_or_all < VALVE_COUNT || valve_or_all == ALL_VALVES)
        && failsafe_pct_valid(u32::from(pct)))
    .then(|| {
        build_numeric(
            Cmd::Sfspo,
            valve_or_all,
            u16::from(pct),
            &[u32::from(valve_or_all), u32::from(pct)],
        )
    })
}

/// "glcfg " (v3)
pub fn build_get_lease_config() -> RequestLine {
    build_bare(Cmd::Glcfg)
}

/// "sstop <v|255> " (v3)
pub fn build_stop(valve_or_all: u8) -> Option<RequestLine> {
    build_valve_or_all(Cmd::Sstop, valve_or_all)
}

/// "gtlnt " (v3)
pub fn build_get_learn_time() -> RequestLine {
    build_bare(Cmd::Gtlnt)
}

/// "ssafe 0 " (v3)
pub fn build_leave_safe_mode() -> RequestLine {
    build_numeric(Cmd::Ssafe, NO_VALVE, 0, &[0])
}

/// v1: "stlnt <seconds> " (0 = learn-time trigger off).
pub fn build_set_learn_time(seconds: u32) -> RequestLine {
    build_numeric(Cmd::Stlnt, NO_VALVE, 0, &[seconds])
}

// ---------------------------------------------------------------- replies

#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ParseStatus {
    #[default]
    Ok = 0,
    /// no token
    Empty = 1,
    /// first token is not one of the command names
    UnknownCommand = 2,
    /// wrong number of fields for this reply
    BadArgCount = 3,
    /// a numeric field is not strict decimal
    BadNumber = 4,
    /// numeric field outside the documented range
    OutOfRange = 5,
    /// id field is not "hh-hh-hh-hh-hh-hh-hh-hh"
    BadOneWireId = 6,
    /// other structure error (list separators, "c:m" pairs, ...)
    BadFormat = 7,
    /// line longer than STM_MAX_LINE_LEN or more than 40 tokens
    TooLong = 8,
}

impl ParseStatus {
    pub fn from_raw(v: u8) -> Option<Self> {
        [
            Self::Ok,
            Self::Empty,
            Self::UnknownCommand,
            Self::BadArgCount,
            Self::BadNumber,
            Self::OutOfRange,
            Self::BadOneWireId,
            Self::BadFormat,
            Self::TooLong,
        ]
        .get(usize::from(v))
        .copied()
    }
}

/// "ok", "empty", "unknown_command", "bad_arg_count", "bad_number", "out_of_range",
/// "bad_onewire_id", "bad_format", "too_long".
pub fn parse_status_name(s: ParseStatus) -> &'static str {
    match s {
        ParseStatus::Ok => "ok",
        ParseStatus::Empty => "empty",
        ParseStatus::UnknownCommand => "unknown_command",
        ParseStatus::BadArgCount => "bad_arg_count",
        ParseStatus::BadNumber => "bad_number",
        ParseStatus::OutOfRange => "out_of_range",
        ParseStatus::BadOneWireId => "bad_onewire_id",
        ParseStatus::BadFormat => "bad_format",
        ParseStatus::TooLong => "too_long",
    }
}

/// Valve status byte (low 7 bits of gvlvd field 3). 0 is ESP-only "no data".
#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ValveStatus {
    #[default]
    Start = 0,
    Idle = 1,
    Opening = 2,
    Closing = 3,
    /// 120 s without end stop; later targets ignored by STM v1
    Failed = 4,
    /// after STM boot / stdet, before A_TEST
    Unknown = 5,
    /// open circuit
    NoValve = 6,
    /// staop pending
    FullOpen = 7,
    /// detected, waiting for calibration
    Connected = 8,
    /// calibration failed
    Blocked = 9,
}

impl ValveStatus {
    pub fn from_raw(v: u8) -> Option<Self> {
        [
            Self::Start,
            Self::Idle,
            Self::Opening,
            Self::Closing,
            Self::Failed,
            Self::Unknown,
            Self::NoValve,
            Self::FullOpen,
            Self::Connected,
            Self::Blocked,
        ]
        .get(usize::from(v))
        .copied()
    }
}

/// gvlvd: "gvlvd v pos cur st t1 t2 mov oc cc dc cr" (11 fields, all required).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ValveData {
    /// 0..11
    pub valve: u8,
    /// 0..100
    pub position: u8,
    /// mA, 0..65535
    pub mean_current: u16,
    /// raw & 0x7F
    pub status: u8,
    /// raw & 0x80
    pub calibrating: bool,
    /// 0.1 C raw incl. sentinels
    pub temp1: i16,
    pub temp2: i16,
    pub moves: u32,
    pub open_count: u32,
    pub close_count: u32,
    /// may be negative
    pub dead_zone: i32,
    /// 0..255
    pub calib_retries: u8,
}

impl Default for ValveData {
    fn default() -> Self {
        Self {
            valve: 0,
            position: 0,
            mean_current: 0,
            status: 0,
            calibrating: false,
            temp1: TEMP_UNASSIGNED,
            temp2: TEMP_UNASSIGNED,
            moves: 0,
            open_count: 0,
            close_count: 0,
            dead_zone: 0,
            calib_retries: 0,
        }
    }
}

/// gvlst: "gvlst 12 s0,s1,...,s11," -- exactly 12 statuses (0..255 raw, no calibration bit),
/// trailing comma optional.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ValveStates {
    pub status: [u8; VALVE_COUNT as usize],
}

/// gonec / gowvc. Count-only form "gonec N" (`has_list` false) or list form "gonec N id,id,..."
/// (`has_list` true, exactly N ids, trailing comma optional).
/// NOTE: "gonec 0" is both the count reply and the empty-list reply; the caller that asked for
/// the list treats count == 0 as an empty list. N is limited to TEMP_SLOT_COUNT (gonec) /
/// VOLT_SLOT_COUNT (gowvc).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OneWireList {
    pub count: u8,
    pub has_list: bool,
    pub ids: [OneWireId; TEMP_SLOT_COUNT as usize],
}

impl Default for OneWireList {
    fn default() -> Self {
        Self {
            count: 0,
            has_list: false,
            ids: [OneWireId::default(); TEMP_SLOT_COUNT as usize],
        }
    }
}

/// goned: "goned <id> <temp>" (valid) or "goned 0" (`valid` false: index out of range on the
/// STM). "goned error" is the v1 gvlon error reply and parses as Cmd::Gvlon with `gvlon_error`
/// (see [`Reply`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TempData {
    pub valid: bool,
    pub id: OneWireId,
    /// 0.1 C raw
    pub value: i16,
}

impl Default for TempData {
    fn default() -> Self {
        Self {
            valid: false,
            id: OneWireId::default(),
            value: TEMP_UNASSIGNED,
        }
    }
}

/// gowvd: "gowvd <id> <vad>" or "gowvd 0". vad in 10 mV, int32.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VoltData {
    pub valid: bool,
    pub id: OneWireId,
    pub vad: i32,
}

impl Default for VoltData {
    fn default() -> Self {
        Self {
            valid: false,
            id: OneWireId::default(),
            vad: VAD_FAILED,
        }
    }
}

/// gvlon single "gvlon v id1 id2" or list "gvlon 12 a1,a2,b1,b2,..." (24 ids). Ids are reported
/// verbatim; a v1 STM may report garbage for unassigned sensors, so consumers resolve them
/// against known ids.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ValveSensors {
    pub is_list: bool,
    /// single form only
    pub valve: u8,
    /// single form: `ids[valve][0..1]` only
    pub ids: [[OneWireId; 2]; VALVE_COUNT as usize],
}

/// gtgtp v pos
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TargetReply {
    pub valve: u8,
    /// 0..100
    pub target: u8,
}

// gvlvx (v2), 19 fields after the command:
// idx status pos target meanCur oc cc dc cr moves calState earlyStops
// cmdRejected lastDir lastReq lastCnt lastStop lastPeak lastMs
// `status` uses the gvlvd encoding, but its bit 7 is not the calibration state in v2: the STM
// sets it only for staln and the movement trigger, and keeps it set until that calibration
// ends. calState (0..15 on the wire) is authoritative: bits 0..1 the phase, bits 2..3 sticky
// flags.
pub const CAL_STATE_IDLE: u8 = 0;
pub const CAL_STATE_REQUESTED: u8 = 1;
pub const CAL_STATE_RUNNING: u8 = 2;
pub const CAL_STATE_MASK: u8 = 0x03;
/// early end stop since the last good calibration
pub const CAL_FLAG_EARLY_STOP: u8 = 0x04;
/// the last calibration did not succeed
pub const CAL_FLAG_LAST_FAILED: u8 = 0x08;
/// CAL_FLAG_EARLY_STOP | CAL_FLAG_LAST_FAILED
pub const CAL_FLAG_MASK: u8 = 0x0C;

#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StopReason {
    #[default]
    None = 0,
    Target = 1,
    EndStop = 2,
    EarlyEndStop = 3,
    Timeout = 4,
    UnderCurrent = 5,
    SafetyOverCurrent = 6,
    Aborted = 7,
}

impl StopReason {
    pub fn from_raw(v: u8) -> Option<Self> {
        [
            Self::None,
            Self::Target,
            Self::EndStop,
            Self::EarlyEndStop,
            Self::Timeout,
            Self::UnderCurrent,
            Self::SafetyOverCurrent,
            Self::Aborted,
        ]
        .get(usize::from(v))
        .copied()
    }
}

/// "none", "target", "endstop", "early_endstop", "timeout", "undercurrent",
/// "safety_overcurrent", "aborted".
pub fn stop_reason_name(r: StopReason) -> &'static str {
    match r {
        StopReason::None => "none",
        StopReason::Target => "target",
        StopReason::EndStop => "endstop",
        StopReason::EarlyEndStop => "early_endstop",
        StopReason::Timeout => "timeout",
        StopReason::UnderCurrent => "undercurrent",
        StopReason::SafetyOverCurrent => "safety_overcurrent",
        StopReason::Aborted => "aborted",
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MoveResult {
    pub dir: MoveDir,
    pub requested_counts: u32,
    pub counted_counts: u32,
    pub stop: StopReason,
    /// 0.1 mA
    pub peak_current: u16,
    pub duration_ms: u32,
}

// gvlvy (v3) field 20: valve flags (the STM's kVlvFlag* values).
/// at its failsafe position: lease expired
pub const STM_FLAG_FS_LEASE: u16 = 0x001;
/// at its failsafe position: valve blocked
pub const STM_FLAG_FS_BLOCKED: u16 = 0x002;
/// no valid calibration counts
pub const STM_FLAG_UNCALIBRATED: u16 = 0x004;
/// next move goes to an end stop first
pub const STM_FLAG_NEEDS_REF: u16 = 0x008;
/// full calibration once the valve is present
pub const STM_FLAG_RECAL: u16 = 0x010;
/// counts restored from EEPROM
pub const STM_FLAG_CAL_RESTORED: u16 = 0x020;
/// automatic calibration retry scheduled
pub const STM_FLAG_RETRY: u16 = 0x040;
/// one early partial stop at this drive
pub const STM_FLAG_EARLY_PENDING: u16 = 0x080;
/// staop hold
pub const STM_FLAG_ASSEMBLY: u16 = 0x100;
/// left at a service move / sstop position
pub const STM_FLAG_SVC_HOLD: u16 = 0x200;

/// "fsLease", "fsBlocked", "uncalibrated", "needsRef", "recal", "calRestored", "retry",
/// "earlyPending", "assembly", "svcHold" for bits 0..9; "" for the reserved bits 10..15 and
/// above.
pub fn stm_flag_name(bit: u8) -> &'static str {
    const NAMES: [&str; 10] = [
        "fsLease",
        "fsBlocked",
        "uncalibrated",
        "needsRef",
        "recal",
        "calRestored",
        "retry",
        "earlyPending",
        "assembly",
        "svcHold",
    ];
    NAMES.get(usize::from(bit)).copied().unwrap_or("")
}

/// gvlvy field 21: why a valve failed (status 4) or is blocked (status 9).
#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ValveFault {
    #[default]
    None = 0,
    MoveTimeout = 1,
    StrokeTimeout = 2,
    /// presence test measured a short
    Short = 3,
    /// blocked
    StrokesTooShort = 4,
    /// the motor tripped the inrush limit at start
    InrushTrip = 5,
}

impl ValveFault {
    pub fn from_raw(v: u8) -> Option<Self> {
        [
            Self::None,
            Self::MoveTimeout,
            Self::StrokeTimeout,
            Self::Short,
            Self::StrokesTooShort,
            Self::InrushTrip,
        ]
        .get(usize::from(v))
        .copied()
    }
}

/// "none", "move_timeout", "stroke_timeout", "short", "strokes_too_short", "inrush_trip";
/// "unknown" for other values.
pub fn valve_fault_name(fault: u8) -> &'static str {
    const NAMES: [&str; 6] = [
        "none",
        "move_timeout",
        "stroke_timeout",
        "short",
        "strokes_too_short",
        "inrush_trip",
    ];
    NAMES.get(usize::from(fault)).copied().unwrap_or("unknown")
}

// gstax field 18: STM configuration load flags.
pub const STM_CFG_LAYOUT_CRC: u8 = 0x01;
pub const STM_CFG_SHADOW_MISSING: u8 = 0x02;
pub const STM_CFG_SETTINGS_CORRUPT: u8 = 0x04;
pub const STM_CFG_SAFETY_CORRUPT: u8 = 0x08;
pub const STM_CFG_SENSOR_SLOT: u8 = 0x10;
pub const STM_CFG_CALIB: u8 = 0x20;
pub const STM_CFG_UNVERIFIED: u8 = 0x40;
pub const STM_CFG_READ_FAILED: u8 = 0x80;

/// "layoutCrc", "shadowMissing", "settingsCorrupt", "safetyCorrupt", "sensorSlot", "calib",
/// "unverified", "readFailed" for bits 0..7; "" above.
pub fn stm_cfg_flag_name(bit: u8) -> &'static str {
    const NAMES: [&str; 8] = [
        "layoutCrc",
        "shadowMissing",
        "settingsCorrupt",
        "safetyCorrupt",
        "sensorSlot",
        "calib",
        "unverified",
        "readFailed",
    ];
    NAMES.get(usize::from(bit)).copied().unwrap_or("")
}

/// gstax field 23: the common-mode protection guard tripped; the short and inrush limits are
/// off until the next STM start.
pub const STM_SYS_PROTECT_SUSPENDED: u8 = 0x01;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ValveEx {
    pub valve: u8,
    /// raw & 0x7F
    pub status: u8,
    /// A calibration runs (calState Running) or was asked for by staln or the movement trigger
    /// (status bit 7). Requested alone (calState 1 without bit 7: time trigger queued, or a
    /// valve found at start-up that calibrates on its first target change) is not
    /// "calibrating": it can last for days.
    pub calibrating: bool,
    /// 0..100
    pub position: u8,
    /// 0..100, the STM's current target
    pub target: u8,
    pub mean_current: u16,
    pub open_count: u32,
    pub close_count: u32,
    pub dead_zone: i32,
    pub calib_retries: u8,
    pub moves: u32,
    /// phase: raw & CAL_STATE_MASK (0 idle, 1 requested, 2 running)
    pub cal_state: u8,
    /// raw & CAL_FLAG_MASK
    pub cal_flags: u8,
    pub early_stops: u32,
    pub cmd_rejected: u32,
    pub last_move: MoveResult,
    /// gvlvy (v3) only; gvlvx leaves the defaults.
    pub v3: bool,
    /// field 20, STM_FLAG_*
    pub flags: u16,
    /// field 21, [`ValveFault`]
    pub fault: u8,
    /// field 22, 0..100 or FAILSAFE_HOLD
    pub fs_pct: u8,
    /// field 23, the target the STM drives to
    pub drive: u8,
    /// field 24, s to the next automatic retry (0 none)
    pub retry_s: u32,
    /// field 25, automatic retries since the fault began
    pub retries: u8,
}

impl Default for ValveEx {
    fn default() -> Self {
        Self {
            valve: 0,
            status: 0,
            calibrating: false,
            position: 0,
            target: 0,
            mean_current: 0,
            open_count: 0,
            close_count: 0,
            dead_zone: 0,
            calib_retries: 0,
            moves: 0,
            cal_state: 0,
            cal_flags: 0,
            early_stops: 0,
            cmd_rejected: 0,
            last_move: MoveResult::default(),
            v3: false,
            flags: 0,
            fault: 0,
            fs_pct: FAILSAFE_HOLD,
            drive: 0,
            retry_s: 0,
            retries: 0,
        }
    }
}

/// gprof (v2): "gprof idx n c1:m1 ... cn:mn", n 0..32, exactly n pairs.
pub const PROFILE_MAX_SAMPLES: u8 = 32;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProfileSample {
    /// motor pulse count at the sample
    pub count: u32,
    /// 0.1 mA
    pub current: u16,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Profile {
    pub valve: u8,
    pub count: u8,
    pub samples: [ProfileSample; PROFILE_MAX_SAMPLES as usize],
}

/// "<cmd> <idx> ok" or "<cmd> <idx> err <code>" (svmov v2; sfspo, sstop v3). index -1: the STM
/// could not read the index; such a reply answers any outstanding request of that command.
/// index 255 (sfspo, sstop): all valves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IndexedResult {
    pub index: i16,
    pub ok: bool,
    pub error_code: u16,
}

impl Default for IndexedResult {
    fn default() -> Self {
        Self {
            index: -1,
            ok: false,
            error_code: 0,
        }
    }
}

/// gstat (v2): "gstat uptime_s resets bootReason rxOverflow parseErr eepState". gstax (v3)
/// starts with the same 6 fields and adds 17 more.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StmStatus {
    pub uptime_s: u32,
    pub resets: u32,
    pub boot_reason: u32,
    pub rx_overflow: u32,
    pub parse_errors: u32,
    pub eep_state: u8,
    /// gstax only; gstat leaves the defaults.
    pub v3: bool,
    /// 7
    pub lease: LeaseState,
    /// 8, while running
    pub lease_remain_s: u32,
    /// 9, a lease command within the last 300 s
    pub lease_client: bool,
    /// 10
    pub lease_timeout_min: u16,
    /// 11, bit v: valve v at its lease failsafe
    pub failsafe_mask: u16,
    /// 12
    pub safe_mode: bool,
    /// 13, watchdog resets in the current window
    pub wdg_resets: u8,
    /// 14..17, since start-up
    pub uart_ore: u32,
    pub uart_fe: u32,
    pub uart_ne: u32,
    pub rx_dropped: u32,
    /// 18, STM_CFG_*
    pub cfg_flags: u8,
    /// 19, loads that repaired/defaulted a block
    pub cfg_events: u32,
    /// 20
    pub eep_writes: u32,
    /// 21, s since the last complete temperature cycle
    pub temp_age_s: u32,
    /// 22, s since the last 1-Wire enumeration
    pub ow_scan_age_s: u32,
    /// 23, STM_SYS_*
    pub sys_flags: u8,
}

/// glcfg (v3): "glcfg <timeoutMin> <fs0> ... <fs11>".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LeaseConfigReply {
    /// 0..1440
    pub timeout_min: u16,
    /// 0..100 or FAILSAFE_HOLD
    pub failsafe_pct: [u8; VALVE_COUNT as usize],
}

/// slhbt (v3) ok form: "slhbt <lease> <remainS>".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HeartbeatReply {
    pub lease: LeaseState,
    pub remain_s: u32,
}

/// Replies without payload ("stgtp", "stons", "staln", "stlnm", "smotc", "staop ", "stdet ",
/// "masns ", "reset ", "stvls v", "scalx ok", "stlnt", "slcfg ok", "ssafe ok"). `error` for the
/// "smotc err" / "scalx err" / "slcfg err" / "ssafe err" / "slhbt err" forms.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ack {
    pub error: bool,
    /// stvls only
    pub valve: u8,
}

impl Default for Ack {
    fn default() -> Self {
        Self {
            error: false,
            valve: NO_VALVE,
        }
    }
}

/// A parsed reply. Plain struct (not an enum) like the C++ one; only the member selected by
/// `cmd` is meaningful. About 1.4 KB: keep exactly one per task, never on small stacks.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Reply {
    pub cmd: Cmd,
    /// "goned error" (v1 gvlon error)
    pub gvlon_error: bool,
    /// Stgtp Stons Stvls Masns Staop Staln Stdet Stlnm Smotc Reset Scalx Stlnt Slcfg Ssafe,
    /// Slhbt error form
    pub ack: Ack,
    /// Gvlvd
    pub valve_data: ValveData,
    /// Gvlst
    pub valve_states: ValveStates,
    /// Gonec, Gowvc
    pub one_wire_list: OneWireList,
    /// Goned
    pub temp_data: TempData,
    /// Gowvd
    pub volt_data: VoltData,
    /// Gvlon
    pub valve_sensors: ValveSensors,
    /// Gtgtp
    pub target: TargetReply,
    /// Gtlnm
    pub learn_movements: u16,
    /// Gmotc
    pub motor_chars: MotorChars,
    /// Gvers
    pub version: Version,
    /// Gvers, 0 when absent
    pub build: u32,
    /// Ghwin (DBGMCU IDCODE & 0xFFF)
    pub hw_id: u16,
    /// Eepst: "1" = nothing pending
    pub eeprom_idle: bool,
    /// Gproto
    pub proto: u8,
    /// Gvlvx, Gvlvy (v3 set)
    pub valve_ex: ValveEx,
    /// Gprof
    pub profile: Profile,
    /// Svmov
    pub service_move: IndexedResult,
    /// Gcalx
    pub breakaway: Breakaway,
    /// Gstat, Gstax (v3 set)
    pub status: StmStatus,
    /// Glcfg
    pub lease_config: LeaseConfigReply,
    /// Slhbt ok form
    pub heartbeat: HeartbeatReply,
    /// Sfspo
    pub failsafe: IndexedResult,
    /// Sstop
    pub stop: IndexedResult,
    /// Gtlnt
    pub learn_time: u32,
}

/// Tokens of a reply line; one more is TooLong.
const MAX_TOKENS: usize = 40;
const VALVE_MAX: u8 = VALVE_COUNT - 1;
/// gstax failsafe mask: one bit per valve
const VALVE_MASK: u16 = (1 << VALVE_COUNT) - 1;
const VALVE_EX_FIELDS: usize = 19;
const VALVE_EX_V3_EXTRA: usize = 6;
const STATUS_FIELDS: usize = 6;
const STATUS_V3_EXTRA: usize = 17;
const LEASE_CONFIG_FIELDS: usize = 1 + VALVE_COUNT as usize;

type Parsed = Result<(), ParseStatus>;

/// 1..10 decimal digits, nothing else.
fn all_digits(s: &[u8]) -> bool {
    (1..=10).contains(&s.len()) && s.iter().all(u8::is_ascii_digit)
}

/// Unsigned decimal in [min, max]. Non-digits -> BadNumber, digits whose value is outside the
/// range (including > 2^32-1) -> OutOfRange.
fn read_u(s: &[u8], min: u32, max: u32) -> Result<u32, ParseStatus> {
    if !all_digits(s) {
        return Err(ParseStatus::BadNumber);
    }
    match parse_uint(s, max) {
        Some(v) if v >= min => Ok(v),
        _ => Err(ParseStatus::OutOfRange),
    }
}

fn read_u8(s: &[u8], max: u8) -> Result<u8, ParseStatus> {
    read_u(s, 0, u32::from(max)).map(|v| v as u8)
}

fn read_u16(s: &[u8], max: u16) -> Result<u16, ParseStatus> {
    read_u(s, 0, u32::from(max)).map(|v| v as u16)
}

fn read_u32(s: &[u8]) -> Result<u32, ParseStatus> {
    read_u(s, 0, u32::MAX)
}

fn read_bool(s: &[u8]) -> Result<bool, ParseStatus> {
    read_u(s, 0, 1).map(|v| v != 0)
}

/// An enum field: its numbers, anything else OutOfRange.
fn read_enum<T>(s: &[u8], from_raw: fn(u8) -> Option<T>) -> Result<T, ParseStatus> {
    from_raw(read_u8(s, u8::MAX)?).ok_or(ParseStatus::OutOfRange)
}

/// Signed decimal: an optional '-' and 1..10 digits (else BadNumber) in [min, max] (else
/// OutOfRange).
fn read_i(s: &[u8], min: i32, max: i32) -> Result<i32, ParseStatus> {
    let digits = match s {
        [b'-', rest @ ..] => rest,
        _ => s,
    };
    if !all_digits(digits) {
        return Err(ParseStatus::BadNumber);
    }
    parse_int(s, min, max).ok_or(ParseStatus::OutOfRange)
}

fn read_i16(s: &[u8]) -> Result<i16, ParseStatus> {
    read_i(s, i32::from(i16::MIN), i32::from(i16::MAX)).map(|v| v as i16)
}

fn read_id(s: &[u8]) -> Result<OneWireId, ParseStatus> {
    parse_one_wire_id(s).ok_or(ParseStatus::BadOneWireId)
}

/// Fields a later STM appends to a v3 reply: strict decimal, ignored.
fn read_extras(extras: &[&[u8]]) -> Parsed {
    for s in extras {
        read_u32(s)?;
    }
    Ok(())
}

/// Splits a comma list "a,b,c" (one trailing comma allowed) into exactly `slots.len()`
/// elements, each parsed into its slot in order. An empty element, or one more than there are
/// slots, is BadFormat where it starts; too few elements are BadFormat at the end.
fn parse_list<T>(
    list: &[u8],
    slots: &mut [T],
    parse: impl Fn(&[u8]) -> Result<T, ParseStatus>,
) -> Parsed {
    let body = list.strip_suffix(b",").unwrap_or(list); // tolerated v1 trailing comma
    let mut items = body.split(|&c| c == b',');
    for slot in slots.iter_mut() {
        match items.next() {
            Some(item) if !item.is_empty() => *slot = parse(item)?,
            _ => return Err(ParseStatus::BadFormat),
        }
    }
    match items.next() {
        Some(_) => Err(ParseStatus::BadFormat),
        None => Ok(()),
    }
}

fn parse_ack(a: &[&[u8]]) -> Parsed {
    if a.is_empty() {
        Ok(())
    } else {
        Err(ParseStatus::BadArgCount)
    }
}

/// "<cmd>" or "<cmd> err" (smotc: `bare_is_ok`) / "<cmd> ok|err" (scalx, slcfg, ssafe).
fn parse_ok_err(a: &[&[u8]], bare_is_ok: bool, ack: &mut Ack) -> Parsed {
    match a {
        [] if bare_is_ok => Ok(()),
        [form] if *form == b"err" => {
            ack.error = true;
            Ok(())
        }
        [form] if !bare_is_ok && *form == b"ok" => Ok(()),
        [_] => Err(ParseStatus::BadFormat),
        _ => Err(ParseStatus::BadArgCount),
    }
}

/// One unsigned field in [min, max].
fn parse_single(a: &[&[u8]], min: u32, max: u32) -> Result<u32, ParseStatus> {
    match a {
        [s] => read_u(s, min, max),
        _ => Err(ParseStatus::BadArgCount),
    }
}

fn parse_valve_data(a: &[&[u8]], d: &mut ValveData) -> Parsed {
    let [valve, position, current, raw, temp1, temp2, moves, oc, cc, dead_zone, retries] = a else {
        return Err(ParseStatus::BadArgCount);
    };
    d.valve = read_u8(valve, VALVE_MAX)?;
    d.position = read_u8(position, 100)?;
    d.mean_current = read_u16(current, u16::MAX)?;
    let raw = read_u8(raw, u8::MAX)?;
    d.temp1 = read_i16(temp1)?;
    d.temp2 = read_i16(temp2)?;
    d.moves = read_u32(moves)?;
    d.open_count = read_u32(oc)?;
    d.close_count = read_u32(cc)?;
    d.dead_zone = read_i(dead_zone, i32::MIN, i32::MAX)?;
    d.calib_retries = read_u8(retries, u8::MAX)?;
    d.status = raw & 0x7F;
    d.calibrating = (raw & 0x80) != 0;
    Ok(())
}

fn parse_valve_states(a: &[&[u8]], s: &mut ValveStates) -> Parsed {
    let [n, list] = a else {
        return Err(ParseStatus::BadArgCount);
    };
    read_u(n, u32::from(VALVE_COUNT), u32::from(VALVE_COUNT))?;
    parse_list(list, &mut s.status, |item| read_u8(item, u8::MAX))
}

fn parse_one_wire_list(a: &[&[u8]], max_count: u8, l: &mut OneWireList) -> Parsed {
    let (n, list) = match a {
        [n] => (n, None),
        [n, list] => (n, Some(list)),
        _ => return Err(ParseStatus::BadArgCount),
    };
    let n = read_u8(n, max_count)?;
    l.count = n;
    let Some(list) = list else {
        return Ok(());
    };
    l.has_list = true;
    let slots = l.ids.get_mut(..usize::from(n)).unwrap_or_default();
    parse_list(list, slots, read_id)
}

/// "goned id temp" / "goned 0" (and "gowvd ..."): the reading, None for the invalid form.
fn parse_sensor_data(
    a: &[&[u8]],
    min: i32,
    max: i32,
) -> Result<Option<(OneWireId, i32)>, ParseStatus> {
    match a {
        [zero] if *zero == b"0" => Ok(None),
        [_] => Err(ParseStatus::BadFormat),
        [id, value] => {
            let id = read_id(id)?;
            Ok(Some((id, read_i(value, min, max)?)))
        }
        _ => Err(ParseStatus::BadArgCount),
    }
}

fn parse_valve_sensors(a: &[&[u8]], s: &mut ValveSensors) -> Parsed {
    match a {
        [valve, id1, id2] => {
            let v = read_u8(valve, VALVE_MAX)?;
            s.valve = v;
            let pair = s
                .ids
                .get_mut(usize::from(v))
                .ok_or(ParseStatus::OutOfRange)?;
            pair[0] = read_id(id1)?;
            pair[1] = read_id(id2)?;
            Ok(())
        }
        [n, list] => {
            read_u(n, u32::from(VALVE_COUNT), u32::from(VALVE_COUNT))?;
            s.is_list = true;
            parse_list(list, s.ids.as_flattened_mut(), read_id)
        }
        _ => Err(ParseStatus::BadArgCount),
    }
}

/// 3..5 fields; missing ones keep their defaults.
fn parse_motor_chars(a: &[&[u8]], m: &mut MotorChars) -> Parsed {
    let [low, high, start, rest @ ..] = a else {
        return Err(ParseStatus::BadArgCount);
    };
    if rest.len() > 2 {
        return Err(ParseStatus::BadArgCount);
    }
    m.field_count = a.len() as u8;
    m.low_factor = read_u8(low, u8::MAX)?;
    m.high_factor = read_u8(high, u8::MAX)?;
    m.start_on_power = read_u8(start, u8::MAX)?;
    if let Some(s) = rest.first() {
        m.min_counts = read_u16(s, u16::MAX)?;
    }
    if let Some(s) = rest.get(1) {
        m.max_calib_retries = read_u8(s, u8::MAX)?;
    }
    Ok(())
}

fn parse_version_reply(a: &[&[u8]], r: &mut Reply) -> Parsed {
    let (text, build) = match a {
        [text] => (text, None),
        [text, build] => (text, Some(build)),
        _ => return Err(ParseStatus::BadArgCount),
    };
    r.version = parse_version(text);
    if !r.version.valid {
        return Err(ParseStatus::BadFormat);
    }
    if let Some(build) = build {
        r.build = read_u32(build)?;
    }
    Ok(())
}

/// The 19 gvlvx fields, also the first 19 of gvlvy.
fn read_valve_ex(f: &[&[u8]; VALVE_EX_FIELDS], x: &mut ValveEx) -> Parsed {
    let [valve, raw, position, target, current, oc, cc, dead_zone, retries, rest @ ..] = f;
    let [moves, cal, early, rejected, dir, requested, counted, stop, peak, duration] = rest;
    x.valve = read_u8(valve, VALVE_MAX)?;
    let raw = read_u8(raw, u8::MAX)?;
    x.position = read_u8(position, 100)?;
    x.target = read_u8(target, 100)?;
    x.mean_current = read_u16(current, u16::MAX)?;
    x.open_count = read_u32(oc)?;
    x.close_count = read_u32(cc)?;
    x.dead_zone = read_i(dead_zone, i32::MIN, i32::MAX)?;
    x.calib_retries = read_u8(retries, u8::MAX)?;
    x.moves = read_u32(moves)?;
    let cal = read_u8(cal, CAL_STATE_MASK + CAL_FLAG_MASK)?;
    x.early_stops = read_u32(early)?;
    x.cmd_rejected = read_u32(rejected)?;
    x.last_move.dir = read_enum(dir, MoveDir::from_raw)?;
    x.last_move.requested_counts = read_u32(requested)?;
    x.last_move.counted_counts = read_u32(counted)?;
    x.last_move.stop = read_enum(stop, StopReason::from_raw)?;
    x.last_move.peak_current = read_u16(peak, u16::MAX)?;
    x.last_move.duration_ms = read_u32(duration)?;
    x.status = raw & 0x7F;
    x.cal_state = cal & CAL_STATE_MASK;
    x.cal_flags = cal & CAL_FLAG_MASK;
    x.calibrating = x.cal_state == CAL_STATE_RUNNING || (raw & 0x80) != 0;
    Ok(())
}

/// gvlvy: the 19 gvlvx fields, 6 more, then numeric extras.
fn parse_valve_ex_v3(a: &[&[u8]], x: &mut ValveEx) -> Parsed {
    let Some((ex, rest)) = a.split_first_chunk::<VALVE_EX_FIELDS>() else {
        return Err(ParseStatus::BadArgCount);
    };
    let Some(([flags, fault, fs_pct, drive, retry_s, retries], extras)) =
        rest.split_first_chunk::<VALVE_EX_V3_EXTRA>()
    else {
        return Err(ParseStatus::BadArgCount);
    };
    read_valve_ex(ex, x)?;
    x.flags = read_u16(flags, u16::MAX)?;
    x.fault = read_u8(fault, u8::MAX)?;
    let fs_pct = read_u8(fs_pct, u8::MAX)?;
    x.drive = read_u8(drive, 100)?;
    x.retry_s = read_u32(retry_s)?;
    x.retries = read_u8(retries, u8::MAX)?;
    if !failsafe_pct_valid(u32::from(fs_pct)) {
        return Err(ParseStatus::OutOfRange);
    }
    x.fs_pct = fs_pct;
    x.v3 = true;
    read_extras(extras)
}

fn parse_profile(a: &[&[u8]], p: &mut Profile) -> Parsed {
    let [valve, n, samples @ ..] = a else {
        return Err(ParseStatus::BadArgCount);
    };
    let valve = read_u8(valve, VALVE_MAX)?;
    let n = read_u8(n, PROFILE_MAX_SAMPLES)?;
    if samples.len() != usize::from(n) {
        return Err(ParseStatus::BadArgCount);
    }
    p.valve = valve;
    for (slot, sample) in p.samples.iter_mut().zip(samples) {
        let mut parts = sample.splitn(2, |&c| c == b':');
        let count = parts.next().unwrap_or_default();
        let current = parts.next().ok_or(ParseStatus::BadFormat)?;
        slot.count = read_u32(count)?;
        slot.current = read_u16(current, u16::MAX)?;
    }
    p.count = n;
    Ok(())
}

/// "<cmd> <idx> ok" / "<cmd> <idx> err <code>". idx -1..11, or also 255 (all valves) when
/// `allow_all`; -1 (the STM could not read the index) only with an error.
fn parse_indexed(a: &[&[u8]], allow_all: bool, res: &mut IndexedResult) -> Parsed {
    let (index, form, code) = match a {
        [index, form] => (index, form, None),
        [index, form, code] => (index, form, Some(code)),
        _ => return Err(ParseStatus::BadArgCount),
    };
    let max = if allow_all { ALL_VALVES } else { VALVE_MAX };
    let v = read_i(index, -1, i32::from(max))?;
    if v > i32::from(VALVE_MAX) && v != i32::from(ALL_VALVES) {
        return Err(ParseStatus::OutOfRange);
    }
    res.index = v as i16;
    let Some(code) = code else {
        if *form != b"ok" || v < 0 {
            return Err(ParseStatus::BadFormat);
        }
        res.ok = true;
        return Ok(());
    };
    if *form != b"err" {
        return Err(ParseStatus::BadFormat);
    }
    res.error_code = read_u16(code, u16::MAX)?;
    Ok(())
}

fn parse_breakaway(a: &[&[u8]], b: &mut Breakaway) -> Parsed {
    let [enable, step, max_ma] = a else {
        return Err(ParseStatus::BadArgCount);
    };
    b.enable = read_bool(enable)?;
    b.step_pct = read_u8(step, 100)?;
    b.max_ma = read_u(max_ma, 20, 60)? as u8;
    Ok(())
}

/// The 6 gstat fields, also the first 6 of gstax.
fn read_status(f: &[&[u8]; STATUS_FIELDS], s: &mut StmStatus) -> Parsed {
    let [uptime, resets, boot_reason, rx_overflow, parse_errors, eep_state] = f;
    s.uptime_s = read_u32(uptime)?;
    s.resets = read_u32(resets)?;
    s.boot_reason = read_u32(boot_reason)?;
    s.rx_overflow = read_u32(rx_overflow)?;
    s.parse_errors = read_u32(parse_errors)?;
    s.eep_state = read_u8(eep_state, u8::MAX)?;
    Ok(())
}

/// gstax: the 6 gstat fields, 17 more, then numeric extras.
fn parse_status_v3(a: &[&[u8]], s: &mut StmStatus) -> Parsed {
    let Some((head, rest)) = a.split_first_chunk::<STATUS_FIELDS>() else {
        return Err(ParseStatus::BadArgCount);
    };
    let Some((v3, extras)) = rest.split_first_chunk::<STATUS_V3_EXTRA>() else {
        return Err(ParseStatus::BadArgCount);
    };
    read_status(head, s)?;
    let [lease, remain, client, timeout, mask, safe, wdg, rest @ ..] = v3;
    let [ore, fe, ne, dropped, cfg, cfg_events, eep_writes, temp_age, scan_age, sys] = rest;
    s.lease = read_enum(lease, LeaseState::from_raw)?;
    s.lease_remain_s = read_u32(remain)?;
    s.lease_client = read_bool(client)?;
    s.lease_timeout_min = read_u16(timeout, FAILSAFE_TIMEOUT_MAX_MIN)?;
    s.failsafe_mask = read_u16(mask, VALVE_MASK)?;
    s.safe_mode = read_bool(safe)?;
    s.wdg_resets = read_u8(wdg, u8::MAX)?;
    s.uart_ore = read_u32(ore)?;
    s.uart_fe = read_u32(fe)?;
    s.uart_ne = read_u32(ne)?;
    s.rx_dropped = read_u32(dropped)?;
    s.cfg_flags = read_u8(cfg, u8::MAX)?;
    s.cfg_events = read_u32(cfg_events)?;
    s.eep_writes = read_u32(eep_writes)?;
    s.temp_age_s = read_u32(temp_age)?;
    s.ow_scan_age_s = read_u32(scan_age)?;
    s.sys_flags = read_u8(sys, u8::MAX)?;
    s.v3 = true;
    read_extras(extras)
}

fn parse_lease_config(a: &[&[u8]], c: &mut LeaseConfigReply) -> Parsed {
    let Some(([timeout, pcts @ ..], extras)) = a.split_first_chunk::<LEASE_CONFIG_FIELDS>() else {
        return Err(ParseStatus::BadArgCount);
    };
    c.timeout_min = read_u16(timeout, FAILSAFE_TIMEOUT_MAX_MIN)?;
    for (slot, s) in c.failsafe_pct.iter_mut().zip(pcts) {
        let v = read_u8(s, u8::MAX)?;
        if !failsafe_pct_valid(u32::from(v)) {
            return Err(ParseStatus::OutOfRange);
        }
        *slot = v;
    }
    read_extras(extras)
}

/// "slhbt <lease> <remainS>" or the error form "slhbt err".
fn parse_heartbeat(a: &[&[u8]], r: &mut Reply) -> Parsed {
    if let [form] = a {
        if *form != b"err" {
            return Err(ParseStatus::BadFormat);
        }
        r.ack.error = true;
        return Ok(());
    }
    let Some(([lease, remain], extras)) = a.split_first_chunk::<2>() else {
        return Err(ParseStatus::BadArgCount);
    };
    r.heartbeat.lease = read_enum(lease, LeaseState::from_raw)?;
    r.heartbeat.remain_s = read_u32(remain)?;
    read_extras(extras)
}

fn parse_payload(cmd: Cmd, a: &[&[u8]], r: &mut Reply) -> Parsed {
    match cmd {
        Cmd::None => Err(ParseStatus::UnknownCommand),
        Cmd::Stgtp
        | Cmd::Stons
        | Cmd::Masns
        | Cmd::Staop
        | Cmd::Staln
        | Cmd::Stdet
        | Cmd::Stlnm
        | Cmd::Reset
        | Cmd::Stlnt => parse_ack(a),
        Cmd::Smotc => parse_ok_err(a, true, &mut r.ack),
        Cmd::Scalx | Cmd::Slcfg | Cmd::Ssafe => parse_ok_err(a, false, &mut r.ack),
        Cmd::Stvls => {
            r.ack.valve = parse_single(a, 0, u32::from(VALVE_MAX))? as u8;
            Ok(())
        }
        Cmd::Gtgtp => {
            let [valve, target] = a else {
                return Err(ParseStatus::BadArgCount);
            };
            r.target.valve = read_u8(valve, VALVE_MAX)?;
            r.target.target = read_u8(target, 100)?;
            Ok(())
        }
        Cmd::Gvlvd => parse_valve_data(a, &mut r.valve_data),
        Cmd::Gvlst => parse_valve_states(a, &mut r.valve_states),
        Cmd::Gonec => parse_one_wire_list(a, TEMP_SLOT_COUNT, &mut r.one_wire_list),
        Cmd::Gowvc => parse_one_wire_list(a, VOLT_SLOT_COUNT, &mut r.one_wire_list),
        Cmd::Goned => {
            let range = (i32::from(i16::MIN), i32::from(i16::MAX));
            if let Some((id, value)) = parse_sensor_data(a, range.0, range.1)? {
                r.temp_data.valid = true;
                r.temp_data.id = id;
                r.temp_data.value = value as i16;
            }
            Ok(())
        }
        Cmd::Gowvd => {
            if let Some((id, vad)) = parse_sensor_data(a, i32::MIN, i32::MAX)? {
                r.volt_data.valid = true;
                r.volt_data.id = id;
                r.volt_data.vad = vad;
            }
            Ok(())
        }
        Cmd::Gvlon => parse_valve_sensors(a, &mut r.valve_sensors),
        Cmd::Gtlnm => {
            r.learn_movements = parse_single(a, 0, u32::from(u16::MAX))? as u16;
            Ok(())
        }
        Cmd::Gmotc => parse_motor_chars(a, &mut r.motor_chars),
        Cmd::Gvers => parse_version_reply(a, r),
        Cmd::Ghwin => {
            r.hw_id = parse_single(a, 0, 0xFFF)? as u16;
            Ok(())
        }
        Cmd::Eepst => {
            r.eeprom_idle = parse_single(a, 0, 1)? != 0;
            Ok(())
        }
        Cmd::Gproto => {
            r.proto = parse_single(a, 1, u32::from(u8::MAX))? as u8;
            Ok(())
        }
        Cmd::Gvlvx => {
            let f =
                <&[&[u8]; VALVE_EX_FIELDS]>::try_from(a).map_err(|_| ParseStatus::BadArgCount)?;
            read_valve_ex(f, &mut r.valve_ex)
        }
        Cmd::Gprof => parse_profile(a, &mut r.profile),
        Cmd::Svmov => parse_indexed(a, false, &mut r.service_move),
        Cmd::Gcalx => parse_breakaway(a, &mut r.breakaway),
        Cmd::Gstat => {
            let f = <&[&[u8]; STATUS_FIELDS]>::try_from(a).map_err(|_| ParseStatus::BadArgCount)?;
            read_status(f, &mut r.status)
        }
        Cmd::Gvlvy => parse_valve_ex_v3(a, &mut r.valve_ex),
        Cmd::Gstax => parse_status_v3(a, &mut r.status),
        Cmd::Slhbt => parse_heartbeat(a, r),
        Cmd::Sfspo => parse_indexed(a, true, &mut r.failsafe),
        Cmd::Glcfg => parse_lease_config(a, &mut r.lease_config),
        Cmd::Sstop => parse_indexed(a, true, &mut r.stop),
        Cmd::Gtlnt => {
            r.learn_time = parse_single(a, 0, u32::MAX)?;
            Ok(())
        }
    }
}

/// Parses one complete line (no CR/LF; all of `line`, NUL not required). Tokens are separated
/// by one or more spaces; trailing spaces are allowed. Every numeric field is strict decimal and
/// range-checked; on any error the status says why and `out` is reset to `Reply::default()`.
/// gvlvx has exactly 19 fields and gstat exactly 6. The v3 replies with a fixed field list
/// (gvlvy >= 25, gstax >= 23, glcfg >= 13, slhbt ok form >= 2) accept extra fields a later STM
/// may append: each must be a strict decimal 0..2^32-1 and is ignored.
///
/// Fills the caller's [`Reply`] (C++ `Reply& out`) instead of returning one: it is 1.4 KB.
pub fn parse_reply(line: &[u8], out: &mut Reply) -> ParseStatus {
    *out = Reply::default();
    // C++ parseReply(nullptr, ...) == Empty: a slice is never null.
    if line.len() > STM_MAX_LINE_LEN {
        return ParseStatus::TooLong;
    }
    let mut tokens = heapless::Vec::<&[u8], MAX_TOKENS>::new();
    for token in line.split(|&c| c == b' ').filter(|t| !t.is_empty()) {
        if tokens.push(token).is_err() {
            return ParseStatus::TooLong;
        }
    }
    let Some((name, args)) = tokens.split_first() else {
        return ParseStatus::Empty;
    };
    let cmd = cmd_from_name(name);
    // v1 quirk: the gvlon error reply carries the goned prefix.
    if cmd == Cmd::Goned && matches!(args, [only] if *only == b"error") {
        out.cmd = Cmd::Gvlon;
        out.gvlon_error = true;
        return ParseStatus::Ok;
    }
    match parse_payload(cmd, args, out) {
        Ok(()) => {
            out.cmd = cmd;
            ParseStatus::Ok
        }
        Err(st) => {
            *out = Reply::default();
            st
        }
    }
}

fn index_matches(r: &IndexedResult, valve: u8) -> bool {
    r.index < 0 || r.index == i16::from(valve)
}

/// A valid sensor reading answers the request only with the expected id (unless the id at that
/// bus index is unknown).
fn sensor_matches(valid: bool, id: &OneWireId, expect: &OneWireId) -> bool {
    !valid || is_zero(expect) || id == expect
}

/// True when `rep` is the answer to `req`: same command and, where the reply carries it, the
/// same valve / bus index. Special cases:
///  - Gvlon request accepts a `gvlon_error` reply.
///  - Goned/Gowvd requests accept the invalid form ("goned 0") -- index match cannot be checked
///    there; a valid reading must carry `req.expect` unless that is zero (a late reply for
///    another bus index is not taken).
///  - Gonec/Gowvc list requests accept a count-only reply with count 0.
///  - Svmov/Sfspo/Sstop accept index -1 (the STM could not read the index).
pub fn reply_matches(req: &RequestLine, rep: &Reply) -> bool {
    if req.cmd == Cmd::None || req.text.is_empty() || rep.cmd != req.cmd {
        return false;
    }
    match req.cmd {
        Cmd::Gvlvd => rep.valve_data.valve == req.valve,
        Cmd::Gvlvx | Cmd::Gvlvy => rep.valve_ex.valve == req.valve,
        Cmd::Gtgtp => rep.target.valve == req.valve,
        Cmd::Gprof => rep.profile.valve == req.valve,
        Cmd::Svmov => index_matches(&rep.service_move, req.valve),
        Cmd::Sfspo => index_matches(&rep.failsafe, req.valve),
        Cmd::Sstop => index_matches(&rep.stop, req.valve),
        Cmd::Goned => sensor_matches(rep.temp_data.valid, &rep.temp_data.id, &req.expect),
        Cmd::Gowvd => sensor_matches(rep.volt_data.valid, &rep.volt_data.id, &req.expect),
        Cmd::Stvls => rep.ack.valve == req.valve,
        Cmd::Gvlon => {
            rep.gvlon_error
                || if req.valve == ALL_VALVES {
                    rep.valve_sensors.is_list
                } else {
                    !rep.valve_sensors.is_list && rep.valve_sensors.valve == req.valve
                }
        }
        Cmd::Gonec | Cmd::Gowvc => {
            if req.arg == u16::from(ALL_VALVES) {
                rep.one_wire_list.has_list || rep.one_wire_list.count == 0
            } else {
                !rep.one_wire_list.has_list
            }
        }
        _ => true,
    }
}

/// Maps a gvlon/list id to a 1-based config slot using the configured slot ids (`slot_ids[i]`
/// for slot i + 1; zero ids are "empty slot"; at most 255 slots count, the C++ `uint8_t
/// slotCount`). Returns 0 when the id is zero, fails CRC, or is not configured.
pub fn resolve_temp_slot(id: &OneWireId, slot_ids: &[OneWireId]) -> u8 {
    // C++ resolveTempSlot(id, nullptr, n) == 0: a slice is never null.
    if is_zero(id) || !crc_valid(id) {
        return 0;
    }
    slot_ids
        .iter()
        .zip(1..=u8::MAX)
        .find(|(slot, _)| *slot == id)
        .map_or(0, |(_, n)| n)
}

/// Chip name for a DBGMCU/bootloader PID: 0x413 "STM32F40xx/41xx", 0x423 "STM32F401xB/C",
/// 0x431 "STM32F411xx", 0x433 "STM32F401xD/E", else "Unknown Chip".
pub fn stm_chip_name(pid: u16) -> &'static str {
    match pid {
        0x413 => "STM32F40xx/41xx",
        0x423 => "STM32F401xB/C",
        0x431 => "STM32F411xx",
        0x433 => "STM32F401xD/E",
        _ => "Unknown Chip",
    }
}

#[cfg(test)]
mod tests;
