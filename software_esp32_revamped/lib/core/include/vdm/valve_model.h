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

class SensorModel;

// Legacy text for a status value (MQTT plain-text payloads):
// 0 "", 1 "idle", 2 "opens", 3 "closes", 4 "failed", 5 "unknown",
// 6 "no valve", 7 "full open", 8 "connected", 9 "blocked", >= 10 "".
const char* valveStatusText(uint8_t status);
// Stable machine name for the new API: "nodata","idle","opening","closing",
// "failed","unknown","novalve","fullopen","connected","blocked","invalid".
const char* valveStatusKey(uint8_t status);

// Who set the desired target (event log, API).
enum class TargetSource : uint8_t {
  None = 0,
  Stm,       // adopted from the STM at (re)sync (gtgtp/gvlvx), ESP boot
  Web,       // HTTP /api/valves/{n}/target
  Mqtt,      // MQTT valves/<V>/target
  Restored,  // from the RTC/NVS copy at ESP boot
  Assembly,  // staop: 100 %, held until the next web/MQTT target
};
// "none","stm","web","mqtt","restored","assembly"; "unknown" out of range
const char* targetSourceName(TargetSource s);

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
  kHealthFailsafe = 1u << 9,          // at its lease failsafe position (STM or ESP emulation)
  kHealthStrokeShort = 1u << 10,      // calibration stroke close to minCounts
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
  uint8_t calState = 0;            // kCalState* phase
  uint8_t calFlags = 0;            // kCalFlag* bits
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
  // protocol 3 (gvlvy)
  bool hasV3 = false;
  uint16_t stmFlags = 0;             // kStmFlag*
  uint8_t fault = 0;                 // ValveFault
  // Failsafe position that applies: gvlvy (protocol 3) or the ESP config
  // (protocols 1/2, kFailsafeHold for inactive valves).
  uint8_t fsPct = kFailsafeHold;
  uint8_t drive = 0;                 // target the STM drives to
  uint32_t retryS = 0;               // s to the next automatic calibration retry
  uint8_t retries = 0;               // automatic retries since the fault began
  bool autoRetry = false;            // retries rose since the last calibration end
  // ESP failsafe emulation (protocols 1/2): fsTarget is pushed instead of desired.
  bool fsOverride = false;
  uint8_t fsTarget = 0;
  bool forcePush = false;            // push the desired target once even if the read-back equals it
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
  kChangeExtended = 1u << 8,     // calState, calFlags, earlyStops, cmdRejected
  kChangeLastMove = 1u << 9,     // moveSeq
  kChangeSync = 1u << 10,
  kChangeSensors = 1u << 11,
  kChangeHealth = 1u << 12,
  kChangeKnown = 1u << 13,
  kChangeFailsafe = 1u << 14,    // hasV3, stmFlags, fault, fsPct, drive, retries, autoRetry,
                                 // fsOverride, fsTarget
};
// Bitmask of groups that differ between two snapshots of one valve.
uint32_t diffValve(const ValveState& before, const ValveState& after);

// Failsafe state of a valve for the API and MQTT: Blocked when the STM
// reports it at its failsafe position because it is blocked (protocol 3),
// Lease when it is at its lease failsafe position (STM flag, or the ESP
// emulation overrides its target), else None.
FailsafeKind failsafeKind(const ValveState& v);
// failsafeKind(v) == FailsafeKind::Lease.
bool valveAtFailsafe(const ValveState& v);
// Calibration stroke close to the minimum: minCounts, openCount and
// closeCount known (> 0) and min(openCount, closeCount) < 1.2 x minCounts.
bool strokeNearMinimum(uint32_t openCount, uint32_t closeCount, uint16_t minCounts);

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
  // Protocol v2 polls gvlvx, which has no temperatures, instead of gvlvd:
  // temp1/temp2 come from the gvlon assignment and the goned readings. Per
  // sensor: zero id or a bad CRC -> kTempUnassigned (nothing assigned); on
  // the bus and fresh (SensorModel::tempFresh) -> the raw reading (sentinels
  // included); on the bus, read before but no longer fresh ->
  // kTempReadError; never read or not on the bus -> kTempReadError once the
  // sensors are `settled` (link up, re-sync and sensor grace over), else
  // kTempUnassigned.
  void applySensorTemps(const SensorModel& sensors, uint32_t nowMs, uint32_t maxAgeMs,
                        bool settled = false);

  // Whether a calibrating valve gets no stgtp (default true). STM 1.x acks
  // a target during a calibration but drops it; protocol v2 takes it and
  // ends the calibration at the newest target.
  void setHoldTargetsWhileCalibrating(bool hold) { holdWhileCalibrating_ = hold; }

  // Target delivery driven by the stm_link task:
  // Next stgtp to send: a valve in Pending (or Failed past failedRetryMs),
  // active, known (valve data or a target read-back), not calibrating (only
  // while held, see above), not waiting for the first read-back of a
  // restored target, pushRetryMs since the last push. Round robin over
  // valves. Moves that valve to AwaitAck and counts the attempt; `pos` is
  // pushTarget(valve). A forcePush valve is pushed although its read-back
  // equals the target (once, after an STM reboot).
  bool nextTargetPush(uint32_t nowMs, uint8_t& valve, uint8_t& pos);
  // The stgtp handed out by nextTargetPush() could not be queued: back to
  // Pending, the attempt is not counted; the next try waits pushRetryMs.
  void onTargetPushDropped(uint8_t valve, uint32_t nowMs);
  void onTargetAck(uint8_t valve, uint32_t nowMs);      // -> AwaitVerify
  void onTargetTimeout(uint8_t valve, uint32_t nowMs);  // -> Pending, or Failed after maxPushAttempts
  // Valve whose read-back is due (AwaitVerify). The caller enqueues gtgtp
  // (v1) or gvlvx (v2); the reply lands in applyTarget/applyValveEx.
  bool nextVerify(uint8_t& valve) const;

  // STM rebooted or was reset/re-flashed: every valve with a desired target
  // goes to Pending with forcePush (pushed once even when the read-back
  // equals it), STM targets become unknown, v2 counters re-baseline. Valves
  // without a desired target adopt the STM value again.
  void onStmRebooted(uint32_t nowMs);

  // Assembly (staop) was queued for valveOrAll: every addressed active valve
  // gets desired 100, source Assembly, sync AwaitAck until the staop result
  // (no stgtp is pushed: it would end the STM's assembly hold). The ESP
  // failsafe emulation never overrides such a valve.
  void setAssembly(uint8_t valveOrAll, uint32_t nowMs);
  void onAssemblyAck(uint8_t valveOrAll, uint32_t nowMs);     // AwaitAck -> AwaitVerify
  // No answer / rejected: Pending (a stgtp 100, or a staop again, follows),
  // Failed after maxPushAttempts like a target push.
  void onAssemblyFailed(uint8_t valveOrAll, uint32_t nowMs);
  // Protocol >= 2: a Pending valve with source Assembly is delivered with a
  // staop (nextAssemblyPush) instead of stgtp 100, so the STM keeps its
  // assembly hold; a protocol 3 read-back counts only with the Assembly
  // flag. Off (default): stgtp 100.
  void setAssemblyViaStaop(bool on) { assemblyViaStaop_ = on; }
  // Like nextTargetPush for the staop deliveries; the valve waits for the
  // staop result (onAssemblyAck/onAssemblyFailed).
  bool nextAssemblyPush(uint32_t nowMs, uint8_t& valve);

  // Desired target restored at ESP boot (RTC/NVS copy). Active valves only;
  // source Restored (Assembly stays Assembly); sync Pending and no push
  // before the first read-back (equal -> Synced without a push). False for
  // an invalid valve/pos or an inactive valve.
  bool restoreDesired(uint8_t valve, uint8_t pos, TargetSource src);
  // +1 whenever desiredValid/desired/source of any valve changes.
  uint32_t desiredRevision() const { return desiredRev_; }

  // ESP failsafe emulation (protocols 1/2): for valves in mask that are
  // active, have a desired target, are not in assembly and have pct <= 100,
  // the value pushed to and verified on the STM is pct[v] instead of
  // desired (fsOverride); desired and source never change. A valve whose
  // pushed value changes goes Pending (attempts 0), or Synced when the known
  // STM target already equals it. mask 0 ends every override. Valves
  // without protocol 3 data take fsPct from pct.
  void setFailsafeDrive(uint16_t mask, const uint8_t (&pct)[kValveCount]);
  uint8_t pushTarget(uint8_t valve) const;  // fsOverride ? fsTarget : desired; 0 out of range
  // From gmotc; 0 = unknown (no stroke check).
  void setMinCounts(uint16_t minCounts);
  uint16_t minCounts() const { return minCounts_; }
  // STM firmware unsupported: every valve known = false, STM data back to
  // defaults; desired targets, sources and the failsafe override stay.
  void forgetStmData();

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
  // holdOk false: a protocol 3 read-back of an Assembly valve without the
  // STM's Assembly flag (the hold is gone even when the target matches).
  void applyReadBack(uint8_t i, uint8_t target, bool holdOk = true);
  // Shared selection of nextTargetPush/nextAssemblyPush.
  bool nextDelivery(uint32_t nowMs, bool assembly, uint8_t& valve);
  void assemblyResult(uint8_t valveOrAll, bool ok);
  void markSynced(uint8_t i);
  // Recomputes health and bumps the revision when anything changed.
  void commit(uint8_t i, const ValveState& before);
  void updateHealth(uint8_t i);

  ValveModelParams params_;
  ValveState v_[kValveCount];
  uint16_t active_ = 0;
  uint8_t pushCursor_ = 0;
  bool holdWhileCalibrating_ = true;
  uint32_t staleRefMs_[kValveCount] = {};  // last gvlvd/gvlvx, or first tick while active
  bool staleRefValid_[kValveCount] = {};
  bool stale_[kValveCount] = {};           // latched until the next valve data
  bool baselined_[kValveCount] = {};       // false: next gvlvx sets the v2 counter baselines
  bool pushedOnce_[kValveCount] = {};      // lastPushMs is meaningful
  // A Failed delivery that is being retried keeps kHealthTargetUnconfirmed
  // until a read-back confirms it, so a stuck valve does not flap the flag.
  bool keepUnconfirmed_[kValveCount] = {};
  bool assemblyPending_[kValveCount] = {};  // AwaitAck waits for a staop result
  bool readBackFirst_[kValveCount] = {};    // restored target: no push before a read-back
  bool assemblyViaStaop_ = false;
  uint16_t minCounts_ = 0;
  uint32_t desiredRev_ = 0;
};

// ---------------------------------------------------------------- sensors

// Consecutive failed readings (goned 0, a sentinel or out-of-range value)
// before a reading is failed; a single failure keeps the previous reading.
constexpr uint8_t kSensorFailDebounce = 2;
// gstax tempAgeS above this: every temperature reading is stale (60 s STM
// refresh + 120 s move timeout + margin).
constexpr uint32_t kStmTempMaxAgeS = 200;

struct TempReading {
  OneWireId id;              // from goned (authoritative) or gonec list
  int16_t raw = kTempUnassigned;
  bool seen = false;         // at least one goned for this bus index
  uint32_t lastSeenMs = 0;
  uint8_t failStreak = 0;    // consecutive failed readings (saturating)
};

struct VoltReading {
  OneWireId id;
  int32_t vad = kVadFailed;  // 10 mV
  bool seen = false;
  uint32_t lastSeenMs = 0;
  uint8_t failStreak = 0;    // consecutive failed readings (saturating)
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
  // index). A good reading (valid form, tempRawValid/vadValid) stores it and
  // resets failStreak. A failed one (invalid form "goned 0", sentinel or out
  // of range) increments failStreak (saturating); below
  // kSensorFailDebounce the previous reading is kept, from then on it is
  // applied: the invalid form marks the index as not seen, a sentinel is
  // stored as the reading.
  void applyTempData(uint8_t busIndex, const TempData& d, uint32_t nowMs);
  void applyVoltData(uint8_t busIndex, const VoltData& d, uint32_t nowMs);
  // A late reply that answered no request: applied to the bus index of its
  // id (true), false when the id is not on the list (nothing changes).
  bool applyStrayTempData(const TempData& d, uint32_t nowMs);
  bool applyStrayVoltData(const VoltData& d, uint32_t nowMs);
  // gstax tempAgeS (protocol 3): seconds since the STM's last complete
  // temperature cycle, reported at nowMs.
  void setStmTempAge(uint32_t ageS, uint32_t nowMs);
  // After stons: forget everything until the next list.
  void clear();

  uint8_t tempCount() const { return tempCount_; }
  uint8_t voltCount() const { return voltCount_; }
  const TempReading& temp(uint8_t busIndex) const;   // out of range -> empty
  const VoltReading& volt(uint8_t busIndex) const;
  // Bus index of the sensor with this id, or -1.
  int findTemp(const OneWireId& id) const;
  int findVolt(const OneWireId& id) const;
  // Reading older than maxAgeMs counts as stale for display/publishing; so
  // does every reading while the STM's temperature age (reported age + whole
  // seconds since the report) is above kStmTempMaxAgeS.
  bool tempFresh(uint8_t busIndex, uint32_t nowMs, uint32_t maxAgeMs) const;

 private:
  TempReading temps_[kTempSlotCount];
  VoltReading volts_[kVoltSlotCount];
  uint8_t tempCount_ = 0;
  uint8_t voltCount_ = 0;
  bool haveStmAge_ = false;
  uint32_t stmAgeS_ = 0;
  uint32_t stmAgeAtMs_ = 0;
};

// goned/gowvd request: sets r.expect to the id the SensorModel knows at bus
// index r.arg (zero when unknown, then any id matches). Other commands are
// left alone.
void expectSensor(RequestLine& r, const SensorModel& s);

}  // namespace vdm
