// Fake Arduino-ESP32 2.0.7 IPAddress.h. Same uint32 layout as the real one: first octet in the
// low byte (IPAddress(192, 168, 1, 2) == 0x0201A8C0), what vdm::parseIpv4/formatIpv4 use.
#pragma once

#include <stdint.h>

#include "WString.h"

class IPAddress {
 public:
  IPAddress() { address_.dword = 0; }
  IPAddress(uint8_t first, uint8_t second, uint8_t third, uint8_t fourth) {
    address_.bytes[0] = first;
    address_.bytes[1] = second;
    address_.bytes[2] = third;
    address_.bytes[3] = fourth;
  }
  IPAddress(uint32_t address) { address_.dword = address; }
  IPAddress(const uint8_t* address) {
    for (int i = 0; i < 4; ++i) address_.bytes[i] = address[i];
  }

  bool fromString(const char* address);
  bool fromString(const String& address) { return fromString(address.c_str()); }

  operator uint32_t() const { return address_.dword; }
  bool operator==(const IPAddress& addr) const { return address_.dword == addr.address_.dword; }
  bool operator!=(const IPAddress& addr) const { return address_.dword != addr.address_.dword; }
  bool operator==(const uint8_t* addr) const {
    for (int i = 0; i < 4; ++i) {
      if (address_.bytes[i] != addr[i]) return false;
    }
    return true;
  }
  uint8_t operator[](int index) const { return address_.bytes[index]; }
  uint8_t& operator[](int index) { return address_.bytes[index]; }
  IPAddress& operator=(uint32_t address) {
    address_.dword = address;
    return *this;
  }

  String toString() const;

 private:
  union {
    uint8_t bytes[4];
    uint32_t dword;
  } address_;
};

extern const IPAddress INADDR_NONE;
