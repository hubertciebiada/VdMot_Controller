#include "stm_service.h"

#include <esp_attr.h>
#include <freertos/FreeRTOS.h>
#include <string.h>
#include <time.h>

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
uint16_t gAttemptId = 0;

// Survive software restarts, panics and watchdog resets (not a power loss:
// then the CRC fails and NVS or nothing is used).
RTC_NOINIT_ATTR uint8_t gRtcTargets[vdm::kPersistedTargetsSize];
RTC_NOINIT_ATTR uint8_t gRtcLease[vdm::kLeaseRecordSize];

vdm::TargetSaver gSaver;
vdm::PersistedTargets gBootTargets;
vdm::RestoreSource gBootSource = vdm::RestoreSource::None;
bool gBootLeaseValid = false;
vdm::LeaseClient::Snapshot gBootLease;

// STM task -> app task.
portMUX_TYPE gMux = portMUX_INITIALIZER_UNLOCKED;
vdm::PersistedTargets gHandTargets;
bool gHandTargetsNew = false;
struct CalibResult {
  uint16_t attempt = 0;
  bool ok = false;
  vdm::CalibFailure reason = vdm::CalibFailure::None;
};
CalibResult gHandResult;
bool gHandResultNew = false;

bool takeTargets(vdm::PersistedTargets& out) {
  portENTER_CRITICAL(&gMux);
  const bool got = gHandTargetsNew;
  if (got) out = gHandTargets;
  gHandTargetsNew = false;
  portEXIT_CRITICAL(&gMux);
  return got;
}

bool takeResult(CalibResult& out) {
  portENTER_CRITICAL(&gMux);
  const bool got = gHandResultNew;
  if (got) out = gHandResult;
  gHandResultNew = false;
  portEXIT_CRITICAL(&gMux);
  return got;
}

// Epoch of the next slot at calib.hour:calib.minute local time, 0 = none. The
// second pass takes the UTC offset in force at the slot (a DST change before it).
int64_t slotEpoch(uint32_t slot, const vdm::LocalTime& now) {
  const time_t first =
      static_cast<time_t>(vdm::calibSlotEpoch(slot, gCfg.calib.hour, gCfg.calib.minute, now));
  struct tm tm;
  localtime_r(&first, &tm);
  vdm::LocalTime at;
  at.valid = true;
  at.year = static_cast<uint16_t>(tm.tm_year + 1900);
  at.month = static_cast<uint8_t>(tm.tm_mon + 1);
  at.mday = static_cast<uint8_t>(tm.tm_mday);
  at.hour = static_cast<uint8_t>(tm.tm_hour);
  at.minute = static_cast<uint8_t>(tm.tm_min);
  at.epoch = first;
  return vdm::calibSlotEpoch(slot, gCfg.calib.hour, gCfg.calib.minute, at);
}

void setCalibInfo(int64_t lastEpoch, uint32_t nextSlot, const vdm::LocalTime& now) {
  app::CalibInfo ci;
  ci.lastScheduledEpoch = lastEpoch;
  ci.nextSlot = nextSlot;
  ci.nextEpoch = slotEpoch(nextSlot, now);
  app::setCalibInfo(ci);
}

void failed(vdm::CalibFailure reason) {
  logger::log(vdm::EventCode::ScheduledCalibrationFailed, vdm::kNoValve,
              static_cast<int32_t>(gCalib.attemptSlot()), static_cast<int32_t>(reason));
}

void calibrationTick(uint32_t now) {
  const vdm::LocalTime lt = net::localTime();
  const vdm::CalibDecision d = gCalib.evaluate(gCfg.calib, lt, now);
  if (d == vdm::CalibDecision::Fire) {
    app::Command c;
    c.type = app::CommandType::Calibrate;
    c.valve = vdm::kAllValves;
    c.source = vdm::TargetSource::None;
    c.scheduled = true;
    c.attempt = ++gAttemptId;
    if (!app::submit(c)) {
      gCalib.onResult(false, now);
      failed(vdm::CalibFailure::NotSent);
    }
  } else if (d == vdm::CalibDecision::SkippedNoTime) {
    logger::log(vdm::EventCode::CalibTimeMissing, vdm::kNoValve, 0);
  } else if (d == vdm::CalibDecision::NoResult) {
    failed(vdm::CalibFailure::NoResult);
  } else if (d == vdm::CalibDecision::Missed) {
    logger::log(vdm::EventCode::ScheduledCalibrationMissed, vdm::kNoValve,
                static_cast<int32_t>(gCalib.attemptSlot()), gCalib.attempts());
  }
  setCalibInfo(app::calibInfo().lastScheduledEpoch, gCalib.nextSlot(gCfg.calib, lt), lt);
}

// The STM confirmed (or not) the staln of the current attempt.
void calibrationResult(uint32_t now) {
  CalibResult r;
  if (!takeResult(r) || r.attempt != gAttemptId) return;
  if (!gCalib.attemptPending()) return;  // NoResult was reported already
  if (!r.ok) {
    gCalib.onResult(false, now);
    failed(r.reason);
    return;
  }
  gCalib.onResult(true, now);
  const vdm::LocalTime lt = net::localTime();
  storage::saveCalibSlot(gCalib.lastSlot());
  storage::saveLastCalib(lt.epoch);
  logger::log(vdm::EventCode::ScheduledCalibration, vdm::kNoValve,
              static_cast<int32_t>(gCalib.lastSlot()), gCalib.lateMinutes());
  setCalibInfo(lt.epoch, gCalib.nextSlot(gCfg.calib, lt), lt);
}

void saveTargets(uint32_t now) {
  gSaver.saved(storage::saveTargets(gSaver.bytes(), vdm::kPersistedTargetsSize), now);
}

void targetsTick(uint32_t now) {
  vdm::PersistedTargets t;
  if (takeTargets(t)) gSaver.update(t, now);
  if (gSaver.due(now)) saveTargets(now);
}

}  // namespace

void begin() {
  gCalib.restoreLastSlot(storage::loadCalibSlot());
  setCalibInfo(storage::loadLastCalib(), 0, vdm::LocalTime{});
  uint8_t nvs[vdm::kPersistedTargetsSize];
  const size_t n = storage::loadTargets(nvs, sizeof nvs);
  gBootSource = vdm::chooseTargets(gRtcTargets, sizeof gRtcTargets, nvs, n, gBootTargets);
  vdm::PersistedTargets stored;
  vdm::decodeTargets(nvs, n, stored);
  gSaver.primeStored(stored);
  // Targets newer than NVS (restart without a flush): NVS catches up.
  if (gBootSource == vdm::RestoreSource::Rtc) gSaver.update(gBootTargets, app::nowMs());
  gBootLeaseValid = vdm::decodeLeaseRecord(gRtcLease, sizeof gRtcLease, gBootLease);
}

void service(uint32_t nowMs) {
  if (storage::configRevision() != gCfgRevision) {
    gCfgRevision = storage::configRevision();
    storage::getConfig(gCfg);
  }
  calibrationResult(nowMs);
  targetsTick(nowMs);
  if (vdm::elapsedMs(nowMs, gLastCalibMs) < kCalibrationTickMs) return;
  gLastCalibMs = nowMs;
  calibrationTick(nowMs);
}

void flushForRestart() {
  const uint32_t now = app::nowMs();
  vdm::PersistedTargets t;
  if (takeTargets(t)) gSaver.update(t, now);
  if (gSaver.dirty()) saveTargets(now);
}

void storeDesiredTargets(const vdm::PersistedTargets& t) {
  uint8_t bytes[vdm::kPersistedTargetsSize];
  vdm::encodeTargets(t, bytes);
  portENTER_CRITICAL(&gMux);
  memcpy(gRtcTargets, bytes, sizeof bytes);
  gHandTargets = t;
  gHandTargetsNew = true;
  portEXIT_CRITICAL(&gMux);
}

void storeLeaseRecord(const vdm::LeaseClient::Snapshot& s) {
  uint8_t bytes[vdm::kLeaseRecordSize];
  vdm::encodeLeaseRecord(s, bytes);
  memcpy(gRtcLease, bytes, sizeof bytes);
}

void postScheduledCalibResult(uint16_t attempt, bool ok, vdm::CalibFailure reason) {
  portENTER_CRITICAL(&gMux);
  gHandResult.attempt = attempt;
  gHandResult.ok = ok;
  gHandResult.reason = reason;
  gHandResultNew = true;
  portEXIT_CRITICAL(&gMux);
}

const vdm::PersistedTargets& bootTargets(vdm::RestoreSource& src) {
  src = gBootSource;
  return gBootTargets;
}

bool bootLease(vdm::LeaseClient::Snapshot& out) {
  out = gBootLease;
  return gBootLeaseValid;
}

}  // namespace stm_service
