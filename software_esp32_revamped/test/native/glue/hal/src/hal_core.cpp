// Fake framework core: journal, clocks (monotonic and wall, gettimeofday()/time() interposed),
// GPIO, UARTs, chip, task watchdog, FreeRTOS, String/IPAddress/Print helpers.
#include <stdio.h>
#include <string.h>
#include <sys/time.h>
#include <time.h>

#include <string>
#include <vector>

#include "Arduino.h"
#include "Esp.h"
#include "HardwareSerial.h"
#include "IPAddress.h"
#include "esp_heap_caps.h"
#include "esp_mac.h"
#include "esp_system.h"
#include "esp_task_wdt.h"
#include "esp_timer.h"
#include "fakes/fakes.h"
#include "fakes/http.h"
#include "freertos/FreeRTOS.h"
#include "hal_internal.h"

namespace fakes {

namespace {

std::vector<std::string> g_journal;
uint64_t g_us = 0;
uint32_t g_ms32 = 0;
bool g_wallSet = false;
int64_t g_wallBaseUs = 0;
uint64_t g_wallBaseMono = 0;
Gpio g_gpio;
SerialPort g_serial[3];
std::string g_serialLine[3];
Esp g_esp;
Rtos g_rtos;

void syncMs32() { g_ms32 = static_cast<uint32_t>(g_us / 1000); }

// One yield of the calling task (vTaskDelay, delay): the clock advances, the yield budget of
// stopAfterYields() shrinks.
void yieldTicks(uint32_t ticks) {
  if (g_rtos.criticalDepth > 0) g_rtos.violation("task delay inside a critical section");
  if (g_rtos.delays.size() < g_rtos.delayStoreLimit) g_rtos.delays.push_back(ticks);
  ++g_rtos.delayCount;
  advanceMs(ticks);
  if (g_rtos.onDelay) g_rtos.onDelay(ticks);
  if (g_rtos.yieldsLeft > 0 && --g_rtos.yieldsLeft == 0) throw YieldLimit{};
}

const char* modeName(uint8_t mode) {
  switch (mode) {
    case INPUT: return "INPUT";
    case OUTPUT: return "OUTPUT";
    case INPUT_PULLUP: return "INPUT_PULLUP";
    case INPUT_PULLDOWN: return "INPUT_PULLDOWN";
    case OUTPUT_OPEN_DRAIN: return "OUTPUT_OPEN_DRAIN";
    default: return "?";
  }
}

// Bytes of port `n` that arrived by now; excess over the RX buffer is dropped (ring overflow).
size_t dueBytes(SerialPort& p) {
  if (p.peer != nullptr) p.peer->poll();
  const uint64_t now = nowMs();
  size_t due = 0;
  for (auto it = p.rx.begin(); it != p.rx.end();) {
    if (it->first > now) break;
    if (due >= p.rxBufferSize) {
      it = p.rx.erase(it);
      ++p.rxDropped;
      continue;
    }
    ++due;
    ++it;
  }
  return due;
}

}  // namespace

// ---------------------------------------------------------------- journal

std::vector<std::string>& journal() { return g_journal; }

void note(const std::string& entry) { g_journal.push_back(entry); }

int find(const std::string& entry, int from) {
  for (size_t i = from < 0 ? 0 : static_cast<size_t>(from); i < g_journal.size(); ++i) {
    if (g_journal[i] == entry) return static_cast<int>(i);
  }
  return -1;
}

std::vector<std::string> journalOf(const std::string& prefix) {
  std::vector<std::string> out;
  for (const std::string& e : g_journal) {
    if (e.compare(0, prefix.size(), prefix) == 0) out.push_back(e);
  }
  return out;
}

// ---------------------------------------------------------------- time

uint64_t nowMs() { return g_us / 1000; }
uint64_t nowUs() { return g_us; }

void setMs(uint64_t ms) {
  g_us = ms * 1000;
  syncMs32();
}

void advanceMs(uint64_t ms) {
  g_us += ms * 1000;
  syncMs32();
}

void advanceUs(uint64_t us) {
  g_us += us;
  syncMs32();
}

const uint32_t& ms32() { return g_ms32; }

void setWallClock(int64_t epoch, int64_t usec) {
  g_wallSet = true;
  g_wallBaseUs = epoch * 1000000 + usec;
  g_wallBaseMono = g_us;
}

int64_t wallClockUs() {
  if (!g_wallSet) return static_cast<int64_t>(g_us);
  return g_wallBaseUs + static_cast<int64_t>(g_us - g_wallBaseMono);
}

// ---------------------------------------------------------------- state accessors

Gpio& gpio() { return g_gpio; }
SerialPort& serial(int n) { return g_serial[n < 0 || n > 2 ? 0 : n]; }
Esp& esp() { return g_esp; }
Rtos& rtos() { return g_rtos; }

void Rtos::violation(const std::string& what) {
  ++violations;
  violationLog.push_back(what);
}

void SerialPort::inject(const std::string& bytes, uint64_t atMs) {
  const uint64_t at = atMs == UINT64_MAX ? nowMs() : atMs;
  for (char c : bytes) rx.emplace_back(at, static_cast<uint8_t>(c));
}

std::string SerialPort::takeTx() {
  std::string out;
  out.swap(tx);
  return out;
}

void reset() {
  g_journal.clear();
  g_us = 0;
  syncMs32();
  g_wallSet = false;
  g_gpio = Gpio{};
  for (int i = 0; i < 3; ++i) {
    g_serial[i] = SerialPort{};
    g_serialLine[i].clear();
  }
  const esp_reset_reason_t reason = g_esp.resetReason;
  g_esp = Esp{};
  g_esp.resetReason = reason;
  g_rtos = Rtos{};
  resetFsVolatile();
  resetNvsVolatile();
  resetOtaVolatile();
  resetNetVolatile();
  resetMqttVolatile();
  resetWebVolatile();
}

}  // namespace fakes

// ---------------------------------------------------------------- libc time (interposed)

// glibc declares tv __nonnull((1)).
extern "C" int gettimeofday(struct timeval* tv, void* tz) __THROW {
  (void)tz;
  const int64_t us = fakes::wallClockUs();
  tv->tv_sec = static_cast<time_t>(us / 1000000);
  tv->tv_usec = static_cast<suseconds_t>(us % 1000000);
  return 0;
}

extern "C" time_t time(time_t* t) __THROW {
  const time_t now = static_cast<time_t>(fakes::wallClockUs() / 1000000);
  if (t != nullptr) *t = now;
  return now;
}

// ---------------------------------------------------------------- Arduino core

void pinMode(uint8_t pin, uint8_t mode) {
  if (pin < fakes::kPins) fakes::gpio().mode[pin] = mode;
  fakes::note("pinMode " + std::to_string(pin) + "=" + fakes::modeName(mode));
}

void digitalWrite(uint8_t pin, uint8_t val) {
  const uint8_t level = val != 0 ? HIGH : LOW;
  fakes::Gpio& g = fakes::gpio();
  if (pin < fakes::kPins) g.level[pin] = level;
  g.writes.emplace_back(pin, level);
  fakes::note("gpio " + std::to_string(pin) + "=" + std::to_string(level));
  if (g.onWrite) g.onWrite(pin, level);
}

int digitalRead(uint8_t pin) {
  fakes::Gpio& g = fakes::gpio();
  if (pin >= fakes::kPins) return LOW;
  if (g.mode[pin] == OUTPUT) return g.level[pin];
  if (g.input) {
    const int v = g.input(pin, fakes::nowMs());
    if (v >= 0) return v != 0 ? HIGH : LOW;
  }
  return g.in[pin] != 0 ? HIGH : LOW;
}

unsigned long millis() { return static_cast<unsigned long>(fakes::ms32()); }

unsigned long micros() { return static_cast<unsigned long>(static_cast<uint32_t>(fakes::nowUs())); }

void delay(uint32_t ms) {
  fakes::note("delay " + std::to_string(ms));
  fakes::yieldTicks(ms);
}

void delayMicroseconds(uint32_t us) { fakes::advanceUs(us); }

void yield() {}

int64_t esp_timer_get_time(void) { return static_cast<int64_t>(fakes::nowUs()); }

// ---------------------------------------------------------------- UARTs

HardwareSerial Serial(0);
HardwareSerial Serial1(1);
HardwareSerial Serial2(2);

void HardwareSerial::begin(unsigned long baud, uint32_t config, int8_t rxPin, int8_t txPin, bool,
                           unsigned long, uint8_t) {
  fakes::SerialPort& p = fakes::serial(uartNr_);
  p.open = true;
  p.baud = static_cast<uint32_t>(baud);
  p.config = config;
  p.rxPin = rxPin;
  p.txPin = txPin;
  ++p.begins;
  char buf[96];
  snprintf(buf, sizeof buf, "serial%d.begin %lu 0x%x %d %d", uartNr_, baud,
           static_cast<unsigned>(config), rxPin, txPin);
  fakes::note(buf);
  if (p.peer != nullptr) p.peer->onBegin(p.baud, config);
}

void HardwareSerial::end(bool) {
  fakes::SerialPort& p = fakes::serial(uartNr_);
  if (p.open && p.peer != nullptr) p.peer->onEnd();
  p.open = false;
  p.rx.clear();  // the driver and its RX ring are gone
  ++p.ends;
  fakes::note("serial" + std::to_string(uartNr_) + ".end");
}

size_t HardwareSerial::setRxBufferSize(size_t newSize) {
  fakes::SerialPort& p = fakes::serial(uartNr_);
  if (p.open || newSize <= 128) return 0;
  p.rxBufferSize = newSize;
  return newSize;
}

size_t HardwareSerial::setTxBufferSize(size_t newSize) {
  fakes::SerialPort& p = fakes::serial(uartNr_);
  if (p.open || newSize <= 128) return 0;
  p.txBufferSize = newSize;
  return newSize;
}

int HardwareSerial::available() {
  fakes::SerialPort& p = fakes::serial(uartNr_);
  if (!p.open) return 0;
  return static_cast<int>(fakes::dueBytes(p));
}

int HardwareSerial::availableForWrite() {
  fakes::SerialPort& p = fakes::serial(uartNr_);
  return p.open ? static_cast<int>(p.txBufferSize > 0 ? p.txBufferSize : 128) : 0;
}

int HardwareSerial::peek() {
  fakes::SerialPort& p = fakes::serial(uartNr_);
  if (!p.open || fakes::dueBytes(p) == 0) return -1;
  return p.rx.front().second;
}

int HardwareSerial::read() {
  uint8_t c = 0;
  return read(&c, 1) == 1 ? c : -1;
}

size_t HardwareSerial::read(uint8_t* buffer, size_t size) {
  fakes::SerialPort& p = fakes::serial(uartNr_);
  if (!p.open) return 0;
  const size_t due = fakes::dueBytes(p);
  size_t n = 0;
  while (n < size && n < due) {
    buffer[n++] = p.rx.front().second;
    p.rx.pop_front();
  }
  return n;
}

size_t HardwareSerial::write(uint8_t c) { return write(&c, 1); }

size_t HardwareSerial::write(const uint8_t* buffer, size_t size) {
  fakes::SerialPort& p = fakes::serial(uartNr_);
  const size_t n = size < p.txAccept ? size : p.txAccept;
  std::string& line = fakes::g_serialLine[uartNr_ < 0 || uartNr_ > 2 ? 0 : uartNr_];
  for (size_t i = 0; i < n; ++i) {
    const char c = static_cast<char>(buffer[i]);
    p.tx.push_back(c);
    if (c == '\n') {
      p.lines.push_back(line);
      line.clear();
    } else if (c != '\r') {
      line.push_back(c);
    }
  }
  if (p.open && p.peer != nullptr && n > 0) p.peer->onTx(buffer, n);
  return n;
}

uint32_t HardwareSerial::baudRate() { return fakes::serial(uartNr_).baud; }

HardwareSerial::operator bool() const { return true; }

// ---------------------------------------------------------------- chip

EspClass ESP;

uint32_t EspClass::getHeapSize() { return fakes::esp().heapSize; }
uint32_t EspClass::getFreeHeap() { return fakes::esp().freeHeap; }
uint32_t EspClass::getMinFreeHeap() { return fakes::esp().minFreeHeap; }
uint32_t EspClass::getMaxAllocHeap() { return fakes::esp().maxAllocHeap; }
uint32_t EspClass::getSketchSize() { return fakes::esp().sketchSize; }
uint32_t EspClass::getFreeSketchSpace() { return fakes::esp().freeSketchSpace; }
const char* EspClass::getSdkVersion() { return "v4.4.4"; }

uint64_t EspClass::getEfuseMac() {
  uint64_t v = 0;
  for (int i = 5; i >= 0; --i) v = (v << 8) | fakes::esp().mac[i];
  return v;
}

void EspClass::restart() { esp_restart(); }

esp_reset_reason_t esp_reset_reason(void) { return fakes::esp().resetReason; }

void esp_restart(void) {
  ++fakes::esp().restarts;
  fakes::note("esp_restart");
  throw fakes::Restarted{};
}

uint32_t esp_get_free_heap_size(void) { return fakes::esp().freeHeap; }
uint32_t esp_get_minimum_free_heap_size(void) { return fakes::esp().minFreeHeap; }

esp_err_t esp_efuse_mac_get_default(uint8_t* mac) {
  if (mac == nullptr) return ESP_ERR_INVALID_ARG;
  memcpy(mac, fakes::esp().mac, 6);
  return ESP_OK;
}

esp_err_t esp_read_mac(uint8_t* mac, esp_mac_type_t type) {
  if (mac == nullptr) return ESP_ERR_INVALID_ARG;
  memcpy(mac, fakes::esp().mac, 6);
  mac[5] = static_cast<uint8_t>(mac[5] + static_cast<uint8_t>(type));
  return ESP_OK;
}

size_t heap_caps_get_free_size(uint32_t) { return fakes::esp().freeHeap; }
size_t heap_caps_get_minimum_free_size(uint32_t) { return fakes::esp().minFreeHeap; }
size_t heap_caps_get_largest_free_block(uint32_t) { return fakes::esp().maxAllocHeap; }

esp_err_t esp_task_wdt_init(uint32_t timeout, bool panic) {
  fakes::Esp& e = fakes::esp();
  ++e.wdtInits;
  e.wdtTimeoutS = timeout;
  e.wdtPanic = panic;
  fakes::note("esp_task_wdt_init " + std::to_string(timeout) + " " + (panic ? "1" : "0"));
  return e.wdtInitResult;
}

esp_err_t esp_task_wdt_add(TaskHandle_t handle) {
  fakes::Esp& e = fakes::esp();
  ++e.wdtAdds;
  e.wdtTasks.push_back(handle);
  fakes::note("esp_task_wdt_add");
  return ESP_OK;
}

esp_err_t esp_task_wdt_delete(TaskHandle_t handle) {
  std::vector<TaskHandle_t>& t = fakes::esp().wdtTasks;
  for (auto it = t.begin(); it != t.end(); ++it) {
    if (*it == handle) {
      t.erase(it);
      return ESP_OK;
    }
  }
  return ESP_ERR_INVALID_ARG;
}

esp_err_t esp_task_wdt_reset(void) {
  ++fakes::esp().wdtResets;
  return ESP_OK;
}

// ---------------------------------------------------------------- FreeRTOS: tasks

namespace {

constexpr uint32_t kQueueMagic = 0x51554555;  // "QUEU"
constexpr uint8_t kKindQueue = 1;
constexpr uint8_t kKindMutex = 2;
constexpr uint8_t kKindBinary = 3;

bool validQueue(QueueHandle_t q, const char* call) {
  if (q != nullptr && q->magic == kQueueMagic) return true;
  fakes::rtos().violation(std::string(call) + " on a queue or semaphore that was not created");
  return false;
}

// A wait of a call that cannot succeed in a single-threaded model: portMAX_DELAY would never
// return on the target (deadlock), a finite wait just passes.
void waitFor(TickType_t ticks, const char* call) {
  if (ticks == 0) return;
  if (fakes::rtos().criticalDepth > 0) {
    fakes::rtos().violation(std::string(call) + " blocks inside a critical section");
  }
  if (ticks == portMAX_DELAY) {
    fakes::rtos().violation(std::string(call) + " waits forever (nothing else can run)");
    return;
  }
  fakes::advanceMs(ticks);
}

}  // namespace

BaseType_t xTaskCreatePinnedToCore(TaskFunction_t pvTaskCode, const char* const pcName,
                                   const uint32_t usStackDepth, void* const pvParameters,
                                   UBaseType_t uxPriority, TaskHandle_t* const pvCreatedTask,
                                   const BaseType_t xCoreID) {
  fakes::Rtos& r = fakes::rtos();
  TaskHandle_t handle =
      reinterpret_cast<TaskHandle_t>(static_cast<uintptr_t>(0x1000 + r.tasks.size() * 0x10));
  r.tasks.push_back({pvTaskCode, pcName != nullptr ? pcName : "", usStackDepth, uxPriority,
                     xCoreID, pvParameters, handle});
  fakes::note(std::string("task ") + (pcName != nullptr ? pcName : ""));
  if (r.createResult != pdPASS) return r.createResult;
  if (pvCreatedTask != nullptr) *pvCreatedTask = handle;
  return pdPASS;
}

BaseType_t xTaskCreate(TaskFunction_t pvTaskCode, const char* const pcName,
                       const uint32_t usStackDepth, void* const pvParameters,
                       UBaseType_t uxPriority, TaskHandle_t* const pvCreatedTask) {
  return xTaskCreatePinnedToCore(pvTaskCode, pcName, usStackDepth, pvParameters, uxPriority,
                                 pvCreatedTask, tskNO_AFFINITY);
}

void vTaskDelay(const TickType_t xTicksToDelay) { fakes::yieldTicks(xTicksToDelay); }

void vTaskDelete(TaskHandle_t xTaskToDelete) {
  fakes::rtos().deleted.push_back(xTaskToDelete);
  fakes::note(xTaskToDelete == nullptr ? "vTaskDelete self" : "vTaskDelete");
  if (xTaskToDelete == nullptr) throw fakes::TaskDeleted{};
}

UBaseType_t uxTaskGetStackHighWaterMark(TaskHandle_t xTask) {
  fakes::Rtos& r = fakes::rtos();
  if (xTask == nullptr) xTask = r.current;
  for (const fakes::TaskRecord& t : r.tasks) {
    if (t.handle != xTask) continue;
    auto it = r.stackHighWater.find(t.name);
    return it != r.stackHighWater.end() ? it->second : 0;
  }
  for (const auto& e : r.extraHandles) {
    if (e.second != xTask) continue;
    auto it = r.stackHighWater.find(e.first);
    return it != r.stackHighWater.end() ? it->second : 0;
  }
  return 0;
}

TaskHandle_t xTaskGetHandle(const char* pcNameToQuery) {
  if (pcNameToQuery == nullptr) return nullptr;
  fakes::Rtos& r = fakes::rtos();
  for (const fakes::TaskRecord& t : r.tasks) {
    if (t.name == pcNameToQuery) return t.handle;
  }
  auto it = r.extraHandles.find(pcNameToQuery);
  return it != r.extraHandles.end() ? it->second : nullptr;
}

TaskHandle_t xTaskGetCurrentTaskHandle(void) { return fakes::rtos().current; }

TickType_t xTaskGetTickCount(void) { return static_cast<TickType_t>(fakes::nowMs()); }

// ---------------------------------------------------------------- FreeRTOS: queues

QueueHandle_t xQueueCreateStatic(UBaseType_t uxQueueLength, UBaseType_t uxItemSize,
                                 uint8_t* pucQueueStorageBuffer, StaticQueue_t* pxQueueBuffer) {
  if (pxQueueBuffer == nullptr || uxQueueLength == 0) return nullptr;
  *pxQueueBuffer = StaticQueue_t{};
  pxQueueBuffer->magic = kQueueMagic;
  pxQueueBuffer->kind = kKindQueue;
  pxQueueBuffer->storage = pucQueueStorageBuffer;
  pxQueueBuffer->length = uxQueueLength;
  pxQueueBuffer->itemSize = uxItemSize;
  return pxQueueBuffer;
}

QueueHandle_t xQueueCreate(UBaseType_t uxQueueLength, UBaseType_t uxItemSize) {
  StaticQueue_t* q = new StaticQueue_t{};
  uint8_t* storage = new uint8_t[uxQueueLength * uxItemSize + 1];
  xQueueCreateStatic(uxQueueLength, uxItemSize, storage, q);
  q->ownsStorage = 1;
  return q;
}

namespace {

BaseType_t queueSend(QueueHandle_t q, const void* item, TickType_t ticks, bool front,
                     const char* call) {
  if (!validQueue(q, call)) return pdFALSE;
  if (q->count >= q->length) {
    waitFor(ticks, call);
    return errQUEUE_FULL;
  }
  UBaseType_t slot;
  if (front) {
    q->head = (q->head + q->length - 1) % q->length;
    slot = q->head;
  } else {
    slot = (q->head + q->count) % q->length;
  }
  memcpy(q->storage + slot * q->itemSize, item, q->itemSize);
  ++q->count;
  return pdTRUE;
}

}  // namespace

BaseType_t xQueueSend(QueueHandle_t xQueue, const void* pvItemToQueue, TickType_t xTicksToWait) {
  return queueSend(xQueue, pvItemToQueue, xTicksToWait, false, "xQueueSend");
}

BaseType_t xQueueSendToBack(QueueHandle_t xQueue, const void* pvItemToQueue,
                            TickType_t xTicksToWait) {
  return queueSend(xQueue, pvItemToQueue, xTicksToWait, false, "xQueueSendToBack");
}

BaseType_t xQueueSendToFront(QueueHandle_t xQueue, const void* pvItemToQueue,
                             TickType_t xTicksToWait) {
  return queueSend(xQueue, pvItemToQueue, xTicksToWait, true, "xQueueSendToFront");
}

BaseType_t xQueueReceive(QueueHandle_t xQueue, void* pvBuffer, TickType_t xTicksToWait) {
  if (!validQueue(xQueue, "xQueueReceive")) return pdFALSE;
  if (xQueue->count == 0) {
    waitFor(xTicksToWait, "xQueueReceive");
    return pdFALSE;
  }
  memcpy(pvBuffer, xQueue->storage + xQueue->head * xQueue->itemSize, xQueue->itemSize);
  xQueue->head = (xQueue->head + 1) % xQueue->length;
  --xQueue->count;
  return pdTRUE;
}

BaseType_t xQueuePeek(QueueHandle_t xQueue, void* pvBuffer, TickType_t xTicksToWait) {
  if (!validQueue(xQueue, "xQueuePeek")) return pdFALSE;
  if (xQueue->count == 0) {
    waitFor(xTicksToWait, "xQueuePeek");
    return pdFALSE;
  }
  memcpy(pvBuffer, xQueue->storage + xQueue->head * xQueue->itemSize, xQueue->itemSize);
  return pdTRUE;
}

UBaseType_t uxQueueMessagesWaiting(QueueHandle_t xQueue) {
  return validQueue(xQueue, "uxQueueMessagesWaiting") ? xQueue->count : 0;
}

UBaseType_t uxQueueSpacesAvailable(QueueHandle_t xQueue) {
  return validQueue(xQueue, "uxQueueSpacesAvailable") ? xQueue->length - xQueue->count : 0;
}

BaseType_t xQueueReset(QueueHandle_t xQueue) {
  if (!validQueue(xQueue, "xQueueReset")) return pdFALSE;
  xQueue->head = 0;
  xQueue->count = 0;
  return pdPASS;
}

void vQueueDelete(QueueHandle_t xQueue) {
  if (!validQueue(xQueue, "vQueueDelete")) return;
  xQueue->magic = 0;
  if (xQueue->ownsStorage) {
    delete[] xQueue->storage;
    delete xQueue;
  }
}

// ---------------------------------------------------------------- FreeRTOS: semaphores

SemaphoreHandle_t xSemaphoreCreateMutexStatic(StaticSemaphore_t* pxMutexBuffer) {
  if (pxMutexBuffer == nullptr) return nullptr;
  *pxMutexBuffer = StaticSemaphore_t{};
  pxMutexBuffer->magic = kQueueMagic;
  pxMutexBuffer->kind = kKindMutex;
  pxMutexBuffer->length = 1;
  pxMutexBuffer->count = 1;  // available
  return pxMutexBuffer;
}

SemaphoreHandle_t xSemaphoreCreateMutex(void) {
  StaticSemaphore_t* s = new StaticSemaphore_t{};
  xSemaphoreCreateMutexStatic(s);
  s->ownsStorage = 1;
  return s;
}

SemaphoreHandle_t xSemaphoreCreateBinaryStatic(StaticSemaphore_t* pxSemaphoreBuffer) {
  if (pxSemaphoreBuffer == nullptr) return nullptr;
  *pxSemaphoreBuffer = StaticSemaphore_t{};
  pxSemaphoreBuffer->magic = kQueueMagic;
  pxSemaphoreBuffer->kind = kKindBinary;
  pxSemaphoreBuffer->length = 1;
  pxSemaphoreBuffer->count = 0;  // created empty, like FreeRTOS
  return pxSemaphoreBuffer;
}

SemaphoreHandle_t xSemaphoreCreateBinary(void) {
  StaticSemaphore_t* s = new StaticSemaphore_t{};
  xSemaphoreCreateBinaryStatic(s);
  s->ownsStorage = 1;
  return s;
}

BaseType_t xSemaphoreTake(SemaphoreHandle_t xSemaphore, TickType_t xBlockTime) {
  if (!validQueue(xSemaphore, "xSemaphoreTake")) return pdFALSE;
  if (xBlockTime != 0 && fakes::rtos().criticalDepth > 0) {
    fakes::rtos().violation("xSemaphoreTake blocks inside a critical section");
  }
  if (xSemaphore->count == 0) {
    if (xSemaphore->kind == kKindMutex) {
      fakes::rtos().violation("second xSemaphoreTake of a held mutex (deadlock)");
    } else {
      waitFor(xBlockTime, "xSemaphoreTake");
    }
    return pdFALSE;
  }
  xSemaphore->count = 0;
  return pdTRUE;
}

BaseType_t xSemaphoreGive(SemaphoreHandle_t xSemaphore) {
  if (!validQueue(xSemaphore, "xSemaphoreGive")) return pdFALSE;
  if (xSemaphore->count != 0) {
    if (xSemaphore->kind == kKindMutex) {
      fakes::rtos().violation("xSemaphoreGive of a mutex that is not held");
    }
    return pdFALSE;
  }
  xSemaphore->count = 1;
  return pdTRUE;
}

void vSemaphoreDelete(SemaphoreHandle_t xSemaphore) { vQueueDelete(xSemaphore); }

// ---------------------------------------------------------------- critical sections

void vPortEnterCritical(portMUX_TYPE* mux) {
  if (mux == nullptr) {
    fakes::rtos().violation("portENTER_CRITICAL without a mux");
    return;
  }
  mux->owner = 0;
  mux->count = mux->count + 1;
  ++fakes::rtos().criticalDepth;
}

void vPortExitCritical(portMUX_TYPE* mux) {
  if (mux == nullptr || mux->count == 0) {
    fakes::rtos().violation("portEXIT_CRITICAL of a section that was not entered");
    return;
  }
  mux->count = mux->count - 1;
  if (mux->count == 0) mux->owner = portMUX_FREE_VAL;
  --fakes::rtos().criticalDepth;
}

void vPortCPUInitializeMutex(portMUX_TYPE* mux) {
  mux->owner = portMUX_FREE_VAL;
  mux->count = 0;
}

// ---------------------------------------------------------------- String, IPAddress, Print

std::string String::fixed(double value, unsigned int decimals) {
  char buf[64];
  snprintf(buf, sizeof buf, "%.*f", static_cast<int>(decimals), value);
  return buf;
}

const IPAddress INADDR_NONE(0, 0, 0, 0);

bool IPAddress::fromString(const char* address) {
  if (address == nullptr) return false;
  unsigned v[4];
  char tail;
  if (sscanf(address, "%u.%u.%u.%u%c", &v[0], &v[1], &v[2], &v[3], &tail) != 4) return false;
  for (int i = 0; i < 4; ++i) {
    if (v[i] > 255) return false;
    address_.bytes[i] = static_cast<uint8_t>(v[i]);
  }
  return true;
}

String IPAddress::toString() const {
  char buf[16];
  snprintf(buf, sizeof buf, "%u.%u.%u.%u", address_.bytes[0], address_.bytes[1],
           address_.bytes[2], address_.bytes[3]);
  return String(buf);
}

size_t Print::write(const uint8_t* buffer, size_t size) {
  size_t n = 0;
  while (n < size && write(buffer[n]) == 1) ++n;
  return n;
}

size_t Print::printf(const char* format, ...) {
  char buf[256];
  va_list ap;
  va_start(ap, format);
  const int n = vsnprintf(buf, sizeof buf, format, ap);
  va_end(ap);
  if (n <= 0) return 0;
  return write(reinterpret_cast<const uint8_t*>(buf),
               static_cast<size_t>(n) < sizeof buf ? static_cast<size_t>(n) : sizeof buf - 1);
}

size_t Stream::readBytes(char* buffer, size_t length) {
  size_t n = 0;
  while (n < length) {
    const int c = read();
    if (c < 0) break;
    buffer[n++] = static_cast<char>(c);
  }
  return n;
}
