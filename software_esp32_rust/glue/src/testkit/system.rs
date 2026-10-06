//! Fake chip: restart and reset reasons, heap figures, the heap gate, RTC memory, the console and
//! the task watchdog.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};

use super::board::{BootState, Reset};
use super::{lock, Journal};
use crate::port::{Console, HeapGate, HeapStats, Rtc, System, Watchdog};

/// Payload of the panic of [`System::restart`]: `testkit::run` catches it and the board boots
/// again with reset reason SW.
#[derive(Debug)]
pub(crate) struct Restarted;

/// Knobs of the chip.
pub(crate) struct SystemState {
    pub(crate) heap: HeapStats,
    /// Lowest free stack per task name (absent: the task does not exist yet).
    pub(crate) stacks: BTreeMap<String, u32>,
    pub(crate) mac: [u8; 6],
    pub(crate) restarts: u32,
}

impl Default for SystemState {
    fn default() -> Self {
        SystemState {
            heap: HeapStats {
                free: 150_000,
                min_free: 120_000,
                largest: 110_000,
            },
            stacks: BTreeMap::new(),
            mac: [0x24, 0x0A, 0xC4, 0x12, 0x34, 0x56],
            restarts: 0,
        }
    }
}

/// The chip of a boot.
#[derive(Clone)]
pub(crate) struct FakeSystem {
    reset: Reset,
    boot: Arc<Mutex<BootState>>,
    state: Arc<Mutex<SystemState>>,
    journal: Journal,
}

impl FakeSystem {
    pub(crate) fn new(reset: Reset, boot: Arc<Mutex<BootState>>, journal: Journal) -> Self {
        FakeSystem {
            reset,
            boot,
            state: Arc::default(),
            journal,
        }
    }
    pub(crate) fn state(&self) -> MutexGuard<'_, SystemState> {
        lock(&self.state)
    }
    /// The reset that started this boot.
    pub(crate) fn reset_kind(&self) -> Reset {
        self.reset
    }
}

impl System for FakeSystem {
    fn restart(&self) -> ! {
        lock(&self.state).restarts += 1;
        self.journal.note("esp_restart");
        lock(&self.boot).end(Reset::Software);
        std::panic::panic_any(Restarted)
    }
    fn reset_reason(&self) -> u8 {
        self.reset.esp_reason()
    }
    fn heap(&self) -> HeapStats {
        lock(&self.state).heap
    }
    fn stack_min_free(&self, task: &str) -> Option<u32> {
        lock(&self.state).stacks.get(task).copied()
    }
    fn base_mac(&self) -> [u8; 6] {
        lock(&self.state).mac
    }
}

/// Script and records of the heap gate (the C++ `fakes::heap()`).
#[derive(Default)]
pub(crate) struct HeapState {
    /// Outcome of the next requests in order; then `fail_all` decides.
    pub(crate) next: VecDeque<bool>,
    pub(crate) fail_all: bool,
    /// Sizes of the granted requests.
    pub(crate) granted: Vec<usize>,
    /// Sizes of the refused requests.
    pub(crate) refused: Vec<usize>,
}

/// The heap gate of a boot.
#[derive(Clone, Default)]
pub(crate) struct FakeHeap(Arc<Mutex<HeapState>>);

impl FakeHeap {
    pub(crate) fn state(&self) -> MutexGuard<'_, HeapState> {
        lock(&self.0)
    }
}

impl HeapGate for FakeHeap {
    fn grant(&self, bytes: usize) -> bool {
        let mut s = lock(&self.0);
        let ok = match s.next.pop_front() {
            Some(v) => v,
            None => !s.fail_all,
        };
        if ok {
            s.granted.push(bytes);
        } else {
            s.refused.push(bytes);
        }
        ok
    }
}

/// Size of the fake RTC slow memory block.
pub(crate) const RTC_SIZE: usize = 512;

/// RTC slow memory: the board's block, kept by every reset but a power-on (0xA5 then).
#[derive(Clone)]
pub(crate) struct FakeRtc(Arc<Mutex<Vec<u8>>>);

impl FakeRtc {
    pub(crate) fn new(block: Arc<Mutex<Vec<u8>>>) -> Self {
        FakeRtc(block)
    }
    /// A copy of the whole block.
    pub(crate) fn snapshot(&self) -> Vec<u8> {
        lock(&self.0).clone()
    }
}

impl Rtc for FakeRtc {
    fn load(&self, offset: usize, out: &mut [u8]) {
        let b = lock(&self.0);
        let src = b
            .get(offset..offset + out.len())
            .unwrap_or_else(|| panic!("RTC load out of range: {offset}+{}", out.len()));
        out.copy_from_slice(src);
    }
    fn store(&self, offset: usize, data: &[u8]) {
        let mut b = lock(&self.0);
        let dst = b
            .get_mut(offset..offset + data.len())
            .unwrap_or_else(|| panic!("RTC store out of range: {offset}+{}", data.len()));
        dst.copy_from_slice(data);
    }
}

/// The serial console: every line.
#[derive(Clone, Default)]
pub(crate) struct FakeConsole(Arc<Mutex<Vec<Vec<u8>>>>);

impl FakeConsole {
    pub(crate) fn lines(&self) -> Vec<Vec<u8>> {
        lock(&self.0).clone()
    }
}

impl Console for FakeConsole {
    fn line(&self, text: &[u8]) {
        lock(&self.0).push(text.to_vec());
    }
}

/// The task watchdog subscription of a thread: counts the feeds.
#[derive(Clone, Default)]
pub(crate) struct FakeWatchdog(Arc<Mutex<u64>>);

impl FakeWatchdog {
    pub(crate) fn feeds(&self) -> u64 {
        *lock(&self.0)
    }
}

impl Watchdog for FakeWatchdog {
    fn feed(&self) {
        *lock(&self.0) += 1;
    }
}
