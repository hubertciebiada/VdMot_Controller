// Desired targets that survive an ESP restart: the 46-byte record kept in
// RTC slow memory (software restarts, panics, watchdog resets) and in NVS
// `vdmrev/targets` (power loss), the boot choice between the two and the
// debounced NVS saver. Hardware-free.
#pragma once

#include <stddef.h>
#include <stdint.h>

#include "vdm/valve_model.h"

namespace vdm {

constexpr size_t kPersistedTargetsSize = 46;

struct PersistedTargets {
  bool valid[kValveCount] = {};
  uint8_t pos[kValveCount] = {};          // 0..100
  TargetSource source[kValveCount] = {};  // None..Assembly
};
bool operator==(const PersistedTargets& a, const PersistedTargets& b);
inline bool operator!=(const PersistedTargets& a, const PersistedTargets& b) { return !(a == b); }

// Layout: "VDTG" (4), version 1 (1), count 12 (1), 12 x {flags bit0 valid,
// pos, source} (36), CRC-32 (vdm::crc32 over bytes 0..41, little endian)
// (4) = 46 bytes. Invalid entries are written as 0, 0, 0. Returns 46.
size_t encodeTargets(const PersistedTargets& t, uint8_t (&out)[kPersistedTargetsSize]);
// false (out = {}) for len != 46, a bad magic/version/count/CRC, or pos > 100,
// source > Assembly or flag bits other than bit0 in a valid entry.
bool decodeTargets(const uint8_t* data, size_t len, PersistedTargets& out);

// Every valve with a desired target.
void captureTargets(const ValveModel& m, PersistedTargets& out);
// restoreDesired() per valid entry; returns the number of valves restored.
uint8_t restoreTargets(ValveModel& m, const PersistedTargets& t);

enum class RestoreSource : uint8_t { None = 0, Rtc = 1, Nvs = 2 };
// The RTC copy wins when it decodes (it is never older than NVS), else NVS,
// else None (out = {}).
RestoreSource chooseTargets(const uint8_t* rtc, size_t rtcLen, const uint8_t* nvs, size_t nvsLen,
                            PersistedTargets& out);

// NVS copy, written by the app task: 5 min after the last change, at most
// 30 min after the first unsaved change, only when the value differs from
// the stored one.
class TargetSaver {
 public:
  static constexpr uint32_t kDebounceMs = 300000;
  static constexpr uint32_t kMaxDelayMs = 1800000;

  void primeStored(const PersistedTargets& t);  // boot: what NVS holds
  // Differs from the current value: new current value, dirty unless it
  // equals the stored one; the debounce restarts, the first-change time is
  // kept while dirty.
  void update(const PersistedTargets& t, uint32_t nowMs);
  bool dirty() const { return dirty_; }
  bool due(uint32_t nowMs) const;
  const uint8_t* bytes() const { return bytes_; }  // encoded current value
  // ok: stored := current, clean; !ok: the next attempt kDebounceMs later.
  void saved(bool ok, uint32_t nowMs);

 private:
  PersistedTargets stored_;
  PersistedTargets current_;
  uint8_t bytes_[kPersistedTargetsSize] = {};
  bool dirty_ = false;
  uint32_t lastChangeMs_ = 0;
  uint32_t firstChangeMs_ = 0;
};

}  // namespace vdm
