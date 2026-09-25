// ESP firmware OTA: streaming upload into the inactive app partition
// (Update library, optional MD5 check), rollback protection
// (verifyRollbackLater + health confirmation via vdm::OtaValidator),
// graceful restart.
#pragma once

#include <stddef.h>
#include <stdint.h>

#include <vdm/json_api.h>

namespace ota {

// Reads the running partition's OTA state (PENDING_VERIFY after an update)
// and arms the validator; the STM link counts when NVS otaStm says it was
// up at the upload. otaStm is erased at once (a pending image gets one boot).
void begin();

// App task, every second: the loopback self-check (GET /api/health) when
// due, validator update; MarkValid -> esp_ota_mark_app_valid_cancel_rollback()
// + AppMarkedValid; Rollback -> RebootRequested(4, missing checks) and the
// restart path rolls back. netOk: net::otaNetOk(); webStarted: the web
// server runs.
void service(uint32_t nowMs, bool netOk, bool linkUp, bool webStarted);

// Validator state for /api/health.
vdm::OtaHealthInfo health(uint32_t nowMs);

// Upload steps called from the web handler (AsyncTCP task). begin fails when
// an upload, an STM image upload or an STM flash is running, or when
// announcedBytes (the request's Content-Length, 0 = unknown) exceeds the
// OTA partition plus multipart framing. `md5` is "" or 32 hex digits; when
// given, Update verifies the image against it. Data must arrive in order.
// end(true) finalises (image verification by the bootloader format and the
// MD5) and schedules a restart in 1 s (after the HTTP response); end(false)
// aborts. A failed write aborts the upload (later chunks are ignored).
bool uploadBegin(size_t announcedBytes, const char* md5);
bool uploadWrite(const uint8_t* data, size_t len);
bool uploadEnd(bool commit);
bool uploadActive();
const char* uploadError();

// Deferred restart from any task: logs RebootRequested(reason, detail)
// (vdm::RebootReason; detail: outage minutes for the net watchdog, missing
// checks for a rollback), waits delayMs so responses/MQTT offline can go
// out, then esp_restart(). The first request wins; later ones are ignored.
void requestRestart(uint8_t reason, uint32_t delayMs, int32_t detail = 0);
bool restartPending();
// App task: performs a requested restart when due and no STM flash runs:
// the STM EEPROM save (vdm::RestartGate, 12 s guard; with jumper X20 the ESP
// restart also resets the STM) and the desired targets (except for a
// factory reset), then the confirmation of a pending image by a user
// restart (reasons 0 and 3, vdm::OtaValidator::confirmBeforeRestart), otaStm
// for an upload, the log flush, and esp_restart() or the rollback.
void serviceRestart(uint32_t nowMs, bool netUp, bool linkUp);

}  // namespace ota
