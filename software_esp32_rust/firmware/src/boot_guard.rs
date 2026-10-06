//! Boot guard: the safety net of an OTA update that does not depend on the bootloader.
//!
//! The devices boot with the bootloader of their first serial flash (Arduino-ESP32, ESP-IDF 4.4
//! era), which has no app rollback: an image that fails boots again and again, and only a serial
//! flash at the device would end that. The guard runs first in `main`, before any driver:
//!
//! - It identifies the running image (the first 8 bytes of the SHA-256 appended to the image)
//!   and the otadata sequence number that selected it (0 when otadata did not select it: serial
//!   flash, or a bootloader fallback).
//! - The selection confirmed last on this device (NVS `vdmrs`/`good` = image + sequence) runs
//!   without a trial. So does an image whose other OTA slot holds no image that validates
//!   (a freshly serial-flashed device): there is nothing to go back to.
//! - Otherwise this boot is a trial attempt. The record {image, sequence, attempts} is kept twice:
//!   in RTC slow memory (survives resets and panics, garbage after power-on, CRC-checked) and in
//!   NVS `vdmrs`/`trial` (survives power loss); the higher count wins. The 4th boot of an
//!   unconfirmed selection, or 15 min of uptime without confirmation, switches back to the other
//!   slot (`esp_ota_set_boot_partition` validates that image first) and restarts.
//! - Confirmation: the health checks passed (spike: Ethernet has an IP and the HTTP server
//!   answered a request). The selection becomes `good`, the trial records are cleared.
//! - Switching back, automatic or by `POST /api/system/switch-back`, records the new selection of
//!   the other image as `good`: it was the running image when this one was uploaded, and two
//!   images that both wait for a network that is down must not take turns every 15 min.
//!
//! An OTA upload is refused while a trial runs: it would overwrite the image to go back to.
//!
//! NVS is opened without ever erasing it: it holds the operator's settings (namespace `vdmrev`
//! of the C++ firmware). Without NVS the guard counts in RTC memory only.

use core::ffi::CStr;
use core::mem::MaybeUninit;
use core::ptr;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use esp_idf_sys as sys;

/// Boots an unconfirmed selection gets; the next one switches back.
pub const MAX_ATTEMPTS: u32 = 3;
/// Uptime by which a selection on trial must be confirmed (feature `short-deadline`: 60 s, for
/// the QEMU harness).
pub const CONFIRM_WITHIN_S: u64 = if cfg!(feature = "short-deadline") { 60 } else { 15 * 60 };

const MAGIC: u32 = u32::from_le_bytes(*b"VDBG");
const NAMESPACE: &CStr = c"vdmrs";
const KEY_TRIAL: &CStr = c"trial";
const KEY_GOOD: &CStr = c"good";
const RECORD_LEN: usize = 24;

const OTA_STATE_INVALID: u32 = 3;
const OTA_STATE_ABORTED: u32 = 4;

/// What the guard decided at boot.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Mode {
    /// This selection was confirmed before.
    Confirmed = 0,
    /// No other slot with a valid image: runs without a trial.
    NoFallback = 1,
    /// Trial attempt; confirmation pending.
    Trial = 2,
    /// The trial was confirmed during this boot.
    ConfirmedNow = 3,
}

impl Mode {
    fn from_raw(v: u8) -> Self {
        match v {
            0 => Self::Confirmed,
            1 => Self::NoFallback,
            2 => Self::Trial,
            _ => Self::ConfirmedNow,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Confirmed => "confirmed",
            Self::NoFallback => "no-fallback",
            Self::Trial => "trial",
            Self::ConfirmedNow => "confirmed-now",
        }
    }
}

/// Image + otadata sequence of a boot selection, and its boot attempts.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Record {
    image: [u8; 8],
    ota_seq: u32,
    attempts: u32,
}

impl Record {
    fn encode(&self) -> [u8; RECORD_LEN] {
        let mut b = [0u8; RECORD_LEN];
        b[0..4].copy_from_slice(&MAGIC.to_le_bytes());
        b[4..12].copy_from_slice(&self.image);
        b[12..16].copy_from_slice(&self.ota_seq.to_le_bytes());
        b[16..20].copy_from_slice(&self.attempts.to_le_bytes());
        let crc = crc32_le(0, &b[0..20]);
        b[20..24].copy_from_slice(&crc.to_le_bytes());
        b
    }

    fn decode(b: &[u8]) -> Option<Self> {
        if b.len() != RECORD_LEN || b[0..4] != MAGIC.to_le_bytes() {
            return None;
        }
        let crc = u32::from_le_bytes([b[20], b[21], b[22], b[23]]);
        if crc != crc32_le(0, &b[0..20]) {
            return None;
        }
        let mut image = [0u8; 8];
        image.copy_from_slice(&b[4..12]);
        Some(Self {
            image,
            ota_seq: u32::from_le_bytes([b[12], b[13], b[14], b[15]]),
            attempts: u32::from_le_bytes([b[16], b[17], b[18], b[19]]),
        })
    }

    fn same_selection(&self, other: &Record) -> bool {
        self.image == other.image && self.ota_seq == other.ota_seq
    }
}

/// CRC-32 (IEEE, reflected) with zlib's chaining semantics: `crc32_le(0xFFFF_FFFF, ..)` is the
/// otadata entry CRC of ESP-IDF (`esp_rom_crc32_le(UINT32_MAX, &seq, 4)`).
fn crc32_le(crc: u32, data: &[u8]) -> u32 {
    let mut c = !crc;
    for &byte in data {
        c ^= u32::from(byte);
        for _ in 0..8 {
            c = if c & 1 != 0 { (c >> 1) ^ 0xEDB8_8320 } else { c >> 1 };
        }
    }
    !c
}

// RTC slow memory, not initialised by the startup code: survives software resets, panics and
// watchdog resets; random after power-on (the CRC rejects it).
#[link_section = ".rtc_noinit.vdm_boot_guard"]
static mut RTC_TRIAL: MaybeUninit<[u8; RECORD_LEN]> = MaybeUninit::uninit();

fn rtc_read() -> Option<Record> {
    // SAFETY: only the guard touches this static, from main before any thread exists and later
    // from the guard thread; volatile because the startup code does not initialise it.
    let b = unsafe { ptr::read_volatile(ptr::addr_of!(RTC_TRIAL)).assume_init() };
    Record::decode(&b)
}

fn rtc_write(rec: Option<&Record>) {
    let b = rec.map_or([0u8; RECORD_LEN], Record::encode);
    // SAFETY: see rtc_read.
    unsafe { ptr::write_volatile(ptr::addr_of_mut!(RTC_TRIAL), MaybeUninit::new(b)) };
}

/// NVS namespace `vdmrs`, opened per operation. `None` when NVS cannot be used.
struct Nvs(sys::nvs_handle_t);

impl Nvs {
    fn open(write: bool) -> Option<Self> {
        let mode = if write {
            sys::nvs_open_mode_t_NVS_READWRITE
        } else {
            sys::nvs_open_mode_t_NVS_READONLY
        };
        let mut h: sys::nvs_handle_t = 0;
        // SAFETY: valid C string and out pointer.
        let err = unsafe { sys::nvs_open(NAMESPACE.as_ptr(), mode, &mut h) };
        (err == sys::ESP_OK).then_some(Self(h))
    }

    fn get(&self, key: &CStr) -> Option<Record> {
        let mut b = [0u8; RECORD_LEN];
        let mut len = b.len();
        // SAFETY: buffer and length match.
        let err = unsafe { sys::nvs_get_blob(self.0, key.as_ptr(), b.as_mut_ptr().cast(), &mut len) };
        if err != sys::ESP_OK || len != RECORD_LEN {
            return None;
        }
        Record::decode(&b)
    }

    fn set(&self, key: &CStr, rec: &Record) -> bool {
        let b = rec.encode();
        // SAFETY: buffer and length match.
        let err = unsafe { sys::nvs_set_blob(self.0, key.as_ptr(), b.as_ptr().cast(), b.len()) };
        err == sys::ESP_OK && unsafe { sys::nvs_commit(self.0) } == sys::ESP_OK
    }

    fn erase(&self, key: &CStr) {
        // SAFETY: valid handle and key; a missing key is not an error here.
        unsafe {
            sys::nvs_erase_key(self.0, key.as_ptr());
            sys::nvs_commit(self.0);
        }
    }
}

impl Drop for Nvs {
    fn drop(&mut self) {
        // SAFETY: the handle came from nvs_open.
        unsafe { sys::nvs_close(self.0) };
    }
}

/// Initialises the default NVS partition and never erases it, whatever the error.
fn nvs_init() -> bool {
    // SAFETY: plain init call; repeated calls return ESP_OK.
    let err = unsafe { sys::nvs_flash_init() };
    if err != sys::ESP_OK {
        println!("boot guard: NVS unavailable (error {err}), counting in RTC memory only; NVS left as it is");
    }
    err == sys::ESP_OK
}

/// First 8 bytes of the image SHA-256 of an app partition.
fn image_id(part: *const sys::esp_partition_t) -> Option<[u8; 8]> {
    if part.is_null() {
        return None;
    }
    let mut sha = [0u8; 32];
    // SAFETY: valid partition pointer, 32-byte buffer.
    let err = unsafe { sys::esp_partition_get_sha256(part, sha.as_mut_ptr()) };
    if err != sys::ESP_OK {
        return None;
    }
    let mut id = [0u8; 8];
    id.copy_from_slice(&sha[..8]);
    Some(id)
}

/// The sequence number of the active otadata entry, when it selects `running`; else 0.
fn active_seq_for(running: *const sys::esp_partition_t) -> u32 {
    // SAFETY: lookups and reads of the otadata partition into local buffers.
    unsafe {
        let ota = sys::esp_partition_find_first(
            sys::esp_partition_type_t_ESP_PARTITION_TYPE_DATA,
            sys::esp_partition_subtype_t_ESP_PARTITION_SUBTYPE_DATA_OTA,
            ptr::null(),
        );
        if ota.is_null() || running.is_null() {
            return 0;
        }
        let mut best: Option<u32> = None;
        for sector in 0..2u32 {
            let mut e = [0u8; 32];
            if sys::esp_partition_read(ota, (sector * 0x1000) as usize, e.as_mut_ptr().cast(), 32)
                != sys::ESP_OK
            {
                continue;
            }
            let seq = u32::from_le_bytes([e[0], e[1], e[2], e[3]]);
            let state = u32::from_le_bytes([e[24], e[25], e[26], e[27]]);
            let crc = u32::from_le_bytes([e[28], e[29], e[30], e[31]]);
            let valid = seq != u32::MAX
                && crc == crc32_le(u32::MAX, &seq.to_le_bytes())
                && state != OTA_STATE_INVALID
                && state != OTA_STATE_ABORTED;
            if valid && best.is_none_or(|b| seq > b) {
                best = Some(seq);
            }
        }
        let Some(seq) = best else { return 0 };
        if seq == 0 {
            return 0;
        }
        // two OTA slots: sequence n selects ota_((n - 1) % 2)
        let slot = (seq - 1) % 2;
        let running_slot = (*running).subtype - sys::esp_partition_subtype_t_ESP_PARTITION_SUBTYPE_APP_OTA_MIN;
        if running_slot == slot { seq } else { 0 }
    }
}

/// The other OTA slot when it holds an image that validates (segments, checksum, SHA-256).
fn valid_other_slot(running: *const sys::esp_partition_t) -> Option<*const sys::esp_partition_t> {
    // SAFETY: partition lookups; the image check only reads flash.
    unsafe {
        let other = sys::esp_ota_get_next_update_partition(running);
        if other.is_null() || other == running {
            return None;
        }
        let pos = sys::esp_partition_pos_t { offset: (*other).address, size: (*other).size };
        let mut data: sys::esp_image_metadata_t = core::mem::zeroed();
        let err = sys::esp_image_verify(sys::esp_image_load_mode_t_ESP_IMAGE_VERIFY_SILENT, &pos, &mut data);
        (err == sys::ESP_OK).then_some(other)
    }
}

fn label(part: *const sys::esp_partition_t) -> String {
    if part.is_null() {
        return "?".into();
    }
    // SAFETY: the label is a NUL-terminated array inside the partition record.
    unsafe { CStr::from_ptr((*part).label.as_ptr()) }.to_string_lossy().into_owned()
}

fn hex(id: &[u8]) -> String {
    id.iter().map(|b| format!("{b:02x}")).collect()
}

static MODE: AtomicU8 = AtomicU8::new(Mode::NoFallback as u8);
static ATTEMPT: AtomicU32 = AtomicU32::new(0);
static NET_UP: AtomicBool = AtomicBool::new(false);
static HTTP_ANSWERED: AtomicBool = AtomicBool::new(false);
static SELECTION: Mutex<Option<Record>> = Mutex::new(None);

/// Runs the guard: counts this boot, switches back when the attempts are used up, and starts the
/// deadline watch of a trial. Call first in `main`.
pub fn start() {
    let nvs_ok = nvs_init();
    // SAFETY: plain lookup.
    let running = unsafe { sys::esp_ota_get_running_partition() };
    let Some(image) = image_id(running) else {
        println!("boot guard: running image not identified, guard off");
        return;
    };
    let ota_seq = active_seq_for(running);
    let current = Record { image, ota_seq, attempts: 0 };
    *SELECTION.lock().unwrap_or_else(|e| e.into_inner()) = Some(current);
    let id = hex(&image);
    let slot = label(running);

    let good = if nvs_ok { Nvs::open(false).and_then(|n| n.get(KEY_GOOD)) } else { None };
    if good.is_some_and(|g| g.same_selection(&current)) {
        rtc_write(None);
        MODE.store(Mode::Confirmed as u8, Ordering::SeqCst);
        println!("boot guard: {slot} image {id} seq {ota_seq} confirmed before");
        return;
    }
    let Some(other) = valid_other_slot(running) else {
        rtc_write(None);
        MODE.store(Mode::NoFallback as u8, Ordering::SeqCst);
        println!("boot guard: {slot} image {id} seq {ota_seq}: no valid image in the other slot, no trial");
        return;
    };

    let from_rtc = rtc_read().filter(|r| r.same_selection(&current)).map_or(0, |r| r.attempts);
    let from_nvs = if nvs_ok {
        Nvs::open(false).and_then(|n| n.get(KEY_TRIAL)).filter(|r| r.same_selection(&current)).map_or(0, |r| r.attempts)
    } else {
        0
    };
    let attempt = from_rtc.max(from_nvs).saturating_add(1);
    println!(
        "boot guard: {slot} image {id} seq {ota_seq} on trial, boot attempt {attempt} of {MAX_ATTEMPTS} (rtc {from_rtc}, nvs {from_nvs}), fallback {}",
        label(other)
    );
    if attempt > MAX_ATTEMPTS {
        switch_back_and_restart("boot attempts used up");
        // only returns when switching back failed: keep running (better than a boot loop)
    }
    let rec = Record { attempts: attempt, ..current };
    rtc_write(Some(&rec));
    if nvs_ok && !Nvs::open(true).is_some_and(|n| n.set(KEY_TRIAL, &rec)) {
        println!("boot guard: trial record not stored in NVS, RTC memory only");
    }
    ATTEMPT.store(attempt, Ordering::SeqCst);
    MODE.store(Mode::Trial as u8, Ordering::SeqCst);

    let spawned = std::thread::Builder::new()
        .name("boot_guard".into())
        .stack_size(6144)
        .spawn(watch);
    if spawned.is_err() {
        println!("boot guard: watch thread not started");
    }
}

/// Deadline and confirmation watch of a trial (own thread, 1 s period).
fn watch() {
    loop {
        std::thread::sleep(Duration::from_secs(1));
        if mode() != Mode::Trial {
            return;
        }
        if NET_UP.load(Ordering::SeqCst) && HTTP_ANSWERED.load(Ordering::SeqCst) {
            confirm();
            return;
        }
        if uptime_s() >= CONFIRM_WITHIN_S {
            switch_back_and_restart(&format!("not confirmed within {CONFIRM_WITHIN_S} s"));
            println!("boot guard: switching back failed, this image keeps running");
            return;
        }
    }
}

fn confirm() {
    let Some(current) = *SELECTION.lock().unwrap_or_else(|e| e.into_inner()) else { return };
    let stored = Nvs::open(true).is_some_and(|n| {
        let ok = n.set(KEY_GOOD, &current);
        n.erase(KEY_TRIAL);
        ok
    });
    rtc_write(None);
    // a bootloader with rollback support may have booted this image as PENDING_VERIFY
    // SAFETY: plain call; returns an error when there is nothing to mark.
    unsafe { sys::esp_ota_mark_app_valid_cancel_rollback() };
    MODE.store(Mode::ConfirmedNow as u8, Ordering::SeqCst);
    println!(
        "boot guard: image {} seq {} confirmed after {} s (net up, http answered){}",
        hex(&current.image),
        current.ota_seq,
        uptime_s(),
        if stored { "" } else { "; NVS record failed" }
    );
}

/// Selects the other slot for the next boot when its image validates, and records that
/// selection as confirmed. Returns the slot label.
pub fn select_other_slot(reason: &str) -> Result<String, String> {
    // SAFETY: partition lookups and otadata update through the ESP-IDF API.
    unsafe {
        let running = sys::esp_ota_get_running_partition();
        let other = sys::esp_ota_get_next_update_partition(running);
        if other.is_null() || other == running {
            return Err("no other slot".into());
        }
        // validates the image (esp_image_verify) before it writes otadata
        let err = sys::esp_ota_set_boot_partition(other);
        if err != sys::ESP_OK {
            return Err(format!("{} holds no valid image (error {err})", label(other)));
        }
        if let Some(image) = image_id(other) {
            let rec = Record { image, ota_seq: active_seq_for(other), attempts: 0 };
            let stored = Nvs::open(true).is_some_and(|n| {
                let ok = n.set(KEY_GOOD, &rec);
                n.erase(KEY_TRIAL);
                ok
            });
            println!(
                "boot guard: switching back to {} image {} seq {} ({reason}){}",
                label(other),
                hex(&image),
                rec.ota_seq,
                if stored { "" } else { "; NVS record failed" }
            );
        }
        rtc_write(None);
        Ok(label(other))
    }
}

fn switch_back_and_restart(reason: &str) {
    if select_other_slot(reason).is_ok() {
        // SAFETY: plain restart.
        unsafe { sys::esp_restart() };
    }
}

pub fn mode() -> Mode {
    Mode::from_raw(MODE.load(Ordering::SeqCst))
}

/// True while an unconfirmed image runs on trial (OTA uploads are refused).
pub fn on_trial() -> bool {
    mode() == Mode::Trial
}

/// Health evidence: the network interface has an IP.
pub fn note_net_up() {
    NET_UP.store(true, Ordering::SeqCst);
}

/// Health evidence: the HTTP server answered a request.
pub fn note_http_answered() {
    HTTP_ANSWERED.store(true, Ordering::SeqCst);
}

pub fn uptime_s() -> u64 {
    // SAFETY: plain call.
    let us = unsafe { sys::esp_timer_get_time() };
    u64::try_from(us).unwrap_or(0) / 1_000_000
}

/// The guard state for /api/health.
pub fn status_json() -> String {
    let sel = *SELECTION.lock().unwrap_or_else(|e| e.into_inner());
    let (image, seq) = sel.map_or((String::new(), 0), |s| (hex(&s.image), s.ota_seq));
    let m = mode();
    let left = if m == Mode::Trial { CONFIRM_WITHIN_S.saturating_sub(uptime_s()) } else { 0 };
    format!(
        "{{\"mode\":\"{}\",\"image\":\"{image}\",\"otaSeq\":{seq},\"attempt\":{},\"maxAttempts\":{MAX_ATTEMPTS},\"secondsLeft\":{left}}}",
        m.name(),
        ATTEMPT.load(Ordering::SeqCst)
    )
}
