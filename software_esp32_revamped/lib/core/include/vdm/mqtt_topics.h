// MQTT topic tree, command-topic parsing, payload formatting and publish
// scheduling. The compat part is byte-exact with the legacy firmware
// (specs/03-mqtt-ha-compat.md §3-§6); the new part follows architecture §3.4.
// Hardware-free.
#pragma once

#include <stddef.h>
#include <stdint.h>

#include "vdm/common.h"

namespace vdm {

constexpr size_t kTopicMax = 127;    // chars without NUL; every built topic fits
constexpr size_t kSegmentMax = kItemNameMax;  // valve/sensor segment chars

// Settings that shape topics (subset of MqttConfig).
struct TopicContext {
  char station[kStationNameMax + 1] = {0};  // validated with isSafeName
  bool pathAsRoot = false;  // legacy publishPathAsRoot: leading '/'
  bool separate = true;     // legacy publishSeparate: "/value" + "/set"
};

// main = ["/"] + (station != "" ? station + "/" : "VdMotFBH/")
// Returns chars written (0 = does not fit).
size_t buildMainTopic(const TopicContext& ctx, char* out, size_t cap);

// Valve/sensor segment: name with ' ' -> '_', or the 1-based index when the
// name is empty ("3"). idx0 is 0-based.
size_t buildSegment(const char* name, uint8_t idx0, char* out, size_t cap);

enum class Topic : uint8_t {
  // ---- compat (legacy tree; "/value" appended when ctx.separate) ----
  CommonIp,              // <main>common/ip
  CommonState,           // <main>common/state
  CommonUptime,          // <main>common/uptime
  CommonMessage,         // <main>common/message
  ValveTarget,           // <main>valves/<V>/target        (also the command topic)
  ValveState,            // <main>valves/<V>/state
  ValveCalibDate,        // <main>valves/<V>/calibration/date
  ValveCalibRepetitions, // <main>valves/<V>/calibration/repetitions
  ValveMeanCurrent,      // <main>valves/<V>/diag/meanCurrrent   (sic, three r)
  ValveOpenCount,        // <main>valves/<V>/diag/openCount
  ValveCloseCount,       // <main>valves/<V>/diag/closeCount
  ValveDeadZoneCount,    // <main>valves/<V>/diag/deadZoneCount
  ValveMoves,            // <main>valves/<V>/diag/moves
  ValveTemp1,            // <main>valves/<V>/temp1
  ValveTemp2,            // <main>valves/<V>/temp2
  ValveActual,           // <main>valves/<V>/actual            NEW, follows the compat suffix rule
  TempId,                // <main>temps/<T>/id
  TempValue,             // <main>temps/<T>/value   ("/value/value" when separate, legacy)
  VoltId,                // <main>sensors/<S>/id
  VoltValue,             // <main>sensors/<S>/value
  VoltUnit,              // <main>sensors/<S>/unit
  // ---- new (never suffixed) ----
  DiagValveLastMove,     // <main>diag/valves/<V>/lastMove    JSON
  DiagValveEarlyStops,   // <main>diag/valves/<V>/earlyStops
  DiagValveCmdRejected,  // <main>diag/valves/<V>/cmdRejected
  DiagValveCalState,     // <main>diag/valves/<V>/calState
  DiagValveProfile,      // <main>diag/valves/<V>/profile     JSON, not retained
  DiagStmProto,          // <main>diag/stm/proto
  DiagStmUptime,         // <main>diag/stm/uptime
  DiagStmResets,         // <main>diag/stm/resets
  DiagStmRxOverflow,     // <main>diag/stm/rxOverflow
  DiagStmParseErr,       // <main>diag/stm/parseErr
  DiagStmLink,           // <main>diag/stm/link
  DiagCalibrationActive, // <main>diag/calibration/active     "0"/"1", retained
  Events,                // <main>events                      JSON, not retained
  Status,                // <main>status                      "online"/"offline", retained LWT
};
constexpr uint8_t kTopicCount = 35;

// True for topics in the legacy tree (suffix rule applies).
bool topicIsCompat(Topic t);
// Retain flag: compat topics use cfg publishRetained; Status and
// DiagCalibrationActive are always retained; Events and DiagValveProfile
// never; other diag topics follow publishRetained.
bool topicRetained(Topic t, bool publishRetained);

// Full publish topic. `segment` is the valve/sensor segment for per-item
// topics (ignored otherwise; required non-empty for them). Returns chars
// written, 0 on overflow or missing segment.
size_t buildTopic(const TopicContext& ctx, Topic t, const char* segment, char* out, size_t cap);

// Subscription filter for the valve target command of one valve:
// separate: "<main>valves/<V>/target/set", else "<main>valves/<V>/target".
size_t buildTargetCommandTopic(const TopicContext& ctx, const char* segment, char* out,
                               size_t cap);

// Parses an inbound topic (spec 03 §4.1, hardened):
//  - a leading '/' on the inbound topic and on main is ignored;
//  - must be "<main>valves/<seg>/target" + ("/set" | "/set/set") when
//    separate, or + ("" | "/set") when not;
//  - <seg> 1..kSegmentMax chars; matched against segments[0..11] (the
//    buildSegment() of every valve, active or not) in index order, first match
//    wins; otherwise a strict number 1..12 selects that valve.
// Returns the 0-based valve or -1 (unknown topic / no such valve).
int parseTargetCommandTopic(const TopicContext& ctx, const char* topic, size_t len,
                            const char segments[kValveCount][kSegmentMax + 1]);

enum class TargetPayload : uint8_t { Ok, Empty, NotNumber, OutOfRange };
// Target payload: optional surrounding spaces/tabs/CR/LF, then an integer
// 0..100, optionally followed by ".0..." zeros only ("55", "55.0", "55.00").
// Also accepts exactly "OPEN" -> 100 and "CLOSE" -> 0 (HA valve entity).
// Rejects: sign, hex, exponent, "nan"/"inf", fractions, > 100, > 16 chars.
TargetPayload parseTargetPayload(const char* p, size_t len, uint8_t& out);

// ---------------------------------------------------------------- payloads

// Temperature tenths -> "21.5" / "-0.5" / "21,5" (germanComma). Invalid raw
// (tempRawValid false) -> "failed". Returns chars written.
size_t formatTemp(int32_t tenths, bool valid, bool germanComma, char* out, size_t cap);
// Volt value with 3 decimals ("12.345", comma option), "failed" when !valid.
size_t formatVolt(double value, bool valid, bool germanComma, char* out, size_t cap);
// Legacy uptime "%ud %u:%02u:%02u", e.g. "3d 4:05:09".
size_t formatUptime(uint32_t seconds, char* out, size_t cap);
// Valve state: plainText -> valveStatusText(status); else decimal status.
size_t formatValveState(uint8_t status, bool plainText, char* out, size_t cap);
// System state 0 ok / 1 info / 2 error; plain "ok","info","error", >= 3 "".
size_t formatSystemState(uint8_t state, bool plainText, char* out, size_t cap);
// Legacy calibration date "%A, %B %d.%Y %H:%M:%S" with English names,
// e.g. "Monday, September 21.2026 14:03:05"; "Failed to obtain time" when
// !t.valid.
size_t formatCalibDate(const LocalTime& t, char* out, size_t cap);
// Legacy uint32 counters are published like itoa(int): values > INT32_MAX
// print negative (compat). Returns chars written.
size_t formatLegacyCounter(uint32_t v, char* out, size_t cap);

// ---------------------------------------------------------------- scheduling

// Legacy publish cadence (spec 03 §6), per item slot:
//  - after (re)connect: everything once (full publish);
//  - periodic mode (!onChange): full publish every publishIntervalMs;
//  - on-change mode: an item goes out when changed and minDelayMs has passed
//    since its last publish, and at least every publishIntervalMs (item
//    heartbeat); a full publish every publishIntervalMs as well.
// Slots are caller-defined small integers (DESIGN.md: common 0, valves
// 1..12, temps 13..46, volts 47..54, stm diag 55).
class PublishScheduler {
 public:
  static constexpr uint8_t kSlots = 64;
  struct Params {
    bool onChange = true;
    uint32_t publishIntervalMs = 10000;  // >= 2000
    uint32_t minDelayMs = 5000;          // <= publishIntervalMs
  };
  void configure(const Params& p);
  void onConnected(uint32_t nowMs);  // forces the next full publish
  // Full publish due now (first after connect, or periodic)? Consumes it.
  bool takeFullPublish(uint32_t nowMs);
  // Item decision in on-change mode; in periodic mode always false (items
  // only go out with the full publish). Consumes the decision (marks the
  // slot published at nowMs) when it returns true.
  bool takeItem(uint8_t slot, bool changed, uint32_t nowMs);
  // After a full publish every slot counts as just published.
  void markAllPublished(uint32_t nowMs);

 private:
  Params p_;
  bool forceFull_ = true;
  uint32_t lastFullMs_ = 0;
  uint32_t lastItemMs_[kSlots] = {0};
  bool itemSeen_[kSlots] = {false};
};

}  // namespace vdm
