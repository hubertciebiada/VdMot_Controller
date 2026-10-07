//! VdMot Revamped ESP32 core: the hardware-free logic of the firmware, a port of
//! `software_esp32_revamped/lib/core` (C++17). Every module keeps the name, the contract
//! and the behaviour of its C++ original; `software_esp32_revamped/DESIGN.md` stays binding.
//! No heap, no unsafe, fixed-size storage, every input bounded (docs/rust/PORTING.md).
#![no_std]

#[cfg(any(test, feature = "test-support"))]
extern crate std;

pub mod calib_schedule;
pub mod common;
pub mod config;
pub mod event_limiter;
pub mod event_log;
pub mod factory_reset;
pub mod failsafe;
pub mod file_manager;
pub mod ha_discovery;
pub mod health_monitor;
pub mod image_store;
pub mod json_api;
pub mod json_writer;
pub mod lease_client;
pub mod legacy_http;
pub mod legacy_import;
pub mod line_assembler;
pub mod link_policy;
pub mod log_sink;
pub mod mqtt_policy;
pub mod mqtt_topics;
pub mod mqtt_values;
pub mod net_policy;
pub mod net_trial;
pub mod ota_policy;
pub mod poll_planner;
pub mod reset_gate;
pub mod restart_gate;
pub mod stm_codec;
pub mod stm_flasher;
pub mod stm_session;
pub mod stm_types;
pub mod sys_health;
pub mod target_store;
pub mod valve_model;
pub mod version;
pub mod web_guard;

/// Shared test helpers (port of test/native/support): the core tests, and with the feature
/// `test-support` the glue tests (the golden STM replies and the AN3155 simulator of the fake
/// STM). Never part of the firmware.
#[cfg(any(test, feature = "test-support"))]
#[allow(clippy::should_implement_trait)] // the helpers keep the C++ names (`SimLcg::next`)
pub mod test_support;
