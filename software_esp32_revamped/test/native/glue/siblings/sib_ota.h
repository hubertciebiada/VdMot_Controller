// Scripted results and recorded calls of the sibling fake of src/ota.cpp
// (siblings/fake_ota.cpp); both files go with src/ota.cpp. See siblings.h.
#pragma once

#include <stddef.h>
#include <stdint.h>

#include <string>
#include <utility>
#include <vector>

#include <vdm/json_api.h>

#include "ota.h"

namespace sib {

struct RestartRequest {
  uint8_t reason;
  uint32_t delayMs;
  int32_t detail;
};
struct OtaService {
  uint32_t nowMs;
  bool netOk;
  bool linkUp;
  bool webStarted;
};
struct OtaRestartService {
  uint32_t nowMs;
  bool netUp;
  bool linkUp;
};
struct Ota {
  // scripted
  bool restartPending = false;  // set by requestRestart()
  bool uploadActive = false;
  bool uploadBeginResult = true;
  bool uploadWriteResult = true;
  bool uploadEndResult = true;
  std::string uploadError = "unknown";
  vdm::OtaHealthInfo health;
  // recorded
  int begins = 0;
  std::vector<RestartRequest> restartRequests;
  std::vector<std::pair<size_t, std::string>> uploadBegins;  // announced bytes, md5
  std::string uploadData;
  std::vector<bool> uploadEnds;  // commit flags
  std::vector<OtaService> services;
  std::vector<OtaRestartService> serviceRestarts;
};
Ota& ota();

}  // namespace sib
