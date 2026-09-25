#include "valve_sim.h"

#include <math.h>

#include "fake_board.h"

namespace sim {

namespace {

const uint32_t kEnablePins[ACTUATOR_COUNT / 2] = {CTRL_ENA0, CTRL_ENA1, CTRL_ENA2,
                                                  CTRL_ENA3, CTRL_ENA4, CTRL_ENA5};

// MUX level that selects the even valve of an L293 channel (hardware.h MUX_ON)
#ifdef HARDWARE_REVISION_C1
constexpr uint8_t kMuxEvenLevel = HIGH;
#else
constexpr uint8_t kMuxEvenLevel = LOW;
#endif

constexpr uint32_t kAdcMid = 2048;  // ANINREFHALF: reference / 2 of the current amplifier

bool isOutput(uint32_t pin) {
  return fake::board.mode[pin] == OUTPUT || fake::board.mode[pin] == OUTPUT_OPEN_DRAIN;
}

bool timerRunsValveLoop() {
  for (const STM32TimerInterrupt* t : fake::timers()) {
    if (t->callback == valve_loop) return true;
  }
  return false;
}

// the valve is at an end stop or at its obstacle in the direction dir
bool stalled(const Valve& v, int dir) {
  if (dir > 0) return v.position >= v.stroke || (v.jamFrom >= 0 && v.position >= v.jamFrom && v.position < v.jamTo);
  return v.position <= 0 || (v.jamFrom >= 0 && v.position > v.jamFrom && v.position <= v.jamTo);
}

}  // namespace

Rig::~Rig() {
  if (!installed_) return;
  fake::board.onMs = nullptr;
  fake::board.afterMs = nullptr;
  fake::board.analog = nullptr;
}

void Rig::install() {
  installed_ = true;
  ms = 0;
  lastState_ = static_cast<uint8_t>(valvestate);
  for (unsigned v = 0; v < ACTUATOR_COUNT; v++) lastStatus_[v] = myvalvemots[v].status;
  fake::board.onMs = [this] { onMs(); };
  fake::board.afterMs = [this] { afterMs(); };
  fake::board.analog = [this](uint32_t pin) { return adc(pin); };
}

void Rig::runMs(uint32_t n) { fake::advanceMs(n); }

bool Rig::runUntil(const std::function<bool()>& done, uint32_t maxMs) {
  for (uint32_t i = 0; i < maxMs; i++) {
    if (done()) return true;
    fake::advanceMs(1);
  }
  return done();
}

bool Rig::powered() const { return isOutput(POWER_ENA) && fake::board.out[POWER_ENA] == LOW; }

int Rig::enabledValve() const {
  for (unsigned k = 0; k < ACTUATOR_COUNT / 2; k++) {
    const uint32_t pin = kEnablePins[k];
    if (isOutput(pin) && fake::board.out[pin] == HIGH) {
      const bool even = fake::board.out[CTRL_MUX] == kMuxEvenLevel;
      return static_cast<int>(2 * k + (even ? 0 : 1));
    }
  }
  return -1;
}

bool Rig::opening() const { return fake::board.out[CTRL_DIRECTION] == LOW; }

uint32_t Rig::adc(uint32_t pin) const {
  if (pin == ANINREFHALF) return kAdcMid;
  if (pin == ANINCURRENT) {
    // inverse of TimerHandler0: current = (adc - ref) * ANINCURRENTGAIN / 100
    const long counts = lround(static_cast<double>(current_) * 100.0 / ANINCURRENTGAIN);
    const long adc = static_cast<long>(kAdcMid) + counts;
    return static_cast<uint32_t>(adc < 0 ? 0 : (adc > 4095 ? 4095 : adc));
  }
  return fake::board.analogValue[pin];
}

void Rig::onMs() {
  ms++;
  unsigned enables = 0;
  for (uint32_t pin : kEnablePins) {
    if (isOutput(pin) && fake::board.out[pin] == HIGH) enables++;
  }
  if (enables > 1) conflicts++;

  const int dir = opening() ? 1 : -1;
  const int e = powered() ? enabledValve() : -1;
  if (e != lastEnabled_) {
    // the motor that lost its enable turns on for its coast pulses
    if (lastEnabled_ >= 0) {
      coastLeft_[lastEnabled_] = valve[lastEnabled_].coastPulses;
      pulseAcc_[lastEnabled_] = 0;
    }
    if (e >= 0) {
      valve[e].enables++;
      coastLeft_[e] = 0;
      coastDir_[e] = dir;
      pulseAcc_[e] = 0;
    }
    enabledMs_ = 0;
    lastEnabled_ = e;
  }

  current_ = 0;
  if (e >= 0) {
    Valve& v = valve[e];
    enabledMs_++;
    coastDir_[e] = dir;
    if (v.connected && v.shorted) {
      current_ = dir * v.shortCurrent_dmA;
    } else if (v.connected) {
      const bool inrush = v.inrushPeak_dmA != 0 && enabledMs_ <= v.inrushMs;
      current_ = dir * (inrush ? v.inrushPeak_dmA : (stalled(v, dir) ? v.stallCurrent_dmA : v.runCurrent_dmA));
      pulseAcc_[e] += v.pulsesPerMs;
      // every pulse runs the EXTI handler, which may switch the enable off at once
      while (pulseAcc_[e] >= 1.0f && !stalled(v, dir) && enabledValve() == e) {
        pulseAcc_[e] -= 1.0f;
        v.position += dir;
        v.pulses++;
        fake::fireExti(REVINPIN);
      }
      if (stalled(v, dir)) pulseAcc_[e] = 0;
    }
    if (enabledValve() != e) current_ = 0;
  }

  for (unsigned k = 0; k < ACTUATOR_COUNT; k++) {
    if (static_cast<int>(k) == e || coastLeft_[k] <= 0) continue;
    Valve& v = valve[k];
    pulseAcc_[k] += v.pulsesPerMs;
    while (pulseAcc_[k] >= 1.0f && coastLeft_[k] > 0 && !stalled(v, coastDir_[k])) {
      pulseAcc_[k] -= 1.0f;
      coastLeft_[k]--;
      v.position += coastDir_[k];
      v.pulses++;
      fake::fireExti(REVINPIN);
    }
    if (stalled(v, coastDir_[k])) coastLeft_[k] = 0;
  }
}

void Rig::afterMs() {
  if (ms % 10 == 0 && !timerRunsValveLoop()) valve_loop();
  const uint8_t state = static_cast<uint8_t>(valvestate);
  if (state != lastState_) {
    transitions.push_back({ms, state, 255, 0});
    lastState_ = state;
  }
  for (unsigned v = 0; v < ACTUATOR_COUNT; v++) {
    const uint8_t status = myvalvemots[v].status;
    if (status != lastStatus_[v]) {
      transitions.push_back({ms, state, static_cast<uint8_t>(v), status});
      lastStatus_[v] = status;
    }
  }
}

}  // namespace sim
