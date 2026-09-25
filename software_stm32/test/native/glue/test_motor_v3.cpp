// src/motor.cpp with the valve sim (glue_motor): partial end stops (W9), keep-status and reference
// moves (K2, W2), sstop (S8), calibration results (W2, S2, S14), presence test short (W10), inrush
// limit (C-4, report only and enforced), the temperature gap between strokes (S1).
#include "glue_test.h"
#include "motor.h"
#include "stub_app.h"
#include "valve_sim.h"
#include "vdm/valve_codes.h"

void TimerHandler0();

namespace {

void startValves(sim::Rig& rig) {
  glue::begin();
  analogReadResolution(12);
  valve_pins_safe();
  valve_setup();
  rig.install();
  rig.runMs(20);
  REQUIRE(+valvestate == A_IDLE);
  rig.transitions.clear();
  stub::calls.clear();
}

bool idle() { return valve_idle(); }

// a valve that knows its stroke: scaler 36, standing at pct % (sim position pct x 36)
void place(sim::Rig& rig, unsigned v, uint8_t pct) {
  myvalvemots[v].scaler = 36;
  myvalvemots[v].opening_count = 3600;
  myvalvemots[v].closing_count = 3600;
  myvalvemots[v].status = VLV_STATE_IDLE;
  myvalvemots[v].actual_position = pct;
  myvalvemots[v].target_position = pct;
  rig.valve[v].position = pct * 36;
}

void run(sim::Rig& rig, char cmd, unsigned v, uint8_t pos, uint8_t flags = 0) {
  REQUIRE(appsetaction(cmd, v, pos, false, flags) == 0);
  rig.runMs(10);
  REQUIRE(rig.runUntil(idle, 200000));
}

vdm::MoveResult last(unsigned v) {
  valve_snapshot s;
  valve_get_snapshot(v, s);
  return s.diag.last;
}

valve_diag diag(unsigned v) {
  valve_snapshot s;
  valve_get_snapshot(v, s);
  return s.diag;
}

// states of valve_loop in order ("5 6 17 7")
std::string states(const sim::Rig& rig) {
  std::string out;
  for (const sim::Rig::Step& s : rig.transitions) {
    if (s.valve != 255) continue;
    if (!out.empty()) out += ' ';
    out += std::to_string(s.valvestate);
  }
  return out;
}

}  // namespace

TEST_CASE("W9: a partial move that meets an obstacle takes its position from the counted pulses") {
  sim::Rig rig;
  startValves(rig);
  place(rig, 0, 20);
  rig.valve[0].jamFrom = 900;  // 25 %
  rig.valve[0].jamTo = 3600;
  run(rig, CMD_A_OPEN, 0, 40);
  CHECK(+myvalvemots[0].actual_position == 25);
  CHECK(+myvalvemots[0].status == VLV_STATE_IDLE);
  CHECK(last(0).stopReason == (uint8_t) vdm::StopReason::EarlyEndStop);
  CHECK(last(0).requestedCounts == 40 * 36);
  CHECK(diag(0).earlyStops == 1);
  CHECK(diag(0).earlyWarn);
  CHECK(diag(0).lastEarly);
  CHECK(diag(0).earlyRun.count() == 1);
  CHECK(+myvalvemots[0].earlyLearnDue == 0);
  CHECK(+myvalvemots[0].moveSeq == 1);
  // the retry stops early again: a calibration is requested
  run(rig, CMD_A_OPEN, 0, 35);
  CHECK(+myvalvemots[0].actual_position == 25);
  CHECK(diag(0).earlyStops == 2);
  CHECK(diag(0).earlyRun.count() == 0);
  CHECK(+myvalvemots[0].earlyLearnDue == 1);
  CHECK(+myvalvemots[0].moveSeq == 2);
}

TEST_CASE("W9: an end stop after 80 % of the requested pulses is not early, the position still counted") {
  sim::Rig rig;
  startValves(rig);
  place(rig, 0, 60);
  rig.valve[0].stroke = 3420;  // the open end stop at 95 %
  run(rig, CMD_A_OPEN, 0, 40);
  CHECK(last(0).stopReason == (uint8_t) vdm::StopReason::EndStop);
  CHECK_FALSE(diag(0).lastEarly);
  CHECK(diag(0).earlyStops == 0);
  CHECK(diag(0).earlyRun.count() == 0);
  CHECK(+myvalvemots[0].actual_position == 60 + (3420 - 60 * 36) / 36);
}

TEST_CASE("K2: a keep-status move of a blocked valve keeps status 9 and never feeds the early run") {
  sim::Rig rig;
  startValves(rig);
  place(rig, 1, 0);
  myvalvemots[1].status = VLV_STATE_BLOCKS;
  rig.valve[1].jamFrom = 1008;  // 28 %
  rig.valve[1].jamTo = 3600;
  run(rig, CMD_A_OPEN, 1, 50, MOVE_KEEP_STATUS);
  CHECK(+myvalvemots[1].status == VLV_STATE_BLOCKS);
  for (const sim::Rig::Step& s : rig.transitions) CHECK_FALSE((s.valve == 1 && s.status != VLV_STATE_BLOCKS));  // 9 also while it moves
  CHECK(+myvalvemots[1].actual_position == 28);
  CHECK(last(1).stopReason == (uint8_t) vdm::StopReason::EndStop);
  CHECK(diag(1).earlyStops == 0);
  CHECK(diag(1).earlyRun.count() == 0);
  CHECK(+myvalvemots[1].moveSeq == 1);
  // a free move to the target keeps it too
  rig.valve[1].jamFrom = -1;
  run(rig, CMD_A_OPEN, 1, 10, MOVE_KEEP_STATUS);
  CHECK(+myvalvemots[1].status == VLV_STATE_BLOCKS);
  CHECK(+myvalvemots[1].actual_position == 38);
  CHECK(last(1).stopReason == (uint8_t) vdm::StopReason::Target);
  run(rig, CMD_A_CLOSE_END, 1, 0, MOVE_KEEP_STATUS);
  CHECK(+myvalvemots[1].status == VLV_STATE_BLOCKS);
  CHECK(+myvalvemots[1].actual_position == 0);
  CHECK_FALSE(diag(1).lastEarly);
}

TEST_CASE("W2: a reference move to the end stop is never early and references the valve") {
  sim::Rig rig;
  startValves(rig);
  place(rig, 2, 30);
  rig.valve[2].position = 3000;   // the valve really stands near the open end
  myvalvemots[2].needsReference = 1;
  run(rig, CMD_A_OPEN_END, 2, 0, MOVE_REFERENCE);
  CHECK(+myvalvemots[2].actual_position == 100);
  CHECK(+myvalvemots[2].needsReference == 0);
  CHECK(last(2).stopReason == (uint8_t) vdm::StopReason::EndStop);
  CHECK(diag(2).earlyStops == 0);
  // without the flag the same short stroke is an early end stop of an end-stop move
  place(rig, 3, 30);
  rig.valve[3].position = 3400;
  run(rig, CMD_A_OPEN_END, 3, 0);
  CHECK(last(3).stopReason == (uint8_t) vdm::StopReason::EarlyEndStop);
  CHECK(+myvalvemots[3].actual_position == 100);
}

TEST_CASE("W2: a move to an end stop that ends at the end stop clears needsReference, a partial one not") {
  sim::Rig rig;
  startValves(rig);
  place(rig, 4, 50);
  myvalvemots[4].needsReference = 1;
  run(rig, CMD_A_OPEN, 4, 10);
  CHECK(+myvalvemots[4].needsReference == 1);
  run(rig, CMD_A_CLOSE_END, 4, 0);
  CHECK(+myvalvemots[4].needsReference == 0);
  CHECK(+myvalvemots[4].actual_position == 0);
}

TEST_CASE("S8: sstop during a service move stops it where the pulses took it") {
  sim::Rig rig;
  startValves(rig);
  place(rig, 2, 10);
  myvalves[2].svcHold = 0;
  REQUIRE(appsetservice(2, vdm::kDirOpen, 10000, 40) == 0);
  rig.runMs(2000);
  CHECK(appstop(5) == -1);
  CHECK(appstop(2) == 2);
  REQUIRE(rig.runUntil(idle, 5000));
  const vdm::MoveResult r = last(2);
  CHECK(r.stopReason == (uint8_t) vdm::StopReason::Aborted);
  CHECK(r.countedCounts > 100);
  CHECK(r.countedCounts < 10000);
  CHECK(+myvalvemots[2].actual_position == 10 + r.countedCounts / 36);
  CHECK(+myvalvemots[2].status == VLV_STATE_IDLE);
  CHECK(appstop(255) == -1);
}

TEST_CASE("S8: sstop of a normal move and of a keep-status move") {
  sim::Rig rig;
  startValves(rig);
  place(rig, 0, 10);
  rig.valve[0].pulsesPerMs = 0.1f;
  REQUIRE(appsetaction(CMD_A_OPEN, 0, 80) == 0);
  rig.runMs(3000);
  CHECK(appstop(255) == 0);
  REQUIRE(rig.runUntil(idle, 5000));
  CHECK(last(0).stopReason == (uint8_t) vdm::StopReason::Aborted);
  CHECK(+myvalvemots[0].actual_position == 10 + last(0).countedCounts / 36);
  CHECK(+myvalvemots[0].actual_position < 90);
  CHECK(+myvalvemots[0].status == VLV_STATE_IDLE);
  CHECK(+myvalvemots[0].moveSeq == 1);
  myvalvemots[0].status = VLV_STATE_BLOCKS;
  REQUIRE(appsetaction(CMD_A_CLOSE_END, 0, 0, false, MOVE_KEEP_STATUS) == 0);
  rig.runMs(3000);
  CHECK(appstop(0) == 0);
  REQUIRE(rig.runUntil(idle, 5000));
  CHECK(+myvalvemots[0].status == VLV_STATE_BLOCKS);
  CHECK(last(0).stopReason == (uint8_t) vdm::StopReason::Aborted);
}

TEST_CASE("S8: sstop before the motor started: the command is dropped or the start refused") {
  sim::Rig rig;
  startValves(rig);
  place(rig, 3, 40);
  REQUIRE(appsetaction(CMD_A_OPEN, 3, 20) == 0);
  CHECK(valve_busy_index() == 3);
  CHECK(appstop(3) == 3);   // not taken yet: dropped
  CHECK(valve_idle());
  CHECK(valve_busy_index() == -1);
  rig.runMs(100);
  CHECK(rig.valve[3].enables == 0);
  // taken, waiting for the PSU (A_OPEN1)
  REQUIRE(appsetaction(CMD_A_OPEN, 3, 20) == 0);
  rig.runMs(20);
  REQUIRE(+valvestate == A_OPEN1);
  CHECK(valve_busy_index() == 3);
  CHECK(appstop(3) == 3);
  rig.runMs(20);
  CHECK(+valvestate == A_IDLE);
  CHECK(last(3).stopReason == (uint8_t) vdm::StopReason::Aborted);
  CHECK(last(3).countedCounts == 0);
  CHECK(+myvalvemots[3].actual_position == 40);
  CHECK(rig.valve[3].enables == 0);
  // a service move waiting for the PSU
  REQUIRE(appsetservice(3, vdm::kDirClose, 100, 30) == 0);
  rig.runMs(20);
  REQUIRE(+valvestate == A_SVC1);
  CHECK(appstop(255) == 3);
  rig.runMs(20);
  CHECK(+valvestate == A_IDLE);
  CHECK(rig.valve[3].enables == 0);
}

TEST_CASE("S8: sstop in a calibration ends it without a result, the position unreferenced") {
  sim::Rig rig;
  startValves(rig);
  place(rig, 0, 50);
  myvalvemots[0].calibrated = 1;
  REQUIRE(appsetaction(CMD_A_LEARN, 0, 0) == 0);
  myvalvemots[0].calibration = 1;
  myvalvemots[0].calibState = calibInProgress;
  rig.runMs(1500);
  REQUIRE(+myvalvemots[0].calibActive == 1);
  CHECK(appstop(0) == 0);
  REQUIRE(rig.runUntil(idle, 5000));
  CHECK(+myvalvemots[0].status == VLV_STATE_IDLE);
  CHECK(+myvalvemots[0].needsReference == 1);
  CHECK(+myvalvemots[0].calibActive == 0);
  CHECK(+myvalvemots[0].calibration == 0);
  CHECK(+myvalvemots[0].calibState == calibIdle);
  CHECK(+myvalvemots[0].calibSeq == 0);
  CHECK_FALSE(diag(0).lastCalFailed);
  CHECK(last(0).stopReason == (uint8_t) vdm::StopReason::Aborted);
  // stopped right after the hand-over (A_LEARN1)
  REQUIRE(appsetaction(CMD_A_LEARN, 0, 0) == 0);
  rig.runMs(10);
  REQUIRE(+valvestate == A_LEARN1);
  appstop(0);
  rig.runMs(10);
  CHECK(+valvestate == A_IDLE);
  CHECK(+myvalvemots[0].calibActive == 0);
}

TEST_CASE("W2, S14: an accepted calibration marks the valve calibrated and restarts the movement trigger") {
  sim::Rig rig;
  startValves(rig);
  rig.valve[0].pulsesPerMs = 1.0f;
  learning_movements = 77;
  myvalves[0].movements = 5;
  myvalves[0].learn_movements = 3;
  myvalvemots[0].recal = 1;
  myvalvemots[0].needsReference = 1;
  myvalvemots[0].faultReason = 4;
  myvalvemots[0].status = VLV_STATE_PRESENT;
  REQUIRE(appsetaction(CMD_A_LEARN, 0, 0) == 0);
  rig.runMs(10);
  REQUIRE(rig.runUntil([] { return myvalvemots[0].calibActive == 0 && valvestate == A_IDLE; }, 30000));
  CHECK(+myvalvemots[0].calibrated == 1);
  CHECK(+myvalvemots[0].recal == 0);
  CHECK(+myvalvemots[0].needsReference == 0);
  CHECK(+myvalvemots[0].faultReason == 0);
  CHECK(+myvalvemots[0].calibFailed == 0);
  CHECK(+myvalvemots[0].calibSeq == 1);
  CHECK(+myvalves[0].movements == 0);          // S14-1: gvlvx moves 0 after every successful calibration
  CHECK(+myvalves[0].learn_movements == 77);
  CHECK(+myvalvemots[0].status == VLV_STATE_IDLE);
}

TEST_CASE("K2, W2: strokes too short block the valve, the record is marked failed") {
  sim::Rig rig;
  startValves(rig);
  rig.valve[0].pulsesPerMs = 1.0f;
  rig.valve[0].stroke = 2500;
  rig.valve[0].position = 1200;
  noOfMinCounts = 3000;
  myvalvemots[0].target_position = 40;
  REQUIRE(appsetaction(CMD_A_LEARN, 0, 0) == 0);
  rig.runMs(10);
  REQUIRE(rig.runUntil([] { return myvalvemots[0].calibActive == 0 && valvestate == A_IDLE; }, 30000));
  CHECK(+myvalvemots[0].status == VLV_STATE_BLOCKS);
  CHECK(+myvalvemots[0].faultReason == (uint8_t) vdm::ValveFault::StrokesTooShort);
  CHECK(+myvalvemots[0].calibFailed == 1);
  CHECK(+myvalvemots[0].calibSeq == 1);
  CHECK(+myvalvemots[0].calibrated == 0);
  CHECK(+myvalvemots[0].actual_position == 0);
  CHECK(+myvalvemots[0].scaler == 89);
  CHECK(diag(0).lastCalFailed);
}

TEST_CASE("S2: contact lost in a calibration keeps the target, the valve needs a full calibration") {
  sim::Rig rig;
  startValves(rig);
  rig.valve[3].connected = false;
  myvalvemots[3].calibrated = 1;
  myvalvemots[3].actual_position = 20;
  myvalvemots[3].target_position = 70;
  REQUIRE(appsetaction(CMD_A_LEARN, 3, 0) == 0);
  rig.runMs(10);
  REQUIRE(rig.runUntil(idle, 30000));
  CHECK(+myvalvemots[3].status == VLV_STATE_OPENCIR);
  CHECK(+myvalvemots[3].target_position == 70);
  CHECK(+myvalvemots[3].actual_position == 70);
  CHECK(+myvalvemots[3].recal == 1);
  CHECK(+myvalvemots[3].connected == 0);
  CHECK(+myvalvemots[3].calibSeq == 0);
  // in a move and a service move too
  place(rig, 4, 30);
  rig.valve[4].connected = false;
  run(rig, CMD_A_CLOSE, 4, 10);
  CHECK(+myvalvemots[4].recal == 1);
  CHECK(+myvalvemots[4].actual_position == 30);
  place(rig, 5, 30);
  rig.valve[5].connected = false;
  REQUIRE(appsetservice(5, vdm::kDirOpen, 100, 30) == 0);
  rig.runMs(10);
  REQUIRE(rig.runUntil(idle, 30000));
  CHECK(+myvalvemots[5].recal == 1);
  CHECK(+myvalvemots[5].status == VLV_STATE_OPENCIR);
}

TEST_CASE("presence test: a calibrated valve is idle and unreferenced, an absent one takes its target") {
  sim::Rig rig;
  startValves(rig);
  myvalvemots[2].calibrated = 1;
  myvalvemots[2].faultReason = 1;
  run(rig, CMD_A_TEST, 2, 0);
  CHECK(+myvalvemots[2].status == VLV_STATE_IDLE);
  CHECK(+myvalvemots[2].needsReference == 1);
  CHECK(+myvalvemots[2].faultReason == 0);
  myvalvemots[2].recal = 1;
  run(rig, CMD_A_TEST, 2, 0);
  CHECK(+myvalvemots[2].status == VLV_STATE_PRESENT);
  rig.valve[3].connected = false;
  myvalvemots[3].actual_position = 0;
  myvalvemots[3].target_position = 35;
  myvalvemots[3].faultReason = 3;
  run(rig, CMD_A_TEST, 3, 0);
  CHECK(+myvalvemots[3].status == VLV_STATE_OPENCIR);
  CHECK(+myvalvemots[3].actual_position == 35);
  CHECK(+myvalvemots[3].faultReason == 0);
  CHECK(+myvalvemots[3].tripSeq == 0);
}

TEST_CASE("W10: a short in the presence test is reported only by default") {
  sim::Rig rig;
  startValves(rig);
  CHECK_FALSE(protect_enforce);
  rig.valve[4].shorted = true;
  rig.valve[4].shortCurrent_dmA = 2600;
  run(rig, CMD_A_TEST, 4, 0);
  CHECK(+myvalvemots[4].status == VLV_STATE_PRESENT);
  CHECK(+myvalvemots[4].faultReason == (uint8_t) vdm::ValveFault::Short);
  CHECK(+myvalvemots[4].tripSeq == 1);
  // a healthy motor
  run(rig, CMD_A_TEST, 5, 0);
  CHECK(+myvalvemots[5].faultReason == 0);
  CHECK(+myvalvemots[5].tripSeq == 0);
}

TEST_CASE("W10: an enforced short fails the valve, the protection guard switches the check off") {
  sim::Rig rig;
  startValves(rig);
  protect_enforce = true;
  rig.valve[4].shorted = true;
  rig.valve[4].shortCurrent_dmA = 2600;
  run(rig, CMD_A_TEST, 4, 0);
  CHECK(+myvalvemots[4].status == VLV_STATE_FAILED);
  CHECK(+myvalvemots[4].faultReason == (uint8_t) vdm::ValveFault::Short);
  CHECK(+myvalvemots[4].tripSeq == 1);
  CHECK(rig.valve[4].pulses == 0);
  protect_suspended = true;
  run(rig, CMD_A_TEST, 4, 0);
  CHECK(+myvalvemots[4].status == VLV_STATE_PRESENT);
  CHECK(+myvalvemots[4].faultReason == 0);
  CHECK(+myvalvemots[4].tripSeq == 1);
}

TEST_CASE("C-4: an inrush above the limit is reported only by default, the move goes on") {
  sim::Rig rig;
  startValves(rig);
  place(rig, 6, 20);
  rig.valve[6].inrushPeak_dmA = 2800;
  rig.valve[6].inrushMs = 60;
  run(rig, CMD_A_OPEN, 6, 20);
  CHECK(+myvalvemots[6].status == VLV_STATE_IDLE);
  CHECK(+myvalvemots[6].actual_position == 40);
  CHECK(+myvalvemots[6].faultReason == (uint8_t) vdm::ValveFault::InrushTrip);
  CHECK(+myvalvemots[6].tripSeq == 1);
  CHECK(last(6).stopReason == (uint8_t) vdm::StopReason::Target);
  // a short inrush peak is normal
  place(rig, 7, 20);
  rig.valve[7].inrushPeak_dmA = 2800;
  rig.valve[7].inrushMs = 15;
  run(rig, CMD_A_OPEN, 7, 20);
  CHECK(+myvalvemots[7].faultReason == 0);
  CHECK(+myvalvemots[7].tripSeq == 0);
}

TEST_CASE("C-4: an enforced inrush trip fails the move at its start, the guard switches it off") {
  sim::Rig rig;
  startValves(rig);
  protect_enforce = true;
  place(rig, 6, 20);
  rig.valve[6].inrushPeak_dmA = 2800;
  rig.valve[6].inrushMs = 60;
  run(rig, CMD_A_OPEN, 6, 20);
  CHECK(+myvalvemots[6].status == VLV_STATE_FAILED);
  CHECK(+myvalvemots[6].faultReason == (uint8_t) vdm::ValveFault::InrushTrip);
  CHECK(+myvalvemots[6].actual_position == 20);
  CHECK(last(6).stopReason == (uint8_t) vdm::StopReason::SafetyOvercurrent);
  CHECK(last(6).peakCurrent >= 2500);
  CHECK(+myvalvemots[6].tripSeq == 1);
  protect_suspended = true;
  myvalvemots[6].status = VLV_STATE_IDLE;
  myvalvemots[6].faultReason = 0;
  run(rig, CMD_A_OPEN, 6, 20);
  CHECK(+myvalvemots[6].status == VLV_STATE_IDLE);
  CHECK(+myvalvemots[6].actual_position == 40);
  CHECK(+myvalvemots[6].faultReason == 0);
}

TEST_CASE("C-4: an enforced inrush trip in a calibration stroke fails it without BLOCKS or counts") {
  sim::Rig rig;
  startValves(rig);
  protect_enforce = true;
  rig.valve[0].inrushPeak_dmA = 2800;
  rig.valve[0].inrushMs = 60;
  REQUIRE(appsetaction(CMD_A_LEARN, 0, 0) == 0);
  rig.runMs(10);
  REQUIRE(rig.runUntil(idle, 30000));
  CHECK(+myvalvemots[0].status == VLV_STATE_FAILED);
  CHECK(+myvalvemots[0].faultReason == (uint8_t) vdm::ValveFault::InrushTrip);
  CHECK(+myvalvemots[0].calibSeq == 0);
  CHECK(+myvalvemots[0].calibActive == 0);
  CHECK(+myvalvemots[0].calibration == 0);
  CHECK(+myvalvemots[0].opening_count == 0);
  CHECK(diag(0).lastCalFailed);
}

TEST_CASE("S1: a due temperature cycle pauses the calibration between the strokes, at most 3 s") {
  sim::Rig rig;
  startValves(rig);
  rig.valve[0].pulsesPerMs = 1.0f;
  temp_refresh_request = true;
  REQUIRE(appsetaction(CMD_A_LEARN, 0, 0) == 0);
  rig.runMs(10);
  REQUIRE(rig.runUntil([] { return valvestate == A_GAP; }, 30000));
  CHECK(stub::callsOf("temp_command").back() == "temp_command(3)");
  const uint32_t gapStart = rig.ms;
  REQUIRE(rig.runUntil([] { return valvestate != A_GAP; }, 10000));
  CHECK(rig.ms - gapStart >= 2990);
  CHECK(rig.ms - gapStart <= 3010);
  CHECK(stub::callsOf("temp_command").back() == "temp_command(2)");
  CHECK(+valvestate == A_LEARN3);
  // the request ends: the next stroke starts at once
  REQUIRE(rig.runUntil([] { return valvestate == A_GAP; }, 30000));
  rig.runMs(100);
  temp_refresh_request = false;
  rig.runMs(20);
  CHECK(+valvestate == A_LEARN4);
  REQUIRE(rig.runUntil([] { return myvalvemots[0].calibActive == 0 && valvestate == A_IDLE; }, 30000));
  CHECK(states(rig) == "5 6 17 7 17 8 9 10 11 1");
  CHECK(+myvalvemots[0].status == VLV_STATE_IDLE);
  CHECK(+myvalvemots[0].calibSeq == 1);
}

TEST_CASE("S1: sstop in the temperature gap ends the calibration") {
  sim::Rig rig;
  startValves(rig);
  rig.valve[0].pulsesPerMs = 1.0f;
  temp_refresh_request = true;
  REQUIRE(appsetaction(CMD_A_LEARN, 0, 0) == 0);
  rig.runMs(10);
  REQUIRE(rig.runUntil([] { return valvestate == A_GAP; }, 30000));
  CHECK(appstop(0) == 0);
  rig.runMs(20);
  CHECK(+valvestate == A_IDLE);
  CHECK(+myvalvemots[0].needsReference == 1);
  CHECK(+myvalvemots[0].calibActive == 0);
  CHECK(stub::callsOf("temp_command").back() == "temp_command(2)");
}

TEST_CASE("C-8: appsetaction and appsetservice invalidate the kept position of their valve") {
  sim::Rig rig;
  startValves(rig);
  place(rig, 3, 40);
  REQUIRE(appsetaction(CMD_A_OPEN, 3, 10, false, MOVE_KEEP_STATUS | MOVE_REFERENCE) == 0);
  CHECK(stub::calls == stub::Calls{"app_warm_moving(3)"});
  CHECK(appsetaction(CMD_A_OPEN, 4, 10) == -1);
  CHECK(stub::calls == stub::Calls{"app_warm_moving(3)"});
  REQUIRE(rig.runUntil(idle, 10000));
  stub::calls.clear();
  REQUIRE(appsetservice(5, vdm::kDirOpen, 10, 30) == 0);
  CHECK(stub::calls == stub::Calls{"app_warm_moving(5)"});
  CHECK(valve_busy_index() == 5);
}
