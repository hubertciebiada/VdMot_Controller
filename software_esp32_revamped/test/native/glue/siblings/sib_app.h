// Scripted results and recorded calls of the sibling fake of src/app.cpp
// (siblings/fake_app.cpp); both files go with src/app.cpp. See siblings.h.
#pragma once

#include <stddef.h>
#include <stdint.h>

#include <deque>
#include <functional>
#include <vector>

#include <vdm/json_api.h>
#include <vdm/stm_types.h>

#include "app.h"

namespace sib {

struct App {
  // scripted
  std::function<void()> onNowMs;      // runs at the start of every nowMs() (time passing)
  int64_t uptimeS = -1;               // -1: esp_timer_get_time() / 1 s
  bool submitResult = true;           // false: the queue is full
  std::deque<app::Command> toReceive;  // receive() hands these out in order
  vdm::StmSnapshot snapshot;          // readStmSnapshot()
  uint32_t snapshotRevision = 0;
  vdm::LinkState link = vdm::LinkState::Unknown;
  bool flashActive = false;
  uint8_t proto = 0;
  vdm::StmSupport support = vdm::StmSupport::Unknown;
  vdm::StmSaveState saveState = vdm::StmSaveState::Idle;
  app::CalibInfo calib;
  vdm::HealthSnapshot health;
  // recorded
  std::vector<app::Command> submitted;
  vdm::StmSnapshot published;         // the last publishStmSnapshot()
  int publishes = 0;
  int setupCalls = 0;
  int saveRequests = 0;
  std::vector<vdm::StmSaveState> saveStates;
  int flashMarks = 0;
  std::vector<app::CalibInfo> calibInfos;
  int healthReads = 0;
};
App& app();

}  // namespace sib
