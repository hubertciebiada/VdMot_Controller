// ESP firmware OTA: streaming upload into the inactive app partition
// (Update library), rollback protection (verifyRollbackLater + health
// confirmation via vdm::OtaValidator), graceful restart.
#pragma once

#include <stddef.h>
#include <stdint.h>

namespace ota {

// Reads the running partition's OTA state (PENDING_VERIFY after an update)
// and arms the validator.
void begin();

// App task, every second: MarkValid -> esp_ota_mark_app_valid_cancel_rollback()
// + AppMarkedValid event; Rollback -> esp_ota_mark_app_invalid_rollback_and_reboot().
void service(uint32_t nowMs, bool netUp, bool linkUp);

// Upload steps called from the web handler (AsyncTCP task). begin fails when
// an upload or an STM flash is already running, or when size exceeds the
// partition. Data must arrive in order. end(true) finalises and schedules a
// restart in 1 s (after the HTTP response); end(false) aborts.
bool uploadBegin(size_t totalSize);
bool uploadWrite(const uint8_t* data, size_t len);
bool uploadEnd(bool commit);
bool uploadActive();
const char* uploadError();

// Deferred restart from any task: logs RebootRequested(reason), waits
// delayMs so responses/MQTT offline can go out, then esp_restart().
void requestRestart(uint8_t reason, uint32_t delayMs);
// App task: performs a requested restart when due.
void serviceRestart(uint32_t nowMs);

}  // namespace ota
