#include "stm_service.h"

#include <vdm/calib_schedule.h>
#include <vdm/common.h>
#include <vdm/config.h>
#include <vdm/event_log.h>
#include <vdm/stm_codec.h>

#include "app.h"
#include "boot_alloc.h"
#include "logger.h"
#include "net.h"
#include "storage.h"

namespace stm_service {

namespace {

constexpr uint32_t kCalibrationTickMs = 10000;

// App task working copy of the config (static: too large for the task stack).
vdm::Config& gCfg = bootAlloc<vdm::Config>();
uint32_t gCfgRevision = 0;
vdm::CalibScheduler gCalib;
uint32_t gLastCalibMs = 0;

void setCalibInfo(int64_t lastEpoch, uint32_t nextSlot) {
  app::CalibInfo ci;
  ci.lastScheduledEpoch = lastEpoch;
  ci.nextSlot = nextSlot;
  app::setCalibInfo(ci);
}

void calibrationTick(uint32_t now) {
  const vdm::LocalTime lt = net::localTime();
  int64_t lastEpoch = app::calibInfo().lastScheduledEpoch;
  const vdm::CalibDecision d = gCalib.evaluate(gCfg.calib, lt, now);
  if (d == vdm::CalibDecision::Fire) {
    app::Command c;
    c.type = app::CommandType::Calibrate;
    c.valve = vdm::kAllValves;
    c.source = vdm::TargetSource::None;
    c.scheduled = true;
    if (app::submit(c)) {
      storage::saveCalibSlot(gCalib.lastSlot());
      storage::saveLastCalib(lt.epoch);
      lastEpoch = lt.epoch;
      logger::log(vdm::EventCode::ScheduledCalibration, vdm::kNoValve,
                  static_cast<int32_t>(gCalib.lastSlot()), gCalib.lateMinutes());
    } else {
      // The slot stays booked in RAM (no retry storm); the miss is reported
      // and the next slot fires normally.
      logger::log(vdm::EventCode::StmQueueFull, vdm::kNoValve,
                  static_cast<int32_t>(vdm::Cmd::Staln));
    }
  } else if (d == vdm::CalibDecision::SkippedNoTime) {
    logger::log(vdm::EventCode::CalibTimeMissing, vdm::kNoValve, 0);
  }
  setCalibInfo(lastEpoch, gCalib.nextSlot(gCfg.calib, lt));
}

}  // namespace

void begin() {
  gCalib.restoreLastSlot(storage::loadCalibSlot());
  setCalibInfo(storage::loadLastCalib(), 0);
}

void service(uint32_t nowMs) {
  if (storage::configRevision() != gCfgRevision) {
    gCfgRevision = storage::configRevision();
    storage::getConfig(gCfg);
  }
  if (vdm::elapsedMs(nowMs, gLastCalibMs) < kCalibrationTickMs) return;
  gLastCalibMs = nowMs;
  calibrationTick(nowMs);
}

// Nothing is kept in RAM only yet.
void flushForRestart() {}

}  // namespace stm_service
