// New cases (no C++ counterpart: the C++ masks every interrupt with __disable_irq and has no
// misuse checks): the priorities of STM32duino, which contexts keep the USART interrupt out
// (the firmware checked it with a predicate that was inverted once) and the access protocol of
// the firmware's IsrCell against a model of BASEPRI.

use std::cell::{Cell, RefCell};
use std::vec;
use std::vec::Vec;

use super::*;
use crate::test_support::fake_board::{expect_panic, Fault};

// ---- priorities

#[test]
fn the_priorities_are_the_stm32duino_levels_in_the_upper_four_bits() {
    assert_eq!(PRIO_USART, 0x10);
    assert_eq!(PRIO_EXTI, 0x60);
    assert_eq!(PRIO_MOTOR, 0xE0);
    // a lower value preempts: the USARTs preempt EXTI4, EXTI4 preempts the timers
    const { assert!(PRIO_USART < PRIO_EXTI && PRIO_EXTI < PRIO_MOTOR) };
}

// ---- blocked

struct M {
    primask: bool,
    basepri: u8,
}

impl Masks for M {
    fn primask(&self) -> bool {
        self.primask
    }

    fn basepri(&self) -> u8 {
        self.basepri
    }
}

fn masks(primask: bool, basepri: u8) -> M {
    M { primask, basepri }
}

#[test]
fn nothing_masked_blocks_no_interrupt() {
    for prio in [0, PRIO_USART, PRIO_EXTI, PRIO_MOTOR, 0xF0] {
        assert!(!blocked(&masks(false, 0), prio), "{prio:#x}");
    }
}

#[test]
fn primask_blocks_every_interrupt_whatever_basepri_says() {
    for basepri in [0, PRIO_USART, PRIO_MOTOR] {
        for prio in [0, PRIO_USART, PRIO_MOTOR] {
            assert!(blocked(&masks(true, basepri), prio));
        }
    }
}

#[test]
fn basepri_masks_its_own_level_and_every_lower_one() {
    // the motor lock: the timers wait, EXTI4 and the USARTs go on
    assert!(blocked(&masks(false, PRIO_MOTOR), PRIO_MOTOR));
    assert!(!blocked(&masks(false, PRIO_MOTOR), PRIO_EXTI));
    assert!(!blocked(&masks(false, PRIO_MOTOR), PRIO_USART));
    // BASEPRI at the USART level masks the USARTs and everything below them
    assert!(blocked(&masks(false, PRIO_USART), PRIO_USART));
    assert!(blocked(&masks(false, PRIO_USART), PRIO_EXTI));
    assert!(blocked(&masks(false, PRIO_USART), PRIO_MOTOR));
    // one level below the USARTs leaves them running
    assert!(!blocked(&masks(false, PRIO_USART + 0x10), PRIO_USART));
    // the highest level masks every interrupt
    assert!(blocked(&masks(false, 0x01), PRIO_USART));
}

// ---- the cell

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    Read,
    Raise(u8),
    Restore(u8),
    Fault,
    /// the closure ran, with this BASEPRI
    Body(u8),
}

/// BASEPRI with the MSR BASEPRI_MAX rule (it only ever masks more) and the calls in order.
struct Core {
    basepri: Cell<u8>,
    ops: RefCell<Vec<Op>>,
}

impl Core {
    fn new(basepri: u8) -> Self {
        Core {
            basepri: Cell::new(basepri),
            ops: RefCell::new(Vec::new()),
        }
    }

    fn op(&self, op: Op) {
        self.ops.borrow_mut().push(op);
    }

    fn body(&self) {
        self.op(Op::Body(self.basepri.get()));
    }

    fn ops(&self) -> Vec<Op> {
        self.ops.borrow().clone()
    }
}

impl CellCore for Core {
    fn basepri(&self) -> u8 {
        self.op(Op::Read);
        self.basepri.get()
    }

    fn raise(&self, level: u8) {
        self.op(Op::Raise(level));
        let now = self.basepri.get();
        if level != 0 && (now == 0 || level < now) {
            self.basepri.set(level);
        }
    }

    fn restore(&self, level: u8) {
        self.op(Op::Restore(level));
        self.basepri.set(level);
    }

    fn fault(&self) -> ! {
        self.op(Op::Fault);
        std::panic::panic_any(Fault)
    }
}

/// Runs `f`; true when it ended in the fault handler.
fn faults(f: impl FnOnce()) -> bool {
    expect_panic::<Fault>(f)
}

#[test]
fn init_stores_once_and_a_second_init_is_a_fault_that_stores_nothing() {
    let core = Core::new(0);
    let guard = CellGuard::new();
    let stored = Cell::new(0);
    guard.init(&core, || stored.set(stored.get() + 1));
    assert_eq!(stored.get(), 1);
    assert!(faults(|| guard.init(&core, || stored.set(stored.get() + 1))));
    assert_eq!(stored.get(), 1);
    // init leaves BASEPRI alone
    assert_eq!(core.ops(), vec![Op::Fault]);
}

#[test]
fn a_handler_before_init_is_a_fault_and_never_reaches_the_value() {
    let core = Core::new(0);
    let guard = CellGuard::default();
    let reached = Cell::new(false);
    assert!(faults(|| guard.isr(&core, || reached.set(true))));
    assert!(!reached.get());
    assert_eq!(core.ops(), vec![Op::Fault]);
}

#[test]
fn a_handler_after_init_reaches_the_value_without_touching_basepri() {
    let core = Core::new(0);
    let guard = CellGuard::new();
    guard.init(&core, || {});
    assert_eq!(guard.isr(&core, || 42), 42);
    assert_eq!(core.ops(), Vec::<Op>::new());
    // a handler may run while the main loop is between two locks, and again
    assert_eq!(guard.isr(&core, || 7), 7);
}

#[test]
fn the_lock_masks_the_timers_runs_the_body_and_restores_the_callers_level() {
    let core = Core::new(0);
    let guard = CellGuard::new();
    guard.init(&core, || {});
    assert_eq!(
        guard.lock(&core, || {
            core.body();
            5
        }),
        5
    );
    assert_eq!(
        core.ops(),
        vec![
            Op::Read,
            Op::Raise(PRIO_MOTOR),
            Op::Body(PRIO_MOTOR),
            Op::Restore(0)
        ]
    );
    assert_eq!(core.basepri.get(), 0);
}

#[test]
fn a_lock_from_a_context_that_masks_more_keeps_that_level() {
    // e.g. a caller with BASEPRI at the USART level: the lock never lowers it
    let core = Core::new(PRIO_USART);
    let guard = CellGuard::new();
    guard.init(&core, || {});
    guard.lock(&core, || core.body());
    assert_eq!(
        core.ops(),
        vec![
            Op::Read,
            Op::Raise(PRIO_MOTOR),
            Op::Body(PRIO_USART),
            Op::Restore(PRIO_USART)
        ]
    );
}

#[test]
fn locks_one_after_the_other_each_mask_and_restore() {
    let core = Core::new(0);
    let guard = CellGuard::new();
    guard.init(&core, || {});
    guard.lock(&core, || core.body());
    guard.lock(&core, || core.body());
    let ops = core.ops();
    assert_eq!(ops.len(), 8);
    assert_eq!(ops[4..], ops[..4]);
}

#[test]
fn a_lock_before_init_is_a_fault_with_the_timers_masked_and_no_body() {
    let core = Core::new(0);
    let guard = CellGuard::new();
    assert!(faults(|| guard.lock(&core, || core.body())));
    assert_eq!(core.ops(), vec![Op::Read, Op::Raise(PRIO_MOTOR), Op::Fault]);
}

#[test]
fn a_nested_lock_is_a_fault() {
    let core = Core::new(0);
    let guard = CellGuard::new();
    guard.init(&core, || {});
    assert!(faults(
        || guard.lock(&core, || guard.lock(&core, || core.body()))
    ));
    assert_eq!(
        core.ops(),
        vec![
            Op::Read,
            Op::Raise(PRIO_MOTOR),
            Op::Read,
            Op::Raise(PRIO_MOTOR),
            Op::Fault
        ]
    );
}

#[test]
fn the_guard_of_a_handler_checks_only_the_init() {
    // the timers cannot run while a lock is open: that is BASEPRI's part, not the guard's
    let core = Core::new(0);
    let guard = CellGuard::new();
    guard.init(&core, || {});
    assert_eq!(guard.lock(&core, || guard.isr(&core, || 3)), 3);
}
