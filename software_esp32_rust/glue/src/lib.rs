//! VdMot Revamped ESP32 glue: the logic of `software_esp32_revamped/src/*.cpp` and of the Arduino
//! libraries it used, over the port traits of [`port`] (docs/rust/GLUE-DESIGN-ESP.md, binding).
//! Every decision lives here and is host-tested against the fakes of `testkit`; the firmware
//! crate only implements the ports and spawns the threads. No global mutable state, no unsafe,
//! no panic on any input, no infallible allocation after boot (`heap`).

pub mod board;
pub mod boot_guard;
pub mod heap;
pub mod http_parse;
pub mod json_body;
pub mod logger;
pub mod mqtt_client;
pub mod mqtt_conn;
pub mod net;
pub mod ota;
pub mod port;

#[cfg(test)]
pub(crate) mod testkit;
