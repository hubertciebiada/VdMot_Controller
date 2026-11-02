//! Desired targets that survive an ESP restart: the 46-byte record kept in RTC slow memory
//! (software restarts, panics, watchdog resets) and in NVS `vdmrev/targets` (power loss), the
//! boot choice between the two and the debounced NVS saver (port of `vdm/target_store.h`). A
//! target in l/min (Rust 2.2.11) needs its ml/min as well: a 58-byte record of its own beside
//! the first, in RTC and in NVS `vdmrev/flowtgt`, so that record and the RTC layout before it
//! stay as they are. Hardware-free.

use crate::common::{elapsed_ms, VALVE_COUNT};
use crate::config::crc32;
use crate::valve_model::{TargetSource, ValveModel};

/// Valves (the arrays of the record).
const N: usize = VALVE_COUNT as usize;

pub const PERSISTED_TARGETS_SIZE: usize = 46;

const MAGIC: [u8; 4] = *b"VDTG";
const VERSION: u8 = 1;
const ENTRIES_AT: usize = 6;
/// 42
const CRC_AT: usize = ENTRIES_AT + 3 * N;
const _: () = assert!(CRC_AT + 4 == PERSISTED_TARGETS_SIZE);

pub const PERSISTED_FLOW_SIZE: usize = 58;

const FLOW_MAGIC: [u8; 4] = *b"VDFT";
/// 54
const FLOW_CRC_AT: usize = ENTRIES_AT + 4 * N;
const _: () = assert!(FLOW_CRC_AT + 4 == PERSISTED_FLOW_SIZE);

#[derive(Clone, Copy, Debug, Default, Eq)]
pub struct PersistedTargets {
    pub valid: [bool; N],
    /// 0..100
    pub pos: [u8; N],
    /// None..Assembly
    pub source: [TargetSource; N],
    /// ml/min of a target in l/min, `pos` is its %
    pub flow_ml: [Option<u16>; N],
}

impl PartialEq for PersistedTargets {
    /// Invalid entries carry no value (encoded as 0, 0, 0): only `valid` is compared for them.
    fn eq(&self, other: &Self) -> bool {
        (0..N).all(|i| {
            self.valid[i] == other.valid[i]
                && (!self.valid[i]
                    || (self.pos[i] == other.pos[i]
                        && self.source[i] == other.source[i]
                        && self.flow_ml[i] == other.flow_ml[i]))
        })
    }
}

/// Layout: "VDTG" (4), version 1 (1), count 12 (1), 12 x {flags bit0 valid, pos, source} (36),
/// CRC-32 (crc32 over bytes 0..41, little endian) (4) = 46 bytes. Invalid entries are written
/// as 0, 0, 0. Returns 46.
pub fn encode_targets(t: &PersistedTargets, out: &mut [u8; PERSISTED_TARGETS_SIZE]) -> usize {
    out[..4].copy_from_slice(&MAGIC);
    out[4] = VERSION;
    out[5] = VALVE_COUNT;
    let values = t.valid.iter().zip(t.pos.iter().zip(&t.source));
    let (entries, _) = out[ENTRIES_AT..CRC_AT].as_chunks_mut::<3>();
    for (e, (&valid, (&pos, &source))) in entries.iter_mut().zip(values) {
        *e = if valid {
            [1, pos, source as u8]
        } else {
            [0, 0, 0]
        };
    }
    let sum = crc32(&out[..CRC_AT], 0);
    out[CRC_AT..].copy_from_slice(&sum.to_le_bytes());
    PERSISTED_TARGETS_SIZE
}

/// false (out = default: all invalid) for a length other than 46, a bad magic/version/count/CRC,
/// or pos > 100, source > Assembly or flag bits other than bit0 in a valid entry.
pub fn decode_targets(data: &[u8], out: &mut PersistedTargets) -> bool {
    *out = PersistedTargets::default();
    let Ok(b) = <&[u8; PERSISTED_TARGETS_SIZE]>::try_from(data) else {
        return false;
    };
    let (body, crc) = b.split_at(CRC_AT);
    if body[..4] != MAGIC || body[4] != VERSION || body[5] != VALVE_COUNT {
        return false;
    }
    if crc != crc32(body, 0).to_le_bytes().as_slice() {
        return false;
    }
    let mut t = PersistedTargets::default();
    let slots = t
        .valid
        .iter_mut()
        .zip(t.pos.iter_mut().zip(t.source.iter_mut()));
    let (entries, _) = body[ENTRIES_AT..].as_chunks::<3>();
    for (&[flags, pos, source], (valid, (pos_out, source_out))) in entries.iter().zip(slots) {
        if flags == 0 {
            continue;
        }
        let (1, true, Some(source)) = (flags, pos <= 100, TargetSource::from_raw(source)) else {
            return false;
        };
        *valid = true;
        *pos_out = pos;
        *source_out = source;
    }
    *out = t;
    true
}

/// Layout: "VDFT" (4), version 1 (1), count 12 (1), 12 x {flags bit0 flow, pos, ml/min little
/// endian} (48), CRC-32 (crc32 over bytes 0..53, little endian) (4) = 58 bytes. Entries without a
/// flow are written as 0, 0, 0, 0. Returns 58.
pub fn encode_flow_targets(t: &PersistedTargets, out: &mut [u8; PERSISTED_FLOW_SIZE]) -> usize {
    out[..4].copy_from_slice(&FLOW_MAGIC);
    out[4] = VERSION;
    out[5] = VALVE_COUNT;
    let values = t.valid.iter().zip(t.pos.iter().zip(&t.flow_ml));
    let (entries, _) = out[ENTRIES_AT..FLOW_CRC_AT].as_chunks_mut::<4>();
    for (e, (&valid, (&pos, &flow))) in entries.iter_mut().zip(values) {
        *e = match flow.filter(|_| valid) {
            Some(ml) => {
                let [low, high] = ml.to_le_bytes();
                [1, pos, low, high]
            }
            None => [0; 4],
        };
    }
    let sum = crc32(&out[..FLOW_CRC_AT], 0);
    out[FLOW_CRC_AT..].copy_from_slice(&sum.to_le_bytes());
    PERSISTED_FLOW_SIZE
}

/// The flows of the record on the targets `t` holds: an entry counts only where `t` has a target
/// of the same %, so a record older than the targets leaves that valve its target in %. false
/// (`t` as it was) for a length other than 58, a bad magic/version/count/CRC or flag bits other
/// than bit0.
pub fn decode_flow_targets(data: &[u8], t: &mut PersistedTargets) -> bool {
    let Ok(b) = <&[u8; PERSISTED_FLOW_SIZE]>::try_from(data) else {
        return false;
    };
    let (body, crc) = b.split_at(FLOW_CRC_AT);
    if body[..4] != FLOW_MAGIC || body[4] != VERSION || body[5] != VALVE_COUNT {
        return false;
    }
    if crc != crc32(body, 0).to_le_bytes().as_slice() {
        return false;
    }
    let (entries, _) = body[ENTRIES_AT..].as_chunks::<4>();
    if entries.iter().any(|e| e[0] > 1) {
        return false;
    }
    let slots = t.valid.iter().zip(t.pos.iter().zip(t.flow_ml.iter_mut()));
    for (&[flags, pos, low, high], (&valid, (&held, flow))) in entries.iter().zip(slots) {
        if flags == 1 && valid && pos == held {
            *flow = Some(u16::from_le_bytes([low, high]));
        }
    }
    true
}

/// Every valve with a desired target.
pub fn capture_targets(m: &ValveModel, out: &mut PersistedTargets) {
    *out = PersistedTargets::default();
    for (v, i) in (0..VALVE_COUNT).zip(0..N) {
        let s = m.valve(v);
        if !s.desired_valid {
            continue;
        }
        out.valid[i] = true;
        out.pos[i] = s.desired;
        out.source[i] = s.source;
        out.flow_ml[i] = s.flow_target();
    }
}

/// `restore_desired()` or `restore_flow()` per valid entry; returns the number of valves
/// restored.
pub fn restore_targets(m: &mut ValveModel, t: &PersistedTargets) -> u8 {
    let mut n = 0;
    for (v, i) in (0..VALVE_COUNT).zip(0..N) {
        let (pos, source) = (t.pos[i], t.source[i]);
        let restored = match t.flow_ml[i] {
            Some(ml) => t.valid[i] && m.restore_flow(v, pos, ml, source),
            None => t.valid[i] && m.restore_desired(v, pos, source),
        };
        if restored {
            n += 1;
        }
    }
    n
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RestoreSource {
    #[default]
    None = 0,
    Rtc = 1,
    Nvs = 2,
}

impl RestoreSource {
    pub fn from_raw(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::None),
            1 => Some(Self::Rtc),
            2 => Some(Self::Nvs),
            _ => None,
        }
    }
}

/// The RTC copy wins when it decodes (it is never older than NVS), else NVS, else None
/// (out = default).
pub fn choose_targets(rtc: &[u8], nvs: &[u8], out: &mut PersistedTargets) -> RestoreSource {
    if decode_targets(rtc, out) {
        return RestoreSource::Rtc;
    }
    if decode_targets(nvs, out) {
        return RestoreSource::Nvs;
    }
    RestoreSource::None
}

/// The flows of the record from where [`choose_targets`] took the targets; without a good record
/// they stay targets in %.
pub fn choose_flow_targets(
    source: RestoreSource,
    rtc: &[u8],
    nvs: &[u8],
    t: &mut PersistedTargets,
) {
    let record = if source == RestoreSource::Rtc {
        rtc
    } else {
        nvs
    };
    decode_flow_targets(record, t);
}

// ---------------------------------------------------------------- TargetSaver

/// NVS copy, written by the app task: 5 min after the last change, at most 30 min after the
/// first unsaved change, only when the value differs from the stored one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TargetSaver {
    stored: PersistedTargets,
    current: PersistedTargets,
    dirty: bool,
    last_change_ms: u32,
    first_change_ms: u32,
}

impl TargetSaver {
    pub const DEBOUNCE_MS: u32 = 300000;
    pub const MAX_DELAY_MS: u32 = 1800000;

    /// Boot: what NVS holds.
    pub fn prime_stored(&mut self, t: &PersistedTargets) {
        self.stored = *t;
        self.current = *t;
        self.dirty = false;
    }

    /// Differs from the current value: new current value, dirty unless it equals the stored
    /// one; the debounce restarts, the first-change time is kept while dirty.
    pub fn update(&mut self, t: &PersistedTargets, now_ms: u32) {
        if *t == self.current {
            return;
        }
        self.current = *t;
        if self.current == self.stored {
            self.dirty = false;
            return;
        }
        if !self.dirty {
            self.first_change_ms = now_ms;
        }
        self.dirty = true;
        self.last_change_ms = now_ms;
    }

    pub fn dirty(&self) -> bool {
        self.dirty
    }

    pub fn due(&self, now_ms: u32) -> bool {
        self.dirty
            && (elapsed_ms(now_ms, self.last_change_ms) >= Self::DEBOUNCE_MS
                || elapsed_ms(now_ms, self.first_change_ms) >= Self::MAX_DELAY_MS)
    }

    /// The encoded current value.
    pub fn bytes(&self) -> [u8; PERSISTED_TARGETS_SIZE] {
        let mut bytes = [0; PERSISTED_TARGETS_SIZE];
        encode_targets(&self.current, &mut bytes);
        bytes
    }

    /// The encoded flows of the current value.
    pub fn flow_bytes(&self) -> [u8; PERSISTED_FLOW_SIZE] {
        let mut bytes = [0; PERSISTED_FLOW_SIZE];
        encode_flow_targets(&self.current, &mut bytes);
        bytes
    }

    /// ok: stored := current, clean; !ok: the next attempt DEBOUNCE_MS later.
    pub fn saved(&mut self, ok: bool, now_ms: u32) {
        if ok {
            self.stored = self.current;
            self.dirty = false;
            return;
        }
        // Retry one debounce later; the maximum delay starts again as well.
        self.last_change_ms = now_ms;
        self.first_change_ms = now_ms;
    }
}

#[cfg(test)]
mod tests;
