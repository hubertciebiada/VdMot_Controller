// Failsafe lease (K1): shared constants, the regulator-alive rule and the
// lease status the STM task publishes. Hardware-free.
//
// The STM drives every valve to its failsafe position when nobody renewed
// its lease for the configured timeout (protocol 3); with an older STM the
// ESP emulates that by pushing the failsafe positions itself.
#pragma once

#include <stdint.h>

namespace vdm {

enum class MqttMode : uint8_t;  // config.h (Off 0, Mqtt 1, MqttHa 2)

constexpr uint8_t kFailsafeHold = 255;            // failsafe position "hold": stay where it is
constexpr uint8_t kFailsafePctDefault = 50;
constexpr uint16_t kFailsafeTimeoutDefaultMin = 60;
constexpr uint16_t kFailsafeTimeoutMinMin = 5;     // == STM kLeaseTimeoutMinMin
constexpr uint16_t kFailsafeTimeoutMaxMin = 1440;  // == STM kLeaseTimeoutMaxMin

// 0 (off) or 5..1440 minutes, the range the STM accepts with slcfg.
bool leaseTimeoutValid(uint32_t minutes);
// 0..100 % or kFailsafeHold.
bool failsafePctValid(uint32_t pct);

enum class LeaseState : uint8_t { Off = 0, Running = 1, Expired = 2 };
const char* leaseStateName(LeaseState s);  // "off","running","expired"; "unknown" out of range

// Who runs the lease: the STM (protocol 3) or the ESP emulation (protocols
// 1/2 with a timeout > 0).
enum class LeaseMode : uint8_t { None = 0, Stm = 1, Emulated = 2 };
const char* leaseModeName(LeaseMode m);  // "none","stm","esp"; "unknown" out of range

// Last status Home Assistant announced on its status topic.
enum class HaStatus : uint8_t { Unknown = 0, Online = 1, Offline = 2 };
const char* haStatusName(HaStatus s);  // "unknown","online","offline"; "unknown" out of range

enum class RegulatorCause : uint8_t { Alive = 0, BrokerDown = 1, HaOffline = 2 };
// "alive","broker_down","ha_offline"; "unknown" out of range
const char* regulatorCauseName(RegulatorCause c);

// What the MQTT task reports about the regulator (mqtt::regulatorState()).
struct RegulatorInput {
  MqttMode mode{};                  // value-initialised = Off
  bool brokerConnected = false;
  HaStatus ha = HaStatus::Unknown;
  uint32_t commandSeq = 0;          // +1 per accepted inbound MQTT command
};
// MQTT off -> Alive (the ESP is the only client); Mqtt -> Alive while the
// broker session is up, else BrokerDown; MqttHa -> BrokerDown without a
// session, HaOffline after an "offline" status, else Alive (Unknown and
// Online count as alive: HA's birth and last will are not always retained).
RegulatorCause regulatorCause(const RegulatorInput& in);

// Failsafe state of one valve in the API and MQTT vocabulary.
enum class FailsafeKind : uint8_t { None = 0, Lease = 1, Blocked = 2 };
const char* failsafeKindName(FailsafeKind k);  // "off","lease","blocked"; "unknown" out of range

struct LeaseStatus {
  LeaseMode mode = LeaseMode::None;  // Stm on protocol 3; Emulated on 1/2 with timeout > 0
  LeaseState state = LeaseState::Off;
  uint32_t remainS = 0;
  uint16_t timeoutMin = 0;           // ESP config value
  uint16_t failsafeMask = 0;         // Stm: gstax field 11; Emulated: valves driven by the ESP
  RegulatorCause regulator = RegulatorCause::BrokerDown;
  uint32_t regulatorLostS = 0;       // 0 while alive
  bool configSynced = false;         // protocol 3: glcfg equals the ESP config
  bool configFailed = false;
  bool configTrusted = true;         // false: ESP runs on default config, nothing is pushed
};

}  // namespace vdm
