//! What the C++ `app.h` published for the tasks (docs/rust/GLUE-DESIGN-ESP.md 2.2): the command
//! queue into the stm thread, the STM snapshot with the profile store, the cheap link, flash,
//! protocol, support and revision values, the STM save state of a restart, the scheduled
//! calibration info and the tasks /api/health found. One [`AppShared`] per firmware: `main`
//! creates it at boot (the snapshot and the profile store are boot allocations, D§9), a test per
//! case.
//!
//! No lock is held while another is taken; readers copy out under the lock, then act.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};

use heapless::Deque;
use vdm_esp_core::common::VALVE_COUNT;
use vdm_esp_core::link_policy::LinkState;
use vdm_esp_core::stm_codec::Profile;
use vdm_esp_core::stm_flasher::FlashPhase;
use vdm_esp_core::stm_types::{StmCommand, StmSaveState, StmSnapshot};
use vdm_esp_core::version::StmSupport;

/// Depth of the queue into the stm thread (C++ `app::kCommandQueueDepth`).
pub const COMMAND_QUEUE_DEPTH: usize = 16;
/// Tasks whose stacks /api/health and the StackLow alarm watch (`app::MONITORED`).
pub const MONITORED_TASKS: usize = 6;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // a panic aborts the firmware, so a poisoned lock exists only in a failing test
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The scheduled calibration for /api/status and `diag/calibration/next` (C++ `app::CalibInfo`),
/// written by stm_service in the app thread.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CalibInfo {
    /// NVS `vdmrev/lastCal`: epoch of the last booked calibration, 0 = none.
    pub last_scheduled_epoch: i64,
    /// yyyymmdd of the next slot, 0 = none or no valid time.
    pub next_slot: u32,
    /// The next slot at `calib.hour:calib.minute` local time, 0 = none.
    pub next_epoch: i64,
}

/// The snapshot and the profiles, under one lock (C++ `gSnapMutex`).
struct Store {
    snapshot: Box<StmSnapshot>,
    /// The last gprof of every valve, kept once outside the snapshot copies.
    profiles: Box<[Profile; VALVE_COUNT as usize]>,
}

/// The data of C++ `app.h` that the tasks share.
pub struct AppShared {
    /// 16 commands (~1.6 KB): a boot block, like the C++ static queue storage.
    queue: Mutex<Box<Deque<StmCommand, COMMAND_QUEUE_DEPTH>>>,
    store: Mutex<Store>,
    link: AtomicU8,
    flash_active: AtomicBool,
    proto: AtomicU8,
    support: AtomicU8,
    revision: AtomicU32,
    save: AtomicU8,
    calib: Mutex<CalibInfo>,
    /// A library task of `app::MONITORED` was found by a resource sample.
    found: [AtomicBool; MONITORED_TASKS],
    /// The smallest largest free heap block of the resource samples, 0 before the first.
    min_largest: AtomicU32,
    /// The app thread ran its first pass: the boot finished (the boot deadline of
    /// GLUE-DESIGN-ESP.md 6.3).
    app_running: AtomicBool,
}

impl Default for AppShared {
    fn default() -> Self {
        Self::new()
    }
}

impl AppShared {
    /// An empty queue, an empty snapshot and profile store, link Unknown, save Idle.
    pub fn new() -> Self {
        AppShared {
            queue: Mutex::new(Box::default()),
            store: Mutex::new(Store {
                snapshot: Box::default(),
                profiles: Box::default(),
            }),
            link: AtomicU8::new(LinkState::Unknown as u8),
            flash_active: AtomicBool::new(false),
            proto: AtomicU8::new(0),
            support: AtomicU8::new(StmSupport::Unknown as u8),
            revision: AtomicU32::new(0),
            save: AtomicU8::new(StmSaveState::Idle as u8),
            calib: Mutex::new(CalibInfo::default()),
            found: Default::default(),
            min_largest: AtomicU32::new(0),
            app_running: AtomicBool::new(false),
        }
    }

    // ------------------------------------------------------------ commands

    /// Queues a command for the stm thread; never waits. False when the queue is full (the
    /// caller answers 503, rejects the MQTT command or retries). A request that needs several
    /// STM commands is one command (SetMotorSettings): web, mqtt and app submit concurrently, so
    /// free space checked before several submits would not stay free.
    pub fn submit(&self, cmd: &StmCommand) -> bool {
        lock(&self.queue).push_back(cmd.clone()).is_ok()
    }

    /// The stm thread: the oldest queued command; never waits.
    pub fn receive(&self) -> Option<StmCommand> {
        lock(&self.queue).pop_front()
    }

    // ------------------------------------------------------------ snapshot

    /// Copies the latest snapshot into `out` (~3 KB: callers keep it in a boot block).
    pub fn read_stm_snapshot(&self, out: &mut StmSnapshot) {
        out.clone_from(&lock(&self.store).snapshot);
    }

    /// Link state of the last snapshot (no copy).
    pub fn stm_link_state(&self) -> LinkState {
        LinkState::from_raw(self.link.load(Ordering::SeqCst)).unwrap_or_default()
    }

    /// The flasher runs (it owns the UART).
    pub fn stm_flash_active(&self) -> bool {
        self.flash_active.load(Ordering::SeqCst)
    }

    /// The stm thread, right after the flasher started: other tasks see it before the next
    /// snapshot.
    pub fn mark_stm_flash_active(&self) {
        self.flash_active.store(true, Ordering::SeqCst);
    }

    /// Changes whenever the stm thread publishes a snapshot.
    pub fn stm_snapshot_revision(&self) -> u32 {
        self.revision.load(Ordering::SeqCst)
    }

    /// Protocol of the STM: 0 unknown, 1..3.
    pub fn stm_protocol(&self) -> u8 {
        self.proto.load(Ordering::SeqCst)
    }

    /// Support level of the running STM firmware (`StmSnapshot::support`).
    pub fn stm_support(&self) -> StmSupport {
        StmSupport::from_raw(self.support.load(Ordering::SeqCst)).unwrap_or_default()
    }

    /// The stm thread publishes a snapshot: the copy first, then the cheap values, the revision
    /// last. The flash is active in every phase but Idle, Done and Failed.
    pub fn publish_stm_snapshot(&self, s: &StmSnapshot) {
        StmSnapshot::clone_from(&mut lock(&self.store).snapshot, s);
        self.link.store(s.link as u8, Ordering::SeqCst);
        let idle = matches!(
            s.flash.phase,
            FlashPhase::Idle | FlashPhase::Done | FlashPhase::Failed
        );
        self.flash_active.store(!idle, Ordering::SeqCst);
        self.proto.store(s.proto, Ordering::SeqCst);
        self.support.store(s.support as u8, Ordering::SeqCst);
        self.revision.store(s.revision, Ordering::SeqCst);
    }

    /// The stm thread: the last gprof of `p.valve` (a valve out of range is ignored).
    pub fn store_profile(&self, p: &Profile) {
        if let Some(slot) = lock(&self.store).profiles.get_mut(usize::from(p.valve)) {
            *slot = *p;
        }
    }

    /// Copies the profile of `valve` into `out`; count 0: none yet, or a valve out of range.
    pub fn read_profile(&self, valve: u8, out: &mut Profile) {
        *out = lock(&self.store)
            .profiles
            .get(usize::from(valve))
            .copied()
            .unwrap_or_default();
    }

    // ------------------------------------------------------------ restart

    /// A restart asks the stm thread for the STM EEPROM save (Waiting).
    pub fn request_stm_save(&self) {
        self.set_stm_save_state(StmSaveState::Waiting);
    }

    /// The STM EEPROM save of a restart.
    pub fn stm_save_state(&self) -> StmSaveState {
        StmSaveState::from_raw(self.save.load(Ordering::SeqCst)).unwrap_or_default()
    }

    /// The stm thread reports the outcome of the save.
    pub fn set_stm_save_state(&self, s: StmSaveState) {
        self.save.store(s as u8, Ordering::SeqCst);
    }

    // ------------------------------------------------------------ calibration

    /// The scheduled calibration for /api/status.
    pub fn calib_info(&self) -> CalibInfo {
        *lock(&self.calib)
    }

    /// stm_service publishes the scheduled calibration.
    pub fn set_calib_info(&self, c: &CalibInfo) {
        *lock(&self.calib) = *c;
    }

    // ------------------------------------------------------------ health

    /// A resource sample found the task at `slot` of `app::MONITORED` (it is never deleted).
    pub fn mark_task_found(&self, slot: usize) {
        if let Some(f) = self.found.get(slot) {
            f.store(true, Ordering::SeqCst);
        }
    }

    /// The task at `slot` was found by a resource sample.
    pub fn task_found(&self, slot: usize) -> bool {
        self.found
            .get(slot)
            .is_some_and(|f| f.load(Ordering::SeqCst))
    }

    /// The smallest largest free block of the resource samples (0 before the first).
    pub fn min_largest(&self) -> u32 {
        self.min_largest.load(Ordering::SeqCst)
    }

    /// The app thread after a resource sample.
    pub fn set_min_largest(&self, bytes: u32) {
        self.min_largest.store(bytes, Ordering::SeqCst);
    }

    // ------------------------------------------------------------ boot

    /// The app thread runs its passes (set by every pass; the boot deadline reads it).
    pub fn mark_app_running(&self) {
        self.app_running.store(true, Ordering::SeqCst);
    }

    /// The app thread ran a pass since the boot.
    pub fn app_running(&self) -> bool {
        self.app_running.load(Ordering::SeqCst)
    }
}

/// Every shared object of the firmware (GLUE-DESIGN-ESP.md 2.2): `main` creates one at boot
/// and leaks it (`&'static`), a test one per case. The modules borrow their parts; each part is
/// a boot block of its own.
pub struct Shared {
    /// C++ `app.h`: queue, snapshot, profiles, save state, calibration info, health.
    pub app: Box<AppShared>,
    /// The event ring, the sink configuration and statistics.
    pub logger: Box<crate::logger::LoggerShared>,
    /// LittleFS ready, the active config and its revision, the boot load.
    pub storage: Box<crate::storage::StorageShared>,
    /// Network state, health, trial and the inbound HTTP counter.
    pub net: Box<crate::net::NetShared>,
    /// The restart request, the OTA health and the upload flags.
    pub ota: Box<crate::ota::OtaShared>,
    /// The MQTT session, the regulator state and the requests of the web server.
    pub mqtt: Box<crate::mqtt_client::MqttShared>,
    /// The hand-over from the stm thread to stm_service.
    pub stm_service: Box<crate::stm_service::StmServiceShared>,
}

impl Default for Shared {
    fn default() -> Self {
        Self::new()
    }
}

impl Shared {
    /// Every part empty (the boot allocations of the parts happen here).
    pub fn new() -> Self {
        Shared {
            app: Box::default(),
            logger: Box::default(),
            storage: Box::default(),
            net: Box::default(),
            ota: Box::default(),
            mqtt: Box::default(),
            stm_service: Box::default(),
        }
    }
}

#[cfg(test)]
mod tests;
