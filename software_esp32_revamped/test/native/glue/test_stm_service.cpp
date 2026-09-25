// Smoke tests of src/stm_service.cpp: the scheduled calibration of the app task.
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

}  // namespace

TEST_CASE("stm_service begin: restores the booked slot and the last calibration time") {
  glue::begin();
  sib::storage().calibSlot = 20260920;
  sib::storage().lastCalib = 1790000000;
  stm_service::begin();
  REQUIRE(sib::app().calibInfos.size() == 1);
  CHECK(sib::app().calib.lastScheduledEpoch == 1790000000);
  CHECK(sib::app().calib.nextSlot == 0);
}

TEST_CASE("stm_service: a due slot submits one scheduled calibration of all valves") {
  glue::begin();
  scheduleWednesdayAt(3);
  sib::net().localTime = wednesday(3, 5);
  stm_service::begin();
  stm_service::service(10000);
  REQUIRE(sib::app().submitted.size() == 1);
  const app::Command& c = sib::app().submitted[0];
  CHECK(c.type == app::CommandType::Calibrate);
  CHECK(c.valve == vdm::kAllValves);
  CHECK(c.scheduled);
  CHECK(sib::storage().savedCalibSlots == std::vector<uint32_t>{20260923});
  CHECK(sib::storage().savedLastCalibs == std::vector<int64_t>{wednesday(3, 5).epoch});
  const vdm::Event e = sib::logger().withCode(vdm::EventCode::ScheduledCalibration).at(0);
  CHECK(e.arg1 == 20260923);
  CHECK(e.arg2 == 5);
  CHECK(sib::app().calib.lastScheduledEpoch == wednesday(3, 5).epoch);
  stm_service::service(20000);  // booked: nothing more
  CHECK(sib::app().submitted.size() == 1);
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

TEST_CASE("stm_service: a full command queue is reported with the staln command") {
  glue::begin();
  scheduleWednesdayAt(3);
  sib::net().localTime = wednesday(3, 1);
  sib::app().submitResult = false;
  stm_service::service(10000);
  const vdm::Event e = sib::logger().withCode(vdm::EventCode::StmQueueFull).at(0);
  CHECK(e.arg1 == static_cast<int32_t>(vdm::Cmd::Staln));
  CHECK(sib::storage().savedCalibSlots.empty());
  CHECK_FALSE(sib::logger().has(vdm::EventCode::ScheduledCalibration));
}

TEST_CASE("stm_service: flushForRestart writes nothing yet") {
  glue::begin();
  stm_service::flushForRestart();
  CHECK(fakes::journal().empty());
}
