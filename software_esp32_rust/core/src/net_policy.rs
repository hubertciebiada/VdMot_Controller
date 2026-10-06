//! Network policy: end-to-end reachability of the network and the watchdog that restarts the
//! interface and then the ESP when it is lost (port of `vdm/net_policy.h`). Hardware-free.

use crate::common::elapsed_ms;

/// Traffic that proves the network works end to end: a gateway ping reply, the MQTT session, an
/// SNTP sync, an HTTP request from a LAN peer, a DHCP lease (events, /api/health).
#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NetEvidence {
    #[default]
    None = 0,
    GatewayPing = 1,
    Mqtt = 2,
    TimeSync = 3,
    InboundHttp = 4,
    DhcpLease = 5,
}

impl NetEvidence {
    pub fn from_raw(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::None),
            1 => Some(Self::GatewayPing),
            2 => Some(Self::Mqtt),
            3 => Some(Self::TimeSync),
            4 => Some(Self::InboundHttp),
            5 => Some(Self::DhcpLease),
            _ => None,
        }
    }
}

/// "none", "ping", "mqtt", "ntp", "http", "dhcp".
pub fn net_evidence_name(e: NetEvidence) -> &'static str {
    match e {
        NetEvidence::None => "none",
        NetEvidence::GatewayPing => "ping",
        NetEvidence::Mqtt => "mqtt",
        NetEvidence::TimeSync => "ntp",
        NetEvidence::InboundHttp => "http",
        NetEvidence::DhcpLease => "dhcp",
    }
}

/// [`NetReachability::change`] (C++ `NetReachability::Change`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetReachabilityChange {
    None,
    Lost,
    Regained,
}

/// End-to-end reachability. Evidence = traffic that crossed the network. The staleness check
/// is armed only by a gateway ping reply, so a gateway that never answers ICMP (or a broker
/// outage) leaves the IP-only behaviour.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NetReachability {
    ip_up: bool,
    started: bool,
    gateway: u32,
    up_ms: u32,
    armed: bool,
    evidence: NetEvidence,
    evidence_ms: u32,
    probe_sent: bool,
    probe_ms: u32,
    was_reachable: bool,
    lost: bool,
    lost_known: bool,
    lost_ms: u32,
}

impl NetReachability {
    /// 2.5 probe intervals
    pub const STALE_MS: u32 = 150_000;
    pub const PROBE_INTERVAL_MS: u32 = 60_000;

    /// Every second. An IP transition (up <-> down) clears the evidence (not proven, age none);
    /// the probe is due at once when the IP comes up. A gateway change (incl. to 0) disarms the
    /// check and makes the probe due at once. The armed flag survives an IP loss with the same
    /// gateway.
    pub fn update(&mut self, ip_up: bool, gateway: u32, now_ms: u32) {
        if !self.started || ip_up != self.ip_up {
            self.started = true;
            self.ip_up = ip_up;
            self.up_ms = now_ms;
            self.evidence = NetEvidence::None;
            self.probe_sent = false;
            if !ip_up {
                self.lost = false;
                self.lost_known = false;
            }
        }
        // While the IP is down the interface reports no gateway: that is no change.
        if ip_up && gateway != self.gateway {
            self.gateway = gateway;
            self.armed = false;
            self.probe_sent = false;
        }
    }

    /// Ignored while the IP is down and for None. GatewayPing arms the check.
    pub fn on_evidence(&mut self, e: NetEvidence, now_ms: u32) {
        if !self.ip_up || e == NetEvidence::None {
            return;
        }
        self.evidence = e;
        self.evidence_ms = now_ms;
        if e == NetEvidence::GatewayPing {
            self.armed = true;
        }
    }

    /// ip_up, gateway != 0, and no probe since the IP came up / the gateway changed, or
    /// >= PROBE_INTERVAL_MS since the last one.
    pub fn probe_due(&self, now_ms: u32) -> bool {
        if !self.ip_up || self.gateway == 0 {
            return false;
        }
        !self.probe_sent || elapsed_ms(now_ms, self.probe_ms) >= Self::PROBE_INTERVAL_MS
    }

    pub fn on_probe_sent(&mut self, now_ms: u32) {
        self.probe_sent = true;
        self.probe_ms = now_ms;
    }

    pub fn armed(&self) -> bool {
        self.armed
    }

    pub fn ip_up(&self) -> bool {
        self.ip_up
    }

    /// ip_up and evidence since the IP came up (the OTA network check).
    pub fn proven(&self) -> bool {
        self.ip_up && self.evidence != NetEvidence::None
    }

    /// ip_up and (not armed, or evidence younger than STALE_MS; the IP coming up starts the
    /// clock when there is no evidence yet).
    pub fn reachable(&self, now_ms: u32) -> bool {
        if !self.ip_up {
            return false;
        }
        if !self.armed {
            return true;
        }
        let since = if self.evidence != NetEvidence::None {
            self.evidence_ms
        } else {
            self.up_ms
        };
        elapsed_ms(now_ms, since) < Self::STALE_MS
    }

    /// None since the IP came up.
    pub fn last_evidence(&self) -> NetEvidence {
        self.evidence
    }

    /// u32::MAX when none since the IP came up.
    pub fn evidence_age_ms(&self, now_ms: u32) -> u32 {
        if self.evidence != NetEvidence::None {
            elapsed_ms(now_ms, self.evidence_ms)
        } else {
            u32::MAX
        }
    }

    /// Transitions while the IP stays up: Lost when reachable() turns false with the IP up,
    /// Regained when it turns true again after a Lost (an IP loss in between cancels it;
    /// NetDown/NetUp report that). Once per second after update()/on_evidence().
    pub fn change(&mut self, now_ms: u32) -> NetReachabilityChange {
        let r = self.reachable(now_ms);
        let mut c = NetReachabilityChange::None;
        if self.ip_up {
            if self.was_reachable && !r {
                self.lost = true;
                self.lost_known = true;
                self.lost_ms = now_ms;
                c = NetReachabilityChange::Lost;
            } else if !self.was_reachable && r && self.lost {
                self.lost = false;
                c = NetReachabilityChange::Regained;
            }
        }
        self.was_reachable = r;
        c
    }

    /// Since the last Lost, 0 when none.
    pub fn lost_for_ms(&self, now_ms: u32) -> u32 {
        if self.lost_known {
            elapsed_ms(now_ms, self.lost_ms)
        } else {
            0
        }
    }
}

/// [`NetWatchdog::update`] (C++ `NetWatchdog::Action`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetWatchdogAction {
    None,
    RestartInterface,
    RestartEsp,
}

/// Network watchdog (legacy netConnTO). `reachable` is [`NetReachability::reachable`]. After
/// `minutes` without reachability the network interface is restarted (once per outage); after
/// another wait_ms() the ESP restarts (once per outage). 0 disables both. wait_ms() grows by
/// GROWTH with every ESP restart of the same outage, up to MAX_WAIT_MIN: each ESP restart also
/// resets the STM when jumper X20 is fitted, stopping motors and recalibrating every valve, and
/// restarting again does not fix a switch that is off. The glue keeps restarts_in_outage()
/// across software restarts (RTC memory). The first minutes after boot count as well.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NetWatchdog {
    minutes: u8,
    restarts: u8,
    down: bool,
    down_since_ms: u32,
    iface_fired: bool,
    esp_fired: bool,
    started: bool,
    iface_restarts: u16,
}

impl Default for NetWatchdog {
    fn default() -> Self {
        Self {
            minutes: 0,
            restarts: 0,
            down: true,
            down_since_ms: 0,
            iface_fired: false,
            esp_fired: false,
            started: false,
            iface_restarts: 0,
        }
    }
}

impl NetWatchdog {
    pub const GROWTH: u32 = 4;
    pub const MAX_WAIT_MIN: u32 = 24 * 60;

    /// Re-arms both actions.
    pub fn configure(&mut self, minutes: u8) {
        self.minutes = minutes;
        self.iface_fired = false;
        self.esp_fired = false;
    }

    /// Watchdog restarts earlier in the outage that is still going on at boot (0 after
    /// power-on or once the network was up).
    pub fn set_restarts_in_outage(&mut self, n: u8) {
        self.restarts = n;
    }

    pub fn restarts_in_outage(&self) -> u8 {
        self.restarts
    }

    /// Wait between the interface restart and the ESP restart: minutes * GROWTH^restarts,
    /// capped.
    pub fn wait_ms(&self) -> u32 {
        let mut min = u32::from(self.minutes); // <= 255, below the cap
        for _ in 0..self.restarts {
            min = (min * Self::GROWTH).min(Self::MAX_WAIT_MIN);
        }
        min * 60_000
    }

    /// Once per second.
    ///  - reachable              -> outage over: restarts 0, both actions re-armed, None
    ///  - first unreachable call -> outage starts (boot counts as a start)
    ///  - minutes == 0           -> None
    ///  - outage >= minutes*60000 + wait_ms() -> RestartEsp once (restarts + 1, saturating)
    ///  - outage >= minutes*60000            -> RestartInterface once
    pub fn update(&mut self, reachable: bool, now_ms: u32) -> NetWatchdogAction {
        if reachable {
            self.down = false;
            self.iface_fired = false;
            self.esp_fired = false;
            self.restarts = 0;
            return NetWatchdogAction::None;
        }
        if !self.started || !self.down {
            // Boot (the first call) or the moment the network was lost.
            self.started = true;
            self.down = true;
            self.down_since_ms = now_ms;
        }
        if self.minutes == 0 {
            return NetWatchdogAction::None;
        }
        let outage = elapsed_ms(now_ms, self.down_since_ms);
        let iface_at = u32::from(self.minutes) * 60_000;
        if !self.esp_fired && outage >= iface_at + self.wait_ms() {
            self.esp_fired = true;
            self.iface_fired = true;
            self.restarts = self.restarts.saturating_add(1);
            return NetWatchdogAction::RestartEsp;
        }
        if !self.iface_fired && outage >= iface_at {
            self.iface_fired = true;
            self.iface_restarts = self.iface_restarts.saturating_add(1);
            return NetWatchdogAction::RestartInterface;
        }
        NetWatchdogAction::None
    }

    /// 0 while reachable.
    pub fn outage_ms(&self, now_ms: u32) -> u32 {
        if self.started && self.down {
            elapsed_ms(now_ms, self.down_since_ms)
        } else {
            0
        }
    }

    /// Since boot, saturating.
    pub fn interface_restarts(&self) -> u16 {
        self.iface_restarts
    }
}

#[cfg(test)]
mod tests;
