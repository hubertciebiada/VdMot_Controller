//! STM link task (C++ `stm_link.cpp`, `stm_link.h`): sole owner of Serial2 and the NRST pin.
//! Runs core [`StmSession`] (link policy, planner, models, lease, reset gate, flasher), implements
//! its port, executes the commands of the queue and publishes the STM snapshot. Nothing else in
//! the firmware touches the UART (one owner thread, message passing).
//!
//! The session owns its port; the port reaches the other glue modules through [`StmLinkHost`]
//! (one method per C++ call), the NRST pin and the LittleFS image of a flash run. The UART stays
//! with [`StmLink`]: the request lines between flash runs, and the flash transport of
//! [`StmSession::flash_step`]. NRST is shared by the port (the session's NRST pulse) and the
//! flash transport (the flasher's pulses), behind a mutex only the stm thread locks.
//!
//! The thread runs [`StmLink::start`] (C++ the task's start), then [`StmLink::pass`] every
//! [`PASS_DELAY_MS`] (`app::run_task`). Port form: the session reads the config under the
//! config lock ([`StmLinkHost::with_config`]); the C++ heap copy of 2.5 KB and the wait for it
//! at the start are gone (docs/rust/GLUE-DESIGN-ESP.md 2.4).

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use vdm_esp_core::calib_schedule::CalibFailure;
use vdm_esp_core::common::elapsed_ms;
use vdm_esp_core::config::Config;
use vdm_esp_core::event_log::Event;
use vdm_esp_core::failsafe::RegulatorInput;
use vdm_esp_core::lease_client::LeaseClientSnapshot;
use vdm_esp_core::stm_codec::Profile;
use vdm_esp_core::stm_flasher::{FlashImage, FlashTransport};
use vdm_esp_core::stm_session::{StmSession, StmSessionPort};
use vdm_esp_core::stm_types::{StmCommand, StmSaveState, StmSnapshot};
use vdm_esp_core::target_store::{PersistedTargets, RestoreSource};

use crate::board::{STM_BAUD, STM_RESET_ASSERTED_LEVEL};
use crate::port::{Clock, OutputPin, Platform, Uart, Watchdog};
use crate::storage::{FileImage, LoadSource};

/// Delay between two passes of the thread (C++ `vTaskDelay(pdMS_TO_TICKS(2))`).
pub const PASS_DELAY_MS: u32 = 2;
/// Commands one pass takes from the queue.
pub const COMMANDS_PER_PASS: usize = 4;
/// One UART read ...
pub const READ_CHUNK: usize = 128;
/// ... and at most this many per pass (512 B) before the thread yields.
pub const READ_CHUNKS_PER_PASS: usize = 4;
/// The NRST pulse.
pub const RESET_PULSE_MS: u32 = 100;
/// The flasher's input discard: at most this many reads ...
pub const DISCARD_READS: usize = 64;
/// ... of this many bytes.
pub const DISCARD_CHUNK: usize = 64;
/// Period of the session's once-a-second work.
pub const SECOND_MS: u32 = 1000;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // a panic aborts the firmware, so a poisoned lock exists only in a failing test
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What the stm thread calls in the other glue modules (C++ `logger::`, `app::`, `storage::`,
/// `ota::`, `stm_service::` and `mqtt::`), one method per C++ call; the firmware wiring
/// implements it. The session's port and the task each hold a clone.
pub trait StmLinkHost {
    /// C++ `logger::log(e)`.
    fn log_event(&mut self, e: &Event);
    /// C++ `app::publishStmSnapshot(s)`.
    fn publish(&mut self, s: &StmSnapshot);
    /// C++ `app::storeProfile(p)`: the profile store keeps the last gprof of the valve.
    fn store_profile(&mut self, p: &Profile);
    /// C++ `storage::requestLastGoodCopy(image)`: the flashed image becomes the last good one.
    fn request_last_good_copy(&mut self, image: &[u8]);
    /// C++ `ota::restartPending()`.
    fn restart_pending(&mut self) -> bool;
    /// C++ `app::markStmFlashActive()`.
    fn mark_flash_active(&mut self);
    /// C++ `stm_service::storeDesiredTargets(t)`: the RTC copy and the hand-over to the NVS
    /// saver.
    fn store_desired_targets(&mut self, t: &PersistedTargets);
    /// C++ `stm_service::postScheduledCalibResult(attempt, ok, reason)`.
    fn post_scheduled_calib_result(&mut self, attempt: u16, ok: bool, reason: CalibFailure);
    /// C++ `app::setStmSaveState(s)`.
    fn set_stm_save_state(&mut self, s: StmSaveState);
    /// C++ `stm_service::storeLeaseRecord(s)`: the RTC record of the ESP failsafe emulation.
    fn store_lease_record(&mut self, s: &LeaseClientSnapshot);
    /// C++ `storage::configRevision()`.
    fn config_revision(&mut self) -> u32;
    /// C++ `storage::getConfig()`: calls `f` with the active config (under the config lock).
    fn with_config(&mut self, f: &mut dyn FnMut(&Config));
    /// C++ `storage::bootLoadSource()`.
    fn boot_load_source(&mut self) -> LoadSource;
    /// C++ `storage::configSavedSinceBoot()`.
    fn config_saved_since_boot(&mut self) -> bool;
    /// C++ `stm_service::bootTargets(src)`.
    fn boot_targets(&mut self) -> (PersistedTargets, RestoreSource);
    /// C++ `stm_service::bootLease(out)`: `None` without a valid RTC record.
    fn boot_lease(&mut self) -> Option<LeaseClientSnapshot>;
    /// C++ `app::receive(out)`: the next queued command; never waits.
    fn receive(&mut self) -> Option<StmCommand>;
    /// C++ `mqtt::regulatorState()`.
    fn regulator_state(&mut self) -> RegulatorInput;
    /// C++ `app::stmSaveState()`.
    fn stm_save_state(&mut self) -> StmSaveState;
}

/// Drives NRST: `asserted` holds the STM in reset (IO15 HIGH through the inverting transistor).
fn set_reset<O: OutputPin>(nrst: &Mutex<O>, asserted: bool) {
    lock(nrst).set(asserted == STM_RESET_ASSERTED_LEVEL);
}

/// BOOT0 LOW, then NRST released (IO15 LOW), on the bare pins. The IO15 strap pull-up holds the
/// STM in reset from the ESP reset until this runs: `main` calls it first, before the link and
/// before anything that can fail (GLUE-DESIGN-ESP.md 6.4 step 1). BOOT0 goes LOW first, so a
/// wired BOOT0 never starts the ROM bootloader.
pub fn release_stm_reset<O: OutputPin>(boot0: &mut O, nrst: &mut O) {
    boot0.set(false);
    nrst.set(!STM_RESET_ASSERTED_LEVEL);
}

/// NRST asserted for [`RESET_PULSE_MS`], then released.
fn pulse<O: OutputPin>(nrst: &Mutex<O>, clock: &impl Clock) {
    set_reset(nrst, true);
    clock.sleep_ms(RESET_PULSE_MS);
    set_reset(nrst, false);
}

/// The port of the session (C++ `Port`).
pub struct SessionPort<'a, P: Platform, H> {
    host: H,
    clock: &'a P::Clock,
    nrst: Arc<Mutex<P::OutputPin>>,
    image: FileImage<&'a P::Fs, &'a P::HeapGate>,
}

impl<P: Platform, H: StmLinkHost> StmSessionPort for SessionPort<'_, P, H> {
    fn log_event(&mut self, e: &Event) {
        self.host.log_event(e);
    }
    fn pulse_reset(&mut self, _now_ms: u32) -> u32 {
        pulse(&self.nrst, self.clock);
        self.clock.now_ms()
    }
    fn publish(&mut self, s: &StmSnapshot) {
        self.host.publish(s);
    }
    fn store_profile(&mut self, p: &Profile) {
        self.host.store_profile(p);
    }
    fn request_last_good_copy(&mut self, image: &[u8]) {
        self.host.request_last_good_copy(image);
    }
    fn restart_pending(&mut self) -> bool {
        self.host.restart_pending()
    }
    fn mark_flash_active(&mut self) {
        self.host.mark_flash_active();
    }
    fn store_desired_targets(&mut self, t: &PersistedTargets) {
        self.host.store_desired_targets(t);
    }
    fn post_scheduled_calib_result(&mut self, attempt: u16, ok: bool, reason: CalibFailure) {
        self.host.post_scheduled_calib_result(attempt, ok, reason);
    }
    fn set_stm_save_state(&mut self, s: StmSaveState) {
        self.host.set_stm_save_state(s);
    }
    fn store_lease_record(&mut self, s: &LeaseClientSnapshot) {
        self.host.store_lease_record(s);
    }
    fn open_image(&mut self, name: &[u8]) -> bool {
        self.image.open(name)
    }
    fn image(&mut self) -> &mut dyn FlashImage {
        &mut self.image
    }
    fn close_image(&mut self) {
        self.image.close();
    }
}

/// The flasher's view of the UART and NRST (C++ `UartTransport`).
struct UartTransport<'x, U, O> {
    uart: &'x mut U,
    nrst: &'x Mutex<O>,
}

impl<U: Uart, O: OutputPin> FlashTransport for UartTransport<'_, U, O> {
    fn configure(&mut self, baud: u32, even_parity: bool) {
        self.uart.configure(baud, even_parity);
    }
    /// The flasher writes at most one frame (<= 260 B) per step after the previous one was
    /// ACKed, so the 512 B TX ring never blocks here.
    fn write(&mut self, data: &[u8]) -> usize {
        self.uart.write(data)
    }
    fn read(&mut self, out: &mut [u8]) -> usize {
        self.uart.read(out)
    }
    /// At most [`DISCARD_READS`] reads of [`DISCARD_CHUNK`] bytes, until one finds nothing (C++:
    /// while `available()`; the port's read never blocks, an empty read ends it).
    fn discard_input(&mut self) {
        let mut buf = [0u8; DISCARD_CHUNK];
        for _ in 0..DISCARD_READS {
            if self.uart.read(&mut buf) == 0 {
                break;
            }
        }
    }
    fn set_reset(&mut self, asserted: bool) {
        set_reset(self.nrst, asserted);
    }
}

/// The ports of the stm thread: the UART, NRST (IO15) and BOOT0 (IO14) are its own.
pub struct StmLinkPorts<'a, P: Platform> {
    pub clock: &'a P::Clock,
    /// The STM images of a flash run.
    pub fs: &'a P::Fs,
    /// The sector 0 a flash run holds in RAM (D9, 16 KiB while the run lasts).
    pub heap: &'a P::HeapGate,
    pub uart: P::Uart,
    pub nrst: P::OutputPin,
    pub boot0: P::OutputPin,
}

type Session<'a, P, H> = StmSession<SessionPort<'a, P, H>>;

/// The stm thread (C++ `stm_link::begin`, `stm_link::task` and their statics).
pub struct StmLink<'a, P: Platform, H: StmLinkHost + Clone> {
    clock: &'a P::Clock,
    uart: P::Uart,
    nrst: Arc<Mutex<P::OutputPin>>,
    boot0: P::OutputPin,
    host: H,
    session: Box<Session<'a, P, H>>,
    cfg_revision: u32,
    last_second_ms: u32,
    watchdog: Option<P::Watchdog>,
}

/// The session, a boot block (about 14 KB on the 64-bit host). `StmSession::new` returns it by
/// value, so without the in-place construction of an optimised build it passes the stack once:
/// only at boot, on the stack of `main` (freed when `main` returns), which must hold it. The C++
/// constructed it in place on the heap.
fn new_session<'a, P: Platform, H: StmLinkHost>(
    port: SessionPort<'a, P, H>,
) -> Box<Session<'a, P, H>> {
    Box::new(StmSession::new(port))
}

impl<'a, P: Platform, H: StmLinkHost + Clone> StmLink<'a, P, H> {
    /// The link with its session (a boot block); nothing is driven yet ([`StmLink::begin`]).
    pub fn new(ports: StmLinkPorts<'a, P>, host: H) -> Self {
        let nrst = Arc::new(Mutex::new(ports.nrst));
        let port = SessionPort {
            host: host.clone(),
            clock: ports.clock,
            nrst: nrst.clone(),
            image: FileImage::new(ports.fs, ports.heap),
        };
        StmLink {
            clock: ports.clock,
            uart: ports.uart,
            nrst,
            boot0: ports.boot0,
            host,
            session: new_session(port),
            cfg_revision: 0,
            last_second_ms: 0,
            watchdog: None,
        }
    }

    /// [`release_stm_reset`] on the link's pins.
    pub fn release_reset(&mut self) {
        release_stm_reset(&mut self.boot0, &mut *lock(&self.nrst));
    }

    /// `app::setup`: NRST released again (R6: the firmware never resets the STM at its own boot;
    /// doing it again is harmless and covers a missing early release) and Serial2 opened at
    /// 115200 8N1.
    pub fn begin(&mut self) {
        self.release_reset();
        self.uart.configure(STM_BAUD, false);
    }

    /// Pulses NRST for [`RESET_PULSE_MS`] (the session asks for it through its port).
    pub fn pulse_reset(&mut self) {
        pulse(&self.nrst, self.clock);
    }

    /// The session (for the tests and the wiring).
    pub fn session(&self) -> &StmSession<SessionPort<'a, P, H>> {
        &self.session
    }

    /// The config into the session. Defaults nobody saved since the boot are not trusted: they
    /// must not overwrite the failsafe settings the STM holds.
    fn reload_config(&mut self) {
        self.cfg_revision = self.host.config_revision();
        let defaults = matches!(
            self.host.boot_load_source(),
            LoadSource::Defaults | LoadSource::DefaultsAfterError
        );
        let trusted = !defaults || self.host.config_saved_since_boot();
        let session = &mut self.session;
        self.host
            .with_config(&mut |c| session.apply_config(c, trusted));
    }

    /// The start of the thread (C++ the task's start): its watchdog, the config, then the
    /// session begins with the boot targets and the lease record of stm_service.
    pub fn start(&mut self, watchdog: P::Watchdog) {
        self.watchdog = Some(watchdog);
        self.reload_config();
        let start = self.clock.now_ms();
        let (targets, source) = self.host.boot_targets();
        let lease = self.host.boot_lease();
        self.session.begin(start, &targets, source, lease.as_ref());
        self.last_second_ms = start;
    }

    /// At most [`READ_CHUNKS_PER_PASS`] reads of [`READ_CHUNK`] bytes into the session.
    fn read_uart(&mut self, now_ms: u32) {
        let mut buf = [0u8; READ_CHUNK];
        for _ in 0..READ_CHUNKS_PER_PASS {
            let n = self.uart.read(&mut buf);
            if n == 0 {
                return;
            }
            self.session.on_rx(buf.get(..n).unwrap_or_default(), now_ms);
        }
    }

    /// One pass of the thread; returns the delay before the next one ([`PASS_DELAY_MS`]):
    /// 1. the watchdog, a changed config,
    /// 2. at most [`COMMANDS_PER_PASS`] commands into the session,
    /// 3. at most 512 UART bytes into the session, `poll`, the next request line onto the UART;
    ///    while flashing a flasher step instead,
    /// 4. once per second `every_second` with the regulator state of MQTT and the STM save
    ///    state of a pending ESP restart,
    /// 5. `publish_if_due`.
    pub fn pass(&mut self) -> u32 {
        if let Some(w) = &self.watchdog {
            w.feed();
        }
        let now = self.clock.now_ms();
        if self.host.config_revision() != self.cfg_revision {
            self.reload_config();
        }
        for _ in 0..COMMANDS_PER_PASS {
            let Some(cmd) = self.host.receive() else {
                break;
            };
            self.session.handle_command(&cmd, now);
        }
        if self.session.flashing() {
            let mut t = UartTransport {
                uart: &mut self.uart,
                nrst: &self.nrst,
            };
            self.session.flash_step(&mut t, now);
        } else {
            self.read_uart(now);
            self.session.poll(now);
            let sent = match self.session.next_to_send(now) {
                Some(line) => {
                    self.uart.write(&line.text);
                    true
                }
                None => false,
            };
            if sent {
                self.session.on_sent(self.clock.now_ms());
            }
        }
        if elapsed_ms(now, self.last_second_ms) >= SECOND_MS {
            self.last_second_ms = now;
            let regulator = self.host.regulator_state();
            let save = self.host.stm_save_state();
            self.session.every_second(now, &regulator, save);
        }
        self.session.publish_if_due(now);
        PASS_DELAY_MS
    }
}

#[cfg(test)]
mod rig;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_mut;
