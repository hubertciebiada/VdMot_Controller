// Structured event log: event codes (single registry for the whole
// firmware), severities, a fixed ring buffer with sequence numbers and
// filtered reads, and text/JSON formatting. Hardware-free, not thread-safe
// (the logger glue wraps it in a mutex).
#pragma once

#include <stddef.h>
#include <stdint.h>

#include "vdm/common.h"

namespace vdm {

class JsonWriter;

// Ordered: a filter "minimum severity" keeps everything >= it.
enum class Severity : uint8_t { Debug = 0, Info = 1, Warning = 2, Error = 3, Critical = 4 };
const char* severityName(Severity s);  // "debug","info","warning","error","critical"
bool parseSeverity(const char* s, size_t len, Severity& out);
// RFC 5424 severity number for syslog: 7,6,4,3,2.
uint8_t syslogSeverity(Severity s);

// Event codes. Numbers are part of the external API (MQTT events, HTTP,
// log files): never renumber, only append. arg1/arg2/text meaning per code
// is binding (DESIGN.md "Event codes"); "-" = unused (0 / "").
enum class EventCode : uint16_t {
  // system 1xx
  Boot = 100,               // arg1 reset reason (esp_reset_reason), arg2 boot count; text fw version
  ConfigImported = 101,     // arg1 keys imported, arg2 keys rejected
  ConfigSaved = 102,        // arg1 config revision; text source ("web","import","factory")
  ConfigDefaults = 103,     // stored config unreadable -> defaults; arg1 reason code
  FsFormatted = 104,        // LittleFS mount failed and was formatted
  EspOtaStarted = 105,      // arg1 image size (0 unknown)
  EspOtaDone = 106,         // arg1 image size; text new version if known
  EspOtaFailed = 107,       // arg1 Update error code
  AppMarkedValid = 108,     // OTA image confirmed; arg1 seconds after boot
  RebootRequested = 109,    // arg1 RebootReason, arg2 detail (net watchdog: outage minutes;
                            // rollback: missing OTA checks bit0 net, bit1 http, bit2 stm)
  LowHeap = 110,            // arg1 free heap, arg2 min free heap
  TimeSynced = 111,         // arg1 step in seconds (clamped to int32)
  CalibTimeMissing = 112,   // scheduled calibration skipped: no valid time; arg1 slot key (yyyymmdd)
  FactoryResetSkipped = 113,  // factory reset pin still set after a reset: settings kept
  StackLow = 114,           // arg1 min free bytes, arg2 stack bytes; text task name
  HeapFragmented = 115,     // arg1 largest free block, arg2 free heap
  LogWriteFailed = 116,     // arg1 step (1 open, 2 write, 3 rotate, 4 size limit), arg2 events lost
  ImportDropped = 117,      // arg1 PI valves, arg2 dropped mask (1 pi, 2 window, 4 messenger,
                            // 8 ds18Timeout, 16 legacyFailsafe); text "ignored <n> keys"
  ConfigRestored = 118,     // arg1 primary DecodeResult (0 = NVS empty)
  ConfigRepaired = 119,     // arg1 repair mask, arg2 repairs; text first key path
  ConfigNewerSchema = 120,  // arg1 base schema, arg2 unknown settings kept
  FilesRemoved = 121,       // arg1 files, arg2 KiB; text file name or "legacy images"
  // network / MQTT / web 2xx
  NetUp = 200,              // arg1 interface (1 eth, 2 wifi); text IP
  NetDown = 201,            // arg1 interface
  MqttConnected = 202,
  MqttDisconnected = 203,   // arg1 PubSubClient state
  MqttCommandRejected = 204,// arg1 valve (1-based, 0 unknown), arg2 TargetPayload of a payload
                            // reject; text reason
  HaDiscoverySent = 205,    // arg1 configs published, arg2 deletes published
  AuthFailed = 206,         // arg1 failures in the current window; text client IP
  NetTrialStarted = 207,    // arg1 window s; text new address
  NetTrialConfirmed = 208,  // arg1 s since the network came up, arg2 1 = by a newer change
  NetTrialReverted = 209,   // arg1 reason (1 not confirmed, 2 no network, 3 interrupted, 4 user,
                            // 5 trial not stored),
                            // arg2 0 ok / -1 revert failed; text previous address
  NetUnreachable = 210,     // arg1 s since the last evidence, arg2 NetEvidence
  NetReachable = 211,       // arg1 outage s
  NetInterfaceRestart = 212,  // arg1 outage s, arg2 1 eth, 2 wifi, 3 both
  RequestRefused = 213,     // arg1 verdict (1 host, 2 origin, 3 header, 4 content type); text client IP
  AuthLocked = 214,         // arg1 lock s, arg2 lockout level; text client IP
  // STM link 3xx
  LinkUp = 300,
  LinkDegraded = 301,       // arg1 consecutive timeouts
  LinkDown = 302,           // arg1 consecutive timeouts
  StmResetByPolicy = 303,   // arg1 consecutive timeouts, arg2 span s
  StmResetByUser = 304,
  StmRebootDetected = 305,  // arg1 cause (1 gstat uptime, 2 gstat resets, 3 v1 heuristic, 4 link recovered)
  StmVersion = 306,         // arg1 protocol (1/2), arg2 hw id; text version
  StmIncompatible = 307,    // text version (below VDM_MIN_STM_VERSION)
  StmRxOverflow = 308,      // arg1 new total (STM rxOverflow or ESP line overflows), arg2 side (0 esp,1 stm)
  StmParseErrors = 309,     // arg1 new total, arg2 side (0 esp,1 stm)
  StmQueueFull = 310,       // arg1 command enum value
  StmFlashStarted = 311,    // arg1 image size; text image name
  StmFlashDone = 312,       // arg1 duration ms; text new STM version
  StmFlashFailed = 313,     // arg1 FlashError, arg2 failing address; text phase
  FailsafeActive = 314,     // arg1 valve mask, arg2 source (1 STM lease, 2 ESP emulation)
  FailsafeEnded = 315,      // arg1 seconds, arg2 source
  RegulatorLost = 316,      // arg1 1 broker down, 2 HA offline
  RegulatorBack = 317,      // arg1 seconds lost
  LeaseConfigFailed = 318,  // arg1 1 no reply, 2 rejected, 3 read-back differs; arg2 attempts
  StmSafeMode = 319,        // arg1 watchdog resets
  StmSafeModeEnded = 320,
  StmConfigRepaired = 321,  // arg1 cfgFlags, arg2 cfgEvents
  StmUartErrors = 322,      // arg1 ore + fe + ne, arg2 rxDropped
  StmEepromWaitTimeout = 323,  // arg1 waited ms, arg2 1 STM reset, 2 flash, 3 ESP restart;
                               // text "" or "stm task silent"
  TargetsRestored = 324,    // arg1 valves, arg2 1 RTC, 2 NVS
  StmProtectionSuspended = 325,
  // valves 4xx (valve = 0-based index in Event::valve)
  TargetSet = 400,          // arg1 new target, arg2 TargetSource
  ValveStateChanged = 401,  // arg1 old status, arg2 new status (Debug unless noted in DESIGN)
  ValveBlocked = 402,       // arg1 calibRetries, arg2 failsafe position (-1 none)
  ValveFailed = 403,        // arg2 ValveFault (protocol 3), -1 unknown
  ValveNoValve = 404,       // active valve reports open circuit
  ValveRecovered = 405,     // arg1 previous bad status
  CalibStarted = 406,       // arg1 1 = scheduled, 2 = automatic retry, 0 = manual/STM
  CalibOk = 407,            // arg1 openCount, arg2 closeCount
  CalibRetry = 408,         // arg1 calibRetries
  CalibFailed = 409,        // arg1 calibRetries (ends in Blocked), arg2 failsafe position (-1 none)
  EarlyStop = 410,          // arg1 earlyStops total, arg2 stop reason
  CmdRejected = 411,        // arg1 cmdRejected total
  TargetNotConfirmed = 412, // arg1 desired, arg2 attempts
  ValveStale = 413,         // arg1 seconds since last data
  ServiceMoveDone = 414,    // arg1 counted counts, arg2 stop reason
  CalibStrokeShort = 415,   // arg1 min(openCount, closeCount), arg2 minCounts
  // sensors 5xx (valve = kNoValve, arg1 config slot 1-based)
  TempSensorFailed = 500,   // arg2 raw value; text sensor id
  TempSensorRecovered = 501,
  SensorCountChanged = 502, // arg1 new count, arg2 kind (0 temp, 1 volt)
  VoltSensorFailed = 503,   // arg2 raw vad
  // calibration schedule 6xx
  ScheduledCalibration = 600,  // arg1 slot key (yyyymmdd), arg2 minutes late
  ScheduledCalibrationFailed = 601,  // arg1 slot key, arg2 1 no reply, 2 not sent, 3 no result,
                                     // 4 STM unsupported
  ScheduledCalibrationMissed = 602,  // arg1 slot key, arg2 attempts
};
// "boot","config_imported",... snake_case, never null ("unknown" for others).
const char* eventCodeName(EventCode c);
// Default severity per code (DESIGN.md table).
Severity eventDefaultSeverity(EventCode c);
// True for codes that are "calibration outcomes" (CalibOk/Retry/Failed):
// they go to MQTT even though CalibOk is Info (architecture R3).
bool eventIsCalibrationOutcome(EventCode c);

// Which events <main>events publishes: No, WarnPlus (when the logged
// severity is Warning or above) or Always.
enum class EventMqtt : uint8_t { No, WarnPlus, Always };
EventMqtt eventMqtt(EventCode c);  // No for unknown codes
// Names of every code with eventMqtt() != No, in registry order (the HA
// event entity's event_types). Writes at most cap names, returns the total.
size_t eventMqttNames(const char** out, size_t cap);

// RebootRequested arg1 (the numbers are the existing contract).
enum class RebootReason : uint8_t { User = 0, Ota = 1, NetWatchdog = 2, FactoryReset = 3,
                                    Rollback = 4, NetRevert = 5 };

constexpr size_t kEventTextMax = 23;  // chars, without NUL

struct Event {
  uint32_t seq = 0;          // assigned by EventLog::append, 1-based, never 0 for stored events
  uint32_t uptimeS = 0;      // seconds since ESP boot
  uint32_t epoch = 0;        // UTC seconds, 0 when the clock was not valid
  EventCode code = EventCode::Boot;
  Severity severity = Severity::Info;
  uint8_t valve = kNoValve;  // 0..11 or kNoValve
  int32_t arg1 = 0;
  int32_t arg2 = 0;
  char text[kEventTextMax + 1] = {0};
};

// Convenience constructor; text is truncated to kEventTextMax (never fails).
Event makeEvent(EventCode code, Severity sev, uint8_t valve, int32_t arg1, int32_t arg2,
                const char* text);

// eventMqtt() is Always, or WarnPlus and the logged severity >= Warning.
bool eventReachesMqtt(const Event& e);

struct EventFilter {
  uint32_t sinceSeq = 0;              // return events with seq > sinceSeq
  Severity minSeverity = Severity::Debug;
  uint8_t valve = kNoValve;           // kNoValve = all valves and system events; a valve
                                      // index also matches kAllValves events
};

// Ring buffer over caller-provided storage. When full the oldest event is
// overwritten; sequence numbers keep increasing so readers detect the gap
// (firstSeq() > sinceSeq + 1).
class EventLog {
 public:
  EventLog(Event* storage, size_t capacity);

  // Stores a copy, assigns and returns its seq (never 0). capacity 0 -> the
  // event is counted as dropped and 0 is returned.
  uint32_t append(const Event& e);

  size_t size() const { return size_; }
  size_t capacity() const { return cap_; }
  uint32_t firstSeq() const;   // seq of the oldest stored event, 0 if empty
  uint32_t lastSeq() const;    // seq of the newest stored event, 0 if empty
  uint32_t dropped() const { return dropped_; }  // overwritten or rejected

  // Copies up to maxOut matching events, oldest first. Returns the count.
  // `nextSince` receives the seq to pass as sinceSeq next time (the seq of
  // the last event examined, so filtered-out events are not re-scanned).
  size_t read(const EventFilter& f, Event* out, size_t maxOut, uint32_t& nextSince) const;
  // Event with this seq if still stored.
  bool get(uint32_t seq, Event& out) const;

  void clear();

 private:
  Event* buf_;
  size_t cap_;
  size_t head_ = 0;  // index of the oldest event
  size_t size_ = 0;
  uint32_t nextSeq_ = 1;
  uint32_t dropped_ = 0;
};

// Human-readable message for an event without the prefix, e.g.
// "valve 3: calibration ok (oc 3120, cc 3350)" ("all valves: " for
// kAllValves). Valve numbers are 1-based in text. Returns chars written
// (truncated to fit, always NUL-terminated).
size_t formatEventMessage(const Event& e, char* out, size_t cap);
// The message of one event standing for the same event on several valves:
// with 2 or more bits in valveMask (bit v = valve v) the prefix is
// "valves 1, 3, 12: " (ascending, bits above 11 ignored); otherwise the same
// as formatEventMessage().
size_t formatEventMessageMulti(const Event& e, uint16_t valveMask, char* out, size_t cap);
// One log/syslog line:
// "#<seq> <iso8601 or +<uptime>s> <SEV> <code_name>[ v<n>] <message>"
size_t formatEventLine(const Event& e, char* out, size_t cap);
// JSON object: {"seq":..,"t":<epoch|null>,"up":..,"sev":"warning","code":410,
// "name":"early_stop","valve":3|null,"a1":..,"a2":..,"text":"..","msg":".."}
// (valve 1-based; null for system and all-valves events). Returns jw.ok().
bool writeEventJson(JsonWriter& jw, const Event& e);
// The <main>events payload: writeEventJson()'s keys plus "event_type" (=
// name, HA event entity) after "name"; with 2 or more bits in valveMask
// "valve":null,"valves":[1-based ascending] and the message of
// formatEventMessageMulti(). Returns jw.ok().
bool writeMqttEventJson(JsonWriter& jw, const Event& e, uint16_t valveMask);
// UTC "2026-09-25T08:13:40+00:00" (HA timestamp sensors). Returns 25; 0 and
// out = "" (cap >= 1) when cap < 26.
size_t formatUtcTimestamp(uint32_t epoch, char* out, size_t cap);

}  // namespace vdm
