// MQTT topic tree, command-topic parsing, payload formatting and publish
// scheduling. The compat part is byte-exact with the legacy firmware
// (software_esp32/src/mqtt.cpp); the new part is described in DESIGN.md
// "MQTT". Hardware-free.
#pragma once

#include <stddef.h>
#include <stdint.h>

#include "vdm/common.h"
#include "vdm/config.h"

namespace vdm {

constexpr size_t kTopicMax = 127;    // chars without NUL; every built topic fits
constexpr size_t kSegmentMax = kItemNameMax;  // valve/sensor segment chars

// Settings that shape topics (subset of MqttConfig).
struct TopicContext {
  // The MQTT root (mqttRootTopic(): mqtt.rootTopic, else the station),
  // validated with isSafeName.
  char station[kStationNameMax + 1] = {0};
  bool pathAsRoot = false;  // legacy publishPathAsRoot: leading '/'
  bool separate = true;     // legacy publishSeparate: "/value" + "/set"
};

// main = ["/"] + (station != "" ? station + "/" : "VdMotFBH/")
// Returns chars written (0 = does not fit).
size_t buildMainTopic(const TopicContext& ctx, char* out, size_t cap);

// Valve/sensor segment: name with ' ' -> '_', or the 1-based index when the
// name is empty ("3"). idx0 is 0-based.
size_t buildSegment(const char* name, uint8_t idx0, char* out, size_t cap);

// A segment as it may appear in a topic: 1..kSegmentMax bytes without NUL,
// '+' and '#'; '/' only between two non-empty parts (topic overrides such as
// "Bad/WC" of the legacy firmware, config.h itemSegment()).
bool topicSegmentValid(const char* seg, size_t len);

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
  // ---- 2.1 (internal numbering, no external meaning) ----
  ValveRequested,        // <main>valves/<V>/requested         desired target, suffix rule
  ValveSync,             // <main>valves/<V>/sync              targetSyncName(), suffix rule
  ValveFailsafe,         // <main>valves/<V>/failsafe          off/lease/blocked, suffix rule
  ValveProblem,          // <main>valves/<V>/problem           "1"/"0", suffix rule
  StmStatus,             // <main>stm/status                   "online"/"offline", retained
  Failsafe,              // <main>failsafe                     "1"/"0", retained
  DiagStmVersion,        // <main>diag/stm/version
  DiagStmStarted,        // <main>diag/stm/started             UTC timestamp
  DiagStmLease,          // <main>diag/stm/lease               off/running/expired
  DiagStmSafeMode,       // <main>diag/stm/safeMode            "1"/"0"
  DiagMqttEventsSuppressed,  // <main>diag/mqtt/eventsSuppressed
  DiagMqttCommandsRejected,  // <main>diag/mqtt/commandsRejected
  DiagCalibrationNext,   // <main>diag/calibration/next        UTC timestamp or ""
  CmdValveCalibrate,     // <main>cmd/valves/<V>/calibrate     commands (buttons), never retained
  CmdCalibrate,          // <main>cmd/calibrate
  CmdRestart,            // <main>cmd/restart
  CmdStmReset,           // <main>cmd/stmReset
  CmdDetect,             // <main>cmd/detect
  CmdStop,               // <main>cmd/stop                     protocol >= 3
  CmdStmSafeExit,        // <main>cmd/stmSafeExit              protocol >= 3
};
constexpr uint8_t kTopicCount = 55;

// True for topics that take the "/value" suffix with `separate`: the legacy
// tree plus valves/<V>/{requested,sync,failsafe,problem}.
bool topicIsCompat(Topic t);
// Retain flag: Status, StmStatus, Failsafe and DiagCalibrationActive are
// always retained; Events, DiagValveProfile and the cmd/ topics never; every
// other topic follows publishRetained.
bool topicRetained(Topic t, bool publishRetained);

// Full publish topic. `segment` is the valve/sensor segment for per-item
// topics (ignored otherwise; required non-empty for them). Returns chars
// written, 0 on overflow or missing segment.
size_t buildTopic(const TopicContext& ctx, Topic t, const char* segment, char* out, size_t cap);

// Subscription filter for the valve target command of one valve:
// separate: "<main>valves/<V>/target/set", else "<main>valves/<V>/target".
size_t buildTargetCommandTopic(const TopicContext& ctx, const char* segment, char* out,
                               size_t cap);

// HA status topic "<prefix>/status" (HA birth / last will). 0 when it does
// not fit or the prefix is null or empty.
size_t buildHaStatusTopic(const char* prefix, char* out, size_t cap);

// Subscriptions of one connection.
constexpr size_t kMaxSubscriptions = 29;
struct Subscription {
  char filter[kTopicMax + 1] = {0};
  uint8_t qos = 0;
};
// Modes Mqtt and MqttHa: the two target command filters with a '+' for the
// valve (separate: "<main>valves/+/target/set" and ".../target/set/set"; not
// separate: "<main>valves/+/target" and ".../target/set"), QoS 1; then
// "<main>cmd/#", QoS 0; then for every valve whose segment contains '/' the
// same two filters spelled out (QoS 1: '+' matches one level only); MqttHa
// also "homeassistant/status" and "<haPrefix>/status" when the prefix
// differs (QoS 1). Off: none. Entries that do not fit `cap` or kTopicMax are
// skipped. `segments` may be null (no spelled-out filters).
size_t buildSubscriptions(const TopicContext& ctx, MqttMode mode, const char* haPrefix,
                          const char segments[kValveCount][kSegmentMax + 1], Subscription* out,
                          size_t cap);

enum class InboundKind : uint8_t {
  None,            // not one of our topics
  HaStatus,        // "homeassistant/status" or "<haPrefix>/status"
  Target,          // <main>valves/<seg>/target command
  CalibrateValve,  // <main>cmd/valves/<seg>/calibrate
  CalibrateAll,    // <main>cmd/calibrate
  Restart,         // <main>cmd/restart
  StmReset,        // <main>cmd/stmReset
  Detect,          // <main>cmd/detect
  StopAll,         // <main>cmd/stop
  StmSafeExit,     // <main>cmd/stmSafeExit
  UnknownCommand,  // <main>cmd/<anything else>
};
struct InboundTopic {
  InboundKind kind = InboundKind::None;
  int8_t valve = -1;       // Target / CalibrateValve: 0..11; -1 = the segment names no valve
  bool stateForm = false;  // Target without separate: exactly "<main>valves/<seg>/target"
};
// Classifies an inbound topic of exactly `len` bytes (NUL bytes -> None):
//  - "homeassistant/status" and "<haPrefix>/status" -> HaStatus;
//  - a leading '/' on the topic and on main is ignored for the rest;
//  - "<main>valves/<seg>/" + ("target/set" | "target/set/set") when
//    separate, + ("target" | "target/set") when not -> Target;
//  - "<main>cmd/valves/<seg>/calibrate" -> CalibrateValve; the fixed cmd/
//    topics -> their kind; any other "<main>cmd/..." -> UnknownCommand.
// <seg> is matched against segments[0..11] (itemSegment() of every valve,
// active or not, '/' allowed) in index order, first match wins; otherwise a
// strict number 1..12 without leading zero selects that valve; any other
// non-empty segment gives valve -1; an empty one gives None.
InboundTopic parseInboundTopic(const TopicContext& ctx, const char* haPrefix, const char* topic,
                               size_t len, const char segments[kValveCount][kSegmentMax + 1]);

enum class TargetPayload : uint8_t { Ok, Empty, NotNumber, OutOfRange, Stop };
// Target payload: optional surrounding spaces/tabs/CR/LF, at most 16 bytes in
// all; "OPEN" -> 100, "CLOSE" -> 0, "STOP" -> Stop (out unchanged);
// otherwise 1+ digits, optionally '.' or ',' and 1+ digits, converted with
// roundTargetPercent() (exact range 0..100, half up: 43.5 -> 44, 99.5 -> 100,
// 100.01 -> OutOfRange). Rejects: sign, exponent, hex, "nan"/"inf", a
// separator without digits on either side, a second separator.
TargetPayload parseTargetPayload(const char* p, size_t len, uint8_t& out);
// Button payload: exactly "PRESS" (HA default payload_press).
bool parseButtonPayload(const char* p, size_t len);

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

// Legacy publish cadence, per item slot:
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
