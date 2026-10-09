//! VdMot Revamped STM32 core: the hardware-free logic of the firmware, a port of
//! `software_stm32/lib/core` (C++17). Every module keeps the name, the contract and the
//! behaviour of its C++ original; `docs/revamped/PROTOCOL_V2.md` stays binding.
//! No heap, no unsafe, fixed-size storage, every input bounded (docs/rust/PORTING.md).
//!
//! Build switches: no -D flag of software_stm32/platformio.ini reaches lib/core (it has no
//! `#if`); FIRMWARE_VERSION, HARDWARE_REVISION_C1/C2, commDebug and appDebug belong to the
//! glue. The one switch of the core is the constant [`protection_guard::PROTECT_ENFORCE`]
//! (C++ `vdm::kProtectEnforce`, default false).
#![no_std]

#[cfg(test)]
extern crate std;

pub mod arg_parser;
pub mod buf_writer;
pub mod calibration;
pub mod config_blocks;
pub mod config_store;
pub mod eeprom_layout;
pub mod end_stop_detector;
pub mod failsafe;
pub mod fault_retry;
pub mod lease;
pub mod legacy_layout;
pub mod line_assembler;
pub mod manual_enable;
pub mod motor_params;
pub mod move_classifier;
pub mod onewire_check;
pub mod presence_test;
pub mod profile_recorder;
pub mod protection_guard;
pub mod replies;
pub mod replies_v2;
pub mod replies_v3;
pub mod reset_guard;
pub mod retry_backoff;
pub mod settings;
pub mod stall_detector;
pub mod store_scheduler;
pub mod system_stats;
pub mod target_rejection;
pub mod temp_filter;
pub mod temp_refresh;
pub mod tokenizer;
pub mod uart_errors;
pub mod valve_codes;
pub mod valve_scheduler;
pub mod warm_state;

#[cfg(test)]
mod test_support;
