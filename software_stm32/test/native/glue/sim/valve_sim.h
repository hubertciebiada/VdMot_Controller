// Valve sim of the STM glue tests (harness-stm 2.6, HS-13): twelve actuators behind the L293/MUX
// wiring of include/hardware.h, driving the real src/motor.cpp through the fake board.
//
// Every millisecond of fake time, in this order: the motor model (pulses on REVINPIN through the
// EXTI handler, which may cut the enable at once), the timer interrupts (TIM1 TimerHandler0 once
// valve_setup() attached it, TIM2 valve_loop once setup_system() attached it), then valve_loop()
// every 10th ms while no timer calls it (glue_motor: main.cpp is a stub). Other interleavings of the
// interrupts are injected by the tests.
//
// A valve moves while the valve PSU is on (POWER_ENA open drain, low), its L293 enable is high and
// the MUX selects it (even valve: MUX_ON level). Currents in 0.1 mA, positive while opening, read by
// TimerHandler0 through ANINCURRENT - ANINREFHALF (12-bit, the inverse of its conversion); the test
// sets analogReadResolution(12) like setup_system() does.
#pragma once

#include <stdint.h>

#include <functional>
#include <vector>

#include "hardware.h"
#include "motor.h"

namespace sim {

struct Valve {
  bool connected = true;             // false: open circuit, no current, no pulses
  bool shorted = false;              // shortCurrent_dmA once enabled, no pulses
  int32_t position = 1800;           // counts from the closed end stop
  int32_t stroke = 3600;             // counts between the end stops
  float pulsesPerMs = 0.2f;          // motor speed
  int32_t runCurrent_dmA = 250;      // turning freely
  int32_t stallCurrent_dmA = 700;    // at an end stop or at the obstacle
  int32_t shortCurrent_dmA = 2000;
  int32_t inrushPeak_dmA = 0;        // current in the first inrushMs after the enable (0: no inrush)
  uint32_t inrushMs = 0;
  int32_t coastPulses = 0;           // pulses the motor turns on after the enable went off
  int32_t jamFrom = -1;              // obstacle: opening stalls in [jamFrom, jamTo),
  int32_t jamTo = -1;                //           closing stalls in (jamFrom, jamTo]

  // ---- read by the tests
  uint32_t pulses = 0;               // pulses the valve turned (coast included)
  uint32_t enables = 0;              // times its enable went on
};

class Rig {
 public:
  Rig() = default;
  ~Rig();
  Rig(const Rig&) = delete;
  Rig& operator=(const Rig&) = delete;

  Valve valve[ACTUATOR_COUNT];

  // Hooks the model into the fake board (after glue::begin(), which removes the hooks).
  void install();
  void runMs(uint32_t ms);
  // Runs until done() is true (checked before every millisecond); false after maxMs.
  bool runUntil(const std::function<bool()>& done, uint32_t maxMs);

  bool powered() const;       // valve PSU on
  int enabledValve() const;   // valve whose enable and MUX position are active, -1 if none
  bool opening() const;       // CTRL_DIRECTION low
  int32_t current_dmA() const { return current_; }

  struct Step {
    uint32_t ms;
    uint8_t valvestate;
    uint8_t valve;   // valve whose status changed, 255 for a change of valvestate
    uint8_t status;
  };
  std::vector<Step> transitions;  // every change of valvestate or of a valve status
  uint32_t ms = 0;                // ms since install()
  uint32_t conflicts = 0;         // ms with more than one enable high

 private:
  void onMs();
  void afterMs();
  uint32_t adc(uint32_t pin) const;

  int32_t current_ = 0;
  int lastEnabled_ = -1;
  uint32_t enabledMs_ = 0;
  float pulseAcc_[ACTUATOR_COUNT] = {};
  int32_t coastLeft_[ACTUATOR_COUNT] = {};
  int coastDir_[ACTUATOR_COUNT] = {};
  uint8_t lastState_ = 0xFF;
  uint8_t lastStatus_[ACTUATOR_COUNT] = {};
  bool installed_ = false;
};

}  // namespace sim
