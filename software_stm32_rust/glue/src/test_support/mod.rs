//! Test support of the glue suites (port of `software_stm32/test/native/fakes`,
//! `test/native/glue/sim` and the link-seam stubs of `test/native/glue/stubs`). The glue has no
//! globals, so every test builds its own fakes; a reboot is a new state built from the persistent
//! stores.
// without the feature `terminal` its suites and the stubs they use are not built
#![cfg_attr(not(feature = "terminal"), allow(dead_code))]

pub mod eeprom_fake;
pub mod fake_board;
pub mod io_fakes;
pub mod onewire_sim;
pub mod ow_fakes;
pub mod stub_log;
pub mod stubs;
pub mod valve_sim;

mod tests;
