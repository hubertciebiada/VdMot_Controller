// Scripted results and recorded calls of the sibling fake of src/stm_service.cpp
// (siblings/fake_stm_service.cpp); both files go with src/stm_service.cpp. See siblings.h.
#pragma once

#include <stddef.h>
#include <stdint.h>

#include <vector>

#include <vdm/calib_schedule.h>
#include <vdm/lease_client.h>
#include <vdm/target_store.h>

#include "stm_service.h"

namespace sib {

struct StmService {
  // scripted
  vdm::PersistedTargets bootTargets;
  vdm::RestoreSource bootSource = vdm::RestoreSource::None;
  bool bootLeaseValid = false;
  vdm::LeaseClient::Snapshot bootLease;
  // recorded
  int begins = 0;
  std::vector<uint32_t> services;
  int restartFlushes = 0;
  std::vector<vdm::PersistedTargets> storedTargets;
  std::vector<vdm::LeaseClient::Snapshot> leaseRecords;
  struct CalibResult {
    uint16_t attempt;
    bool ok;
    vdm::CalibFailure reason;
  };
  std::vector<CalibResult> calibResults;
};
StmService& stmService();

}  // namespace sib
