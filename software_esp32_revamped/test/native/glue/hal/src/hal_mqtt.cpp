// Fake PubSubClient and broker (fakes::mqtt()).
#include <string.h>

#include <string>

#include "PubSubClient.h"
#include "fakes/fakes.h"
#include "hal_internal.h"

namespace fakes {

namespace {

Mqtt g_mqtt;
const void* g_socket = nullptr;  // the Client of the (last constructed) PubSubClient

}  // namespace

Mqtt& mqtt() { return g_mqtt; }

void resetMqttVolatile() { g_mqtt = Mqtt{}; }

void Mqtt::dropConnection() {
  connected = false;
  state = MQTT_CONNECTION_LOST;
  note("mqtt.dropped");
}

std::vector<MqttMessage> Mqtt::publishedTo(const std::string& topic) const {
  std::vector<MqttMessage> out;
  for (const MqttMessage& m : published) {
    if (m.topic == topic) out.push_back(m);
  }
  return out;
}

bool isMqttSocket(const void* client) { return client != nullptr && client == g_socket; }

void mqttSocketStopped() {
  ++g_mqtt.netStops;
  note("mqtt.socket.stop");
  if (g_mqtt.connected) {
    g_mqtt.connected = false;
    g_mqtt.state = MQTT_CONNECTION_LOST;
  }
}

bool mqttSocketConnected() { return g_mqtt.connected; }

}  // namespace fakes

PubSubClient::PubSubClient() = default;

PubSubClient::PubSubClient(Client& client) : client_(&client) { fakes::g_socket = &client; }

PubSubClient::~PubSubClient() { releaseDelivery(); }

PubSubClient& PubSubClient::setServer(IPAddress ip, uint16_t port) {
  fakes::mqtt().host = ip.toString().str();
  fakes::mqtt().port = port;
  return *this;
}

PubSubClient& PubSubClient::setServer(uint8_t* ip, uint16_t port) {
  return setServer(IPAddress(ip), port);
}

PubSubClient& PubSubClient::setServer(const char* domain, uint16_t port) {
  fakes::mqtt().host = domain != nullptr ? domain : "";
  fakes::mqtt().port = port;
  return *this;
}

PubSubClient& PubSubClient::setCallback(MQTT_CALLBACK_SIGNATURE) {
  this->callback = callback;
  fakes::mqtt().hasCallback = static_cast<bool>(callback);
  return *this;
}

PubSubClient& PubSubClient::setClient(Client& client) {
  client_ = &client;
  fakes::g_socket = &client;
  return *this;
}

PubSubClient& PubSubClient::setKeepAlive(uint16_t keepAlive) {
  fakes::mqtt().keepAlive = keepAlive;
  return *this;
}

PubSubClient& PubSubClient::setSocketTimeout(uint16_t timeout) {
  fakes::mqtt().socketTimeout = timeout;
  return *this;
}

boolean PubSubClient::setBufferSize(uint16_t size) {
  ++fakes::mqtt().bufferSizeCalls;
  if (size == 0) return false;
  bufferSize_ = size;
  fakes::mqtt().bufferSize = size;
  return true;
}

uint16_t PubSubClient::getBufferSize() { return bufferSize_; }

boolean PubSubClient::connect(const char* id) {
  return connect(id, nullptr, nullptr, nullptr, 0, false, nullptr, true);
}

boolean PubSubClient::connect(const char* id, const char* user, const char* pass) {
  return connect(id, user, pass, nullptr, 0, false, nullptr, true);
}

boolean PubSubClient::connect(const char* id, const char* willTopic, uint8_t willQos,
                              boolean willRetain, const char* willMessage) {
  return connect(id, nullptr, nullptr, willTopic, willQos, willRetain, willMessage, true);
}

boolean PubSubClient::connect(const char* id, const char* user, const char* pass,
                              const char* willTopic, uint8_t willQos, boolean willRetain,
                              const char* willMessage) {
  return connect(id, user, pass, willTopic, willQos, willRetain, willMessage, true);
}

boolean PubSubClient::connect(const char* id, const char* user, const char* pass,
                              const char* willTopic, uint8_t willQos, boolean willRetain,
                              const char* willMessage, boolean cleanSession) {
  fakes::Mqtt& m = fakes::mqtt();
  ++m.connects;
  m.clientId = id != nullptr ? id : "";
  m.userNull = user == nullptr;
  m.user = user != nullptr ? user : "";
  m.passNull = pass == nullptr;
  m.pass = pass != nullptr ? pass : "";
  m.willTopic = willTopic != nullptr ? willTopic : "";
  m.willQos = willQos;
  m.willRetain = willRetain;
  m.willMessage = willMessage != nullptr ? willMessage : "";
  m.cleanSession = cleanSession;
  fakes::note("mqtt.connect " + m.clientId);
  if (m.connected) return true;  // the library returns at once when already connected
  if (!m.connectResult) {
    m.connected = false;
    m.state = m.failState;
    return false;
  }
  m.connected = true;
  m.state = MQTT_CONNECTED;
  return true;
}

void PubSubClient::disconnect() {
  fakes::Mqtt& m = fakes::mqtt();
  ++m.disconnects;
  fakes::note("mqtt.disconnect");
  m.connected = false;
  m.state = MQTT_DISCONNECTED;
}

boolean PubSubClient::publish(const char* topic, const char* payload) {
  return publish(topic, payload, false);
}

boolean PubSubClient::publish(const char* topic, const char* payload, boolean retained) {
  return publishBytes(topic, reinterpret_cast<const uint8_t*>(payload),
                      payload != nullptr ? static_cast<unsigned>(strlen(payload)) : 0, retained);
}

boolean PubSubClient::publish(const char* topic, const uint8_t* payload, unsigned int plength) {
  return publishBytes(topic, payload, plength, false);
}

boolean PubSubClient::publish(const char* topic, const uint8_t* payload, unsigned int plength,
                              boolean retained) {
  return publishBytes(topic, payload, plength, retained);
}

boolean PubSubClient::publishBytes(const char* topic, const uint8_t* payload,
                                   unsigned int plength, boolean retained) {
  fakes::Mqtt& m = fakes::mqtt();
  const long index = m.publishCalls++;
  // The library builds the message in its one buffer: the callback's topic and payload are gone.
  if (m.inCallback) releaseDelivery();
  if (!m.connected || topic == nullptr) return false;
  const size_t topicLen = strnlen(topic, bufferSize_);
  if (bufferSize_ < MQTT_MAX_HEADER_SIZE + 2 + topicLen + plength) return false;
  if (index == m.failPublishAt) return false;
  m.published.push_back(
      {topic, std::string(reinterpret_cast<const char*>(payload), plength), retained != 0});
  return true;
}

boolean PubSubClient::subscribe(const char* topic) { return subscribe(topic, 0); }

boolean PubSubClient::subscribe(const char* topic, uint8_t qos) {
  fakes::Mqtt& m = fakes::mqtt();
  if (qos > 1 || topic == nullptr) return false;
  if (bufferSize_ < 9 + strnlen(topic, bufferSize_)) return false;
  if (!m.connected || m.failSubscribe) return false;
  m.subscribed.emplace_back(topic, qos);
  return true;
}

boolean PubSubClient::unsubscribe(const char* topic) {
  fakes::Mqtt& m = fakes::mqtt();
  if (!m.connected || topic == nullptr) return false;
  m.unsubscribed.emplace_back(topic);
  return true;
}

boolean PubSubClient::loop() {
  fakes::Mqtt& m = fakes::mqtt();
  ++m.loops;
  if (!m.connected) return false;
  if (!callback) return true;
  do {
    if (m.inbox.empty()) return true;
    const fakes::MqttMessage msg = m.inbox.front();
    m.inbox.pop_front();
    releaseDelivery();
    deliveryTopic_ = new char[msg.topic.size() + 1];
    memcpy(deliveryTopic_, msg.topic.c_str(), msg.topic.size() + 1);
    deliveryPayload_ = new uint8_t[msg.payload.size() > 0 ? msg.payload.size() : 1];
    memcpy(deliveryPayload_, msg.payload.data(), msg.payload.size());
    m.inCallback = true;
    callback(deliveryTopic_, deliveryPayload_, static_cast<unsigned>(msg.payload.size()));
    m.inCallback = false;
    releaseDelivery();
  } while (m.burst);
  return true;
}

void PubSubClient::releaseDelivery() {
  delete[] deliveryTopic_;
  delete[] deliveryPayload_;
  deliveryTopic_ = nullptr;
  deliveryPayload_ = nullptr;
}

boolean PubSubClient::connected() {
  fakes::Mqtt& m = fakes::mqtt();
  return m.connected;
}

int PubSubClient::state() { return fakes::mqtt().state; }

size_t PubSubClient::write(uint8_t) { return 0; }
size_t PubSubClient::write(const uint8_t*, size_t) { return 0; }
