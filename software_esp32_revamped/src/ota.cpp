#include "ota.h"

#include <Arduino.h>
#include <Update.h>
#include <esp_ota_ops.h>

#include <vdm/event_log.h>
#include <vdm/health_monitor.h>

#include "logger.h"

namespace ota {

namespace {

vdm::OtaValidator gValidator;
volatile bool gUploadActive = false;
const char* gUploadError = "";
size_t gUploadExpected = 0;
size_t gUploadWritten = 0;

volatile bool gRestartPending = false;
uint32_t gRestartAtMs = 0;

bool pendingVerify() {
  const esp_partition_t* running = esp_ota_get_running_partition();
  esp_ota_img_states_t state;
  return running != nullptr && esp_ota_get_state_partition(running, &state) == ESP_OK &&
         state == ESP_OTA_IMG_PENDING_VERIFY;
}

}  // namespace

void begin() { gValidator.begin(pendingVerify(), millis()); }

void service(uint32_t nowMs, bool netUp, bool linkUp) {
  switch (gValidator.update(netUp, linkUp, nowMs)) {
    case vdm::OtaValidator::Decision::MarkValid:
      esp_ota_mark_app_valid_cancel_rollback();
      logger::log(vdm::EventCode::AppMarkedValid, vdm::kNoValve,
                  static_cast<int32_t>(nowMs / 1000));
      break;
    case vdm::OtaValidator::Decision::Rollback:
      logger::log(vdm::EventCode::RebootRequested, vdm::kNoValve, 4);
      delay(200);  // let the serial line out; the app task is the caller
      esp_ota_mark_app_invalid_rollback_and_reboot();
      break;
    default:
      break;
  }
}

bool uploadBegin(size_t totalSize) {
  if (gUploadActive) {
    gUploadError = "busy";
    return false;
  }
  if (!Update.begin(totalSize > 0 ? totalSize : UPDATE_SIZE_UNKNOWN, U_FLASH)) {
    gUploadError = Update.errorString();
    logger::log(vdm::EventCode::EspOtaFailed, vdm::kNoValve, Update.getError());
    return false;
  }
  gUploadActive = true;
  gUploadError = "";
  gUploadExpected = totalSize;
  gUploadWritten = 0;
  logger::log(vdm::EventCode::EspOtaStarted, vdm::kNoValve, static_cast<int32_t>(totalSize));
  return true;
}

bool uploadWrite(const uint8_t* data, size_t len) {
  if (!gUploadActive) return false;
  if (Update.write(const_cast<uint8_t*>(data), len) != len) {
    gUploadError = Update.errorString();
    Update.abort();
    gUploadActive = false;
    logger::log(vdm::EventCode::EspOtaFailed, vdm::kNoValve, Update.getError());
    return false;
  }
  gUploadWritten += len;
  return true;
}

bool uploadEnd(bool commit) {
  if (!gUploadActive) return false;
  gUploadActive = false;
  if (!commit || (gUploadExpected > 0 && gUploadWritten != gUploadExpected)) {
    Update.abort();
    gUploadError = commit ? "size mismatch" : "aborted";
    logger::log(vdm::EventCode::EspOtaFailed, vdm::kNoValve, -1);
    return false;
  }
  if (!Update.end(true)) {
    gUploadError = Update.errorString();
    logger::log(vdm::EventCode::EspOtaFailed, vdm::kNoValve, Update.getError());
    return false;
  }
  logger::log(vdm::EventCode::EspOtaDone, vdm::kNoValve, static_cast<int32_t>(gUploadWritten));
  requestRestart(1, 1000);
  return true;
}

bool uploadActive() { return gUploadActive; }

const char* uploadError() { return gUploadError; }

void requestRestart(uint8_t reason, uint32_t delayMs) {
  if (gRestartPending) return;
  logger::log(vdm::EventCode::RebootRequested, vdm::kNoValve, reason);
  gRestartAtMs = millis() + delayMs;
  gRestartPending = true;
}

void serviceRestart(uint32_t nowMs) {
  if (gRestartPending && vdm::timeReached(nowMs, gRestartAtMs)) {
    logger::service(false);  // flush the log file before going down
    esp_restart();
  }
}

}  // namespace ota
