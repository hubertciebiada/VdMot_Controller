// Fake me-no-dev/AsyncTCP 1.1.1: the client of a web request (remote address from the request
// driver, fakes/http.h) and the listening server.
#pragma once

#include <stdint.h>

#include "IPAddress.h"

class AsyncClient {
 public:
  AsyncClient() = default;
  explicit AsyncClient(uint32_t remoteIp, uint16_t remotePort = 50000)
      : remoteIp_(remoteIp), remotePort_(remotePort) {}

  IPAddress remoteIP() { return IPAddress(remoteIp_); }
  uint16_t remotePort() { return remotePort_; }
  IPAddress localIP() { return IPAddress(localIp_); }
  uint16_t localPort() { return 80; }
  uint32_t getRemoteAddress() { return remoteIp_; }
  bool connected() { return connected_; }
  void close(bool now = false) {
    (void)now;
    connected_ = false;
  }

  uint32_t remoteIp_ = 0;
  uint16_t remotePort_ = 50000;
  uint32_t localIp_ = 0;
  bool connected_ = true;
};

class AsyncServer {
 public:
  explicit AsyncServer(uint16_t port) : port_(port) {}
  void begin() { begun_ = true; }
  void end() { begun_ = false; }
  uint16_t port() const { return port_; }

 private:
  uint16_t port_;
  bool begun_ = false;
};
