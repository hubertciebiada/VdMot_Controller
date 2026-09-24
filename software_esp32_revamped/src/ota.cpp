#include "ota.h"

#include <Arduino.h>
#include <Update.h>
#include <esp_ota_ops.h>
#include <freertos/FreeRTOS.h>
#include <string.h>

#include <vdm/event_log.h>
#include <vdm/health_monitor.h>

#include "app.h"
#include "logger.h"
#include "storage.h"

namespace ota {

namespace {

// Multipart framing around the firmware file (boundaries, part headers and
// an optional MD5 field) is far below this.
constexpr size_t kFramingSlack = 16 * 1024;

vdm::OtaValidator gValidator;
bool gRollbackPending = false;  // app task only

// Upload state: only touched by the AsyncTCP task (web handlers).
bool gUploadActive = false;
bool gUploadFailed = false;
char gUploadError[48] = "";
size_t gUploadWritten = 0;

portMUX_TYPE gMux = portMUX_INITIALIZER_UNLOCKED;
bool gRestartPending = false;
uint32_t gRestartAtMs = 0;

bool pendingVerify() {
  const esp_partition_t* running = esp_ota_get_running_partition();
  esp_ota_img_states_t state;
  return running != nullptr && esp_ota_get_state_partition(running, &state) == ESP_OK &&
         state == ESP_OTA_IMG_PENDING_VERIFY;
}

void setError(const char* text) { vdm::copyString(gUploadError, sizeof gUploadError, text); }

bool isHex32(const char* s) {
  if (s == nullptr || strlen(s) != 32) return false;
  for (size_t i = 0; i < 32; ++i) {
    const char c = s[i];
    if (!((c >= '0' && c <= '9') || (c >= 'a' && c <= 'f') || (c >= 'A' && c <= 'F'))) return false;
  }
  return true;
}

void fail(const char* text, int32_t code) {
  setError(text);
  if (Update.isRunning()) Update.abort();
  gUploadActive = false;
  gUploadFailed = true;
  logger::log(vdm::EventCode::EspOtaFailed, vdm::kNoValve, code);
}

}  // namespace

void begin() { gValidator.begin(pendingVerify(), millis()); }

void service(uint32_t nowMs, bool netUp, bool linkUp) {
  switch (gValidator.update(netUp, linkUp, nowMs)) {
    case vdm::OtaValidator::Decision::MarkValid:
      if (esp_ota_mark_app_valid_cancel_rollback() == ESP_OK) {
        logger::log(vdm::EventCode::AppMarkedValid, vdm::kNoValve,
                    static_cast<int32_t>(app::uptimeS()));
      }
      break;
    case vdm::OtaValidator::Decision::Rollback:
      logger::logSev(vdm::EventCode::RebootRequested, vdm::Severity::Warning, vdm::kNoValve, 4);
      gRollbackPending = true;
      break;
    default:
      break;
  }
  // Never reboot in the middle of an STM flash (the STM would be left
  // half-erased): the rollback waits until the flasher is done.
  if (!gRollbackPending || app::stmFlashActive()) return;
  gRollbackPending = false;
  logger::service(false);  // flush the reason to the log file
  delay(200);              // let the serial line out; the app task is the caller
  esp_ota_mark_app_invalid_rollback_and_reboot();
  // Only returns when there is no valid partition to roll back to: keep
  // running this image (better than a boot loop).
}

bool uploadBegin(size_t announcedBytes, const char* md5) {
  if (gUploadActive) {
    setError("busy");
    return false;
  }
  gUploadFailed = false;
  if (app::stmFlashActive() || storage::imageUploadActive()) {
    setError("stm flash or upload running");
    return false;
  }
  const esp_partition_t* target = esp_ota_get_next_update_partition(nullptr);
  if (target == nullptr) {
    setError("no ota partition");
    return false;
  }
  if (announcedBytes > target->size + kFramingSlack) {
    setError("image too large");
    return false;
  }
  if (md5 != nullptr && md5[0] != '\0' && !isHex32(md5)) {
    setError("md5 invalid");
    return false;
  }
  if (!Update.begin(UPDATE_SIZE_UNKNOWN, U_FLASH)) {
    setError(Update.errorString());
    logger::log(vdm::EventCode::EspOtaFailed, vdm::kNoValve, Update.getError());
    return false;
  }
  if (md5 != nullptr && md5[0] != '\0') {
    // Update compares against its lowercase digest string.
    char lower[33];
    for (size_t i = 0; i < 32; ++i) {
      const char c = md5[i];
      lower[i] = (c >= 'A' && c <= 'F') ? static_cast<char>(c - 'A' + 'a') : c;
    }
    lower[32] = '\0';
    if (!Update.setMD5(lower)) {
      Update.abort();
      setError("md5 invalid");
      return false;
    }
  }
  gUploadActive = true;
  gUploadError[0] = '\0';
  gUploadWritten = 0;
  logger::log(vdm::EventCode::EspOtaStarted, vdm::kNoValve, static_cast<int32_t>(announcedBytes));
  return true;
}

bool uploadWrite(const uint8_t* data, size_t len) {
  if (!gUploadActive) return false;
  if (len == 0) return true;
  if (Update.write(const_cast<uint8_t*>(data), len) != len) {
    fail(Update.errorString(), Update.getError());
    return false;
  }
  gUploadWritten += len;
  return true;
}

bool uploadEnd(bool commit) {
  if (!gUploadActive) {
    if (!gUploadFailed) setError("no upload");
    return false;
  }
  if (!commit) {
    fail("aborted", -1);
    return false;
  }
  if (gUploadWritten == 0) {
    fail("empty image", -2);
    return false;
  }
  // Size unknown up front (multipart): end(true) takes what was written;
  // Update checks the MD5 when one was given and esp_ota_end() verifies the
  // app image (segments, checksum, appended SHA-256) before it is selected.
  if (!Update.end(true)) {
    fail(Update.errorString(), Update.getError());
    return false;
  }
  gUploadActive = false;
  logger::log(vdm::EventCode::EspOtaDone, vdm::kNoValve, static_cast<int32_t>(gUploadWritten));
  requestRestart(1, 1000);
  return true;
}

bool uploadActive() { return gUploadActive; }

const char* uploadError() { return gUploadError[0] ? gUploadError : "unknown"; }

void requestRestart(uint8_t reason, uint32_t delayMs) {
  bool first = false;
  portENTER_CRITICAL(&gMux);
  if (!gRestartPending) {
    gRestartPending = true;
    gRestartAtMs = millis() + delayMs;
    first = true;
  }
  portEXIT_CRITICAL(&gMux);
  if (!first) return;
  logger::logSev(vdm::EventCode::RebootRequested,
                 (reason == 2 || reason == 4) ? vdm::Severity::Warning : vdm::Severity::Info,
                 vdm::kNoValve, reason);
}

bool restartPending() {
  portENTER_CRITICAL(&gMux);
  const bool p = gRestartPending;
  portEXIT_CRITICAL(&gMux);
  return p;
}

void serviceRestart(uint32_t nowMs) {
  portENTER_CRITICAL(&gMux);
  const bool due = gRestartPending && vdm::timeReached(nowMs, gRestartAtMs);
  portEXIT_CRITICAL(&gMux);
  if (!due) return;
  // An ESP OTA still being written would be lost anyway; the flasher is the
  // exception: never restart in the middle of an STM flash (the STM would be
  // left half-erased). Retry every pass until it is done.
  if (app::stmFlashActive()) return;
  logger::service(false);  // flush the log file before going down
  esp_restart();
}

}  // namespace ota
