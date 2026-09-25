// Sibling fake of src/stm_service.cpp (see siblings.h).
#include "fakes/fakes.h"
#include "siblings.h"
#include "stm_service.h"

namespace stm_service {

void begin() {
  ++sib::stmService().begins;
  fakes::note("stm_service.begin");
}

void service(uint32_t nowMs) {
  sib::stmService().services.push_back(nowMs);
  fakes::note("stm_service.service " + std::to_string(nowMs));
}

void flushForRestart() {
  ++sib::stmService().restartFlushes;
  fakes::note("stm_service.flushForRestart");
}

void storeDesiredTargets(const vdm::PersistedTargets& t) {
  sib::stmService().storedTargets.push_back(t);
}

void storeLeaseRecord(const vdm::LeaseClient::Snapshot& s) {
  sib::stmService().leaseRecords.push_back(s);
}

void postScheduledCalibResult(uint16_t attempt, bool ok, vdm::CalibFailure reason) {
  sib::stmService().calibResults.push_back({attempt, ok, reason});
  fakes::note("stm_service.postScheduledCalibResult " + std::to_string(attempt) + " " +
              (ok ? "ok" : "failed") + " " + vdm::calibFailureName(reason));
}

const vdm::PersistedTargets& bootTargets(vdm::RestoreSource& src) {
  src = sib::stmService().bootSource;
  return sib::stmService().bootTargets;
}

bool bootLease(vdm::LeaseClient::Snapshot& out) {
  out = sib::stmService().bootLease;
  return sib::stmService().bootLeaseValid;
}

}  // namespace stm_service
