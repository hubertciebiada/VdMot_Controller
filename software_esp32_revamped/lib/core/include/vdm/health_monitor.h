// Health decisions: turns model/link changes into Events (with per-condition
// dedup). Hardware-free.
#pragma once

#include <stddef.h>
#include <stdint.h>

#include "vdm/event_log.h"
#include "vdm/link_policy.h"
#include "vdm/valve_model.h"

namespace vdm {

// Maximum events one call may produce (callers size their arrays with it).
constexpr size_t kMaxEventsPerUpdate = 8;

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
  //  - kHealthTargetUnconfirmed set (sync -> Failed): TargetNotConfirmed;
  //    kHealthStale set: ValveStale (arg1 = the model's default staleness
  //    threshold in s); either flag clearing: ValveRecovered with arg1 0
  //    and arg2 = the cleared flag bits;
  //  - desired target change by the web or MQTT: TargetSet (Info); adopting
  //    the STM's target (source Stm) is silent;
  //  - any other status change: ValveStateChanged (Debug).
  // A calibration outcome replaces the status event it implies: CalibFailed
  // suppresses ValveBlocked, and CalibOk/CalibFailed suppress
  // ValveRecovered. Nothing is emitted for the first snapshot of a valve
  // (known false->true) except Blocked/Failed/NoValve if the valve starts in
  // that state (and TargetSet). When more than maxOut events arise, the
  // most severe kinds (status, calibration) come first.
  size_t onValve(uint8_t valve, const ValveState& before, const ValveState& after,
                 bool active, Event* out, size_t maxOut);

  // LinkUp / LinkDegraded / LinkDown on state transitions (Booting and
  // Suspended are silent: the reset/flash events describe them).
  size_t onLink(LinkState before, LinkState after, uint8_t consecutiveTimeouts,
                Event* out, size_t maxOut);

  // STM counters (gstat, or the ESP line assembler): StmRxOverflow /
  // StmParseErrors when the total increased, at most once per 10 min per
  // counter and side (the latest total is reported). side 0 = ESP (counts
  // from 0 at ESP boot), 1 = STM: its first observation after ESP boot is
  // only the baseline. A total below the last one (STM reboot) re-baselines
  // silently. side > 1 is ignored.
  size_t onStmCounters(uint32_t rxOverflowTotal, uint32_t parseErrTotal, uint8_t side,
                       uint32_t nowMs, Event* out, size_t maxOut);

  // Temperature sensor in config slot (1-based, 1..34) went invalid / valid
  // again. `wasValid`/`isValid` from tempRawValid(); first observation
  // (wasKnown false) is silent. TempSensorFailed carries arg2 = raw; the
  // caller may put the sensor id into the event text.
  size_t onTempSensor(uint8_t slot, bool wasKnown, bool wasValid, bool isValid, int16_t raw,
                      Event* out, size_t maxOut);

 private:
  // Rate-limited "total increased" tracking of one counter on one side.
  struct CounterTrack {
    bool baselined = false;
    uint32_t total = 0;        // last total reported (or the baseline)
    bool reported = false;     // lastEventMs is meaningful
    uint32_t lastEventMs = 0;
  };
  static bool counterIncreased(CounterTrack& t, uint32_t total, uint32_t nowMs);
  CounterTrack rxOverflow_[2];
  CounterTrack parseErr_[2];
};

}  // namespace vdm
