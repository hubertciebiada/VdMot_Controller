// MQTT task: PubSubClient over WiFiClient (works on ETH and WiFi), LWT on
// <main>status, legacy-compatible publishing (vdm::PublishScheduler),
// target command subscription, HA discovery iteration (one message per loop
// pass), rate-limited events. Owns its own copies of config and snapshots.
#pragma once

#include <stdint.h>

#include <vdm/json_api.h>

namespace mqtt {

// Binding numbers (DESIGN.md "MQTT").
constexpr uint16_t kBufferSize = 1280;        // PubSubClient packet buffer (topic + payload)
constexpr uint16_t kSocketTimeoutS = 5;       // CONNACK / read wait (independent of keepalive)
constexpr uint32_t kConnectTimeoutMs = 3000;  // TCP connect
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
};
Status status();

// Requests from web handlers (async, applied by the MQTT task).
void requestReconnect();
enum class DiscoveryAction : uint8_t { Publish, Delete, DeleteAndPublish };
void requestDiscovery(DiscoveryAction a);

}  // namespace mqtt
