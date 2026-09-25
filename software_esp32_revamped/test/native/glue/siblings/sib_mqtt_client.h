// Scripted results and recorded calls of the sibling fake of src/mqtt_client.cpp
// (siblings/fake_mqtt_client.cpp); both files go with src/mqtt_client.cpp. See siblings.h.
#pragma once

#include <stddef.h>
#include <stdint.h>

#include <vector>

#include <vdm/common.h>
#include <vdm/failsafe.h>

#include "mqtt_client.h"

namespace sib {

struct Mqtt {
  // scripted
  mqtt::Status status;
  vdm::RegulatorInput regulator;
  bool calibEnded[vdm::kValveCount] = {};
  vdm::LocalTime calibEnd[vdm::kValveCount];
  // recorded
  int begins = 0;
  int tasks = 0;
  int reconnectRequests = 0;
  std::vector<mqtt::DiscoveryAction> discoveryRequests;
};
Mqtt& mqtt();

}  // namespace sib
