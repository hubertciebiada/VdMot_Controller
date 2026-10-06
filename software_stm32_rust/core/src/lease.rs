//! Lease of the valve targets: while a client renews it, the valves follow their
//! targets; once it expires they go to their failsafe positions (failsafe).
//! The timeout is set by slcfg and stored in the EEPROM. Hardware-free.

/// the targets never expire
pub const LEASE_TIMEOUT_OFF: u16 = 0;
pub const LEASE_TIMEOUT_MIN_MIN: u16 = 5;
pub const LEASE_TIMEOUT_MAX_MIN: u16 = 1440;
/// timeout of an STM that no ESP 2.1 configured (or whose stored value is lost):
/// the polls of a legacy or 2.0.0 ESP renew it, a dead ESP lets it expire
pub const LEASE_TIMEOUT_DEFAULT_MIN: u16 = 60;
/// a lease client is present while it sent a lease command within this time
pub const LEASE_CLIENT_IDLE_S: u32 = 300;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum LeaseState {
    /// timeout 0
    Off = 0,
    Running = 1,
    /// failsafe active
    Expired = 2,
}

/// 0 (off) or LEASE_TIMEOUT_MIN_MIN..LEASE_TIMEOUT_MAX_MIN minutes
pub fn lease_timeout_valid(minutes: u32) -> bool {
    minutes == u32::from(LEASE_TIMEOUT_OFF)
        || (u32::from(LEASE_TIMEOUT_MIN_MIN)..=u32::from(LEASE_TIMEOUT_MAX_MIN)).contains(&minutes)
}

/// Stored value at start-up: anything slcfg accepts is kept, anything else
/// loads LEASE_TIMEOUT_DEFAULT_MIN.
pub fn sanitize_lease_timeout(minutes: u16) -> u16 {
    if lease_timeout_valid(u32::from(minutes)) {
        minutes
    } else {
        LEASE_TIMEOUT_DEFAULT_MIN
    }
}

/// C++ Lease::Snapshot
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub since_renewal_s: u32,
    pub since_client_s: u32,
    pub client: bool,
}

/// The lease itself. Time comes in as elapsed seconds (advance); all counters
/// saturate, so a lease never wraps back to Running.
#[derive(Clone, Copy, Debug, Default)]
pub struct Lease {
    timeout_min: u16,
    /// a fresh lease at start-up
    since_renewal_s: u32,
    since_client_s: u32,
    client: bool,
}

impl Lease {
    /// both counters, saturating at u32::MAX
    pub fn advance(&mut self, elapsed_s: u32) {
        self.since_renewal_s = self.since_renewal_s.saturating_add(elapsed_s);
        self.since_client_s = self.since_client_s.saturating_add(elapsed_s);
    }

    /// slcfg (validated by the caller); no renewal, except that switching the
    /// lease on (0 -> non-zero) starts a fresh lease
    pub fn set_timeout(&mut self, minutes: u16) {
        if self.timeout_min == LEASE_TIMEOUT_OFF && minutes != LEASE_TIMEOUT_OFF {
            self.since_renewal_s = 0;
        }
        self.timeout_min = minutes;
    }

    /// slhbt/slcfg/sfspo/glcfg: a lease client is present
    pub fn lease_command(&mut self) {
        self.client = true;
        self.since_client_s = 0;
    }

    /// slhbt: a lease command; alive renews
    pub fn heartbeat(&mut self, alive: bool) {
        self.lease_command();
        if alive {
            self.since_renewal_s = 0;
        }
    }

    /// gvlvd/gvlvx request: renews only while no lease client is present
    /// (a legacy or 2.0.0 ESP keeps the lease alive with its polls)
    pub fn valve_poll(&mut self) {
        if !self.client_present() {
            self.since_renewal_s = 0;
        }
    }

    /// Off with timeout 0; Expired once timeout * 60 s passed without a renewal
    pub fn state(&self) -> LeaseState {
        if self.timeout_min == LEASE_TIMEOUT_OFF {
            LeaseState::Off
        } else if self.since_renewal_s >= u32::from(self.timeout_min) * 60 {
            LeaseState::Expired
        } else {
            LeaseState::Running
        }
    }

    /// seconds to the expiry while Running, else 0
    pub fn remaining_s(&self) -> u32 {
        if self.state() != LeaseState::Running {
            return 0;
        }
        u32::from(self.timeout_min) * 60 - self.since_renewal_s
    }

    /// a lease command arrived within LEASE_CLIENT_IDLE_S
    pub fn client_present(&self) -> bool {
        self.client_seen_within(LEASE_CLIENT_IDLE_S)
    }

    /// a lease command arrived within the last s seconds
    pub fn client_seen_within(&self, s: u32) -> bool {
        self.client && self.since_client_s < s
    }

    pub fn timeout(&self) -> u16 {
        self.timeout_min
    }

    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            since_renewal_s: self.since_renewal_s,
            since_client_s: self.since_client_s,
            client: self.client,
        }
    }

    /// warm reset; the timeout is set separately
    pub fn restore(&mut self, s: &Snapshot) {
        self.since_renewal_s = s.since_renewal_s;
        self.since_client_s = s.since_client_s;
        self.client = s.client;
    }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_class;
