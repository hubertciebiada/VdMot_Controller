//! Network trial: a change of the settings that decide whether and where the device is reachable
//! runs on trial after the restart and is reverted unless the user confirms it from the new
//! address. Record format (NVS "netTrial"), boot rule and the trial window (port of
//! `vdm/net_trial.h`). Hardware-free.

use crate::common::{copy_string, elapsed_ms, format_ipv4, TextBuf};
use crate::config::{crc32, NetConfig, NetInterface, SECRET_MAX, SSID_MAX};

pub const NET_TRIAL_WINDOW_MS: u32 = 120_000;
pub const NET_TRIAL_BLOB_MAX: usize = 136;

const MAGIC: &[u8; 4] = b"VDNT";
const VERSION: u8 = 1;
/// magic, version, state, iface, dhcp, 4 addresses, 2 lengths, trial CRC, CRC
const FIXED_BYTES: usize = 34;

#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NetTrialState {
    #[default]
    Armed = 1,
    Running = 2,
}

impl NetTrialState {
    pub fn from_raw(v: u8) -> Option<Self> {
        match v {
            1 => Some(Self::Armed),
            2 => Some(Self::Running),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NetTrialRecord {
    pub state: NetTrialState,
    /// only the trial fields are encoded; restored on revert
    pub previous: NetConfig,
    /// net_trial_fields_crc() of the settings on trial
    pub trial_crc: u32,
}

/// The trial fields (iface .. password) as in the blob into `w`.
fn put_fields(n: &NetConfig, w: &mut TextBuf<'_>) {
    w.push(n.iface as u8);
    w.push(u8::from(n.dhcp));
    for v in [n.ip, n.mask, n.gateway, n.dns] {
        w.push_bytes(&v.to_le_bytes());
    }
    for text in [n.ssid.as_slice(), n.wifi_password.as_slice()] {
        w.push(text.len() as u8);
        w.push_bytes(text);
    }
}

/// CRC-32 (config crc32) over the trial fields of `n` (iface, dhcp, ip, mask, gateway, dns,
/// ssid, wifi_password), encoded as in the blob.
pub fn net_trial_fields_crc(n: &NetConfig) -> u32 {
    // C++ buf[kNetTrialBlobMax]: the record is at most 130 bytes, TextBuf keeps one for a NUL
    let mut buf = [0u8; NET_TRIAL_BLOB_MAX];
    let mut w = TextBuf::new(&mut buf);
    put_fields(n, &mut w);
    crc32(w.as_bytes(), 0)
}

/// Blob: "VDNT", u8 version 1, u8 state, u8 iface, u8 dhcp, u32 ip, mask, gateway, dns (LE,
/// legacy layout), u8 len + ssid, u8 len + wifi_password, u32 trial_crc, u32 CRC-32 over all
/// bytes before it. At most 130 bytes. Returns the bytes written, 0 when `out` is too small
/// (nothing written then).
pub fn encode_net_trial(r: &NetTrialRecord, out: &mut [u8]) -> usize {
    let mut buf = [0u8; NET_TRIAL_BLOB_MAX];
    let mut w = TextBuf::new(&mut buf);
    w.push_bytes(MAGIC);
    w.push(VERSION);
    w.push(r.state as u8);
    put_fields(&r.previous, &mut w);
    w.push_bytes(&r.trial_crc.to_le_bytes());
    let crc = crc32(w.as_bytes(), 0);
    w.push_bytes(&crc.to_le_bytes());
    let blob = w.as_bytes();
    match out.get_mut(..blob.len()) {
        Some(dst) => {
            dst.copy_from_slice(blob);
            blob.len()
        }
        None => 0,
    }
}

fn le32(p: &[u8]) -> u32 {
    p.get(..4)
        .and_then(|b| b.try_into().ok())
        .map_or(0, u32::from_le_bytes)
}

/// The record of `data` into `out`: false (out unchanged) for a short blob, another magic,
/// version != 1, state not 1/2, iface > 2, dhcp > 1, ssid > 32, password > 64, a length mismatch
/// or a CRC error. Only the trial fields of `out.previous` are written.
pub fn decode_net_trial(data: &[u8], out: &mut NetTrialRecord) -> bool {
    if data.len() < FIXED_BYTES || data.get(..4) != Some(MAGIC.as_slice()) || data[4] != VERSION {
        return false;
    }
    let (Some(state), Some(iface)) = (
        NetTrialState::from_raw(data[5]),
        NetInterface::from_raw(data[6]),
    ) else {
        return false;
    };
    if data[7] > 1 {
        return false;
    }
    let ssid = usize::from(data[24]);
    if ssid > SSID_MAX || 25 + ssid >= data.len() {
        return false;
    }
    let pass = usize::from(data[25 + ssid]);
    if pass > SECRET_MAX || data.len() != FIXED_BYTES + ssid + pass {
        return false;
    }
    let crc_at = data.len() - 4;
    if crc32(&data[..crc_at], 0) != le32(&data[crc_at..]) {
        return false;
    }
    out.state = state;
    let n = &mut out.previous;
    n.iface = iface;
    n.dhcp = data[7] != 0;
    n.ip = le32(&data[8..]);
    n.mask = le32(&data[12..]);
    n.gateway = le32(&data[16..]);
    n.dns = le32(&data[20..]);
    // C strings: a NUL inside a stored text ends it (the C++ copies the bytes into a char array)
    copy_string(&mut n.ssid, &data[25..25 + ssid]);
    copy_string(&mut n.wifi_password, &data[26 + ssid..26 + ssid + pass]);
    out.trial_crc = le32(&data[crc_at - 4..]);
    true
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetTrialBoot {
    None = 0,
    Stale = 1,
    Start = 2,
    RevertNow = 3,
}

impl NetTrialBoot {
    pub fn from_raw(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::None),
            1 => Some(Self::Stale),
            2 => Some(Self::Start),
            3 => Some(Self::RevertNow),
            _ => None,
        }
    }
}

/// No record -> None; `r.trial_crc` != net_trial_fields_crc(current) -> Stale (the stored config
/// is no longer the one on trial: erase, no action); Armed -> Start; Running (the previous boot
/// ended during the trial: crash, power loss, restart) -> RevertNow.
pub fn net_trial_at_boot(r: Option<&NetTrialRecord>, current: &NetConfig) -> NetTrialBoot {
    let Some(r) = r else {
        return NetTrialBoot::None;
    };
    if r.trial_crc != net_trial_fields_crc(current) {
        return NetTrialBoot::Stale;
    }
    match r.state {
        NetTrialState::Running => NetTrialBoot::RevertNow,
        NetTrialState::Armed => NetTrialBoot::Start,
    }
}

/// Revert: copies iface, dhcp, ip, mask, gateway, dns, ssid, wifi_password of `prev` into `dst`;
/// reconnect_timeout_min stays.
pub fn apply_net_trial_fields(dst: &mut NetConfig, prev: &NetConfig) {
    let keep = dst.reconnect_timeout_min;
    *dst = prev.clone();
    dst.reconnect_timeout_min = keep;
}

/// "192.168.1.50" for a static configuration, "dhcp" otherwise (event texts), cut to fit `out`.
/// Returns the length.
pub fn format_net_address(n: &NetConfig, out: &mut [u8]) -> usize {
    let mut ip = [0u8; 16];
    let text: &[u8] = if n.dhcp {
        b"dhcp"
    } else {
        let len = format_ipv4(n.ip, &mut ip);
        ip.get(..len).unwrap_or_default()
    };
    let mut w = TextBuf::new(out);
    w.push_bytes(text);
    w.len()
}

/// Why a trial was reverted. NotStored: the trial record could not be stored, so the trial could
/// not be reverted after an interrupted boot; the new settings do not run at all.
#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NetTrialRevert {
    #[default]
    NotConfirmed = 1,
    NoNetwork = 2,
    Interrupted = 3,
    User = 4,
    NotStored = 5,
}

impl NetTrialRevert {
    pub fn from_raw(v: u8) -> Option<Self> {
        match v {
            1 => Some(Self::NotConfirmed),
            2 => Some(Self::NoNetwork),
            3 => Some(Self::Interrupted),
            4 => Some(Self::User),
            5 => Some(Self::NotStored),
            _ => None,
        }
    }
}

/// What [`NetTrial::update`] asks for (C++ `NetTrial::Decision`).
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetTrialDecision {
    None = 0,
    Revert = 1,
}

/// The trial window of a running network trial.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NetTrial {
    window_ms: u32,
    active: bool,
    up: bool,
    start_ms: u32,
    up_ms: u32,
    reason: NetTrialRevert,
}

impl Default for NetTrial {
    fn default() -> Self {
        Self::new(NET_TRIAL_WINDOW_MS)
    }
}

impl NetTrial {
    pub fn new(window_ms: u32) -> Self {
        Self {
            window_ms,
            active: false,
            up: false,
            start_ms: 0,
            up_ms: 0,
            reason: NetTrialRevert::NotConfirmed,
        }
    }

    /// Boot with an Armed record.
    pub fn start(&mut self, now_ms: u32) {
        self.active = true;
        self.up = false;
        self.start_ms = now_ms;
        self.reason = NetTrialRevert::NotConfirmed;
    }

    pub fn active(&self) -> bool {
        self.active
    }

    /// Every second: Revert once when the network was not up within the window of start()
    /// (NoNetwork), or the window passed since it first came up without confirm()
    /// (NotConfirmed). The window is not restarted by a later IP loss.
    pub fn update(&mut self, net_up: bool, now_ms: u32) -> NetTrialDecision {
        if !self.active {
            return NetTrialDecision::None;
        }
        if net_up && !self.up {
            self.up = true;
            self.up_ms = now_ms;
        }
        if self.up {
            if elapsed_ms(now_ms, self.up_ms) < self.window_ms {
                return NetTrialDecision::None;
            }
            self.reason = NetTrialRevert::NotConfirmed;
        } else {
            if elapsed_ms(now_ms, self.start_ms) < self.window_ms {
                return NetTrialDecision::None;
            }
            self.reason = NetTrialRevert::NoNetwork;
        }
        self.active = false;
        NetTrialDecision::Revert
    }

    pub fn reason(&self) -> NetTrialRevert {
        self.reason
    }

    /// True when a trial was active (then inactive).
    pub fn confirm(&mut self) -> bool {
        if !self.active {
            return false;
        }
        self.active = false;
        true
    }

    /// 0 when inactive.
    pub fn remaining_ms(&self, now_ms: u32) -> u32 {
        if !self.active {
            return 0;
        }
        let since = if self.up { self.up_ms } else { self.start_ms };
        self.window_ms.saturating_sub(elapsed_ms(now_ms, since))
    }

    /// Since the network came up, 0 before.
    pub fn up_for_ms(&self, now_ms: u32) -> u32 {
        if self.up {
            elapsed_ms(now_ms, self.up_ms)
        } else {
            0
        }
    }
}

#[cfg(test)]
mod tests;
