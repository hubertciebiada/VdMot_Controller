//! VdMot Revamped STM32 glue: the logic of `software_stm32/src/*.cpp` and of the library code it
//! uses, over the HAL traits of [`hal`] (docs/rust/GLUE-DESIGN-STM.md). Every module keeps the
//! name, the contract and the behaviour of its C++ original; calls into another glue module go
//! through the module's `<Module>Env` trait, the link seam the tests stub. No statics, no heap,
//! no unsafe; the firmware owns the instances.
#![cfg_attr(not(test), no_std)]

pub mod dallas;
pub mod ds2438;
pub mod eeprom;
pub mod eeprom24;
pub mod hal;
pub mod i2c_bus;
pub mod onewire;
pub mod ow_devices;
pub mod print;
pub mod sysstat;

#[cfg(test)]
mod test_support;
