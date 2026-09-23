// Stub: contract in vdm/json_api.h; implemented by the core implementer.
#include "vdm/json_api.h"

namespace vdm {

const char* netStateName(NetState) { return ""; }
const char* mqttStateName(MqttState) { return ""; }

bool writeStatusJson(JsonWriter&, const StatusSnapshot&) { return false; }
bool writeValvesJson(JsonWriter&, const ValveView*, uint8_t, uint32_t) { return false; }
bool writeProfileJson(JsonWriter&, const Profile&) { return false; }
bool writeSensorsJson(JsonWriter&, const SensorView*, uint8_t, const SensorView*, uint8_t) {
  return false;
}
bool writeEventsJson(JsonWriter&, const Event*, size_t, uint32_t, uint32_t, uint32_t, uint32_t) {
  return false;
}
bool writeFlashStatusJson(JsonWriter&, const FlashStatus&, const char*) { return false; }
bool writeMotorJson(JsonWriter&, const MotorChars&, uint16_t, const Breakaway*, bool) {
  return false;
}

bool writeErrorJson(JsonWriter& jw, const char* code, const char* detail) {
  jw.beginObject();
  jw.kv("error", code);
  jw.kv("detail", detail);
  jw.endObject();
  return jw.complete();
}

RouteMatch matchApiRoute(HttpMethod, const char*, size_t, bool) { return RouteMatch{}; }

}  // namespace vdm
