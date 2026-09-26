// Sibling fake of src/app.cpp (see siblings.h). The snapshot plumbing mirrors app.cpp: a
// published snapshot is what readStmSnapshot() and the cheap accessors return afterwards.
#include <Arduino.h>
#include <esp_timer.h>

#include "app.h"
#include "fakes/fakes.h"
#include "siblings.h"

namespace app {

void setup() {
  ++sib::app().setupCalls;
  fakes::note("app.setup");
}

uint32_t nowMs() {
  if (sib::app().onNowMs) sib::app().onNowMs();
  return millis();
}

uint32_t uptimeS() {
  const sib::App& a = sib::app();
  if (a.uptimeS >= 0) return static_cast<uint32_t>(a.uptimeS);
  return static_cast<uint32_t>(esp_timer_get_time() / 1000000LL);
}

bool submit(const Command& cmd) {
  sib::App& a = sib::app();
  fakes::note("app.submit " + std::to_string(static_cast<int>(cmd.type)));
  if (!a.submitResult) return false;
  a.submitted.push_back(cmd);
  return true;
}

bool receive(Command& out) {
  sib::App& a = sib::app();
  if (a.toReceive.empty()) return false;
  out = a.toReceive.front();
  a.toReceive.pop_front();
  return true;
}

void readStmSnapshot(StmSnapshot& out) { out = sib::app().snapshot; }

vdm::LinkState stmLinkState() { return sib::app().link; }

bool stmFlashActive() { return sib::app().flashActive; }

void markStmFlashActive() {
  ++sib::app().flashMarks;
  sib::app().flashActive = true;
  fakes::note("app.markStmFlashActive");
}

uint32_t stmSnapshotRevision() { return sib::app().snapshotRevision; }

uint8_t stmProtocol() { return sib::app().proto; }

vdm::StmSupport stmSupport() { return sib::app().support; }

void publishStmSnapshot(const StmSnapshot& in) {
  sib::App& a = sib::app();
  ++a.publishes;
  a.published = in;
  a.snapshot = in;
  a.link = in.link;
  a.flashActive = in.flash.phase != vdm::FlashPhase::Idle &&
                  in.flash.phase != vdm::FlashPhase::Done &&
                  in.flash.phase != vdm::FlashPhase::Failed;
  a.proto = in.proto;
  a.support = in.support;
  a.snapshotRevision = in.revision;
}

void requestStmSave() {
  ++sib::app().saveRequests;
  sib::app().saveState = vdm::StmSaveState::Waiting;
  fakes::note("app.requestStmSave");
}

vdm::StmSaveState stmSaveState() { return sib::app().saveState; }

void setStmSaveState(vdm::StmSaveState s) {
  sib::app().saveStates.push_back(s);
  sib::app().saveState = s;
}

CalibInfo calibInfo() { return sib::app().calib; }

void setCalibInfo(const CalibInfo& c) {
  sib::app().calibInfos.push_back(c);
  sib::app().calib = c;
}

void readHealth(vdm::HealthSnapshot& out) {
  ++sib::app().healthReads;
  out = sib::app().health;
}

}  // namespace app
