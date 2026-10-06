//! Port of test/native/test_restart_gate__mut.cpp: an expired guard stays final.

use super::*;
use RestartGateStep as St;

#[test]
fn after_the_guard_expired_it_proceeds_even_when_the_clock_reads_the_start_again() {
    let mut g = RestartGate::default();
    assert_eq!(g.update(StmSaveState::Idle, 5000), St::RequestStmSave);
    assert_eq!(
        g.update(StmSaveState::Waiting, 5000 + RestartGate::GUARD_MS),
        St::Proceed
    );
    assert!(g.guard_expired());
    // 2^32 ms later (millis() wrapped): the elapsed time is below the guard again
    assert_eq!(g.update(StmSaveState::Waiting, 5000), St::Proceed);
    assert_eq!(g.update(StmSaveState::Idle, 5001), St::Proceed);
}
