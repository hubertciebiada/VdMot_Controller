// Test side of the fake board of the STM glue tests: state of the fakes, time, interrupts, UART,
// EEPROM and 1-Wire bus helpers. Included by tests, stubs and the valve sim only (STL allowed); the
// glue sees the framework headers of this directory.
//
// Persistent stores (the 24LC64 array, .noinit RAM, RCC->CSR) belong to the boot: the runner hooks
// (glue/runner_hooks.cpp) hand them to the next boot of a case, fake::reset() leaves them alone.
#pragma once

#include <stdint.h>

#include <deque>
#include <functional>
#include <string>
#include <vector>

#include "Arduino.h"
#include "DallasTemperature.h"
#include "I2C_eeprom.h"
#include "IWatchdog.h"
#include "OneWire.h"
#include "STM32TimerInterrupt.h"
#include "Wire.h"

namespace fake {

constexpr uint32_t kPins = NUM_DIGITAL_PINS;
constexpr uint32_t kAnyPin = 0xFFFFFFFF;
constexpr uint32_t kEepromSize = 8192;  // 24LC64

// Calls that do not return on the device throw these (glue::run() turns the resets into reboots).
struct SystemReset {};     // HAL_NVIC_SystemReset()
struct WatchdogReset {};   // the IWDG ran out
struct BootloaderJump {};  // JumpToBootloader() (glue/stubs/stub_boot_jump.cpp)

// Recorded calls, in one sequence over all kinds. pin/value: Mode pin/mode, Write pin/level,
// Attach pin/mode, Detach pin, WireBegin sda/scl, WireEnd, Delay -/ms, DelayUs -/us,
// WatchdogBegin -/timeout us, Reload.
enum class Ev : uint8_t {
  Mode,
  Write,
  Attach,
  Detach,
  WireBegin,
  WireEnd,
  Delay,
  DelayUs,
  WatchdogBegin,
  Reload
};
struct Event {
  uint32_t seq;
  Ev kind;
  uint32_t pin;
  uint32_t value;
  uint64_t us;  // fake time of the call
  bool operator==(const Event& o) const {
    return kind == o.kind && pin == o.pin && value == o.value;
  }
};

struct Board {
  uint64_t nowUs = 0;
  uint32_t autoAdvanceUs = 0;  // added by every millis()/micros() call: busy loops see time pass
  uint32_t mode[kPins] = {};
  uint8_t out[kPins] = {};      // output latch
  uint8_t in[kPins] = {};       // input level without an input hook
  std::function<int(uint32_t pin)> input;       // input level of a pin; < 0: use in[pin]
  std::function<uint32_t(uint32_t pin)> analog;  // 12-bit ADC value of a pin (valve sim)
  uint32_t analogValue[kPins] = {};              // 12-bit ADC value without an analog hook
  int analogBits = 10;                           // analogReadResolution()
  void (*exti[kPins])() = {};
  uint32_t extiMode[kPins] = {};
  uint32_t primask = 0;
  uint32_t irqDisables = 0;     // __disable_irq() calls
  uint32_t devId = 0x423;       // STM32F401xB/C
  uint32_t portClocks = 0;      // bit n: set_GPIO_Port_Clock(n)
  // every millisecond boundary the time passes: onMs (valve sim: motor, pulses), the timers,
  // afterMs; not while PRIMASK is set
  std::function<void()> onMs;
  std::function<void()> afterMs;
  std::vector<Event> events;
  uint32_t seq = 0;
  size_t eventLimit = 1000000;  // later events are counted in eventsDropped only
  size_t eventsDropped = 0;
};
extern Board board;

// Volatile state of every fake back to the start of a boot; the persistent stores stay.
void reset();
// Power-on: reset() and the power-on reset flags in RCC->CSR (PORRSTF, PINRSTF, BORRSTF).
void powerOn();

// Time. Every millisecond boundary on the way runs board.onMs, the attached timers and
// board.afterMs; afterwards an IWatchdog that was not reloaded for its timeout throws WatchdogReset.
void advanceUs(uint64_t us);
void advanceMs(uint32_t ms);

std::vector<Event> eventsOf(Ev kind, uint32_t pin = kAnyPin);
// Rising edge on an EXTI pin: runs the attached handler (not while PRIMASK is set).
void fireExti(uint32_t pin);
std::vector<STM32TimerInterrupt*> timers();
STM32TimerInterrupt* timerOf(TIM_TypeDef* tim);

// UART: bytes arrive one by one through the port's RX callback (nothing arrives before begin()).
void inject(HardwareSerial& port, const std::string& bytes);
// HAL_UART_ERROR_* reported with the next injected byte
void uartError(HardwareSerial& port, uint32_t halError);
std::string takeTx(HardwareSerial& port);  // written bytes since the last takeTx(), then cleared
std::string tx(HardwareSerial& port);      // the same without clearing

// 24LC64. Transfers are numbered from 1 per direction; a failing write returns writeError, a
// failing read returns 0 and leaves the buffer as it was.
struct EepromOp {
  bool write;
  uint16_t address;
  uint16_t length;
  bool ok;
};
struct Eeprom {
  Eeprom() { memset(bytes, 0xFF, sizeof bytes); }  // a new controller: erased
  uint8_t bytes[kEepromSize];
  std::vector<EepromOp> ops;
  unsigned writes = 0;
  unsigned reads = 0;
  unsigned failWritesFrom = 0;   // write #n fails (0: none) ...
  unsigned failWritesCount = 0;  // ... and the next count - 1 (0: every later one)
  unsigned failReadsFrom = 0;
  unsigned failReadsCount = 0;
  int writeError = 2;            // Wire: NACK on the address
  uint32_t usPerTransfer = 0;    // fake time of a transfer
};
extern Eeprom eeprom;
void eraseEeprom();  // every byte 0xFF

// 1-Wire bus of the DS18B20 sensors and DS2438 monitors.
struct OneWireDevice {
  uint8_t rom[8];
  int16_t raw = 0;          // temperature in 1/128 degC
  uint8_t resolution = 12;  // bits
  bool present = true;
  unsigned failReads = 0;   // the next reads return DEVICE_DISCONNECTED_RAW
};
struct OneWireBus {
  std::deque<OneWireDevice> devices;  // search order; references stay valid when devices are added
};
extern OneWireBus oneWireBus;
// A device with a valid ROM: family, serial, 0, 0, 0, 0, 0, CRC.
OneWireDevice& addOneWire(uint8_t family, uint8_t serial, int16_t raw = 0);
// The present device with this ROM, nullptr if there is none.
OneWireDevice* findOneWire(const uint8_t* rom);

}  // namespace fake
