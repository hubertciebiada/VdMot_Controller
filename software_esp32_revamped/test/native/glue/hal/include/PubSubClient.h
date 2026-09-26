// Fake knolleary/PubSubClient 2.8 (ESP32 build: std::function callback). The broker is
// fakes::mqtt(): connect() records every argument and answers connectResult; publish() fails when
// not connected, when the message does not fit the buffer (like the library: 5 + 2 + topic +
// payload > bufferSize), at fakes::mqtt().failPublishAt or to failPublishTopic (once); loop()
// delivers one message of fakes::mqtt().inbox per call through the callback. The callback gets
// heap copies of topic and payload (exact size); a publish inside the callback frees them like
// the library's shared buffer overwrites them, so a later use is an ASan error.
#pragma once

#include <stddef.h>
#include <stdint.h>

#include <functional>

#include "Arduino.h"
#include "Client.h"
#include "IPAddress.h"

#define MQTT_VERSION_3_1 3
#define MQTT_VERSION_3_1_1 4
#define MQTT_VERSION MQTT_VERSION_3_1_1
#define MQTT_MAX_PACKET_SIZE 256
#define MQTT_KEEPALIVE 15
#define MQTT_SOCKET_TIMEOUT 15

#define MQTT_CONNECTION_TIMEOUT -4
#define MQTT_CONNECTION_LOST -3
#define MQTT_CONNECT_FAILED -2
#define MQTT_DISCONNECTED -1
#define MQTT_CONNECTED 0
#define MQTT_CONNECT_BAD_PROTOCOL 1
#define MQTT_CONNECT_BAD_CLIENT_ID 2
#define MQTT_CONNECT_UNAVAILABLE 3
#define MQTT_CONNECT_BAD_CREDENTIALS 4
#define MQTT_CONNECT_UNAUTHORIZED 5

#define MQTT_MAX_HEADER_SIZE 5

#define MQTTQOS0 (0 << 1)
#define MQTTQOS1 (1 << 1)
#define MQTTQOS2 (2 << 1)

#define MQTT_CALLBACK_SIGNATURE std::function<void(char*, uint8_t*, unsigned int)> callback

class PubSubClient : public Print {
 public:
  PubSubClient();
  explicit PubSubClient(Client& client);
  ~PubSubClient();

  PubSubClient& setServer(IPAddress ip, uint16_t port);
  PubSubClient& setServer(uint8_t* ip, uint16_t port);
  PubSubClient& setServer(const char* domain, uint16_t port);
  PubSubClient& setCallback(MQTT_CALLBACK_SIGNATURE);
  PubSubClient& setClient(Client& client);
  PubSubClient& setKeepAlive(uint16_t keepAlive);
  PubSubClient& setSocketTimeout(uint16_t timeout);
  boolean setBufferSize(uint16_t size);
  uint16_t getBufferSize();

  boolean connect(const char* id);
  boolean connect(const char* id, const char* user, const char* pass);
  boolean connect(const char* id, const char* willTopic, uint8_t willQos, boolean willRetain,
                  const char* willMessage);
  boolean connect(const char* id, const char* user, const char* pass, const char* willTopic,
                  uint8_t willQos, boolean willRetain, const char* willMessage);
  boolean connect(const char* id, const char* user, const char* pass, const char* willTopic,
                  uint8_t willQos, boolean willRetain, const char* willMessage,
                  boolean cleanSession);
  void disconnect();
  boolean publish(const char* topic, const char* payload);
  boolean publish(const char* topic, const char* payload, boolean retained);
  boolean publish(const char* topic, const uint8_t* payload, unsigned int plength);
  boolean publish(const char* topic, const uint8_t* payload, unsigned int plength,
                  boolean retained);
  boolean subscribe(const char* topic);
  boolean subscribe(const char* topic, uint8_t qos);
  boolean unsubscribe(const char* topic);
  boolean loop();
  boolean connected();
  int state();

  size_t write(uint8_t) override;
  size_t write(const uint8_t* buffer, size_t size) override;

 private:
  boolean publishBytes(const char* topic, const uint8_t* payload, unsigned int plength,
                       boolean retained);
  void releaseDelivery();

  Client* client_ = nullptr;
  MQTT_CALLBACK_SIGNATURE;
  uint16_t bufferSize_ = MQTT_MAX_PACKET_SIZE;  // the library allocates this in the constructor
  char* deliveryTopic_ = nullptr;      // heap copies handed to the running callback
  uint8_t* deliveryPayload_ = nullptr;
};
