//! Failsafe lease: shared constants, the regulator-alive rule and the lease status the STM task
//! publishes (port of `vdm/failsafe.h`). Hardware-free.
//!
//! The STM drives every valve to its failsafe position when nobody renewed its lease for the
//! configured timeout (protocol 3); with an older STM the ESP emulates that by pushing the
//! failsafe positions itself.

use crate::config::MqttMode;

/// Failsafe position "hold": stay where it is.
pub const FAILSAFE_HOLD: u8 = 255;
pub const FAILSAFE_PCT_DEFAULT: u8 = 50;
pub const FAILSAFE_TIMEOUT_DEFAULT_MIN: u16 = 60;
/// == STM kLeaseTimeoutMinMin
pub const FAILSAFE_TIMEOUT_MIN_MIN: u16 = 5;
/// == STM kLeaseTimeoutMaxMin
pub const FAILSAFE_TIMEOUT_MAX_MIN: u16 = 1440;

/// 0 (off) or 5..1440 minutes, the range the STM accepts with slcfg.
pub fn lease_timeout_valid(minutes: u32) -> bool {
    minutes == 0
        || (u32::from(FAILSAFE_TIMEOUT_MIN_MIN)..=u32::from(FAILSAFE_TIMEOUT_MAX_MIN))
            .contains(&minutes)
}

/// 0..100 % or [`FAILSAFE_HOLD`].
pub fn failsafe_pct_valid(pct: u32) -> bool {
    pct <= 100 || pct == u32::from(FAILSAFE_HOLD)
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LeaseState {
    #[default]
    Off = 0,
    Running = 1,
    Expired = 2,
}

impl LeaseState {
    pub fn from_raw(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Off),
            1 => Some(Self::Running),
            2 => Some(Self::Expired),
            _ => None,
        }
    }
}

/// "off", "running", "expired".
pub fn lease_state_name(s: LeaseState) -> &'static str {
    match s {
        LeaseState::Off => "off",
        LeaseState::Running => "running",
        LeaseState::Expired => "expired",
    }
}

/// Who runs the lease: the STM (protocol 3) or the ESP emulation (protocols 1/2 with a
/// timeout > 0).
#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LeaseMode {
    #[default]
    None = 0,
    Stm = 1,
    Emulated = 2,
}

impl LeaseMode {
    pub fn from_raw(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::None),
            1 => Some(Self::Stm),
            2 => Some(Self::Emulated),
            _ => None,
        }
    }
}

/// "none", "stm", "esp".
pub fn lease_mode_name(m: LeaseMode) -> &'static str {
    match m {
        LeaseMode::None => "none",
        LeaseMode::Stm => "stm",
        LeaseMode::Emulated => "esp",
    }
}

/// Last status Home Assistant announced on its status topic.
#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HaStatus {
    #[default]
    Unknown = 0,
    Online = 1,
    Offline = 2,
}

impl HaStatus {
    pub fn from_raw(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Unknown),
            1 => Some(Self::Online),
            2 => Some(Self::Offline),
            _ => None,
        }
    }
}

/// "unknown", "online", "offline".
pub fn ha_status_name(s: HaStatus) -> &'static str {
    match s {
        HaStatus::Unknown => "unknown",
        HaStatus::Online => "online",
        HaStatus::Offline => "offline",
    }
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RegulatorCause {
    #[default]
    Alive = 0,
    BrokerDown = 1,
    HaOffline = 2,
}

impl RegulatorCause {
    pub fn from_raw(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Alive),
            1 => Some(Self::BrokerDown),
            2 => Some(Self::HaOffline),
            _ => None,
        }
    }
}

/// "alive", "broker_down", "ha_offline".
pub fn regulator_cause_name(c: RegulatorCause) -> &'static str {
    match c {
        RegulatorCause::Alive => "alive",
        RegulatorCause::BrokerDown => "broker_down",
        RegulatorCause::HaOffline => "ha_offline",
    }
}

/// What the MQTT task reports about the regulator (mqtt::regulatorState()).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RegulatorInput {
    /// default Off
    pub mode: MqttMode,
    pub broker_connected: bool,
    pub ha: HaStatus,
    /// +1 per accepted inbound MQTT command
    pub command_seq: u32,
}

/// MQTT off -> Alive (the ESP is the only client); Mqtt -> Alive while the broker session is
/// up, else BrokerDown; MqttHa -> BrokerDown without a session, HaOffline after an "offline"
/// status, else Alive (Unknown and Online count as alive: HA's birth and last will are not
/// always retained).
pub fn regulator_cause(input: &RegulatorInput) -> RegulatorCause {
    if input.mode == MqttMode::Off {
        return RegulatorCause::Alive;
    }
    if !input.broker_connected {
        return RegulatorCause::BrokerDown;
    }
    if input.mode == MqttMode::MqttHa && input.ha == HaStatus::Offline {
        return RegulatorCause::HaOffline;
    }
    RegulatorCause::Alive
}

/// Failsafe state of one valve in the API and MQTT vocabulary.
#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FailsafeKind {
    #[default]
    None = 0,
    Lease = 1,
    Blocked = 2,
}

impl FailsafeKind {
    pub fn from_raw(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::None),
            1 => Some(Self::Lease),
            2 => Some(Self::Blocked),
            _ => None,
        }
    }
}

/// "off", "lease", "blocked".
pub fn failsafe_kind_name(k: FailsafeKind) -> &'static str {
    match k {
        FailsafeKind::None => "off",
        FailsafeKind::Lease => "lease",
        FailsafeKind::Blocked => "blocked",
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LeaseStatus {
    /// Stm on protocol 3; Emulated on 1/2 with timeout > 0
    pub mode: LeaseMode,
    pub state: LeaseState,
    pub remain_s: u32,
    /// ESP config value
    pub timeout_min: u16,
    /// Stm: gstax field 11; Emulated: valves driven by the ESP
    pub failsafe_mask: u16,
    pub regulator: RegulatorCause,
    /// 0 while alive
    pub regulator_lost_s: u32,
    /// protocol 3: glcfg equals the ESP config
    pub config_synced: bool,
    pub config_failed: bool,
    /// false: ESP runs on default config, nothing is pushed
    pub config_trusted: bool,
}

impl Default for LeaseStatus {
    fn default() -> Self {
        Self {
            mode: LeaseMode::None,
            state: LeaseState::Off,
            remain_s: 0,
            timeout_min: 0,
            failsafe_mask: 0,
            regulator: RegulatorCause::BrokerDown,
            regulator_lost_s: 0,
            config_synced: false,
            config_failed: false,
            config_trusted: true,
        }
    }
}

#[cfg(test)]
mod tests;
