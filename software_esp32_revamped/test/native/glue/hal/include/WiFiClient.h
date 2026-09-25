// Fake Arduino-ESP32 2.0.7 WiFiClient.h. The MQTT client's socket: stop() is recorded
// (fakes::mqtt().netStops) and drops the broker session of the PubSubClient fake. A connection to
// an address is answered by fakes::net().tcpResponder (e.g. a loopback HTTP self-check): the
// request bytes written before the first read go to the responder, its answer is read back.
#pragma once

#include <stddef.h>
#include <stdint.h>

#include <string>

#include "Client.h"
#include "IPAddress.h"

class WiFiClient : public Client {
 public:
  int connect(IPAddress ip, uint16_t port) override;
  int connect(IPAddress ip, uint16_t port, int32_t timeout_ms);
  int connect(const char* host, uint16_t port) override;
  int connect(const char* host, uint16_t port, int32_t timeout_ms);
  size_t write(uint8_t c) override;
  size_t write(const uint8_t* buf, size_t size) override;
  using Print::write;
  int available() override;
  int read() override;
  int read(uint8_t* buf, size_t size) override;
  int peek() override;
  void flush() override;
  void stop() override;
  uint8_t connected() override;
  operator bool() override { return connected() != 0; }
  IPAddress remoteIP() const;
  uint16_t remotePort() const;
  int setTimeout(uint32_t seconds);

 private:
  void respond();
  bool open_ = false;
  bool answered_ = false;
  uint32_t ip_ = 0;
  uint16_t port_ = 0;
  std::string request_;
  std::string reply_;
  size_t replyPos_ = 0;
};
