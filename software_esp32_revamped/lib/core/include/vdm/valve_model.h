// ESP-side model of the STM: per-valve state built from replies, desired
// targets with acknowledged + verified delivery, derived health flags, and
// the 1-Wire sensor readings. Hardware-free. Owned by the stm_link task;
// other tasks only see copies (snapshots).
#pragma once

#include <stddef.h>
#include <stdint.h>

#include "vdm/common.h"
#include "vdm/stm_codec.h"

namespace vdm {

// Legacy text for a status value (MQTT plain-text payloads, spec 03 §5.2):
// 0 "", 1 "idle", 2 "opens", 3 "closes", 4 "failed", 5 "unknown",
// 6 "no valve", 7 "full open", 8 "connected", 9 "blocked", >= 10 "".
const char* valveStatusText(uint8_t status);
// Stable machine name for the new API: "nodata","idle","opening","closing",
// "failed","unknown","novalve","fullopen","connected","blocked","invalid".
const char* valveStatusKey(uint8_t status);

// Who set the desired target (event log, API).
enum class TargetSource : uint8_t {
  None = 0,
  Stm,      // adopted from the STM at (re)sync (gtgtp/gvlvx), ESP boot
  Web,      // HTTP /api/valves/{n}/target
  Mqtt,     // MQTT valves/<V>/target
};
const char* targetSourceName(TargetSource s);  // "none","stm","web","mqtt"

// Delivery state of the desired target (see DESIGN.md "Target delivery").
enum class TargetSync : uint8_t {
  Unknown,     // no desired target and STM target not read yet
  Synced,      // STM target verified equal to desired
  Pending,     // desired differs / needs (re)push; waits for backoff and !calibrating
  AwaitAck,    // stgtp handed out, waiting for its ack
  AwaitVerify, // acked; waiting for a read-back (gtgtp v1 / gvlvx v2)
  Failed,      // maxPushAttempts reached; retried after failedRetryMs
};
const char* targetSyncName(TargetSync s);

// Health flags (bitmask). Derived on every update; see DESIGN.md "Health".
enum HealthFlag : uint16_t {
  kHealthBlocked = 1u << 0,           // status 9
  kHealthFailed = 1u << 1,            // status 4
  kHealthNoValve = 1u << 2,           // status 6 on an ACTIVE valve
  kHealthCalibRetries = 1u << 3,      // calibRetries > 0 (last calibration needed retries)
  kHealthEarlyStop = 1u << 4,         // earlyStops increased since ESP boot (v2)
  kHealthCmdRejected = 1u << 5,       // cmdRejected increased since ESP boot (v2)
  kHealthStale = 1u << 6,             // active valve: no data for staleMs
  kHealthTargetUnconfirmed = 1u << 7, // TargetSync::Failed, and while that target is retried
  kHealthTempFailed = 1u << 8,        // an assigned sensor reports a sentinel
};

struct ValveState {
  bool known = false;          // at least one gvlvd/gvlvx/gvlst applied
  uint32_t lastSeenMs = 0;     // nowMs of the last gvlvd/gvlvx
  uint8_t status = 0;          // ValveStatus value (0 = no data yet)
  bool calibrating = false;
  uint8_t position = 0;        // % estimated by the STM
  uint16_t meanCurrent = 0;    // mA
  int16_t temp1 = kTempUnassigned;  // raw 0.1 C incl. sentinels
  int16_t temp2 = kTempUnassigned;
  uint32_t moves = 0;
  uint32_t openCount = 0;
  uint32_t closeCount = 0;
  int32_t deadZone = 0;
  uint8_t calibRetries = 0;
  // v2 only (hasExtended)
  bool hasExtended = false;
  uint8_t calState = 0;
  uint32_t earlyStops = 0;
  uint32_t cmdRejected = 0;
  uint32_t earlyStopsAtBoot = 0;   // first value seen after ESP boot / STM reboot
  uint32_t cmdRejectedAtBoot = 0;
  MoveResult lastMove;
  uint32_t moveSeq = 0;            // +1 each time lastMove changes
  // target delivery
  bool desiredValid = false;
  uint8_t desired = 0;             // 0..100
  TargetSource source = TargetSource::None;
  bool stmTargetKnown = false;
  uint8_t stmTarget = 0;
  TargetSync sync = TargetSync::Unknown;
  uint8_t pushAttempts = 0;
  uint32_t lastPushMs = 0;
  // sensor assignment reported by the STM (gvlon), resolved to config slots
  OneWireId sensorId[2];
  uint8_t sensorSlot[2] = {0, 0};  // 1-based config slot, 0 = none/unknown
  uint16_t health = 0;             // HealthFlag bits
  // +1 on every change of any field above except the lastSeenMs/lastPushMs
  // timestamps (a poll that returns the same data is not a change).
  uint32_t revision = 0;
};

// Field groups for change detection (publishing, events).
enum ValveChange : uint32_t {
  kChangeStatus = 1u << 0,       // status or calibrating
  kChangePosition = 1u << 1,
  kChangeTarget = 1u << 2,       // desired
  kChangeMeanCurrent = 1u << 3,
  kChangeTemp1 = 1u << 4,
  kChangeTemp2 = 1u << 5,
  kChangeCounters = 1u << 6,     // moves, openCount, closeCount, deadZone
  kChangeCalibRetries = 1u << 7,
  kChangeExtended = 1u << 8,     // calState, earlyStops, cmdRejected
  kChangeLastMove = 1u << 9,     // moveSeq
  kChangeSync = 1u << 10,
  kChangeSensors = 1u << 11,
  kChangeHealth = 1u << 12,
  kChangeKnown = 1u << 13,
};
// Bitmask of groups that differ between two snapshots of one valve.
uint32_t diffValve(const ValveState& before, const ValveState& after);

struct ValveModelParams {
  uint32_t staleMs = 60000;          // active valve without data -> kHealthStale
  uint8_t maxPushAttempts = 5;       // stgtp attempts before TargetSync::Failed
  uint32_t pushRetryMs = 2000;       // min spacing between stgtp to one valve
  uint32_t failedRetryMs = 300000;   // Failed -> Pending again after 5 min
};

class ValveModel {
 public:
  explicit ValveModel(const ValveModelParams& params = ValveModelParams());

  // Active valves (config). Inactive valves keep their data but never get
  // targets and never raise health flags other than Blocked/Failed.
  void setActiveMask(uint16_t mask);
  uint16_t activeMask() const { return active_; }

  // User/MQTT command. Returns false (no change) for valve >= 12, pos > 100
  // or an inactive valve. Same value as desired -> true without a re-push,
  // except that a Failed delivery is re-armed (Pending, attempts reset).
  // A new value equal to the known STM target (no stgtp in flight) is
  // Synced at once; otherwise it goes Pending.
  bool setDesiredTarget(uint8_t valve, uint8_t pos, TargetSource src, uint32_t nowMs);

  // Reply application. Out-of-range indices are ignored (codec already
  // validated). Each call updates health and revision.
  void applyValveData(const ValveData& d, uint32_t nowMs);
  void applyValveEx(const ValveEx& d, uint32_t nowMs);  // also a target read-back
  void applyValveStates(const ValveStates& s, uint32_t nowMs);  // only fills status of !known valves
  // gtgtp read-back. If no desired target exists yet (ESP boot), the STM
  // target is adopted as desired with source Stm -> Synced.
  void applyTarget(const TargetReply& t, uint32_t nowMs);
  // gvlon; ids resolved to config slots with resolveTempSlot().
  void applyValveSensors(const ValveSensors& s, const OneWireId* slotIds, uint8_t slotCount);

  // Target delivery driven by the stm_link task:
  // Next stgtp to send: a valve in Pending (or Failed past failedRetryMs),
  // active, known, not calibrating, pushRetryMs since the last push. Round
  // robin over valves. Moves that valve to AwaitAck and counts the attempt.
  bool nextTargetPush(uint32_t nowMs, uint8_t& valve, uint8_t& pos);
  void onTargetAck(uint8_t valve, uint32_t nowMs);      // -> AwaitVerify
  void onTargetTimeout(uint8_t valve, uint32_t nowMs);  // -> Pending, or Failed after maxPushAttempts
  // Valve whose read-back is due (AwaitVerify). The caller enqueues gtgtp
  // (v1) or gvlvx (v2); the reply lands in applyTarget/applyValveEx.
  bool nextVerify(uint8_t& valve) const;

  // STM rebooted or was reset/re-flashed: every valve with a desired target
  // goes to Pending (re-push), STM targets become unknown, v2 counters
  // re-baseline. Valves without a desired target adopt the STM value again.
  void onStmRebooted(uint32_t nowMs);

  // Staleness evaluation; call about once per second.
  void tick(uint32_t nowMs);

  const ValveState& valve(uint8_t i) const;  // i >= 12 returns a static empty state
  bool anyCalibrating() const;
  // Valve needs fast polling: moving (opening/closing), calibrating, or a
  // delivery in progress (Pending, AwaitAck, AwaitVerify). A Failed delivery
  // waits failedRetryMs and does not need fast polling.
  bool isBusy(uint8_t i) const;

 private:
  bool isActive(uint8_t i) const;
  void markSeen(uint8_t i, uint32_t nowMs);
  void applyReadBack(uint8_t i, uint8_t target);
  void markSynced(uint8_t i);
  // Recomputes health and bumps the revision when anything changed.
  void commit(uint8_t i, const ValveState& before);
  void updateHealth(uint8_t i);

  ValveModelParams params_;
  ValveState v_[kValveCount];
  uint16_t active_ = 0;
  uint8_t pushCursor_ = 0;
  uint32_t staleRefMs_[kValveCount] = {};  // last gvlvd/gvlvx, or first tick while active
  bool staleRefValid_[kValveCount] = {};
  bool stale_[kValveCount] = {};           // latched until the next valve data
  bool baselined_[kValveCount] = {};       // false: next gvlvx sets the v2 counter baselines
  bool pushedOnce_[kValveCount] = {};      // lastPushMs is meaningful
  // A Failed delivery that is being retried keeps kHealthTargetUnconfirmed
  // until a read-back confirms it, so a stuck valve does not flap the flag.
  bool keepUnconfirmed_[kValveCount] = {};
};

// ---------------------------------------------------------------- sensors

struct TempReading {
  OneWireId id;              // from goned (authoritative) or gonec list
  int16_t raw = kTempUnassigned;
  bool seen = false;         // at least one goned for this bus index
  uint32_t lastSeenMs = 0;
};

struct VoltReading {
  OneWireId id;
  int32_t vad = kVadFailed;  // 10 mV
  bool seen = false;
  uint32_t lastSeenMs = 0;
};

// Raw temperature is a real reading (not -500, -1270 or 850) and within
// -550..1250 (DS18B20 range).
bool tempRawValid(int16_t raw);
// vad > kVadFailed.
bool vadValid(int32_t vad);

// Bus-indexed sensor readings (index = STM bus index, NOT config slot).
// Config slots are resolved by id at render time, so a sensor keeps its
// slot when the bus order changes.
class SensorModel {
 public:
  // gonec/gowvc. A count change clears readings beyond the new count and
  // returns true (caller requests the id list). A list reply replaces ids.
  bool applyTempList(const OneWireList& l, uint32_t nowMs);
  bool applyVoltList(const OneWireList& l, uint32_t nowMs);
  // goned/gowvd for the bus index that was requested (the reply carries no
  // index). Invalid form ("goned 0") marks the index as not seen.
  void applyTempData(uint8_t busIndex, const TempData& d, uint32_t nowMs);
  void applyVoltData(uint8_t busIndex, const VoltData& d, uint32_t nowMs);
  // After stons: forget everything until the next list.
  void clear();

  uint8_t tempCount() const { return tempCount_; }
  uint8_t voltCount() const { return voltCount_; }
  const TempReading& temp(uint8_t busIndex) const;   // out of range -> empty
  const VoltReading& volt(uint8_t busIndex) const;
  // Bus index of the sensor with this id, or -1.
  int findTemp(const OneWireId& id) const;
  int findVolt(const OneWireId& id) const;
  // Reading older than maxAgeMs counts as stale for display/publishing.
  bool tempFresh(uint8_t busIndex, uint32_t nowMs, uint32_t maxAgeMs) const;

 private:
  TempReading temps_[kTempSlotCount];
  VoltReading volts_[kVoltSlotCount];
  uint8_t tempCount_ = 0;
  uint8_t voltCount_ = 0;
};

}  // namespace vdm
