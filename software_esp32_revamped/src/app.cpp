#include "app.h"

#include <Arduino.h>
#include <esp_system.h>
#include <esp_task_wdt.h>
#include <freertos/FreeRTOS.h>
#include <freertos/queue.h>
#include <freertos/semphr.h>
#include <freertos/task.h>

#include <vdm/calib_schedule.h>
#include <vdm/config.h>
#include <vdm/event_log.h>

#include "board.h"
#include "logger.h"
#include "mqtt_client.h"
#include "net.h"
#include "ota.h"
#include "stm_link.h"
#include "storage.h"
#include "web_server.h"

namespace app {

namespace {

StaticQueue_t gQueueStorage;
uint8_t gQueueItems[kCommandQueueDepth * sizeof(Command)];
QueueHandle_t gQueue = nullptr;

StaticSemaphore_t gSnapMutexStorage;
SemaphoreHandle_t gSnapMutex = nullptr;
StmSnapshot gSnapshot;

// App task working copies (static: too large for the task stack).
vdm::Config gCfg;
uint32_t gCfgRevision = 0;
vdm::CalibScheduler gCalib;

// Factory reset pin: held LOW for kFactoryResetHoldMs at boot.
bool factoryResetRequested() {
  pinMode(board::kFactoryResetPin, INPUT_PULLUP);
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

void calibrationTick(uint32_t now) {
  vdm::LocalTime lt = net::localTime();
  const vdm::CalibDecision d = gCalib.evaluate(gCfg.calib, lt, now);
  if (d == vdm::CalibDecision::Fire) {
    Command c;
    c.type = CommandType::Calibrate;
    c.valve = vdm::kAllValves;
    if (submit(c)) {
      storage::saveCalibSlot(gCalib.lastSlot());
      storage::saveLastCalib(lt.epoch);
      logger::log(vdm::EventCode::ScheduledCalibration, vdm::kNoValve,
                  static_cast<int32_t>(gCalib.lastSlot()), gCalib.lateMinutes());
    } else {
      logger::log(vdm::EventCode::StmQueueFull, vdm::kNoValve,
                  static_cast<int32_t>(vdm::Cmd::Staln));
    }
  } else if (d == vdm::CalibDecision::SkippedNoTime) {
    logger::log(vdm::EventCode::CalibTimeMissing, vdm::kNoValve, 0);
  }
}

void appTask(void*) {
  esp_task_wdt_add(nullptr);
  uint32_t lastSecond = 0;
  uint32_t lastCalib = 0;
  for (;;) {
    esp_task_wdt_reset();
    const uint32_t now = nowMs();

    if (storage::configRevision() != gCfgRevision) {
      gCfgRevision = storage::configRevision();
      storage::getConfig(gCfg);
    }

    if (vdm::elapsedMs(now, lastSecond) >= 1000) {
      lastSecond = now;
      net::service(now);
      if (net::isUp() && !web::started()) web::begin();
      ota::service(now, net::isUp(), stmLinkState() == vdm::LinkState::Up);
    }
    if (vdm::elapsedMs(now, lastCalib) >= 10000) {
      lastCalib = now;
      calibrationTick(now);
    }
    logger::service(net::isUp());
    ota::serviceRestart(now);
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

vdm::LinkState stmLinkState() {
  if (gSnapMutex == nullptr) return vdm::LinkState::Unknown;
  xSemaphoreTake(gSnapMutex, portMAX_DELAY);
  const vdm::LinkState s = gSnapshot.link;
  xSemaphoreGive(gSnapMutex);
  return s;
}

void publishStmSnapshot(const StmSnapshot& in) {
  if (gSnapMutex == nullptr) return;
  xSemaphoreTake(gSnapMutex, portMAX_DELAY);
  gSnapshot = in;
  xSemaphoreGive(gSnapMutex);
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

  if (factoryReset) storage::factoryReset();
  vdm::ImportReport report;
  const storage::LoadSource src = storage::loadConfig(gCfg, report);
  storage::setActiveConfig(gCfg);
  gCfgRevision = storage::configRevision();

  const uint32_t boots = storage::incrementBootCount();
  logger::log(vdm::EventCode::Boot, vdm::kNoValve, static_cast<int32_t>(esp_reset_reason()),
              static_cast<int32_t>(boots), vdm::firmwareVersion());
  if (formatted) logger::log(vdm::EventCode::FsFormatted);
  if (!fsOk) logger::log(vdm::EventCode::FsFormatted, vdm::kNoValve, -1);
  if (factoryReset) logger::log(vdm::EventCode::RebootRequested, vdm::kNoValve, 3);
  if (src == storage::LoadSource::Imported) {
    logger::log(vdm::EventCode::ConfigImported, vdm::kNoValve, report.imported, report.rejected,
                report.firstRejected);
  } else if (src == storage::LoadSource::DefaultsAfterError) {
    logger::log(vdm::EventCode::ConfigDefaults, vdm::kNoValve, 1);
  }
  logger::configure(gCfg.syslog.level, gCfg.syslog.server, gCfg.syslog.port, gCfg.persistLog,
                    gCfg.station);

  gCalib.restoreLastSlot(storage::loadCalibSlot());

  net::begin(gCfg);
  ota::begin();
  mqtt::begin();

  esp_task_wdt_init(kTaskWdtTimeoutS, true);
  startTask(kStmTask, stmTaskEntry);
  startTask(kAppTask, appTask);
  startTask(kMqttTask, mqttTaskEntry);
}

}  // namespace app
