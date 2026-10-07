//! Rate limit and aggregation of events published to MQTT (port of `vdm/event_limiter.h`).
//! Hardware-free.

use core::cmp::Reverse;

use crate::common::{elapsed_ms, ALL_VALVES, VALVE_COUNT};
use crate::event_log::{event_reaches_mqtt, Event, Severity};

const HOUR_MS: u64 = 3_600_000;

/// A token bucket: its per-hour count, refilled one token per 3600000 / per_hour ms.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Bucket {
    per_hour: u16,
    /// tokens x 1000
    tokens_milli: u32,
    last_refill_ms: u32,
    /// sub-milli-token refill carried over (< 3600000)
    remainder: u32,
}

impl Bucket {
    /// Full: the first refill only moves last_refill_ms (the start time is irrelevant).
    fn full(per_hour: u16) -> Self {
        Self {
            per_hour,
            tokens_milli: u32::from(per_hour) * 1000,
            last_refill_ms: 0,
            remainder: 0,
        }
    }

    fn refill(&mut self, now_ms: u32) {
        let capacity = u32::from(self.per_hour) * 1000;
        let num =
            u64::from(elapsed_ms(now_ms, self.last_refill_ms)) * u64::from(self.per_hour) * 1000
                + u64::from(self.remainder);
        self.last_refill_ms = now_ms;
        let add = num / HOUR_MS;
        // < 3600000
        self.remainder = (num % HOUR_MS) as u32;
        if u64::from(self.tokens_milli) + add >= u64::from(capacity) {
            self.tokens_milli = capacity;
            // Remainders are multiples of 1000 (so is an hour in ms): a remainder off by < 1000
            // never changes a refill.
            self.remainder = 0;
        } else {
            // < capacity
            self.tokens_milli += add as u32;
        }
    }
}

/// One (code, valve) entry of the key table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Key {
    code: u16,
    valve: u8,
    used: bool,
    last_ms: u32,
    /// per-key time of the class that booked it
    hold_ms: u32,
}

const FREE_KEY: Key = Key {
    code: 0,
    valve: 0,
    used: false,
    last_ms: 0,
    hold_ms: 0,
};

/// Rate limit for events published to MQTT (`<root>/events`) by priority class:
///   Critical: no hourly budget, one event per (code, valve) per critical_per_key_ms;
///   Error:    own token bucket of error_per_hour, one per (code, valve) per per_key_ms;
///   Normal (everything else that reaches MQTT): bucket of max_per_hour, per key per_key_ms.
/// A bucket holds its per-hour count and refills one token per 3600000/per_hour ms. Events for
/// which [`event_reaches_mqtt`] is false are refused without counting them as suppressed. Fixed
/// table of [`KEYS`](Self::KEYS) (valve, code) entries; when full the least recently used entry
/// is recycled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventRateLimiter {
    per_key_ms: u32,
    critical_per_key_ms: u32,
    normal: Bucket,
    error: Bucket,
    keys: [Key; EventRateLimiter::KEYS],
    suppressed: u32,
}

impl Default for EventRateLimiter {
    /// The C++ default arguments: 600000, 30, 30, 60000.
    fn default() -> Self {
        Self::new(600_000, 30, 30, 60_000)
    }
}

impl EventRateLimiter {
    pub const KEYS: usize = 64;

    /// C++ defaults: per_key_ms 600000, max_per_hour 30, error_per_hour 30,
    /// critical_per_key_ms 60000 ([`Default`]).
    pub fn new(
        per_key_ms: u32,
        max_per_hour: u16,
        error_per_hour: u16,
        critical_per_key_ms: u32,
    ) -> Self {
        Self {
            per_key_ms,
            critical_per_key_ms,
            normal: Bucket::full(max_per_hour),
            error: Bucket::full(error_per_hour),
            keys: [FREE_KEY; Self::KEYS],
            suppressed: 0,
        }
    }

    /// True when the event may be published now (and books it). Key entries older than their
    /// per-key time are freed on every call, so a millis() wrap cannot resurrect them.
    pub fn allow(&mut self, e: &Event, now_ms: u32) -> bool {
        self.normal.refill(now_ms);
        self.error.refill(now_ms);
        for k in &mut self.keys {
            if k.used && elapsed_ms(now_ms, k.last_ms) >= k.hold_ms {
                k.used = false;
            }
        }
        if !event_reaches_mqtt(e) {
            return false;
        }

        let critical = e.severity >= Severity::Critical;
        let code = e.code as u16;
        if self
            .keys
            .iter()
            .any(|k| k.used && k.code == code && k.valve == e.valve)
        {
            // Still within its per-key time (older entries were freed above).
            self.suppressed = self.suppressed.wrapping_add(1);
            return false;
        }
        let bucket = if critical {
            None
        } else if e.severity == Severity::Error {
            Some(&mut self.error)
        } else {
            Some(&mut self.normal)
        };
        if let Some(b) = bucket {
            if b.tokens_milli < 1000 {
                self.suppressed = self.suppressed.wrapping_add(1);
                return false;
            }
            b.tokens_milli -= 1000;
        }
        // A free entry, else the least recently used one (the first of equally old ones).
        let slot = match self.keys.iter().position(|k| !k.used) {
            Some(free) => free,
            None => self
                .keys
                .iter()
                .enumerate()
                .min_by_key(|(_, k)| Reverse(elapsed_ms(now_ms, k.last_ms)))
                .map_or(0, |(i, _)| i),
        };
        self.keys[slot] = Key {
            code,
            valve: e.valve,
            used: true,
            last_ms: now_ms,
            hold_ms: if critical {
                self.critical_per_key_ms
            } else {
                self.per_key_ms
            },
        };
        true
    }

    pub fn suppressed(&self) -> u32 {
        self.suppressed
    }
}

/// One MQTT event: a single event, or one code on several valves.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PublishEvent {
    /// valve = ALL_VALVES for >= 2 bits in valve_mask
    pub event: Event,
    /// bit v = valve v; 0 for events that are not about a valve
    pub valve_mask: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Slot {
    used: bool,
    opened_ms: u32,
    first: Event,
    severity: Severity,
    mask: u16,
}

impl Default for Slot {
    fn default() -> Self {
        Self {
            used: false,
            opened_ms: 0,
            first: Event::default(),
            severity: Severity::Debug,
            mask: 0,
        }
    }
}

/// Merges valve events of the same code within [`WINDOW_MS`](Self::WINDOW_MS) into one MQTT
/// event. A flushed slot with one valve bit is the original event; with two or more bits valve
/// = ALL_VALVES, the mask, the first event's seq/time/args/text and the highest severity of the
/// merged events.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EventAggregator {
    slots: [Slot; EventAggregator::SLOTS],
    duplicates: u32,
}

impl EventAggregator {
    pub const WINDOW_MS: u32 = 2000;
    pub const SLOTS: usize = 4;

    fn take(s: &mut Slot) -> PublishEvent {
        let mut out = PublishEvent {
            event: s.first.clone(),
            valve_mask: s.mask,
        };
        if s.mask.count_ones() >= 2 {
            out.event.valve = ALL_VALVES;
            out.event.severity = s.severity;
        }
        s.used = false;
        out
    }

    /// Valve events only (e.valve < VALVE_COUNT; others return None and are not taken). Joins
    /// the slot of its code or opens one; when all slots are busy the oldest is flushed and
    /// returned (the C++ true and `flushed`). A valve already in its slot is counted in
    /// duplicates() and dropped.
    pub fn offer(&mut self, e: &Event, now_ms: u32) -> Option<PublishEvent> {
        if e.valve >= VALVE_COUNT {
            return None;
        }
        let bit = 1u16 << e.valve;
        let mut free = None;
        let mut oldest: Option<(usize, u32)> = None;
        for (i, s) in self.slots.iter_mut().enumerate() {
            if !s.used {
                free = free.or(Some(i));
                continue;
            }
            if s.first.code == e.code {
                if s.mask & bit != 0 {
                    self.duplicates = self.duplicates.wrapping_add(1);
                } else {
                    // the bit is not set yet
                    s.mask += bit;
                    s.severity = s.severity.max(e.severity);
                }
                return None;
            }
            let age = elapsed_ms(now_ms, s.opened_ms);
            if oldest.is_none_or(|(_, a)| age > a) {
                oldest = Some((i, age));
            }
        }
        let (idx, flushed) = match free {
            Some(i) => (i, None),
            None => {
                // all slots busy: there is an oldest one
                let i = oldest.map_or(0, |(i, _)| i);
                (i, Some(Self::take(&mut self.slots[i])))
            }
        };
        self.slots[idx] = Slot {
            used: true,
            opened_ms: now_ms,
            first: e.clone(),
            severity: e.severity,
            mask: bit,
        };
        flushed
    }

    /// The oldest slot whose window passed (any slot with `flush_all`), taken out. C++ default
    /// for flush_all: false.
    pub fn poll(&mut self, now_ms: u32, flush_all: bool) -> Option<PublishEvent> {
        let mut oldest: Option<(usize, u32)> = None;
        for (i, s) in self.slots.iter().enumerate() {
            let age = elapsed_ms(now_ms, s.opened_ms);
            if !s.used || (!flush_all && age < Self::WINDOW_MS) {
                continue;
            }
            if oldest.is_none_or(|(_, a)| age > a) {
                oldest = Some((i, age));
            }
        }
        let (i, _) = oldest?;
        Some(Self::take(&mut self.slots[i]))
    }

    /// On disconnect: pending slots are dropped (the duplicates counter stays).
    pub fn reset(&mut self) {
        for s in &mut self.slots {
            s.used = false;
        }
    }

    pub fn duplicates(&self) -> u32 {
        self.duplicates
    }
}

#[cfg(test)]
mod tests;
