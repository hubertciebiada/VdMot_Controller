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

  // 0 = unknown (probe pending), 1 = v1, 2 = v2 (higher values count as 2).
  // Selects gvlvd vs gvlvx and enables gstat/gcalx/gprof; below 2 pending
  // v2-only one-shots are dropped.
  void setProtocol(uint8_t proto);
  uint8_t protocol() const { return proto_; }
  void setActiveMask(uint16_t mask);                 // bit i = valve i active
  void setValveBusy(uint8_t valve, bool busy);       // from ValveModel
  void setSensorCounts(uint8_t temps, uint8_t volts);  // from gonec/gowvc (clamped to 34/8)

  // Starts the full re-sync sequence (ESP boot, STM reboot detected, link
  // recovered, after flashing). Periodic polling of valves continues
  // interleaved: when a re-sync step and another item are both due they
  // alternate, so at most every second request is a re-sync step while other
  // work is waiting (with nothing else due, steps go back to back).
  // The protocol goes back to 0 (the STM may have been re-flashed; v1
  // commands work on both, so nothing v2-only is sent until `gproto`
  // answers). Pending one-shots the sequence re-reads anyway (lists, motor
  // params, targets) and v2-only ones (profiles) are dropped.
  void requestResync();
  bool resyncActive() const { return step_ != ResyncStep::Done; }
  ResyncStep resyncStep() const { return step_; }
  // True when the request last returned by next() was a re-sync step (the
  // caller queues those at Priority::Config, everything else at Poll).
  bool lastWasResync() const { return lastWasResync_; }

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
  // gowvd, counts, inactive valves, gvers; ties in that order). Marks the
  // item as handed out. A handed-out step or one-shot whose result never
  // arrives (request evicted or dropped by an STM reset) is handed out again
  // after kLostRequestMs.
  bool next(uint32_t nowMs, RequestLine& out);

  // Result of a request produced by next(). Re-sync steps and one-shots are
  // repeated until they succeed (a failed step is retried after the other
  // due items, never more than once per valveActiveMs). A Proto timeout sets
  // protocol 1 (the caller applies a `gproto` reply with setProtocol() before
  // reporting it here). Periodic items are simply due again after their
  // period. Results of requests the planner did not hand out are ignored.
  void onResult(const RequestLine& request, bool ok, uint32_t nowMs);

  // A parsed gvers reply. A revamped STM (isRevamped) always speaks v2, so
  // when the probe had timed out (protocol 1, e.g. the probe hit the STM's
  // start-up window) the re-sync is restarted once to probe again. Armed
  // again only after a successful v2 probe, so an STM that really stays
  // silent on gproto is not probed in a loop.
  void onVersion(bool revamped);

  static constexpr uint16_t kLostRequestMs = 10000;

 private:
  // One-shot items: bit i of pending_/inflight_.
  static constexpr uint8_t kItemTarget = 0;                         // 0..11
  static constexpr uint8_t kItemProfile = kItemTarget + kValveCount;  // 12..23
  static constexpr uint8_t kItemTempList = kItemProfile + kValveCount;
  static constexpr uint8_t kItemVoltList = kItemTempList + 1;
  static constexpr uint8_t kItemValveSensors = kItemVoltList + 1;
  static constexpr uint8_t kItemMotorChars = kItemValveSensors + 1;
  static constexpr uint8_t kItemLearnMovements = kItemMotorChars + 1;
  static constexpr uint8_t kItemBreakaway = kItemLearnMovements + 1;
  static constexpr uint8_t kItemCount = kItemBreakaway + 1;
  static constexpr uint32_t kResyncCovered =
      ((1u << kValveCount) - 1u) << kItemTarget | 1u << kItemTempList | 1u << kItemVoltList |
      1u << kItemValveSensors | 1u << kItemMotorChars | 1u << kItemLearnMovements |
      1u << kItemBreakaway;
  static constexpr uint32_t kV2Items = ((1u << kValveCount) - 1u) << kItemProfile |
                                       1u << kItemBreakaway;

  // A hold makes an item ineligible until holdFor ms after heldAt.
  struct Hold {
    uint32_t heldAt = 0;
    uint16_t holdFor = 0;
  };
  static bool holdExpired(const Hold& h, uint32_t nowMs);
  static void hold(Hold& h, uint32_t nowMs, uint16_t ms);

  bool buildItem(uint8_t item, RequestLine& out) const;
  bool buildStep(RequestLine& out) const;
  bool valveRequest(uint8_t valve, RequestLine& out) const;
  void skipStepsForProtocol();
  void advanceStep();
  bool nextResync(uint32_t nowMs, RequestLine& out);
  bool nextOneShot(uint32_t nowMs, RequestLine& out);
  bool nextPeriodic(uint32_t nowMs, RequestLine& out);
  void setPending(uint8_t item);
  void prime(uint32_t nowMs);

  PollCadence cadence_;
  uint8_t proto_ = 0;
  bool reprobed_ = false;
  uint16_t activeMask_ = 0;
  uint16_t busyMask_ = 0;
  uint8_t tempCount_ = 0;
  uint8_t voltCount_ = 0;

  ResyncStep step_ = ResyncStep::Done;
  uint8_t stepValve_ = 0;  // Targets step: next valve
  bool stepInflight_ = false;
  Hold stepHold_;
  bool lastWasResync_ = false;

  uint32_t pending_ = 0;
  uint32_t inflight_ = 0;
  Hold itemHold_[kItemCount];

  bool primed_ = false;
  uint32_t valveLastMs_[kValveCount] = {0};
  uint8_t tempIndex_ = 0;
  uint8_t voltIndex_ = 0;
  uint32_t tempLastMs_ = 0;
  uint32_t voltLastMs_ = 0;
  uint32_t tempCountLastMs_ = 0;
  uint32_t voltCountLastMs_ = 0;
  uint32_t statusLastMs_ = 0;
  uint32_t versionLastMs_ = 0;
};

}  // namespace vdm
