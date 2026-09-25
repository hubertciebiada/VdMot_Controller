// Sibling fake of src/mqtt_client.cpp (see siblings.h).
#include "fakes/fakes.h"
#include "mqtt_client.h"
#include "siblings.h"

namespace mqtt {

void begin() {
  ++sib::mqtt().begins;
  fakes::note("mqtt.begin");
}

void task(void*) {
  ++sib::mqtt().tasks;
  fakes::note("mqtt.task");
}

Status status() { return sib::mqtt().status; }

vdm::RegulatorInput regulatorState() { return sib::mqtt().regulator; }

bool calibrationEnd(uint8_t valve, vdm::LocalTime& out) {
  const sib::Mqtt& m = sib::mqtt();
  if (valve >= vdm::kValveCount || !m.calibEnded[valve]) return false;
  out = m.calibEnd[valve];
  return true;
}

void requestReconnect() {
  ++sib::mqtt().reconnectRequests;
  fakes::note("mqtt.requestReconnect");
}

void requestDiscovery(DiscoveryAction a) {
  sib::mqtt().discoveryRequests.push_back(a);
  fakes::note("mqtt.requestDiscovery " + std::to_string(static_cast<int>(a)));
}

}  // namespace mqtt
