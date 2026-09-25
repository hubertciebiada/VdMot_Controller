// Values published over MQTT that are derived from the config and the STM
// snapshot. Hardware-free.
#pragma once

#include <stddef.h>
#include <stdint.h>

#include "vdm/config.h"
#include "vdm/link_policy.h"
#include "vdm/valve_model.h"

namespace vdm {

// System-wide conditions besides the link and the valves.
struct SystemFlags {
  bool safeMode = false;  // the STM runs in safe mode
  bool failsafe = false;  // valves are at their failsafe positions (lease or ESP emulation)
};

// Legacy MQTT common/state value (0 ok, 1 info, 2 error), derived:
//  2 when the link is Down, the STM is in safe mode, or an ACTIVE valve has
//    kHealthBlocked or kHealthFailed;
//  1 when the link is Unknown/Degraded/Booting, the failsafe is active, or
//    an active valve has any other health flag;
//  0 otherwise (Suspended during flashing counts as 1).
uint8_t systemState(LinkState link, const ValveState* valves, uint8_t count, uint16_t activeMask,
                    const SystemFlags& f = {});

// Bit v set for every active valve of the config.
uint16_t activeValveMask(const Config& c);

// Temperature of config slot slot1 (1-based) with its offset from the bus
// readings: the reading with the slot's id that was seen, is valid and not
// older than staleMs. false = "failed" (also for an empty slot).
bool slotTempTenths(const Config& c, const TempReading* bus, uint8_t count, uint8_t slot1,
                    uint32_t nowMs, uint32_t staleMs, int32_t& tenths);
// Value of volt slot i (0-based) in its unit ((vad / 100 + offset) * factor),
// same rules.
bool slotVoltValue(const Config& c, const VoltReading* bus, uint8_t count, uint8_t i,
                   uint32_t nowMs, uint32_t staleMs, double& value);
// The STM assigned temp slot slot1 (1-based) to a valve (sensor 1 or 2).
bool tempAssignedToValve(const ValveState* valves, uint8_t slot1);
// temps/<T>/… of slot i: active, id set, and allTemps or not assigned to a valve.
bool tempPublished(const Config& c, const ValveState* valves, uint8_t i);
// E23: sensors/<S>/… for every configured volt slot (id set), active or not.
bool voltPublishedMqtt(const Config& c, uint8_t i);
// Discovery announces a volt slot when it is active and has an id.
bool voltAnnounced(const Config& c, uint8_t i);

// E22: 0-based STM bus index of the reading with this id among the first
// `count` readings, -1 when absent or the id is zero.
int findTempBus(const TempReading* bus, uint8_t count, const OneWireId& id);
int findVoltBus(const VoltReading* bus, uint8_t count, const OneWireId& id);
// Topic segment of temp or volt slot idx0: itemSegment() when the slot has a
// topic override or a name; otherwise busIndex + 1 (legacy: unnamed sensors
// are published under their STM bus number). 0 (out = "") for an unnamed
// slot with busIndex -1 or when it does not fit.
size_t sensorTopicSegment(const Config& c, ItemKind kind, uint8_t idx0, int busIndex, char* out,
                          size_t cap);

// W5: value of valves/<V>/target. Not separate: the desired target (the
// topic doubles as command topic). Separate: the STM's target as read back
// (stmTarget while stmTargetKnown), nothing while a restored target is not
// synced yet, or the desired target while the ESP emulates the failsafe of
// this valve (fsOverride: the STM then holds the failsafe position). false =
// nothing to publish now.
bool publishedTarget(const ValveState& v, bool separate, uint8_t& out);

constexpr uint16_t kProblemMask = kHealthBlocked | kHealthFailed | kHealthNoValve | kHealthStale |
                                  kHealthTargetUnconfirmed | kHealthTempFailed;
bool valveProblem(const ValveState& v);  // health & kProblemMask
bool stmOnline(LinkState s);             // Up or Degraded

// Display name of valve idx0 for new HA entity names: the configured name as
// it is (UTF-8, spaces), or "Valve <n>" when empty. Returns the length (0 and
// out = "" when it does not fit).
size_t valveDisplayName(const char* configName, uint8_t idx0, char* out, size_t cap);

// End of the last calibration per valve (legacy calibration/date), observed
// from the snapshots: a valve whose `calibrating` falls from true to false
// ended one. The first observation only learns the state.
class CalibEndTracker {
 public:
  // Returns the mask of valves that ended a calibration now (stamped `now`).
  uint16_t observe(const ValveState* valves, const LocalTime& now);
  bool ended(uint8_t valve) const;
  const LocalTime& end(uint8_t valve) const;
  bool dirty(uint8_t valve) const;   // ended since the last clearDirty()
  void clearDirty(uint8_t valve);

 private:
  bool observed_ = false;
  uint16_t calibrating_ = 0;
  uint16_t ended_ = 0;
  uint16_t dirty_ = 0;
  LocalTime end_[kValveCount];
};

}  // namespace vdm
