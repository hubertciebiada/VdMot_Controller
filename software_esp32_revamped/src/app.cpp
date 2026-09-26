#include "app.h"

#include <Arduino.h>
#include <esp_system.h>
#include <esp_task_wdt.h>
#include <esp_timer.h>
#include <freertos/FreeRTOS.h>
#include <freertos/queue.h>
#include <freertos/semphr.h>
#include <freertos/task.h>
#include <string.h>

#include <vdm/config.h>
#include <vdm/event_log.h>
#include <vdm/factory_reset.h>
#include <vdm/json_api.h>
#include <vdm/sys_health.h>

#include "board.h"
#include "boot_alloc.h"
#include "logger.h"
#include "mqtt_client.h"
#include "net.h"
#include "ota.h"
#include "stm_link.h"
#include "stm_service.h"
#include "storage.h"
#include "web_server.h"

namespace app {

namespace {

StaticQueue_t gQueueStorage;
uint8_t gQueueItems[kCommandQueueDepth * sizeof(Command)];
QueueHandle_t gQueue = nullptr;

StaticSemaphore_t gSnapMutexStorage;
SemaphoreHandle_t gSnapMutex = nullptr;
StmSnapshot& gSnapshot = bootAlloc<StmSnapshot>();
volatile vdm::LinkState gLinkState = vdm::LinkState::Unknown;
volatile bool gFlashActive = false;
volatile uint8_t gProto = 0;
volatile uint32_t gSnapRevision = 0;
volatile vdm::StmSupport gSupport = vdm::StmSupport::Unknown;
volatile vdm::StmSaveState gSaveState = vdm::StmSaveState::Idle;

portMUX_TYPE gCalibMux = portMUX_INITIALIZER_UNLOCKED;
CalibInfo gCalibInfo;

// App task working copies (static: too large for the task stack).
vdm::Config& gCfg = bootAlloc<vdm::Config>();
uint32_t gCfgRevision = 0;

// Tasks whose stack high-water mark is watched: ours, AsyncTCP's (created by
// the first AsyncServer::begin) and the Arduino event task (runs net's event
// handler); the library tasks are looked up by name until found.
struct MonitoredTask {
  const char* name;
  uint32_t stackBytes;
  TaskHandle_t handle;
};
constexpr size_t kMonitoredTasks = 5;
MonitoredTask gTasks[kMonitoredTasks] = {{kStmTask.name, kStmTask.stackBytes, nullptr},
                                         {kAppTask.name, kAppTask.stackBytes, nullptr},
                                         {kMqttTask.name, kMqttTask.stackBytes, nullptr},
                                         {"async_tcp", 16384, nullptr},
                                         {"arduino_events", 4096, nullptr}};
portMUX_TYPE gHealthMux = portMUX_INITIALIZER_UNLOCKED;  // gTasks handles, gMinLargest
uint32_t gMinLargest = 0;
vdm::ResourceMonitor gResources;  // app task
bool gFactoryLatched = false;     // app task after setup

// Factory reset pin at boot: the first sample, and (only when it is LOW and
// no latch is set) whether it stays LOW for kFactoryResetHoldMs.
vdm::FactoryPinDecision checkFactoryPin(bool latched) {
  pinMode(board::kFactoryResetPin, INPUT_PULLUP);
  delay(2);  // let the pull-up settle
  const bool low = digitalRead(board::kFactoryResetPin) == LOW;
  bool held = false;
  if (low && !latched) {
    vdm::PinHold hold(board::kFactoryResetHoldMs);
    hold.begin(millis());
    vdm::PinHold::State st = vdm::PinHold::State::Holding;
    while (st == vdm::PinHold::State::Holding) {
      delay(board::kFactoryResetSampleMs);
      st = hold.sample(digitalRead(board::kFactoryResetPin) == LOW, millis());
    }
    held = st == vdm::PinHold::State::Held;
  }
  return vdm::factoryPinAtBoot(low, held, latched);
}

void startTask(const TaskSpec& spec, TaskFunction_t fn, size_t slot) {
  TaskHandle_t handle = nullptr;
  if (xTaskCreatePinnedToCore(fn, spec.name, spec.stackBytes, nullptr, spec.priority, &handle,
                              spec.core) != pdPASS) {
    // Without its tasks the firmware cannot work; a reboot is the only
    // recovery (logged as panic reason on the next boot).
    abort();
  }
  portENTER_CRITICAL(&gHealthMux);
  gTasks[slot].handle = handle;
  portEXIT_CRITICAL(&gHealthMux);
}

// Live effects of a config change (DESIGN.md "Config schema", apply
// semantics). The stm and mqtt tasks follow configRevision() themselves.
void applyConfigChange() {
  gCfgRevision = storage::configRevision();
  storage::getConfig(gCfg);
  logger::configure(gCfg.syslog.level, gCfg.syslog.server, gCfg.syslog.port, gCfg.persistLog,
                    gCfg.station);
  net::reconfigure(gCfg);
}

// Every 10 s: heap, fragmentation and stack alarms (vdm::ResourceMonitor).
void sampleResources(uint32_t now) {
  vdm::Event ev[2];
  size_t n = gResources.onHeap(ESP.getFreeHeap(), ESP.getMinFreeHeap(), ESP.getMaxAllocHeap(),
                               now, ev, 2);
  for (size_t i = 0; i < n; ++i) logger::log(ev[i]);
  for (size_t i = 0; i < kMonitoredTasks; ++i) {
    MonitoredTask& t = gTasks[i];
    if (t.handle == nullptr) {
      TaskHandle_t h = xTaskGetHandle(t.name);
      if (h == nullptr) continue;
      portENTER_CRITICAL(&gHealthMux);
      t.handle = h;
      portEXIT_CRITICAL(&gHealthMux);
    }
    n = gResources.onStack(static_cast<uint8_t>(i), t.name, t.stackBytes,
                           uxTaskGetStackHighWaterMark(t.handle), ev, 1);
    if (n != 0) logger::log(ev[0]);
  }
  portENTER_CRITICAL(&gHealthMux);
  gMinLargest = gResources.minLargestBlock();
  portEXIT_CRITICAL(&gHealthMux);
}

// Every second while latched: the jumper was removed, so the next fitting
// resets again.
void checkFactoryLatch() {
  if (!vdm::factoryPinRuntimeClear(digitalRead(board::kFactoryResetPin) == LOW, gFactoryLatched)) {
    return;
  }
  gFactoryLatched = false;
  storage::setFactoryLatched(false);
}

void appTask(void*) {
  esp_task_wdt_add(nullptr);
  uint32_t lastSecond = 0;
  uint32_t lastResources = nowMs();
  for (;;) {
    esp_task_wdt_reset();
    const uint32_t now = nowMs();

    if (storage::configRevision() != gCfgRevision) applyConfigChange();

    const bool linkUp = stmLinkState() == vdm::LinkState::Up;
    if (vdm::elapsedMs(now, lastSecond) >= 1000) {
      lastSecond = now;
      net::service(now, mqtt::status().state == vdm::MqttState::Connected);
      if (net::isUp() && !web::started()) web::begin();
      ota::service(now, net::otaNetOk(), linkUp, web::started());
      checkFactoryLatch();
      stm_service::service(now);
    }
    if (vdm::elapsedMs(now, lastResources) >= 10000) {
      lastResources = now;
      sampleResources(now);
    }
    logger::service(net::isUp());
    storage::service();
    ota::serviceRestart(now, net::isUp(), linkUp);
    vTaskDelay(pdMS_TO_TICKS(100));
  }
}

void stmTaskEntry(void* arg) { stm_link::task(arg); }
void mqttTaskEntry(void* arg) { mqtt::task(arg); }

}  // namespace

uint32_t nowMs() { return millis(); }

uint32_t uptimeS() { return static_cast<uint32_t>(esp_timer_get_time() / 1000000LL); }

bool submit(const Command& cmd) {
  return gQueue != nullptr && xQueueSend(gQueue, &cmd, 0) == pdTRUE;
}

bool receive(Command& out) {
  return gQueue != nullptr && xQueueReceive(gQueue, &out, 0) == pdTRUE;
}

void readStmSnapshot(StmSnapshot& out) {
  if (gSnapMutex == nullptr) return;
  xSemaphoreTake(gSnapMutex, portMAX_DELAY);
  out = gSnapshot;
  xSemaphoreGive(gSnapMutex);
}

vdm::LinkState stmLinkState() { return gLinkState; }

bool stmFlashActive() { return gFlashActive; }

void markStmFlashActive() { gFlashActive = true; }

uint8_t stmProtocol() { return gProto; }

vdm::StmSupport stmSupport() { return gSupport; }

uint32_t stmSnapshotRevision() { return gSnapRevision; }

void publishStmSnapshot(const StmSnapshot& in) {
  if (gSnapMutex == nullptr) return;
  xSemaphoreTake(gSnapMutex, portMAX_DELAY);
  gSnapshot = in;
  xSemaphoreGive(gSnapMutex);
  gLinkState = in.link;
  gFlashActive = in.flash.phase != vdm::FlashPhase::Idle &&
                 in.flash.phase != vdm::FlashPhase::Done &&
                 in.flash.phase != vdm::FlashPhase::Failed;
  gProto = in.proto;
  gSupport = in.support;
  gSnapRevision = in.revision;
}

void requestStmSave() { gSaveState = vdm::StmSaveState::Waiting; }

vdm::StmSaveState stmSaveState() { return gSaveState; }

void setStmSaveState(vdm::StmSaveState s) { gSaveState = s; }

CalibInfo calibInfo() {
  portENTER_CRITICAL(&gCalibMux);
  const CalibInfo c = gCalibInfo;
  portEXIT_CRITICAL(&gCalibMux);
  return c;
}

void setCalibInfo(const CalibInfo& c) {
  portENTER_CRITICAL(&gCalibMux);
  gCalibInfo = c;
  portEXIT_CRITICAL(&gCalibMux);
}

void readHealth(vdm::HealthSnapshot& out) {
  const uint32_t now = nowMs();
  out = vdm::HealthSnapshot{};
  out.version = vdm::firmwareVersion();
  out.uptimeS = uptimeS();
  out.freeHeap = ESP.getFreeHeap();
  out.minFreeHeap = ESP.getMinFreeHeap();
  out.largestFreeBlock = ESP.getMaxAllocHeap();
  MonitoredTask tasks[kMonitoredTasks];
  portENTER_CRITICAL(&gHealthMux);
  out.minLargestFreeBlock = gMinLargest;
  memcpy(tasks, gTasks, sizeof tasks);
  portEXIT_CRITICAL(&gHealthMux);
  for (const MonitoredTask& t : tasks) {
    if (t.handle == nullptr) continue;
    vdm::TaskStackInfo& i = out.tasks[out.taskCount++];
    i.name = t.name;
    i.stackBytes = t.stackBytes;
    i.minFreeBytes = uxTaskGetStackHighWaterMark(t.handle);
  }
  out.net = net::health(now);
  out.ota = ota::health(now);
  out.log = logger::stats(now);
}

void setup() {
  Serial.begin(115200);
  // R6: release the STM from reset first thing (IO15 pull-up may hold it).
  stm_link::begin();

  gQueue = xQueueCreateStatic(kCommandQueueDepth, sizeof(Command), gQueueItems, &gQueueStorage);
  gSnapMutex = xSemaphoreCreateMutexStatic(&gSnapMutexStorage);

  logger::begin();
  // GPIO2 jumper: once per fitting (latch in NVS), not at every restart.
  const vdm::FactoryPinDecision pin = checkFactoryPin(storage::factoryLatched());

  bool formatted = false;
  const bool fsOk = storage::beginFs(formatted);

  const bool factoryReset = pin == vdm::FactoryPinDecision::Reset;
  const bool resetOk = factoryReset && storage::factoryReset();
  // The latch records a reset done: after a failed one the next boot with the
  // jumper still fitted tries again.
  if (resetOk) storage::setFactoryLatched(true);
  if (pin == vdm::FactoryPinDecision::ClearLatch) storage::setFactoryLatched(false);
  gFactoryLatched = resetOk || pin == vdm::FactoryPinDecision::KeepLatched;
  vdm::ImportReport report;
  storage::LoadDetails loadDetails;
  storage::loadConfig(gCfg, report, loadDetails);
  storage::setActiveConfig(gCfg);
  gCfgRevision = storage::configRevision();

  const uint32_t boots = storage::incrementBootCount();
  const esp_reset_reason_t reason = esp_reset_reason();
  logger::logSev(vdm::EventCode::Boot,
                 vdm::isAbnormalReset(reason) ? vdm::Severity::Warning : vdm::Severity::Info,
                 vdm::kNoValve, static_cast<int32_t>(reason), static_cast<int32_t>(boots),
                 vdm::firmwareVersion());
  if (formatted) logger::log(vdm::EventCode::FsFormatted);
  if (!fsOk) logger::log(vdm::EventCode::FsFormatted, vdm::kNoValve, -1);
  if (factoryReset) {
    logger::log(vdm::EventCode::ConfigSaved, vdm::kNoValve, static_cast<int32_t>(gCfgRevision),
                resetOk ? 0 : -1, "factory");
  }
  if (pin == vdm::FactoryPinDecision::KeepLatched) logger::log(vdm::EventCode::FactoryResetSkipped);
  logger::configure(gCfg.syslog.level, gCfg.syslog.server, gCfg.syslog.port, gCfg.persistLog,
                    gCfg.station);

  stm_service::begin();

  net::begin(gCfg);
  ota::begin();
  mqtt::begin();

  esp_task_wdt_init(kTaskWdtTimeoutS, true);
  startTask(kStmTask, stmTaskEntry, 0);
  startTask(kAppTask, appTask, 1);
  startTask(kMqttTask, mqttTaskEntry, 2);
}

}  // namespace app
