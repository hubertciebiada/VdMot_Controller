//! STM link request scheduler: fixed-capacity priority queue, exactly one outstanding request,
//! per-request timeout and retry, consecutive-failure tracking, STM reset decision
//! (architecture R6) and STM reboot detection (port of `vdm/link_policy.h`). Hardware-free: the
//! stm_link glue task feeds bytes/lines and time in and writes the returned request lines to
//! the UART.

use crate::common::{elapsed_ms, ALL_VALVES, VALVE_COUNT};
use crate::stm_codec::{
    cmd_is_idempotent, reply_matches, Cmd, Reply, RequestLine, StmStatus, ValveData, ValveStatus,
};

#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    User = 0,
    Config = 1,
    #[default]
    Poll = 2,
}

impl Priority {
    pub fn from_raw(v: u8) -> Option<Self> {
        [Self::User, Self::Config, Self::Poll]
            .get(usize::from(v))
            .copied()
    }
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LinkState {
    /// nothing received since ESP boot
    #[default]
    Unknown = 0,
    /// last request answered
    Up = 1,
    /// >= 1 consecutive timed-out attempt
    Degraded = 2,
    /// >= down_after consecutive timed-out attempts
    Down = 3,
    /// STM was reset by us; requests held for boot_holdoff_ms
    Booting = 4,
    /// UART owned by the flasher; nothing is sent
    Suspended = 5,
}

impl LinkState {
    pub fn from_raw(v: u8) -> Option<Self> {
        [
            Self::Unknown,
            Self::Up,
            Self::Degraded,
            Self::Down,
            Self::Booting,
            Self::Suspended,
        ]
        .get(usize::from(v))
        .copied()
    }
}

/// "unknown", "up", "degraded", "down", "booting", "suspended".
pub fn link_state_name(s: LinkState) -> &'static str {
    match s {
        LinkState::Unknown => "unknown",
        LinkState::Up => "up",
        LinkState::Degraded => "degraded",
        LinkState::Down => "down",
        LinkState::Booting => "booting",
        LinkState::Suspended => "suspended",
    }
}

/// Binding numbers (DESIGN.md "STM link policy"). Timeouts are measured from the moment the
/// request was handed to the UART ([`LinkPolicy::on_sent`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LinkParams {
    /// normal requests
    pub timeout_ms: u16,
    /// list replies: gonec 255, gowvc 255, gvlon 255, gprof
    pub long_timeout_ms: u16,
    /// stons, masns, stdet, reset, smotc, stvls (EEPROM/1-Wire work)
    pub slow_timeout_ms: u16,
    /// extra attempts for idempotent commands
    pub retries: u8,
    /// quiet time after a reply/timeout before the next send
    pub inter_request_gap_ms: u16,
    /// after STM reset: STM boot window (~3.6 s) + 1-Wire
    pub boot_holdoff_ms: u16,
    /// consecutive timed-out attempts -> Down
    pub down_after: u8,
    /// R6: >= 5 consecutive timeouts ...
    pub reset_min_timeouts: u8,
    /// ... spanning >= 60 s ...
    pub reset_min_span_ms: u32,
    /// ... and at most one policy reset per 10 min
    pub reset_min_interval_ms: u32,
}

impl Default for LinkParams {
    fn default() -> Self {
        Self {
            timeout_ms: 400,
            long_timeout_ms: 1500,
            slow_timeout_ms: 3000,
            retries: 2,
            inter_request_gap_ms: 5,
            boot_holdoff_ms: 5000,
            down_after: 5,
            reset_min_timeouts: 5,
            reset_min_span_ms: 60_000,
            reset_min_interval_ms: 600_000,
        }
    }
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Outcome {
    /// matching reply received (for acks: success form)
    Ok = 0,
    /// matching reply in its error form ("smotc err", "svmov v err n", "goned 0", gvlon error)
    Rejected = 1,
    /// no matching reply after all attempts
    #[default]
    Timeout = 2,
}

/// Result of one request, reported exactly once per dequeued request.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Completion {
    pub request: RequestLine,
    pub priority: Priority,
    /// caller's correlation id (0 = none)
    pub tag: u16,
    pub outcome: Outcome,
    /// 1 + retries used
    pub attempts: u8,
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnqueueResult {
    /// appended
    Queued = 0,
    /// merged into an equal queued request (see [`LinkPolicy::enqueue`])
    Coalesced = 1,
    /// no room even after evicting Poll entries
    Full = 2,
    /// empty request text or cmd None
    Invalid = 3,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LinkStats {
    /// attempts written to the UART
    pub sent: u32,
    /// completions with Ok/Rejected
    pub answered: u32,
    /// timed-out attempts (not requests)
    pub timeouts: u32,
    /// completions with Timeout
    pub failed_requests: u32,
    /// valid replies that matched no outstanding request
    pub stray_lines: u32,
    /// lines rejected by parse_reply
    pub parse_errors: u32,
    /// enqueue() == Full
    pub queue_full: u32,
    /// Poll requests dropped to make room
    pub evictions: u32,
    /// STM resets decided by should_reset_stm()
    pub policy_resets: u32,
    /// STM resets requested by the user
    pub user_resets: u32,
    pub consecutive_timeouts: u8,
    /// now_ms of the last matching reply
    pub last_reply_ms: u32,
}

/// A matching reply in its error form completes the request as Rejected.
fn is_rejection(r: &Reply) -> bool {
    match r.cmd {
        Cmd::Smotc | Cmd::Scalx | Cmd::Slhbt | Cmd::Slcfg | Cmd::Ssafe => r.ack.error,
        Cmd::Svmov => !r.service_move.ok,
        Cmd::Sfspo => !r.failsafe.ok,
        Cmd::Sstop => !r.stop.ok,
        Cmd::Goned => !r.temp_data.valid,
        Cmd::Gowvd => !r.volt_data.valid,
        Cmd::Gvlon => r.gvlon_error,
        _ => false,
    }
}

/// One queued request (C++ `LinkPolicy::Entry`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct LinkPolicyEntry {
    line: RequestLine,
    priority: Priority,
    tag: u16,
    /// attempts already made (retries re-queue with > 0)
    attempts: u8,
}

#[derive(Clone, Debug)]
pub struct LinkPolicy {
    params: LinkParams,
    /// Sorted by priority, FIFO within a priority.
    queue: heapless::Vec<LinkPolicyEntry, { LinkPolicy::QUEUE_CAPACITY }>,
    current: LinkPolicyEntry,
    outstanding: bool,
    sent_at_ms: u32,
    quiet_active: bool,
    quiet_since_ms: u32,
    have_heard: bool,
    suspended: bool,
    booting: bool,
    boot_at_ms: u32,
    first_timeout_ms: u32,
    policy_reset_done: bool,
    last_policy_reset_ms: u32,
    stats: LinkStats,
}

impl Default for LinkPolicy {
    fn default() -> Self {
        Self::new(LinkParams::default())
    }
}

impl LinkPolicy {
    pub const QUEUE_CAPACITY: usize = 24;

    pub fn new(params: LinkParams) -> Self {
        Self {
            params,
            queue: heapless::Vec::new(),
            current: LinkPolicyEntry::default(),
            outstanding: false,
            sent_at_ms: 0,
            quiet_active: false,
            quiet_since_ms: 0,
            have_heard: false,
            suspended: false,
            booting: false,
            boot_at_ms: 0,
            first_timeout_ms: 0,
            policy_reset_done: false,
            last_policy_reset_ms: 0,
            stats: LinkStats::default(),
        }
    }

    // ------------------------------------------------------------ queue

    /// Index where an entry of priority `p` goes: after the last entry with the same or higher
    /// priority (`at_head` false) or before the first entry of the same or lower priority
    /// (`at_head` true, used for retries).
    fn insert_pos(&self, p: Priority, at_head: bool) -> usize {
        self.queue
            .iter()
            .take_while(|e| {
                if at_head {
                    e.priority < p
                } else {
                    e.priority <= p
                }
            })
            .count()
    }

    /// `entry` at its place; the caller made room.
    fn insert(&mut self, entry: LinkPolicyEntry, at_head: bool) {
        let pos = self.insert_pos(entry.priority, at_head);
        let _ = self.queue.insert(pos, entry);
    }

    /// Frees one slot when the queue is full by evicting the newest Poll entry.
    fn make_room(&mut self) -> bool {
        if !self.queue.is_full() {
            return true;
        }
        // Sorted by priority: the newest Poll entry, if any, is the last one.
        match self.queue.last() {
            Some(e) if e.priority == Priority::Poll => {
                self.queue.pop();
                self.stats.evictions = self.stats.evictions.wrapping_add(1);
                true
            }
            _ => false,
        }
    }

    /// Queues a request. Ordering: by priority (User before Config before Poll), FIFO within a
    /// priority. Coalescing, so repeated clicks/polls cannot fill the queue:
    ///  - stgtp for a valve already queued: the queued line is replaced by the new one (latest
    ///    target wins), position kept, tag updated, retry count restarted (a queued retry
    ///    becomes a fresh request) -> Coalesced;
    ///  - any other byte-identical line already queued -> Coalesced (the higher priority of the
    ///    two is kept; a non-zero tag replaces the queued one).
    ///
    /// A raised entry moves behind the entries already queued at its new priority. The
    /// outstanding request is never coalesced with. When the queue is full, the newest Poll
    /// entry is evicted to make room for a User/Config request; a Poll request never evicts
    /// anything -> Full. Allowed in every state; while Booting/Suspended requests wait.
    pub fn enqueue(
        &mut self,
        request: &RequestLine,
        priority: Priority,
        tag: u16,
    ) -> EnqueueResult {
        // C++ also refuses len > kRequestMaxLen, a command number >= kCmdCount and a priority
        // above Poll, which a Text<63>, a Cmd and a Priority cannot hold.
        if request.text.is_empty() || request.cmd == Cmd::None {
            return EnqueueResult::Invalid;
        }

        let same_target = |e: &LinkPolicyEntry| {
            request.cmd == Cmd::Stgtp && e.line.cmd == Cmd::Stgtp && e.line.valve == request.valve
        };
        let found = self
            .queue
            .iter()
            .position(|e| same_target(e) || e.line.text == request.text);
        if let Some(i) = found {
            let e = &mut self.queue[i];
            if same_target(e) {
                // latest target wins, as a new request with its own retries
                e.line = request.clone();
                e.tag = tag;
                e.attempts = 0;
            } else if tag != 0 {
                e.tag = tag;
            }
            if priority < e.priority {
                let mut moved = self.queue.remove(i);
                moved.priority = priority;
                self.insert(moved, false);
            }
            return EnqueueResult::Coalesced;
        }

        if self.queue.is_full() && (priority == Priority::Poll || !self.make_room()) {
            self.stats.queue_full = self.stats.queue_full.wrapping_add(1);
            return EnqueueResult::Full;
        }
        self.insert(
            LinkPolicyEntry {
                line: request.clone(),
                priority,
                tag,
                attempts: 0,
            },
            false,
        );
        EnqueueResult::Queued
    }

    // ------------------------------------------------------------ sending

    fn timeout_for(&self, r: &RequestLine) -> u16 {
        match r.cmd {
            Cmd::Gonec | Cmd::Gowvc if r.arg == u16::from(ALL_VALVES) => {
                self.params.long_timeout_ms
            }
            Cmd::Gvlon if r.valve == ALL_VALVES => self.params.long_timeout_ms,
            Cmd::Gprof => self.params.long_timeout_ms,
            Cmd::Stons | Cmd::Masns | Cmd::Stdet | Cmd::Reset | Cmd::Smotc | Cmd::Stvls => {
                self.params.slow_timeout_ms
            }
            _ => self.params.timeout_ms,
        }
    }

    fn boot_hold_active(&self, now_ms: u32) -> bool {
        self.booting && elapsed_ms(now_ms, self.boot_at_ms) < u32::from(self.params.boot_holdoff_ms)
    }

    /// Returns the next line to write to the UART, or None when a request is outstanding, the
    /// queue is empty, the inter-request gap has not passed, or the link is Booting/Suspended.
    /// The caller must call [`on_sent`](Self::on_sent) right after the write (the timeout
    /// starts there).
    pub fn next_to_send(&mut self, now_ms: u32) -> Option<&RequestLine> {
        if self.suspended || self.outstanding || self.queue.is_empty() {
            return None;
        }
        if self.boot_hold_active(now_ms) {
            return None;
        }
        self.booting = false;
        if self.quiet_active
            && elapsed_ms(now_ms, self.quiet_since_ms) < u32::from(self.params.inter_request_gap_ms)
        {
            return None;
        }
        self.current = self.queue.remove(0);
        self.current.attempts = self.current.attempts.saturating_add(1);
        self.outstanding = true;
        self.sent_at_ms = now_ms; // on_sent() refines it; also covers a caller that forgets
        Some(&self.current.line)
    }

    pub fn on_sent(&mut self, now_ms: u32) {
        if !self.outstanding {
            return;
        }
        self.sent_at_ms = now_ms;
        self.stats.sent = self.stats.sent.wrapping_add(1);
    }

    fn complete(&mut self, outcome: Outcome, now_ms: u32) -> Completion {
        self.outstanding = false;
        self.quiet_active = true;
        self.quiet_since_ms = now_ms;
        Completion {
            request: self.current.line.clone(),
            priority: self.current.priority,
            tag: self.current.tag,
            outcome,
            attempts: self.current.attempts,
        }
    }

    // ------------------------------------------------------------ receiving

    /// Feeds a parsed line. Returns the completion when it completes the outstanding request
    /// ([`reply_matches`]). A valid reply that matches nothing is counted as stray and returns
    /// None (the caller may still apply self-identifying data: gvlvd, gvlvx, gtgtp, goned,
    /// gowvd, gprof). Any matching reply resets the consecutive-timeout counter and makes the
    /// link Up.
    pub fn on_reply(&mut self, reply: &Reply, now_ms: u32) -> Option<Completion> {
        if !self.outstanding || !reply_matches(&self.current.line, reply) {
            self.stats.stray_lines = self.stats.stray_lines.wrapping_add(1);
            return None;
        }
        let outcome = if is_rejection(reply) {
            Outcome::Rejected
        } else {
            Outcome::Ok
        };
        let c = self.complete(outcome, now_ms);
        self.stats.answered = self.stats.answered.wrapping_add(1);
        self.stats.consecutive_timeouts = 0;
        self.stats.last_reply_ms = now_ms;
        self.have_heard = true;
        Some(c)
    }

    /// A line that parse_reply rejected: counted only; it neither completes nor proves the link
    /// (noise on the UART must not look healthy).
    pub fn on_parse_error(&mut self, _now_ms: u32) {
        self.stats.parse_errors = self.stats.parse_errors.wrapping_add(1);
    }

    /// Timeout handling; call every loop iteration. When the outstanding attempt expired: an
    /// idempotent request with attempts left is re-sent (it goes back to the head of its
    /// priority; on a full queue the newest Poll entry is evicted for it) and None is returned;
    /// otherwise (or when nothing can be evicted) the request completes with Outcome::Timeout.
    /// A probe (RequestLine::probe: gproto, which a v1 STM leaves unanswered by design) never
    /// counts toward consecutive_timeouts.
    pub fn poll(&mut self, now_ms: u32) -> Option<Completion> {
        if self.booting && !self.boot_hold_active(now_ms) {
            self.booting = false;
        }
        // Retire the reset rate limit while it is still measurable (elapsed_ms wraps after 49
        // days).
        if self.policy_reset_done
            && elapsed_ms(now_ms, self.last_policy_reset_ms) >= self.params.reset_min_interval_ms
        {
            self.policy_reset_done = false;
        }
        if !self.outstanding
            || elapsed_ms(now_ms, self.sent_at_ms) < u32::from(self.timeout_for(&self.current.line))
        {
            return None;
        }

        self.stats.timeouts = self.stats.timeouts.wrapping_add(1);
        if !self.current.line.probe {
            if self.stats.consecutive_timeouts == 0 {
                self.first_timeout_ms = now_ms;
            }
            self.stats.consecutive_timeouts = self.stats.consecutive_timeouts.saturating_add(1);
        }

        if cmd_is_idempotent(self.current.line.cmd)
            && self.current.attempts <= self.params.retries
            && self.make_room()
        {
            self.outstanding = false;
            self.quiet_active = true;
            self.quiet_since_ms = now_ms;
            self.insert(self.current.clone(), true);
            return None;
        }
        let c = self.complete(Outcome::Timeout, now_ms);
        self.stats.failed_requests = self.stats.failed_requests.wrapping_add(1);
        Some(c)
    }

    // ------------------------------------------------------------ resets

    /// R6: true when consecutive_timeouts >= reset_min_timeouts AND the first of them is >=
    /// reset_min_span_ms old AND no policy reset happened in the last reset_min_interval_ms AND
    /// not Booting/Suspended.
    pub fn should_reset_stm(&self, now_ms: u32) -> bool {
        // Booting needs no check: entering it clears consecutive_timeouts and nothing is sent
        // until it ends.
        let n = self.stats.consecutive_timeouts;
        if self.suspended || n == 0 || n < self.params.reset_min_timeouts {
            return false;
        }
        if elapsed_ms(now_ms, self.first_timeout_ms) < self.params.reset_min_span_ms {
            return false;
        }
        !self.policy_reset_done
            || elapsed_ms(now_ms, self.last_policy_reset_ms) >= self.params.reset_min_interval_ms
    }

    fn start_hold(&mut self, now_ms: u32) {
        self.outstanding = false;
        self.booting = true;
        self.boot_at_ms = now_ms;
        self.quiet_active = false;
        self.have_heard = false;
        self.stats.consecutive_timeouts = 0;
    }

    /// Must be called when the glue has pulsed NRST (by policy or by the user). Drops the
    /// outstanding request (no completion), removes all Poll entries, keeps User/Config
    /// entries, clears the failure counters and enters Booting for boot_holdoff_ms; afterwards
    /// the state is Unknown until the first matching reply.
    pub fn on_stm_reset(&mut self, now_ms: u32, by_policy: bool) {
        self.queue.retain(|e| e.priority != Priority::Poll);
        if by_policy {
            self.stats.policy_resets = self.stats.policy_resets.wrapping_add(1);
            self.policy_reset_done = true;
            self.last_policy_reset_ms = now_ms;
        } else {
            self.stats.user_resets = self.stats.user_resets.wrapping_add(1);
        }
        self.start_hold(now_ms);
    }

    /// ESP boot: the IO15 strap pull-up holds NRST while the ESP boots, so the STM is most
    /// likely starting up as well. Enters Booting for boot_holdoff_ms like on_stm_reset(), but
    /// counts no reset and keeps the queue. Without it the first gproto probe lands in the
    /// STM's start-up window, times out and selects protocol v1 for a v2 STM.
    pub fn hold_after_esp_boot(&mut self, now_ms: u32) {
        self.start_hold(now_ms);
    }

    /// The flasher takes the UART: drops the outstanding request and all queued entries (they
    /// are stale after a re-flash) and returns how many were dropped.
    pub fn suspend(&mut self) -> usize {
        let dropped = self.queue.len() + usize::from(self.outstanding);
        self.queue.clear();
        self.outstanding = false;
        self.suspended = true;
        dropped
    }

    /// The flasher returns the UART: Booting (the flasher reset the STM).
    pub fn resume(&mut self, now_ms: u32) {
        self.suspended = false;
        self.start_hold(now_ms);
    }

    pub fn state(&self, now_ms: u32) -> LinkState {
        if self.suspended {
            return LinkState::Suspended;
        }
        if self.boot_hold_active(now_ms) {
            return LinkState::Booting;
        }
        let n = self.stats.consecutive_timeouts;
        if n > 0 && n >= self.params.down_after {
            return LinkState::Down;
        }
        if n > 0 {
            return LinkState::Degraded;
        }
        if self.have_heard {
            LinkState::Up
        } else {
            LinkState::Unknown
        }
    }

    pub fn stats(&self) -> &LinkStats {
        &self.stats
    }

    pub fn queued(&self) -> usize {
        self.queue.len()
    }

    /// Queued entries of priority `p` (C++ `queued(Priority)`).
    pub fn queued_with(&self, p: Priority) -> usize {
        self.queue.iter().filter(|e| e.priority == p).count()
    }

    pub fn busy(&self) -> bool {
        self.outstanding
    }

    /// A request of priority `p` is outstanding.
    pub fn busy_with(&self, p: Priority) -> bool {
        self.outstanding && self.current.priority == p
    }
}

/// [`RebootDetector::on_link_state`] (C++ `RebootDetector::Recovery`).
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RebootDetectorRecovery {
    None = 0,
    Reboot = 1,
    CheckStatus = 2,
}

/// Detects that the STM rebooted underneath us (power glitch, watchdog, reset by the user at
/// the board) so the ESP can re-sync: re-read parameters and re-push the desired targets.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RebootDetector {
    have_status: bool,
    last: StmStatus,
    last_ms: u32,
    check_armed: bool,
    calibrated: [bool; VALVE_COUNT as usize],
}

impl RebootDetector {
    pub const UPTIME_SLACK_S: u32 = 5;

    /// gstat/gstax. The first status after reset() only primes. Returns the cause of the
    /// StmRebootDetected event, 0 = no reboot:
    ///  2 when the reset counter differs from the previous status;
    ///  1 when the uptime decreased, or uptime + UPTIME_SLACK_S + e / 1000 < previous uptime +
    ///    e, where e is the whole seconds between the two statuses on the ESP clock (a power-on
    ///    restart after a long outage).
    ///
    /// Clears an armed link-recovery check.
    pub fn on_status(&mut self, s: &StmStatus, now_ms: u32) -> u8 {
        self.check_armed = false;
        let mut cause = 0;
        if self.have_status {
            let e = elapsed_ms(now_ms, self.last_ms) / 1000;
            if s.resets != self.last.resets {
                cause = 2;
            } else if s.uptime_s < self.last.uptime_s
                || u64::from(s.uptime_s) + u64::from(Self::UPTIME_SLACK_S) + u64::from(e / 1000)
                    < u64::from(self.last.uptime_s) + u64::from(e)
            {
                cause = 1;
            }
        }
        self.have_status = true;
        self.last = *s;
        self.last_ms = now_ms;
        cause
    }

    /// v1 heuristic: true when a valve that previously reported a non-zero open_count or
    /// close_count now reports open_count == close_count == moves == 0 with status Unknown(5),
    /// Connected(8) or NoValve(6).
    pub fn on_valve_data(&mut self, d: &ValveData) -> bool {
        let Some(calibrated) = self.calibrated.get_mut(usize::from(d.valve)) else {
            return false;
        };
        if d.open_count > 0 || d.close_count > 0 {
            *calibrated = true;
            return false;
        }
        let boot_state = matches!(
            ValveStatus::from_raw(d.status),
            Some(ValveStatus::Unknown | ValveStatus::NoValve | ValveStatus::Connected)
        );
        if !*calibrated || d.moves != 0 || !boot_state {
            return false;
        }
        // Every valve lost its counts in the reboot: forget all of them so the other valves'
        // first post-reboot replies do not report it again.
        self.calibrated = [false; VALVE_COUNT as usize];
        true
    }

    /// Down -> Up: Reboot on protocol <= 1 (no gstat), CheckStatus on 2/3 (arms a check: the
    /// caller asks for gstat/gstax at once, on_status() decides). Any other transition: None.
    pub fn on_link_state(
        &mut self,
        prev: LinkState,
        cur: LinkState,
        proto: u8,
    ) -> RebootDetectorRecovery {
        if prev != LinkState::Down || cur != LinkState::Up {
            return RebootDetectorRecovery::None;
        }
        if proto <= 1 {
            return RebootDetectorRecovery::Reboot;
        }
        self.check_armed = true;
        RebootDetectorRecovery::CheckStatus
    }

    /// The status request of an armed check timed out: true (once) = treat it as a reboot
    /// (cause 4).
    pub fn on_status_failed(&mut self) -> bool {
        let armed = self.check_armed;
        self.check_armed = false;
        armed
    }

    /// Forget history (after an ESP-initiated reset or a re-flash, where the reboot is known);
    /// also disarms the check.
    pub fn reset(&mut self) {
        self.check_armed = false;
        self.have_status = false;
        self.last = StmStatus::default();
        self.calibrated = [false; VALVE_COUNT as usize];
    }
}

#[cfg(test)]
mod tests;
