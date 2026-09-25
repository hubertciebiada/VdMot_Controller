// Fake board of the STM glue tests: state and behaviour of the framework fakes in fakes/include.
#include "fake_board.h"

#include <algorithm>

#include "vdm/onewire_check.h"

RCC_TypeDef fake_RCC = {{0, 0}};
TIM_TypeDef fake_TIM1 = {1}, fake_TIM2 = {2}, fake_TIM3 = {3};
USART_TypeDef fake_USART1 = {1}, fake_USART2 = {2}, fake_USART6 = {6};

HardwareSerial Serial1(USART1);
HardwareSerial Serial2(USART2);
IWatchdogClass IWatchdog;
TwoWire Wire;

namespace fake {

Board board;
Eeprom eeprom;
OneWireBus oneWireBus;

namespace {

// Glue objects register at static initialization, so the registries are function-local statics.
std::vector<HardwareSerial*>& ports() {
  static std::vector<HardwareSerial*> list;
  return list;
}

std::vector<STM32TimerInterrupt*>& timerList() {
  static std::vector<STM32TimerInterrupt*> list;
  return list;
}

void record(Ev kind, uint32_t pin, uint32_t value) {
  if (board.events.size() >= board.eventLimit) {
    board.eventsDropped++;
    return;
  }
  board.events.push_back({++board.seq, kind, pin, value, board.nowUs});
}

bool validPin(uint32_t pin) { return pin < kPins; }

int inputLevel(uint32_t pin) {
  if (board.input) {
    const int level = board.input(pin);
    if (level >= 0) return level != 0;
  }
  return board.in[pin] != 0;
}

void millisecondTick() {
  if (board.onMs) board.onMs();
  // TIM1 before TIM2: the list is sorted by timer
  for (STM32TimerInterrupt* t : timerList()) {
    if (t->callback == nullptr || t->intervalUs == 0) continue;
    if (board.nowUs - t->lastFireUs < t->intervalUs) continue;
    t->lastFireUs = board.nowUs;
    t->fires++;
    t->callback();
  }
  if (board.afterMs) board.afterMs();
}

void checkWatchdog() {
  if (!IWatchdog.enabled || board.nowUs - IWatchdog.lastReloadUs < IWatchdog.timeoutUs) return;
  IWatchdog.enabled = false;  // the reset stops it until the next begin()
  throw WatchdogReset{};
}

bool failing(unsigned n, unsigned from, unsigned count) {
  return from != 0 && n >= from && (count == 0 || n < from + count);
}

}  // namespace

void reset() {
  board = Board();
  // pull-ups of the I2C lines on the controller board (hardware.h I2C_SCL_PIN, I2C_SDA_PIN)
  board.in[PB6] = 1;
  board.in[PB7] = 1;
  for (HardwareSerial* p : ports()) p->fakeReset();
  for (STM32TimerInterrupt* t : timerList()) {
    t->intervalUs = 0;
    t->callback = nullptr;
    t->failAttach = false;
    t->attaches = 0;
    t->lastFireUs = 0;
    t->fires = 0;
  }
  IWatchdog = IWatchdogClass();
  Wire = TwoWire();
  eeprom.ops.clear();
  eeprom.writes = 0;
  eeprom.reads = 0;
  eeprom.failWritesFrom = 0;
  eeprom.failWritesCount = 0;
  eeprom.failReadsFrom = 0;
  eeprom.failReadsCount = 0;
  eeprom.writeError = 2;
  eeprom.usPerTransfer = 0;
  oneWireBus.devices.clear();
}

void powerOn() {
  reset();
  fake_RCC.CSR.value = RCC_CSR_PORRSTF | RCC_CSR_PINRSTF | RCC_CSR_BORRSTF;
}

void advanceUs(uint64_t us) {
  static int depth = 0;
  const uint64_t target = board.nowUs + us;
  // an interrupt handler that waits: no nested interrupts, the outer loop continues after it
  if (depth > 0) {
    board.nowUs = target;
    return;
  }
  struct Depth {
    Depth() { ++depth; }
    ~Depth() { --depth; }
  } guard;
  for (;;) {
    const uint64_t next = (board.nowUs / 1000 + 1) * 1000;
    if (next > target) break;
    board.nowUs = next;
    if (board.primask == 0) millisecondTick();
    checkWatchdog();
  }
  if (board.nowUs < target) board.nowUs = target;
  checkWatchdog();
}

void advanceMs(uint32_t ms) { advanceUs(static_cast<uint64_t>(ms) * 1000); }

std::vector<Event> eventsOf(Ev kind, uint32_t pin) {
  std::vector<Event> out;
  for (const Event& e : board.events) {
    if (e.kind == kind && (pin == kAnyPin || e.pin == pin)) out.push_back(e);
  }
  return out;
}

void fireExti(uint32_t pin) {
  if (!validPin(pin) || board.primask != 0 || board.exti[pin] == nullptr) return;
  if (board.extiMode[pin] == RISING || board.extiMode[pin] == CHANGE) board.exti[pin]();
}

std::vector<STM32TimerInterrupt*> timers() { return timerList(); }

STM32TimerInterrupt* timerOf(TIM_TypeDef* tim) {
  for (STM32TimerInterrupt* t : timerList()) {
    if (t->timer == tim) return t;
  }
  return nullptr;
}

void inject(HardwareSerial& port, const std::string& bytes) {
  for (char c : bytes) port.fakeReceive(static_cast<uint8_t>(c));
}

void uartError(HardwareSerial& port, uint32_t halError) { port.nextError = halError; }

std::string tx(HardwareSerial& port) { return std::string(port.txLog, port.txLength); }

std::string takeTx(HardwareSerial& port) {
  std::string out = tx(port);
  port.txLength = 0;
  return out;
}

void eraseEeprom() { memset(eeprom.bytes, 0xFF, sizeof eeprom.bytes); }

OneWireDevice& addOneWire(uint8_t family, uint8_t serial, int16_t raw) {
  OneWireDevice d;
  const uint8_t rom[8] = {family, serial, 0, 0, 0, 0, 0, 0};
  memcpy(d.rom, rom, sizeof rom);
  d.rom[7] = vdm::crc8(d.rom, 7);
  d.raw = raw;
  oneWireBus.devices.push_back(d);
  return oneWireBus.devices.back();
}

OneWireDevice* findOneWire(const uint8_t* rom) {
  for (OneWireDevice& d : oneWireBus.devices) {
    if (d.present && memcmp(d.rom, rom, 8) == 0) return &d;
  }
  return nullptr;
}

}  // namespace fake

using fake::board;

// ---------------------------------------------------------------- CMSIS / HAL
uint32_t __get_PRIMASK(void) { return board.primask; }

void __set_PRIMASK(uint32_t priMask) { board.primask = priMask & 1; }

void __disable_irq(void) {
  board.primask = 1;
  board.irqDisables++;
}

void __enable_irq(void) { board.primask = 0; }

void HAL_NVIC_SystemReset(void) { throw fake::SystemReset{}; }

uint32_t HAL_GetDEVID(void) { return board.devId; }

// ---------------------------------------------------------------- Arduino core
GPIO_TypeDef* set_GPIO_Port_Clock(uint32_t port) {
  if (port < 32) board.portClocks |= 1UL << port;
  return nullptr;
}

uint32_t millis(void) {
  const uint32_t ms = static_cast<uint32_t>(board.nowUs / 1000);
  if (board.autoAdvanceUs) fake::advanceUs(board.autoAdvanceUs);
  return ms;
}

uint32_t micros(void) {
  const uint32_t us = static_cast<uint32_t>(board.nowUs);
  if (board.autoAdvanceUs) fake::advanceUs(board.autoAdvanceUs);
  return us;
}

void delay(uint32_t ms) {
  fake::record(fake::Ev::Delay, 0, ms);
  fake::advanceUs(static_cast<uint64_t>(ms) * 1000);
}

void delayMicroseconds(uint32_t us) {
  fake::record(fake::Ev::DelayUs, 0, us);
  fake::advanceUs(us);
}

void pinMode(uint32_t pin, uint32_t mode) {
  if (!fake::validPin(pin)) return;
  board.mode[pin] = mode;
  fake::record(fake::Ev::Mode, pin, mode);
}

void digitalWrite(uint32_t pin, uint32_t value) {
  if (!fake::validPin(pin)) return;
  board.out[pin] = value ? 1 : 0;
  fake::record(fake::Ev::Write, pin, value ? 1 : 0);
}

int digitalRead(uint32_t pin) {
  if (!fake::validPin(pin)) return LOW;
  if (board.mode[pin] == OUTPUT) return board.out[pin];
  // open drain: driven low by a 0 in the latch, otherwise the level of the line
  if (board.mode[pin] == OUTPUT_OPEN_DRAIN && board.out[pin] == 0) return LOW;
  return fake::inputLevel(pin);
}

uint32_t analogRead(uint32_t pin) {
  if (!fake::validPin(pin)) return 0;
  uint32_t raw = board.analog ? board.analog(pin) : board.analogValue[pin];
  if (raw > 4095) raw = 4095;
  // the ADC converts with 12 bits, analogRead() maps to the configured resolution
  if (board.analogBits < 12) return raw >> (12 - board.analogBits);
  return raw << (board.analogBits - 12);
}

void analogReadResolution(int bits) {
  if (bits > 0 && bits <= 16) board.analogBits = bits;
}

void attachInterrupt(uint32_t pin, void (*callback)(void), uint32_t mode) {
  if (!fake::validPin(pin)) return;
  board.exti[pin] = callback;
  board.extiMode[pin] = mode;
  fake::record(fake::Ev::Attach, pin, mode);
}

void detachInterrupt(uint32_t pin) {
  if (!fake::validPin(pin)) return;
  board.exti[pin] = nullptr;
  fake::record(fake::Ev::Detach, pin, 0);
}

// ---------------------------------------------------------------- Stream / HardwareSerial
size_t Stream::readBytes(char* buffer, size_t length) {
  size_t n = 0;
  while (n < length) {
    const int c = read();
    if (c < 0) {
      fake::advanceMs(static_cast<uint32_t>(timeoutMs_));
      break;
    }
    buffer[n++] = static_cast<char>(c);
  }
  return n;
}

HardwareSerial::HardwareSerial(void* p) : peripheral(p) {
  fake::ports().push_back(this);
  fakeReset();
}

HardwareSerial::~HardwareSerial() {
  std::vector<HardwareSerial*>& list = fake::ports();
  list.erase(std::remove(list.begin(), list.end(), this), list.end());
}

void HardwareSerial::fakeReset() {
  baud = 0;
  config = 0;
  rxPin = 0xFFFFFFFF;
  txPin = 0xFFFFFFFF;
  begins = 0;
  ends = 0;
  flushes = 0;
  readCalls = 0;
  lostBytes = 0;
  txLength = 0;
  txDropped = 0;
  nextError = HAL_UART_ERROR_NONE;
  ready_ = false;
  memset(&serial_, 0, sizeof serial_);
  serial_.uart = static_cast<USART_TypeDef*>(peripheral);
  serial_.handle.Instance = serial_.uart;
  serial_.rx_buff = rxBuffer_;
}

void HardwareSerial::begin(unsigned long baudRate, uint8_t frame) {
  baud = baudRate;
  config = frame;
  begins++;
  ready_ = true;
  serial_.rx_callback = _rx_complete_irq;
}

void HardwareSerial::end() {
  ends++;
  ready_ = false;
  serial_.rx_callback = nullptr;
  serial_.rx_head = serial_.rx_tail;
}

int HardwareSerial::available() {
  return static_cast<int>((static_cast<unsigned>(SERIAL_RX_BUFFER_SIZE) + serial_.rx_head - serial_.rx_tail) %
                          SERIAL_RX_BUFFER_SIZE);
}

int HardwareSerial::peek() {
  if (serial_.rx_head == serial_.rx_tail) return -1;
  return serial_.rx_buff[serial_.rx_tail];
}

int HardwareSerial::read() {
  readCalls++;
  if (serial_.rx_head == serial_.rx_tail) return -1;
  const unsigned char c = serial_.rx_buff[serial_.rx_tail];
  serial_.rx_tail = static_cast<uint16_t>((serial_.rx_tail + 1) % SERIAL_RX_BUFFER_SIZE);
  return c;
}

int HardwareSerial::availableForWrite() { return SERIAL_TX_BUFFER_SIZE - 1; }

void HardwareSerial::flush() { flushes++; }

size_t HardwareSerial::write(uint8_t c) {
  if (txLength < kTxLogSize) {
    txLog[txLength++] = static_cast<char>(c);
  } else {
    txDropped++;
  }
  return 1;
}

size_t HardwareSerial::write(const uint8_t* buffer, size_t size) {
  for (size_t i = 0; i < size; i++) write(buffer[i]);
  return size;
}

void HardwareSerial::_rx_complete_irq(serial_t* obj) {
  const uint8_t c = obj->recv;
  const uint16_t i = static_cast<uint16_t>((obj->rx_head + 1u) % SERIAL_RX_BUFFER_SIZE);
  // the ring keeps one slot free: a byte that would make head reach tail is dropped
  if (i != obj->rx_tail) {
    obj->rx_buff[obj->rx_head] = c;
    obj->rx_head = i;
  }
}

void HardwareSerial::fakeReceive(uint8_t byte) {
  if (!ready_ || serial_.rx_callback == nullptr) {
    lostBytes++;
    return;
  }
  serial_.recv = byte;
  serial_.handle.ErrorCode = nextError;
  nextError = HAL_UART_ERROR_NONE;
  serial_.rx_callback(&serial_);
  // HAL_UART_ErrorCallback of the core clears the error after the byte
  serial_.handle.ErrorCode = HAL_UART_ERROR_NONE;
}

// ---------------------------------------------------------------- Wire, IWatchdog, timers
void TwoWire::begin() {
  begins++;
  running = true;
  fake::record(fake::Ev::WireBegin, sda, scl);
}

void TwoWire::end() {
  ends++;
  running = false;
  fake::record(fake::Ev::WireEnd, sda, scl);
}

void IWatchdogClass::begin(uint32_t timeout, uint32_t window) {
  (void)window;
  if (timeout < IWDG_TIMEOUT_MIN || timeout > IWDG_TIMEOUT_MAX) return;
  begins++;
  enabled = true;
  timeoutUs = timeout;
  lastReloadMs = static_cast<uint32_t>(board.nowUs / 1000);
  lastReloadUs = board.nowUs;
  fake::record(fake::Ev::WatchdogBegin, 0, timeout);
}

void IWatchdogClass::set(uint32_t timeout, uint32_t window) {
  (void)window;
  if (timeout < IWDG_TIMEOUT_MIN || timeout > IWDG_TIMEOUT_MAX) return;
  timeoutUs = timeout;
}

void IWatchdogClass::reload() {
  if (!enabled) return;
  reloads++;
  lastReloadMs = static_cast<uint32_t>(board.nowUs / 1000);
  lastReloadUs = board.nowUs;
  fake::record(fake::Ev::Reload, 0, 0);
}

bool IWatchdogClass::isReset(bool clear) {
  const bool status = (RCC->CSR & RCC_CSR_IWDGRSTF) != 0;
  if (status && clear) clearReset();
  return status;
}

void IWatchdogClass::clearReset() { RCC->CSR |= RCC_CSR_RMVF; }

STM32TimerInterrupt::STM32TimerInterrupt(TIM_TypeDef* t) : timer(t) {
  std::vector<STM32TimerInterrupt*>& list = fake::timerList();
  const auto at = std::find_if(list.begin(), list.end(),
                               [t](const STM32TimerInterrupt* other) { return other->timer->index > t->index; });
  list.insert(at, this);
}

STM32TimerInterrupt::~STM32TimerInterrupt() {
  std::vector<STM32TimerInterrupt*>& list = fake::timerList();
  list.erase(std::remove(list.begin(), list.end(), this), list.end());
}

bool STM32TimerInterrupt::attachInterruptInterval(unsigned long interval, timerCallback cb) {
  attaches++;
  if (failAttach) return false;
  intervalUs = interval;
  callback = cb;
  lastFireUs = board.nowUs;
  return true;
}

// ---------------------------------------------------------------- I2C_eeprom (24LC64)
I2C_eeprom::I2C_eeprom(const uint8_t address, TwoWire* wire)
    : deviceAddress(address), deviceSize(fake::kEepromSize), bus(wire) {}

I2C_eeprom::I2C_eeprom(const uint8_t address, const uint32_t size, TwoWire* wire)
    : deviceAddress(address), deviceSize(size), bus(wire) {}

bool I2C_eeprom::begin(int8_t writeProtectPin) {
  (void)writeProtectPin;
  return true;
}

bool I2C_eeprom::isConnected() { return true; }

int I2C_eeprom::writeBlock(const uint16_t memoryAddress, const uint8_t* buffer, const uint16_t length) {
  fake::Eeprom& e = fake::eeprom;
  e.writes++;
  const bool fail = fake::failing(e.writes, e.failWritesFrom, e.failWritesCount);
  e.ops.push_back({true, memoryAddress, length, !fail});
  if (e.usPerTransfer) fake::advanceUs(e.usPerTransfer);
  if (fail) return e.writeError;
  // the 24LC64 decodes 13 address bits
  for (uint16_t i = 0; i < length; i++) e.bytes[(memoryAddress + i) % fake::kEepromSize] = buffer[i];
  return 0;
}

uint16_t I2C_eeprom::readBlock(const uint16_t memoryAddress, uint8_t* buffer, const uint16_t length) {
  fake::Eeprom& e = fake::eeprom;
  e.reads++;
  const bool fail = fake::failing(e.reads, e.failReadsFrom, e.failReadsCount);
  e.ops.push_back({false, memoryAddress, length, !fail});
  if (e.usPerTransfer) fake::advanceUs(e.usPerTransfer);
  if (fail) return 0;
  for (uint16_t i = 0; i < length; i++) buffer[i] = e.bytes[(memoryAddress + i) % fake::kEepromSize];
  return length;
}

int I2C_eeprom::writeByte(const uint16_t memoryAddress, const uint8_t value) {
  return writeBlock(memoryAddress, &value, 1);
}

uint8_t I2C_eeprom::readByte(const uint16_t memoryAddress) {
  uint8_t value = 0;
  readBlock(memoryAddress, &value, 1);
  return value;
}

// ---------------------------------------------------------------- 1-Wire
uint8_t OneWire::reset() {
  for (const fake::OneWireDevice& d : fake::oneWireBus.devices) {
    if (d.present) return 1;
  }
  return 0;
}

void OneWire::select(const uint8_t rom[8]) { memcpy(selected, rom, sizeof selected); }

bool OneWire::search(uint8_t* newAddr, bool searchMode) {
  (void)searchMode;
  searches++;
  while (searchPos < fake::oneWireBus.devices.size()) {
    const fake::OneWireDevice& d = fake::oneWireBus.devices[searchPos++];
    if (!d.present) continue;
    memcpy(newAddr, d.rom, 8);
    return true;
  }
  return false;
}

uint8_t OneWire::crc8(const uint8_t* addr, uint8_t len) { return vdm::crc8(addr, len); }

void DallasTemperature::begin(void) {
  begins++;
  devices = 0;
  DeviceAddress address;
  wire->reset_search();
  while (wire->search(address)) {
    if (!validAddress(address)) continue;
    devices++;
    if (!validFamily(address)) continue;
    const fake::OneWireDevice* d = fake::findOneWire(address);
    if (d != nullptr && d->resolution > bitResolution) bitResolution = d->resolution;
  }
}

bool DallasTemperature::validAddress(const uint8_t* address) {
  return OneWire::crc8(address, 7) == address[7];
}

bool DallasTemperature::validFamily(const uint8_t* address) {
  switch (address[0]) {
    case 0x10:  // DS18S20
    case 0x28:  // DS18B20
    case 0x22:  // DS1822
    case 0x3B:  // DS1825
    case 0x42:  // DS28EA00
      return true;
    default:
      return false;
  }
}

void DallasTemperature::requestTemperatures(void) {
  requests++;
  if (waitForConversion) fake::advanceMs(millisToWaitForConversion(bitResolution));
}

int16_t DallasTemperature::getTemp(const uint8_t* address) {
  reads++;
  fake::OneWireDevice* d = fake::findOneWire(address);
  if (d == nullptr) return DEVICE_DISCONNECTED_RAW;
  if (d->failReads > 0) {
    d->failReads--;
    return DEVICE_DISCONNECTED_RAW;
  }
  return d->raw;
}

float DallasTemperature::getTempC(const uint8_t* address) {
  const int16_t raw = getTemp(address);
  if (raw <= DEVICE_DISCONNECTED_RAW) return DEVICE_DISCONNECTED_C;
  return static_cast<float>(raw) * 0.0078125f;
}

uint16_t DallasTemperature::millisToWaitForConversion(uint8_t resolution) {
  switch (resolution) {
    case 9:
      return 94;
    case 10:
      return 188;
    case 11:
      return 375;
    default:
      return 750;
  }
}
