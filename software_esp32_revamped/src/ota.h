// ESP firmware OTA: streaming upload into the inactive app partition
// (Update library, optional MD5 check), rollback protection
// (verifyRollbackLater + health confirmation via vdm::OtaValidator),
// graceful restart.
#pragma once

#include <stddef.h>
#include <stdint.h>

namespace ota {

// Reads the running partition's OTA state (PENDING_VERIFY after an update)
// and arms the validator.
void begin();

// App task, every second: MarkValid -> esp_ota_mark_app_valid_cancel_rollback()
// + AppMarkedValid event; Rollback -> RebootRequested(4), log flush,
// esp_ota_mark_app_invalid_rollback_and_reboot().
void service(uint32_t nowMs, bool netUp, bool linkUp);

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

// Deferred restart from any task: logs RebootRequested(reason), waits
// delayMs so responses/MQTT offline can go out, then esp_restart(). The
// first request wins; later ones are ignored.
void requestRestart(uint8_t reason, uint32_t delayMs);
bool restartPending();
// App task: performs a requested restart when due (log flushed first).
void serviceRestart(uint32_t nowMs);

}  // namespace ota
