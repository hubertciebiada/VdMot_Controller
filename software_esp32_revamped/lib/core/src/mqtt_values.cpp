#include "vdm/mqtt_values.h"

#include <stdio.h>
#include <string.h>

#include <algorithm>

namespace vdm {

uint8_t systemState(LinkState link, const ValveState* valves, uint8_t count, uint16_t activeMask,
                    const SystemFlags& f) {
  bool info = link != LinkState::Up || f.failsafe;
  if (link == LinkState::Down || f.safeMode) return 2;
  if (valves != nullptr) {
    const uint8_t n = std::min(count, kValveCount);
    for (uint8_t i = 0; i < n; ++i) {
      if (((activeMask >> i) & 1u) == 0) continue;
      const uint16_t h = valves[i].health;
      if (h & (kHealthBlocked | kHealthFailed)) return 2;
      if (h != 0) info = true;
    }
  }
  return info ? 1 : 0;
}

uint16_t activeValveMask(const Config& c) {
  uint16_t m = 0;
  for (uint8_t i = 0; i < kValveCount; ++i) {
    if (c.valves[i].active) m = static_cast<uint16_t>(m | (1u << i));
  }
  return m;
}

bool slotTempTenths(const Config& c, const TempReading* bus, uint8_t count, uint8_t slot1,
                    uint32_t nowMs, uint32_t staleMs, int32_t& tenths) {
  if (slot1 == 0 || slot1 > kTempSlotCount) return false;
  const TempSlotConfig& s = c.temps[slot1 - 1];
  const int b = findTempBus(bus, count, s.id);
  if (b < 0) return false;
  const TempReading& r = bus[b];
  if (!r.seen || !tempRawValid(r.raw) || elapsedMs(nowMs, r.lastSeenMs) > staleMs) return false;
  tenths = static_cast<int32_t>(r.raw) + s.offset;
  return true;
}

bool slotVoltValue(const Config& c, const VoltReading* bus, uint8_t count, uint8_t i,
                   uint32_t nowMs, uint32_t staleMs, double& value) {
  if (i >= kVoltSlotCount) return false;
  const VoltSlotConfig& s = c.volts[i];
  const int b = findVoltBus(bus, count, s.id);
  if (b < 0) return false;
  const VoltReading& r = bus[b];
  if (!r.seen || !vadValid(r.vad) || elapsedMs(nowMs, r.lastSeenMs) > staleMs) return false;
  value = (static_cast<double>(r.vad) / 100.0 + s.offset) * s.factor;
  return true;
}

bool tempAssignedToValve(const ValveState* valves, uint8_t slot1) {
  if (valves == nullptr || slot1 == 0) return false;
  for (uint8_t v = 0; v < kValveCount; ++v) {
    if (valves[v].sensorSlot[0] == slot1 || valves[v].sensorSlot[1] == slot1) return true;
  }
  return false;
}

bool tempPublished(const Config& c, const ValveState* valves, uint8_t i) {
  if (i >= kTempSlotCount) return false;
  const TempSlotConfig& s = c.temps[i];
  return s.active && !isZero(s.id) &&
         (c.mqtt.allTemps || !tempAssignedToValve(valves, static_cast<uint8_t>(i + 1)));
}

bool voltPublishedMqtt(const Config& c, uint8_t i) {
  return i < kVoltSlotCount && !isZero(c.volts[i].id);
}

bool voltAnnounced(const Config& c, uint8_t i) {
  return voltPublishedMqtt(c, i) && c.volts[i].active;
}

int findTempBus(const TempReading* bus, uint8_t count, const OneWireId& id) {
  if (bus == nullptr || isZero(id)) return -1;
  const uint8_t n = std::min(count, kTempSlotCount);
  for (uint8_t b = 0; b < n; ++b) {
    if (bus[b].id == id) return b;
  }
  return -1;
}

int findVoltBus(const VoltReading* bus, uint8_t count, const OneWireId& id) {
  if (bus == nullptr || isZero(id)) return -1;
  const uint8_t n = std::min(count, kVoltSlotCount);
  for (uint8_t b = 0; b < n; ++b) {
    if (bus[b].id == id) return b;
  }
  return -1;
}

size_t sensorTopicSegment(const Config& c, ItemKind kind, uint8_t idx0, int busIndex, char* out,
                          size_t cap) {
  if (out == nullptr || cap == 0) return 0;
  out[0] = '\0';
  const char* name = nullptr;
  const char* topic = nullptr;
  if (kind == ItemKind::Temp && idx0 < kTempSlotCount) {
    name = c.temps[idx0].name;
    topic = c.temps[idx0].topic;
  } else if (kind == ItemKind::Volt && idx0 < kVoltSlotCount) {
    name = c.volts[idx0].name;
    topic = c.volts[idx0].topic;
  } else {
    return 0;
  }
  if (name[0] != '\0' || topic[0] != '\0') return itemSegment(c, kind, idx0, out, cap);
  if (busIndex < 0) return 0;
  const int n = snprintf(out, cap, "%d", busIndex + 1);
  if (static_cast<size_t>(n) < cap) return static_cast<size_t>(n);
  out[0] = '\0';
  return 0;
}

bool publishedTarget(const ValveState& v, bool separate, uint8_t& out) {
  if (!separate || v.fsOverride) {
    if (!v.desiredValid) return false;
    out = v.desired;
    return true;
  }
  if (v.source == TargetSource::Restored && v.sync != TargetSync::Synced) return false;
  if (!v.stmTargetKnown) return false;
  out = v.stmTarget;
  return true;
}

bool valveProblem(const ValveState& v) { return (v.health & kProblemMask) != 0; }

bool stmOnline(LinkState s) { return s == LinkState::Up || s == LinkState::Degraded; }

size_t valveDisplayName(const char* configName, uint8_t idx0, char* out, size_t cap) {
  if (out == nullptr || cap == 0) return 0;
  int n;
  if (configName != nullptr && configName[0] != '\0') {
    n = snprintf(out, cap, "%s", configName);
  } else {
    n = snprintf(out, cap, "Valve %u", static_cast<unsigned>(idx0) + 1u);
  }
  if (static_cast<size_t>(n) < cap) return static_cast<size_t>(n);
  out[0] = '\0';
  return 0;
}

// ---------------------------------------------------------------- CalibEndTracker

uint16_t CalibEndTracker::observe(const ValveState* valves, const LocalTime& now) {
  if (valves == nullptr) return 0;
  uint16_t cal = 0;
  for (uint8_t i = 0; i < kValveCount; ++i) {
    if (valves[i].calibrating) cal = static_cast<uint16_t>(cal | (1u << i));
  }
  const uint16_t endedNow = observed_ ? static_cast<uint16_t>(calibrating_ & ~cal) : 0;
  for (uint8_t i = 0; i < kValveCount; ++i) {
    if ((endedNow >> i) & 1u) end_[i] = now;
  }
  ended_ = static_cast<uint16_t>(ended_ | endedNow);
  dirty_ = static_cast<uint16_t>(dirty_ | endedNow);
  calibrating_ = cal;
  observed_ = true;
  return endedNow;
}

bool CalibEndTracker::ended(uint8_t valve) const {
  return valve < kValveCount && ((ended_ >> valve) & 1u) != 0;
}

const LocalTime& CalibEndTracker::end(uint8_t valve) const {
  return end_[valve < kValveCount ? valve : 0];
}

bool CalibEndTracker::dirty(uint8_t valve) const {
  return valve < kValveCount && ((dirty_ >> valve) & 1u) != 0;
}

void CalibEndTracker::clearDirty(uint8_t valve) {
  if (valve < kValveCount) dirty_ = static_cast<uint16_t>(dirty_ & ~(1u << valve));
}

}  // namespace vdm
