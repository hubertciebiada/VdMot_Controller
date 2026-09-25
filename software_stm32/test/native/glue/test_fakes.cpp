// Tests of the fake board, the runner hooks and the valve sim (glue_fakes): the glue suites trust
// them, so each fake behaviour the glue depends on is pinned here.
#include <string>
#include <vector>

#include "glue_test.h"
#include "hardware.h"
#include "stub_motor.h"
#include "valve_sim.h"

using fake::Ev;

namespace {

// Print into a port and return what it wrote.
template <class F>
std::string printed(F&& fn) {
  fake::takeTx(Serial6);
  fn(Serial6);
  return fake::takeTx(Serial6);
}

// Stands for warm RAM of the firmware (WarmState, reset counter): kept by warm resets.
__attribute__((section(".noinit"))) uint8_t g_warm[16];

bool warmIs(uint8_t value) {
  for (uint8_t b : g_warm) {
    if (b != value) return false;
  }
  return true;
}

unsigned g_extiCalls = 0;
void countExti() { g_extiCalls++; }

std::vector<std::string> g_ticks;
void tim1Tick() { g_ticks.push_back("tim1@" + std::to_string(millis())); }
void tim2Tick() { g_ticks.push_back("tim2@" + std::to_string(millis())); }

serial_t* serialOf(HardwareSerial& port) {
  // the arithmetic of the core's get_serial_obj(), which no header declares
  return reinterpret_cast<serial_t*>(reinterpret_cast<char*>(port.getHandle()) - offsetof(serial_t, handle));
}

std::vector<uint32_t> g_rxErrors;
void countingRx(serial_t* obj) {
  g_rxErrors.push_back(+obj->handle.ErrorCode);
  HardwareSerial::_rx_complete_irq(obj);
}

}  // namespace

// ---------------------------------------------------------------- Print

TEST_CASE("Print: numbers are formatted like on the 32-bit target") {
  glue::begin();
  CHECK(printed([](Print& p) { p.print(-1, HEX); }) == "FFFFFFFF");
  CHECK(printed([](Print& p) { p.print(-5, DEC); }) == "-5");
  CHECK(printed([](Print& p) { p.print(static_cast<uint8_t>(65), DEC); }) == "65");
  CHECK(printed([](Print& p) { p.print('A'); }) == "A");
  CHECK(printed([](Print& p) { p.println(); }) == "\r\n");
  CHECK(printed([](Print& p) { p.print(65, 0); }) == "A");
  CHECK(printed([](Print& p) { p.print(3000000000UL); }) == "3000000000");
  CHECK(printed([](Print& p) { p.print(1.5, 2); }) == "1.50");
  CHECK(printed([](Print& p) { p.print(-2147483647L - 1); }) == "-2147483648");
  CHECK(printed([](Print& p) { p.print(static_cast<uint32_t>(0x423)); }) == "1059");
  CHECK(printed([](Print& p) { p.print(255, HEX); }) == "FF");
  CHECK(printed([](Print& p) { p.print(5, 1); }) == "5");
  CHECK(printed([](Print& p) { p.print(1.999f, 2); }) == "2.00");
  CHECK(printed([](Print& p) { p.print(-0.25, 1); }) == "-0.3");
  CHECK(printed([](Print& p) { p.println("gvers"); }) == "gvers\r\n");
}

TEST_CASE("Print: println of every argument kind ends with CR LF") {
  glue::begin();
  CHECK(printed([](Print& p) { p.println(7, DEC); }) == "7\r\n");
  CHECK(printed([](Print& p) { p.println(static_cast<unsigned int>(7), DEC); }) == "7\r\n");
  CHECK(printed([](Print& p) { p.println(static_cast<uint8_t>(7), DEC); }) == "7\r\n");
  CHECK(printed([](Print& p) { p.println('x'); }) == "x\r\n");
  CHECK(printed([](Print& p) { p.println(String(12u) + ":" + String(3u)); }) == "12:3\r\n");
}

// ---------------------------------------------------------------- UART

TEST_CASE("UART: bytes arrive only after begin(), in order across the wrap of the ring") {
  glue::begin();
  fake::inject(Serial1, "lost");
  CHECK(Serial1.lostBytes == 4);
  CHECK(Serial1.available() == 0);
  Serial1.begin(115200, SERIAL_8N1);
  CHECK(Serial1.baud == 115200);
  CHECK(Serial1.config == SERIAL_8N1);
  CHECK(static_cast<bool>(Serial1));
  std::string sent;
  std::string got;
  for (int round = 0; round < 3; round++) {
    const std::string chunk(700, static_cast<char>('a' + round));
    fake::inject(Serial1, chunk);
    sent += chunk;
    while (Serial1.available() > 0) got += static_cast<char>(Serial1.read());
  }
  CHECK(got == sent);
  CHECK(Serial1.read() == -1);
  CHECK(Serial1.readCalls == 2101);
}

TEST_CASE("UART: the ring holds SERIAL_RX_BUFFER_SIZE - 1 bytes, later bytes are dropped") {
  glue::begin();
  Serial1.begin(115200);
  fake::inject(Serial1, std::string(SERIAL_RX_BUFFER_SIZE + 5, 'x'));
  CHECK(Serial1.available() == SERIAL_RX_BUFFER_SIZE - 1);
}

TEST_CASE("UART: the RX callback can be replaced through getHandle() and sees the HAL error of its byte") {
  glue::begin();
  g_rxErrors.clear();
  CHECK(HAL_UART_ERROR_PE == 0x01);
  CHECK(HAL_UART_ERROR_NE == 0x02);
  CHECK(HAL_UART_ERROR_FE == 0x04);
  CHECK(HAL_UART_ERROR_ORE == 0x08);
  Serial1.begin(115200);
  serial_t* obj = serialOf(Serial1);
  CHECK(obj->rx_callback == HardwareSerial::_rx_complete_irq);
  obj->rx_callback = countingRx;
  fake::inject(Serial1, "a");
  fake::uartError(Serial1, HAL_UART_ERROR_FE);
  fake::inject(Serial1, "bc");
  CHECK(g_rxErrors == std::vector<uint32_t>{0, HAL_UART_ERROR_FE, 0});
  CHECK(+obj->rx_head == 3);
  CHECK(obj->rx_tail == 0);
  CHECK(Serial1.getHandle()->ErrorCode == HAL_UART_ERROR_NONE);
  // begin() attaches the core's handler again (the firmware replaces it after begin)
  Serial1.begin(115200);
  CHECK(obj->rx_callback == HardwareSerial::_rx_complete_irq);
}

TEST_CASE("UART: readBytes() of missing bytes waits its timeout in fake time") {
  glue::begin();
  Serial1.begin(115200);
  fake::inject(Serial1, "abc");
  uint8_t buffer[8] = {};
  CHECK(Serial1.readBytes(buffer, 8) == 3);
  CHECK(std::string(reinterpret_cast<char*>(buffer), 3) == "abc");
  CHECK(millis() == 1000);
}

TEST_CASE("UART: end() drops the received bytes and stops the reception") {
  glue::begin();
  Serial1.begin(9600);
  fake::inject(Serial1, "ab");
  Serial1.end();
  CHECK(Serial1.available() == 0);
  CHECK_FALSE(static_cast<bool>(Serial1));
  fake::inject(Serial1, "c");
  CHECK(Serial1.lostBytes == 1);
}

// ---------------------------------------------------------------- time, pins, interrupts

TEST_CASE("time: millis() wraps at 2^32 ms, delay() and delayMicroseconds() advance it") {
  glue::begin();
  fake::board.nowUs = (1ull << 32) * 1000 - 2000;
  CHECK(millis() == 0xFFFFFFFEu);
  delay(3);
  CHECK(millis() == 1);
  delayMicroseconds(1500);
  CHECK(micros() == static_cast<uint32_t>(fake::board.nowUs));
  CHECK(millis() == 2);
  CHECK(fake::eventsOf(Ev::Delay) == std::vector<fake::Event>{{0, Ev::Delay, 0, 3, 0}});
  CHECK(fake::eventsOf(Ev::DelayUs) == std::vector<fake::Event>{{0, Ev::DelayUs, 0, 1500, 0}});
}

TEST_CASE("time: autoAdvanceUs lets a busy loop on millis() see time pass") {
  glue::begin();
  fake::board.autoAdvanceUs = 250;
  const uint32_t start = millis();
  unsigned polls = 0;
  while (millis() - start < 10) polls++;
  CHECK(polls == 39);
}

TEST_CASE("pins: writes, modes and reads are recorded in one sequence") {
  glue::begin();
  digitalWrite(PB9, HIGH);
  pinMode(PB9, OUTPUT_OPEN_DRAIN);
  CHECK(fake::board.events.size() == 2);
  CHECK(fake::board.events[0].kind == Ev::Write);
  CHECK(fake::board.events[1].kind == Ev::Mode);
  CHECK(fake::board.events[0].seq < fake::board.events[1].seq);
  // open drain: a 1 in the latch releases the line (the board pulls it up or a test drives it)
  fake::board.in[PB9] = 1;
  CHECK(digitalRead(PB9) == HIGH);
  fake::board.in[PB9] = 0;
  CHECK(digitalRead(PB9) == LOW);
  digitalWrite(PB9, LOW);
  fake::board.in[PB9] = 1;
  CHECK(digitalRead(PB9) == LOW);
  pinMode(PA8, OUTPUT);
  digitalWrite(PA8, HIGH);
  CHECK(digitalRead(PA8) == HIGH);
  // I2C lines are pulled up on the board
  pinMode(PB7, INPUT);
  CHECK(digitalRead(PB7) == HIGH);
  fake::board.input = [](uint32_t pin) { return pin == PB7 ? 0 : -1; };
  CHECK(digitalRead(PB7) == LOW);
  CHECK(digitalRead(PB6) == HIGH);
}

TEST_CASE("pins: analogRead() maps the 12-bit value to the configured resolution") {
  glue::begin();
  fake::board.analogValue[PA0] = 4095;
  CHECK(analogRead(PA0) == 1023);
  analogReadResolution(12);
  CHECK(analogRead(PA0) == 4095);
  fake::board.analog = [](uint32_t pin) { return pin == PA1 ? 2048u : 5000u; };
  CHECK(analogRead(PA1) == 2048);
  CHECK(analogRead(PA0) == 4095);
}

TEST_CASE("EXTI: fireExti runs an attached handler, not a detached one, not with interrupts disabled") {
  glue::begin();
  g_extiCalls = 0;
  fake::fireExti(PA4);
  CHECK(g_extiCalls == 0);
  attachInterrupt(digitalPinToInterrupt(PA4), countExti, RISING);
  fake::fireExti(PA4);
  CHECK(g_extiCalls == 1);
  __disable_irq();
  fake::fireExti(PA4);
  __enable_irq();
  CHECK(g_extiCalls == 1);
  detachInterrupt(digitalPinToInterrupt(PA4));
  fake::fireExti(PA4);
  CHECK(g_extiCalls == 1);
  CHECK(fake::eventsOf(Ev::Attach, PA4).size() == 1);
  CHECK(fake::eventsOf(Ev::Detach, PA4).size() == 1);
}

TEST_CASE("PRIMASK: __get/__set restore the previous state and count the disables") {
  glue::begin();
  const uint32_t outer = __get_PRIMASK();
  __disable_irq();
  const uint32_t inner = __get_PRIMASK();
  __disable_irq();
  __set_PRIMASK(inner);
  CHECK(__get_PRIMASK() == 1);
  __set_PRIMASK(outer);
  CHECK(__get_PRIMASK() == 0);
  CHECK(fake::board.irqDisables == 2);
}

TEST_CASE("timers: attached callbacks run at their interval, TIM1 before TIM2, not while masked") {
  glue::begin();
  g_ticks.clear();
  STM32Timer t2(TIM2);
  STM32Timer t1(TIM1);
  CHECK(t2.attachInterruptInterval(2000, tim2Tick));
  CHECK(t1.attachInterruptInterval(1000, tim1Tick));
  fake::advanceMs(2);
  CHECK(g_ticks == std::vector<std::string>{"tim1@1", "tim1@2", "tim2@2"});
  g_ticks.clear();
  __disable_irq();
  fake::advanceMs(3);
  __enable_irq();
  CHECK(g_ticks.empty());
  t1.failAttach = true;
  CHECK_FALSE(t1.attachInterruptInterval(1000, tim1Tick));
  CHECK(t1.attaches == 2);
  CHECK(fake::timerOf(TIM1) == &t1);
}

TEST_CASE("watchdog: time passing its timeout without a reload is a watchdog reset") {
  glue::begin();
  IWatchdog.reload();
  CHECK(IWatchdog.reloads == 0);  // not started: reload() does nothing
  IWatchdog.begin(100);           // below IWDG_TIMEOUT_MIN
  CHECK_FALSE(IWatchdog.isEnabled());
  IWatchdog.begin(8000000);
  CHECK(IWatchdog.timeoutUs == 8000000);
  fake::advanceMs(7999);
  IWatchdog.reload();
  CHECK(IWatchdog.lastReloadMs == 7999);
  fake::advanceMs(7999);
  CHECK_THROWS_AS(fake::advanceMs(1), fake::WatchdogReset);
  CHECK(fake::eventsOf(Ev::WatchdogBegin).size() == 1);
  CHECK(fake::eventsOf(Ev::Reload).size() == 1);
}

TEST_CASE("HAL: a system reset throws, the device id is the F401CC's") {
  glue::begin();
  CHECK_THROWS_AS(HAL_NVIC_SystemReset(), fake::SystemReset);
  CHECK(HAL_GetDEVID() == 0x423);
}

TEST_CASE("Wire: pins, begin and end are recorded") {
  glue::begin();
  Wire.setSDA(PB7);
  Wire.setSCL(PB6);
  Wire.begin();
  Wire.end();
  CHECK(Wire.begins == 1);
  CHECK(Wire.ends == 1);
  CHECK_FALSE(Wire.running);
  CHECK(fake::eventsOf(Ev::WireBegin) == std::vector<fake::Event>{{0, Ev::WireBegin, PB7, PB6, 0}});
  CHECK(fake::eventsOf(Ev::WireEnd).size() == 1);
}

// ---------------------------------------------------------------- 24LC64

TEST_CASE("EEPROM: erased, written and read back, transfers logged") {
  glue::begin();
  I2C_eeprom chip(0x50, 8192);
  CHECK(chip.deviceAddress == 0x50);
  CHECK(chip.readByte(100) == 0xFF);
  const uint8_t data[3] = {1, 2, 3};
  CHECK(chip.writeBlock(8191, data, 3) == 0);  // 13 address bits: wraps to 0
  uint8_t back[3] = {};
  CHECK(chip.readBlock(8191, back, 3) == 3);
  CHECK(back[2] == 3);
  CHECK(fake::eeprom.bytes[0] == 2);
  CHECK(fake::eeprom.ops.size() == 3);
  CHECK(fake::eeprom.ops[1].write);
  CHECK(fake::eeprom.ops[1].address == 8191);
  CHECK(fake::eeprom.ops[1].length == 3);
}

TEST_CASE("EEPROM: failReadsFrom 2, failReadsCount 1 fails exactly read #2; failing writes change nothing") {
  glue::begin();
  I2C_eeprom chip(0x50, 8192);
  fake::eeprom.failReadsFrom = 2;
  fake::eeprom.failReadsCount = 1;
  uint8_t b[2] = {7, 7};
  CHECK(chip.readBlock(0, b, 2) == 2);
  b[0] = 7;
  CHECK(chip.readBlock(0, b, 2) == 0);
  CHECK(b[0] == 7);
  CHECK(chip.readBlock(0, b, 2) == 2);
  fake::eeprom.failWritesFrom = 1;
  const uint8_t data[1] = {9};
  CHECK(chip.writeBlock(5, data, 1) == 2);
  CHECK(chip.writeBlock(5, data, 1) == 2);
  CHECK(fake::eeprom.bytes[5] == 0xFF);
  CHECK_FALSE(fake::eeprom.ops.back().ok);
}

// ---------------------------------------------------------------- 1-Wire

TEST_CASE("1-Wire: search in list order, CRC of lib/core, DallasTemperature on the bus") {
  glue::begin();
  fake::addOneWire(0x28, 1, 21 * 128);
  fake::addOneWire(0x26, 2);
  fake::OneWireDevice& gone = fake::addOneWire(0x28, 3);
  gone.present = false;
  OneWire bus(PB10);
  DallasTemperature dallas(&bus);
  dallas.begin();
  CHECK(dallas.getDeviceCount() == 2);
  CHECK(dallas.getResolution() == 12);
  CHECK(DallasTemperature::millisToWaitForConversion(dallas.getResolution()) == 750);
  CHECK(DallasTemperature::millisToWaitForConversion(9) == 94);
  DeviceAddress a;
  bus.reset_search();
  REQUIRE(bus.search(a));
  CHECK(a[1] == 1);
  CHECK(dallas.validAddress(a));
  CHECK(dallas.validFamily(a));
  CHECK(dallas.getTempC(a) == 21.0f);
  REQUIRE(bus.search(a));
  CHECK_FALSE(dallas.validFamily(a));
  CHECK_FALSE(bus.search(a));
  a[7] ^= 1;
  CHECK_FALSE(dallas.validAddress(a));
}

TEST_CASE("1-Wire: getTemp() gives DEVICE_DISCONNECTED_RAW for a failing read and an absent sensor") {
  glue::begin();
  fake::OneWireDevice& s = fake::addOneWire(0x28, 1, 85 * 128);
  s.failReads = 1;
  OneWire bus(PB10);
  DallasTemperature dallas(&bus);
  CHECK(dallas.getTemp(s.rom) == DEVICE_DISCONNECTED_RAW);
  CHECK(dallas.getTemp(s.rom) == 85 * 128);
  s.present = false;
  CHECK(dallas.getTempC(s.rom) == DEVICE_DISCONNECTED_C);
  CHECK(dallas.reads == 3);
}

// ---------------------------------------------------------------- stubs, runner hooks

TEST_CASE("stubs: glue::begin() empties the call log and restores every knob") {
  glue::begin();
  CHECK(stub::calls.empty());
  stub::motor.action = -1;
  myvalvemots[3].status = 7;
  CHECK(appsetaction('o', 3, 10) == -1);
  CHECK(stub::calls == stub::Calls{"appsetaction(o, 3, 10, 0)"});
  glue::begin();
  CHECK(stub::calls.empty());
  CHECK(stub::motor.action == 0);
  CHECK(+myvalvemots[3].status == 0);
}

TEST_CASE("runner hooks: warm RAM is 0xA5 after power-on and survives pin, software and watchdog resets") {
  glue::begin();
  switch (testkit::boot()) {
    case 0:
      CHECK(warmIs(0xA5));
      CHECK(RCC->CSR == (RCC_CSR_PORRSTF | RCC_CSR_PINRSTF | RCC_CSR_BORRSTF));
      memset(g_warm, 1, sizeof g_warm);
      fake::eeprom.bytes[10] = 1;
      testkit::reboot(testkit::Reset::Pin);
    case 1:
      CHECK(warmIs(1));
      CHECK(fake::eeprom.bytes[10] == 1);
      // the power-on flags were not cleared (no RMVF): they add up
      CHECK(RCC->CSR == (RCC_CSR_PORRSTF | RCC_CSR_PINRSTF | RCC_CSR_BORRSTF));
      RCC->CSR |= RCC_CSR_RMVF;
      CHECK(RCC->CSR == 0);
      memset(g_warm, 2, sizeof g_warm);
      testkit::reboot(testkit::Reset::Software);
    case 2:
      CHECK(warmIs(2));
      CHECK(RCC->CSR == (RCC_CSR_SFTRSTF | RCC_CSR_PINRSTF));
      RCC->CSR |= RCC_CSR_RMVF;
      memset(g_warm, 3, sizeof g_warm);
      testkit::reboot(testkit::Reset::Watchdog);
    case 3:
      CHECK(warmIs(3));
      CHECK(RCC->CSR == (RCC_CSR_IWDGRSTF | RCC_CSR_PINRSTF));
      CHECK(IWatchdog.isReset(true));
      CHECK(RCC->CSR == 0);
      fake::eeprom.bytes[10] = 4;
      testkit::reboot(testkit::Reset::PowerOn);
    default:
      CHECK(testkit::boot() == 4);
      CHECK(warmIs(0xA5));
      CHECK(fake::eeprom.bytes[10] == 4);  // the EEPROM survives a power-on too
      CHECK(RCC->CSR == (RCC_CSR_PORRSTF | RCC_CSR_PINRSTF | RCC_CSR_BORRSTF));
  }
}

TEST_CASE("runner hooks: noinitSnapshot() copies the whole .noinit section") {
  glue::begin();
  const std::vector<uint8_t> warm = glue::noinitSnapshot();
  CHECK(warm.size() >= sizeof g_warm);
  for (uint8_t b : warm) CHECK(b == 0xA5);
}

TEST_CASE("glue::run turns a system reset into a software reboot and a watchdog reset into a watchdog reboot") {
  glue::begin();
  switch (testkit::boot()) {
    case 0:
      glue::run([] { HAL_NVIC_SystemReset(); });
      FAIL("glue::run returned after a system reset");
      break;
    case 1:
      CHECK(testkit::lastReset() == testkit::Reset::Software);
      glue::run([] {
        IWatchdog.begin(1000);
        fake::advanceMs(2);
      });
      FAIL("glue::run returned after a watchdog reset");
      break;
    default:
      CHECK(testkit::lastReset() == testkit::Reset::Watchdog);
      CHECK(testkit::boot() == 2);
  }
}

// ---------------------------------------------------------------- valve sim

TEST_CASE("sim: the enable pins and the MUX select each of the 12 valves") {
  glue::begin();
  sim::Rig rig;
  rig.install();
  const uint32_t ena[6] = {CTRL_ENA0, CTRL_ENA1, CTRL_ENA2, CTRL_ENA3, CTRL_ENA4, CTRL_ENA5};
  for (uint32_t pin : ena) pinMode(pin, OUTPUT);
  pinMode(CTRL_MUX, OUTPUT);
  CHECK(rig.enabledValve() == -1);
  for (int v = 0; v < ACTUATOR_COUNT; v++) {
    for (uint32_t pin : ena) digitalWrite(pin, LOW);
    if (v % 2) {
      MUX_OFF();
    } else {
      MUX_ON();
    }
    digitalWrite(ena[v / 2], HIGH);
    CHECK(rig.enabledValve() == v);
  }
}

TEST_CASE("sim: a powered, enabled valve turns one pulse per 1/pulsesPerMs ms, pulses equal the position change") {
  glue::begin();
  g_extiCalls = 0;
  sim::Rig rig;
  rig.install();
  analogReadResolution(12);
  attachInterrupt(digitalPinToInterrupt(REVINPIN), countExti, RISING);
  pinMode(POWER_ENA, OUTPUT_OPEN_DRAIN);
  PSU_ON();
  pinMode(CTRL_MUX, OUTPUT);
  pinMode(CTRL_DIRECTION, OUTPUT);
  pinMode(CTRL_ENA1, OUTPUT);
  MUX_OFF();  // odd valve: 3
  DIR_OFF();  // opening
  digitalWrite(CTRL_ENA1, HIGH);
  rig.runMs(100);
  CHECK(rig.valve[3].position == 1820);
  CHECK(rig.valve[3].pulses == 20);
  CHECK(g_extiCalls == 20);
  CHECK(rig.valve[3].enables == 1);
  CHECK(rig.current_dmA() == 250);
  // 250 dmA -> 181 ADC counts above the reference -> 249 dmA in TimerHandler0's conversion
  CHECK(analogRead(ANINCURRENT) - analogRead(ANINREFHALF) == 181);
  DIR_ON();  // closing: the sign of the current follows the direction
  rig.runMs(10);
  CHECK(rig.valve[3].position == 1818);
  CHECK(rig.current_dmA() == -250);
  PSU_OFF();
  rig.runMs(10);
  CHECK(rig.valve[3].position == 1818);
  CHECK(rig.current_dmA() == 0);
}

TEST_CASE("sim: end stop, obstacle, open circuit, short, inrush and coast") {
  glue::begin();
  sim::Rig rig;
  rig.install();
  pinMode(POWER_ENA, OUTPUT_OPEN_DRAIN);
  PSU_ON();
  pinMode(CTRL_MUX, OUTPUT);
  pinMode(CTRL_DIRECTION, OUTPUT);
  pinMode(CTRL_ENA0, OUTPUT);
  MUX_ON();  // valve 0
  DIR_OFF();
  sim::Valve& v = rig.valve[0];
  v.position = 3598;
  v.inrushPeak_dmA = 900;
  v.inrushMs = 2;
  digitalWrite(CTRL_ENA0, HIGH);
  rig.runMs(2);
  CHECK(rig.current_dmA() == 900);
  rig.runMs(20);
  CHECK(v.position == 3600);  // stops at the end stop
  CHECK(rig.current_dmA() == 700);
  digitalWrite(CTRL_ENA0, LOW);
  v.position = 100;
  v.jamFrom = 105;
  v.jamTo = 110;
  v.coastPulses = 3;
  digitalWrite(CTRL_ENA0, HIGH);
  rig.runMs(40);
  CHECK(v.position == 105);
  CHECK(rig.current_dmA() == 700);
  DIR_ON();
  v.position = 120;
  rig.runMs(50);
  CHECK(v.position == 110);
  DIR_OFF();
  v.jamFrom = -1;
  v.position = 200;
  rig.runMs(5);
  CHECK(v.position == 201);
  digitalWrite(CTRL_ENA0, LOW);
  rig.runMs(20);
  CHECK(v.position == 204);  // 3 coast pulses after the enable went off
  CHECK(rig.current_dmA() == 0);
  v.connected = false;
  digitalWrite(CTRL_ENA0, HIGH);
  rig.runMs(10);
  CHECK(v.position == 204);
  CHECK(rig.current_dmA() == 0);
  v.connected = true;
  v.shorted = true;
  rig.runMs(1);
  CHECK(rig.current_dmA() == 2000);
  CHECK(v.position == 204);
}

TEST_CASE("sim: runs valve_loop() every 10 ms while no timer does") {
  glue::begin();
  sim::Rig rig;
  rig.install();
  rig.runMs(30);
  CHECK(stub::callsOf("valve_loop") == stub::Calls(3, "valve_loop()"));
  STM32Timer tim2(TIM2);
  tim2.attachInterruptInterval(10000, valve_loop);
  stub::calls.clear();
  rig.runMs(30);
  CHECK(stub::callsOf("valve_loop").size() == 3);  // the timer's calls only
}
