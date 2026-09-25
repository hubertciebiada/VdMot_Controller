// Fake Arduino-ESP32 2.0.7 HardwareSerial.h. The ports are fakes::serial(0..2): received bytes
// carry the fake time they arrive (a byte is available once the clock reached it), written bytes
// go to fakes::serial(n).tx and to an attached fakes::SerialPeer (e.g. the fake STM).
#pragma once

#include <stddef.h>
#include <stdint.h>

#include "Print.h"

#define SERIAL_5N1 0x8000010
#define SERIAL_6N1 0x8000014
#define SERIAL_7N1 0x8000018
#define SERIAL_8N1 0x800001c
#define SERIAL_5E1 0x8000012
#define SERIAL_6E1 0x8000016
#define SERIAL_7E1 0x800001a
#define SERIAL_8E1 0x800001e
#define SERIAL_8O1 0x800001f
#define SERIAL_8N2 0x800003c
#define SERIAL_8E2 0x800003e

class HardwareSerial : public Stream {
 public:
  explicit HardwareSerial(int uartNr) : uartNr_(uartNr) {}

  void begin(unsigned long baud, uint32_t config = SERIAL_8N1, int8_t rxPin = -1,
             int8_t txPin = -1, bool invert = false, unsigned long timeout_ms = 20000UL,
             uint8_t rxfifo_full_thrhd = 112);
  void end(bool fullyTerminate = true);
  // Like the driver: only before begin() (0 while the port is open) and above the 128 B FIFO.
  size_t setRxBufferSize(size_t newSize);
  size_t setTxBufferSize(size_t newSize);

  int available() override;
  int availableForWrite() override;
  int peek() override;
  int read() override;
  size_t read(uint8_t* buffer, size_t size);
  size_t read(char* buffer, size_t size) { return read(reinterpret_cast<uint8_t*>(buffer), size); }
  void flush() override {}
  void flush(bool) {}
  size_t write(uint8_t c) override;
  size_t write(const uint8_t* buffer, size_t size) override;
  using Print::write;
  uint32_t baudRate();
  operator bool() const;

  int num() const { return uartNr_; }

 private:
  int uartNr_;
};

extern HardwareSerial Serial;
extern HardwareSerial Serial1;
extern HardwareSerial Serial2;
