// Fake Arduino-ESP32 2.0.7 Preferences.h on the NVS fake (nvs.h), with the return values of the
// real class: put* returns the bytes written (0 on failure), get* the default when the key is
// missing or of another type, getBytes() 0 when the buffer is too small.
#pragma once

#include <stddef.h>
#include <stdint.h>

#include "WString.h"
#include "nvs.h"

typedef enum {
  PT_I8,
  PT_U8,
  PT_I16,
  PT_U16,
  PT_I32,
  PT_U32,
  PT_I64,
  PT_U64,
  PT_STR,
  PT_BLOB,
  PT_INVALID
} PreferenceType;

class Preferences {
 public:
  Preferences() = default;
  ~Preferences();

  bool begin(const char* name, bool readOnly = false, const char* partition_label = nullptr);
  void end();

  bool clear();
  bool remove(const char* key);

  size_t putChar(const char* key, int8_t value);
  size_t putUChar(const char* key, uint8_t value);
  size_t putShort(const char* key, int16_t value);
  size_t putUShort(const char* key, uint16_t value);
  size_t putInt(const char* key, int32_t value);
  size_t putUInt(const char* key, uint32_t value);
  size_t putLong(const char* key, int32_t value);
  size_t putULong(const char* key, uint32_t value);
  size_t putLong64(const char* key, int64_t value);
  size_t putULong64(const char* key, uint64_t value);
  size_t putBool(const char* key, bool value);
  size_t putString(const char* key, const char* value);
  size_t putString(const char* key, const String& value) { return putString(key, value.c_str()); }
  size_t putBytes(const char* key, const void* value, size_t len);

  bool isKey(const char* key);
  PreferenceType getType(const char* key);
  int8_t getChar(const char* key, int8_t defaultValue = 0);
  uint8_t getUChar(const char* key, uint8_t defaultValue = 0);
  int16_t getShort(const char* key, int16_t defaultValue = 0);
  uint16_t getUShort(const char* key, uint16_t defaultValue = 0);
  int32_t getInt(const char* key, int32_t defaultValue = 0);
  uint32_t getUInt(const char* key, uint32_t defaultValue = 0);
  int32_t getLong(const char* key, int32_t defaultValue = 0);
  uint32_t getULong(const char* key, uint32_t defaultValue = 0);
  int64_t getLong64(const char* key, int64_t defaultValue = 0);
  uint64_t getULong64(const char* key, uint64_t defaultValue = 0);
  bool getBool(const char* key, bool defaultValue = false);
  size_t getString(const char* key, char* value, size_t maxLen);
  String getString(const char* key, String defaultValue = String());
  size_t getBytesLength(const char* key);
  size_t getBytes(const char* key, void* buf, size_t maxLen);

 private:
  nvs_handle_t handle_ = 0;
  bool started_ = false;
  bool readOnly_ = false;
};
