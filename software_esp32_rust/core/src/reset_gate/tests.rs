//! Port of test/native/test_reset_gate.cpp: waiting for the STM EEPROM before a reset or
//! restart.

use super::*;
use ResetGateState as State;

#[test]
fn constants() {
    assert_eq!(ResetGate::POLL_MS, 500);
    assert_eq!(ResetGate::MAX_WAIT_MS, 10_000);
    let mut g = ResetGate::default();
    assert_eq!(g.state(), State::Idle);
    assert_eq!(g.update(100_000), State::Idle);
    assert!(!g.poll_due(0, false));
    assert_eq!(g.waited_ms(5000), 0);
}

#[test]
fn an_stm_that_does_not_answer_is_ready_at_once() {
    let mut g = ResetGate::default();
    g.begin(1000, false);
    assert_eq!(g.state(), State::Ready);
    assert!(!g.poll_due(1000, false));
    assert_eq!(g.update(20_000), State::Ready);
}

#[test]
fn polls_at_once_waits_for_queued_requests_and_the_reply_then_every_500_ms() {
    let mut g = ResetGate::default();
    g.begin(1000, true);
    assert_eq!(g.state(), State::Waiting);
    assert!(g.poll_due(1000, false));
    assert!(!g.poll_due(1000, true)); // User/Config requests first
    g.on_poll_sent(1000);
    assert!(!g.poll_due(5000, false)); // one eepst at a time
    g.on_eepst(true, false, 1100); // "eepst 0": a write is pending
    assert_eq!(g.state(), State::Waiting);
    assert!(!g.poll_due(1100 + 499, false));
    assert!(g.poll_due(1100 + 500, false));
    assert!(!g.poll_due(1100 + 500, true));
    g.on_poll_sent(1600);
    g.on_eepst(false, false, 2000); // timed out: the next poll 500 ms later
    assert!(!g.poll_due(2499, false));
    assert!(g.poll_due(2500, false));
    g.on_poll_sent(2500);
    g.on_eepst(false, true, 2600); // no answer is never idle
    assert_eq!(g.state(), State::Waiting);
    g.on_poll_sent(3100);
    g.on_eepst(true, true, 3200);
    assert_eq!(g.state(), State::Ready);
    assert!(!g.poll_due(10_000, false));
    assert_eq!(g.update(1000 + 10_000), State::Ready);
    assert_eq!(g.waited_ms(3200), 2200);
}

#[test]
fn times_out_10_s_after_begin() {
    let mut g = ResetGate::default();
    g.begin(5000, true);
    assert_eq!(g.update(5000 + 9999), State::Waiting);
    assert_eq!(g.update(5000 + 10_000), State::TimedOut);
    assert_eq!(g.waited_ms(5000 + 10_000), 10_000);
    assert!(!g.poll_due(20_000, false));
    g.on_eepst(true, true, 20_000); // a late reply does not change the outcome
    assert_eq!(g.state(), State::TimedOut);
    g.reset();
    assert_eq!(g.state(), State::Idle);
    assert_eq!(g.waited_ms(30_000), 0);
}

#[test]
fn begin_again_restarts_the_wait_and_the_first_poll() {
    let mut g = ResetGate::default();
    g.begin(0, true);
    g.on_poll_sent(0);
    g.begin(20_000, true);
    assert!(g.poll_due(20_000, false));
    assert_eq!(g.update(29_999), State::Waiting);
    g.reset();
    g.begin(40_000, true);
    g.on_poll_sent(40_000);
    g.reset();
    g.begin(50_000, true);
    assert!(g.poll_due(50_000, false)); // reset forgot the poll in flight
}
