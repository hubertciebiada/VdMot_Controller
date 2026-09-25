// Smoke tests of src/motor.cpp with the valve sim (glue_motor, and glue_motor_c1 with the C1 MUX
// wiring): safe pins, setup, a full calibration, a partial move, an open circuit, the presence test.
#include "glue_test.h"
#include "motor.h"
#include "stub_app.h"
#include "valve_sim.h"

void TimerHandler0();
extern STM32Timer ITimer0;

using fake::Ev;

namespace {

// what setup_system() does for the valve machine: 12-bit ADC, safe pins, valve_setup(); then the
// first two valve_loop() runs (motor M_INIT, M_IDLE) leave A_INIT
void startValves(sim::Rig& rig) {
  glue::begin();
  analogReadResolution(12);
  valve_pins_safe();
  valve_setup();
  rig.install();
  rig.runMs(20);
  REQUIRE(+valvestate == A_IDLE);
  rig.transitions.clear();
}

bool idle() { return valve_idle(); }

// valvestate changes recorded by the sim, in order ("2 5 6")
std::string states(const sim::Rig& rig) {
  std::string out;
  for (const sim::Rig::Step& s : rig.transitions) {
    if (s.valve != 255) continue;
    if (!out.empty()) out += ' ';
    out += std::to_string(s.valvestate);
  }
  return out;
}

const sim::Rig::Step* statusChange(const sim::Rig& rig, uint8_t valve, uint8_t status) {
  for (const sim::Rig::Step& s : rig.transitions) {
    if (s.valve == valve && s.status == status) return &s;
  }
  return nullptr;
}

fake::Event ev(Ev kind, uint32_t pin, uint32_t value) { return {0, kind, pin, value, 0}; }

}  // namespace

TEST_CASE("valve_pins_safe: the PSU latch is preset off before the pin becomes an output, enables low") {
  glue::begin();
  valve_pins_safe();
  const std::vector<fake::Event> expected = {
      ev(Ev::Write, POWER_ENA, HIGH), ev(Ev::Mode, POWER_ENA, OUTPUT_OPEN_DRAIN), ev(Ev::Write, POWER_ENA, HIGH),
      ev(Ev::Mode, CTRL_ENA0, OUTPUT), ev(Ev::Mode, CTRL_ENA1, OUTPUT), ev(Ev::Mode, CTRL_ENA2, OUTPUT),
      ev(Ev::Mode, CTRL_ENA3, OUTPUT), ev(Ev::Mode, CTRL_ENA4, OUTPUT), ev(Ev::Mode, CTRL_ENA5, OUTPUT),
      ev(Ev::Write, CTRL_ENA0, LOW), ev(Ev::Write, CTRL_ENA1, LOW), ev(Ev::Write, CTRL_ENA2, LOW),
      ev(Ev::Write, CTRL_ENA3, LOW), ev(Ev::Write, CTRL_ENA4, LOW), ev(Ev::Write, CTRL_ENA5, LOW)};
  CHECK(fake::board.events == expected);
  CHECK(fake::board.portClocks == 1u << STM_PORT(POWER_ENA));
}

TEST_CASE("valve_setup: pins, start values of all valves and the 1 ms current timer") {
  glue::begin();
  startOnPower = 30;
  CHECK(valve_setup() == 0);
  CHECK(fake::board.mode[CTRL_MUX] == OUTPUT);
  CHECK(fake::board.mode[CTRL_DIRECTION] == OUTPUT);
  CHECK(fake::board.mode[CTRL_ENA5] == OUTPUT);
  CHECK(fake::board.mode[REVINPIN] == INPUT);
  for (unsigned v = 0; v < ACTUATOR_COUNT; v++) {
    CAPTURE(v);
    CHECK(+myvalvemots[v].actual_position == 30);
    CHECK(+myvalvemots[v].target_position == 30);
    CHECK(+myvalvemots[v].meancurrent == vdm::kMeanCurrentDefault_mA);
    CHECK(+myvalvemots[v].scaler == 89);
    CHECK(+myvalvemots[v].calibRetries == 0);
    CHECK(+myvalvemots[v].calibActive == 0);
  }
  CHECK(+valvestate == A_INIT);
  CHECK(ITimer0.intervalUs == 1000);
  CHECK(ITimer0.callback == TimerHandler0);
}

TEST_CASE("calibration: three strokes learn the stroke of the valve, then it goes back to its target") {
  sim::Rig rig;
  startValves(rig);
  rig.valve[0].pulsesPerMs = 1.0f;
  REQUIRE(appsetaction(CMD_A_LEARN, 0, 0) == 0);
  rig.runMs(10);
  REQUIRE(+myvalvemots[0].calibActive == 1);
  REQUIRE(rig.runUntil([] { return myvalvemots[0].calibActive == 0 && valvestate == A_IDLE; }, 30000));
  // A_LEARN1 .. A_LEARN4, A_SET, A_SET1, A_SET2, A_IDLE
  CHECK(states(rig) == "5 6 7 8 9 10 11 1");
  CHECK(+myvalvemots[0].status == VLV_STATE_IDLE);
  const unsigned opening = myvalvemots[0].opening_count;
  const unsigned closing = myvalvemots[0].closing_count;
  CHECK(opening >= 3598u);
  CHECK(opening <= 3602u);
  CHECK(closing >= 3598u);
  CHECK(closing <= 3602u);
  CHECK(+myvalvemots[0].deadzone_count == static_cast<int>(closing) - static_cast<int>(opening));
  CHECK(+myvalvemots[0].scaler == opening / 100);
  CHECK(+myvalvemots[0].actual_position == 50);
  CHECK(+myvalvemots[0].calibration == 0);
  CHECK(+myvalvemots[0].calibState == calibIdle);
  // back at 50 %: 50 x scaler counts from the closed end stop, plus the pulse that stops the motor
  CHECK(rig.valve[0].position == static_cast<int32_t>(50 * myvalvemots[0].scaler + 1));
  CHECK(rig.conflicts == 0);
}

TEST_CASE("partial move: opening by change % stops on pulse scaler x change + 1") {
  sim::Rig rig;
  startValves(rig);
  myvalvemots[0].scaler = 36;
  myvalvemots[0].status = VLV_STATE_IDLE;
  const int32_t start = rig.valve[0].position;
  REQUIRE(appsetaction(CMD_A_OPEN, 0, 10) == 0);
  rig.runMs(10);
  REQUIRE(rig.runUntil(idle, 10000));
  CHECK(rig.valve[0].position - start == 36 * 10 + 1);
  CHECK(+myvalvemots[0].actual_position == 60);
  CHECK(+myvalvemots[0].status == VLV_STATE_IDLE);
  CHECK(+myvalves[0].movements == 1);
}

TEST_CASE("open circuit: OPENCIR on the tick after the undercurrent counter exceeds TIMEOUT_UNDERCURRENT") {
  sim::Rig rig;
  startValves(rig);
  rig.valve[1].connected = false;
  myvalvemots[1].status = VLV_STATE_IDLE;
  myvalvemots[1].target_position = 60;
  REQUIRE(appsetaction(CMD_A_OPEN, 1, 10) == 0);
  REQUIRE(rig.runUntil(idle, 10000));
  const sim::Rig::Step* opening = statusChange(rig, 1, VLV_STATE_OPENING);
  const sim::Rig::Step* opencir = statusChange(rig, 1, VLV_STATE_OPENCIR);
  REQUIRE(opening != nullptr);
  REQUIRE(opencir != nullptr);
  // valve_loop ticks (10 ms) after the start of the move: M_OPEN 1, relay settle 10, M_START 1,
  // M_TURNON 2 (TimerHandler0 enables the motor in between), debounce 3, undercurrent 201 (> 200),
  // M_UNDERCURR 1
  CHECK(opencir->ms - opening->ms == 10 * (1 + 10 + 1 + 2 + 3 + 201 + 1));
  CHECK(+myvalvemots[1].actual_position == 60);  // the target: the position is unknown
  CHECK(+myvalvemots[1].connected == 0);
  CHECK(rig.valve[1].pulses == 0);
}

TEST_CASE("presence test: a connected valve is PRESENT, a disconnected one OPENCIR") {
  sim::Rig rig;
  startValves(rig);
  rig.valve[3].connected = false;
  REQUIRE(appsetaction(CMD_A_TEST, 2, 0) == 0);
  rig.runMs(10);
  REQUIRE(rig.runUntil(idle, 10000));
  REQUIRE(appsetaction(CMD_A_TEST, 3, 0) == 0);
  rig.runMs(10);
  REQUIRE(rig.runUntil(idle, 10000));
  CHECK(+myvalvemots[2].status == VLV_STATE_PRESENT);
  CHECK(+myvalvemots[3].status == VLV_STATE_OPENCIR);
  CHECK(+myvalvemots[3].connected == 0);
  CHECK(rig.valve[2].enables == 1);
}

TEST_CASE("appsetaction: taken only while idle without a pending command, valve 11 yes, 12 no") {
  sim::Rig rig;
  startValves(rig);
  CHECK(appsetaction(CMD_A_TEST, 12, 0) == -1);
  CHECK(appsetaction(CMD_A_TEST, 11, 0) == 0);
  CHECK_FALSE(valve_idle());
  CHECK(appsetaction(CMD_A_TEST, 10, 0) == -1);
  CHECK(appsetaction(CMD_A_TEST, 10, 0, true) == 0);
  CHECK(fake::board.irqDisables >= 4);
  CHECK(__get_PRIMASK() == 0);
}
