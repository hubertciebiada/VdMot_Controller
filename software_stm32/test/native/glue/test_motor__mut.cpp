// src/motor.cpp edges with the valve sim (glue_motor): clamped positions, early checks of moves to
// an end stop and of calibration strokes, service move bounds and counts, argument checks, PSU and
// temperature lock timing, calibration retries and results, the motor state machine and TimerHandler0.
#include "glue_test.h"
#include "motor.h"
#include "stub_app.h"
#include "valve_sim.h"
#include "vdm/valve_codes.h"

void TimerHandler0();
byte motorcycle(int mvalvenr, byte cmd);
void callback_motorstop();
extern volatile byte isr_stop_request;
extern volatile byte isr_turning;
extern volatile byte isr_timer_go;
extern volatile byte isr_timer_fin;
extern volatile byte isr_overcurrentevent;
extern volatile int isr_valvenr;
extern volatile int current_mA;
extern volatile int analog_current;
extern volatile int analog_current_old;
extern uint8_t calibRetries;
extern uint8_t maxCalibRetries;

namespace {

constexpr byte kResIdle = 2;
constexpr byte kResOpens = 3;
constexpr byte kResError = 11;

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

void service(sim::Rig& rig, unsigned v, uint8_t dir, uint16_t counts, uint8_t maxmA) {
  REQUIRE(appsetservice(v, dir, counts, maxmA) == 0);
  rig.runMs(10);
  REQUIRE(rig.runUntil(idle, 200000));
}

void learn(sim::Rig& rig, unsigned v) {
  REQUIRE(appsetaction(CMD_A_LEARN, v, 0) == 0);
  rig.runMs(10);
  REQUIRE(rig.runUntil([v] { return myvalvemots[v].calibActive == 0 && valvestate == A_IDLE; }, 2000000));
}

valve_diag diag(unsigned v) {
  valve_snapshot s;
  valve_get_snapshot(v, s);
  return s.diag;
}

vdm::MoveResult last(unsigned v) { return diag(v).last; }

// ms from now until the valve PSU goes off
uint32_t msToPsuOff(sim::Rig& rig) {
  const uint32_t start = rig.ms;
  REQUIRE(rig.runUntil([&rig] { return !rig.powered(); }, 40000));
  return rig.ms - start;
}

const sim::Rig::Step* stateStep(const sim::Rig& rig, uint8_t state, size_t from = 0) {
  for (size_t i = from; i < rig.transitions.size(); i++) {
    const sim::Rig::Step& s = rig.transitions[i];
    if (s.valve == 255 && s.valvestate == state) return &s;
  }
  return nullptr;
}

}  // namespace

// ------------------------------------------------------------------ positions of normal moves

TEST_CASE("normal move: the position is clamped to 0..100 % at the target count") {
  sim::Rig rig;
  startValves(rig);
  for (unsigned v = 0; v < 4; v++) rig.valve[v].stroke = 100000;
  place(rig, 0, 95);
  run(rig, CMD_A_OPEN, 0, 10);
  CHECK(last(0).stopReason == (uint8_t) vdm::StopReason::Target);
  CHECK(+myvalvemots[0].actual_position == 100);
  place(rig, 1, 91);
  run(rig, CMD_A_OPEN, 1, 10);
  CHECK(+myvalvemots[1].actual_position == 100);
  place(rig, 2, 60);
  run(rig, CMD_A_CLOSE, 2, 10);
  CHECK(last(2).stopReason == (uint8_t) vdm::StopReason::Target);
  CHECK(+myvalvemots[2].actual_position == 50);
  place(rig, 3, 20);
  rig.valve[3].position = 3000;
  run(rig, CMD_A_CLOSE, 3, 30);
  CHECK(last(3).stopReason == (uint8_t) vdm::StopReason::Target);
  CHECK(+myvalvemots[3].actual_position == 0);
}

TEST_CASE("normal move: the learning counter counts down to 0 and stays there") {
  sim::Rig rig;
  startValves(rig);
  place(rig, 0, 20);
  myvalves[0].learn_movements = 3;
  run(rig, CMD_A_OPEN, 0, 5);
  CHECK(+myvalves[0].learn_movements == 2);
  myvalves[0].learn_movements = 0;
  run(rig, CMD_A_OPEN, 0, 5);
  CHECK(+myvalves[0].learn_movements == 0);
  CHECK(+myvalves[0].movements == 2);
}

// ------------------------------------------------------------------ early checks of moves to an end stop

TEST_CASE("move to the end stop: early below half of the learned travel from the start position") {
  sim::Rig rig;
  startValves(rig);
  // from 20 %: expected 80 % of 3600 counts, early below 1440 counts
  place(rig, 0, 20);
  rig.valve[0].jamFrom = 720 + 1450;
  rig.valve[0].jamTo = 100000;
  run(rig, CMD_A_OPEN_END, 0, 0);
  CHECK(last(0).stopReason == (uint8_t) vdm::StopReason::EndStop);
  CHECK_FALSE(diag(0).lastEarly);
  place(rig, 1, 20);
  myvalvemots[1].closing_count = 200;   // only the travel of the move direction counts
  rig.valve[1].jamFrom = 720 + 1430;
  rig.valve[1].jamTo = 100000;
  run(rig, CMD_A_OPEN_END, 1, 0);
  CHECK(last(1).stopReason == (uint8_t) vdm::StopReason::EarlyEndStop);
  CHECK(diag(1).lastEarly);
  // a move to the end stop never feeds the run of early partial stops
  CHECK(diag(1).earlyRun.count() == 0);
  CHECK(diag(1).earlyStops == 1);
  CHECK(+myvalvemots[1].actual_position == 100);
}

TEST_CASE("move to the end stop: no early check without a valid learned travel") {
  sim::Rig rig;
  startValves(rig);
  // scaler 0: nothing learned
  place(rig, 0, 20);
  myvalvemots[0].scaler = 0;
  rig.valve[0].jamFrom = 720 + 100;
  rig.valve[0].jamTo = 100000;
  run(rig, CMD_A_OPEN_END, 0, 0);
  CHECK(last(0).stopReason == (uint8_t) vdm::StopReason::EndStop);
  CHECK_FALSE(diag(0).lastEarly);
  // scaler 1 is valid
  place(rig, 1, 20);
  myvalvemots[1].scaler = 1;
  rig.valve[1].jamFrom = 720 + 100;
  rig.valve[1].jamTo = 100000;
  run(rig, CMD_A_OPEN_END, 1, 0);
  CHECK(diag(1).lastEarly);
  // a travel of exactly kMinTravelCounts is valid
  place(rig, 2, 20);
  myvalvemots[2].opening_count = vdm::kMinTravelCounts;
  rig.valve[2].jamFrom = 720 + 10;
  rig.valve[2].jamTo = 100000;
  run(rig, CMD_A_OPEN_END, 2, 0);
  CHECK(diag(2).lastEarly);
  // one count less is not
  place(rig, 3, 20);
  myvalvemots[3].opening_count = vdm::kMinTravelCounts - 1;
  rig.valve[3].jamFrom = 720 + 10;
  rig.valve[3].jamTo = 100000;
  run(rig, CMD_A_OPEN_END, 3, 0);
  CHECK_FALSE(diag(3).lastEarly);
  // no travel, a full stroke expected, not a single count turned
  place(rig, 4, 0);
  myvalvemots[4].scaler = 0;
  rig.valve[4].jamFrom = 0;
  rig.valve[4].jamTo = 100000;
  run(rig, CMD_A_OPEN_END, 4, 0);
  CHECK(last(4).countedCounts == 0);
  CHECK_FALSE(diag(4).lastEarly);
}

TEST_CASE("move to the end stop: a start position above 100 % expects no travel") {
  sim::Rig rig;
  startValves(rig);
  for (unsigned v = 0; v < 2; v++) {
    place(rig, v, 20);
    rig.valve[v].jamFrom = 720 + 100;
    rig.valve[v].jamTo = 100000;
  }
  myvalvemots[0].actual_position = 150;
  run(rig, CMD_A_OPEN_END, 0, 0);
  CHECK(last(0).stopReason == (uint8_t) vdm::StopReason::EndStop);
  CHECK_FALSE(diag(0).lastEarly);
  myvalvemots[1].actual_position = 101;
  run(rig, CMD_A_OPEN_END, 1, 0);
  CHECK_FALSE(diag(1).lastEarly);
}

// ------------------------------------------------------------------ requested counts, refused moves

TEST_CASE("normal move: the requested counts stay below kRunToEndStop, a refused move records nothing turned") {
  sim::Rig rig;
  startValves(rig);
  place(rig, 0, 10);
  run(rig, CMD_A_OPEN, 0, 5);
  REQUIRE(last(0).countedCounts > 0);
  REQUIRE(last(0).durationMs > 0);
  const struct { unsigned scaler; uint8_t change; } cases[] = {{771, 85}, {1000, 100}, {655, 100}};
  const uint16_t expected[] = {65534, 65534, 65500};
  for (unsigned i = 0; i < 3; i++) {
    CAPTURE(i);
    myvalvemots[0].scaler = cases[i].scaler;
    REQUIRE(appsetaction(CMD_A_OPEN, 0, cases[i].change) == 0);
    rig.runMs(20);
    REQUIRE(+valvestate == A_OPEN1);
    CHECK(appstop(0) == 0);
    REQUIRE(rig.runUntil(idle, 1000));
    const vdm::MoveResult r = last(0);
    CHECK(r.requestedCounts == expected[i]);
    CHECK(r.stopReason == (uint8_t) vdm::StopReason::Aborted);
    CHECK(r.countedCounts == 0);
    CHECK(r.peakCurrent == 0);
    CHECK(r.durationMs == 0);
  }
}

// ------------------------------------------------------------------ calibration strokes

TEST_CASE("calibration: a full stroke below half of the learned travel is early") {
  sim::Rig rig;
  startValves(rig);
  place(rig, 0, 0);
  noOfMinCounts = 100;
  rig.valve[0].stroke = 1790;
  rig.valve[0].position = 1790;
  rig.valve[0].pulsesPerMs = 1.0f;
  myvalvemots[0].target_position = 0;
  learn(rig, 0);
  CHECK(+myvalvemots[0].calibSeq == 1);
  CHECK(+myvalvemots[0].status == VLV_STATE_IDLE);
  // the last stroke (closing, 1790 counts against 3600 learned) is early
  CHECK(last(0).stopReason == (uint8_t) vdm::StopReason::EarlyEndStop);
  CHECK(diag(0).lastEarly);
  CHECK_FALSE(diag(0).lastCalFailed);
  CHECK_FALSE(diag(0).earlyWarn);
}

TEST_CASE("calibration: the strokes close, open and close; the scaler is the opening count / 100") {
  sim::Rig rig;
  startValves(rig);
  rig.valve[0].stroke = 9920;
  rig.valve[0].pulsesPerMs = 2.0f;
  myvalvemots[0].target_position = 0;
  REQUIRE(appsetaction(CMD_A_LEARN, 0, 0) == 0);
  REQUIRE(rig.runUntil([] { return valvestate == A_LEARN3; }, 60000));
  rig.runMs(1000);
  CHECK(+myvalvemots[0].status == VLV_STATE_OPENING);
  REQUIRE(rig.runUntil([] { return valvestate == A_LEARN4; }, 60000));
  rig.runMs(1000);
  CHECK(+myvalvemots[0].status == VLV_STATE_CLOSING);
  REQUIRE(rig.runUntil([] { return myvalvemots[0].calibActive == 0 && valvestate == A_IDLE; }, 60000));
  CHECK(+myvalvemots[0].opening_count >= 9900u);
  CHECK(+myvalvemots[0].scaler == 99);
  CHECK(+myvalvemots[0].actual_position == 0);
  CHECK_FALSE(diag(0).lastCalFailed);
}

TEST_CASE("calibration: an enforced inrush trip of the first stroke is not early, the next calibration runs") {
  sim::Rig rig;
  startValves(rig);
  protect_enforce = true;
  place(rig, 0, 50);
  rig.valve[0].inrushPeak_dmA = 2800;
  rig.valve[0].inrushMs = 60;
  learn(rig, 0);
  CHECK(+myvalvemots[0].status == VLV_STATE_FAILED);
  CHECK(last(0).stopReason == (uint8_t) vdm::StopReason::SafetyOvercurrent);
  CHECK_FALSE(diag(0).lastEarly);
  rig.valve[0].inrushPeak_dmA = 0;
  rig.valve[0].pulsesPerMs = 1.0f;
  myvalvemots[0].target_position = 0;
  learn(rig, 0);
  CHECK(+myvalvemots[0].status == VLV_STATE_IDLE);
  CHECK(+myvalvemots[0].calibSeq == 1);
}

TEST_CASE("calibration: early warning and failed flag over a failed and a successful calibration") {
  sim::Rig rig;
  startValves(rig);
  place(rig, 0, 20);
  rig.valve[0].pulsesPerMs = 1.0f;
  rig.valve[0].jamFrom = 900;
  rig.valve[0].jamTo = 3600;
  run(rig, CMD_A_OPEN, 0, 40);
  REQUIRE(diag(0).earlyWarn);
  // strokes too short: BLOCKS, the warning stays
  rig.valve[0].jamFrom = -1;
  rig.valve[0].stroke = 50;
  rig.valve[0].position = 20;
  learn(rig, 0);
  CHECK(+myvalvemots[0].status == VLV_STATE_BLOCKS);
  CHECK(diag(0).lastCalFailed);
  CHECK(diag(0).earlyWarn);
  // success: both cleared as soon as the pass is accepted, before the move to the target
  rig.valve[0].stroke = 3600;
  myvalvemots[0].target_position = 50;
  REQUIRE(appsetaction(CMD_A_LEARN, 0, 0) == 0);
  REQUIRE(rig.runUntil([] { return valvestate == A_SET2; }, 60000));
  CHECK_FALSE(diag(0).lastCalFailed);
  CHECK_FALSE(diag(0).earlyWarn);
  REQUIRE(rig.runUntil([] { return myvalvemots[0].calibActive == 0 && valvestate == A_IDLE; }, 60000));
  CHECK_FALSE(diag(0).lastCalFailed);
  CHECK_FALSE(diag(0).earlyWarn);
  CHECK(+myvalvemots[0].actual_position == 50);
}

TEST_CASE("calibration: the retries restart at 0 with every calibration, the valve count saturates") {
  sim::Rig rig;
  startValves(rig);
  rig.valve[0].pulsesPerMs = 2.0f;
  rig.valve[0].stroke = 50;
  rig.valve[0].position = 20;
  calibRetries = 5;
  myvalvemots[0].calibRetries = 5;
  maxCalibRetries = 1;
  learn(rig, 0);
  CHECK(+myvalvemots[0].status == VLV_STATE_BLOCKS);
  CHECK(+myvalvemots[0].calibRetries == 2);
  CHECK(+calibRetries == 2);
  // a successful calibration after failed ones
  calibRetries = 5;
  myvalvemots[0].calibRetries = 5;
  rig.valve[0].stroke = 3600;
  myvalvemots[0].target_position = 0;
  learn(rig, 0);
  CHECK(+myvalvemots[0].status == VLV_STATE_IDLE);
  CHECK(+myvalvemots[0].calibRetries == 0);
}

TEST_CASE("calibration: the retry count of the valve saturates at 255") {
  sim::Rig rig;
  startValves(rig);
  rig.valve[0].pulsesPerMs = 2.0f;
  rig.valve[0].stroke = 30;
  rig.valve[0].position = 10;
  maxCalibRetries = 255;
  learn(rig, 0);
  CHECK(+myvalvemots[0].status == VLV_STATE_BLOCKS);
  CHECK(+myvalvemots[0].calibRetries == 255);
  CHECK(+calibRetries == 0);   // 256 failed passes
}

TEST_CASE("calibration: contact lost marks the calibration failed") {
  sim::Rig rig;
  startValves(rig);
  rig.valve[3].connected = false;
  learn(rig, 3);
  CHECK(+myvalvemots[3].status == VLV_STATE_OPENCIR);
  CHECK(diag(3).lastCalFailed);
}

TEST_CASE("calibration: a stroke without an end stop times out and fails the valve") {
  sim::Rig rig;
  startValves(rig);
  rig.valve[0].stallCurrent_dmA = 300;   // below the end-stop bound: no trip
  rig.valve[0].jamFrom = 0;
  rig.valve[0].jamTo = 100000;
  rig.valve[0].position = 1000;
  learn(rig, 0);
  CHECK(+myvalvemots[0].status == VLV_STATE_FAILED);
  CHECK(+myvalvemots[0].faultReason == (uint8_t) vdm::ValveFault::StrokeTimeout);
  CHECK(last(0).stopReason == (uint8_t) vdm::StopReason::Timeout);
  CHECK(diag(0).lastCalFailed);
}

TEST_CASE("calibration: target 1 % is set after the calibration, target 0 needs no move") {
  sim::Rig rig;
  startValves(rig);
  rig.valve[3].pulsesPerMs = 1.0f;
  myvalvemots[3].target_position = 1;
  learn(rig, 3);
  CHECK(+myvalvemots[3].actual_position == 1);
  CHECK(last(3).stopReason == (uint8_t) vdm::StopReason::Target);
  CHECK_FALSE(diag(3).lastCalFailed);
  // target 0: valve 0 is not touched by the end of the calibration of valve 3
  myvalvemots[0].status = VLV_STATE_IDLE;
  myvalvemots[0].connected = 0;
  myvalvemots[3].target_position = 0;
  learn(rig, 3);
  CHECK(+myvalvemots[3].actual_position == 0);
  CHECK_FALSE(diag(3).lastCalFailed);
  CHECK(+myvalvemots[0].connected == 0);
}

TEST_CASE("calibration: blocked valve 3 leaves valve 0 alone") {
  sim::Rig rig;
  startValves(rig);
  rig.valve[3].pulsesPerMs = 1.0f;
  rig.valve[3].stroke = 50;
  rig.valve[3].position = 20;
  myvalvemots[0].status = VLV_STATE_IDLE;
  myvalvemots[0].connected = 0;
  learn(rig, 3);
  CHECK(+myvalvemots[3].status == VLV_STATE_BLOCKS);
  CHECK(+myvalvemots[0].connected == 0);
}

TEST_CASE("calibration: sstop while the move to the target waits for its start") {
  sim::Rig rig;
  startValves(rig);
  rig.valve[0].pulsesPerMs = 1.0f;
  myvalvemots[0].target_position = 50;
  REQUIRE(appsetaction(CMD_A_LEARN, 0, 0) == 0);
  REQUIRE(rig.runUntil([] { return valvestate == A_SET1; }, 60000));
  const uint32_t set1 = rig.ms;
  rig.runMs(500);
  CHECK(+valvestate == A_SET1);   // waits for the PSU (1 s)
  CHECK(appstop(0) == 0);
  REQUIRE(rig.runUntil(idle, 1000));
  CHECK(rig.ms - set1 < 1000);
  CHECK(+myvalvemots[0].calibActive == 0);
  CHECK_FALSE(diag(0).lastCalFailed);
  CHECK(last(0).stopReason == (uint8_t) vdm::StopReason::Aborted);
}

TEST_CASE("calibration: sstop while a stroke waits for its start leaves the motor ready for the next move") {
  sim::Rig rig;
  startValves(rig);
  place(rig, 0, 10);
  rig.valve[0].pulsesPerMs = 0.1f;
  // a move ended by sstop: its stop cause is Aborted
  REQUIRE(appsetaction(CMD_A_OPEN, 0, 80) == 0);
  rig.runMs(3000);
  CHECK(appstop(0) == 0);
  REQUIRE(rig.runUntil(idle, 5000));
  REQUIRE(last(0).stopReason == (uint8_t) vdm::StopReason::Aborted);
  const uint32_t enables = rig.valve[0].enables;
  // the calibration waits in A_LEARN2 for the first stroke
  REQUIRE(appsetaction(CMD_A_LEARN, 0, 0) == 0);
  REQUIRE(rig.runUntil([] { return valvestate == A_LEARN2; }, 1000));
  rig.runMs(100);
  CHECK(appstop(0) == 0);
  REQUIRE(rig.runUntil(idle, 5000));
  CHECK(rig.valve[0].enables == enables + 1);   // the stroke started and was stopped
  CHECK(+myvalvemots[0].needsReference == 1);
  // the next move runs
  place(rig, 0, 10);
  run(rig, CMD_A_OPEN, 0, 5);
  CHECK(last(0).stopReason == (uint8_t) vdm::StopReason::Target);
  CHECK(+myvalvemots[0].actual_position == 15);
}

TEST_CASE("calibration: the temperature gap ends after exactly 3 s") {
  sim::Rig rig;
  startValves(rig);
  rig.valve[0].pulsesPerMs = 1.0f;
  temp_refresh_request = true;
  REQUIRE(appsetaction(CMD_A_LEARN, 0, 0) == 0);
  REQUIRE(rig.runUntil([] { return valvestate == A_LEARN3; }, 60000));
  const sim::Rig::Step* gap = stateStep(rig, A_GAP);
  const sim::Rig::Step* next = stateStep(rig, A_LEARN3);
  REQUIRE(gap != nullptr);
  REQUIRE(next != nullptr);
  CHECK(next->ms - gap->ms == 3000);
}

// ------------------------------------------------------------------ service moves

TEST_CASE("service move: exactly the requested counts, the position from the scaler") {
  sim::Rig rig;
  startValves(rig);
  place(rig, 0, 10);
  const int32_t start = rig.valve[0].position;
  REQUIRE(appsetservice(0, vdm::kDirOpen, 500, 40) == 0);
  rig.runMs(1500);
  CHECK(+myvalvemots[0].status == VLV_STATE_OPENING);
  REQUIRE(rig.runUntil(idle, 20000));
  const vdm::MoveResult r = last(0);
  CHECK(r.stopReason == (uint8_t) vdm::StopReason::Target);
  CHECK(r.countedCounts == 500);
  CHECK(rig.valve[0].position - start == 500);
  CHECK(r.durationMs >= 2500);
  CHECK(r.durationMs < 3000);
  CHECK(+myvalvemots[0].actual_position == 10 + 500 / 36);
  CHECK(+myvalvemots[0].status == VLV_STATE_IDLE);
}

TEST_CASE("service move: an end stop is never early and leaves the valve idle") {
  sim::Rig rig;
  startValves(rig);
  place(rig, 0, 10);
  service(rig, 0, vdm::kDirClose, 1000, 40);
  CHECK(last(0).stopReason == (uint8_t) vdm::StopReason::EndStop);
  CHECK_FALSE(diag(0).lastEarly);
  CHECK(+myvalvemots[0].status == VLV_STATE_IDLE);
  CHECK(+myvalvemots[0].actual_position == 0);
}

TEST_CASE("service move: the end-stop bound is maxmA x 10") {
  sim::Rig rig;
  startValves(rig);
  // 48 mA filtered at the obstacle, bound 45 mA: stops at the obstacle
  place(rig, 0, 10);
  rig.valve[0].stallCurrent_dmA = 530;
  rig.valve[0].jamFrom = 400;
  rig.valve[0].jamTo = 100000;
  service(rig, 0, vdm::kDirOpen, 1000, 45);
  CHECK(last(0).stopReason == (uint8_t) vdm::StopReason::EndStop);
  // 53 mA filtered at the obstacle, bound 55 mA: runs into the timeout
  place(rig, 1, 10);
  rig.valve[1].stallCurrent_dmA = 580;
  rig.valve[1].jamFrom = 400;
  rig.valve[1].jamTo = 100000;
  service(rig, 1, vdm::kDirOpen, 1000, 55);
  CHECK(last(1).stopReason == (uint8_t) vdm::StopReason::Timeout);
  CHECK(last(1).durationMs == 120270);
  CHECK(+myvalvemots[1].status == VLV_STATE_FAILED);
}

TEST_CASE("service move: the position with scaler 1 is clamped to 100 % of travel") {
  sim::Rig rig;
  startValves(rig);
  place(rig, 0, 10);
  myvalvemots[0].scaler = 1;
  service(rig, 0, vdm::kDirOpen, 50, 40);
  CHECK(+myvalvemots[0].actual_position == 60);
  const struct { uint16_t counts; uint8_t pos; } cases[] = {{101, 50}, {150, 50}, {99, 51}};
  for (unsigned i = 0; i < 3; i++) {
    CAPTURE(i);
    const unsigned v = 1 + i;
    place(rig, v, 10);
    rig.valve[v].position = 3000;
    myvalvemots[v].scaler = 1;
    myvalvemots[v].actual_position = 150;
    service(rig, v, vdm::kDirClose, cases[i].counts, 40);
    CHECK(last(v).countedCounts == cases[i].counts);
    CHECK(+myvalvemots[v].actual_position == cases[i].pos);
  }
}

TEST_CASE("appsetservice: argument limits, busy") {
  sim::Rig rig;
  startValves(rig);
  auto take = [](unsigned v, uint8_t dir, uint16_t counts, uint8_t maxmA) {
    const int16_t r = appsetservice(v, dir, counts, maxmA);
    if (r == 0) appstop(255);
    return r;
  };
  CHECK(take(12, vdm::kDirOpen, 100, 30) == -1);
  CHECK(take(11, vdm::kDirOpen, 100, 30) == 0);
  CHECK(take(0, 2, 100, 30) == -1);
  CHECK(take(0, vdm::kDirClose, 100, 30) == 0);
  CHECK(take(0, vdm::kDirOpen, 0, 30) == -1);
  CHECK(take(0, vdm::kDirOpen, SVMOV_COUNTS_MIN, 30) == 0);
  CHECK(take(0, vdm::kDirOpen, SVMOV_COUNTS_MAX, 30) == 0);
  CHECK(take(0, vdm::kDirOpen, SVMOV_COUNTS_MAX + 1, 30) == -1);
  CHECK(take(0, vdm::kDirOpen, 100, SVMOV_MAXMA_MIN - 1) == -1);
  CHECK(take(0, vdm::kDirOpen, 100, SVMOV_MAXMA_MIN) == 0);
  CHECK(take(0, vdm::kDirOpen, 100, SVMOV_MAXMA_MAX) == 0);
  CHECK(take(0, vdm::kDirOpen, 100, SVMOV_MAXMA_MAX + 1) == -1);
  CHECK(valve_idle());
  // a command is pending: busy
  REQUIRE(appsetservice(0, vdm::kDirOpen, 100, 30) == 0);
  CHECK(appsetservice(1, vdm::kDirOpen, 100, 30) == -2);
  // taken, the machine is busy
  rig.runMs(20);
  REQUIRE(+valvestate == A_SVC1);
  CHECK(appsetservice(1, vdm::kDirOpen, 100, 30) == -2);
}

// ------------------------------------------------------------------ snapshots and profiles

TEST_CASE("valve_get_snapshot and valve_get_profile: valve 12 and above give empty results") {
  sim::Rig rig;
  startValves(rig);
  place(rig, 0, 20);
  run(rig, CMD_A_OPEN, 0, 10);
  vdm::ProfileRecorder p;
  valve_get_profile(0, p);
  CHECK(p.size() > 0);
  valve_get_profile(12, p);
  CHECK(p.size() == 0);
  valve_snapshot s;
  valve_get_snapshot(0, s);
  CHECK(s.diag.last.countedCounts > 0);
  valve_get_snapshot(12, s);
  CHECK(s.diag.last.countedCounts == 0);
  CHECK(s.actual_position == 0);
}

TEST_CASE("profile: a move that ends at an end stop ends with the trip current") {
  sim::Rig rig;
  startValves(rig);
  place(rig, 0, 60);
  run(rig, CMD_A_OPEN_END, 0, 0);
  REQUIRE(last(0).stopReason == (uint8_t) vdm::StopReason::EndStop);
  vdm::ProfileRecorder p;
  valve_get_profile(0, p);
  REQUIRE(p.size() > 1);
  CHECK(p.at(p.size() - 1).current > 300);
}

// ------------------------------------------------------------------ PSU and temperature lock timing

TEST_CASE("idle: temperature measurement unlocked after 50 idle cycles, PSU off after 1500") {
  sim::Rig rig;
  startValves(rig);
  place(rig, 0, 20);
  run(rig, CMD_A_OPEN, 0, 5);
  stub::calls.clear();
  const uint32_t start = rig.ms;
  REQUIRE(rig.runUntil([] { return !stub::callsOf("temp_command").empty(); }, 2000));
  CHECK(rig.ms - start == 510);
  CHECK(stub::callsOf("temp_command").front() == "temp_command(3)");
  CHECK(msToPsuOff(rig) == 15010 - 510);
  // the next 15 s
  const size_t writes = fake::eventsOf(fake::Ev::Write, POWER_ENA).size();
  rig.runMs(15000);
  CHECK(fake::eventsOf(fake::Ev::Write, POWER_ENA).size() == writes);
  rig.runMs(20);
  CHECK(fake::eventsOf(fake::Ev::Write, POWER_ENA).size() > writes);
}

TEST_CASE("idle: the PSU goes off 15 s after a presence test, a service move and a calibration") {
  sim::Rig rig;
  startValves(rig);
  run(rig, CMD_A_TEST, 2, 0);
  CHECK(msToPsuOff(rig) == 15010);
  place(rig, 1, 10);
  service(rig, 1, vdm::kDirOpen, 100, 40);
  CHECK(msToPsuOff(rig) == 15010);
  rig.valve[0].pulsesPerMs = 1.0f;
  myvalvemots[0].target_position = 0;
  learn(rig, 0);
  CHECK(msToPsuOff(rig) == 15010);
  myvalvemots[0].target_position = 30;
  learn(rig, 0);
  CHECK(msToPsuOff(rig) == 15010);
  // a retried calibration
  rig.valve[0].stroke = 50;
  rig.valve[0].position = 20;
  maxCalibRetries = 1;
  learn(rig, 0);
  REQUIRE(+myvalvemots[0].status == VLV_STATE_BLOCKS);
  CHECK(msToPsuOff(rig) == 15010);
}

// ------------------------------------------------------------------ valve_loop without the sim

TEST_CASE("valve_loop: A_INIT until the motor machine is idle, no valve touched") {
  glue::begin();
  analogReadResolution(12);
  valve_setup();
  for (unsigned v = 0; v < 2; v++) {
    myvalvemots[v].status = VLV_STATE_IDLE;
    myvalvemots[v].connected = 0;
  }
  valve_loop();
  CHECK(+valvestate == A_INIT);
  CHECK(+myvalvemots[1].connected == 0);
  myvalvemots[0].connected = 0;
  valve_loop();
  CHECK(+valvestate == A_IDLE);
  CHECK(+myvalvemots[0].connected == 0);
  CHECK(+myvalvemots[1].connected == 0);
  valve_loop();
  CHECK(+myvalvemots[0].connected == 1);
}

TEST_CASE("valve_setup: no valve starts failed, calibrated or with a pending reference") {
  glue::begin();
  for (unsigned v = 0; v < ACTUATOR_COUNT; v++) {
    myvalvemots[v].calibFailed = 1;
    myvalvemots[v].calibrated = 1;
    myvalvemots[v].needsReference = 1;
    myvalvemots[v].earlyLearnDue = 1;
  }
  valve_setup();
  for (unsigned v = 0; v < ACTUATOR_COUNT; v++) {
    CAPTURE(v);
    CHECK(+myvalvemots[v].calibFailed == 0);
    CHECK(+myvalvemots[v].calibrated == 0);
    CHECK(+myvalvemots[v].needsReference == 0);
    CHECK(+myvalvemots[v].earlyLearnDue == 0);
  }
}

TEST_CASE("normal move: the end-stop bound is never escalated") {
  sim::Rig rig;
  startValves(rig);
  motor_set_escalation(vdm::EscalationConfig{1, 25, 50});
  // 39 mA filtered at the end stop: above the 34 mA bound, below its first escalation (42.5 mA)
  place(rig, 0, 60);
  rig.valve[0].stallCurrent_dmA = 440;
  run(rig, CMD_A_OPEN_END, 0, 0);
  CHECK(last(0).stopReason == (uint8_t) vdm::StopReason::EndStop);
}

TEST_CASE("valve_loop: counts its runs, idle is never a stall") {
  glue::begin();
  analogReadResolution(12);
  valve_setup();
  for (unsigned i = 0; i < 30100; i++) valve_loop();
  CHECK(valve_loop_ticks == 30100u);
  CHECK(+valvestate == A_IDLE);
  CHECK_FALSE(valve_loop_stalled);
}

TEST_CASE("open circuit of valve 11: not connected") {
  sim::Rig rig;
  startValves(rig);
  rig.valve[11].connected = false;
  place(rig, 11, 30);
  run(rig, CMD_A_OPEN, 11, 10);
  CHECK(+myvalvemots[11].status == VLV_STATE_OPENCIR);
  CHECK(+myvalvemots[11].connected == 0);
}

TEST_CASE("undercurrent: exactly the threshold is a turning motor, in both directions") {
  sim::Rig rig;
  startValves(rig);
  place(rig, 0, 20);
  rig.valve[0].runCurrent_dmA = 20;
  run(rig, CMD_A_OPEN, 0, 10);
  CHECK(+myvalvemots[0].status == VLV_STATE_IDLE);
  CHECK(+myvalvemots[0].actual_position == 30);
  run(rig, CMD_A_CLOSE, 0, 10);
  CHECK(+myvalvemots[0].status == VLV_STATE_IDLE);
  CHECK(+myvalvemots[0].actual_position == 20);
}

TEST_CASE("presence test: 2 s for the PSU, then the test") {
  sim::Rig rig;
  startValves(rig);
  run(rig, CMD_A_TEST, 2, 0);
  const sim::Rig::Step* test = stateStep(rig, A_TEST);
  const sim::Rig::Step* back = stateStep(rig, A_IDLE);
  REQUIRE(test != nullptr);
  REQUIRE(back != nullptr);
  CHECK(back->ms - test->ms == 2250);
  CHECK(+myvalvemots[2].status == VLV_STATE_PRESENT);
}

// ------------------------------------------------------------------ motor state machine

TEST_CASE("motorcycle: valve numbers outside 0..11 are refused") {
  glue::begin();
  CHECK(motorcycle(0, 'n') == 1);   // M_INIT
  CHECK(motorcycle(12, 'o') == kResIdle);
  CHECK(motorcycle(-1, 'o') == kResIdle);
  CHECK(motorcycle(11, 'o') == kResOpens);
}

TEST_CASE("motorcycle: a stop request of the EXTI handler in idle is dropped") {
  glue::begin();
  motorcycle(0, 'n');
  isr_stop_request = 1;
  CHECK(motorcycle(0, 'n') == kResIdle);
  CHECK(+isr_stop_request == 0);
  CHECK(motorcycle(0, 'n') == kResIdle);
}

TEST_CASE("motorcycle: the motor enable times out after TIMEOUT_TURNON cycles without TimerHandler0") {
  glue::begin();
  motorcycle(0, 'n');
  REQUIRE(motorcycle(0, 'o') == kResOpens);
  unsigned calls = 0;
  byte r = 0;
  do {
    r = motorcycle(0, 'n');
    calls++;
  } while (r != kResError && calls < 100);
  CHECK(r == kResError);
  CHECK(calls == 1 + 10 + 1 + 11);   // M_OPEN, settle, M_START, 11 M_TURNON
  CHECK(motorcycle(0, 'n') == kResIdle);
}

TEST_CASE("motorcycle: halt and the EXTI stop cancel the soft start") {
  sim::Rig rig;
  startValves(rig);
  place(rig, 0, 20);
  run(rig, CMD_A_OPEN, 0, 5);
  CHECK(+isr_turning == 0);
  CHECK(+isr_timer_go == 0);
  CHECK(+isr_timer_fin == 0);
  isr_turning = 1;
  callback_motorstop();
  CHECK(+isr_turning == 0);
  CHECK(+isr_stop_request == 1);
}

// ------------------------------------------------------------------ TimerHandler0

TEST_CASE("TimerHandler0: the test current filter, idle current 0") {
  glue::begin();
  analogReadResolution(12);
  fake::board.analogValue[ANINREFHALF] = 2048;
  const struct { uint32_t adc; int old; } cases[] = {{2048 + 2047, 10000}, {2048 + 1000, 7777}, {2048 - 1500, 12345}, {2048 + 7, 3}};
  for (const auto& c : cases) {
    CAPTURE(c.adc);
    fake::board.analogValue[ANINCURRENT] = c.adc;
    analog_current_old = c.old;
    current_mA = 5;
    TimerHandler0();
    const int value = (int) (((int32_t) c.adc - 2048) * ANINCURRENTGAIN / 100);
    const int expected = (int) (((int32_t) c.old * 9800 + (int32_t) value * 200) / 10000);
    CHECK(analog_current == expected);
    CHECK(analog_current_old == expected);
    CHECK(current_mA == 0);
  }
}

TEST_CASE("TimerHandler0: the motor is enabled only while turning, requested and not yet enabled") {
  glue::begin();
  analogReadResolution(12);
  fake::board.analogValue[ANINREFHALF] = 2048;
  fake::board.analogValue[ANINCURRENT] = 2048;
  isr_valvenr = 4;
  const struct { byte turning, go, fin; bool on; } cases[] = {
      {1, 1, 0, true}, {0, 1, 0, false}, {1, 0, 0, false}, {1, 1, 1, false}};
  for (const auto& c : cases) {
    CAPTURE(+c.turning);
    CAPTURE(+c.go);
    CAPTURE(+c.fin);
    fake::board.out[CTRL_ENA2] = 0;
    isr_turning = c.turning;
    isr_timer_go = c.go;
    isr_timer_fin = c.fin;
    TimerHandler0();
    CHECK((fake::board.out[CTRL_ENA2] == HIGH) == c.on);
    CHECK(+isr_timer_fin == (c.on ? 1 : c.fin));
  }
}

// ------------------------------------------------------------------ mean current of the strokes

// One sample of the turning current every 51 cycles from the 51st on, kMinMeanSamples (4) of them
// make a stroke mean valid. The opening stroke (15 mA) is just long enough for the 4th sample,
// the closing stroke (25 mA) ends just before its 4th: only the opening mean is learned. One
// sample more or less on either stroke changes the result (20 mA: both or none valid).
TEST_CASE("calibration: stroke means need 4 samples, one every 51 cycles") {
  sim::Rig rig;
  startValves(rig);
  noOfMinCounts = 100;
  rig.valve[0].stroke = 2023;
  rig.valve[0].position = 0;
  rig.valve[0].pulsesPerMs = 1.0f;
  rig.valve[0].runCurrent_dmA = 150;
  myvalvemots[0].target_position = 0;
  REQUIRE(+myvalvemots[0].meancurrent == 20);
  REQUIRE(appsetaction(CMD_A_LEARN, 0, 0) == 0);
  REQUIRE(rig.runUntil([] { return valvestate == A_LEARN4; }, 200000));
  rig.valve[0].pulsesPerMs = 0.9932f;
  rig.valve[0].runCurrent_dmA = 300;
  REQUIRE(rig.runUntil([] { return myvalvemots[0].calibActive == 0 && valvestate == A_IDLE; }, 200000));
  CHECK(+myvalvemots[0].calibSeq == 1);
  CHECK(+myvalvemots[0].meancurrent == 15);
}
