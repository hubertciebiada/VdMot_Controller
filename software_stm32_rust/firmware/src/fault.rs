//! Faults, unexpected interrupts and panics (D4, docs/rust/GLUE-DESIGN-STM.md §5.5): valve
//! outputs off, a `FaultRecord` in `.uninit`, the IWDG start key, spin until the IWDG resets
//! the chip into the boot stage (B5). The reset reason is the IWDG, as after a C++ hang, so the
//! safe mode (3 watchdog resets within 10 min) counts a fault loop the same way.
//!
//! - before the application stage the IWDG is stopped: the start key runs it with its reset
//!   values (prescaler /4, reload 0xFFF: 512 ms nominal);
//! - after it the IWDG runs with 8 s and is never reloaded again.
//!
//! D9: these handlers lie in sector 0 with the boot stage (image check).
//! TODO(glue): the MPU stack guard of §5.5 and the terminal print of a valid record.

use core::mem::MaybeUninit;
use core::ptr::{addr_of, addr_of_mut};

use cortex_m::peripheral::SCB;
use cortex_m_rt::{exception, ExceptionFrame};
use stm32_metapac as pac;
use stm32_metapac::gpio;

use crate::boot_hw::{iwdg_key, ENA_PORT_A_MASK, ENA_PORT_B_MASK, PSU_PB_MASK};

/// What stopped the firmware (word 1 of the record).
#[derive(Clone, Copy)]
#[repr(u32)]
#[cfg_attr(feature = "boot-probe", allow(dead_code))]
pub enum Kind {
    HardFault = 1,
    Nmi = 2,
    MemManage = 3,
    BusFault = 4,
    UsageFault = 5,
    /// an interrupt without a handler (the IRQ number is in the PC word)
    Unexpected = 6,
    Panic = 7,
}

/// "VDFR"
const RECORD_MAGIC: u32 = 0x5644_4652;
/// magic, kind, pc, lr, xpsr, cfsr, hfsr, bfar, count, check
const RECORD_WORDS: usize = 10;
const IWDG_START: u16 = 0xCCCC;

/// Rust-only record (no C++ counterpart), in cortex-m-rt's `.uninit` outside the C++ cells.
#[link_section = ".uninit.vdm_fault"]
static mut FAULT_RECORD: MaybeUninit<[u32; RECORD_WORDS]> = MaybeUninit::uninit();

#[exception]
unsafe fn HardFault(ef: &ExceptionFrame) -> ! {
    stop(Kind::HardFault, ef.pc(), ef.lr(), ef.xpsr())
}

#[exception]
unsafe fn NonMaskableInt() -> ! {
    stop(Kind::Nmi, 0, 0, 0)
}

#[exception]
unsafe fn MemoryManagement() -> ! {
    stop(Kind::MemManage, 0, 0, 0)
}

#[exception]
unsafe fn BusFault() -> ! {
    stop(Kind::BusFault, 0, 0, 0)
}

#[exception]
unsafe fn UsageFault() -> ! {
    stop(Kind::UsageFault, 0, 0, 0)
}

#[exception]
unsafe fn DefaultHandler(irqn: i16) -> ! {
    stop(Kind::Unexpected, irqn as u32, 0, 0)
}

/// The panic handler of the images.
#[cfg_attr(feature = "boot-probe", allow(dead_code))]
pub fn on_panic() -> ! {
    stop(Kind::Panic, 0, 0, 0)
}

/// Spins for good (the end of every fault).
pub fn halt() -> ! {
    loop {
        cortex_m::asm::nop();
    }
}

fn stop(kind: Kind, pc: u32, lr: u32, xpsr: u32) -> ! {
    cortex_m::interrupt::disable();
    outputs_off();
    record(kind, pc, lr, xpsr);
    iwdg_key(IWDG_START);
    halt()
}

/// ENA0..5 low and the valve PSU off (PB9 high, open drain); BSRR is atomic and needs no
/// knowledge of the pin modes.
fn outputs_off() {
    pac::GPIOA
        .bsrr()
        .write_value(gpio::regs::Bsrr(ENA_PORT_A_MASK << 16));
    pac::GPIOB
        .bsrr()
        .write_value(gpio::regs::Bsrr((ENA_PORT_B_MASK << 16) | PSU_PB_MASK));
}

fn check(words: &[u32; RECORD_WORDS]) -> u32 {
    !words.iter().take(RECORD_WORDS - 1).fold(0, |x, w| x ^ w)
}

fn record(kind: Kind, pc: u32, lr: u32, xpsr: u32) {
    let p = addr_of_mut!(FAULT_RECORD) as *mut [u32; RECORD_WORDS];
    // SAFETY: the fault handler runs alone (interrupts off, nothing returns); the record is
    // plain words in .uninit, read and written as such
    let old = unsafe { core::ptr::read_volatile(p) };
    let count = if old[0] == RECORD_MAGIC && old[9] == check(&old) {
        old[8].wrapping_add(1)
    } else {
        1
    };
    // SAFETY: SCB fault status registers at their fixed addresses, read only
    let (cfsr, hfsr, bfar) = unsafe {
        let scb = &*SCB::PTR;
        (scb.cfsr.read(), scb.hfsr.read(), scb.bfar.read())
    };
    let mut words = [
        RECORD_MAGIC,
        kind as u32,
        pc,
        lr,
        xpsr,
        cfsr,
        hfsr,
        bfar,
        count,
        0,
    ];
    words[9] = check(&words);
    // SAFETY: as above
    unsafe { core::ptr::write_volatile(p, words) };
}

/// The record of the last fault, if valid (TODO(glue): printed by the terminal at start).
#[allow(dead_code)]
pub fn last_record() -> Option<[u32; RECORD_WORDS]> {
    // SAFETY: plain words in .uninit; random after power-on, so only the check decides
    let words = unsafe { core::ptr::read_volatile(addr_of!(FAULT_RECORD) as *const [u32; RECORD_WORDS]) };
    (words[0] == RECORD_MAGIC && words[9] == check(&words)).then_some(words)
}
