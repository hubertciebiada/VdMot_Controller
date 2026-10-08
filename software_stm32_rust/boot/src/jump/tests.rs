// Port of the order of `JumpToBootloader` (src/boot_jump.cpp: `HAL_RCC_DeInit`, SysTick off,
// `__disable_irq`, MEMRMP, MSP and PC from 0x1FFF0000) against a model of the clock flags; the
// C++ wait loops end on HAL_GetTick timeouts, these on the bound of the polls.
#![allow(
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::panic
)]

use std::vec;
use std::vec::Vec;

use super::*;
use crate::io::ClockIo;
use crate::poll::SPIN_LIMIT;
use crate::test_support::{expect_jump, Jumped};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    HsiOn,
    HsiReady(bool),
    CfgrReset,
    SwsHsi(bool),
    OscillatorsOff,
    TickStop,
    IrqDisable,
    SyscfgOn,
    SystemMemoryAt0,
    Bootload,
}

/// HSIRDY from the poll `hsi_at` on, SWS = HSI from the poll `sws_at` on (1: the first;
/// 0: never).
struct Rcc {
    ops: Vec<Op>,
    hsi_at: u32,
    sws_at: u32,
    hsi_polls: u32,
    sws_polls: u32,
}

impl Rcc {
    fn new(hsi_at: u32, sws_at: u32) -> Self {
        Rcc {
            ops: Vec::new(),
            hsi_at,
            sws_at,
            hsi_polls: 0,
            sws_polls: 0,
        }
    }

    /// The jump, which ends in the fake `bootload`.
    fn jump(&mut self) {
        assert!(
            expect_jump(|| jump(&mut *self)),
            "the jump did not end in bootload"
        );
    }
}

impl ClockIo for Rcc {
    fn tick_start(&mut self, _reload: u32) {
        panic!("SysTick started by the jump");
    }
    fn tick(&mut self) -> bool {
        false
    }
    fn tick_stop(&mut self) {
        self.ops.push(Op::TickStop);
    }
    fn hse_on(&mut self) {
        panic!("HSE on in the jump");
    }
    fn hse_ready(&mut self) -> bool {
        false
    }
    fn hse_off(&mut self) {}
    fn sysclk_select_hse(&mut self) {
        panic!("SYSCLK to HSE in the jump");
    }
    fn sysclk_is_hse(&mut self) -> bool {
        false
    }
}

impl JumpIo for Rcc {
    fn hsi_on(&mut self) {
        self.ops.push(Op::HsiOn);
    }

    fn hsi_ready(&mut self) -> bool {
        self.hsi_polls += 1;
        let ready = self.hsi_polls >= self.hsi_at && self.hsi_at != 0;
        self.ops.push(Op::HsiReady(ready));
        ready
    }

    fn cfgr_reset(&mut self) {
        self.ops.push(Op::CfgrReset);
    }

    fn sysclk_is_hsi(&mut self) -> bool {
        self.sws_polls += 1;
        let hsi = self.sws_polls >= self.sws_at && self.sws_at != 0;
        self.ops.push(Op::SwsHsi(hsi));
        hsi
    }

    fn oscillators_off(&mut self) {
        self.ops.push(Op::OscillatorsOff);
    }

    fn irq_disable(&mut self) {
        self.ops.push(Op::IrqDisable);
    }

    fn syscfg_on(&mut self) {
        self.ops.push(Op::SyscfgOn);
    }

    fn system_memory_at_0(&mut self) {
        self.ops.push(Op::SystemMemoryAt0);
    }

    fn bootload(&mut self) -> ! {
        self.ops.push(Op::Bootload);
        std::panic::panic_any(Jumped)
    }
}

/// The steps after the clock switch, the same in every case.
const TAIL: [Op; 6] = [
    Op::OscillatorsOff,
    Op::TickStop,
    Op::IrqDisable,
    Op::SyscfgOn,
    Op::SystemMemoryAt0,
    Op::Bootload,
];

#[test]
fn the_clocks_go_back_to_hsi_before_systick_interrupts_the_remap_and_the_jump() {
    let mut r = Rcc::new(2, 3);
    r.jump();
    let mut want = vec![
        Op::HsiOn,
        Op::HsiReady(false),
        Op::HsiReady(true),
        Op::CfgrReset,
        Op::SwsHsi(false),
        Op::SwsHsi(false),
        Op::SwsHsi(true),
    ];
    want.extend(TAIL);
    assert_eq!(r.ops, want);
}

#[test]
fn a_running_hsi_and_an_immediate_switch_cost_one_poll_each() {
    let mut r = Rcc::new(1, 1);
    r.jump();
    let mut want = vec![
        Op::HsiOn,
        Op::HsiReady(true),
        Op::CfgrReset,
        Op::SwsHsi(true),
    ];
    want.extend(TAIL);
    assert_eq!(r.ops, want);
}

#[test]
fn an_hsi_that_never_reports_ready_ends_its_wait_after_the_bound_and_the_jump_goes_on() {
    let mut r = Rcc::new(0, 1);
    r.jump();
    let polls = r.ops.iter().filter(|o| **o == Op::HsiReady(false)).count();
    assert_eq!(polls, SPIN_LIMIT as usize);
    let after = 1 + SPIN_LIMIT as usize;
    assert_eq!(r.ops[after..after + 2], [Op::CfgrReset, Op::SwsHsi(true)]);
    assert_eq!(r.ops[after + 2..], TAIL);
}

#[test]
fn a_switch_that_never_completes_ends_its_wait_after_the_bound_and_the_jump_goes_on() {
    let mut r = Rcc::new(1, 0);
    r.jump();
    let polls = r.ops.iter().filter(|o| **o == Op::SwsHsi(false)).count();
    assert_eq!(polls, SPIN_LIMIT as usize);
    assert_eq!(r.ops.len(), 3 + SPIN_LIMIT as usize + TAIL.len());
    assert_eq!(r.ops[3 + SPIN_LIMIT as usize..], TAIL);
}
