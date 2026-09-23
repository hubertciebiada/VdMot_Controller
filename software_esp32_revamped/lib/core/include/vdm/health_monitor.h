// Health decisions: turns model/link changes into Events (with per-condition
// dedup), rate-limits events for MQTT, decides when an OTA image is healthy
// enough to be marked valid, and when a lost network justifies an ESP
// restart. Hardware-free.
#pragma once

#include <stddef.h>
#include <stdint.h>

#include "vdm/event_log.h"
#include "vdm/link_policy.h"
#include "vdm/valve_model.h"

namespace vdm {

// Maximum events one call may produce (callers size their arrays with it).
constexpr size_t kMaxEventsPerUpdate = 8;

// Legacy MQTT common/state value (0 ok, 1 info, 2 error), derived:
//  2 when the link is Down, or an ACTIVE valve has kHealthBlocked or
//    kHealthFailed;
//  1 when the link is Unknown/Degraded/Booting, or an active valve has any
//    other health flag;
//  0 otherwise (Suspended during flashing counts as 1).
uint8_t systemState(LinkState link, const ValveState* valves, uint8_t count, uint16_t activeMask);

class HealthMonitor {
 public:
  // Compares two snapshots of one valve and appends events to `out` (at most
  // maxOut, never more than kMaxEventsPerUpdate). Rules (DESIGN.md "Health"):
  //  - status transitions into Blocked/Failed/NoValve(active only) raise
  //    ValveBlocked/ValveFailed/ValveNoValve once; leaving them raises
  //    ValveRecovered;
  //  - calibrating false->true: CalibStarted; true->false: CalibOk when the
  //    new status is Idle, CalibFailed when Blocked; calibRetries increase
  //    during calibration: CalibRetry;
  //  - earlyStops / cmdRejected increase: EarlyStop / CmdRejected;
  //  - sync -> Failed: TargetNotConfirmed; kHealthStale set: ValveStale;
  //  - desired target change: TargetSet (Info);
  //  - any other status change: ValveStateChanged (Debug).
  // Nothing is emitted for the first snapshot of a valve (known false->true)
  // except Blocked/Failed/NoValve if the valve starts in that state.
  size_t onValve(uint8_t valve, const ValveState& before, const ValveState& after,
                 bool active, Event* out, size_t maxOut);

  // LinkUp / LinkDegraded / LinkDown on state transitions (Booting and
  // Suspended are silent: the reset/flash events describe them).
  size_t onLink(LinkState before, LinkState after, uint8_t consecutiveTimeouts,
                Event* out, size_t maxOut);

  // STM counters (gstat, or the ESP line assembler): StmRxOverflow /
  // StmParseErrors when the total increased, at most once per 10 min per
  // counter (the latest total is reported).
  size_t onStmCounters(uint32_t rxOverflowTotal, uint32_t parseErrTotal, uint8_t side,
                       uint32_t nowMs, Event* out, size_t maxOut);

  // Temperature sensor in config slot (1-based) went invalid / valid again.
  // `wasValid`/`isValid` from tempRawValid(); first observation is silent.
  size_t onTempSensor(uint8_t slot, bool wasKnown, bool wasValid, bool isValid, int16_t raw,
                      Event* out, size_t maxOut);

 private:
  uint32_t lastRxOverflowEventMs_[2] = {0, 0};
  uint32_t lastParseErrEventMs_[2] = {0, 0};
  bool rxOverflowReported_[2] = {false, false};
  bool parseErrReported_[2] = {false, false};
};

// Rate limit for events published to MQTT (<root>/events), architecture
// §3.4: at most one event per (valve, code) per perKeyMs, and at most
// maxPerHour events in any rolling hour (token bucket: capacity maxPerHour,
// one token per 3600000/maxPerHour ms). Events below Warning are rejected
// unless eventIsCalibrationOutcome(). Fixed table of kKeys (valve, code)
// entries; when full the least recently used entry is recycled.
class EventRateLimiter {
 public:
  static constexpr size_t kKeys = 64;
  explicit EventRateLimiter(uint32_t perKeyMs = 600000, uint16_t maxPerHour = 30);
  // True when the event may be published now (and books it).
  bool allow(const Event& e, uint32_t nowMs);
  uint32_t suppressed() const { return suppressed_; }

 private:
  struct Key {
    uint16_t code;
    uint8_t valve;
    bool used;
    uint32_t lastMs;
  };
  uint32_t perKeyMs_;
  uint16_t maxPerHour_;
  uint32_t tokensMilli_;   // tokens x 1000
  uint32_t lastRefillMs_;
  bool started_ = false;
  Key keys_[kKeys];
  uint32_t suppressed_ = 0;
};

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

 private:
  uint32_t confirmMs_, networkOnlyMs_, giveUpMs_;
  bool pending_ = false;
  uint32_t startMs_ = 0;
  bool healthy_ = false;
  uint32_t healthySinceMs_ = 0;
};

// Network watchdog (legacy netConnTO): the ESP restarts after `minutes`
// consecutive minutes without an IP address. 0 disables it. The first
// minutes after boot count as well (a device that never gets a link keeps
// restarting every N minutes, like the legacy firmware).
class NetWatchdog {
 public:
  void configure(uint8_t minutes);
  // Call once per second. True = restart the ESP now (returned once).
  bool update(bool netUp, uint32_t nowMs);

 private:
  uint8_t minutes_ = 0;
  bool down_ = true;
  uint32_t downSinceMs_ = 0;
  bool fired_ = false;
  bool started_ = false;
};

}  // namespace vdm
