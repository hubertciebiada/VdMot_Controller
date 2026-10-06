//! ESP OTA policy: decides when a freshly flashed image is healthy enough to be marked valid,
//! and when it is rolled back (port of `vdm/ota_policy.h`). Hardware-free.

use crate::common::{c_str, elapsed_ms};

/// [`OtaValidator::update`] (C++ `OtaValidator::Decision`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OtaValidatorDecision {
    NotPending,
    Wait,
    MarkValid,
    Rollback,
}

/// OTA rollback confirmation. A freshly flashed image is marked valid after `confirm_ms` of
/// uninterrupted health:
///  - net  - the network is up and proven end to end (NetReachability, net_policy),
///  - http - the loopback self-check GET /api/health answered 200 within HTTP_FRESH_MS,
///  - stm  - the STM link is Up; counted only when stm_required (the link was Up when the image
///    was uploaded).
///
/// If that never happened by `give_up_ms` of uptime, the image is rolled back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OtaValidator {
    confirm_ms: u32,
    give_up_ms: u32,
    pending: bool,
    stm_required: bool,
    start_ms: u32,
    healthy: bool,
    healthy_since_ms: u32,
    checked: bool,
    check_ok: bool,
    check_ms: u32,
    missing: u8,
}

impl Default for OtaValidator {
    /// confirm 120 s, give up after 900 s (the C++ default arguments).
    fn default() -> Self {
        Self::new(120_000, 900_000)
    }
}

impl OtaValidator {
    pub const CHECK_NET: u8 = 1;
    pub const CHECK_HTTP: u8 = 2;
    pub const CHECK_STM: u8 = 4;
    pub const SELF_CHECK_INTERVAL_MS: u32 = 10_000;
    pub const HTTP_FRESH_MS: u32 = 30_000;

    pub fn new(confirm_ms: u32, give_up_ms: u32) -> Self {
        Self {
            confirm_ms,
            give_up_ms,
            pending: false,
            stm_required: false,
            start_ms: 0,
            healthy: false,
            healthy_since_ms: 0,
            checked: false,
            check_ok: false,
            check_ms: 0,
            missing: 0,
        }
    }

    /// `pending_verify`: esp_ota_get_state_partition() == ESP_OTA_IMG_PENDING_VERIFY. A second
    /// call re-arms everything.
    pub fn begin(&mut self, pending_verify: bool, stm_required: bool, now_ms: u32) {
        self.pending = pending_verify;
        self.stm_required = stm_required;
        self.start_ms = now_ms;
        self.healthy = false;
        self.healthy_since_ms = now_ms;
        self.checked = false;
        self.check_ok = false;
        self.check_ms = now_ms;
        self.missing = 0;
    }

    /// Due while pending and the web server runs: at once, then every SELF_CHECK_INTERVAL_MS.
    pub fn self_check_due(&self, web_started: bool, now_ms: u32) -> bool {
        if !self.pending || !web_started {
            return false;
        }
        !self.checked || elapsed_ms(now_ms, self.check_ms) >= Self::SELF_CHECK_INTERVAL_MS
    }

    pub fn on_self_check(&mut self, ok: bool, now_ms: u32) {
        self.checked = true;
        self.check_ok = ok;
        self.check_ms = now_ms;
        // A failed check ends the healthy period at once.
        if !ok {
            self.healthy = false;
        }
    }

    /// The http check at `now_ms` (last self-check ok and younger than HTTP_FRESH_MS).
    pub fn http_ok(&self, now_ms: u32) -> bool {
        self.checked && self.check_ok && elapsed_ms(now_ms, self.check_ms) < Self::HTTP_FRESH_MS
    }

    /// Every second. Returns MarkValid / Rollback exactly once, then NotPending.
    pub fn update(&mut self, net_ok: bool, link_up: bool, now_ms: u32) -> OtaValidatorDecision {
        if !self.pending {
            return OtaValidatorDecision::NotPending;
        }
        let checks = [
            (net_ok, Self::CHECK_NET),
            (self.http_ok(now_ms), Self::CHECK_HTTP),
            (!self.stm_required || link_up, Self::CHECK_STM),
        ];
        // the bits of the failed checks (distinct bits: the sum is their union)
        self.missing = checks
            .iter()
            .filter(|(ok, _)| !ok)
            .map(|(_, bit)| bit)
            .sum();
        let healthy = self.missing == 0;
        if healthy && !self.healthy {
            self.healthy_since_ms = now_ms;
        }
        self.healthy = healthy;
        if self.healthy && elapsed_ms(now_ms, self.healthy_since_ms) >= self.confirm_ms {
            self.pending = false;
            return OtaValidatorDecision::MarkValid;
        }
        if elapsed_ms(now_ms, self.start_ms) >= self.give_up_ms {
            self.pending = false;
            return OtaValidatorDecision::Rollback;
        }
        OtaValidatorDecision::Wait
    }

    /// CHECK_* bits that failed at the last update() (CHECK_STM only when stm_required).
    pub fn missing(&self) -> u8 {
        self.missing
    }

    /// A user restart (reboot, network settings, factory reset) of a pending image confirms it
    /// when net_up and (!stm_required || link_up): the request itself came over HTTP. The
    /// bootloader treats any other reset of a pending image as a failed boot. Returns true (and
    /// ends pending) when the caller must mark the image valid first.
    pub fn confirm_before_restart(
        &mut self,
        user_requested: bool,
        net_up: bool,
        link_up: bool,
    ) -> bool {
        if !self.pending || !user_requested || !net_up || (self.stm_required && !link_up) {
            return false;
        }
        self.pending = false;
        true
    }

    pub fn pending(&self) -> bool {
        self.pending
    }

    pub fn stm_required(&self) -> bool {
        self.stm_required
    }

    /// 0 while not healthy or not pending.
    pub fn healthy_for_ms(&self, now_ms: u32) -> u32 {
        if self.pending && self.healthy {
            elapsed_ms(now_ms, self.healthy_since_ms)
        } else {
            0
        }
    }

    /// Until give_up_ms, 0 when not pending.
    pub fn remaining_ms(&self, now_ms: u32) -> u32 {
        if !self.pending {
            return 0;
        }
        self.give_up_ms
            .saturating_sub(elapsed_ms(now_ms, self.start_ms))
    }
}

/// Upload MD5 as given by the user (a C string): exactly 32 hex digits, returned lowercase.
/// None otherwise (the C++ false with `out` unchanged).
pub fn normalize_md5(input: &[u8]) -> Option<[u8; 32]> {
    let input: &[u8; 32] = c_str(input).try_into().ok()?;
    if !input.iter().all(u8::is_ascii_hexdigit) {
        return None;
    }
    Some(input.map(|c| c.to_ascii_lowercase()))
}

#[cfg(test)]
mod tests;
