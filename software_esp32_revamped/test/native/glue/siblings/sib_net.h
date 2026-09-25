// Scripted results and recorded calls of the sibling fake of src/net.cpp
// (siblings/fake_net.cpp); both files go with src/net.cpp. See siblings.h.
#pragma once

#include <stddef.h>
#include <stdint.h>

#include <functional>
#include <string>
#include <vector>

#include <vdm/config.h>
#include <vdm/json_api.h>

#include "net.h"

namespace sib {

struct NetService {
  uint32_t nowMs;
  bool mqttConnected;
};
struct Net {
  // scripted
  bool up = false;
  net::Info info;
  vdm::NetHealthInfo health;
  bool otaNetOk = false;
  std::string hostname = "VdMot";
  vdm::LocalTime localTime;
  bool timeValid = false;
  uint32_t lastSyncEpoch = 0;
  net::TrialInfo trial;
  bool trialConfirmResult = false;
  bool trialRevertResult = false;
  std::function<void(vdm::Config&)> onBegin;  // begin() may change the config it is given
  // recorded
  int begins = 0;
  vdm::Config begunWith;
  std::vector<NetService> services;
  std::vector<vdm::Config> reconfigures;
  std::vector<uint32_t> inboundHttp;
  int trialConfirms = 0;
  int trialReverts = 0;
};
Net& net();

}  // namespace sib
