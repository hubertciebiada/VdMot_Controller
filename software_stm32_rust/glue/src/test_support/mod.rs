//! Test support of the glue suites (port of `software_stm32/test/native/fakes` and of the
//! link-seam stubs of `test/native/glue/stubs`).
// without the feature `terminal` its suites and the stubs they use are not built
#![cfg_attr(not(feature = "terminal"), allow(dead_code))]

pub mod eeprom_fake;
pub mod io_fakes;
pub mod onewire_sim;
pub mod ow_fakes;
pub mod stub_log;
pub mod stubs;

mod tests;
