// MQTT task: PubSubClient over WiFiClient (works on ETH and WiFi), LWT on
// <main>status, legacy-compatible publishing (vdm::PublishScheduler), new
// diag topics, target command subscription, HA discovery iteration (one
// message per loop pass), rate-limited events. Owns its own copies of
// config and snapshots.
#pragma once

#include <stdint.h>

#include <vdm/common.h>
#include <vdm/failsafe.h>
#include <vdm/json_api.h>

namespace mqtt {

// Binding numbers (DESIGN.md "MQTT").
constexpr uint16_t kBufferSize = 1792;        // PubSubClient packet buffer (topic + payload;
                                              // discovery payloads up to 1535)
constexpr uint16_t kSocketTimeoutS = 5;       // CONNACK / read wait (independent of keepalive)
constexpr uint32_t kConnectTimeoutMs = 3000;  // TCP connect (WiFiClient default)
constexpr uint32_t kBackoffMinMs = 2000;      // reconnect back-off, doubles per failure
constexpr uint32_t kBackoffMaxMs = 60000;
constexpr uint32_t kDiscoveryPaceMs = 20;     // between discovery messages

void begin();
void task(void* arg);  // app::kMqttTask

struct Status {
  vdm::MqttState state = vdm::MqttState::Disabled;
  int8_t rc = 0;
  uint32_t reconnects = 0;
  uint32_t publishFailures = 0;
  uint32_t commandsRejected = 0;
  uint32_t eventsSuppressed = 0;
  bool discoveryRunning = false;
  char clientId[24] = {0};                         // of the current session
  vdm::HaStatus haStatus = vdm::HaStatus::Unknown;  // last homeassistant/status seen
};
Status status();

// Any task: what the failsafe lease needs to know about the regulator (MQTT
// mode, broker session, Home Assistant status, accepted commands).
vdm::RegulatorInput regulatorState();

// End of the last calibration of `valve` (0-based) seen since boot; false
// when there was none.
bool calibrationEnd(uint8_t valve, vdm::LocalTime& out);

// Requests from web handlers (async, applied by the MQTT task).
void requestReconnect();
enum class DiscoveryAction : uint8_t { Publish, Delete, DeleteAndPublish };
void requestDiscovery(DiscoveryAction a);

}  // namespace mqtt
