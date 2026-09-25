// Sibling fake of src/ota.cpp (see siblings.h).
#include "fakes/fakes.h"
#include "ota.h"
#include "siblings.h"

namespace ota {

void begin() {
  ++sib::ota().begins;
  fakes::note("ota.begin");
}

void service(uint32_t nowMs, bool netOk, bool linkUp, bool webStarted) {
  sib::ota().services.push_back({nowMs, netOk, linkUp, webStarted});
  fakes::note("ota.service " + std::to_string(nowMs));
}

vdm::OtaHealthInfo health(uint32_t) { return sib::ota().health; }

bool uploadBegin(size_t announcedBytes, const char* md5) {
  sib::Ota& o = sib::ota();
  o.uploadBegins.emplace_back(announcedBytes, md5 != nullptr ? md5 : "");
  o.uploadData.clear();
  if (o.uploadBeginResult) o.uploadActive = true;
  return o.uploadBeginResult;
}

bool uploadWrite(const uint8_t* data, size_t len) {
  sib::Ota& o = sib::ota();
  if (!o.uploadActive) return false;
  if (!o.uploadWriteResult) {
    o.uploadActive = false;  // ota aborts itself
    return false;
  }
  o.uploadData.append(reinterpret_cast<const char*>(data), len);
  return true;
}

bool uploadEnd(bool commit) {
  sib::Ota& o = sib::ota();
  o.uploadEnds.push_back(commit);
  const bool wasActive = o.uploadActive;
  o.uploadActive = false;
  return wasActive && commit && o.uploadEndResult;
}

bool uploadActive() { return sib::ota().uploadActive; }

const char* uploadError() { return sib::ota().uploadError.c_str(); }

void requestRestart(uint8_t reason, uint32_t delayMs, int32_t detail) {
  sib::Ota& o = sib::ota();
  o.restartRequests.push_back({reason, delayMs, detail});
  o.restartPending = true;
  fakes::note("ota.requestRestart " + std::to_string(reason) + " " + std::to_string(delayMs));
}

bool restartPending() { return sib::ota().restartPending; }

void serviceRestart(uint32_t nowMs, bool netUp, bool linkUp) {
  sib::ota().serviceRestarts.push_back({nowMs, netUp, linkUp});
}

}  // namespace ota
