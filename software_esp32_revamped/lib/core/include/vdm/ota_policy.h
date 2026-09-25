// ESP OTA policy: decides when a freshly flashed image is healthy enough to
// be marked valid, and when it is rolled back. Hardware-free.
#pragma once

#include <stddef.h>
#include <stdint.h>

namespace vdm {

// OTA rollback confirmation. A freshly flashed image is marked valid after
// confirmMs of uninterrupted health:
//   net  - the network is up and proven end to end (NetReachability, net_policy.h),
//   http - the loopback self-check GET /api/health answered 200 within kHttpFreshMs,
//   stm  - the STM link is Up; counted only when stmRequired (the link was Up when
//          the image was uploaded).
// If that never happened by giveUpMs of uptime, the image is rolled back.
class OtaValidator {
 public:
  enum class Decision : uint8_t { NotPending, Wait, MarkValid, Rollback };
  static constexpr uint8_t kCheckNet = 1;
  static constexpr uint8_t kCheckHttp = 2;
  static constexpr uint8_t kCheckStm = 4;
  static constexpr uint32_t kSelfCheckIntervalMs = 10000;
  static constexpr uint32_t kHttpFreshMs = 30000;
  explicit OtaValidator(uint32_t confirmMs = 120000, uint32_t giveUpMs = 900000);
  // pendingVerify: esp_ota_get_state_partition() == ESP_OTA_IMG_PENDING_VERIFY.
  // A second call re-arms everything.
  void begin(bool pendingVerify, bool stmRequired, uint32_t nowMs);
  // Due while pending and the web server runs: at once, then every kSelfCheckIntervalMs.
  bool selfCheckDue(bool webStarted, uint32_t nowMs) const;
  void onSelfCheck(bool ok, uint32_t nowMs);
  // Every second. Returns MarkValid / Rollback exactly once, then NotPending.
  Decision update(bool netOk, bool linkUp, uint32_t nowMs);
  // kCheck* bits that failed at the last update() (kCheckStm only when stmRequired).
  uint8_t missing() const { return missing_; }
  // The http check at `nowMs` (last self-check ok and younger than kHttpFreshMs).
  bool httpOk(uint32_t nowMs) const;
  // A user restart (reboot, network settings, factory reset) of a pending
  // image confirms it when netUp and (!stmRequired || linkUp): the request
  // itself came over HTTP. The bootloader treats any other reset of a
  // pending image as a failed boot. Returns true (and ends pending) when the
  // caller must mark the image valid first.
  bool confirmBeforeRestart(bool userRequested, bool netUp, bool linkUp);
  bool pending() const { return pending_; }
  bool stmRequired() const { return stmRequired_; }
  uint32_t healthyForMs(uint32_t nowMs) const;  // 0 while not healthy or not pending
  uint32_t remainingMs(uint32_t nowMs) const;   // until giveUpMs, 0 when not pending

 private:
  uint32_t confirmMs_, giveUpMs_;
  bool pending_ = false, stmRequired_ = false;
  uint32_t startMs_ = 0;
  bool healthy_ = false;
  uint32_t healthySinceMs_ = 0;
  bool checked_ = false, checkOk_ = false;
  uint32_t checkMs_ = 0;
  uint8_t missing_ = 0;
};

// Upload MD5 as given by the user: exactly 32 hex digits, written lowercase
// (NUL-terminated) to out[33]. False (out unchanged) otherwise.
bool normalizeMd5(const char* in, char* out);

}  // namespace vdm
