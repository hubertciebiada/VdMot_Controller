#include "vdm/target_store.h"

#include "vdm/config.h"

namespace vdm {

namespace {

constexpr uint8_t kMagic[4] = {'V', 'D', 'T', 'G'};
constexpr uint8_t kVersion = 1;
constexpr size_t kEntriesAt = 6;
constexpr size_t kCrcAt = kEntriesAt + 3 * kValveCount;  // 42

}  // namespace

bool operator==(const PersistedTargets& a, const PersistedTargets& b) {
  for (uint8_t i = 0; i < kValveCount; ++i) {
    if (a.valid[i] != b.valid[i]) return false;
    // Invalid entries carry no value (encoded as 0, 0, 0).
    if (a.valid[i] && (a.pos[i] != b.pos[i] || a.source[i] != b.source[i])) return false;
  }
  return true;
}

size_t encodeTargets(const PersistedTargets& t, uint8_t (&out)[kPersistedTargetsSize]) {
  for (size_t i = 0; i < 4; ++i) out[i] = kMagic[i];
  out[4] = kVersion;
  out[5] = kValveCount;
  for (uint8_t v = 0; v < kValveCount; ++v) {
    uint8_t* e = out + kEntriesAt + 3 * v;
    e[0] = t.valid[v] ? 1 : 0;
    e[1] = t.valid[v] ? t.pos[v] : 0;
    e[2] = t.valid[v] ? static_cast<uint8_t>(t.source[v]) : 0;
  }
  const uint32_t crc = crc32(out, kCrcAt);
  for (size_t i = 0; i < 4; ++i) out[kCrcAt + i] = static_cast<uint8_t>(crc >> (8 * i));
  return kPersistedTargetsSize;
}

bool decodeTargets(const uint8_t* data, size_t len, PersistedTargets& out) {
  out = PersistedTargets{};
  if (data == nullptr || len != kPersistedTargetsSize) return false;
  for (size_t i = 0; i < 4; ++i) {
    if (data[i] != kMagic[i]) return false;
  }
  if (data[4] != kVersion || data[5] != kValveCount) return false;
  uint32_t crc = 0;
  for (size_t i = 0; i < 4; ++i) crc |= static_cast<uint32_t>(data[kCrcAt + i]) << (8 * i);
  if (crc != crc32(data, kCrcAt)) return false;
  PersistedTargets t;
  for (uint8_t v = 0; v < kValveCount; ++v) {
    const uint8_t* e = data + kEntriesAt + 3 * v;
    if (e[0] == 0) continue;
    if (e[0] != 1 || e[1] > 100 || e[2] > static_cast<uint8_t>(TargetSource::Assembly)) {
      return false;
    }
    t.valid[v] = true;
    t.pos[v] = e[1];
    t.source[v] = static_cast<TargetSource>(e[2]);
  }
  out = t;
  return true;
}

void captureTargets(const ValveModel& m, PersistedTargets& out) {
  out = PersistedTargets{};
  for (uint8_t v = 0; v < kValveCount; ++v) {
    const ValveState& s = m.valve(v);
    if (!s.desiredValid) continue;
    out.valid[v] = true;
    out.pos[v] = s.desired;
    out.source[v] = s.source;
  }
}

uint8_t restoreTargets(ValveModel& m, const PersistedTargets& t) {
  uint8_t n = 0;
  for (uint8_t v = 0; v < kValveCount; ++v) {
    if (t.valid[v] && m.restoreDesired(v, t.pos[v], t.source[v])) ++n;
  }
  return n;
}

RestoreSource chooseTargets(const uint8_t* rtc, size_t rtcLen, const uint8_t* nvs, size_t nvsLen,
                            PersistedTargets& out) {
  if (decodeTargets(rtc, rtcLen, out)) return RestoreSource::Rtc;
  if (decodeTargets(nvs, nvsLen, out)) return RestoreSource::Nvs;
  return RestoreSource::None;
}

// ---------------------------------------------------------------- TargetSaver

void TargetSaver::primeStored(const PersistedTargets& t) {
  stored_ = t;
  current_ = t;
  encodeTargets(current_, bytes_);
  dirty_ = false;
}

void TargetSaver::update(const PersistedTargets& t, uint32_t nowMs) {
  if (t == current_) return;
  current_ = t;
  encodeTargets(current_, bytes_);
  if (current_ == stored_) {
    dirty_ = false;
    return;
  }
  if (!dirty_) firstChangeMs_ = nowMs;
  dirty_ = true;
  lastChangeMs_ = nowMs;
}

bool TargetSaver::due(uint32_t nowMs) const {
  return dirty_ && (elapsedMs(nowMs, lastChangeMs_) >= kDebounceMs ||
                    elapsedMs(nowMs, firstChangeMs_) >= kMaxDelayMs);
}

void TargetSaver::saved(bool ok, uint32_t nowMs) {
  if (ok) {
    stored_ = current_;
    dirty_ = false;
    return;
  }
  // Retry one debounce later; the maximum delay starts again as well.
  lastChangeMs_ = nowMs;
  firstChangeMs_ = nowMs;
}

}  // namespace vdm
