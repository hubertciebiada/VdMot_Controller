// Sibling fake of src/net.cpp (see siblings.h).
#include "fakes/fakes.h"
#include "net.h"
#include "siblings.h"

namespace net {

void begin(vdm::Config& cfg) {
  sib::Net& n = sib::net();
  ++n.begins;
  fakes::note("net.begin");
  if (n.onBegin) n.onBegin(cfg);
  n.begunWith = cfg;
}

void service(uint32_t nowMs, bool mqttConnected) {
  sib::net().services.push_back({nowMs, mqttConnected});
  fakes::note("net.service " + std::to_string(nowMs));
}

bool isUp() { return sib::net().up; }

Info info() { return sib::net().info; }

vdm::NetHealthInfo health(uint32_t) { return sib::net().health; }

bool otaNetOk() { return sib::net().otaNetOk; }

const char* hostname() { return sib::net().hostname.c_str(); }

void noteInboundHttp(uint32_t remoteIp) { sib::net().inboundHttp.push_back(remoteIp); }

bool requestTrialConfirm() {
  ++sib::net().trialConfirms;
  return sib::net().trialConfirmResult;
}

bool requestTrialRevert() {
  ++sib::net().trialReverts;
  return sib::net().trialRevertResult;
}

TrialInfo trialInfo() { return sib::net().trial; }

vdm::LocalTime localTime() { return sib::net().localTime; }

bool timeValid() { return sib::net().timeValid; }

uint32_t lastSyncEpoch() { return sib::net().lastSyncEpoch; }

void reconfigure(const vdm::Config& cfg) {
  sib::net().reconfigures.push_back(cfg);
  fakes::note("net.reconfigure");
}

}  // namespace net
