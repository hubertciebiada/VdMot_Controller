#include "ota.h"

#include <Arduino.h>
#include <IPAddress.h>
#include <Update.h>
#include <WiFiClient.h>
#include <esp_ota_ops.h>
#include <freertos/FreeRTOS.h>
#include <freertos/task.h>
#include <stdio.h>
#include <string.h>

#include <vdm/event_log.h>
#include <vdm/ota_policy.h>
#include <vdm/restart_gate.h>
#include <vdm/sys_health.h>

#include "app.h"
#include "logger.h"
#include "net.h"
#include "stm_service.h"
#include "storage.h"

namespace ota {

namespace {

// Multipart framing around the firmware file (boundaries, part headers and
// an optional MD5 field) is far below this.
constexpr size_t kFramingSlack = 16 * 1024;
// Loopback self-check of the web server (W17): connect and answer limits.
constexpr int32_t kSelfCheckConnectMs = 1000;
constexpr uint32_t kSelfCheckAnswerMs = 3000;

// App task only.
vdm::OtaValidator gValidator;
vdm::RestartGate gGate;

// Written by the AsyncTCP task at the upload end, read by the app task.
volatile bool gUploadStmUp = false;

// Upload state: only touched by the AsyncTCP task (web handlers).
bool gUploadActive = false;
bool gUploadFailed = false;
char gUploadError[48] = "";
size_t gUploadWritten = 0;

portMUX_TYPE gMux = portMUX_INITIALIZER_UNLOCKED;
bool gRestartPending = false;
uint32_t gRestartAtMs = 0;
uint8_t gRestartReason = 0;
vdm::OtaHealthInfo gHealth;  // copy for health(), written by service()

bool pendingVerify() {
  const esp_partition_t* running = esp_ota_get_running_partition();
  esp_ota_img_states_t state;
  return running != nullptr && esp_ota_get_state_partition(running, &state) == ESP_OK &&
         state == ESP_OTA_IMG_PENDING_VERIFY;
}

void setError(const char* text) { vdm::copyString(gUploadError, sizeof gUploadError, text); }

void fail(const char* text, int32_t code) {
  setError(text);
  if (Update.isRunning()) Update.abort();
  gUploadActive = false;
  gUploadFailed = true;
  logger::log(vdm::EventCode::EspOtaFailed, vdm::kNoValve, code);
}

vdm::Severity rebootSeverity(uint8_t reason) {
  return (reason == 2 || reason == 4 || reason == 5) ? vdm::Severity::Warning
                                                     : vdm::Severity::Info;
}

// GET /api/health over the loopback interface; true when the status line
// says 200. Blocks the app task for at most ~4 s, only while the image is
// pending.
bool selfCheck() {
  char ip[16];
  vdm::formatIpv4(net::info().ip, ip, sizeof ip);
  WiFiClient c;
  char buf[12];
  size_t n = 0;
  if (c.connect(IPAddress(127, 0, 0, 1), 80, kSelfCheckConnectMs)) {
    char req[96];
    const int len = snprintf(req, sizeof req,
                             "GET /api/health HTTP/1.1\r\nHost: %s\r\nConnection: close\r\n\r\n", ip);
    c.write(reinterpret_cast<const uint8_t*>(req), static_cast<size_t>(len));
    const uint32_t start = millis();
    while (n < sizeof buf && vdm::elapsedMs(millis(), start) < kSelfCheckAnswerMs) {
      if (c.available() > 0) {
        const int r = c.read(reinterpret_cast<uint8_t*>(buf) + n, sizeof buf - n);
        if (r > 0) n += static_cast<size_t>(r);
      } else if (!c.connected()) {
        break;
      } else {
        vTaskDelay(pdMS_TO_TICKS(10));
      }
    }
  }
  c.stop();
  return vdm::httpStatusOk(buf, n);
}

void markValid() {
  if (esp_ota_mark_app_valid_cancel_rollback() == ESP_OK) {
    logger::log(vdm::EventCode::AppMarkedValid, vdm::kNoValve,
                static_cast<int32_t>(app::uptimeS()));
  }
}

}  // namespace

void begin() {
  const bool pending = pendingVerify();
  // A pending image gets exactly one boot to read the upload's link state.
  const bool stmRequired = pending && storage::otaStmRequired();
  storage::clearOtaStmRequired();
  gValidator.begin(pending, stmRequired, millis());
  portENTER_CRITICAL(&gMux);
  gHealth = vdm::OtaHealthInfo{};
  gHealth.pending = pending;
  gHealth.stmRequired = stmRequired;
  portEXIT_CRITICAL(&gMux);
}

void service(uint32_t nowMs, bool netOk, bool linkUp, bool webStarted) {
  if (gValidator.selfCheckDue(webStarted, nowMs) && net::isUp()) {
    gValidator.onSelfCheck(selfCheck(), nowMs);
  }
  const vdm::OtaValidator::Decision d = gValidator.update(netOk, linkUp, nowMs);
  if (d == vdm::OtaValidator::Decision::MarkValid) {
    markValid();
  } else if (d == vdm::OtaValidator::Decision::Rollback) {
    // Through the common restart path (STM EEPROM wait, log flush, MQTT
    // offline); a restart already pending becomes the rollback, except the
    // restart into an uploaded image: that image is selected already and
    // the rollback would discard it, while this image is left all the same.
    const int32_t missing = gValidator.missing();
    portENTER_CRITICAL(&gMux);
    const bool upload = gRestartPending && gRestartReason == 1;
    if (!upload) {
      if (!gRestartPending) {
        gRestartPending = true;
        gRestartAtMs = nowMs + 1000;
      }
      gRestartReason = 4;
    }
    portEXIT_CRITICAL(&gMux);
    if (!upload) {
      logger::logSev(vdm::EventCode::RebootRequested, vdm::Severity::Warning, vdm::kNoValve, 4,
                     missing);
    }
  }
  vdm::OtaHealthInfo h;
  h.pending = gValidator.pending();
  h.stmRequired = gValidator.stmRequired();
  h.netOk = netOk;
  h.httpOk = gValidator.httpOk(nowMs);
  h.stmOk = linkUp;
  h.healthyForS = gValidator.healthyForMs(nowMs) / 1000;
  h.remainingS = gValidator.remainingMs(nowMs) / 1000;
  portENTER_CRITICAL(&gMux);
  gHealth = h;
  portEXIT_CRITICAL(&gMux);
}

vdm::OtaHealthInfo health(uint32_t) {
  portENTER_CRITICAL(&gMux);
  const vdm::OtaHealthInfo h = gHealth;
  portEXIT_CRITICAL(&gMux);
  return h;
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
  const bool withMd5 = md5 != nullptr && md5[0] != '\0';
  char lower[33];  // Update compares against its lowercase digest string
  if (withMd5 && !vdm::normalizeMd5(md5, lower)) {
    setError("md5 invalid");
    return false;
  }
  if (!Update.begin(UPDATE_SIZE_UNKNOWN, U_FLASH)) {
    setError(Update.errorString());
    logger::log(vdm::EventCode::EspOtaFailed, vdm::kNoValve, Update.getError());
    return false;
  }
  if (withMd5 && !Update.setMD5(lower)) {
    Update.abort();
    setError("md5 invalid");
    return false;
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
  // The new image must prove the STM link only when it was up now; the NVS
  // write happens in the app task (serviceRestart).
  gUploadStmUp = app::stmLinkState() == vdm::LinkState::Up;
  requestRestart(1, 1000);
  return true;
}

bool uploadActive() { return gUploadActive; }

const char* uploadError() { return gUploadError[0] ? gUploadError : "unknown"; }

void requestRestart(uint8_t reason, uint32_t delayMs, int32_t detail) {
  bool first = false;
  portENTER_CRITICAL(&gMux);
  if (!gRestartPending) {
    gRestartPending = true;
    gRestartAtMs = millis() + delayMs;
    gRestartReason = reason;
    first = true;
  } else if (reason == 1 && gRestartReason == 4) {
    // An uploaded image replaces a pending rollback, which would discard it.
    gRestartReason = reason;
    first = true;
  }
  portEXIT_CRITICAL(&gMux);
  if (!first) return;
  logger::logSev(vdm::EventCode::RebootRequested, rebootSeverity(reason), vdm::kNoValve, reason,
                 detail);
}

bool restartPending() {
  portENTER_CRITICAL(&gMux);
  const bool p = gRestartPending;
  portEXIT_CRITICAL(&gMux);
  return p;
}

void serviceRestart(uint32_t nowMs, bool netUp, bool linkUp) {
  portENTER_CRITICAL(&gMux);
  const bool due = gRestartPending && vdm::timeReached(nowMs, gRestartAtMs);
  const uint8_t reason = gRestartReason;
  portEXIT_CRITICAL(&gMux);
  if (!due) return;
  // Never restart in the middle of an STM flash (the STM would be left
  // half-erased). Retry every pass until it is done.
  if (app::stmFlashActive()) return;
  // With jumper X20 the ESP restart also resets the STM: let a pending STM
  // EEPROM write finish first, and save the desired targets.
  switch (gGate.update(app::stmSaveState(), nowMs)) {
    case vdm::RestartGate::Step::RequestStmSave:
      app::requestStmSave();
      if (reason != 3) stm_service::flushForRestart();
      return;
    case vdm::RestartGate::Step::Wait:
      return;
    case vdm::RestartGate::Step::Proceed:
      break;
  }
  if (gGate.guardExpired()) {
    logger::log(vdm::EventCode::StmEepromWaitTimeout, vdm::kNoValve,
                static_cast<int32_t>(gGate.waitedMs(nowMs)), 3, "stm task silent");
  }
  // Reboot button, network/station settings (0) or factory reset (3) right
  // after an update: without this the bootloader would boot the old image.
  if (gValidator.confirmBeforeRestart(reason == 0 || reason == 3, netUp, linkUp)) markValid();
  if (reason == 1) storage::setOtaStmRequired(gUploadStmUp);
  logger::flush();  // the whole backlog, with the reason, before going down
  if (reason != 4) esp_restart();
  esp_ota_mark_app_invalid_rollback_and_reboot();
  // Only returns when there is no other valid image: keep running this one
  // (better than a boot loop).
  logger::log(vdm::EventCode::EspOtaFailed, vdm::kNoValve, -3);
  gGate.reset();
  portENTER_CRITICAL(&gMux);
  gRestartPending = false;
  portEXIT_CRITICAL(&gMux);
}

}  // namespace ota
