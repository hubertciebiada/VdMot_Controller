//! VdMot Revamped STM32 boot stage: the logic that runs after every reset before any other
//! code (docs/rust/GLUE-DESIGN-STM.md §5): reset capture, the boot clock, the boot window in
//! which the ESP starts an update (`DEADBEEF` -> `BEEFIT` -> ROM bootloader) and the layout
//! of the ID block the ESP flasher checks. Port of `software_stm32/src/otasupport.cpp` and
//! `sysstat_capture_reset` (`src/sysstat.cpp`).
//!
//! Requirement (operator): flashing never forces anyone to open the cabinet. The ESP drives
//! NRST but not BOOT0, so this window is the only remote way into the ROM bootloader.
//!
//! B3: the boot stage cannot panic. The crate denies indexing, unwrap, expect, panic and
//! unchecked arithmetic (Cargo.toml lints); the firmware's `boot-probe` build links the boot
//! stage with a panic handler that does not exist, so a panic path fails the link.
//! No `static`, no heap, no unsafe: the firmware owns the register implementation
//! (`firmware/src/boot_hw.rs`), whose calls are single register accesses; every wait, its
//! bound and every branch of the boot stage are here (docs/rust/GLUE-DESIGN-STM.md §1.1).
#![no_std]

#[cfg(test)]
extern crate std;

pub mod app_check;
pub mod capture;
pub mod fault_record;
pub mod fifo;
pub mod gpio;
pub mod id_block;
pub mod io;
pub mod jump;
pub mod poll;
pub mod stage;
pub mod uart;
pub mod watchdog;
pub mod window;

pub use capture::{capture_reset, ResetInfo, NOINIT_LEN};
pub use id_block::BootId;
pub use io::{BootHw, BootIo, ClockIo, FlashRead, IwdgIo, JumpIo};
pub use stage::{boot, run, BootEnd, BootToken};
pub use window::WindowEnd;

#[cfg(test)]
mod test_support;
