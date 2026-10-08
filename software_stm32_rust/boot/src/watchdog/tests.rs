// Port of the sequence of STM32duino's IWatchdog `begin(8000000)` (`LL_IWDG_Enable`, `set`:
// prescaler and reload for 8 s at LSI_VALUE 32000, write access, PR, RLR, wait for
// `LL_IWDG_IsReady`, `LL_IWDG_ReloadCounter`); the C++ waits for the ready flag without a
// bound, the boot stage with one (B2).
#![allow(clippy::indexing_slicing, clippy::arithmetic_side_effects)]

use std::vec;
use std::vec::Vec;

use super::*;
use crate::poll::SPIN_LIMIT;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    Key(u16),
    Pr(u32),
    Rlr(u32),
    /// a read of SR, true while PVU or RVU is set
    Updating(bool),
}

/// IWDG_SR clears at the poll given (1: the first; 0: never).
struct Iwdg {
    ops: Vec<Op>,
    ready_at: u32,
    polls: u32,
}

impl Iwdg {
    fn new(ready_at: u32) -> Self {
        Iwdg {
            ops: Vec::new(),
            ready_at,
            polls: 0,
        }
    }
}

impl IwdgIo for Iwdg {
    fn iwdg_key(&mut self, key: u16) {
        self.ops.push(Op::Key(key));
    }

    fn iwdg_prescaler(&mut self, pr: u32) {
        self.ops.push(Op::Pr(pr));
    }

    fn iwdg_reload_value(&mut self, rlr: u32) {
        self.ops.push(Op::Rlr(rlr));
    }

    fn iwdg_updating(&mut self) -> bool {
        self.polls += 1;
        let updating = self.polls != self.ready_at;
        self.ops.push(Op::Updating(updating));
        updating
    }
}

#[test]
fn begin_8_s_starts_unlocks_writes_div_64_and_3999_waits_and_reloads() {
    let mut w = Iwdg::new(3);
    start(&mut w);
    assert_eq!(
        w.ops,
        vec![
            Op::Key(0xCCCC),
            Op::Key(0x5555),
            Op::Pr(4),
            Op::Rlr(3999),
            Op::Updating(true),
            Op::Updating(true),
            Op::Updating(false),
            Op::Key(0xAAAA)
        ]
    );
}

#[test]
fn values_already_in_the_lsi_domain_cost_one_poll() {
    let mut w = Iwdg::new(1);
    start(&mut w);
    assert_eq!(w.ops[4..], [Op::Updating(false), Op::Key(KEY_RELOAD)]);
}

#[test]
fn a_status_that_never_clears_ends_the_wait_after_the_bound_and_still_reloads() {
    let mut w = Iwdg::new(0);
    start(&mut w);
    let polls = w.ops.iter().filter(|o| **o == Op::Updating(true)).count();
    assert_eq!(polls, SPIN_LIMIT as usize);
    assert_eq!(w.ops.len(), 4 + SPIN_LIMIT as usize + 1);
    assert_eq!(w.ops.last(), Some(&Op::Key(KEY_RELOAD)));
}

#[test]
fn a_second_start_writes_the_same_sequence() {
    let mut w = Iwdg::new(1);
    start(&mut w);
    let first = w.ops.clone();
    w.polls = 0;
    start(&mut w);
    assert_eq!(w.ops[first.len()..], first[..]);
}

#[test]
fn the_values_are_the_8_s_of_iwatchdog_begin() {
    // STM32duino `set(8000000)`: t = 8 s x LSI_VALUE 32000 = 256000 counts; the first divider
    // 4 << p with 256000 / div <= 4095 is 64 (p = 4), reload 256000 / 64 - 1
    let t: u32 = 8 * 32_000;
    let (p, div) = (0u32..7)
        .map(|p| (p, 4u32 << p))
        .find(|&(_, div)| t / div <= 0x0FFF)
        .unwrap_or((u32::MAX, 0));
    assert_eq!((p, div), (PR_DIV64, 64));
    assert_eq!(t / div - 1, RLR_8S);
    assert_eq!(
        (KEY_ENABLE, KEY_RELOAD, KEY_START),
        (0x5555, 0xAAAA, 0xCCCC)
    );
}
