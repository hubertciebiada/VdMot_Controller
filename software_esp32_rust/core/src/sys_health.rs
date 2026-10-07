//! System health: heap and stack alarms, the heap guard, the OTA self-check answer, reset
//! reasons (port of `vdm/sys_health.h`). Hardware-free.

use crate::common::{elapsed_ms, NO_VALVE};
use crate::event_log::{event_default_severity, make_event, Event, EventCode};

/// A task's stack is low below max(512, stack_bytes / 8) free bytes.
pub fn stack_low_threshold(stack_bytes: u32) -> u32 {
    (stack_bytes / 8).max(512)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ResourceMonitor {
    heap_reported: bool,
    heap_report_ms: u32,
    frag_reported: bool,
    frag_report_ms: u32,
    min_largest: u32,
    sampled: bool,
    /// per task index
    stack_reported: [bool; ResourceMonitor::MAX_TASKS as usize],
}

/// True when an alarm is due: never reported, or `REPEAT_MS` since the last report.
fn alarm_due(reported: bool, report_ms: u32, now_ms: u32) -> bool {
    !reported || elapsed_ms(now_ms, report_ms) >= ResourceMonitor::REPEAT_MS
}

impl ResourceMonitor {
    pub const LOW_HEAP_BYTES: u32 = 30 * 1024;
    /// 2x Update's 4 KB buffer
    pub const LOW_LARGEST_BLOCK_BYTES: u32 = 8 * 1024;
    pub const REPEAT_MS: u32 = 3_600_000;
    pub const MAX_TASKS: u8 = 8;

    /// Every 10 s. LowHeap (free < LOW_HEAP_BYTES; arg1 free, arg2 min free) and HeapFragmented
    /// (largest < LOW_LARGEST_BLOCK_BYTES; arg1 largest, arg2 free), each at most once per
    /// REPEAT_MS, LowHeap first. Tracks the minimum largest block. Returns the events written to
    /// `out` (at most `out.len()`).
    pub fn on_heap(
        &mut self,
        free_heap: u32,
        min_free_heap: u32,
        largest_block: u32,
        now_ms: u32,
        out: &mut [Event],
    ) -> usize {
        self.min_largest = if self.sampled {
            self.min_largest.min(largest_block)
        } else {
            largest_block
        };
        self.sampled = true;
        let mut slots = out.iter_mut();
        let mut n = 0;
        if free_heap < Self::LOW_HEAP_BYTES
            && alarm_due(self.heap_reported, self.heap_report_ms, now_ms)
        {
            if let Some(slot) = slots.next() {
                self.heap_reported = true;
                self.heap_report_ms = now_ms;
                *slot = alarm(EventCode::LowHeap, free_heap, min_free_heap);
                n += 1;
            }
        }
        if largest_block < Self::LOW_LARGEST_BLOCK_BYTES
            && alarm_due(self.frag_reported, self.frag_report_ms, now_ms)
        {
            if let Some(slot) = slots.next() {
                self.frag_reported = true;
                self.frag_report_ms = now_ms;
                *slot = alarm(EventCode::HeapFragmented, largest_block, free_heap);
                n += 1;
            }
        }
        n
    }

    /// StackLow once per task index per boot when min_free_bytes <
    /// stack_low_threshold(stack_bytes); text = `name` (a C string, truncated to
    /// EVENT_TEXT_MAX). task >= MAX_TASKS is ignored. Returns the events written to `out`.
    pub fn on_stack(
        &mut self,
        task: u8,
        name: &[u8],
        stack_bytes: u32,
        min_free_bytes: u32,
        out: &mut [Event],
    ) -> usize {
        let (Some(reported), Some(slot)) = (
            self.stack_reported.get_mut(usize::from(task)),
            out.first_mut(),
        ) else {
            return 0;
        };
        if *reported || min_free_bytes >= stack_low_threshold(stack_bytes) {
            return 0;
        }
        *reported = true;
        *slot = make_event(
            EventCode::StackLow,
            event_default_severity(EventCode::StackLow),
            NO_VALVE,
            min_free_bytes as i32,
            stack_bytes as i32,
            name,
        );
        1
    }

    /// 0 before the first [`on_heap`](Self::on_heap)
    pub fn min_largest_block(&self) -> u32 {
        self.min_largest
    }
}

/// A heap alarm with its default severity; the byte counts become the int32 args like the C++
/// casts.
fn alarm(code: EventCode, arg1: u32, arg2: u32) -> Event {
    make_event(
        code,
        event_default_severity(code),
        NO_VALVE,
        arg1 as i32,
        arg2 as i32,
        b"",
    )
}

/// [`HeapGuard`]'s low period (C++ `HeapGuard::State`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum HeapGuardState {
    #[default]
    Idle,
    Low,
    Fired,
}

/// Heap guard: a leak that keeps the free heap below CRITICAL_BYTES on every sample for HOLD_MS
/// ends in a controlled restart instead of a crash. Armed from ARM_MS of uptime on, so a boot
/// that starts below the threshold restarts every ARM_MS + HOLD_MS at most, not every HOLD_MS.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HeapGuard {
    armed: bool,
    state: HeapGuardState,
    low_since_ms: u32,
}

impl HeapGuard {
    pub const CRITICAL_BYTES: u32 = 12 * 1024;
    pub const HOLD_MS: u32 = 60_000;
    pub const ARM_MS: u32 = 600_000;

    /// Every second. `uptime_ms`: millis() since boot; armed once it reaches ARM_MS (the wrap
    /// after 49.7 days does not disarm it). `blocked` (an ESP upload or an STM flash runs, or a
    /// restart is pending) ends a low period like a sample at or above CRITICAL_BYTES, so the
    /// hold time starts again after it. True once per low period: at its first sample HOLD_MS or
    /// more after its first one.
    pub fn on_sample(&mut self, free_heap: u32, blocked: bool, uptime_ms: u32) -> bool {
        if !self.armed && uptime_ms < Self::ARM_MS {
            return false;
        }
        self.armed = true;
        if blocked || free_heap >= Self::CRITICAL_BYTES {
            self.state = HeapGuardState::Idle;
            return false;
        }
        if self.state == HeapGuardState::Idle {
            self.state = HeapGuardState::Low;
            self.low_since_ms = uptime_ms;
        }
        if self.state != HeapGuardState::Low
            || elapsed_ms(uptime_ms, self.low_since_ms) < Self::HOLD_MS
        {
            return false;
        }
        self.state = HeapGuardState::Fired;
        true
    }
}

/// OTA self-check: the status line starts "HTTP/1.0 200" or "HTTP/1.1 200" followed by the end,
/// ' ' or '\r'.
pub fn http_status_ok(line: &[u8]) -> bool {
    match line {
        [b'H', b'T', b'T', b'P', b'/', b'1', b'.', b'0' | b'1', b' ', b'2', b'0', b'0', rest @ ..] =>
        {
            matches!(rest.first(), None | Some(b' ' | b'\r'))
        }
        _ => false,
    }
}

/// ESP-IDF esp_reset_reason_t values that point at a crash or a power problem: PANIC 4,
/// INT_WDT 5, TASK_WDT 6, WDT 7, BROWNOUT 9.
pub fn is_abnormal_reset(reason: i32) -> bool {
    matches!(reason, 4..=7 | 9)
}

#[cfg(test)]
mod tests;
