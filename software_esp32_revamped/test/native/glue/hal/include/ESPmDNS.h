// Fake Arduino-ESP32 2.0.7 ESPmDNS.h: calls recorded in fakes::net() (mdnsBegins, mdnsServices).
#pragma once

#include <stdint.h>

#include "WString.h"

class MDNSResponder {
 public:
  bool begin(const char* hostName);
  void end();
  bool addService(char* service, char* proto, uint16_t port);
  bool addService(const char* service, const char* proto, uint16_t port) {
    return addService(const_cast<char*>(service), const_cast<char*>(proto), port);
  }
  bool addService(String service, String proto, uint16_t port) {
    return addService(service.c_str(), proto.c_str(), port);
  }
};

extern MDNSResponder MDNS;
