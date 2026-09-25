// Fake Arduino-ESP32 2.0.7 Update.h: a running/not running state machine with scripted results
// and recorded calls (fakes::ota().update). A successful end() marks the next OTA partition as the
// new boot image (fakes::ota(): state NEW, pending verification after the next boot).
#pragma once

#include <stddef.h>
#include <stdint.h>

#include "Print.h"

#define UPDATE_ERROR_OK (0)
#define UPDATE_ERROR_WRITE (1)
#define UPDATE_ERROR_ERASE (2)
#define UPDATE_ERROR_READ (3)
#define UPDATE_ERROR_SPACE (4)
#define UPDATE_ERROR_SIZE (5)
#define UPDATE_ERROR_STREAM (6)
#define UPDATE_ERROR_MD5 (7)
#define UPDATE_ERROR_MAGIC_BYTE (8)
#define UPDATE_ERROR_ACTIVATE (9)
#define UPDATE_ERROR_NO_PARTITION (10)
#define UPDATE_ERROR_BAD_ARGUMENT (11)
#define UPDATE_ERROR_ABORT (12)

#define UPDATE_SIZE_UNKNOWN 0xFFFFFFFF

#define U_FLASH 0
#define U_SPIFFS 100
#define U_AUTH 200

#define ENCRYPTED_BLOCK_SIZE 16

class UpdateClass {
 public:
  bool begin(size_t size = UPDATE_SIZE_UNKNOWN, int command = U_FLASH, int ledPin = -1,
             uint8_t ledOn = 0, const char* label = nullptr);
  size_t write(uint8_t* data, size_t len);
  bool end(bool evenIfRemaining = false);
  void abort();
  const char* errorString();
  bool setMD5(const char* expected_md5);
  uint8_t getError();
  void clearError();
  bool hasError();
  bool isRunning();
  bool isFinished();
  size_t size();
  size_t progress();
  size_t remaining();
};

extern UpdateClass Update;
