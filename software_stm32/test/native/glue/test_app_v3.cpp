// Protocol 3 in src/app.cpp (glue_app, valve state machine stubbed): lease and failsafe positions,
// assembly hold, blocked valves and automatic retries, warm restore, calibration records, stop,
// learn time rule, protection guard, safe mode, temperature hold.
#include "glue_test.h"
#include "stub_eeprom.h"
#include "stub_motor.h"
#include "stub_owDevices.h"
#include "stub_sysstat.h"
#include "stub_terminal.h"
#include "vdm/valve_codes.h"

namespace {

// app_setup, then every valve idle, calibrated and at its target 50
void begin() {
  glue::begin();
  app_setup();
  for (unsigned v = 0; v < ACTUATOR_COUNT; v++) {
    myvalvemots[v].status = VLV_STATE_IDLE;
    myvalvemots[v].calibrated = 1;
    myvalvemots[v].connected = 1;
    myvalvemots[v].scaler = 36;
  }
  stub::calls.clear();
  fake::takeTx(Serial6);
}

stub::Calls loopActions(int passes = 1) {
  stub::calls.clear();
  for (int i = 0; i < passes; i++) app_loop();
  return stub::callsOf("appsetaction");
}

valve_v3_info v3(uint16_t v) {
  valve_v3_info info = {0xFFFF, 0xFF, 0xFF, 0xFF, 0xFFFFFFFF, 0xFF};
  app_get_valve_v3(v, info);
  return info;
}

// the lease of 5 minutes expired
void expireLease() {
  app_lease_configure(5);
  app_1s_tick(300);
  REQUIRE(app_lease_state() == 2);
}

}  // namespace

TEST_CASE("lease: default 60 min, slcfg, expiry, valve polls, heartbeats") {
  begin();
  CHECK(app_lease_timeout() == 60);
  CHECK(app_lease_state() == 1);
  CHECK(app_lease_remaining_s() == 3600);
  CHECK_FALSE(app_lease_client());
  app_lease_configure(5);
  CHECK(app_lease_timeout() == 5);
  CHECK(app_lease_remaining_s() == 300);
  app_1s_tick(299);
  CHECK(app_lease_remaining_s() == 1);
  app_1s_tick(1);
  CHECK(app_lease_state() == 2);
  CHECK(app_lease_remaining_s() == 0);
  app_lease_poll();  // no lease client: a gvlvx poll renews
  CHECK(app_lease_state() == 1);
  CHECK(app_lease_remaining_s() == 300);
  app_lease_command();
  CHECK(app_lease_client());
  app_1s_tick(299);
  app_lease_poll();
  CHECK(app_lease_remaining_s() == 1);
  app_lease_heartbeat(false);
  CHECK(app_lease_remaining_s() == 1);
  app_lease_heartbeat(true);
  CHECK(app_lease_remaining_s() == 300);
  app_lease_configure(0);
  CHECK(app_lease_state() == 0);
  CHECK(stub::calls.empty());
}

TEST_CASE("lease K1-5: gvlvx polls of an ESP 2.0.0 keep the lease, gvlvy polls without slhbt do not") {
  begin();
  app_lease_configure(5);
  for (int i = 0; i < 20; i++) {
    app_1s_tick(60);
    app_lease_poll();
  }
  CHECK(app_lease_state() == 1);
  app_lease_heartbeat(false);  // a protocol-3 ESP: its polls no longer renew
  for (int i = 0; i < 4; i++) {
    app_1s_tick(60);
    app_lease_poll();
    app_lease_heartbeat(false);
  }
  CHECK(app_lease_state() == 1);
  app_1s_tick(60);
  CHECK(app_lease_state() == 2);
}

TEST_CASE("failsafe K1-4: an expired lease drives the valves to their failsafe positions, not the target") {
  begin();
  app_set_failsafe(255, 20);
  app_set_failsafe(3, 255);
  CHECK(app_failsafe_pct(0) == 20);
  CHECK(app_failsafe_pct(3) == 255);
  CHECK(app_failsafe_pct(12) == 255);
  CHECK(app_failsafe_mask() == 0);
  CHECK(loopActions().empty());
  expireLease();
  CHECK(app_failsafe_mask() == 0x0FF7);
  CHECK(loopActions() == stub::Calls{"appsetaction(c, 0, 30, 0)"});
  CHECK(+myvalvemots[0].target_position == 50);
  valve_v3_info info = v3(0);
  CHECK(info.flags == vdm::kVlvFlagFsLease);
  CHECK(info.fsPct == 20);
  CHECK(info.drive == 20);
  CHECK(info.fault == 0);
  CHECK(info.retryS == 0);
  CHECK(info.retries == 0);
  info = v3(3);
  CHECK(info.flags == 0);
  CHECK(info.fsPct == 255);
  CHECK(info.drive == 50);
  // slhbt 1: the stored targets again
  app_lease_heartbeat(true);
  CHECK(app_failsafe_mask() == 0);
  CHECK(loopActions().empty());
  CHECK(v3(0).drive == 50);
}

TEST_CASE("failsafe: a failed or open-circuit valve is not driven, a blocked one without the lease") {
  begin();
  app_set_failsafe(255, 20);
  myvalvemots[1].status = VLV_STATE_FAILED;
  myvalvemots[2].status = VLV_STATE_OPENCIR;
  myvalvemots[3].status = VLV_STATE_BLOCKS;
  myvalvemots[3].actual_position = 20;
  expireLease();
  CHECK(app_failsafe_mask() == (0x0FFF & ~0x000E));
  CHECK(v3(1).drive == 50);
  CHECK(v3(2).drive == 50);
  CHECK(v3(3).drive == 20);
  CHECK(v3(3).flags == (vdm::kVlvFlagFsBlocked | vdm::kVlvFlagRetry));  // the retry was scheduled meanwhile
}

TEST_CASE("assembly hold K1-7: staop opens fully and keeps the valve at 100 until the next stgtp") {
  begin();
  app_set_failsafe(255, 255);
  app_set_failsafe(2, 20);
  expireLease();
  CHECK(app_set_valveopen(2) == 0);
  CHECK(+myvalves[2].assemblyHold == 1);
  CHECK(loopActions() == stub::Calls{"appsetaction(p, 2, 0, 0)"});
  CHECK(+myvalvemots[2].status == VLV_STATE_FULLOPEN);
  myvalvemots[2].status = VLV_STATE_IDLE;
  myvalvemots[2].actual_position = 100;
  CHECK(loopActions().empty());
  CHECK(v3(2).flags == vdm::kVlvFlagAssembly);
  CHECK(v3(2).drive == 100);
  CHECK(app_failsafe_mask() == 0);
  myvalvemots[2].target_position = 40;
  app_target_changed(2);
  CHECK(+myvalves[2].assemblyHold == 0);
  CHECK(app_failsafe_mask() == 0x0004);
  CHECK(loopActions() == stub::Calls{"appsetaction(c, 2, 80, 0)"});
  app_set_valveopen(255);
  for (unsigned v = 0; v < ACTUATOR_COUNT; v++) CHECK(+myvalves[v].assemblyHold == 1);
}

TEST_CASE("app_target_changed: ends the service and assembly holds and touches the valve") {
  begin();
  myvalves[4].svcHold = 9;
  myvalves[4].assemblyHold = 1;
  app_target_changed(4);
  CHECK(+myvalves[4].svcHold == 0);
  CHECK(+myvalves[4].assemblyHold == 0);
  CHECK(+myvalves[4].touched == 1);
  CHECK(+myvalves[5].touched == 0);
  app_target_changed(12);
  // touched: a valve without a referenced position makes its reference move at once
  myvalvemots[4].needsReference = 1;
  CHECK(loopActions() == stub::Calls{"appsetaction(p, 4, 0, 0, 0x02)"});
  CHECK(+myvalves[4].touched == 0);
}

TEST_CASE("blocked valve K2: failsafe move with the status kept, rejected targets, retries 1 h and 6 h") {
  begin();
  app_lease_configure(0);
  app_set_failsafe(3, 50);
  myvalvemots[3].status = VLV_STATE_BLOCKS;
  myvalvemots[3].faultReason = (uint8_t) vdm::ValveFault::StrokesTooShort;
  myvalvemots[3].actual_position = 0;
  myvalvemots[3].target_position = 30;
  myvalves[3].rejectedTarget = 30;
  CHECK(loopActions() == stub::Calls{"appsetaction(o, 3, 50, 0, 0x01)"});
  myvalvemots[3].actual_position = 50;
  CHECK(loopActions().empty());
  CHECK(+myvalves[3].cmdRejected == 0);
  myvalvemots[3].target_position = 80;
  CHECK(loopActions().empty());
  CHECK(+myvalves[3].cmdRejected == 1);
  app_1s_tick(1);
  valve_v3_info info = v3(3);
  CHECK(info.flags == (vdm::kVlvFlagFsBlocked | vdm::kVlvFlagRetry));
  CHECK(info.fault == 4);
  CHECK(info.fsPct == 50);
  CHECK(info.drive == 50);
  CHECK(info.retryS == 3600);
  CHECK(info.retries == 0);
  app_1s_tick(3599);
  CHECK(v3(3).retryS == 1);
  CHECK(+myvalves[3].retryLearn == 0);
  app_1s_tick(1);
  CHECK(+myvalves[3].retryLearn == 1);
  CHECK(v3(3).retries == 1);
  CHECK(v3(3).retryS == 0);
  CHECK(app_learn_pending(3, VLV_STATE_BLOCKS, false));
  CHECK(loopActions().empty());
  CHECK(+myvalvemots[3].status == VLV_STATE_PRESENT);
  CHECK(loopActions() == stub::Calls{"appsetaction(l, 3, 0, 0)"});
  CHECK(+myvalves[3].retryLearn == 0);
  CHECK(+myvalvemots[3].calibState == calibInProgress);
  // blocked again after the retry
  myvalvemots[3].status = VLV_STATE_BLOCKS;
  app_1s_tick(1);
  CHECK(v3(3).retryS == 0);  // still busy: the handed-over calibration
  myvalvemots[3].calibState = calibIdle;
  app_1s_tick(1);
  CHECK(v3(3).retryS == 21600);
  CHECK(v3(3).retries == 1);
  // fine again: reset
  myvalvemots[3].status = VLV_STATE_IDLE;
  app_1s_tick(1);
  CHECK(v3(3).retries == 0);
  CHECK(v3(3).flags == 0);
}

TEST_CASE("blocked valve: failsafe 255 keeps it where the calibration left it") {
  begin();
  app_set_failsafe(3, 255);
  myvalvemots[3].status = VLV_STATE_BLOCKS;
  myvalvemots[3].actual_position = 0;
  CHECK(loopActions().empty());
  CHECK(v3(3).flags == 0);
}

TEST_CASE("short K2/W10: the retry is a presence test, calibration requests become tests") {
  begin();
  app_lease_configure(0);
  myvalvemots[5].status = VLV_STATE_FAILED;
  myvalvemots[5].faultReason = (uint8_t) vdm::ValveFault::Short;
  myvalvemots[5].actual_position = 30;
  app_1s_tick(1);
  app_1s_tick(3600);
  CHECK(+myvalves[5].retestRequest == RETEST_TEST);
  CHECK(+myvalves[5].retryLearn == 0);
  const stub::Calls actions = loopActions(12);
  CHECK(actions == stub::Calls{"appsetaction(x, 5, 0, 0)"});
  CHECK(+myvalves[5].retestRequest == 0);
  CHECK(+myvalvemots[5].status == VLV_STATE_UNKNOWN);
  CHECK(+myvalvemots[5].actual_position == 0);
  CHECK(+myvalvemots[5].recal == 0);
  // staln of a shorted valve
  myvalvemots[5].status = VLV_STATE_FAILED;
  CHECK(app_set_valvelearning(5) == 0);
  app_loop();
  CHECK(+myvalves[5].forcedLearn == 0);
  CHECK(+myvalvemots[5].calibration == 0);
  CHECK(+myvalvemots[5].calibState == calibIdle);
  CHECK(+myvalvemots[5].status == VLV_STATE_UNKNOWN);
  // a failed valve with another fault calibrates
  myvalvemots[6].status = VLV_STATE_FAILED;
  myvalvemots[6].faultReason = (uint8_t) vdm::ValveFault::MoveTimeout;
  app_set_valvelearning(6);
  app_loop();
  CHECK(+myvalves[6].forcedLearn == 1);
}

TEST_CASE("time and movement triggers skip failed and blocked valves") {
  begin();
  app_set_learntime(120);
  app_set_learnmovements(50);
  myvalvemots[0].status = VLV_STATE_BLOCKS;
  myvalvemots[1].status = VLV_STATE_FAILED;
  myvalves[1].learn_movements = 0;
  myvalves[2].learn_movements = 0;
  app_10s_loop(10);
  CHECK(+myvalves[0].timedLearn == 0);
  CHECK(+myvalves[0].learn_time == 120);
  CHECK(+myvalvemots[1].calibration == 0);
  CHECK(+myvalvemots[2].calibration == 1);
}

TEST_CASE("learn time S3: real seconds, stagger, the 1200 s trigger of valve 11") {
  begin();
  app_set_learntime(1200);
  CHECK(app_get_learntime() == 1200);
  CHECK(+myvalves[0].learn_time == 100);
  CHECK(+myvalves[11].learn_time == 1200);
  for (int i = 1; i < 120; i++) {
    app_10s_loop(10);
    REQUIRE(+myvalves[11].timedLearn == 0);
  }
  CHECK(+myvalves[11].learn_time == 10);
  app_10s_loop(10);
  CHECK(+myvalves[11].timedLearn == 1);
  CHECK(+myvalves[11].learn_time == 1200);
  // a late call counts its real seconds
  app_set_learntime(1200);
  app_10s_loop(99);
  CHECK(+myvalves[0].learn_time == 1);
}

TEST_CASE("learn time C-7: stored 0 is the ESP schedule only while a lease client was seen within 24 h") {
  begin();
  app_set_learntime(0);
  CHECK(app_get_learntime() == 0);
  CHECK(+myvalves[11].learn_time == 604800);
  app_lease_command();
  app_1s_tick(1);
  CHECK(+myvalves[11].learn_time == 0);
  CHECK(+myvalves[0].learn_time == 0);
  app_10s_loop(10);
  CHECK(+myvalves[0].timedLearn == 0);
  app_1s_tick(86398);
  CHECK(+myvalves[11].learn_time == 0);
  app_1s_tick(1);
  CHECK(+myvalves[11].learn_time == 604800);
  CHECK(+myvalves[0].learn_time == 50400);
  app_1s_tick(1);
  CHECK(+myvalves[0].learn_time == 50400);  // no reload while the value stays
  app_lease_heartbeat(false);
  app_1s_tick(1);
  CHECK(+myvalves[11].learn_time == 0);
  app_set_learntime(3600);
  CHECK(+myvalves[11].learn_time == 3600);
}

TEST_CASE("early stops W9: the second one in a row requests a calibration, not of a blocked valve") {
  begin();
  myvalvemots[4].earlyLearnDue = 1;
  myvalvemots[6].earlyLearnDue = 1;
  myvalvemots[6].status = VLV_STATE_BLOCKS;
  app_set_failsafe(6, 255);
  CHECK(app_learn_pending(4, VLV_STATE_IDLE, false) == false);
  CHECK(loopActions().empty());
  CHECK(+myvalvemots[4].earlyLearnDue == 0);
  CHECK(+myvalves[4].earlyLearn == 1);
  CHECK(+myvalvemots[4].status == VLV_STATE_PRESENT);
  CHECK(+myvalvemots[6].earlyLearnDue == 0);
  CHECK(+myvalves[6].earlyLearn == 0);
  CHECK(loopActions() == stub::Calls{"appsetaction(l, 4, 0, 0)"});
  CHECK(+myvalves[4].earlyLearn == 0);
}

TEST_CASE("end-stop latch: the end of a move reaches the scheduler, one retry after an early stop") {
  begin();
  myvalvemots[0].target_position = 70;
  CHECK(loopActions() == stub::Calls{"appsetaction(o, 0, 20, 0)"});
  myvalvemots[0].actual_position = 55;
  myvalvemots[0].moveSeq = 1;
  stub::motor.snapshot[0].diag.last.stopReason = (uint8_t) vdm::StopReason::EarlyEndStop;
  stub::motor.snapshot[0].diag.lastEarly = true;
  stub::motor.snapshot[0].status = VLV_STATE_IDLE;
  stub::calls.clear();
  app_loop();
  CHECK(stub::callsOf("valve_get_snapshot") == stub::Calls{"valve_get_snapshot(0)"});
  CHECK(stub::callsOf("appsetaction") == stub::Calls{"appsetaction(o, 0, 15, 0)"});
  myvalvemots[0].moveSeq = 2;
  CHECK(loopActions(5).empty());
  CHECK(stub::callsOf("valve_get_snapshot") == stub::Calls{"valve_get_snapshot(0)"});
  myvalvemots[0].target_position = 80;
  CHECK(loopActions() == stub::Calls{"appsetaction(o, 0, 25, 0)"});
}

TEST_CASE("calibration records W2: accepted and blocked calibrations go to the EEPROM once") {
  begin();
  myvalvemots[1].opening_count = 3600;
  myvalvemots[1].closing_count = 3650;
  myvalvemots[1].meancurrent = 25;
  myvalvemots[1].calibSeq = 1;
  myvalves[1].calibRestored = 1;
  stub::calls.clear();
  app_loop();
  CHECK(stub::callsOf("eeprom_store_calib") == stub::Calls{"eeprom_store_calib(1)"});
  CHECK(stub::eeprom.lastCalib.openingCount == 3600);
  CHECK(stub::eeprom.lastCalib.closingCount == 3650);
  CHECK(stub::eeprom.lastCalib.meanCurrent == 25);
  CHECK(stub::eeprom.lastCalib.flags == vdm::kCalibValid);
  CHECK(+myvalves[1].calibRestored == 0);
  CHECK(+myvalves[1].storedSeq == 1);
  stub::calls.clear();
  app_loop();
  CHECK(stub::callsOf("eeprom_store_calib").empty());
  // blocked: the counts of the last success, marked failed
  myvalvemots[1].calibSeq = 2;
  myvalvemots[1].calibFailed = 1;
  app_loop();
  CHECK(stub::eeprom.lastCalib.flags == (vdm::kCalibValid | vdm::kCalibFailed));
  // never calibrated and blocked
  myvalvemots[2].calibrated = 0;
  myvalvemots[2].calibFailed = 1;
  myvalvemots[2].calibSeq = 5;
  app_loop();
  CHECK(stub::eeprom.lastCalib.flags == vdm::kCalibFailed);
  CHECK(+myvalves[2].storedSeq == 5);
}

TEST_CASE("app_restore W2: calibration records after a power-on, the test decides the status") {
  begin();
  eep_content.calib[2] = vdm::CalibRecord{3600, 3650, 25, vdm::kCalibValid};
  eep_content.calib[4] = vdm::CalibRecord{3000, 2950, 30, vdm::kCalibValid | vdm::kCalibFailed};
  eep_content.calib[5] = vdm::CalibRecord{0, 0, 0, vdm::kCalibFailed};
  for (unsigned v = 0; v < ACTUATOR_COUNT; v++) {
    myvalvemots[v].calibrated = 0;
    myvalvemots[v].status = VLV_STATE_UNKNOWN;
  }
  myvalvemots[2].calibSeq = 3;
  stub::sysstat.reason = vdm::BootReason::PowerOn;
  app_restore();
  CHECK(+myvalvemots[2].opening_count == 3600);
  CHECK(+myvalvemots[2].closing_count == 3650);
  CHECK(+myvalvemots[2].deadzone_count == 50);
  CHECK(+myvalvemots[2].scaler == 36);
  CHECK(+myvalvemots[2].meancurrent == 25);
  CHECK(+myvalvemots[2].calibrated == 1);
  CHECK(+myvalvemots[2].recal == 0);
  CHECK(+myvalves[2].calibRestored == 1);
  CHECK(+myvalves[2].storedSeq == 3);
  CHECK(+myvalvemots[2].status == VLV_STATE_UNKNOWN);
  CHECK(+myvalvemots[4].deadzone_count == -50);
  CHECK(+myvalvemots[4].recal == 1);
  CHECK(+myvalvemots[4].calibrated == 1);
  CHECK(+myvalvemots[5].calibrated == 0);
  CHECK(+myvalvemots[5].recal == 1);
  CHECK(+myvalvemots[0].calibrated == 0);
  CHECK(+myvalves[0].calibRestored == 0);
  CHECK(v3(2).flags == vdm::kVlvFlagCalRestored);
  CHECK(v3(0).flags == vdm::kVlvFlagUncalibrated);
  CHECK(v3(4).flags == (vdm::kVlvFlagCalRestored | vdm::kVlvFlagRecal));
}

TEST_CASE("app_restore W2/K1-8: a warm reset restores positions, holds, retries and the expired lease") {
  begin();
  app_set_failsafe(255, 40);
  myvalvemots[0].status = VLV_STATE_BLOCKS;
  myvalvemots[0].actual_position = 40;
  myvalvemots[0].target_position = 30;
  myvalves[1].assemblyHold = 1;
  myvalvemots[1].actual_position = 100;
  myvalvemots[1].target_position = 100;
  myvalvemots[2].needsReference = 1;
  myvalvemots[2].recal = 1;
  myvalvemots[3].status = VLV_STATE_OPENCIR;
  myvalvemots[5].status = VLV_STATE_CLOSING;   // moving: not referenced after the reset
  myvalvemots[6].status = VLV_STATE_IDLE;
  myvalvemots[6].actual_position = 77;
  stub::motor.busy = 6;                          // a command for valve 6 is handed over
  expireLease();
  app_1s_tick(10);                               // valve 0: its retry was scheduled by expireLease()
  app_1s_tick(100);
  app_warm_save();
  // the reset: RAM of app.cpp from app_setup() again, the calibration records of the EEPROM
  app_set_failsafe(255, 50);
  app_lease_configure(0);
  app_lease_configure(60);
  app_setup();
  valve_setup();
  for (unsigned v = 0; v < ACTUATOR_COUNT; v++) {
    myvalvemots[v].calibrated = 0;
    eep_content.calib[v] = vdm::CalibRecord{3600, 3600, 20, vdm::kCalibValid};
  }
  stub::sysstat.reason = vdm::BootReason::Pin;
  stub::eeprom.cfgFlags = vdm::kCfgSafetyCorrupt;
  app_restore();
  CHECK(+myvalvemots[0].status == VLV_STATE_BLOCKS);
  CHECK(+myvalvemots[0].actual_position == 40);
  CHECK(+myvalvemots[0].target_position == 30);
  CHECK(+myvalves[0].rejectedTarget == 30);
  CHECK(+myvalvemots[0].connected == 1);
  CHECK(v3(0).retryS == 3490);
  CHECK(v3(0).fsPct == 40);                  // block B damaged: the copy of the last run
  CHECK(+myvalves[1].assemblyHold == 1);
  CHECK(+myvalvemots[1].actual_position == 100);
  CHECK(+myvalvemots[2].needsReference == 1);
  CHECK(+myvalvemots[2].recal == 1);
  CHECK(+myvalvemots[2].status == VLV_STATE_IDLE);
  CHECK(+myvalvemots[3].status == VLV_STATE_OPENCIR);
  CHECK(+myvalvemots[3].connected == 0);
  CHECK(+myvalvemots[5].status == VLV_STATE_IDLE);
  CHECK(+myvalvemots[5].needsReference == 1);
  CHECK(+myvalvemots[6].needsReference == 1);
  CHECK(+myvalvemots[6].actual_position == 77);
  CHECK(+myvalvemots[7].needsReference == 0);
  CHECK(+myvalvemots[7].status == VLV_STATE_IDLE);
  // the lease stays expired (the source of its timeout is the default: the copy of 5 min is taken)
  CHECK(app_lease_timeout() == 5);
  CHECK(app_lease_state() == 2);
  // the failsafe applies at once: the first plain move goes to the failsafe position
  CHECK(loopActions() == stub::Calls{"appsetaction(c, 4, 10, 0)"});
}

TEST_CASE("app_restore: a power-on or an EEPROM lease source ignores the warm copies") {
  begin();
  myvalvemots[0].actual_position = 10;
  app_set_failsafe(255, 30);
  app_lease_configure(5);
  app_warm_save();
  myvalvemots[0].actual_position = 50;
  app_set_failsafe(255, 50);
  app_lease_configure(60);
  stub::sysstat.reason = vdm::BootReason::PowerOn;
  app_restore();
  CHECK(+myvalvemots[0].actual_position == 50);
  CHECK(app_failsafe_pct(0) == 50);
  CHECK(app_lease_timeout() == 60);
  stub::sysstat.reason = vdm::BootReason::Software;
  stub::eeprom.leaseSource = vdm::kLeaseSourceSettings;
  app_restore();
  CHECK(+myvalvemots[0].actual_position == 10);
  CHECK(app_failsafe_pct(0) == 50);   // block B was fine
  CHECK(app_lease_timeout() == 60);
  stub::eeprom.cfgFlags = vdm::kCfgReadFailed;
  app_restore();
  CHECK(app_failsafe_pct(0) == 30);
}

TEST_CASE("app_warm_moving C-8: a valve handed a command restores unreferenced") {
  begin();
  eep_content.calib[3] = vdm::CalibRecord{3600, 3600, 20, vdm::kCalibValid};
  eep_content.calib[4] = vdm::CalibRecord{3600, 3600, 20, vdm::kCalibValid};
  app_warm_moving(3);    // before any record: nothing to invalidate
  app_warm_save();
  app_warm_moving(4);
  app_warm_moving(12);
  stub::sysstat.reason = vdm::BootReason::IndependentWatchdog;
  app_restore();
  CHECK(+myvalvemots[3].needsReference == 0);
  CHECK(+myvalvemots[4].needsReference == 1);
  CHECK(+myvalvemots[4].status == VLV_STATE_IDLE);
}

TEST_CASE("app_warm_save: an invalid kept valve takes the cold path alone") {
  begin();
  myvalvemots[2].actual_position = 101;
  myvalvemots[2].status = VLV_STATE_UNKNOWN;
  myvalvemots[3].actual_position = 20;
  app_warm_save();
  myvalvemots[2].actual_position = 50;
  myvalvemots[3].actual_position = 50;
  stub::sysstat.reason = vdm::BootReason::Pin;
  app_restore();
  CHECK(+myvalvemots[2].actual_position == 50);
  CHECK(+myvalvemots[3].actual_position == 20);
}

TEST_CASE("app_stop S8: stops the valve, cancels its requests, leaves it in a service hold") {
  begin();
  myvalvemots[3].status = VLV_STATE_PRESENT;
  myvalves[3].forcedLearn = 1;
  myvalves[3].timedLearn = 1;
  myvalves[3].retryLearn = 1;
  myvalves[3].earlyLearn = 1;
  myvalvemots[3].calibration = 1;
  myvalvemots[3].calibState = calibStarted;
  myvalvemots[4].status = VLV_STATE_PRESENT;
  myvalvemots[4].calibrated = 0;
  myvalvemots[5].status = VLV_STATE_PRESENT;
  myvalvemots[5].recal = 1;
  myvalvemots[6].calibration = 1;
  myvalvemots[6].calibActive = 1;
  myvalvemots[6].calibState = calibInProgress;
  stub::motor.stop = 7;
  CHECK(app_stop(255) == 0);
  CHECK(stub::calls == stub::Calls{"appstop(255)"});
  CHECK(+myvalvemots[3].status == VLV_STATE_IDLE);
  CHECK(+myvalves[3].forcedLearn == 0);
  CHECK(+myvalves[3].timedLearn == 0);
  CHECK(+myvalves[3].retryLearn == 0);
  CHECK(+myvalves[3].earlyLearn == 0);
  CHECK(+myvalvemots[3].calibration == 0);
  CHECK(+myvalvemots[3].calibState == calibIdle);
  CHECK(+myvalvemots[4].status == VLV_STATE_PRESENT);   // first calibration still pending
  CHECK(+myvalvemots[5].status == VLV_STATE_PRESENT);
  CHECK(+myvalvemots[6].calibration == 1);              // the running calibration ends through the stop
  CHECK(+myvalves[7].svcHold == SVMOV_HOLD_10S);
  CHECK(+myvalves[3].svcHold == 0);
  stub::motor.stop = -1;
  app_stop(255);
  CHECK(+myvalves[0].svcHold == 0);
  stub::calls.clear();
  CHECK(app_stop(2) == 0);
  CHECK(stub::calls == stub::Calls{"appstop(2)"});
  CHECK(+myvalves[2].svcHold == SVMOV_HOLD_10S);
  CHECK(+myvalves[1].svcHold == 0);
  myvalves[1].forcedLearn = 1;
  app_stop(2);
  CHECK(+myvalves[1].forcedLearn == 1);
  stub::calls.clear();
  CHECK(app_stop(12) == -1);
  CHECK(stub::calls.empty());
  CHECK(app_stop(11) == 0);
  CHECK(v3(11).flags == vdm::kVlvFlagSvcHold);
}

TEST_CASE("protection guard C-4: trips on 3 valves suspend the limits, failed valves are tested again") {
  begin();
  for (unsigned v = 0; v < 3; v++) {
    myvalvemots[v].status = VLV_STATE_FAILED;
    myvalvemots[v].faultReason = (uint8_t) vdm::ValveFault::Short;
  }
  myvalvemots[4].status = VLV_STATE_FAILED;
  myvalvemots[4].faultReason = (uint8_t) vdm::ValveFault::MoveTimeout;
  myvalvemots[5].faultReason = (uint8_t) vdm::ValveFault::InrushTrip;   // reported only
  myvalvemots[0].tripSeq = 1;
  myvalvemots[1].tripSeq = 3;
  stub::sysstat.uptime = 100;
  app_loop();
  CHECK_FALSE(app_protect_suspended());
  CHECK_FALSE(protect_suspended);
  CHECK(+myvalvemots[0].status == VLV_STATE_FAILED);
  stub::sysstat.uptime = 699;
  myvalvemots[2].tripSeq = 1;
  app_loop();
  CHECK(app_protect_suspended());
  CHECK(protect_suspended);
  CHECK(+myvalvemots[0].status == VLV_STATE_UNKNOWN);
  CHECK(+myvalvemots[2].status == VLV_STATE_UNKNOWN);
  CHECK(+myvalves[1].rejectedTarget == VALVE_NO_TARGET);
  CHECK(+myvalvemots[4].status == VLV_STATE_FAILED);
  CHECK(+myvalvemots[5].status == VLV_STATE_IDLE);
}

TEST_CASE("protection guard: trips of one valve do not suspend") {
  begin();
  for (uint8_t i = 1; i <= 5; i++) {
    myvalvemots[0].tripSeq = i;
    app_loop();
  }
  CHECK_FALSE(app_protect_suspended());
}

TEST_CASE("safe mode S9: no command to the valve state machine, presence tests after it ends") {
  begin();
  for (unsigned v = 0; v < ACTUATOR_COUNT; v++) myvalvemots[v].status = VLV_STATE_UNKNOWN;
  myvalvemots[1].target_position = 80;
  stub::sysstat.safeMode = true;
  CHECK(loopActions(1000).empty());
  stub::sysstat.safeMode = false;
  CHECK(loopActions() == stub::Calls{"appsetaction(x, 0, 0, 0)"});
}

TEST_CASE("temperature hold S1: no new motor command while a cycle is due, up to 3 s") {
  begin();
  myvalvemots[0].target_position = 70;
  fake::advanceMs(59999);
  app_loop();
  CHECK_FALSE(temp_refresh_request);
  myvalvemots[0].target_position = 50;
  fake::advanceMs(1);
  CHECK(app_temp_age_s() == 60);
  myvalvemots[0].target_position = 70;
  CHECK(loopActions().empty());
  CHECK(temp_refresh_request);
  app_temp_cycle_done();
  CHECK(app_temp_age_s() == 0);
  CHECK(loopActions() == stub::Calls{"appsetaction(o, 0, 20, 0)"});
  CHECK_FALSE(temp_refresh_request);
  fake::advanceMs(5000);
  CHECK(app_temp_age_s() == 5);
  // a cycle that does not come: the hold ends after 3 s
  fake::advanceMs(55000);
  CHECK(loopActions().empty());
  fake::advanceMs(2999);
  CHECK(loopActions().empty());
  fake::advanceMs(1);
  CHECK(loopActions() == stub::Calls{"appsetaction(o, 0, 20, 0)"});
}

TEST_CASE("stdet W2-7: every valve is tested again and a present one calibrates fully") {
  begin();
  myvalves[3].svcHold = 4;
  app_scan_valves();
  CHECK(+myvalves[3].svcHold == 0);
  CHECK(+myvalves[3].retestRequest == RETEST_DETECT);
  app_loop();
  for (unsigned v = 0; v < ACTUATOR_COUNT; v++) {
    CAPTURE(v);
    CHECK(+myvalvemots[v].recal == 1);
    CHECK(+myvalvemots[v].status == (v == 0 ? VLV_STATE_UNKNOWN : VLV_STATE_UNKNOWN));
    CHECK(+myvalves[v].retestRequest == 0);
    CHECK(+myvalves[v].rejectedTarget == (v == 0 ? 50 : VALVE_NO_TARGET));
  }
}

TEST_CASE("app_learn_pending W12: the calibration requirement does not depend on the status") {
  begin();
  CHECK_FALSE(app_learn_pending(0, VLV_STATE_IDLE, false));
  CHECK(app_learn_pending(0, VLV_STATE_IDLE, true));
  CHECK(app_learn_pending(0, VLV_STATE_PRESENT, false));
  myvalvemots[0].calibrated = 0;
  CHECK(app_learn_pending(0, VLV_STATE_IDLE, false));
  CHECK(app_learn_pending(0, VLV_STATE_FULLOPEN, false));
  CHECK_FALSE(app_learn_pending(0, VLV_STATE_FAILED, false));
  CHECK_FALSE(app_learn_pending(0, VLV_STATE_BLOCKS, false));
  CHECK_FALSE(app_learn_pending(0, VLV_STATE_UNKNOWN, false));
  CHECK(app_service_move(0, 0, 100, 20) == -3);
  myvalvemots[0].calibrated = 1;
  myvalvemots[0].recal = 1;
  CHECK(app_learn_pending(0, VLV_STATE_IDLE, false));
  myvalvemots[0].recal = 0;
  myvalves[0].retryLearn = 1;
  CHECK(app_learn_pending(0, VLV_STATE_IDLE, false));
  myvalves[0].retryLearn = 0;
  myvalves[0].earlyLearn = 1;
  CHECK(app_learn_pending(0, VLV_STATE_IDLE, false));
  myvalves[0].earlyLearn = 0;
  myvalves[0].timedLearn = 1;
  CHECK(app_learn_pending(0, VLV_STATE_IDLE, false));
  myvalves[0].timedLearn = 0;
  myvalves[0].forcedLearn = 1;
  CHECK(app_learn_pending(0, VLV_STATE_IDLE, false));
  CHECK_FALSE(app_learn_pending(12, VLV_STATE_PRESENT, true));
}

TEST_CASE("staop W12-2: an uncalibrated valve opens fully and calibrates at its own target change") {
  begin();
  myvalvemots[4].calibrated = 0;
  app_set_failsafe(255, 255);
  app_set_valveopen(4);
  CHECK(loopActions() == stub::Calls{"appsetaction(p, 4, 0, 0)"});
  myvalvemots[4].status = VLV_STATE_IDLE;
  myvalvemots[4].actual_position = 100;
  CHECK(loopActions().empty());
  CHECK(app_learn_pending(4, VLV_STATE_IDLE, false));
  myvalvemots[4].target_position = 30;
  app_target_changed(4);
  CHECK(loopActions() == stub::Calls{"appsetaction(l, 4, 0, 0)"});
  CHECK(+myvalves[4].touched == 0);
  CHECK(+myvalves[4].rejectedTarget == 30);
}

TEST_CASE("app_get_valve_v3: flags of every source, an invalid valve") {
  begin();
  myvalvemots[1].calibrated = 0;
  myvalvemots[1].needsReference = 1;
  myvalvemots[1].recal = 1;
  myvalvemots[1].faultReason = 5;
  myvalves[1].svcHold = 1;
  myvalves[1].assemblyHold = 1;
  stub::motor.snapshot[1].diag.earlyRun.onMove(true);
  valve_v3_info info = v3(1);
  CHECK(info.flags == (vdm::kVlvFlagUncalibrated | vdm::kVlvFlagNeedsRef | vdm::kVlvFlagRecal |
                       vdm::kVlvFlagEarlyPending | vdm::kVlvFlagAssembly | vdm::kVlvFlagSvcHold));
  CHECK(info.fault == 5);
  stub::motor.snapshot[1].diag.earlyRun.onMove(true);   // the run starts again
  CHECK((v3(1).flags & vdm::kVlvFlagEarlyPending) == 0);
  info = v3(12);
  CHECK(info.flags == 0);
  CHECK(info.fault == 0);
  CHECK(info.fsPct == 255);
  CHECK(info.drive == 0);
  CHECK(info.retryS == 0);
  CHECK(info.retries == 0);
}

TEST_CASE("app_load_config: lease timeout, failsafe positions and learn time from the EEPROM mirror") {
  glue::begin();
  stub::eeprom.leaseSource = vdm::kLeaseSourceSettings;
  eep_content.leaseTimeoutMin = 30;
  eep_content.learnTimeS = 3600;
  for (unsigned v = 0; v < ACTUATOR_COUNT; v++) eep_content.failsafePct[v] = static_cast<uint8_t>(v * 10);
  eep_content.failsafePct[11] = 101;
  app_setup();
  CHECK(app_lease_timeout() == 30);
  CHECK(app_get_learntime() == 3600);
  CHECK(+myvalves[11].learn_time == 3600);
  CHECK(app_failsafe_pct(0) == 0);
  CHECK(app_failsafe_pct(10) == 100);
  CHECK(app_failsafe_pct(11) == 50);
  CHECK(+eep_content.failsafePct[11] == 50);
  eep_content.leaseTimeoutMin = 3;
  app_load_config();
  CHECK(app_lease_timeout() == 60);
  CHECK(eep_content.leaseTimeoutMin == 60);
  stub::eeprom.leaseSource = vdm::kLeaseSourceSafety;
  eep_content.leaseTimeoutMin = 0;
  app_load_config();
  CHECK(app_lease_timeout() == 0);
  stub::eeprom.leaseSource = vdm::kLeaseSourceDefault;
  app_load_config();
  CHECK(app_lease_timeout() == 60);
  // the countdowns start again only for a changed learn time
  myvalves[0].learn_time = 7;
  app_load_config();
  CHECK(+myvalves[0].learn_time == 7);
}
