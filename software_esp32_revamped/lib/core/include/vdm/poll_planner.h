// Decides which STM request to issue next when no user/config request is
// waiting: re-sync sequence, one-shot reads, and periodic polling with a
// per-category cadence. Hardware-free; no queue of its own (the caller
// enqueues the returned line at Priority::Poll, or Config for re-sync).
#pragma once

#include <stddef.h>
#include <stdint.h>

#include "vdm/stm_codec.h"

namespace vdm {

// Binding cadence (DESIGN.md "Poll cadence"). All values are the period of
// one item, e.g. every active valve is read every valveActiveMs.
struct PollCadence {
  uint16_t valveBusyMs = 500;       // valve moving/calibrating or target not synced
  uint16_t valveActiveMs = 2000;    // active valve, idle
  uint16_t valveInactiveMs = 30000; // inactive valve (still shown on the dashboard)
  uint16_t tempDataMs = 10000;      // each DS18 bus index (goned)
  uint16_t voltDataMs = 10000;      // each DS2438 bus index (gowvd)
  uint16_t sensorCountMs = 30000;   // gonec / gowvc count check
  uint16_t statusMs = 10000;        // gstat (v2 only)
  uint32_t versionMs = 300000;      // gvers re-read (both protocols)
};

// Re-sync steps in the order they are issued. v2-only steps are skipped on a
// v1 STM; Gproto is always first and its timeout means "v1".
enum class ResyncStep : uint8_t {
  Proto,          // gproto
  Version,        // gvers
  HwId,           // ghwin
  MotorChars,     // gmotc
  LearnMovements, // gtlnm
  Breakaway,      // gcalx (v2)
  TempList,       // gonec 255
  VoltList,       // gowvc 255
  ValveSensors,   // gvlon 255
  ValveStates,    // gvlst (v1 only; v2 uses gvlvx)
  Targets,        // gtgtp 0..11 (v1) / gvlvx 0..11 (v2): adopt STM targets
  Done,
};

class PollPlanner {
 public:
  explicit PollPlanner(const PollCadence& cadence = PollCadence());

  // 0 = unknown (probe pending), 1 = v1, 2 = v2. Selects gvlvd vs gvlvx and
  // enables gstat/gcalx/gprof.
  void setProtocol(uint8_t proto);
  uint8_t protocol() const { return proto_; }
  void setActiveMask(uint16_t mask);                 // bit i = valve i active
  void setValveBusy(uint8_t valve, bool busy);       // from ValveModel
  void setSensorCounts(uint8_t temps, uint8_t volts);  // from gonec/gowvc (clamped to 34/8)

  // Starts the full re-sync sequence (ESP boot, STM reboot detected, link
  // recovered, after flashing). Periodic polling of valves continues
  // interleaved: at most every second request is a re-sync step.
  void requestResync();
  bool resyncActive() const { return step_ != ResyncStep::Done; }
  ResyncStep resyncStep() const { return step_; }

  // One-shot reads, each coalesced (a pending one is not queued twice).
  void requestTempList();              // gonec 255 (count changed / after stons)
  void requestVoltList();              // gowvc 255
  void requestValveSensors();          // gvlon 255 (after stvls+masns)
  void requestMotorParams();           // gmotc + gtlnm (+ gcalx on v2)
  void requestTarget(uint8_t valve);   // gtgtp (v1) / gvlvx (v2) read-back
  void requestProfile(uint8_t valve);  // gprof (v2 only; ignored on v1)

  // Produces the next request, or false when nothing is due. Priority:
  // re-sync step > target read-backs > profiles > sensor/param one-shots >
  // most overdue periodic item (busy valves, active valves, gstat, goned,
  // gowvd, counts, inactive valves, gvers). Marks the item as handed out.
  bool next(uint32_t nowMs, RequestLine& out);

  // Result of a request produced by next(). Re-sync steps and one-shots are
  // repeated until they succeed (a failed step is retried after the other
  // due items, never more than once per valveActiveMs). A Proto timeout sets
  // protocol 1. Periodic items are simply due again after their period.
  void onResult(const RequestLine& request, bool ok, uint32_t nowMs);

 private:
  PollCadence cadence_;
  uint8_t proto_ = 0;
  uint16_t activeMask_ = 0;
  uint16_t busyMask_ = 0;
  uint8_t tempCount_ = 0;
  uint8_t voltCount_ = 0;
  ResyncStep step_ = ResyncStep::Done;
  uint8_t targetStepValve_ = 0;
  bool lastWasResync_ = false;
  uint16_t targetReadMask_ = 0;
  uint16_t profileMask_ = 0;
  bool wantTempList_ = false;
  bool wantVoltList_ = false;
  bool wantValveSensors_ = false;
  bool wantMotorParams_ = false;
  uint32_t valveDueMs_[kValveCount] = {0};
  uint8_t tempIndex_ = 0;
  uint8_t voltIndex_ = 0;
  uint32_t tempDueMs_ = 0;
  uint32_t voltDueMs_ = 0;
  uint32_t countDueMs_ = 0;
  uint32_t statusDueMs_ = 0;
  uint32_t versionDueMs_ = 0;
};

}  // namespace vdm
