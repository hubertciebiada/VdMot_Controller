//! The board and its boots (docs/rust/GLUE-DESIGN-ESP.md 5.3): [`FakeBoard`] keeps NVS, the
//! LittleFS tree, the OTA slots with otadata and the RTC block; [`FakeBoard::boot`] starts the
//! next boot after the reset that ended the last one and returns its [`Device`] (every port fake
//! of that boot); [`run`] catches the restart or crash that ends a boot.

use std::panic::{self, AssertUnwindSafe};
use std::sync::{Arc, Mutex, Once};

use super::clock::YieldLimit;
use super::fs::FsStore;
use super::nvs::NvsStore;
use super::ota::OtaStore;
use super::system::{Restarted, RTC_SIZE};
use super::uart::{FakeInputPin, FakeOutputPin};
use super::{
    lock, FakeClock, FakeConsole, FakeEthernet, FakeFs, FakeGpio, FakeHeap, FakeHttpServer,
    FakeMd5, FakeNvs, FakeOta, FakePinger, FakeRtc, FakeSntp, FakeSystem, FakeTcp, FakeUart,
    FakeUdp, FakeWall, FakeWatchdog, FakeWifi, Journal, SlotImage,
};
use crate::port::{AppId, Platform};

/// How a boot ended, i.e. how the next one starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Reset {
    /// Power cycle: the RTC block is garbage (0xA5).
    PowerOn,
    /// `esp_restart()`.
    Software,
    /// The EN pin.
    Pin,
    /// A panic (also an abort or an exception).
    Panic,
    /// The task watchdog.
    TaskWdt,
}

impl Reset {
    /// The `esp_reset_reason_t` number of the next boot.
    pub(crate) fn esp_reason(self) -> u8 {
        match self {
            Reset::PowerOn => 1, // ESP_RST_POWERON
            Reset::Pin => 2,     // ESP_RST_EXT
            Reset::Software => 3,
            Reset::Panic => 4,
            Reset::TaskWdt => 6,
        }
    }
}

/// Payload of [`FakeBoard::crash`].
#[derive(Debug)]
pub(crate) struct Crashed(pub(crate) Reset);

/// One boot as the bootloader started it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct BootRecord {
    pub(crate) reset: Reset,
    pub(crate) slot: usize,
    pub(crate) app: Option<AppId>,
    pub(crate) foreign: bool,
}

/// Boot bookkeeping of a board.
#[derive(Debug)]
pub(crate) struct BootState {
    /// How the running boot ended (`None` while it runs).
    pending: Option<Reset>,
    /// Every boot so far.
    pub(crate) history: Vec<BootRecord>,
}

impl BootState {
    /// Ends the running boot with `kind` (the first end counts).
    pub(crate) fn end(&mut self, kind: Reset) {
        self.pending.get_or_insert(kind);
    }
}

/// Boots of one case at most (the C++ runner's limit).
pub(crate) const MAX_BOOTS: usize = 8;

/// App ids of the images the cases install.
pub(crate) const APP_A: AppId = AppId(*b"app-A\0\0\0");
pub(crate) const APP_B: AppId = AppId(*b"app-B\0\0\0");
pub(crate) const APP_C: AppId = AppId(*b"app-C\0\0\0");
pub(crate) const APP_CPP: AppId = AppId(*b"cpp-2.1\0");

/// The persistent part of a device across simulated boots.
#[derive(Clone)]
pub(crate) struct FakeBoard {
    nvs: Arc<Mutex<NvsStore>>,
    fs: Arc<Mutex<FsStore>>,
    ota: Arc<Mutex<OtaStore>>,
    rtc: Arc<Mutex<Vec<u8>>>,
    boot: Arc<Mutex<BootState>>,
}

/// What the bootloader started.
pub(crate) enum Booted {
    /// A glue image: its device.
    Glue(Box<Device>),
    /// The C++ firmware: the case cannot follow it.
    Foreign(AppId),
    /// No slot loads.
    NoImage,
}

impl Default for FakeBoard {
    fn default() -> Self {
        Self::with_slots([SlotImage::glue(APP_A), SlotImage::EMPTY], 0)
    }
}

impl FakeBoard {
    /// A board fresh from the factory: glue image [`APP_A`] in slot 0, slot 1 empty.
    pub(crate) fn new() -> Self {
        Self::default()
    }
    /// A board with these slots and `otadata` selecting the boot slot.
    pub(crate) fn with_slots(slots: [SlotImage; 2], otadata: usize) -> Self {
        FakeBoard {
            nvs: Arc::default(),
            fs: Arc::default(),
            ota: Arc::new(Mutex::new(OtaStore {
                slots,
                otadata,
                running: otadata,
            })),
            rtc: Arc::new(Mutex::new(vec![0xA5; RTC_SIZE])),
            boot: Arc::new(Mutex::new(BootState {
                pending: Some(Reset::PowerOn),
                history: Vec::new(),
            })),
        }
    }
    /// Starts the next boot; it must be a glue image.
    pub(crate) fn boot(&self) -> Device {
        match self.boot_any() {
            Booted::Glue(d) => *d,
            Booted::Foreign(app) => panic!("the bootloader started the foreign image {app:?}"),
            Booted::NoImage => panic!("no slot holds a bootable image"),
        }
    }
    /// Starts the next boot after the reset that ended the last one.
    pub(crate) fn boot_any(&self) -> Booted {
        let reset = {
            let mut b = lock(&self.boot);
            let Some(reset) = b.pending.take() else {
                panic!("boot() while the last boot runs: end it with a restart, reset() or crash()")
            };
            assert!(b.history.len() < MAX_BOOTS, "more than {MAX_BOOTS} boots");
            reset
        };
        if reset == Reset::PowerOn {
            lock(&self.rtc).fill(0xA5);
        }
        let mut ota = lock(&self.ota);
        let slot = ota.bootloader();
        let record = BootRecord {
            reset,
            slot: slot.unwrap_or(ota.running),
            app: slot.and_then(|s| ota.slots[s].app),
            foreign: slot.is_some_and(|s| ota.slots[s].foreign),
        };
        let image = slot.map(|s| ota.slots[s]);
        drop(ota);
        lock(&self.boot).history.push(record);
        match image {
            None => Booted::NoImage,
            Some(i) if i.foreign => Booted::Foreign(i.app.unwrap_or(AppId([0; 8]))),
            Some(_) => Booted::Glue(Box::new(Device::new(self, reset))),
        }
    }
    /// Ends the running boot from outside (a power cut, the EN pin): the next boot starts with
    /// `kind`, also when the boot had ended already or none ran yet (a warm first boot).
    pub(crate) fn reset(&self, kind: Reset) {
        lock(&self.boot).pending = Some(kind);
    }
    /// Crashes the running boot: it ends with `kind`; [`run`] catches the panic.
    pub(crate) fn crash(&self, kind: Reset) -> ! {
        lock(&self.boot).end(kind);
        panic::panic_any(Crashed(kind))
    }
    /// Every boot so far.
    pub(crate) fn history(&self) -> Vec<BootRecord> {
        lock(&self.boot).history.clone()
    }
    /// The NVS store, outside a boot (setup and inspection).
    pub(crate) fn nvs(&self) -> FakeNvs {
        FakeNvs::new(self.nvs.clone(), Journal::default())
    }
    /// The LittleFS tree, outside a boot.
    pub(crate) fn fs(&self) -> FakeFs {
        FakeFs::new(self.fs.clone(), Journal::default())
    }
    /// The OTA slots, outside a boot.
    pub(crate) fn ota(&self) -> FakeOta {
        FakeOta::new(self.ota.clone(), Journal::default())
    }
    /// The RTC block.
    pub(crate) fn rtc(&self) -> FakeRtc {
        FakeRtc::new(self.rtc.clone())
    }
}

/// The port fakes of one boot.
pub(crate) struct Device {
    pub(crate) reset: Reset,
    pub(crate) journal: Journal,
    pub(crate) clock: FakeClock,
    pub(crate) wall: FakeWall,
    pub(crate) nvs: FakeNvs,
    pub(crate) fs: FakeFs,
    pub(crate) ota: FakeOta,
    pub(crate) rtc: FakeRtc,
    pub(crate) system: FakeSystem,
    pub(crate) heap: FakeHeap,
    pub(crate) console: FakeConsole,
    pub(crate) watchdog: FakeWatchdog,
    pub(crate) tcp: FakeTcp,
    pub(crate) udp: FakeUdp,
    pub(crate) uart: FakeUart,
    pub(crate) gpio: FakeGpio,
    pub(crate) eth: FakeEthernet,
    pub(crate) wifi: FakeWifi,
    pub(crate) sntp: FakeSntp,
    pub(crate) pinger: FakePinger,
    pub(crate) http: FakeHttpServer,
    pub(crate) md5: FakeMd5,
}

impl Device {
    fn new(board: &FakeBoard, reset: Reset) -> Self {
        let journal = Journal::default();
        let clock = FakeClock::default();
        Device {
            reset,
            wall: FakeWall::new(clock.clone()),
            nvs: FakeNvs::new(board.nvs.clone(), journal.clone()),
            fs: FakeFs::new(board.fs.clone(), journal.clone()),
            ota: FakeOta::new(board.ota.clone(), journal.clone()),
            rtc: FakeRtc::new(board.rtc.clone()),
            system: FakeSystem::new(reset, board.boot.clone(), journal.clone()),
            heap: FakeHeap::default(),
            console: FakeConsole::default(),
            watchdog: FakeWatchdog::default(),
            tcp: FakeTcp::new(clock.clone(), journal.clone()),
            udp: FakeUdp::default(),
            uart: FakeUart::new(clock.clone()),
            gpio: FakeGpio::new(clock.clone(), journal.clone()),
            eth: FakeEthernet::default(),
            wifi: FakeWifi::default(),
            sntp: FakeSntp::default(),
            pinger: FakePinger::default(),
            http: FakeHttpServer::default(),
            md5: FakeMd5::default(),
            clock,
            journal,
        }
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        if std::thread::panicking() {
            return;
        }
        assert_eq!(self.fs.open_handles(), 0, "a LittleFS file is still open");
        assert_eq!(
            self.nvs.knobs().open_handles,
            0,
            "an NVS handle is still open"
        );
    }
}

/// The port types of the tests.
pub(crate) struct TestPlatform;

impl Platform for TestPlatform {
    type Clock = FakeClock;
    type WallClock = FakeWall;
    type Watchdog = FakeWatchdog;
    type Console = FakeConsole;
    type Uart = FakeUart;
    type OutputPin = FakeOutputPin;
    type InputPin = FakeInputPin;
    type Nvs = FakeNvs;
    type Fs = FakeFs;
    type Tcp = FakeTcp;
    type Udp = FakeUdp;
    type HttpServer = FakeHttpServer;
    type Ota = FakeOta;
    type Md5 = FakeMd5;
    type System = FakeSystem;
    type HeapGate = FakeHeap;
    type Ethernet = FakeEthernet;
    type Wifi = FakeWifi;
    type Sntp = FakeSntp;
    type Pinger = FakePinger;
    type Rtc = FakeRtc;
}

/// How the code under [`run`] ended.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Ended<R> {
    /// It returned.
    Returned(R),
    /// A restart or a crash ended the boot; the board boots next with this reset.
    Reset(Reset),
    /// [`FakeClock::stop_after_sleeps`] ended a task loop.
    Stopped,
}

impl<R> Ended<R> {
    /// The returned value; panics on a reset.
    pub(crate) fn returned(self) -> R {
        match self {
            Ended::Returned(r) => r,
            Ended::Reset(k) => panic!("the boot ended with a reset ({k:?}) instead of returning"),
            Ended::Stopped => panic!("the task loop was stopped instead of returning"),
        }
    }
}

fn quiet_panics() {
    static HOOK: Once = Once::new();
    HOOK.call_once(|| {
        let default = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            let p = info.payload();
            if p.is::<Restarted>() || p.is::<Crashed>() || p.is::<YieldLimit>() {
                return;
            }
            default(info);
        }));
    });
}

/// Runs `f` like the device runs code that may restart: a restart or a crash ends the boot (the
/// board keeps how), a stopped task loop ends the run, every other panic goes on.
pub(crate) fn run<R>(f: impl FnOnce() -> R) -> Ended<R> {
    quiet_panics();
    match panic::catch_unwind(AssertUnwindSafe(f)) {
        Ok(r) => Ended::Returned(r),
        Err(p) => {
            if p.is::<Restarted>() {
                Ended::Reset(Reset::Software)
            } else if let Some(c) = p.downcast_ref::<Crashed>() {
                Ended::Reset(c.0)
            } else if p.is::<YieldLimit>() {
                Ended::Stopped
            } else {
                panic::resume_unwind(p)
            }
        }
    }
}
