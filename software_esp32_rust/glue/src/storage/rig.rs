//! Test rig of the storage suites: the fake host (the C++ sibling logger and net), one boot of a
//! board with storage, the config blobs and loads of the C++ tests.
#![allow(clippy::large_stack_frames, clippy::large_stack_arrays)]

use std::sync::atomic::AtomicBool;
use std::sync::Mutex;

use super::*;
use crate::testkit::{Device, FakeBoard, FakeFs, FakeHeap, FakeNvs, Reset};
use vdm_esp_core::common::copy_string;
use vdm_esp_core::event_log::{event_default_severity, make_event, Event};

/// The logger and net of the C++ siblings: the events storage logged (as the ring would hold
/// them) and the network trial flag.
#[derive(Default)]
pub(super) struct Host {
    events: Mutex<Vec<Event>>,
    pub(super) trial: AtomicBool,
}

impl StorageHost for Host {
    fn log(&self, code: EventCode, valve: u8, arg1: i32, arg2: i32, text: &[u8]) {
        let e = make_event(code, event_default_severity(code), valve, arg1, arg2, text);
        self.events.lock().unwrap().push(e);
    }
    fn net_trial_active(&self) -> bool {
        self.trial.load(std::sync::atomic::Ordering::Relaxed)
    }
}

impl Host {
    pub(super) fn events(&self) -> Vec<Event> {
        self.events.lock().unwrap().clone()
    }
    pub(super) fn with_code(&self, code: EventCode) -> Vec<Event> {
        self.events()
            .into_iter()
            .filter(|e| e.code == code)
            .collect()
    }
    pub(super) fn has(&self, code: EventCode) -> bool {
        !self.with_code(code).is_empty()
    }
}

pub(super) type TestStorage<'a> = Storage<'a, FakeNvs, FakeFs, FakeHeap, &'a Host>;

/// One boot of a board: the device, storage's published data and the host.
pub(super) struct Rig {
    pub(super) board: FakeBoard,
    pub(super) dev: Device,
    pub(super) shared: StorageShared,
    pub(super) host: Host,
}

impl Rig {
    /// A board fresh from the factory, booted.
    pub(super) fn new() -> Self {
        let board = FakeBoard::new();
        let dev = board.boot();
        Rig {
            board,
            dev,
            shared: StorageShared::new(),
            host: Host::default(),
        }
    }

    /// Storage of this boot.
    pub(super) fn storage(&self) -> TestStorage<'_> {
        Storage::new(
            &self.shared,
            self.dev.nvs.clone(),
            self.dev.fs.clone(),
            self.dev.heap.clone(),
            &self.host,
        )
    }

    /// The next boot after a reset of `kind`: NVS and LittleFS stay, everything else is new.
    pub(super) fn reboot(&mut self, kind: Reset) {
        self.board.reset(kind);
        self.dev = self.board.boot();
        self.shared = StorageShared::new();
        self.host = Host::default();
    }
}

/// Mounts LittleFS (it must work).
pub(super) fn mount(st: &TestStorage<'_>) {
    let mut formatted = false;
    assert!(st.begin_fs(&mut formatted));
}

/// The defaults with `station`.
pub(super) fn named(station: &[u8]) -> Box<Config> {
    let mut c = Box::<Config>::default();
    copy_string(&mut c.station, station);
    c
}

/// The `cfg` blob of `c`.
pub(super) fn blob_of(c: &Config) -> Vec<u8> {
    let mut b = vec![0u8; CONFIG_BLOB_MAX];
    let n = encode_config(c, &mut b);
    b.truncate(n);
    b
}

/// The `cfgx` blob of `c` with the kept records `keep`.
pub(super) fn ext_of(c: &Config, keep: &[u8]) -> Vec<u8> {
    let mut b = vec![0u8; CONFIG_EXT_BLOB_MAX];
    let n = encode_config_ext(c, &mut b, keep);
    b.truncate(n);
    b
}

/// One byte in the middle flipped.
pub(super) fn corrupt(mut b: Vec<u8>) -> Vec<u8> {
    let mid = b.len() / 2;
    b[mid] ^= 0xFF;
    b
}

/// What `load_config` returned.
pub(super) struct Load {
    pub(super) src: LoadSource,
    pub(super) cfg: Box<Config>,
    pub(super) report: ImportReport,
    pub(super) details: LoadDetails,
}

pub(super) fn load(st: &TestStorage<'_>) -> Load {
    let mut l = Load {
        src: LoadSource::Defaults,
        cfg: Box::default(),
        report: ImportReport::default(),
        details: LoadDetails::default(),
    };
    l.src = st.load_config(&mut l.cfg, &mut l.report, &mut l.details);
    l
}

/// The content of a file, also without a mount (the C++ `fakes::fs().read`); `None` when it is
/// missing or a directory.
pub(super) fn file(dev: &Device, path: &str) -> Option<Vec<u8>> {
    dev.fs.read(path)
}

/// The file exists, also without a mount (the C++ `fakes::fs().exists` of a file).
pub(super) fn has_file(dev: &Device, path: &str) -> bool {
    dev.fs.read(path).is_some()
}

/// `path` is a directory of the tree.
pub(super) fn is_dir(dev: &Device, path: &str) -> bool {
    dev.fs.paths().iter().any(|p| p == path) && dev.fs.read(path).is_none()
}

/// `ns/key` as an integer of any width, 0 when missing (the C++ `getU`/`getI`).
pub(super) fn nvs_int(dev: &Device, ns: &str, key: &str) -> i64 {
    dev.nvs.get_i(ns, key)
}
