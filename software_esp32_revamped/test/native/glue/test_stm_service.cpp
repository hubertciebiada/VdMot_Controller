// Tests of src/stm_service.cpp: the scheduled calibration confirmed by the STM, the NVS copy of
// the desired targets, the RTC records across software restarts and the boot choice.
#include <string.h>

#include <vdm/target_store.h>

#include "glue_test.h"
#include "stm_service.h"

namespace {

// Wednesday 2026-09-23 local (TZ UTC) hh:mm.
vdm::LocalTime wednesday(uint8_t hour, uint8_t minute) {
  vdm::LocalTime t;
  t.valid = true;
  t.year = 2026;
  t.month = 9;
  t.mday = 23;
  t.wday = 3;
  t.hour = hour;
  t.minute = minute;
  t.epoch = 1790121600 + hour * 3600 + minute * 60;
  return t;
}

void scheduleWednesdayAt(uint8_t hour) {
  sib::storage().active.calib.dayMask = 1u << 3;
  sib::storage().active.calib.hour = hour;
  sib::storage().active.calib.minute = 0;
  sib::storage().revision = 1;
}

vdm::PersistedTargets targets(uint8_t valve, uint8_t pos, vdm::TargetSource src) {
  vdm::PersistedTargets t;
  t.valid[valve] = true;
  t.pos[valve] = pos;
  t.source[valve] = src;
  return t;
}

std::vector<uint8_t> encoded(const vdm::PersistedTargets& t) {
  uint8_t b[vdm::kPersistedTargetsSize];
  vdm::encodeTargets(t, b);
  return std::vector<uint8_t>(b, b + sizeof b);
}

// Fires the Wednesday 03:00 slot at 03:05 (service at 10 s) and returns the submitted attempt.
uint16_t fire() {
  scheduleWednesdayAt(3);
  sib::net().localTime = wednesday(3, 5);
  stm_service::begin();
  stm_service::service(10000);
  REQUIRE(sib::app().submitted.size() == 1);
  return sib::app().submitted[0].attempt;
}

}  // namespace

// ================================================================ scheduled calibration

TEST_CASE("stm_service begin: restores the booked slot and the last calibration time") {
  glue::begin();
  sib::storage().calibSlot = 20260920;
  sib::storage().lastCalib = 1790000000;
  stm_service::begin();
  REQUIRE(sib::app().calibInfos.size() == 1);
  CHECK(sib::app().calib.lastScheduledEpoch == 1790000000);
  CHECK(sib::app().calib.nextSlot == 0);
  CHECK(sib::app().calib.nextEpoch == 0);
}

TEST_CASE("stm_service: a due slot submits one scheduled calibration and books nothing yet") {
  glue::begin();
  const uint16_t attempt = fire();
  const app::Command& c = sib::app().submitted[0];
  CHECK(c.type == app::CommandType::Calibrate);
  CHECK(c.valve == vdm::kAllValves);
  CHECK(c.scheduled);
  CHECK(attempt == 1);
  CHECK(sib::storage().savedCalibSlots.empty());
  CHECK_FALSE(sib::logger().has(vdm::EventCode::ScheduledCalibration));
  CHECK(sib::app().calib.nextSlot == 20260923);  // not booked: still today
  stm_service::service(20000);  // waiting for the STM: nothing more
  CHECK(sib::app().submitted.size() == 1);
}

TEST_CASE("stm_service: the STM's confirmation books the slot and logs it") {
  glue::begin();
  const uint16_t attempt = fire();
  stm_service::postScheduledCalibResult(attempt, true, vdm::CalibFailure::None);
  sib::net().localTime = wednesday(3, 6);
  stm_service::service(11000);
  CHECK(sib::storage().savedCalibSlots == std::vector<uint32_t>{20260923});
  CHECK(sib::storage().savedLastCalibs == std::vector<int64_t>{wednesday(3, 6).epoch});
  const vdm::Event e = sib::logger().withCode(vdm::EventCode::ScheduledCalibration).at(0);
  CHECK(e.arg1 == 20260923);
  CHECK(e.arg2 == 5);
  CHECK(sib::app().calib.lastScheduledEpoch == wednesday(3, 6).epoch);
  CHECK(sib::app().calib.nextSlot == 20260930);
  CHECK(sib::app().calib.nextEpoch == 1790121600 + 7 * 86400 + 3 * 3600);
  stm_service::service(12000);  // the result is taken once
  CHECK(sib::storage().savedCalibSlots.size() == 1);
}

TEST_CASE("stm_service: a result of another attempt is ignored") {
  glue::begin();
  const uint16_t attempt = fire();
  stm_service::postScheduledCalibResult(static_cast<uint16_t>(attempt + 1), true,
                                        vdm::CalibFailure::None);
  stm_service::service(11000);
  CHECK(sib::storage().savedCalibSlots.empty());
  CHECK_FALSE(sib::logger().has(vdm::EventCode::ScheduledCalibration));
}

TEST_CASE("stm_service: no reply is reported and the slot fires again 10 min later") {
  glue::begin();
  const uint16_t attempt = fire();
  stm_service::postScheduledCalibResult(attempt, false, vdm::CalibFailure::NoReply);
  stm_service::service(11000);
  const vdm::Event e = sib::logger().withCode(vdm::EventCode::ScheduledCalibrationFailed).at(0);
  CHECK(e.arg1 == 20260923);
  CHECK(e.arg2 == 1);
  CHECK(sib::storage().savedCalibSlots.empty());
  sib::net().localTime = wednesday(3, 14);
  stm_service::service(11000 + 599999);
  CHECK(sib::app().submitted.size() == 1);
  sib::net().localTime = wednesday(3, 15);
  stm_service::service(11000 + 600000 + 10000);
  REQUIRE(sib::app().submitted.size() == 2);
  CHECK(sib::app().submitted[1].attempt == attempt + 1);
}

TEST_CASE("stm_service: without a result within 60 s the attempt failed (no result)") {
  glue::begin();
  fire();
  stm_service::service(69999);
  stm_service::service(80000);
  const vdm::Event e = sib::logger().withCode(vdm::EventCode::ScheduledCalibrationFailed).at(0);
  CHECK(e.arg1 == 20260923);
  CHECK(e.arg2 == 3);
  // A late confirmation does not book it any more.
  stm_service::postScheduledCalibResult(1, true, vdm::CalibFailure::None);
  stm_service::service(81000);
  CHECK(sib::storage().savedCalibSlots.empty());
}

TEST_CASE("stm_service: a full command queue is a failed attempt (not sent)") {
  glue::begin();
  scheduleWednesdayAt(3);
  sib::net().localTime = wednesday(3, 1);
  sib::app().submitResult = false;
  stm_service::service(10000);
  const vdm::Event e = sib::logger().withCode(vdm::EventCode::ScheduledCalibrationFailed).at(0);
  CHECK(e.arg1 == 20260923);
  CHECK(e.arg2 == 2);
  CHECK(sib::storage().savedCalibSlots.empty());
  CHECK_FALSE(sib::logger().has(vdm::EventCode::ScheduledCalibration));
}

TEST_CASE("stm_service: a window that closes without a confirmation is reported as missed") {
  glue::begin();
  const uint16_t attempt = fire();
  stm_service::postScheduledCalibResult(attempt, false, vdm::CalibFailure::NoReply);
  stm_service::service(11000);
  sib::net().localTime = wednesday(5, 0);
  stm_service::service(1000000);
  const vdm::Event e = sib::logger().withCode(vdm::EventCode::ScheduledCalibrationMissed).at(0);
  CHECK(e.arg1 == 20260923);
  CHECK(e.arg2 == 1);
  CHECK(e.severity == vdm::Severity::Error);
}

TEST_CASE("stm_service: the schedule is evaluated every 10 s only") {
  glue::begin();
  scheduleWednesdayAt(3);
  sib::net().localTime = wednesday(3, 0);
  stm_service::service(9999);
  CHECK(sib::app().submitted.empty());
  CHECK(sib::app().calibInfos.empty());
  stm_service::service(10000);
  CHECK(sib::app().submitted.size() == 1);
}

TEST_CASE("stm_service: missing time is reported once") {
  glue::begin();
  scheduleWednesdayAt(3);
  stm_service::service(3600001);
  CHECK(sib::logger().withCode(vdm::EventCode::CalibTimeMissing).size() == 1);
}

// ================================================================ desired targets (NVS)

TEST_CASE("stm_service: a desired-target change is written to NVS 5 min later") {
  glue::begin();
  stm_service::begin();
  stm_service::storeDesiredTargets(targets(0, 42, vdm::TargetSource::Mqtt));
  stm_service::service(1000);
  CHECK(sib::storage().targets.empty());
  stm_service::service(1000 + 299999);
  CHECK(sib::storage().targets.empty());
  stm_service::service(1000 + 300000);
  CHECK(sib::storage().targets == encoded(targets(0, 42, vdm::TargetSource::Mqtt)));
}

TEST_CASE("stm_service: a failed NVS write is retried 5 min later") {
  glue::begin();
  stm_service::begin();
  sib::storage().saveTargetsResult = false;
  stm_service::storeDesiredTargets(targets(1, 7, vdm::TargetSource::Web));
  stm_service::service(0);
  stm_service::service(300000);
  CHECK(sib::storage().targets.empty());
  sib::storage().saveTargetsResult = true;
  stm_service::service(599999);
  CHECK(sib::storage().targets.empty());
  stm_service::service(600000);
  CHECK(sib::storage().targets == encoded(targets(1, 7, vdm::TargetSource::Web)));
}

TEST_CASE("stm_service: the value NVS holds already is never written again") {
  glue::begin();
  sib::storage().targets = encoded(targets(0, 42, vdm::TargetSource::Mqtt));
  stm_service::begin();
  sib::storage().targets.clear();
  stm_service::storeDesiredTargets(targets(0, 42, vdm::TargetSource::Mqtt));
  stm_service::service(1000);
  stm_service::service(10000000);
  CHECK(sib::storage().targets.empty());
}

TEST_CASE("stm_service: flushForRestart writes dirty targets at once, nothing when clean") {
  glue::begin();
  stm_service::begin();
  stm_service::flushForRestart();
  CHECK(sib::storage().targets.empty());
  stm_service::storeDesiredTargets(targets(3, 9, vdm::TargetSource::Web));
  stm_service::flushForRestart();
  CHECK(sib::storage().targets == encoded(targets(3, 9, vdm::TargetSource::Web)));
  sib::storage().targets.clear();
  stm_service::flushForRestart();
  CHECK(sib::storage().targets.empty());
}

// ================================================================ boot choice (RTC / NVS)

TEST_CASE("stm_service: after power-on only NVS targets exist") {
  glue::begin();
  sib::storage().targets = encoded(targets(2, 60, vdm::TargetSource::Web));
  stm_service::begin();
  vdm::RestoreSource src = vdm::RestoreSource::None;
  const vdm::PersistedTargets& t = stm_service::bootTargets(src);
  CHECK(src == vdm::RestoreSource::Nvs);
  CHECK(t == targets(2, 60, vdm::TargetSource::Web));
  vdm::LeaseClient::Snapshot lease;
  CHECK_FALSE(stm_service::bootLease(lease));
}

TEST_CASE("stm_service: nothing stored boots without targets") {
  glue::begin();
  stm_service::begin();
  vdm::RestoreSource src = vdm::RestoreSource::Rtc;
  const vdm::PersistedTargets& t = stm_service::bootTargets(src);
  CHECK(src == vdm::RestoreSource::None);
  CHECK(t == vdm::PersistedTargets{});
}

TEST_CASE("stm_service: the RTC copies survive a software restart and win over NVS") {
  if (testkit::boot() == 0) {
    glue::begin();
    stm_service::storeDesiredTargets(targets(0, 33, vdm::TargetSource::Mqtt));
    vdm::LeaseClient::Snapshot s;
    s.lost = true;
    s.active = true;
    s.mask = 0x005;
    s.lostElapsedMs = 123456;
    stm_service::storeLeaseRecord(s);
    testkit::reboot(testkit::Reset::Software);
  }
  glue::begin();
  sib::storage().targets = encoded(targets(0, 20, vdm::TargetSource::Web));
  stm_service::begin();
  vdm::RestoreSource src = vdm::RestoreSource::None;
  const vdm::PersistedTargets& t = stm_service::bootTargets(src);
  CHECK(src == vdm::RestoreSource::Rtc);
  CHECK(t == targets(0, 33, vdm::TargetSource::Mqtt));
  vdm::LeaseClient::Snapshot lease;
  REQUIRE(stm_service::bootLease(lease));
  CHECK(lease.lost);
  CHECK(lease.active);
  CHECK(lease.mask == 0x005);
  CHECK(lease.lostElapsedMs == 123456u);
}

TEST_CASE("stm_service: a power-on forgets the RTC copies") {
  if (testkit::boot() == 0) {
    glue::begin();
    stm_service::storeDesiredTargets(targets(0, 33, vdm::TargetSource::Mqtt));
    stm_service::storeLeaseRecord(vdm::LeaseClient::Snapshot{});
    testkit::reboot(testkit::Reset::PowerOn);
  }
  glue::begin();
  stm_service::begin();
  vdm::RestoreSource src = vdm::RestoreSource::Rtc;
  stm_service::bootTargets(src);
  CHECK(src == vdm::RestoreSource::None);
  vdm::LeaseClient::Snapshot lease;
  CHECK_FALSE(stm_service::bootLease(lease));
}
