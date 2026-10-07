//! Test harness of the glue (docs/rust/GLUE-DESIGN-ESP.md 5.1 and 5.3): a fake of every port of
//! [`crate::port`], the board that keeps NVS, LittleFS, the OTA slots and the RTC block across
//! simulated boots, and the journal of side effects. No globals: every test builds its own
//! [`FakeBoard`], so the cases run in parallel threads.
//!
//! Volatile fakes (clock, heap gate, sockets, UART, pins, network) are new at every boot; the
//! persistent stores belong to the board. Failure knobs of a store are volatile (they belong to
//! the boot that scripts them), its content is not.
#![allow(dead_code)] // fakes serve the suites of every glue module, not all of them use each knob

pub(crate) mod board;
pub(crate) mod broker;
pub(crate) mod clock;
pub(crate) mod fake_stm;
pub(crate) mod fs;
pub(crate) mod http;
pub(crate) mod md5;
pub(crate) mod net;
pub(crate) mod nvs;
pub(crate) mod ota;
pub(crate) mod system;
pub(crate) mod uart;

mod tests;

use std::sync::{Arc, Mutex, MutexGuard};

pub(crate) use board::{run, Device, Ended, FakeBoard, Reset};
pub(crate) use broker::FakeBroker;
pub(crate) use clock::{FakeClock, FakeWall};
pub(crate) use fake_stm::FakeStm;
pub(crate) use fs::FakeFs;
pub(crate) use http::{FakeHttpServer, FakeRequest};
pub(crate) use md5::FakeMd5;
pub(crate) use net::{FakeEthernet, FakePinger, FakeSntp, FakeTcp, FakeUdp, FakeWifi};
pub(crate) use nvs::FakeNvs;
pub(crate) use ota::{FakeOta, SlotImage};
pub(crate) use system::{FakeConsole, FakeHeap, FakeRtc, FakeSystem, FakeWatchdog};
pub(crate) use uart::{FakeGpio, FakeUart};

/// Locks a fake's state; a poisoned lock (a panic while it was held) is a broken invariant of
/// the case, so the test fails right there.
pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    match m.lock() {
        Ok(g) => g,
        Err(_) => panic!("a fake's lock is poisoned: an earlier panic broke the case"),
    }
}

/// One ordered list of side effects across the fakes of a boot, e.g. `gpio 15=1`,
/// `nvs set vdmrev/otaTrial`, `esp_restart`.
#[derive(Clone, Default)]
pub(crate) struct Journal(Arc<Mutex<Vec<String>>>);

impl Journal {
    /// Appends one entry.
    pub(crate) fn note(&self, entry: impl Into<String>) {
        lock(&self.0).push(entry.into());
    }
    /// Every entry in order.
    pub(crate) fn entries(&self) -> Vec<String> {
        lock(&self.0).clone()
    }
    /// Entries starting with `prefix`, in order.
    pub(crate) fn of(&self, prefix: &str) -> Vec<String> {
        lock(&self.0)
            .iter()
            .filter(|e| e.starts_with(prefix))
            .cloned()
            .collect()
    }
    /// Index of the first entry equal to `entry` at or after `from`.
    pub(crate) fn find(&self, entry: &str, from: usize) -> Option<usize> {
        lock(&self.0)
            .iter()
            .enumerate()
            .skip(from)
            .find(|(_, e)| e.as_str() == entry)
            .map(|(i, _)| i)
    }
    /// Forgets every entry.
    pub(crate) fn clear(&self) {
        lock(&self.0).clear();
    }
}
