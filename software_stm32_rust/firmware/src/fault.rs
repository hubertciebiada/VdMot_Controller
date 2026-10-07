//! Faults, unexpected interrupts and panics (D4, docs/rust/GLUE-DESIGN-STM.md §5.5): valve
//! outputs off, a `FaultRecord` (vdm_stm_boot::fault_record) in `.uninit`, the IWDG start key,
//! spin until the IWDG resets the chip into the boot stage (B5). The reset reason is the IWDG, as
//! after a C++ hang, so the safe mode (3 watchdog resets within 10 min) counts a fault loop the
//! same way.
//!
//! - before the end of the boot window the IWDG is stopped: the start key runs it with its reset
//!   values (prescaler /4, reload 0xFFF: 512 ms nominal);
//! - from the end of the window on (boot stage, application) the IWDG runs with 8 s and is never
//!   reloaded again.
//!
//! The MPU guard below the stack turns a stack overflow into a fault instead of a silent
//! corruption of `.uninit`. D9: these handlers lie in sector 0 with the boot stage (image check).

use core::mem::MaybeUninit;
use core::ptr::{addr_of, addr_of_mut};

use cortex_m::peripheral::{MPU, SCB};
use cortex_m_rt::{exception, ExceptionFrame};
use stm32_metapac as pac;
use stm32_metapac::gpio;
use vdm_stm_boot::fault_record::{
    FaultRecord, KIND_BUS_FAULT, KIND_HARD_FAULT, KIND_MEM_MANAGE, KIND_NMI, KIND_PANIC,
    KIND_UNEXPECTED, KIND_USAGE_FAULT, RECORD_WORDS,
};

use crate::boot_hw::{iwdg_key, ENA_PORT_A_MASK, ENA_PORT_B_MASK, PSU_PB_MASK};

const IWDG_START: u16 = 0xCCCC;

/// Rust-only record (no C++ counterpart), in cortex-m-rt's `.uninit` outside the C++ cells.
#[link_section = ".uninit.vdm_fault"]
static mut FAULT_RECORD: MaybeUninit<[u32; RECORD_WORDS]> = MaybeUninit::uninit();

#[exception]
unsafe fn HardFault(ef: &ExceptionFrame) -> ! {
    stop(KIND_HARD_FAULT, ef.pc(), ef.lr(), ef.xpsr())
}

#[exception]
unsafe fn NonMaskableInt() -> ! {
    stop(KIND_NMI, 0, 0, 0)
}

#[exception]
unsafe fn MemoryManagement() -> ! {
    stop(KIND_MEM_MANAGE, 0, 0, 0)
}

#[exception]
unsafe fn BusFault() -> ! {
    stop(KIND_BUS_FAULT, 0, 0, 0)
}

#[exception]
unsafe fn UsageFault() -> ! {
    stop(KIND_USAGE_FAULT, 0, 0, 0)
}

#[exception]
unsafe fn DefaultHandler(irqn: i16) -> ! {
    stop(KIND_UNEXPECTED, irqn as u32, 0, 0)
}

/// The panic handler of the images.
#[cfg_attr(feature = "boot-probe", allow(dead_code))]
pub fn on_panic() -> ! {
    stop(KIND_PANIC, 0, 0, 0)
}

/// Spins for good (the end of every fault).
pub fn halt() -> ! {
    loop {
        cortex_m::asm::nop();
    }
}

fn stop(kind: u32, pc: u32, lr: u32, xpsr: u32) -> ! {
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

fn record(kind: u32, pc: u32, lr: u32, xpsr: u32) {
    let p = addr_of_mut!(FAULT_RECORD) as *mut [u32; RECORD_WORDS];
    // SAFETY: the fault handler runs alone (interrupts off, nothing returns); the record is
    // plain words in .uninit, read and written as such
    let old = unsafe { core::ptr::read_volatile(p) };
    // SAFETY: SCB fault status registers at their fixed addresses, read only
    let (cfsr, hfsr, bfar) = unsafe {
        let scb = &*SCB::PTR;
        (scb.cfsr.read(), scb.hfsr.read(), scb.bfar.read())
    };
    let r = FaultRecord {
        kind,
        pc,
        lr,
        xpsr,
        cfsr,
        hfsr,
        bfar,
        count: FaultRecord::next_count(&old),
    };
    // SAFETY: as above
    unsafe { core::ptr::write_volatile(p, r.to_words()) };
}

/// The record of the last fault, if valid (printed by the terminal at start).
#[cfg_attr(feature = "boot-probe", allow(dead_code))]
pub fn last_record() -> Option<FaultRecord> {
    // SAFETY: plain words in .uninit; random after power-on, so only the check decides
    let words =
        unsafe { core::ptr::read_volatile(addr_of!(FAULT_RECORD) as *const [u32; RECORD_WORDS]) };
    FaultRecord::from_words(&words)
}

#[cfg_attr(feature = "boot-probe", allow(dead_code))]
extern "C" {
    /// cortex-m-rt: the lowest address the stack may reach (end of `.uninit`)
    static _stack_end: u32;
}

/// §5.5: 32 bytes without access at the bottom of the stack (MPU region 0, the default map
/// everywhere else). An overflow faults (MemManage, escalated to HardFault; a lockup when the
/// stacking fails) and the IWDG resets the chip instead of corrupting `.uninit`.
#[cfg_attr(feature = "boot-probe", allow(dead_code))]
pub fn stack_guard() {
    let end = addr_of!(_stack_end) as u32;
    let base = end.wrapping_add(31) & !31;
    // SAFETY: MPU registers at their fixed address; region 0 is the only region, the default
    // memory map stays for everything else (PRIVDEFENA)
    unsafe {
        let mpu = &*MPU::PTR;
        mpu.ctrl.write(0);
        mpu.rnr.write(0);
        mpu.rbar.write(base);
        // XN, AP 000 (no access), SIZE 4 (2^5 = 32 bytes), ENABLE
        mpu.rasr.write((1 << 28) | (4 << 1) | 1);
        // PRIVDEFENA, ENABLE
        mpu.ctrl.write(0b101);
    }
    cortex_m::asm::dsb();
    cortex_m::asm::isb();
}
