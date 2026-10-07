//! Boot guard (docs/rust/GLUE-DESIGN-ESP.md section 6): the rollback of a new image in the
//! application, because the devices' bootloader has no rollback support (an image never reaches
//! PENDING_VERIFY). An image whose [`AppId`] is not the last confirmed one (NVS `otaOk`) runs on
//! trial: every boot before its confirmation is counted (NVS `otaTrial`, mirrored in RTC memory
//! for boots whose NVS write failed); above [`BOOT_LIMIT`] boots, or when the health checks of
//! D§16 give up, the guard selects the fallback slot and restarts. The confirmation, the health
//! checks and the manual switch back are driven by the `ota` glue through [`BootGuard`].
//!
//! With the choices the firmware spike proved in QEMU (`firmware/src/boot_guard.rs`): no trial
//! when the other slot holds no image that validates; a switch away from an image on trial makes
//! its fallback, which ran before that image was uploaded, the confirmed image before the restart
//! (two images that both wait for a network that is down must not take turns); a manual switch
//! away from a confirmed image confirms no image (its target may be the image that failed its
//! last trial, or one that never ran): the target boots on trial with this image as its
//! fallback; every confirmation also marks the image valid in otadata (for a bootloader with
//! rollback support); NVS is never erased, only the guard's keys are removed.
//!
//! Records (little endian, each with a CRC-32 over the bytes before it):
//! - `otaOk` (NVS `vdmrev`, blob 16 B): "VDOK", the `AppId` of the last confirmed image.
//! - `otaTrial` (NVS `vdmrev`, blob 32 B): "VDOT", version 1, state (1 trial, 2 switched back),
//!   boots, flags (bit0 STM link required, bit1 a switched-away image is recorded), the `AppId`
//!   on trial, the `AppId` the guard last switched away from, the fallback slot address.
//! - guard mirror (RTC block offset 0, 28 B): "VBGD", the `AppId` on trial, boots, the
//!   breadcrumb of the last switch (reason 1 boot limit, 2 health; 0 none), two zero bytes, the
//!   `AppId` switched away from.
//!
//! The C++ firmware ignores all three; an OTA between C++ and Rust loses the RTC mirror (its CRC
//! fails), which only restarts a boot count.

use crate::port::{AppId, Nvs, NvsInt, NvsNamespace, Ota, Rtc, SlotInfo, System};
use vdm_esp_core::config::crc32;

/// NVS namespace of the records.
pub const NVS_NAMESPACE: &str = "vdmrev";
/// NVS key of the confirmed image record.
pub const KEY_OK: &str = "otaOk";
/// NVS key of the trial record.
pub const KEY_TRIAL: &str = "otaTrial";
/// NVS key of the "STM link was up at the ESP upload" flag (u8 1), read and erased at boot.
pub const KEY_STM: &str = "otaStm";
/// Keys of the guard a Rust factory reset keeps (with storage's `frLatch`).
pub const FACTORY_RESET_KEEPS: [&str; 2] = [KEY_OK, KEY_TRIAL];
/// Unconfirmed boots an image gets; the next one switches to the fallback.
pub const BOOT_LIMIT: u8 = 3;
/// `setup` must reach the first app-task pass within this time after `main` starts, or the
/// firmware's one-shot timer restarts (a counted boot).
pub const BOOT_DEADLINE_MS: u32 = 60_000;
/// The `AppId` of an image built without the ELF SHA-256 in its app descriptor (`esptool
/// elf2image` without `--elf-sha256-offset`, as every legacy 1.4.x image): it names no build,
/// every such image would look like every other, so the guard treats a running image with it as
/// unknown and stays out (as without a readable description). The firmware build fails on it
/// (tools/rust/esp/app_id.py). A fallback is identified by its slot, so such an image (the legacy
/// firmware) stays a valid fallback.
pub const UNKNOWN_APP: AppId = AppId([0; 8]);
/// Place of the guard mirror in the RTC block.
pub const MIRROR_OFFSET: usize = 0;
/// Size of the guard mirror.
pub const MIRROR_LEN: usize = 28;

const OK_LEN: usize = 16;
const TRIAL_LEN: usize = 32;
const TRIAL_VERSION: u8 = 1;
const STATE_TRIAL: u8 = 1;
const STATE_SWITCHED_BACK: u8 = 2;
const FLAG_STM: u8 = 1;
const FLAG_AWAY: u8 = 2;

/// Why the guard left an image (the breadcrumb, event 107 arg2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum SwitchReason {
    /// More than [`BOOT_LIMIT`] unconfirmed boots.
    BootLimit = 1,
    /// 15 min without 120 s of health (D§16).
    Health = 2,
}

/// Who asks for the switch at run time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwitchCause {
    /// The health checks gave up (`OtaValidator` → Rollback).
    Health,
    /// `POST /api/system/ota/switch-back`.
    Manual,
}

/// What the guard has the logger record once it runs (event 107 `esp_ota_failed`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuardEvent {
    /// The previous image failed its trial: arg1 -4, arg2 the reason.
    PreviousImageFailed(SwitchReason),
    /// No switch possible (no valid fallback, or the fallback failed the last trial): arg1 -3,
    /// the image keeps running as confirmed.
    NoFallback,
}

impl GuardEvent {
    /// Event code (107).
    pub fn code(self) -> u16 {
        107
    }
    /// arg1 and arg2 of the event.
    pub fn args(self) -> (i32, i32) {
        match self {
            GuardEvent::PreviousImageFailed(r) => (-4, r as i32),
            GuardEvent::NoFallback => (-3, 0),
        }
    }
}

/// The running image's state for this boot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BootVerdict {
    /// Confirmed: no trial, the health checks do not run.
    Confirmed,
    /// On trial (`OtaValidator::begin(pending = true, stm_required)`).
    Trial {
        /// This boot's number in the trial (1..=BOOT_LIMIT).
        boots: u8,
        /// The STM link must be up to confirm (NVS `otaStm` at the upload).
        stm_required: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TrialRecord {
    state: u8,
    boots: u8,
    stm_required: bool,
    app: AppId,
    away: Option<AppId>,
    fallback: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Mirror {
    app: AppId,
    boots: u8,
    reason: u8,
    away: AppId,
}

fn put_crc(rec: &mut [u8]) {
    let n = rec.len() - 4;
    let c = crc32(&rec[..n], 0).to_le_bytes();
    rec[n..].copy_from_slice(&c);
}

fn crc_ok(rec: &[u8]) -> bool {
    let n = rec.len() - 4;
    rec[n..] == crc32(&rec[..n], 0).to_le_bytes()
}

fn app_at(rec: &[u8], at: usize) -> AppId {
    let mut id = [0u8; 8];
    id.copy_from_slice(&rec[at..at + 8]);
    AppId(id)
}

fn encode_ok(app: AppId) -> [u8; OK_LEN] {
    let mut r = [0u8; OK_LEN];
    r[..4].copy_from_slice(b"VDOK");
    r[4..12].copy_from_slice(&app.0);
    put_crc(&mut r);
    r
}

fn decode_ok(r: &[u8; OK_LEN]) -> Option<AppId> {
    (&r[..4] == b"VDOK" && crc_ok(r)).then(|| app_at(r, 4))
}

fn encode_trial(t: &TrialRecord) -> [u8; TRIAL_LEN] {
    let mut r = [0u8; TRIAL_LEN];
    r[..4].copy_from_slice(b"VDOT");
    r[4] = TRIAL_VERSION;
    r[5] = t.state;
    r[6] = t.boots;
    // two distinct bits: the sum is their OR
    r[7] = if t.stm_required { FLAG_STM } else { 0 } + if t.away.is_some() { FLAG_AWAY } else { 0 };
    r[8..16].copy_from_slice(&t.app.0);
    r[16..24].copy_from_slice(&t.away.unwrap_or(AppId([0; 8])).0);
    r[24..28].copy_from_slice(&t.fallback.to_le_bytes());
    put_crc(&mut r);
    r
}

fn decode_trial(r: &[u8; TRIAL_LEN]) -> Option<TrialRecord> {
    let state_ok = r[5] == STATE_TRIAL || r[5] == STATE_SWITCHED_BACK;
    if &r[..4] != b"VDOT" || r[4] != TRIAL_VERSION || !state_ok || !crc_ok(r) {
        return None;
    }
    Some(TrialRecord {
        state: r[5],
        boots: r[6],
        stm_required: r[7] & FLAG_STM != 0,
        app: app_at(r, 8),
        away: (r[7] & FLAG_AWAY != 0).then(|| app_at(r, 16)),
        fallback: u32::from_le_bytes([r[24], r[25], r[26], r[27]]),
    })
}

fn encode_mirror(m: &Mirror) -> [u8; MIRROR_LEN] {
    let mut r = [0u8; MIRROR_LEN];
    r[..4].copy_from_slice(b"VBGD");
    r[4..12].copy_from_slice(&m.app.0);
    r[12] = m.boots;
    r[13] = m.reason;
    r[16..24].copy_from_slice(&m.away.0);
    put_crc(&mut r);
    r
}

fn decode_mirror(r: &[u8; MIRROR_LEN]) -> Option<Mirror> {
    (&r[..4] == b"VBGD" && crc_ok(r)).then(|| Mirror {
        app: app_at(r, 4),
        boots: r[12],
        reason: r[13],
        away: app_at(r, 16),
    })
}

fn load_mirror(rtc: &impl Rtc) -> Option<Mirror> {
    let mut r = [0u8; MIRROR_LEN];
    rtc.load(MIRROR_OFFSET, &mut r);
    decode_mirror(&r)
}

fn store_mirror(rtc: &impl Rtc, m: &Mirror) {
    rtc.store(MIRROR_OFFSET, &encode_mirror(m));
}

fn clear_mirror(rtc: &impl Rtc) {
    rtc.store(MIRROR_OFFSET, &[0u8; MIRROR_LEN]);
}

fn read_ok(ns: &impl NvsNamespace) -> Option<AppId> {
    let mut r = [0u8; OK_LEN];
    (ns.get_blob(KEY_OK, &mut r)? == OK_LEN).then_some(())?;
    decode_ok(&r)
}

fn read_trial(ns: &impl NvsNamespace) -> Option<TrialRecord> {
    let mut r = [0u8; TRIAL_LEN];
    (ns.get_blob(KEY_TRIAL, &mut r)? == TRIAL_LEN).then_some(())?;
    decode_trial(&r)
}

/// The guard of this boot: the running image and its trial, for the run-time steps of 6.3.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BootGuard {
    app: Option<AppId>,
    trial: Option<TrialRecord>,
}

/// What [`BootGuard::boot`] decided.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BootReport {
    /// Confirmed or on trial.
    pub verdict: BootVerdict,
    /// Events for the logger, in order (at most two: a breadcrumb and a refused switch).
    pub events: [Option<GuardEvent>; 2],
}

impl BootReport {
    fn push(&mut self, e: GuardEvent) {
        if let Some(slot) = self.events.iter_mut().find(|s| s.is_none()) {
            *slot = Some(e);
        }
    }
}

/// What the boot decision found (an image that is not the confirmed one).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Inputs {
    app: AppId,
    old: Option<TrialRecord>,
    mirror: Option<Mirror>,
    stm_flag: bool,
    /// The other slot when its image validates.
    fallback: Option<SlotInfo>,
}

/// What the boot decision does for an image that is not the confirmed one (steps 2 to 6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Plan {
    /// Step 4: no switch possible: the image is confirmed.
    NoFallback,
    /// Step 5: over the boot limit: this record, then the switch to `target`.
    Switch {
        record: TrialRecord,
        target: Option<AppId>,
    },
    /// Step 6: the trial runs with this record.
    Trial(TrialRecord),
}

/// Steps 2 to 6 of 6.2.
fn plan(i: &Inputs) -> Plan {
    // steps 2 and 3: a new trial, or the next boot of a running one; the mirror counts the
    // boots whose NVS write failed (not after a breadcrumb: that trial ended with a switch)
    let running = i.old.filter(|t| t.app == i.app && t.state == STATE_TRIAL);
    let nvs_boots = running.map_or(0, |t| t.boots);
    let rtc_boots = i
        .mirror
        .filter(|m| m.app == i.app && m.reason == 0)
        .map_or(0, |m| m.boots);
    let mut record = TrialRecord {
        state: STATE_TRIAL,
        boots: nvs_boots.max(rtc_boots).saturating_add(1),
        stm_required: running.map_or(i.stm_flag, |t| t.stm_required),
        app: i.app,
        away: i.old.and_then(|t| t.away),
        fallback: 0,
    };
    // step 4: nothing that validates to go back to, or only the image that failed last
    let Some(fallback) = i.fallback.filter(|f| f.app != record.away) else {
        return Plan::NoFallback;
    };
    record.fallback = running.map_or(fallback.address, |t| t.fallback);
    if record.boots > BOOT_LIMIT {
        record.state = STATE_SWITCHED_BACK;
        record.away = Some(i.app);
        return Plan::Switch {
            record,
            target: fallback.app,
        };
    }
    Plan::Trial(record)
}

/// The breadcrumb of a switch away from another image, for the log.
fn breadcrumb(mirror: Option<Mirror>, app: AppId) -> Option<GuardEvent> {
    let m = mirror.filter(|m| m.away != app)?;
    [SwitchReason::BootLimit, SwitchReason::Health]
        .into_iter()
        .find(|r| *r as u8 == m.reason)
        .map(GuardEvent::PreviousImageFailed)
}

/// `otaStm`, read once and erased (a later boot of the same image does not see it).
fn take_stm_flag(ns: Option<&mut impl NvsNamespace>) -> bool {
    let Some(n) = ns else {
        return false;
    };
    let flag = n.get_int(KEY_STM, NvsInt::U8) == Some(1);
    n.remove(KEY_STM);
    flag
}

/// Writes the trial record and its mirror (`reason` 0: no breadcrumb).
fn write_trial(ns: Option<&mut impl NvsNamespace>, rtc: &impl Rtc, t: &TrialRecord, reason: u8) {
    if let Some(n) = ns {
        n.set_blob(KEY_TRIAL, &encode_trial(t));
    }
    store_mirror(
        rtc,
        &Mirror {
            app: t.app,
            boots: t.boots,
            reason,
            away: if reason == 0 { AppId([0; 8]) } else { t.app },
        },
    );
}

/// Selects the slot at `address` and restarts. `confirm`: the image there becomes the confirmed
/// one first (the fallback of a trial: it ran before the image on trial was uploaded, and two
/// images that both wait for a network that is down must not take turns); `None`: no image is
/// confirmed any more, so a glue image there boots on trial with this one as its fallback.
/// Returns only when the selection is refused.
fn switch_and_restart<NS: NvsNamespace>(
    mut ns: Option<NS>,
    ota: &impl Ota,
    system: &impl System,
    address: u32,
    confirm: Option<AppId>,
) -> Option<NS> {
    if ota.set_boot(address).is_err() {
        return ns;
    }
    if let Some(n) = ns.as_mut() {
        match confirm {
            Some(t) => n.set_blob(KEY_OK, &encode_ok(t)),
            None => n.remove(KEY_OK),
        };
    }
    drop(ns);
    system.restart()
}

impl BootGuard {
    /// The decision at boot (6.2), in `main` after the NRST release and the NVS init, before
    /// LittleFS, the network and the threads. A switch to the fallback restarts here and never
    /// returns. A failed NVS write never stops the boot: the RTC mirror still counts the boots
    /// across software, panic and watchdog resets.
    pub fn boot<N: Nvs, O: Ota, R: Rtc, S: System>(
        nvs: &N,
        ota: &O,
        rtc: &R,
        system: &S,
    ) -> (BootGuard, BootReport) {
        let mut report = BootReport {
            verdict: BootVerdict::Confirmed,
            events: [None; 2],
        };
        let mut ns = nvs.open(NVS_NAMESPACE, true);
        let stm_flag = take_stm_flag(ns.as_mut());
        let mut guard = BootGuard {
            app: ota.running().app.filter(|a| *a != UNKNOWN_APP),
            trial: None,
        };
        // no readable description of the running image, or one that names no build: the guard
        // stays out
        if let Some(app) = guard.app {
            guard.decide(ns, ota, rtc, system, app, stm_flag, &mut report);
        }
        (guard, report)
    }

    /// Steps 1 to 6 of 6.2 for the running image `app`.
    #[allow(clippy::too_many_arguments)]
    fn decide<NS: NvsNamespace>(
        &mut self,
        mut ns: Option<NS>,
        ota: &impl Ota,
        rtc: &impl Rtc,
        system: &impl System,
        app: AppId,
        stm_flag: bool,
        report: &mut BootReport,
    ) {
        let mirror = load_mirror(rtc);
        if let Some(e) = breadcrumb(mirror, app) {
            report.push(e);
        }
        let old = ns.as_ref().and_then(read_trial);
        // step 1: the confirmed image
        if ns.as_ref().and_then(read_ok) == Some(app) {
            if let (Some(n), Some(_)) = (ns.as_mut(), old) {
                n.remove(KEY_TRIAL);
            }
            clear_mirror(rtc);
            ota.mark_valid();
            return;
        }
        let inputs = Inputs {
            app,
            old,
            mirror,
            stm_flag,
            fallback: ota
                .other()
                .filter(|o| o.app.is_some() && ota.verify(o.address)),
        };
        self.apply(plan(&inputs), ns, ota, rtc, system, report);
    }

    /// Carries out the plan of steps 4 to 6.
    fn apply<NS: NvsNamespace>(
        &mut self,
        plan: Plan,
        mut ns: Option<NS>,
        ota: &impl Ota,
        rtc: &impl Rtc,
        system: &impl System,
        report: &mut BootReport,
    ) {
        match plan {
            Plan::Trial(record) => {
                write_trial(ns.as_mut(), rtc, &record, 0);
                self.trial = Some(record);
                report.verdict = self.verdict();
                return;
            }
            Plan::Switch { record, target } => {
                write_trial(ns.as_mut(), rtc, &record, SwitchReason::BootLimit as u8);
                // returns only when the fallback does not verify after all: keep running this one
                ns = switch_and_restart(ns, ota, system, record.fallback, target);
            }
            Plan::NoFallback => {}
        }
        if let Some(app) = self.app {
            self.confirm(ns.as_mut(), ota, rtc, app);
        }
        report.push(GuardEvent::NoFallback);
    }

    /// Ends a trial as confirmed: `otaOk` := the running image, `otaTrial` removed, RTC mirror
    /// cleared, the image marked valid in otadata.
    fn confirm<NS: NvsNamespace>(
        &mut self,
        ns: Option<&mut NS>,
        ota: &impl Ota,
        rtc: &impl Rtc,
        app: AppId,
    ) {
        if let Some(n) = ns {
            n.set_blob(KEY_OK, &encode_ok(app));
            n.remove(KEY_TRIAL);
        }
        clear_mirror(rtc);
        ota.mark_valid();
        self.trial = None;
    }

    /// The running image is on trial (ESP uploads are refused: `409 upload_failed "image on
    /// trial"`, the upload would overwrite the fallback).
    pub fn on_trial(&self) -> bool {
        self.trial.is_some()
    }

    /// The trial's verdict now (after a confirmation: Confirmed).
    pub fn verdict(&self) -> BootVerdict {
        match self.trial {
            Some(t) => BootVerdict::Trial {
                boots: t.boots,
                stm_required: t.stm_required,
            },
            None => BootVerdict::Confirmed,
        }
    }

    /// `OtaValidator` → MarkValid (120 s healthy), or a user restart that confirms first: the
    /// running image becomes the confirmed one (the caller logs event 108). No effect without a
    /// trial.
    pub fn mark_valid<N: Nvs, O: Ota, R: Rtc>(&mut self, nvs: &N, ota: &O, rtc: &R) {
        let (Some(app), Some(_)) = (self.app, self.trial) else {
            return;
        };
        let mut ns = nvs.open(NVS_NAMESPACE, true);
        self.confirm(ns.as_mut(), ota, rtc, app);
    }

    /// The restart into an uploaded image (restart reason 1): no image stays confirmed. The
    /// uploaded image runs on trial by its own `AppId` anyway; this image, should another
    /// firmware (the C++ one) run in between and install it again, proves itself on trial
    /// instead of trusting a confirmation from before that firmware ran.
    pub fn leave_for_upload<N: Nvs>(&self, nvs: &N) {
        if let Some(mut ns) = nvs.open(NVS_NAMESPACE, true) {
            ns.remove(KEY_OK);
        }
    }

    /// A switch is possible at all: the other slot holds an app (`409 no_fallback` otherwise).
    pub fn fallback_available<O: Ota>(ota: &O) -> bool {
        ota.other().and_then(|o| o.app).is_some()
    }

    /// The switch at the end of the restart path (6.3): during a trial `otaTrial` becomes
    /// switched back (after the health checks with the breadcrumb, reason 2), then the other slot
    /// is selected, its image (the trial's fallback) becomes the confirmed one and the chip
    /// restarts. From a confirmed image (only the manual switch) the confirmation is removed
    /// instead: the other image may be the one that failed its last trial or one that never ran,
    /// so it boots on trial with this image as its fallback. Returns only when the other slot
    /// cannot be selected: the image keeps running, a trial ends as confirmed, and the caller
    /// logs the returned event (107 arg1 -3).
    pub fn switch_to_fallback<N: Nvs, O: Ota, R: Rtc, S: System>(
        &mut self,
        nvs: &N,
        ota: &O,
        rtc: &R,
        system: &S,
        cause: SwitchCause,
    ) -> GuardEvent {
        let mut ns = nvs.open(NVS_NAMESPACE, true);
        let other = ota.other();
        if let (Some(app), Some(mut t)) = (self.app, self.trial) {
            t.state = STATE_SWITCHED_BACK;
            let reason = match cause {
                SwitchCause::Health => {
                    t.away = Some(app);
                    SwitchReason::Health as u8
                }
                SwitchCause::Manual => 0,
            };
            if reason == 0 {
                if let Some(n) = ns.as_mut() {
                    n.set_blob(KEY_TRIAL, &encode_trial(&t));
                }
                clear_mirror(rtc);
            } else {
                write_trial(ns.as_mut(), rtc, &t, reason);
            }
        }
        let address = self.trial.map(|t| t.fallback).or(other.map(|o| o.address));
        // only a trial's fallback ran before this image: from a confirmed image nothing is
        // confirmed, the target proves itself on trial
        let confirm = self.trial.and(other).and_then(|o| o.app);
        if let Some(a) = address {
            ns = switch_and_restart(ns, ota, system, a, confirm);
        }
        if let (Some(app), Some(_)) = (self.app, self.trial) {
            self.confirm(ns.as_mut(), ota, rtc, app);
        }
        GuardEvent::NoFallback
    }
}

#[cfg(test)]
mod tests;
