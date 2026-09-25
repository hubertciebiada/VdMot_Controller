// ESP OTA policy: decides when a freshly flashed image is healthy enough to
// be marked valid, and when it is rolled back. Hardware-free.
#pragma once

#include <stdint.h>

namespace vdm {

// OTA rollback confirmation (architecture §3): a freshly flashed ESP image
// is marked valid after it has been healthy for confirmMs (network up AND
// STM link Up, continuously), or after networkOnlyMs of uptime with the
// network up now (an STM problem must not roll back a good ESP image). If
// neither happened by giveUpMs of uptime, the image is rolled back.
class OtaValidator {
 public:
  enum class Decision : uint8_t { NotPending, Wait, MarkValid, Rollback };
  explicit OtaValidator(uint32_t confirmMs = 120000, uint32_t networkOnlyMs = 600000,
                        uint32_t giveUpMs = 900000);
  // pendingVerify: esp_ota_get_state_partition() == ESP_OTA_IMG_PENDING_VERIFY.
  void begin(bool pendingVerify, uint32_t nowMs);
  // Call every second. Returns MarkValid / Rollback exactly once, then
  // NotPending forever.
  Decision update(bool netUp, bool linkUp, uint32_t nowMs);
  // A restart is about to happen. The bootloader treats any reset of a
  // pending image as a failed boot and rolls back, so a restart the user
  // asked for through this firmware (reboot button, network settings,
  // factory reset) with the network up now confirms the image: that request
  // proves more than the timer. Returns true (and ends pending) when the
  // caller must mark the image valid first. Other restarts (network
  // watchdog, rollback) keep the rollback.
  bool confirmBeforeRestart(bool userRequested, bool netUp);
  bool pending() const { return pending_; }

 private:
  uint32_t confirmMs_, networkOnlyMs_, giveUpMs_;
  bool pending_ = false;
  uint32_t startMs_ = 0;
  bool healthy_ = false;
  uint32_t healthySinceMs_ = 0;
};

}  // namespace vdm
