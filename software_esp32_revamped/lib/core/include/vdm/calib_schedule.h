// Scheduled calibration: weekday bitmask + local hour:minute -> "calibrate
// all valves now" decision, once per calendar-day slot, tolerant to missing
// time, reboots and DST/NTP jumps. Hardware-free.
#pragma once

#include <stddef.h>
#include <stdint.h>

#include "vdm/common.h"
#include "vdm/config.h"
#include "vdm/link_policy.h"
#include "vdm/stm_codec.h"

namespace vdm {

// Slot key of a local calendar date: yyyymmdd (e.g. 20260923). 0 when `t`
// is not valid: valid flag clear, date outside 2000..9999 or impossible
// (Feb 30), hour/minute/second out of range, or wday not matching the date.
uint32_t calibSlotKey(const LocalTime& t);

// UTC epoch of local date `slotKey` (a valid yyyymmdd, e.g. from nextSlot())
// at hour:minute, taking the UTC offset of `ref` (its local fields against its
// epoch). 0 when slotKey is 0 or `ref` is not valid. Glue calls it again with
// the local time at the first result so a DST change before the slot counts.
int64_t calibSlotEpoch(uint32_t slotKey, uint8_t hour, uint8_t minute, const LocalTime& ref);

enum class CalibDecision : uint8_t {
  None,           // nothing to do
  Fire,           // send "staln 255" now; the slot is booked only by onResult(true)
  SkippedNoTime,  // today's slot window passed (or is passing) without valid time; reported once
  NoResult,       // the last Fire got no result within kResultTimeoutMs: a failed attempt
  Missed,         // the window of an attempted, unconfirmed slot closed (once per slot)
};

// Why a scheduled calibration was not confirmed (arg2 of
// ScheduledCalibrationFailed).
enum class CalibFailure : uint8_t { None = 0, NoReply = 1, NotSent = 2, NoResult = 3, Unsupported = 4 };
// "none","no_reply","not_sent","no_result","stm_unsupported"; "unknown" out of range
const char* calibFailureName(CalibFailure f);

// Rules (DESIGN.md "Calibration schedule"):
//  - A slot exists on local dates whose weekday bit is set in dayMask.
//  - The slot fires when local time is in [hh:mm, hh:mm + graceMinutes)
//    (same local date) and the slot key is newer than the last booked key.
//    A Fire opens an attempt; only onResult(true) books the slot. A failed
//    attempt (onResult(false), or NoResult after kResultTimeoutMs) fires
//    again kRetryMs later while the window is open; a window that closes
//    without success gives Missed once.
//    A spring-forward gap that skips hh:mm still fires at the first minute
//    after it; a fall-back repetition does not fire twice (same date key).
//  - An NTP step backwards to an earlier date never re-fires a booked date
//    (keys only move forward).
//  - The booked key is persisted by glue (restoreLastSlot at boot), so an ESP
//    reboot inside the window does not calibrate again.
//  - While time is invalid nothing fires. If time is still invalid when the
//    scheduler is evaluated and the ESP has been up longer than
//    noTimeReportMs, SkippedNoTime is returned once per ESP boot.
//  - dayMask == 0 (bit 7 is ignored) or hour > 23 or minute > 59: never
//    fires. graceMinutes 0 is treated as 1.
//  - A local time that calibSlotKey() rejects counts as "no valid time".
//  - Stale bookings: a booked key in the future (clock was wrong) is
//    discarded when a valid time more than 2 days before it is seen.
class CalibScheduler {
 public:
  static constexpr uint32_t kRetryMs = 600000;        // next attempt in the same window
  static constexpr uint32_t kResultTimeoutMs = 60000;

  explicit CalibScheduler(uint16_t graceMinutes = 120, uint32_t noTimeReportMs = 3600000);

  // A key that is not a valid yyyymmdd date is ignored (-> 0).
  void restoreLastSlot(uint32_t slotKey);
  uint32_t lastSlot() const { return lastSlot_; }

  // Call about every 10 s. `upMs` is time since ESP boot.
  CalibDecision evaluate(const CalibScheduleConfig& cfg, const LocalTime& now, uint32_t upMs);

  // Result of the last Fire. ok: the slot is booked (lastSlot() = the
  // attempted key), returns true (the glue persists it). !ok: the next Fire
  // comes kRetryMs later while the window is open. Ignored (false) when no
  // attempt is pending.
  bool onResult(bool ok, uint32_t upMs);
  bool attemptPending() const { return pending_; }
  uint32_t attemptSlot() const { return attemptKey_; }  // key of the current/last attempt, 0 none
  uint8_t attempts() const { return attempts_; }        // attempts for attemptSlot()

  // Minutes the last firing was late relative to hh:mm (for the event).
  uint16_t lateMinutes() const { return lateMinutes_; }

  // yyyymmdd of the next date whose slot will still fire (today while its
  // window is open and it is not booked yet), looking up to 7 days ahead;
  // 0 when the schedule is off or the time is not valid. For /api/status.
  uint32_t nextSlot(const CalibScheduleConfig& cfg, const LocalTime& now) const;

 private:
  uint16_t graceMinutes_;
  uint32_t noTimeReportMs_;
  uint32_t lastSlot_ = 0;
  bool noTimeReported_ = false;
  uint16_t lateMinutes_ = 0;
  bool pending_ = false;
  uint32_t attemptKey_ = 0;
  uint32_t attemptAtMs_ = 0;
  uint8_t attempts_ = 0;
  bool holdValid_ = false;     // no Fire before kRetryMs after holdFromMs_
  uint32_t holdFromMs_ = 0;
  bool booked_ = false;        // the attempted key was confirmed
  uint32_t missedKey_ = 0;     // Missed reported for this key
};

// ---------------------------------------------------------------- STM learn time

// Default STM calibration interval (stlnt) while the ESP schedule is off.
constexpr uint32_t kStmLearnTimeDefaultS = 604800;
// 0 (the STM's own time trigger off) while the ESP schedule is enabled
// (dayMask & 0x7F != 0, hour <= 23, minute <= 59), else kStmLearnTimeDefaultS.
uint32_t stmLearnTime(const CalibScheduleConfig& cfg);

// Keeps the STM's time trigger (stlnt) in line with the ESP schedule.
// Protocol 3: gtlnt; a value other than the desired one -> stlnt -> gtlnt to
// verify; equal: nothing until setDesired() or a new session. Protocols 1/2
// (no read-back, not persisted by those STMs): desired 0 -> one stlnt 0 per
// session; desired != 0 -> one stlnt <desired> only after a stlnt 0 went out
// in this session. Protocol 0: nothing. Failures retry after kRetryMs.
class LearnTimeSync {
 public:
  static constexpr uint32_t kRetryMs = 60000;
  static constexpr uint32_t kLostRequestMs = 10000;  // no completion -> counts as a failure

  void setDesired(uint32_t seconds);
  void setProtocol(uint8_t proto);  // a change starts a new session
  void onStmReboot();               // new session
  // Next request (Priority::Config), false when nothing is due; one in flight.
  bool next(uint32_t nowMs, RequestLine& out);
  void onCompletion(const RequestLine& req, Outcome o, const Reply* rep, uint32_t nowMs);
  bool haveStmValue() const { return haveStm_; }
  uint32_t stmValue() const { return stmValue_; }

 private:
  enum class Step : uint8_t { Read, Write, Verify, Done };
  void newSession();
  void fail(uint32_t nowMs);

  bool haveDesired_ = false;
  uint32_t desired_ = 0;
  uint8_t proto_ = 0;
  Step step_ = Step::Read;
  bool inFlight_ = false;
  RequestLine inFlightReq_;
  uint32_t inFlightValue_ = 0;  // stlnt value in flight
  uint32_t inFlightAtMs_ = 0;
  bool holdValid_ = false;
  uint32_t holdFromMs_ = 0;
  bool haveStm_ = false;
  uint32_t stmValue_ = 0;
  // protocols 1/2, this session
  bool zeroSent_ = false;
  bool sentValid_ = false;
  uint32_t sentValue_ = 0;
};

}  // namespace vdm
