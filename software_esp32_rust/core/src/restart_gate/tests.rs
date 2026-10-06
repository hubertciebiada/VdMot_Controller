//! Port of test/native/test_restart_gate.cpp: STM EEPROM save before an ESP restart.

use super::*;
use RestartGateStep as St;

#[test]
fn request_first_proceed_on_every_final_answer() {
    let finals = [
        StmSaveState::Saved,
        StmSaveState::Unavailable,
        StmSaveState::TimedOut,
    ];
    for f in finals {
        let mut g = RestartGate::default();
        assert_eq!(g.waited_ms(500), 0);
        assert_eq!(g.update(f, 1000), St::RequestStmSave); // the answer of an earlier restart counts not
        assert_eq!(g.update(StmSaveState::Waiting, 1100), St::Wait);
        assert_eq!(g.update(StmSaveState::Idle, 1200), St::Wait);
        assert_eq!(g.waited_ms(1200), 200);
        assert_eq!(g.update(f, 1300), St::Proceed);
        assert!(!g.guard_expired());
        assert_eq!(g.update(StmSaveState::Waiting, 1400), St::Proceed); // final
        assert!(!g.guard_expired());
    }
}

#[test]
fn the_guard_proceeds_at_12000_ms_not_11999() {
    let mut g = RestartGate::default();
    let t0: u32 = 0xFFFF_F000;
    assert_eq!(g.update(StmSaveState::Idle, t0), St::RequestStmSave);
    assert_eq!(
        g.update(StmSaveState::Waiting, t0.wrapping_add(11_999)),
        St::Wait
    );
    assert!(!g.guard_expired());
    assert_eq!(
        g.update(StmSaveState::Waiting, t0.wrapping_add(12_000)),
        St::Proceed
    );
    assert!(g.guard_expired());
    assert_eq!(g.waited_ms(t0.wrapping_add(12_000)), 12_000);
    assert_eq!(
        g.update(StmSaveState::Waiting, t0.wrapping_add(12_001)),
        St::Proceed
    );
    assert_eq!(RestartGate::GUARD_MS, 12_000);
}

#[test]
fn reset_starts_over() {
    let mut g = RestartGate::default();
    g.update(StmSaveState::Idle, 0);
    assert_eq!(g.update(StmSaveState::Waiting, 12_000), St::Proceed);
    g.reset();
    assert!(!g.guard_expired());
    assert_eq!(g.waited_ms(20_000), 0);
    assert_eq!(g.update(StmSaveState::Saved, 20_000), St::RequestStmSave);
    assert_eq!(g.update(StmSaveState::Waiting, 20_001), St::Wait);
}
