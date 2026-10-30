//! The flow layer of a valve (Rust 2.2.8, STM 2.2.8): the external target 0..100 % of a valve
//! with a measured curve means 0..qmax l/min instead of a stroke percentage. The whole flow
//! band of a floor-heating loop lies in 5..11 % of the stroke, so the layer positions the valve
//! itself with service moves from a reference end stop (P1: a move to the end stop, one move
//! of the curve's pulses away from it) and asks the STM to keep that position (`skeep`); the
//! STM's own target of such a valve is the fallback position it drives to when the hold is
//! lost. Hardware-free: the per-valve configuration with its NVS blob and JSON forms, the curve
//! mapping both ways and the positioning state machine that hands requests to the session and
//! reads the valve model. Owned by [`crate::stm_session::StmSession`].

use crate::common::{elapsed_ms, parse_uint, VALVE_COUNT};
use crate::config::crc32;
use crate::event_log::{event_default_severity, make_event, Event, EventCode};
use crate::json_writer::JsonWriter;
use crate::stm_codec::{
    stop_reason_name, MoveDir, StopReason, STM_FLAG_FS_LEASE, STM_FLAG_SVC_KEEP,
};
use crate::valve_model::{TargetSource, ValveModel, ValveState};

/// Valves (the arrays of the layer).
const N: usize = VALVE_COUNT as usize;

/// Points of a curve.
pub const FLOW_POINTS_MIN: usize = 2;
pub const FLOW_POINTS_MAX: usize = 16;
/// Pulses of a position (the svmov range).
pub const FLOW_COUNTS_MAX: u16 = 10000;
/// qmax in ml/min: 0.01 .. 20 l/min.
pub const FLOW_QMAX_MIN_ML: u16 = 10;
pub const FLOW_QMAX_MAX_ML: u16 = 20000;
/// Dead band in ml/min: 0 .. 5 l/min.
pub const FLOW_DEADBAND_MAX_ML: u16 = 5000;
/// Seconds between two positionings: 0 .. 1 h.
pub const FLOW_INTERVAL_MAX_S: u16 = 3600;
/// Pulses added to the curve counts.
pub const FLOW_OFFSET_MAX: i16 = 10000;
/// Gain x 1000: 0.1 .. 10.
pub const FLOW_GAIN_MIN_MILLI: u16 = 100;
pub const FLOW_GAIN_MAX_MILLI: u16 = 10000;
/// Current bounds of the moves (the svmov range).
pub const FLOW_MA_MIN: u8 = 5;
pub const FLOW_MA_MAX: u8 = 60;

/// The end stop the curve counts from.
#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RefSide {
    /// pulses above the closed end stop (the flow rises along the curve)
    #[default]
    Closed = 0,
    /// pulses below the open end stop (the flow falls along the curve)
    Open = 1,
}

impl RefSide {
    pub fn from_raw(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Closed),
            1 => Some(Self::Open),
            _ => None,
        }
    }
}

/// "closed", "open".
pub fn ref_side_name(s: RefSide) -> &'static str {
    match s {
        RefSide::Closed => "closed",
        RefSide::Open => "open",
    }
}

/// One point of the measured curve: pulses from the reference end stop and the flow there.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FlowPoint {
    pub counts: u16,
    /// l/min x 1000
    pub flow_ml: u16,
}

/// The flow configuration of one valve (API `GET/POST /api/valves/{n}/flow`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FlowConfig {
    pub enabled: bool,
    /// l/min x 1000 that a target of 100 % means
    pub qmax_ml: u16,
    /// mA: the end-stop threshold of a closing move
    pub ref_ma: u8,
    /// mA: the threshold of an opening move
    pub open_ma: u8,
    /// stroke % the STM drives to when the hold is lost (its target while the layer runs)
    pub fallback_pct: u8,
    /// s between two positionings of the valve
    pub min_interval_s: u16,
    /// ml/min: a smaller change of the target flow is not applied
    pub deadband_ml: u16,
    /// pulses added to the curve counts (the flowbench correction)
    pub offset: i16,
    /// x 1000: the curve counts are multiplied by it before the offset
    pub gain_milli: u16,
    /// the end stop the positioning starts from (the curve always counts from the closed one)
    pub ref_side: RefSide,
    /// open reference: the counted open stroke from the closed end stop in pulses; the final
    /// closing move is this minus the target counts
    pub stroke_counts: u16,
    /// closed reference: a warm-up of the end stop (close, open 300, close) before the final
    /// move
    pub pre_close: bool,
    /// points in use (0 or [`FLOW_POINTS_MIN`]..[`FLOW_POINTS_MAX`])
    pub count: u8,
    pub points: [FlowPoint; FLOW_POINTS_MAX],
}

impl FlowConfig {
    /// Disabled, 2.7 l/min, 34 / 30 mA, fallback 15 %, 5 min, 0.05 l/min, no correction,
    /// closed reference without a warm-up, no points.
    pub const DEFAULT: Self = Self {
        enabled: false,
        qmax_ml: 2700,
        ref_ma: 34,
        open_ma: 30,
        fallback_pct: 15,
        min_interval_s: 300,
        deadband_ml: 50,
        offset: 0,
        gain_milli: 1000,
        ref_side: RefSide::Closed,
        stroke_counts: 0,
        pre_close: false,
        count: 0,
        points: [FlowPoint {
            counts: 0,
            flow_ml: 0,
        }; FLOW_POINTS_MAX],
    };

    /// The points in use.
    pub fn points(&self) -> &[FlowPoint] {
        let n = usize::from(self.count).min(FLOW_POINTS_MAX);
        self.points.get(..n).unwrap_or_default()
    }

    /// The members that change where a target lands: a change repositions the valve.
    fn curve_key(
        &self,
    ) -> (
        u16,
        i16,
        u16,
        RefSide,
        u16,
        u8,
        [FlowPoint; FLOW_POINTS_MAX],
    ) {
        (
            self.qmax_ml,
            self.offset,
            self.gain_milli,
            self.ref_side,
            self.stroke_counts,
            self.count,
            self.points,
        )
    }
}

impl Default for FlowConfig {
    fn default() -> Self {
        Self::DEFAULT
    }
}

// ---------------------------------------------------------------- validation

/// Why a flow configuration (or its JSON) is refused; [`flow_error_text`] is the API detail.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlowError {
    Malformed = 0,
    UnknownKey = 1,
    Enabled = 2,
    Qmax = 3,
    RefMa = 4,
    OpenMa = 5,
    FallbackPct = 6,
    MinInterval = 7,
    Deadband = 8,
    Offset = 9,
    Gain = 10,
    /// the shape of `points`
    Points = 11,
    /// the number of points
    PointCount = 12,
    PointCounts = 13,
    PointFlow = 14,
    RefSide = 15,
    PreClose = 16,
    StrokeCounts = 17,
}

/// The API detail of an error.
pub fn flow_error_text(e: FlowError) -> &'static str {
    match e {
        FlowError::Malformed => "malformed JSON",
        FlowError::UnknownKey => "unknown key",
        FlowError::Enabled => "enabled true|false",
        FlowError::Qmax => "qmax 0.01..20 l/min",
        FlowError::RefMa => "refmA 5..60",
        FlowError::OpenMa => "openmA 5..60",
        FlowError::FallbackPct => "fallbackPct 0..100",
        FlowError::MinInterval => "minIntervalS 0..3600",
        FlowError::Deadband => "deadbandLpm 0..5",
        FlowError::Offset => "offset -10000..10000",
        FlowError::Gain => "gain 0.1..10",
        FlowError::Points => "points [[counts,lpm],...]",
        FlowError::PointCount => "points 2..16 (enabled needs them)",
        FlowError::PointCounts => "points counts 0..10000, increasing",
        FlowError::PointFlow => "points lpm 0..65.535, non-decreasing",
        FlowError::RefSide => "refSide closed|open",
        FlowError::PreClose => "preClose true|false",
        FlowError::StrokeCounts => "strokeCounts 0..10000 (refSide open needs 1..10000)",
    }
}

/// The rules of [`FlowConfig`]: qmax 0.01..20 l/min, both currents 5..60 mA, the fallback
/// 0..100 %, the interval 0..3600 s, the dead band 0..5 l/min, the offset -10000..10000, the
/// gain 0.1..10, the stroke 0..10000 (at least 1 with the open reference), no points or 2..16
/// (an enabled layer needs them) with counts strictly increasing (the first may be 0, every
/// one at most 10000) and flows non-decreasing.
pub fn validate_flow_config(c: &FlowConfig) -> Result<(), FlowError> {
    if !(FLOW_QMAX_MIN_ML..=FLOW_QMAX_MAX_ML).contains(&c.qmax_ml) {
        return Err(FlowError::Qmax);
    }
    if !(FLOW_MA_MIN..=FLOW_MA_MAX).contains(&c.ref_ma) {
        return Err(FlowError::RefMa);
    }
    if !(FLOW_MA_MIN..=FLOW_MA_MAX).contains(&c.open_ma) {
        return Err(FlowError::OpenMa);
    }
    if c.fallback_pct > 100 {
        return Err(FlowError::FallbackPct);
    }
    if c.min_interval_s > FLOW_INTERVAL_MAX_S {
        return Err(FlowError::MinInterval);
    }
    if c.deadband_ml > FLOW_DEADBAND_MAX_ML {
        return Err(FlowError::Deadband);
    }
    if !(-FLOW_OFFSET_MAX..=FLOW_OFFSET_MAX).contains(&c.offset) {
        return Err(FlowError::Offset);
    }
    if !(FLOW_GAIN_MIN_MILLI..=FLOW_GAIN_MAX_MILLI).contains(&c.gain_milli) {
        return Err(FlowError::Gain);
    }
    if c.stroke_counts > FLOW_COUNTS_MAX || (c.ref_side == RefSide::Open && c.stroke_counts == 0) {
        return Err(FlowError::StrokeCounts);
    }
    let n = usize::from(c.count);
    if n > FLOW_POINTS_MAX || (n != 0 && n < FLOW_POINTS_MIN) || (c.enabled && n == 0) {
        return Err(FlowError::PointCount);
    }
    let mut prev: Option<FlowPoint> = None;
    for &p in c.points() {
        if p.counts > FLOW_COUNTS_MAX {
            return Err(FlowError::PointCounts);
        }
        if let Some(q) = prev {
            if p.counts <= q.counts {
                return Err(FlowError::PointCounts);
            }
            if p.flow_ml < q.flow_ml {
                return Err(FlowError::PointFlow);
            }
        }
        prev = Some(p);
    }
    Ok(())
}

// ---------------------------------------------------------------- mapping

/// The flow of a target in ml/min: pct / 100 x qmax, rounded.
pub fn pct_flow(c: &FlowConfig, pct: u8) -> u32 {
    (u32::from(pct) * u32::from(c.qmax_ml) + 50) / 100
}

/// The curve counts of a flow: the first position where the piecewise-linear curve reaches
/// `flow_ml`; the first point's counts at or below its flow, the last point's counts above the
/// last flow (rounded to the nearest pulse). 0 without points.
pub fn curve_counts(c: &FlowConfig, flow_ml: u32) -> u16 {
    let Some((&first, rest)) = c.points().split_first() else {
        return 0;
    };
    if flow_ml <= u32::from(first.flow_ml) {
        return first.counts;
    }
    let mut prev = first;
    for &p in rest {
        if flow_ml <= u32::from(p.flow_ml) {
            // prev.flow < flow <= p.flow: the flow rises on this segment
            let dq = u64::from(p.flow_ml.saturating_sub(prev.flow_ml)).max(1);
            let dc = u64::from(p.counts.saturating_sub(prev.counts));
            let q = u64::from(flow_ml).saturating_sub(u64::from(prev.flow_ml));
            let add = (q * dc * 2 + dq) / (2 * dq);
            return prev.counts.saturating_add(add.min(dc) as u16);
        }
        prev = p;
    }
    prev.counts
}

/// The flow of the curve at `counts` in ml/min: linear between the points, the first point's
/// flow below it, the last point's flow above it. 0 without points.
pub fn curve_flow(c: &FlowConfig, counts: u16) -> u32 {
    let Some((&first, rest)) = c.points().split_first() else {
        return 0;
    };
    if counts <= first.counts {
        return u32::from(first.flow_ml);
    }
    let mut prev = first;
    for &p in rest {
        if counts <= p.counts {
            // prev.counts < counts <= p.counts
            let dc = i64::from(p.counts.saturating_sub(prev.counts)).max(1);
            let dq = i64::from(p.flow_ml) - i64::from(prev.flow_ml);
            let x = i64::from(counts.saturating_sub(prev.counts));
            // rounded to the nearest, the sign of dq kept
            let add = (x * dq * 2 + dc * dq.signum()) / (2 * dc);
            return (i64::from(prev.flow_ml) + add).clamp(0, i64::from(u16::MAX)) as u32;
        }
        prev = p;
    }
    u32::from(prev.flow_ml)
}

/// round(gain x raw) + offset, 1..10000.
fn corrected_counts(c: &FlowConfig, raw: u16) -> u16 {
    let scaled = (i64::from(raw) * i64::from(c.gain_milli) + 500) / 1000;
    (scaled + i64::from(c.offset)).clamp(1, i64::from(FLOW_COUNTS_MAX)) as u16
}

/// The curve counts behind corrected pulses: (counts - offset) / gain, rounded, at least 0.
fn raw_counts(c: &FlowConfig, corrected: u16) -> u16 {
    let gain = i64::from(c.gain_milli.max(1));
    let v = (i64::from(corrected) - i64::from(c.offset)) * 1000;
    if v <= 0 {
        return 0;
    }
    ((v + gain / 2) / gain).min(i64::from(u16::MAX)) as u16
}

/// The pulses the layer moves away from the reference end stop for a target: round(offset +
/// gain x curve counts), 1..10000; 0 for a target of 0 (closed: a close to the end stop alone,
/// whatever the reference side).
pub fn target_counts(c: &FlowConfig, pct: u8) -> u16 {
    if pct == 0 {
        return 0;
    }
    corrected_counts(c, curve_counts(c, pct_flow(c, pct)))
}

/// The flow the layer estimates at `counts` corrected pulses (ml/min); 0 at 0 (closed).
pub fn counts_flow(c: &FlowConfig, counts: u16) -> u32 {
    if counts == 0 {
        return 0;
    }
    curve_flow(c, raw_counts(c, counts))
}

/// The % of a flow: flow / qmax x 100, rounded, at most 100.
pub fn flow_pct(c: &FlowConfig, flow_ml: u32) -> u8 {
    let qmax = u64::from(c.qmax_ml.max(1));
    ((u64::from(flow_ml) * 100 + qmax / 2) / qmax).min(100) as u8
}

// ---------------------------------------------------------------- NVS blob

/// One valve in the blob.
pub const FLOW_RECORD_SIZE: usize = 19 + 4 * FLOW_POINTS_MAX;
/// NVS `vdmrev/flow`: "VDMF" (4), version 1 (1), count 12 (1), 12 records, CRC-32 (4).
pub const FLOW_BLOB_SIZE: usize = 6 + N * FLOW_RECORD_SIZE + 4;

const FLOW_MAGIC: [u8; 4] = *b"VDMF";
const FLOW_VERSION: u8 = 1;
const RECORDS_AT: usize = 6;
const FLOW_CRC_AT: usize = RECORDS_AT + N * FLOW_RECORD_SIZE;
const _: () = assert!(FLOW_CRC_AT + 4 == FLOW_BLOB_SIZE);
const POINTS_AT: usize = 19;

/// Record: enabled (1), qmax (2), refmA (1), openmA (1), fallbackPct (1), minIntervalS (2),
/// deadband (2), offset (2), gain (2), refSide (1), preClose (1), strokeCounts (2), count (1),
/// 16 x {counts (2), flow (2)}; little endian.
fn encode_record(c: &FlowConfig, out: &mut [u8; FLOW_RECORD_SIZE]) {
    out[0] = u8::from(c.enabled);
    out[1..3].copy_from_slice(&c.qmax_ml.to_le_bytes());
    out[3] = c.ref_ma;
    out[4] = c.open_ma;
    out[5] = c.fallback_pct;
    out[6..8].copy_from_slice(&c.min_interval_s.to_le_bytes());
    out[8..10].copy_from_slice(&c.deadband_ml.to_le_bytes());
    out[10..12].copy_from_slice(&c.offset.to_le_bytes());
    out[12..14].copy_from_slice(&c.gain_milli.to_le_bytes());
    out[14] = c.ref_side as u8;
    out[15] = u8::from(c.pre_close);
    out[16..18].copy_from_slice(&c.stroke_counts.to_le_bytes());
    out[18] = c.count;
    let (cells, _) = out[POINTS_AT..].as_chunks_mut::<4>();
    for (cell, p) in cells.iter_mut().zip(&c.points) {
        cell[..2].copy_from_slice(&p.counts.to_le_bytes());
        cell[2..].copy_from_slice(&p.flow_ml.to_le_bytes());
    }
}

fn flag(b: u8) -> Option<bool> {
    match b {
        0 => Some(false),
        1 => Some(true),
        _ => None,
    }
}

fn decode_record(b: &[u8; FLOW_RECORD_SIZE]) -> Option<FlowConfig> {
    let u16_at = |i: usize| u16::from_le_bytes([b[i], b[i + 1]]);
    let mut c = FlowConfig {
        enabled: flag(b[0])?,
        qmax_ml: u16_at(1),
        ref_ma: b[3],
        open_ma: b[4],
        fallback_pct: b[5],
        min_interval_s: u16_at(6),
        deadband_ml: u16_at(8),
        offset: i16::from_le_bytes([b[10], b[11]]),
        gain_milli: u16_at(12),
        ref_side: RefSide::from_raw(b[14])?,
        pre_close: flag(b[15])?,
        stroke_counts: u16_at(16),
        count: b[18],
        points: [FlowPoint::default(); FLOW_POINTS_MAX],
    };
    let (cells, _) = b[POINTS_AT..].as_chunks::<4>();
    for (p, cell) in c.points.iter_mut().zip(cells) {
        p.counts = u16::from_le_bytes([cell[0], cell[1]]);
        p.flow_ml = u16::from_le_bytes([cell[2], cell[3]]);
    }
    validate_flow_config(&c).ok().map(|()| c)
}

/// Writes the blob; returns [`FLOW_BLOB_SIZE`].
pub fn encode_flow_configs(c: &[FlowConfig; N], out: &mut [u8; FLOW_BLOB_SIZE]) -> usize {
    out[..4].copy_from_slice(&FLOW_MAGIC);
    out[4] = FLOW_VERSION;
    out[5] = VALVE_COUNT;
    let (records, _) = out[RECORDS_AT..FLOW_CRC_AT].as_chunks_mut::<FLOW_RECORD_SIZE>();
    for (r, cfg) in records.iter_mut().zip(c) {
        encode_record(cfg, r);
    }
    let sum = crc32(&out[..FLOW_CRC_AT], 0);
    out[FLOW_CRC_AT..].copy_from_slice(&sum.to_le_bytes());
    FLOW_BLOB_SIZE
}

/// false (out = defaults) for another length, a bad magic/version/count/CRC or a record that
/// fails [`validate_flow_config`].
pub fn decode_flow_configs(data: &[u8], out: &mut [FlowConfig; N]) -> bool {
    *out = [FlowConfig::DEFAULT; N];
    let Ok(b) = <&[u8; FLOW_BLOB_SIZE]>::try_from(data) else {
        return false;
    };
    let (body, crc) = b.split_at(FLOW_CRC_AT);
    if body[..4] != FLOW_MAGIC || body[4] != FLOW_VERSION || body[5] != VALVE_COUNT {
        return false;
    }
    if crc != crc32(body, 0).to_le_bytes().as_slice() {
        return false;
    }
    let mut decoded = [FlowConfig::DEFAULT; N];
    let (records, _) = body[RECORDS_AT..].as_chunks::<FLOW_RECORD_SIZE>();
    for (slot, r) in decoded.iter_mut().zip(records) {
        let Some(c) = decode_record(r) else {
            return false;
        };
        *slot = c;
    }
    *out = decoded;
    true
}

// ---------------------------------------------------------------- JSON

/// `{"enabled":..,"qmax":2.700,"refmA":34,"openmA":30,"fallbackPct":15,"minIntervalS":300,
/// "deadbandLpm":0.050,"offset":0,"gain":1.000,"refSide":"closed","strokeCounts":0,
/// "preClose":false,"points":[[229,0.000],...]}` (the flows with three decimals).
pub fn write_flow_config_json(jw: &mut JsonWriter<'_>, c: &FlowConfig) {
    jw.begin_object();
    jw.kv("enabled", c.enabled);
    jw.key("qmax");
    jw.fixed(i32::from(c.qmax_ml), 3);
    jw.kv("refmA", c.ref_ma);
    jw.kv("openmA", c.open_ma);
    jw.kv("fallbackPct", c.fallback_pct);
    jw.kv("minIntervalS", c.min_interval_s);
    jw.key("deadbandLpm");
    jw.fixed(i32::from(c.deadband_ml), 3);
    jw.kv("offset", c.offset);
    jw.key("gain");
    jw.fixed(i32::from(c.gain_milli), 3);
    jw.kv("refSide", ref_side_name(c.ref_side));
    jw.kv("strokeCounts", c.stroke_counts);
    jw.kv("preClose", c.pre_close);
    jw.key("points");
    jw.begin_array();
    for p in c.points() {
        jw.begin_array();
        jw.value(p.counts);
        jw.fixed(i32::from(p.flow_ml), 3);
        jw.end_array();
    }
    jw.end_array();
    jw.end_object();
}

/// The text of a JSON document being read.
struct Cursor<'a> {
    s: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn peek(&self) -> Option<u8> {
        self.s.get(self.pos).copied()
    }

    fn ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.pos += 1;
        }
    }

    /// The next byte is `c`: consumed.
    fn take(&mut self, c: u8) -> bool {
        if self.peek() == Some(c) {
            self.pos += 1;
            return true;
        }
        false
    }

    /// The next bytes are `word`: consumed.
    fn word(&mut self, word: &[u8]) -> bool {
        let rest = self.s.get(self.pos..).unwrap_or_default();
        if rest.starts_with(word) {
            self.pos += word.len();
            return true;
        }
        false
    }

    /// A JSON string (the quotes consumed, escapes kept as written): its bytes.
    fn string(&mut self) -> Option<&'a [u8]> {
        if !self.take(b'"') {
            return None;
        }
        let start = self.pos;
        loop {
            match self.peek()? {
                b'"' => {
                    let s = self.s.get(start..self.pos)?;
                    self.pos += 1;
                    return Some(s);
                }
                b'\\' => self.pos += 2,
                c if c < 0x20 => return None,
                _ => self.pos += 1,
            }
        }
    }

    /// A JSON number without an exponent: its value x 1000 (at most three decimals; more, an
    /// exponent or a value beyond 2000000 is a malformed number for the layer).
    fn milli(&mut self) -> Option<i32> {
        let neg = self.take(b'-');
        let start = self.pos;
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.pos += 1;
        }
        let int = self.s.get(start..self.pos)?;
        if int.is_empty() || (int.len() > 1 && int[0] == b'0') {
            return None;
        }
        let mut v = i64::from(parse_uint(int, 2_000_000)?) * 1000;
        if self.take(b'.') {
            let start = self.pos;
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
            let frac = self.s.get(start..self.pos)?;
            if frac.is_empty() || frac.len() > 3 {
                return None;
            }
            let mut f = i64::from(parse_uint(frac, 999)?);
            for _ in frac.len()..3 {
                f *= 10;
            }
            v += f;
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            return None;
        }
        i32::try_from(if neg { -v } else { v }).ok()
    }

    /// A JSON bool, else `err`.
    fn boolean(&mut self, err: FlowError) -> Result<bool, FlowError> {
        if self.word(b"true") {
            Ok(true)
        } else if self.word(b"false") {
            Ok(false)
        } else {
            Err(err)
        }
    }
}

/// An integer key: a whole number in `min..=max`, else `err`.
fn integer(v: i32, min: i64, max: i64, err: FlowError) -> Result<i64, FlowError> {
    if v % 1000 != 0 {
        return Err(err);
    }
    let n = i64::from(v / 1000);
    if !(min..=max).contains(&n) {
        return Err(err);
    }
    Ok(n)
}

/// `points`: `[[counts, lpm], ...]` into `c.points`/`c.count` (the shape only; the rules are
/// [`validate_flow_config`]'s).
fn read_points(cur: &mut Cursor<'_>, c: &mut FlowConfig) -> Result<(), FlowError> {
    if !cur.take(b'[') {
        return Err(FlowError::Points);
    }
    let mut n = 0usize;
    cur.ws();
    if cur.take(b']') {
        c.count = 0;
        return Ok(());
    }
    loop {
        cur.ws();
        if !cur.take(b'[') {
            return Err(FlowError::Points);
        }
        cur.ws();
        let counts = cur.milli().ok_or(FlowError::Points)?;
        let counts = integer(counts, 0, i64::from(u16::MAX), FlowError::PointCounts)?;
        cur.ws();
        if !cur.take(b',') {
            return Err(FlowError::Points);
        }
        cur.ws();
        let flow = cur.milli().ok_or(FlowError::Points)?;
        if !(0..=i32::from(u16::MAX)).contains(&flow) {
            return Err(FlowError::PointFlow);
        }
        cur.ws();
        if !cur.take(b']') {
            return Err(FlowError::Points);
        }
        let Some(p) = c.points.get_mut(n) else {
            return Err(FlowError::PointCount);
        };
        *p = FlowPoint {
            counts: counts as u16,
            flow_ml: flow as u16,
        };
        n += 1;
        cur.ws();
        if cur.take(b']') {
            break;
        }
        if !cur.take(b',') {
            return Err(FlowError::Points);
        }
    }
    c.count = n as u8;
    Ok(())
}

/// Applies one member to `c`.
fn apply_member(cur: &mut Cursor<'_>, key: &[u8], c: &mut FlowConfig) -> Result<(), FlowError> {
    cur.ws();
    match key {
        b"enabled" => c.enabled = cur.boolean(FlowError::Enabled)?,
        b"preClose" => c.pre_close = cur.boolean(FlowError::PreClose)?,
        b"refSide" => {
            let s = cur.string().ok_or(FlowError::RefSide)?;
            c.ref_side = match s {
                b"closed" => RefSide::Closed,
                b"open" => RefSide::Open,
                _ => return Err(FlowError::RefSide),
            };
        }
        b"points" => read_points(cur, c)?,
        _ => {
            let err = match key {
                b"qmax" => FlowError::Qmax,
                b"refmA" => FlowError::RefMa,
                b"openmA" => FlowError::OpenMa,
                b"fallbackPct" => FlowError::FallbackPct,
                b"minIntervalS" => FlowError::MinInterval,
                b"strokeCounts" => FlowError::StrokeCounts,
                b"deadbandLpm" => FlowError::Deadband,
                b"offset" => FlowError::Offset,
                b"gain" => FlowError::Gain,
                _ => return Err(FlowError::UnknownKey),
            };
            let v = cur.milli().ok_or(err)?;
            match key {
                b"qmax" => c.qmax_ml = u16::try_from(v).map_err(|_| err)?,
                b"refmA" => c.ref_ma = integer(v, 0, 255, err)? as u8,
                b"openmA" => c.open_ma = integer(v, 0, 255, err)? as u8,
                b"fallbackPct" => c.fallback_pct = integer(v, 0, 255, err)? as u8,
                b"minIntervalS" => c.min_interval_s = integer(v, 0, 65535, err)? as u16,
                b"strokeCounts" => c.stroke_counts = integer(v, 0, 65535, err)? as u16,
                b"deadbandLpm" => c.deadband_ml = u16::try_from(v).map_err(|_| err)?,
                b"offset" => c.offset = integer(v, -32768, 32767, err)? as i16,
                _ => c.gain_milli = u16::try_from(v).map_err(|_| err)?,
            }
        }
    }
    Ok(())
}

/// `POST /api/valves/{n}/flow`: applies the members of a flat JSON object to `c` (the members
/// not present keep their values; `points` replaces every point), then checks the result with
/// [`validate_flow_config`]. Numbers are plain decimals with at most three decimals (`qmax`,
/// `deadbandLpm`, `gain` and the flows), whole numbers elsewhere; `enabled` and `preClose` are
/// JSON bools, `refSide` is "closed" or "open". On an error `c` is left as it was.
pub fn patch_flow_config_json(c: &mut FlowConfig, json: &[u8]) -> Result<(), FlowError> {
    let mut patched = *c;
    let mut cur = Cursor { s: json, pos: 0 };
    cur.ws();
    if !cur.take(b'{') {
        return Err(FlowError::Malformed);
    }
    cur.ws();
    if !cur.take(b'}') {
        loop {
            cur.ws();
            let key = cur.string().ok_or(FlowError::Malformed)?;
            cur.ws();
            if !cur.take(b':') {
                return Err(FlowError::Malformed);
            }
            apply_member(&mut cur, key, &mut patched)?;
            cur.ws();
            if cur.take(b'}') {
                break;
            }
            if !cur.take(b',') {
                return Err(FlowError::Malformed);
            }
        }
    }
    cur.ws();
    if cur.pos != json.len() {
        return Err(FlowError::Malformed);
    }
    validate_flow_config(&patched)?;
    *c = patched;
    Ok(())
}

// ---------------------------------------------------------------- state

/// The state of the layer on a valve (API `flow.state`).
#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FlowState {
    /// not enabled
    #[default]
    Off = 0,
    /// enabled, but the STM firmware is below 2.2.8
    Unsupported = 1,
    /// enabled, the position is not held: a positioning follows when the valve is free
    Unknown = 2,
    /// the moves of a positioning run
    Positioning = 3,
    /// the STM keeps the position
    Held = 4,
    /// the last positioning failed; tried again after 5 min or with the next target
    Failed = 5,
}

/// "off", "unsupported", "unknown", "positioning", "held", "failed".
pub fn flow_state_name(s: FlowState) -> &'static str {
    match s {
        FlowState::Off => "off",
        FlowState::Unsupported => "unsupported",
        FlowState::Unknown => "unknown",
        FlowState::Positioning => "positioning",
        FlowState::Held => "held",
        FlowState::Failed => "failed",
    }
}

/// What the snapshot shows of a valve's flow layer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FlowView {
    pub enabled: bool,
    pub state: FlowState,
    /// the held position is known: `counts`, `flow_ml` and `pct` are its values
    pub known: bool,
    /// pulses from the reference end stop (0: closed)
    pub counts: u16,
    /// pulses of the current or last positioning
    pub target_counts: u16,
    /// ml/min the curve gives at `counts`
    pub flow_ml: u32,
    /// `flow_ml` / qmax in %: the position the valve reports
    pub pct: u8,
    /// a positioning completed since the start: `last_ms` and `last_peak` are the final move's
    pub last_valid: bool,
    /// duration of the final move of the last positioning (ms)
    pub last_ms: u32,
    /// its largest filtered current (0.1 mA)
    pub last_peak: u16,
}

/// The step of a positioning (FlowPositionFailed arg1).
#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Step {
    #[default]
    None = 0,
    /// `stgtp fallbackPct`
    Fallback = 1,
    /// the move to the reference end stop
    Reference = 2,
    /// the warm-up opening of 300 pulses (preClose)
    WarmUp = 3,
    /// the warm-up's close back to the end stop (preClose)
    WarmClose = 4,
    /// the final move of the curve's pulses
    Final = 5,
    /// `skeep 1`
    Keep = 6,
}

/// A request of the layer; the session builds the line and reports the result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlowRequest {
    /// `stgtp valve pct`: the fallback target
    SetTarget { valve: u8, pct: u8 },
    /// `svmov valve dir counts maxmA`
    Move {
        valve: u8,
        dir: MoveDir,
        counts: u16,
        max_ma: u8,
    },
    /// `skeep valve on`
    Keep { valve: u8, on: bool },
}

impl FlowRequest {
    pub fn valve(&self) -> u8 {
        match *self {
            FlowRequest::SetTarget { valve, .. }
            | FlowRequest::Move { valve, .. }
            | FlowRequest::Keep { valve, .. } => valve,
        }
    }
}

/// The result of a request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlowOutcome {
    Ok,
    /// the STM's error code
    Rejected(u16),
    NoReply,
}

/// What the layer needs of the link each pass.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FlowInputs {
    /// the STM answers (link Up or Degraded)
    pub link_up: bool,
    /// the lease is expired: the STM drives the failsafe positions
    pub lease_expired: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct FlowValve {
    /// Unknown, Positioning, Held or Failed (Off and Unsupported are derived)
    state: FlowState,
    step: Step,
    /// the request of this valve that was handed out: its result is awaited
    req: Option<FlowRequest>,
    /// the STM accepted a move: its record is awaited
    move_pending: bool,
    move_dir: MoveDir,
    move_req: u16,
    /// move_seq of the valve when the move was handed out
    move_seq_at: u32,
    move_since_ms: u32,
    /// moves to the reference end stop that ended before it
    ref_attempts: u8,
    busy_retries: u16,
    /// the next request of the sequence waits until `wait_until_ms`
    waiting: bool,
    wait_until_ms: u32,
    /// ... or until a calibration of the valve ended
    wait_calib: bool,
    calib_seen: bool,
    /// the goal of the positioning
    target_counts: u16,
    target_pct: u8,
    /// the desired target the positioning serves (served_valid)
    served_desired: u8,
    served_valid: bool,
    /// the position the STM keeps (held_valid), the move_seq it was taken at and when
    held_counts: u16,
    held_valid: bool,
    held_seq: u32,
    held_at_ms: u32,
    /// the final move of the last positioning (last_valid)
    last_valid: bool,
    last_ms: u32,
    last_peak: u16,
    last_start_ms: u32,
    started_once: bool,
    /// `stgtp fallbackPct` goes before the next positioning (unless the STM target equals it)
    needs_fallback: bool,
    /// `skeep 0` goes out (the layer was disabled)
    release_pending: bool,
    /// a counts request arrived while a positioning ran
    pending_raw: Option<u16>,
    /// the curve changed while a positioning ran: another one follows
    restart_pending: bool,
    failed_ms: u32,
}

impl FlowValve {
    /// A valve the layer knows nothing about: unknown, the fallback target due.
    const FRESH: Self = Self {
        state: FlowState::Unknown,
        step: Step::None,
        req: None,
        move_pending: false,
        move_dir: MoveDir::Open,
        move_req: 0,
        move_seq_at: 0,
        move_since_ms: 0,
        ref_attempts: 0,
        busy_retries: 0,
        waiting: false,
        wait_until_ms: 0,
        wait_calib: false,
        calib_seen: false,
        target_counts: 0,
        target_pct: 0,
        served_desired: 0,
        served_valid: false,
        held_counts: 0,
        held_valid: false,
        held_seq: 0,
        held_at_ms: 0,
        last_valid: false,
        last_ms: 0,
        last_peak: 0,
        last_start_ms: 0,
        started_once: false,
        needs_fallback: true,
        release_pending: false,
        pending_raw: None,
        restart_pending: false,
        failed_ms: 0,
    };
}

/// Pulses the move to the closed reference adds to the believed position.
pub const CLOSE_EXTRA_COUNTS: u16 = 500;
/// Pulses of a further move to the reference when the first did not reach the end stop ...
pub const REF_RETRY_COUNTS: u16 = 800;
/// ... and how many such moves a positioning tries at most.
pub const REF_ATTEMPTS: u8 = 3;
/// Pulses of the warm-up opening (preClose).
pub const WARM_UP_COUNTS: u16 = 300;
/// A move the STM accepted must have ended within this time.
pub const MOVE_TIMEOUT_MS: u32 = 120_000;
/// `svmov err 2` (busy): the move is sent again after ...
pub const BUSY_RETRY_MS: u32 = 3_000;
/// ... at most this often.
pub const BUSY_RETRIES_MAX: u16 = 100;
/// `svmov err 3` (calibration pending): the move is sent again when the calibration ended,
/// or after this time.
pub const CALIB_RETRY_MS: u32 = 60_000;
/// A failed positioning is tried again after this time (or with the next target).
pub const FAILED_RETRY_MS: u32 = 300_000;

const STATUS_OPENING: u8 = 2;
const STATUS_CLOSING: u8 = 3;

/// The valve is free for a service move: its v3 data is known, it neither moves nor calibrates,
/// it is not at its lease failsafe and the STM answers.
fn can_move(s: &ValveState, inp: FlowInputs) -> bool {
    s.known
        && s.has_v3
        && !s.calibrating
        && s.status != STATUS_OPENING
        && s.status != STATUS_CLOSING
        && (s.stm_flags & STM_FLAG_FS_LEASE) == 0
        && !inp.lease_expired
        && inp.link_up
}

/// The layer over the 12 valves.
#[derive(Clone, Debug)]
pub struct FlowLayer {
    cfg: [FlowConfig; N],
    v: [FlowValve; N],
    supported: bool,
    cursor: usize,
    dirty: bool,
}

impl Default for FlowLayer {
    fn default() -> Self {
        Self::new()
    }
}

static DEFAULT_CONFIG: FlowConfig = FlowConfig::DEFAULT;

impl FlowLayer {
    /// Every valve disabled, the STM not yet known to support the layer.
    pub fn new() -> Self {
        Self {
            cfg: [FlowConfig::DEFAULT; N],
            v: [FlowValve::FRESH; N],
            supported: false,
            cursor: 0,
            dirty: false,
        }
    }

    /// The configuration of a valve (the default for one out of range).
    pub fn config(&self, valve: u8) -> &FlowConfig {
        self.cfg.get(usize::from(valve)).unwrap_or(&DEFAULT_CONFIG)
    }

    /// The configuration of every valve (storage, config changes). A valve that gets enabled
    /// starts unknown and sends the fallback target before its first positioning; one that gets
    /// disabled releases its keep; an enabled valve whose curve, correction, reference side or
    /// qmax changed is positioned again.
    pub fn set_configs(&mut self, cfgs: &[FlowConfig; N]) {
        for ((cur, new), fv) in self.cfg.iter_mut().zip(cfgs).zip(self.v.iter_mut()) {
            if cur.enabled != new.enabled {
                if new.enabled {
                    *fv = FlowValve::FRESH;
                } else {
                    let release = fv.state != FlowState::Unknown || fv.req.is_some();
                    *fv = FlowValve {
                        release_pending: release && self.supported,
                        req: fv.req,
                        needs_fallback: false,
                        ..FlowValve::FRESH
                    };
                }
                self.dirty = true;
            } else if new.enabled && cur.curve_key() != new.curve_key() {
                if fv.state == FlowState::Positioning {
                    fv.restart_pending = true;
                } else {
                    fv.state = FlowState::Unknown;
                }
                self.dirty = true;
            }
            *cur = *new;
        }
    }

    /// The STM firmware supports the layer (2.2.8 and up). While it does not, every enabled
    /// valve shows `unsupported` and nothing is sent; a change forgets the requests in flight.
    pub fn set_supported(&mut self, on: bool) {
        if self.supported == on {
            return;
        }
        self.supported = on;
        self.forget_stm();
    }

    pub fn supported(&self) -> bool {
        self.supported
    }

    /// Bit v: the layer delivers the target of valve v (enabled and supported).
    pub fn active_mask(&self) -> u16 {
        // Disjoint bits: `+` is the C++ `|`.
        (0u16..)
            .zip(&self.cfg)
            .filter(|(_, c)| c.enabled && self.supported)
            .fold(0, |m, (i, _)| m + (1 << i))
    }

    fn active(&self, i: usize) -> bool {
        self.supported && self.cfg.get(i).is_some_and(|c| c.enabled)
    }

    /// Every valve unknown with the fallback target due; the interval and the last move kept.
    fn forget_stm(&mut self) {
        for fv in &mut self.v {
            *fv = FlowValve {
                started_once: fv.started_once,
                last_start_ms: fv.last_start_ms,
                last_valid: fv.last_valid,
                last_ms: fv.last_ms,
                last_peak: fv.last_peak,
                ..FlowValve::FRESH
            };
        }
        self.dirty = true;
    }

    /// A new STM session (reboot, reset, flash, re-sync): the keeps are gone with the STM's
    /// RAM, every enabled valve is unknown (a held one with the event FlowHoldLost) and sends
    /// the fallback target first.
    pub fn on_stm_rebooted(&mut self, log: &mut dyn FnMut(&Event)) {
        for (i, fv) in self.v.iter().enumerate() {
            if self.active(i) && fv.state == FlowState::Held {
                log(&event(EventCode::FlowHoldLost, i as u8, 0, 0, b""));
            }
        }
        self.forget_stm();
    }

    /// The valve moves for the layer: the planner polls it fast.
    pub fn busy(&self, valve: u8) -> bool {
        let i = usize::from(valve);
        self.active(i)
            && self
                .v
                .get(i)
                .is_some_and(|fv| fv.state == FlowState::Positioning)
    }

    /// A positioning was handed out or completed: the snapshot is due.
    pub fn take_dirty(&mut self) -> bool {
        core::mem::take(&mut self.dirty)
    }

    /// `POST /api/valves/{n}/flow/counts`: positions the valve to `counts` corrected pulses at
    /// once (0: closed), outside the dead band and the interval; `desired` is the valve's
    /// desired target, which the layer then counts as served until it changes. While a
    /// positioning runs, the request follows it. False when the layer does not run on the
    /// valve.
    pub fn position_counts(
        &mut self,
        valve: u8,
        counts: u16,
        desired: Option<u8>,
        now_ms: u32,
    ) -> bool {
        let i = usize::from(valve);
        if !self.active(i) {
            return false;
        }
        let (Some(fv), Some(cfg)) = (self.v.get_mut(i), self.cfg.get(i)) else {
            return false;
        };
        let counts = counts.min(FLOW_COUNTS_MAX);
        fv.served_desired = desired.unwrap_or(0);
        fv.served_valid = desired.is_some();
        if fv.state == FlowState::Positioning {
            fv.pending_raw = Some(counts);
        } else {
            fv.pending_raw = None;
            fv.target_counts = counts;
            fv.target_pct = flow_pct(cfg, counts_flow(cfg, counts));
            begin_sequence(fv, cfg, None, now_ms);
        }
        self.dirty = true;
        true
    }

    /// The next request of the layer, at most one in flight over all valves (the STM moves one
    /// valve at a time): the release of a disabled valve, the continuation of a running
    /// positioning, else the start of one for a valve whose desired target is not the one its
    /// held position serves. A new target is applied only when the flow changes by the dead
    /// band and the interval since the last positioning has passed; a target of 0 and the first
    /// positioning since the start are not delayed. A valve in assembly, calibrating, moving,
    /// at its lease failsafe or without v3 data waits.
    pub fn next(
        &mut self,
        model: &ValveModel,
        inp: FlowInputs,
        now_ms: u32,
    ) -> Option<FlowRequest> {
        if self.v.iter().any(|fv| fv.req.is_some() || fv.move_pending) {
            return None;
        }
        let cursor = self.cursor;
        for i in (cursor..N).chain(0..cursor) {
            let valve = i as u8;
            if self.v[i].release_pending && inp.link_up {
                let req = FlowRequest::Keep { valve, on: false };
                self.v[i].release_pending = false;
                self.v[i].req = Some(req);
                self.dirty = true;
                return Some(req);
            }
            if !self.active(i) {
                continue;
            }
            let s = model.valve(valve);
            let (fv, cfg) = (&mut self.v[i], &self.cfg[i]);
            let req = if fv.state == FlowState::Positioning {
                continue_sequence(fv, cfg, s, valve, inp, now_ms)
            } else {
                start_if_due(fv, cfg, s, valve, inp, now_ms)
            };
            if let Some(req) = req {
                fv.req = Some(req);
                self.cursor = (i + 1) % N;
                self.dirty = true;
                return Some(req);
            }
        }
        None
    }

    /// The request of `valve` could not be queued: a release goes out again, a positioning
    /// tries again after [`BUSY_RETRY_MS`].
    pub fn on_dropped(&mut self, valve: u8, now_ms: u32) {
        let Some(fv) = self.v.get_mut(usize::from(valve)) else {
            return;
        };
        let Some(req) = fv.req.take() else {
            return;
        };
        if matches!(req, FlowRequest::Keep { on: false, .. }) {
            fv.release_pending = true;
        } else if fv.state == FlowState::Positioning {
            wait(fv, BUSY_RETRY_MS, false, now_ms);
        }
    }

    /// The result of the request of `valve` that [`next`](Self::next) handed out.
    pub fn on_result(
        &mut self,
        valve: u8,
        outcome: FlowOutcome,
        now_ms: u32,
        log: &mut dyn FnMut(&Event),
    ) {
        let i = usize::from(valve);
        let Some(fv) = self.v.get_mut(i) else {
            return;
        };
        let Some(req) = fv.req.take() else {
            return;
        };
        self.dirty = true;
        if matches!(req, FlowRequest::Keep { on: false, .. }) || !self.active(i) {
            return;
        }
        let fv = &mut self.v[i];
        if fv.state != FlowState::Positioning {
            return;
        }
        match (req, outcome) {
            (FlowRequest::SetTarget { .. }, FlowOutcome::Ok) => {
                fv.needs_fallback = false;
                fv.step = Step::Reference;
            }
            (FlowRequest::Move { dir, counts, .. }, FlowOutcome::Ok) => {
                fv.move_pending = true;
                fv.move_dir = dir;
                fv.move_req = counts;
                fv.move_since_ms = now_ms;
            }
            (FlowRequest::Move { .. }, FlowOutcome::Rejected(2)) => {
                fv.busy_retries = fv.busy_retries.saturating_add(1);
                if fv.busy_retries > BUSY_RETRIES_MAX {
                    fail(fv, valve, 2, b"busy", now_ms, log);
                } else {
                    wait(fv, BUSY_RETRY_MS, false, now_ms);
                }
            }
            (FlowRequest::Move { .. }, FlowOutcome::Rejected(3)) => {
                wait(fv, CALIB_RETRY_MS, true, now_ms);
            }
            (FlowRequest::Keep { .. }, FlowOutcome::Ok) => held(fv, valve, now_ms, log),
            (_, FlowOutcome::Rejected(code)) => {
                fail(fv, valve, i32::from(code), b"rejected", now_ms, log);
            }
            (_, FlowOutcome::NoReply) => fail(fv, valve, -1, b"no reply", now_ms, log),
        }
    }

    /// Once per second: the end of an accepted move (its record arrived: direction and
    /// requested pulses match, the valve no longer moves), its timeout, the end of a
    /// calibration a move waits for, and a held valve whose keep is gone (the STM flag, or
    /// another move of the valve) -> unknown, event FlowHoldLost.
    pub fn tick(&mut self, model: &ValveModel, now_ms: u32, log: &mut dyn FnMut(&Event)) {
        for i in 0..N {
            if !self.active(i) {
                continue;
            }
            let valve = i as u8;
            let s = model.valve(valve);
            let fv = &mut self.v[i];
            if s.calibrating {
                fv.calib_seen = true;
            } else if fv.calib_seen {
                fv.calib_seen = false;
                if fv.wait_calib {
                    fv.waiting = false;
                    fv.wait_calib = false;
                }
            }
            match fv.state {
                FlowState::Positioning if fv.move_pending => {
                    let moving = s.status == STATUS_OPENING || s.status == STATUS_CLOSING;
                    let record = s.move_seq != fv.move_seq_at
                        && s.last_move.dir == fv.move_dir
                        && s.last_move.requested_counts == u32::from(fv.move_req);
                    if record && !moving {
                        fv.move_pending = false;
                        move_done(fv, &self.cfg[i], s, valve, now_ms, log);
                        self.dirty = true;
                    } else if elapsed_ms(now_ms, fv.move_since_ms) >= MOVE_TIMEOUT_MS {
                        fail(fv, valve, -2, b"timeout", now_ms, log);
                        self.dirty = true;
                    }
                }
                FlowState::Held => {
                    let since = elapsed_ms(s.last_seen_ms, fv.held_at_ms);
                    let fresh = (1..0x8000_0000).contains(&since);
                    let lost = (fresh && (s.stm_flags & STM_FLAG_SVC_KEEP) == 0)
                        || s.move_seq != fv.held_seq;
                    if lost {
                        fv.state = FlowState::Unknown;
                        fv.held_valid = false;
                        log(&event(
                            EventCode::FlowHoldLost,
                            valve,
                            i32::from(s.stm_flags),
                            0,
                            b"",
                        ));
                        self.dirty = true;
                    }
                }
                _ => {}
            }
        }
    }

    /// What the snapshot shows of valve `valve`.
    pub fn view(&self, valve: u8) -> FlowView {
        let i = usize::from(valve);
        let (Some(cfg), Some(fv)) = (self.cfg.get(i), self.v.get(i)) else {
            return FlowView::default();
        };
        let state = if !cfg.enabled {
            FlowState::Off
        } else if !self.supported {
            FlowState::Unsupported
        } else {
            fv.state
        };
        let running = state != FlowState::Off && state != FlowState::Unsupported;
        let known = running && fv.held_valid;
        let flow_ml = if known {
            counts_flow(cfg, fv.held_counts)
        } else {
            0
        };
        FlowView {
            enabled: cfg.enabled,
            state,
            known,
            counts: if known { fv.held_counts } else { 0 },
            target_counts: fv.target_counts,
            flow_ml,
            pct: if known { flow_pct(cfg, flow_ml) } else { 0 },
            last_valid: running && fv.last_valid,
            last_ms: fv.last_ms,
            last_peak: fv.last_peak,
        }
    }
}

/// An event with the default severity of its code.
fn event(code: EventCode, valve: u8, a1: i32, a2: i32, text: &[u8]) -> Event {
    make_event(code, event_default_severity(code), valve, a1, a2, text)
}

/// Starts the moves of a positioning: the fallback target first when it is due and the STM's
/// target is not it already, else the move to the reference end stop.
fn begin_sequence(fv: &mut FlowValve, cfg: &FlowConfig, s: Option<&ValveState>, now_ms: u32) {
    fv.state = FlowState::Positioning;
    fv.ref_attempts = 0;
    fv.busy_retries = 0;
    fv.waiting = false;
    fv.wait_calib = false;
    fv.move_pending = false;
    fv.restart_pending = false;
    fv.last_start_ms = now_ms;
    fv.started_once = true;
    // The fallback target goes out when the STM's target is known to be another one (another
    // client, an assembly), and after an enable or a new STM session while it is not known.
    let send_fallback = match s {
        Some(s) if s.stm_target_known => s.stm_target != cfg.fallback_pct,
        _ => fv.needs_fallback,
    };
    if send_fallback {
        fv.step = Step::Fallback;
    } else {
        fv.needs_fallback = false;
        fv.step = Step::Reference;
    }
}

/// Starts a positioning to the desired target `pct`.
fn start(fv: &mut FlowValve, cfg: &FlowConfig, s: &ValveState, pct: u8, now_ms: u32) {
    fv.target_pct = pct;
    fv.target_counts = target_counts(cfg, pct);
    fv.served_desired = pct;
    fv.served_valid = true;
    fv.pending_raw = None;
    begin_sequence(fv, cfg, Some(s), now_ms);
}

/// A valve that is not positioning: starts one when its desired target is due.
fn start_if_due(
    fv: &mut FlowValve,
    cfg: &FlowConfig,
    s: &ValveState,
    valve: u8,
    inp: FlowInputs,
    now_ms: u32,
) -> Option<FlowRequest> {
    if !can_move(s, inp) || s.source == TargetSource::Assembly {
        return None;
    }
    if let Some(counts) = fv.pending_raw.take() {
        fv.target_counts = counts;
        fv.target_pct = flow_pct(cfg, counts_flow(cfg, counts));
        begin_sequence(fv, cfg, Some(s), now_ms);
        return continue_sequence(fv, cfg, s, valve, inp, now_ms);
    }
    if !s.desired_valid {
        return None;
    }
    let desired = s.desired;
    let same = fv.served_valid && desired == fv.served_desired;
    match fv.state {
        FlowState::Held => {
            if same {
                return None;
            }
            if desired != 0 {
                let q_new = pct_flow(cfg, desired);
                let q_held = counts_flow(cfg, fv.held_counts);
                if q_new.abs_diff(q_held) < u32::from(cfg.deadband_ml) {
                    return None;
                }
                let interval = u32::from(cfg.min_interval_s) * 1000;
                if fv.started_once && elapsed_ms(now_ms, fv.last_start_ms) < interval {
                    return None;
                }
            }
        }
        FlowState::Failed if same && elapsed_ms(now_ms, fv.failed_ms) < FAILED_RETRY_MS => {
            return None;
        }
        _ => {}
    }
    start(fv, cfg, s, desired, now_ms);
    continue_sequence(fv, cfg, s, valve, inp, now_ms)
}

/// Open reference: the pulses of the final closing move from the open end stop, the stroke
/// minus the target counts (0: the target is the open end stop).
fn final_close(fv: &FlowValve, cfg: &FlowConfig) -> u16 {
    cfg.stroke_counts.saturating_sub(fv.target_counts)
}

/// The direction and threshold of the move to the reference end stop: closed for a target of
/// 0 and the closed reference, open for the open one.
fn reference_move(fv: &FlowValve, cfg: &FlowConfig) -> (MoveDir, u8) {
    if fv.target_counts == 0 || cfg.ref_side == RefSide::Closed {
        (MoveDir::Close, cfg.ref_ma)
    } else {
        (MoveDir::Open, cfg.open_ma)
    }
}

/// The next request of a running positioning, None while it waits.
fn continue_sequence(
    fv: &mut FlowValve,
    cfg: &FlowConfig,
    s: &ValveState,
    valve: u8,
    inp: FlowInputs,
    now_ms: u32,
) -> Option<FlowRequest> {
    if !inp.link_up || fv.move_pending {
        return None;
    }
    if fv.waiting {
        if elapsed_ms(now_ms, fv.wait_until_ms) < 0x8000_0000 {
            fv.waiting = false;
            fv.wait_calib = false;
        } else {
            return None;
        }
    }
    if fv.step == Step::Fallback {
        return Some(FlowRequest::SetTarget {
            valve,
            pct: cfg.fallback_pct,
        });
    }
    if fv.step == Step::Keep {
        return Some(FlowRequest::Keep { valve, on: true });
    }
    if fv.step == Step::None || !can_move(s, inp) {
        return None;
    }
    let (dir, counts, max_ma) = match fv.step {
        Step::Reference => {
            let (dir, ma) = reference_move(fv, cfg);
            (dir, distinct(reference_counts(fv, s, dir), s), ma)
        }
        Step::WarmUp => (MoveDir::Open, distinct(WARM_UP_COUNTS, s), cfg.open_ma),
        Step::WarmClose => (
            MoveDir::Close,
            distinct(WARM_UP_COUNTS + CLOSE_EXTRA_COUNTS, s),
            cfg.ref_ma,
        ),
        _ => match cfg.ref_side {
            RefSide::Closed => (MoveDir::Open, fv.target_counts, cfg.open_ma),
            RefSide::Open => (MoveDir::Close, final_close(fv, cfg), cfg.ref_ma),
        },
    };
    fv.move_seq_at = s.move_seq;
    Some(FlowRequest::Move {
        valve,
        dir,
        counts,
        max_ma,
    })
}

/// Pulses of the move to the reference end stop: closing, the held position plus
/// [`CLOSE_EXTRA_COUNTS`] (the stroke when the position is unknown); opening, the whole range;
/// [`REF_RETRY_COUNTS`] for a further attempt.
fn reference_counts(fv: &FlowValve, s: &ValveState, dir: MoveDir) -> u16 {
    if fv.ref_attempts > 0 {
        return REF_RETRY_COUNTS;
    }
    if dir == MoveDir::Open {
        return FLOW_COUNTS_MAX;
    }
    if fv.held_valid {
        return fv
            .held_counts
            .saturating_add(CLOSE_EXTRA_COUNTS)
            .min(FLOW_COUNTS_MAX);
    }
    let stroke = s.open_count.max(s.close_count);
    if stroke == 0 {
        FLOW_COUNTS_MAX
    } else {
        stroke.min(u32::from(FLOW_COUNTS_MAX)) as u16
    }
}

/// One pulse more (less at the maximum) when the move would equal the STM's last move, so the
/// record of this move differs from the last one (the ESP sees a change).
fn distinct(n: u16, s: &ValveState) -> u16 {
    if s.last_move.requested_counts != u32::from(n) {
        return n;
    }
    if n == FLOW_COUNTS_MAX {
        n - 1
    } else {
        n + 1
    }
}

/// The record of the accepted move arrived: the next step, or the failure.
fn move_done(
    fv: &mut FlowValve,
    cfg: &FlowConfig,
    s: &ValveState,
    valve: u8,
    now_ms: u32,
    log: &mut dyn FnMut(&Event),
) {
    let stop = s.last_move.stop;
    let at_end = matches!(stop, StopReason::EndStop | StopReason::EarlyEndStop);
    let settled = at_end || stop == StopReason::Target;
    match fv.step {
        Step::Reference | Step::WarmClose if at_end => {
            if fv.target_counts == 0 {
                // closed: the reference close is the final move
                fv.held_counts = 0;
                finish_move(fv, s);
            } else if cfg.ref_side == RefSide::Open && final_close(fv, cfg) == 0 {
                // the target is the open end stop itself: nothing to close
                fv.held_counts = cfg.stroke_counts;
                finish_move(fv, s);
            } else if fv.step == Step::Reference && fv.pre_close(cfg) {
                fv.step = Step::WarmUp;
            } else {
                fv.step = Step::Final;
            }
        }
        Step::Reference | Step::WarmClose if stop == StopReason::Target => {
            fv.ref_attempts = fv.ref_attempts.saturating_add(1);
            if fv.ref_attempts >= REF_ATTEMPTS {
                fail(fv, valve, stop as i32, b"no end stop", now_ms, log);
            }
        }
        Step::WarmUp if settled => fv.step = Step::WarmClose,
        Step::Final if settled => {
            let counted = s.last_move.counted_counts.min(u32::from(FLOW_COUNTS_MAX)) as u16;
            // the held position counts from the closed end stop on both sides
            fv.held_counts = match cfg.ref_side {
                RefSide::Closed => counted,
                RefSide::Open => cfg.stroke_counts.saturating_sub(counted),
            };
            finish_move(fv, s);
        }
        _ => fail(
            fv,
            valve,
            stop as i32,
            stop_reason_name(stop).as_bytes(),
            now_ms,
            log,
        ),
    }
}

impl FlowValve {
    /// The warm-up of the closed end stop runs for this positioning.
    fn pre_close(&self, cfg: &FlowConfig) -> bool {
        cfg.pre_close && cfg.ref_side == RefSide::Closed
    }
}

/// The final move of a positioning ended: its record is kept, the keep follows.
fn finish_move(fv: &mut FlowValve, s: &ValveState) {
    fv.held_seq = s.move_seq;
    fv.last_valid = true;
    fv.last_ms = s.last_move.duration_ms;
    fv.last_peak = s.last_move.peak_current;
    fv.step = Step::Keep;
}

/// The keep was acknowledged: the position is held, event FlowPositioned.
fn held(fv: &mut FlowValve, valve: u8, now_ms: u32, log: &mut dyn FnMut(&Event)) {
    fv.state = FlowState::Held;
    fv.step = Step::None;
    fv.held_valid = true;
    fv.held_at_ms = now_ms;
    log(&event(
        EventCode::FlowPositioned,
        valve,
        i32::from(fv.held_counts),
        i32::from(fv.target_pct),
        b"",
    ));
    if fv.restart_pending {
        fv.restart_pending = false;
        fv.state = FlowState::Unknown;
    }
}

/// The positioning failed: event FlowPositionFailed (arg1 the step, arg2 `code`).
fn fail(
    fv: &mut FlowValve,
    valve: u8,
    code: i32,
    text: &[u8],
    now_ms: u32,
    log: &mut dyn FnMut(&Event),
) {
    let step = fv.step as i32;
    fv.state = FlowState::Failed;
    fv.step = Step::None;
    fv.move_pending = false;
    fv.waiting = false;
    fv.wait_calib = false;
    fv.held_valid = false;
    fv.failed_ms = now_ms;
    fv.restart_pending = false;
    log(&event(
        EventCode::FlowPositionFailed,
        valve,
        step,
        code,
        text,
    ));
}

/// The next request of the sequence waits `ms` (and, `calib`, until a calibration ended).
fn wait(fv: &mut FlowValve, ms: u32, calib: bool, now_ms: u32) {
    fv.waiting = true;
    fv.wait_until_ms = now_ms.wrapping_add(ms);
    fv.wait_calib = calib;
}

#[cfg(test)]
mod tests;
