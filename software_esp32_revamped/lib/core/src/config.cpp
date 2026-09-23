// Stub: contract in vdm/config.h; implemented by the core implementer.
// setDefaults() is real: glue relies on it from the first boot on.
#include "vdm/config.h"

#include "vdm/json_writer.h"

namespace vdm {

void setDefaults(Config& c) { c = Config{}; }

bool validateConfig(const Config&, char* path, size_t pathCap) {
  if (path && pathCap) path[0] = '\0';
  return false;
}

const char* setResultName(SetResult) { return ""; }

SetResult setConfigValue(Config&, const char*, const ConfigValue&, bool) {
  return SetResult::UnknownKey;
}

bool writeConfigJson(JsonWriter&, const Config&) { return false; }

size_t encodeConfig(const Config&, uint8_t*, size_t) { return 0; }

DecodeResult decodeConfig(const uint8_t*, size_t, Config& out) {
  setDefaults(out);
  return DecodeResult::TooShort;
}

uint32_t crc32(const uint8_t*, size_t, uint32_t crc) { return crc; }

}  // namespace vdm
