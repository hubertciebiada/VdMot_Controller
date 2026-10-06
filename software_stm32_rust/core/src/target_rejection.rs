//! Targets that a failed or blocked valve does not execute (S04). Hardware-free.

/// rejected_target value: no target known (the valve was not driven since its
/// status was reset).
pub const NO_REJECTED_TARGET: u8 = 255;

/// rejected_target holds the last target that is not a new request: the target
/// of the last move or calibration handed to the valve state machine (the
/// caller records it at the handover), the target a failed or blocked valve
/// already stands at, or the last target counted here.
///
/// Called for a failed or blocked valve (whose motor is not driven):
///  - target == actual: the valve is where it is asked to be; nothing is
///    rejected, and the target is recorded, so a later different target
///    counts even if it is the one the fault left behind.
///  - target == rejected_target: already known, nothing changes.
///  - otherwise the target is a change the valve does not execute: it is
///    recorded and counted in cmd_rejected (saturating). Only when no target is
///    known (NO_REJECTED_TARGET) it is taken as the one the fault left behind and
///    only recorded.
///
/// Returns true for a counted change.
pub fn reject_target(
    rejected_target: &mut u8,
    cmd_rejected: &mut u16,
    target: u8,
    actual: u8,
) -> bool {
    if target == actual {
        *rejected_target = target;
        return false;
    }
    if *rejected_target == target {
        return false;
    }

    let change = *rejected_target != NO_REJECTED_TARGET;
    *rejected_target = target;
    if change {
        *cmd_rejected = cmd_rejected.saturating_add(1);
    }
    change
}

#[cfg(test)]
mod tests;
