// Fake Arduino-ESP32 2.0.7 WiFiUdp.h: one outgoing packet at a time, sent packets recorded in
// fakes::net().udpSent.
#pragma once

#include <stddef.h>
#include <stdint.h>

#include <string>

#include "IPAddress.h"
#include "Print.h"

class WiFiUDP : public Stream {
 public:
  uint8_t begin(uint16_t port);
  void stop();
  int beginPacket();
  int beginPacket(IPAddress ip, uint16_t port);
  int beginPacket(const char* host, uint16_t port);
  int endPacket();
  size_t write(uint8_t c) override;
  size_t write(const uint8_t* buffer, size_t size) override;
  using Print::write;
  int parsePacket();
  int available() override;
  int read() override;
  int read(unsigned char* buffer, size_t len);
  int read(char* buffer, size_t len) { return read(reinterpret_cast<unsigned char*>(buffer), len); }
  int peek() override;
  void flush() override;

 private:
  bool open_ = false;
  uint32_t ip_ = 0;
  uint16_t port_ = 0;
  std::string data_;
};
