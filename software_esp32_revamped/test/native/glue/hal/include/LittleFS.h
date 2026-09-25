// Fake Arduino-ESP32 2.0.7 LittleFS.h: the mount of the "spiffs" partition (fakes::fs()).
#pragma once

#include <stddef.h>
#include <stdint.h>

#include "FS.h"

namespace fs {

class LittleFSFS : public FS {
 public:
  // Mounts when the partition holds a file system (fakes::fs().formatted) and mountOk is set;
  // formatOnFail formats first like the real begin().
  bool begin(bool formatOnFail = false, const char* basePath = "/littlefs",
             uint8_t maxOpenFiles = 10, const char* partitionLabel = "spiffs");
  void end();
  bool format();
  size_t totalBytes();
  size_t usedBytes();
};

}  // namespace fs

extern fs::LittleFSFS LittleFS;
