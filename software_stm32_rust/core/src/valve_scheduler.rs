//! The decision part of app_loop: which valve the valve state machine works on
//! next (presence test, move, calibration). One decision per call while the
//! valve state machine is idle; the glue builds the views and applies the
//! decision. Hardware-free.
//!
//! Order of one call (spec-stm 6.10 and 13 C-1..C-3):
//! ```text
//!  0. safe mode or a temperature hold -> nothing.
//!  1. presence test of an unknown valve (test index 0 2 4 .. 10 1 3 .. 11 0 ..).
//!  2. plain moves (targets before calibrations), round robin:
//!     a. full open requested (staop) -> to the open end stop;
//!     b. open circuit and drive != actual -> presence test (retest);
//!     c. blocked at its failsafe position, no request -> move there, status kept;
//!     d. idle, calibrated, no request -> reference move to an end stop when the
//!        position is not referenced (and the valve is due: drive != actual,
//!        touched by a target request or driven by the lease failsafe), else a
//!        move to the drive target.
//!  3. calibrations and requests, round robin:
//!     a. present and (a target change happened, an explicit request, due) -> calibrate;
//!     b. idle and calibration pending (never calibrated, recal) and due -> calibrate;
//!     c. a request of a valve that is not unknown or present -> mark it present.
//!  After 12 step-2 decisions in a row step 3 goes first once (fairness).
//! ```
//!
//! End-stop latch per valve: a move that ends at an end stop (EndStop,
//! EarlyEndStop, SafetyOvercurrent) is not repeated in the same direction while
//! the drive value and the status stay; after an early end stop of a normal move
//! exactly one retry is allowed. A move of a failed or blocked valve gets no
//! retry. The latch clears when the drive or the status changes, after a move in
//! the other direction, a completed reference move, and when the valve gets a
//! test or a calibration (and clear_latch() after a successful calibration).

use crate::legacy_layout::VALVE_COUNT;
use crate::move_classifier::{StopReason, DIR_CLOSE, DIR_OPEN};
use crate::valve_codes::{
    ST_BLOCKED, ST_FAILED, ST_FULL_OPEN, ST_IDLE, ST_OPEN_CIRCUIT, ST_PRESENT, ST_UNKNOWN,
};

const VALVES: usize = VALVE_COUNT as usize;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ValveView {
    pub status: u8,
    pub actual: u8,
    pub target: u8,
    /// failsafe::drive_target()
    pub drive: u8,
    /// drive source BlockedFailsafe
    pub blocked_failsafe: bool,
    /// drive source LeaseFailsafe
    pub lease_forced: bool,
    /// movement trigger / staln flag (calibration && calibStarted)
    pub calib_flag: bool,
    /// staln
    pub forced_learn: bool,
    /// time trigger
    pub timed_learn: bool,
    /// automatic retry of a failed or blocked valve
    pub retry_learn: bool,
    /// second early partial stop in a row
    pub early_learn: bool,
    /// counts of a successful calibration (learned or restored)
    pub calibrated: bool,
    /// a full calibration is required
    pub recal: bool,
    /// position not referenced
    pub needs_reference: bool,
    /// left where a service move or sstop ended
    pub svc_hold: bool,
    /// accepted stgtp since the last reference move or calibration
    pub touched: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum ActionKind {
    #[default]
    None = 0,
    Test = 1,
    OpenEnd = 2,
    CloseEnd = 3,
    Open = 4,
    Close = 5,
    Learn = 6,
    MarkPresent = 7,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Decision {
    pub kind: ActionKind,
    pub valve: u8,
    /// Open / Close: % to move
    pub delta: u8,
    /// the move keeps the status of the valve (blocked valve to its failsafe)
    pub keep_status: bool,
    /// reference move of a valve whose position is not referenced
    pub reference: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SchedulerInputs {
    pub safe_mode: bool,
    pub hold_for_temperature: bool,
}

#[derive(Clone, Copy, Debug, Default)]
struct Latch {
    valid: bool,
    dir: u8,
    drive: u8,
    status: u8,
    /// one retry in dir left
    retry: bool,
}

#[derive(Clone, Copy, Debug, Default)]
struct Pending {
    valid: bool,
    dir: u8,
    drive: u8,
    keep_status: bool,
    reference: bool,
}

fn faulted(v: &ValveView) -> bool {
    v.status == ST_FAILED || v.status == ST_BLOCKED
}

/// a failed or blocked valve is calibrated only on staln and by its automatic retry
fn explicit_req(v: &ValveView) -> bool {
    if faulted(v) {
        return v.forced_learn || v.retry_learn;
    }
    v.forced_learn || v.retry_learn || v.early_learn || v.calib_flag
}

fn any_req(v: &ValveView) -> bool {
    explicit_req(v) || (v.timed_learn && !faulted(v))
}

fn pending_cal(v: &ValveView) -> bool {
    !v.calibrated || v.recal
}

/// the valve has something to do: its drive target differs, a target request
/// touched it, or the lease failsafe drives it
fn due(v: &ValveView) -> bool {
    v.drive != v.actual || v.touched || v.lease_forced
}

fn move_dir(v: &ValveView) -> u8 {
    if v.drive > v.actual {
        DIR_OPEN
    } else {
        DIR_CLOSE
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ValveScheduler {
    latch: [Latch; VALVES],
    pending: [Pending; VALVES],
    test_index: u8,
    rr: u8,
    step2_run: u8,
    first_change: bool,
}

impl ValveScheduler {
    pub const FAIRNESS_RUN: u8 = 12;

    pub fn next(&mut self, v: &[ValveView; VALVES], input: &SchedulerInputs) -> Decision {
        let mut d = Decision::default();
        if input.safe_mode || input.hold_for_temperature {
            return d;
        }

        // even and odd valves alternate (fewer MUX relay switches on the C1 board)
        let t = self.test_index;
        self.test_index += 2;
        if self.test_index == VALVE_COUNT {
            self.test_index = 1;
        } else if self.test_index > VALVE_COUNT {
            self.test_index = 0;
        }
        if v[usize::from(t)].status == ST_UNKNOWN {
            d.kind = ActionKind::Test;
            d.valve = t;
            self.latch[usize::from(t)].valid = false;
            return d;
        }

        if self.step2_run >= Self::FAIRNESS_RUN && self.step3(v, &mut d) {
            self.step2_run = 0;
            return d;
        }
        if self.step2(v, &mut d) {
            if self.step2_run < Self::FAIRNESS_RUN {
                self.step2_run += 1;
            }
            return d;
        }
        // the run counts step-2 decisions in a row only
        self.step2_run = 0;
        self.step3(v, &mut d);
        d
    }

    /// a counted rejected target of a failed or blocked valve is a target change too
    pub fn note_change(&mut self) {
        self.first_change = true;
    }

    pub fn first_change(&self) -> bool {
        self.first_change
    }

    fn move_allowed(&mut self, i: usize, v: &ValveView, dir: u8) -> bool {
        let l = &mut self.latch[i];
        if !l.valid {
            return true;
        }
        if l.drive != v.drive || l.status != v.status {
            l.valid = false;
            return true;
        }
        dir != l.dir || l.retry
    }

    fn set_move(&mut self, d: &mut Decision, i: usize, v: &ValveView, keep_status: bool) {
        let dir = move_dir(v);
        d.valve = i as u8;
        d.keep_status = keep_status;
        if v.drive == 100 {
            d.kind = ActionKind::OpenEnd;
        } else if v.drive == 0 {
            d.kind = ActionKind::CloseEnd;
        } else if dir == DIR_OPEN {
            d.kind = ActionKind::Open;
            d.delta = v.drive.wrapping_sub(v.actual);
        } else {
            d.kind = ActionKind::Close;
            d.delta = v.actual.wrapping_sub(v.drive);
        }
        if self.latch[i].valid && self.latch[i].dir == dir {
            self.latch[i].retry = false;
        }
        self.pending[i] = Pending {
            valid: true,
            dir,
            drive: v.drive,
            keep_status,
            reference: false,
        };
    }

    fn step2(&mut self, v: &[ValveView; VALVES], d: &mut Decision) -> bool {
        for n in 0..VALVES {
            let i = (usize::from(self.rr) + n) % VALVES;
            let x = &v[i];
            let mut found = true;
            if x.status == ST_FULL_OPEN {
                d.kind = ActionKind::OpenEnd;
                d.valve = i as u8;
                self.pending[i] = Pending {
                    valid: true,
                    dir: DIR_OPEN,
                    drive: x.drive,
                    keep_status: false,
                    reference: false,
                };
            } else if x.status == ST_OPEN_CIRCUIT && x.drive != x.actual {
                d.kind = ActionKind::Test;
                d.valve = i as u8;
                self.latch[i].valid = false;
            } else if x.status == ST_BLOCKED
                && x.blocked_failsafe
                && !x.svc_hold
                && !any_req(x)
                && x.drive != x.actual
                && self.move_allowed(i, x, move_dir(x))
            {
                self.set_move(d, i, x, true);
            } else if x.status == ST_IDLE
                && !x.svc_hold
                && !pending_cal(x)
                && !any_req(x)
                && x.needs_reference
                && due(x)
            {
                // the start is not known: to the end stop nearer to the drive target first
                let dir = if x.drive >= 50 { DIR_OPEN } else { DIR_CLOSE };
                d.kind = if dir == DIR_OPEN {
                    ActionKind::OpenEnd
                } else {
                    ActionKind::CloseEnd
                };
                d.valve = i as u8;
                d.reference = true;
                self.latch[i].valid = false;
                self.pending[i] = Pending {
                    valid: true,
                    dir,
                    drive: x.drive,
                    keep_status: false,
                    reference: true,
                };
                self.first_change = true;
            } else if x.status == ST_IDLE
                && !x.svc_hold
                && !pending_cal(x)
                && !any_req(x)
                && !x.needs_reference
                && x.drive != x.actual
                && self.move_allowed(i, x, move_dir(x))
            {
                self.set_move(d, i, x, false);
                self.first_change = true;
            } else {
                found = false;
            }
            if found {
                self.rr = ((i + 1) % VALVES) as u8;
                return true;
            }
        }
        false
    }

    fn step3(&mut self, v: &[ValveView; VALVES], d: &mut Decision) -> bool {
        for n in 0..VALVES {
            let i = (usize::from(self.rr) + n) % VALVES;
            let x = &v[i];
            if x.status == ST_PRESENT && (self.first_change || explicit_req(x) || due(x)) {
                d.kind = ActionKind::Learn;
                if x.drive != x.actual {
                    self.first_change = true;
                }
            } else if x.status == ST_IDLE && pending_cal(x) && !x.svc_hold && due(x) {
                d.kind = ActionKind::Learn;
                self.first_change = true;
            } else if any_req(x) && x.status != ST_UNKNOWN && x.status != ST_PRESENT {
                d.kind = ActionKind::MarkPresent;
            } else {
                continue;
            }
            d.valve = i as u8;
            self.latch[i].valid = false;
            self.pending[i].valid = false;
            self.rr = ((i + 1) % VALVES) as u8;
            return true;
        }
        false
    }

    /// The move the last decision for the valve started has ended; status is the
    /// status after the move. early: the classification of the move.
    pub fn move_ended(&mut self, valve: u8, reason: StopReason, early: bool, status: u8) {
        let i = usize::from(valve);
        let Some(slot) = self.pending.get_mut(i) else {
            return;
        };
        let p = *slot;
        slot.valid = false;
        if !p.valid {
            return;
        }
        let l = &mut self.latch[i];
        let end_stop = matches!(
            reason,
            StopReason::EndStop | StopReason::EarlyEndStop | StopReason::SafetyOvercurrent
        );
        if p.reference || !end_stop {
            l.valid = false;
            return;
        }
        if l.valid && l.dir == p.dir && l.drive == p.drive {
            // the retry ended at the end stop again: no further move this way
            l.status = status;
            return;
        }
        *l = Latch {
            valid: true,
            dir: p.dir,
            drive: p.drive,
            status,
            retry: early && !p.keep_status,
        };
    }

    /// a successful calibration of the valve
    pub fn clear_latch(&mut self, valve: u8) {
        if let Some(l) = self.latch.get_mut(usize::from(valve)) {
            l.valid = false;
        }
    }

    pub fn latched(&self, valve: u8) -> bool {
        self.latch.get(usize::from(valve)).is_some_and(|l| l.valid)
    }
}
