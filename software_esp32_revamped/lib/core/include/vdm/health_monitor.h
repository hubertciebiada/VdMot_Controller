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
  //  - ValveBlocked and CalibFailed carry arg2 = the failsafe position the
  //    STM drives the blocked valve to (protocol 3 FS_BLOCKED flag, fsPct not
  //    hold), else -1; ValveFailed arg2 = the fault (protocol 3), else -1;
  //  - CalibStarted arg1 2 when the calibration is an automatic retry
  //    (autoRetry), else 0;
  //  - kHealthStrokeShort set: CalibStrokeShort (arg1 min(openCount,
  //    closeCount), arg2 setMinCounts());
  //  - earlyStops / cmdRejected increase: EarlyStop / CmdRejected;
  //  - kHealthTargetUnconfirmed set (sync -> Failed): TargetNotConfirmed;
  //    kHealthStale set: ValveStale (arg1 = the model's default staleness
  //    threshold in s); either flag clearing: ValveRecovered with arg1 0
  //    and arg2 = the cleared flag bits;
  //  - desired target change by the web, MQTT or an assembly: TargetSet
  //    (Info); adopting the STM's target (source Stm) and restoring one
  //    (Restored) are silent;
  //  - any other status change: ValveStateChanged (Debug).
  // A calibration outcome replaces the status event it implies: CalibFailed
  // suppresses ValveBlocked, and CalibOk/CalibFailed suppress
  // ValveRecovered. Nothing is emitted for the first snapshot of a valve
  // (known false->true) except Blocked/Failed/NoValve if the valve starts in
  // that state (and TargetSet). When more than maxOut events arise, the
  // most severe kinds (status, calibration) come first.
  size_t onValve(uint8_t valve, const ValveState& before, const ValveState& after,
                 bool active, Event* out, size_t maxOut);
  // minCounts of the STM (gmotc), arg2 of CalibStrokeShort.
  void setMinCounts(uint16_t minCounts) { minCounts_ = minCounts; }

  // Protocol 3 system events from two gstax statuses; before == nullptr (or
  // not protocol 3): the first status since the ESP booted or the STM
  // rebooted. Only when after.v3:
  //  - safeMode 0 -> 1, or 1 on the first status: StmSafeMode (arg1
  //    wdgResets); 1 -> 0: StmSafeModeEnded;
  //  - cfgEvents increased, or > 0 on the first status while the STM uptime
  //    is below 600 s (an old repair is not reported again after a
  //    restart): StmConfigRepaired (arg1 cfgFlags, arg2 cfgEvents);
  //  - uartOre + uartFe + uartNe + rxDropped increased (the first status is
  //    the baseline, a lower total re-baselines silently, at most one event
  //    per 10 min with the latest totals): StmUartErrors (arg1 ore + fe + ne,
  //    arg2 rxDropped);
  //  - sysFlags PROTECT_SUSPENDED set, not set before (or on the first
  //    status): StmProtectionSuspended.
  size_t onStmStatus(const StmStatus* before, const StmStatus& after, uint32_t nowMs, Event* out,
                     size_t maxOut);

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
  CounterTrack uart_;
  uint16_t minCounts_ = 0;
};

}  // namespace vdm
