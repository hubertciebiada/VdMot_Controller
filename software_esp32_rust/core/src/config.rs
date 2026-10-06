//! Configuration schema of the ESP firmware (port of `vdm/config.h`).
//!
//! For now only `MqttMode`, which `failsafe` needs; the config port fills in the rest of the
//! module (schema, defaults, validation, repair, key-path setter, JSON export, NVS encodings).

/// MQTT mode of the configuration (C++ `enum class MqttMode : uint8_t`).
#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MqttMode {
    #[default]
    Off = 0,
    Mqtt = 1,
    MqttHa = 2,
}
