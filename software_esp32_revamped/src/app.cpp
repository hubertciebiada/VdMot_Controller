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
#include <vdm/json_api.h>

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

constexpr uint32_t kLowHeapBytes = 30 * 1024;
constexpr uint32_t kLowHeapRepeatMs = 3600000;

// Factory reset pin: held LOW for kFactoryResetHoldMs at boot.
bool factoryResetRequested() {
  pinMode(board::kFactoryResetPin, INPUT_PULLUP);
  delay(2);  // let the pull-up settle
  if (digitalRead(board::kFactoryResetPin) != LOW) return false;
  delay(board::kFactoryResetHoldMs);
  return digitalRead(board::kFactoryResetPin) == LOW;
}

void startTask(const TaskSpec& spec, TaskFunction_t fn) {
  TaskHandle_t handle = nullptr;
  if (xTaskCreatePinnedToCore(fn, spec.name, spec.stackBytes, nullptr, spec.priority, &handle,
                              spec.core) != pdPASS) {
    // Without its tasks the firmware cannot work; a reboot is the only
    // recovery (logged as panic reason on the next boot).
    abort();
  }
}

// Boot reasons that point at a crash or a power problem (Boot is a Warning).
bool abnormalReset(esp_reset_reason_t r) {
  switch (r) {
    case ESP_RST_PANIC:
    case ESP_RST_INT_WDT:
    case ESP_RST_TASK_WDT:
    case ESP_RST_WDT:
    case ESP_RST_BROWNOUT:
      return true;
    default:
      return false;
  }
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

void checkHeap(uint32_t now) {
  static bool reported = false;
  static uint32_t lastReportMs = 0;
  const uint32_t freeHeap = ESP.getFreeHeap();
  if (freeHeap >= kLowHeapBytes) return;
  if (reported && vdm::elapsedMs(now, lastReportMs) < kLowHeapRepeatMs) return;
  reported = true;
  lastReportMs = now;
  logger::log(vdm::EventCode::LowHeap, vdm::kNoValve, static_cast<int32_t>(freeHeap),
              static_cast<int32_t>(ESP.getMinFreeHeap()));
}

void appTask(void*) {
  esp_task_wdt_add(nullptr);
  uint32_t lastSecond = 0;
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
      checkHeap(now);
      stm_service::service(now);
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
  out = vdm::HealthSnapshot{};
  out.version = vdm::firmwareVersion();
  out.uptimeS = uptimeS();
  out.freeHeap = ESP.getFreeHeap();
  out.minFreeHeap = ESP.getMinFreeHeap();
  out.largestFreeBlock = ESP.getMaxAllocHeap();
}

void setup() {
  Serial.begin(115200);
  // R6: release the STM from reset first thing (IO15 pull-up may hold it).
  stm_link::begin();

  gQueue = xQueueCreateStatic(kCommandQueueDepth, sizeof(Command), gQueueItems, &gQueueStorage);
  gSnapMutex = xSemaphoreCreateMutexStatic(&gSnapMutexStorage);

  logger::begin();
  const bool factoryReset = factoryResetRequested();

  bool formatted = false;
  const bool fsOk = storage::beginFs(formatted);

  const bool resetOk = factoryReset && storage::factoryReset();
  vdm::ImportReport report;
  storage::LoadDetails loadDetails;
  storage::loadConfig(gCfg, report, loadDetails);
  storage::setActiveConfig(gCfg);
  gCfgRevision = storage::configRevision();

  const uint32_t boots = storage::incrementBootCount();
  const esp_reset_reason_t reason = esp_reset_reason();
  logger::logSev(vdm::EventCode::Boot,
                 abnormalReset(reason) ? vdm::Severity::Warning : vdm::Severity::Info,
                 vdm::kNoValve, static_cast<int32_t>(reason), static_cast<int32_t>(boots),
                 vdm::firmwareVersion());
  if (formatted) logger::log(vdm::EventCode::FsFormatted);
  if (!fsOk) logger::log(vdm::EventCode::FsFormatted, vdm::kNoValve, -1);
  if (factoryReset) {
    logger::log(vdm::EventCode::ConfigSaved, vdm::kNoValve, static_cast<int32_t>(gCfgRevision),
                resetOk ? 0 : -1, "factory");
  }
  logger::configure(gCfg.syslog.level, gCfg.syslog.server, gCfg.syslog.port, gCfg.persistLog,
                    gCfg.station);

  stm_service::begin();

  net::begin(gCfg);
  ota::begin();
  mqtt::begin();

  esp_task_wdt_init(kTaskWdtTimeoutS, true);
  startTask(kStmTask, stmTaskEntry);
  startTask(kAppTask, appTask);
  startTask(kMqttTask, mqttTaskEntry);
}

}  // namespace app
