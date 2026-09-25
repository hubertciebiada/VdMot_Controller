// STM application protocol codec (ESP side): builds request lines and parses
// reply lines into typed structs. Hardware-free, no state.
//
// Wire format (protocol v1 = STM 1.4.x, v2 = revamped STM 2.0, v3 = STM 2.1):
//   request : "<cmd>" { " " <arg> } " " "\r\n"  -- EVERY token, including the
//             last, is followed by one space (the v1 STM tokenizer needs it);
//             numbers are non-negative decimal; max 5 args; line <= 63 chars.
//   reply   : "<cmd>" { " " <arg> } [" "] and CR/LF (one line per request).
// Replies are matched to requests by their 5-char command (see replyMatches).
#pragma once

#include <stddef.h>
#include <stdint.h>

#include "vdm/common.h"
#include "vdm/failsafe.h"
#include "vdm/version.h"

namespace vdm {

enum class Cmd : uint8_t {
  None = 0,
  // protocol v1
  Stgtp,  // set target:            stgtp <valve> <pos>
  Gtgtp,  // get target:            gtgtp <valve>
  Gvlvd,  // valve data:            gvlvd <valve>
  Gvlst,  // all valve states:      gvlst
  Gonec,  // DS18 count / list:     gonec | gonec 255
  Goned,  // DS18 data by bus idx:  goned <idx>
  Gvlon,  // valve sensor ids:      gvlon <valve|255>
  Gowvc,  // DS2438 count / list:   gowvc | gowvc 255
  Gowvd,  // DS2438 data:           gowvd <idx>
  Stons,  // new 1-Wire search:     stons
  Stvls,  // set valve sensor ids:  stvls <valve> <id1> <id2>
  Masns,  // re-match sensor ids:   masns
  Staop,  // assembly (open fully): staop <valve|255>
  Staln,  // calibrate:             staln <valve|255>
  Stdet,  // re-detect valves:      stdet 255
  Stlnm,  // set learn movements:   stlnm <n>
  Gtlnm,  // get learn movements:   gtlnm
  Smotc,  // set motor chars:       smotc <low> <high> <sop> <minCnt> <maxRetr>
  Gmotc,  // get motor chars:       gmotc
  Gvers,  // version:               gvers
  Ghwin,  // MCU device id:         ghwin
  Eepst,  // EEPROM write state:    eepst
  Reset,  // soft reset (EEPROM-safe): reset
  // protocol v2 (revamped STM only; a v1 STM stays silent)
  Gproto,  // protocol version:     gproto
  Gvlvx,   // extended valve data:  gvlvx <valve>
  Gprof,   // current profile:      gprof <valve>
  Svmov,   // service move:         svmov <valve> <dir> <counts> <maxmA>
  Scalx,   // set breakaway:        scalx <enable> <stepPct> <maxmA>
  Gcalx,   // get breakaway:        gcalx
  Gstat,   // STM health:           gstat
  // protocol v3 (STM 2.1: gproto answers 3)
  Gvlvy,   // valve data v3:        gvlvy <valve>
  Gstax,   // STM health v3:        gstax
  Slhbt,   // lease heartbeat:      slhbt <0|1>
  Slcfg,   // set lease timeout:    slcfg <min>
  Sfspo,   // set failsafe pos.:    sfspo <valve|255> <pct|255>
  Glcfg,   // get lease config:     glcfg
  Sstop,   // stop a move:          sstop <valve|255>
  Gtlnt,   // get learn time:       gtlnt
  Ssafe,   // leave safe mode:      ssafe 0
  // protocol v1 command, appended (the numbers are external)
  Stlnt,   // set learn time:       stlnt <seconds>
};
constexpr uint8_t kCmdCount = 41;  // including None

// "stgtp" etc.; "" for None/out of range. Never null.
const char* cmdName(Cmd c);
// Exact match of `len` bytes against the 40 names; None if unknown.
Cmd cmdFromName(const char* s, size_t len);
// Lowest protocol that answers the command: 1 for the v1 commands and
// Stlnt, 2 for Gproto..Gstat, 3 for Gvlvy..Ssafe; 0 for None/out of range.
uint8_t cmdMinProtocol(Cmd c);
// True for the commands a v1 STM does not answer: cmdMinProtocol(c) >= 2.
bool cmdIsV2(Cmd c);
// True when sending the same request twice has the same effect as once
// (all get*, stgtp, stvls, stlnm, smotc, scalx, stlnt and every v3 command:
// a repeated heartbeat, stop or safe-mode exit changes nothing). Only these
// are retried by LinkPolicy. Actions (staln, staop, stdet, stons, masns,
// svmov, reset) are never retried automatically.
bool cmdIsIdempotent(Cmd c);

// ---------------------------------------------------------------- requests

constexpr size_t kRequestMaxLen = 63;  // chars incl. "\r\n", excl. NUL

// One encoded request. `valve` is the valve the request is about (0..11),
// kAllValves for "255" requests, or kNoValve. `arg` is the first numeric
// argument when there is one that is not a valve (sensor bus index, count;
// slhbt: alive, slcfg: minutes, sfspo: pct), else 0; LinkPolicy uses
// (cmd, valve, arg) to match replies.
struct RequestLine {
  char text[kRequestMaxLen + 1] = {0};
  uint8_t len = 0;
  Cmd cmd = Cmd::None;
  uint8_t valve = kNoValve;
  uint16_t arg = 0;
  // May stay unanswered by design (gproto on a v1 STM): its timeouts never
  // count toward a link failure. Set only by buildGetProto().
  bool probe = false;
  // goned/gowvd: id expected at bus index `arg`; zero = unknown, any id
  // matches (see replyMatches).
  OneWireId expect;
};

// Motor characteristics (STM EEPROM), as in gmotc/smotc.
// lowFactor/highFactor: end-stop current threshold in tenths x mean current.
struct MotorChars {
  uint8_t lowFactor = 17;
  uint8_t highFactor = 17;
  uint8_t startOnPower = 30;     // % assumed and targeted after STM boot
  uint16_t minCounts = 3000;     // min pulses of a full stroke (calibration)
  uint8_t maxCalibRetries = 2;   // extra calibration attempts before BLOCKED
  uint8_t fieldCount = 5;        // parse only: fields present in gmotc (3..5)
};

// Values smotc may send: those the v1 STM keeps across a reboot as well
// (its boot acceptance): factors 10..40, startOnPower 0..100,
// minCounts 0..60000, maxCalibRetries 0..2.
bool motorCharsValid(const MotorChars& m);

// Breakaway escalation (v2, scalx/gcalx).
struct Breakaway {
  bool enable = false;
  uint8_t stepPct = 0;  // 0..100
  uint8_t maxmA = 60;   // 20..60
};
bool breakawayValid(const Breakaway& b);

// learnAfterMovements accepted by stlnm: 0 (disabled) or 50..65534.
bool learnMovementsValid(uint32_t n);

// Builders. Each validates its arguments first; on invalid input it returns
// false and sets out = RequestLine{} (len 0). Valve indices are 0-based.
bool buildSetTarget(uint8_t valve, uint8_t pos, RequestLine& out);        // valve 0..11, pos 0..100
bool buildGetTarget(uint8_t valve, RequestLine& out);                     // 0..11
bool buildValveData(uint8_t valve, RequestLine& out);                     // 0..11
bool buildValveStates(RequestLine& out);                                  // "gvlst "
bool buildTempCount(RequestLine& out);                                    // "gonec "
bool buildTempList(RequestLine& out);                                     // "gonec 255 "
bool buildTempData(uint8_t busIndex, RequestLine& out);                   // 0..33
bool buildValveSensors(uint8_t valveOrAll, RequestLine& out);             // 0..11 or kAllValves
bool buildVoltCount(RequestLine& out);                                    // "gowvc "
bool buildVoltList(RequestLine& out);                                     // "gowvc 255 "
bool buildVoltData(uint8_t busIndex, RequestLine& out);                   // 0..7
bool buildScanOneWire(RequestLine& out);                                  // "stons "
// Zero ids unassign. Non-zero ids must pass crcValid() (the STM ignores bad
// CRCs silently, so they are rejected here).
bool buildSetValveSensors(uint8_t valve, const OneWireId& s1, const OneWireId& s2,
                          RequestLine& out);
bool buildMatchSensors(RequestLine& out);                                 // "masns "
bool buildAssembly(uint8_t valveOrAll, RequestLine& out);                 // staop
bool buildCalibrate(uint8_t valveOrAll, RequestLine& out);                // staln
bool buildDetect(RequestLine& out);                                       // "stdet 255 "
bool buildSetLearnMovements(uint32_t n, RequestLine& out);                // learnMovementsValid
bool buildGetLearnMovements(RequestLine& out);
bool buildSetMotorChars(const MotorChars& m, RequestLine& out);           // motorCharsValid, 5 args
bool buildGetMotorChars(RequestLine& out);
bool buildGetVersion(RequestLine& out);
bool buildGetHwId(RequestLine& out);
bool buildEepromState(RequestLine& out);
bool buildSoftReset(RequestLine& out);
// v2
bool buildGetProto(RequestLine& out);
bool buildValveEx(uint8_t valve, RequestLine& out);                       // 0..11
bool buildProfile(uint8_t valve, RequestLine& out);                       // 0..11
enum class MoveDir : uint8_t { Open = 0, Close = 1 };
bool buildServiceMove(uint8_t valve, MoveDir dir, uint16_t counts, uint8_t maxmA,
                      RequestLine& out);                                  // counts 1..10000, maxmA 5..60
bool buildSetBreakaway(const Breakaway& b, RequestLine& out);             // breakawayValid
bool buildGetBreakaway(RequestLine& out);
bool buildGetStatus(RequestLine& out);
// v3
bool buildValveExV3(uint8_t valve, RequestLine& out);                     // "gvlvy <v> ", 0..11
bool buildGetStatusV3(RequestLine& out);                                  // "gstax "
bool buildHeartbeat(bool alive, RequestLine& out);                        // "slhbt <0|1> "
bool buildSetLeaseTimeout(uint32_t minutes, RequestLine& out);            // leaseTimeoutValid
bool buildSetFailsafe(uint8_t valveOrAll, uint8_t pct, RequestLine& out); // 0..11|255, failsafePctValid
bool buildGetLeaseConfig(RequestLine& out);                               // "glcfg "
bool buildStop(uint8_t valveOrAll, RequestLine& out);                     // "sstop <v|255> "
bool buildGetLearnTime(RequestLine& out);                                 // "gtlnt "
bool buildLeaveSafeMode(RequestLine& out);                                // "ssafe 0 "
// v1: "stlnt <seconds> " (0 = learn-time trigger off).
bool buildSetLearnTime(uint32_t seconds, RequestLine& out);

// ---------------------------------------------------------------- replies

enum class ParseStatus : uint8_t {
  Ok,
  Empty,           // no token
  UnknownCommand,  // first token is not one of the 30 names
  BadArgCount,     // wrong number of fields for this reply
  BadNumber,       // a numeric field is not strict decimal
  OutOfRange,      // numeric field outside the documented range
  BadOneWireId,    // id field is not "hh-hh-hh-hh-hh-hh-hh-hh"
  BadFormat,       // other structure error (list separators, "c:m" pairs, ...)
  TooLong,         // line longer than kStmMaxLineLen or more than 40 tokens
};
const char* parseStatusName(ParseStatus s);

// Valve status byte (low 7 bits of gvlvd field 3). 0 is ESP-only "no data".
enum class ValveStatus : uint8_t {
  Start = 0,
  Idle = 1,
  Opening = 2,
  Closing = 3,
  Failed = 4,     // 120 s without end stop; later targets ignored by STM v1
  Unknown = 5,    // after STM boot / stdet, before A_TEST
  NoValve = 6,    // open circuit
  FullOpen = 7,   // staop pending
  Connected = 8,  // detected, waiting for calibration
  Blocked = 9,    // calibration failed
};

// gvlvd: "gvlvd v pos cur st t1 t2 mov oc cc dc cr" (11 fields, all required).
struct ValveData {
  uint8_t valve = 0;           // 0..11
  uint8_t position = 0;        // 0..100
  uint16_t meanCurrent = 0;    // mA, 0..65535
  uint8_t status = 0;          // raw & 0x7F
  bool calibrating = false;    // raw & 0x80
  int16_t temp1 = kTempUnassigned;  // 0.1 C raw incl. sentinels
  int16_t temp2 = kTempUnassigned;
  uint32_t moves = 0;
  uint32_t openCount = 0;
  uint32_t closeCount = 0;
  int32_t deadZone = 0;        // may be negative
  uint8_t calibRetries = 0;    // 0..255
};

// gvlst: "gvlst 12 s0,s1,...,s11," -- exactly 12 statuses (0..255 raw, no
// calibration bit), trailing comma optional.
struct ValveStates {
  uint8_t status[kValveCount] = {0};
};

// gonec / gowvc. Count-only form "gonec N" (hasList=false) or list form
// "gonec N id,id,..." (hasList=true, exactly N ids, trailing comma optional).
// NOTE: "gonec 0" is both the count reply and the empty-list reply; the
// caller that asked for the list treats count==0 as an empty list.
// N is limited to kTempSlotCount (gonec) / kVoltSlotCount (gowvc).
struct OneWireList {
  uint8_t count = 0;
  bool hasList = false;
  OneWireId ids[kTempSlotCount];
};

// goned: "goned <id> <temp>" (valid) or "goned 0" (valid=false: index out of
// range on the STM). "goned error" is the v1 gvlon error reply and parses as
// Cmd::Gvlon with gvlonError=true (see Reply).
struct TempData {
  bool valid = false;
  OneWireId id;
  int16_t value = kTempUnassigned;  // 0.1 C raw
};

// gowvd: "gowvd <id> <vad>" or "gowvd 0". vad in 10 mV, int32.
struct VoltData {
  bool valid = false;
  OneWireId id;
  int32_t vad = kVadFailed;
};

// gvlon single "gvlon v id1 id2" or list "gvlon 12 a1,a2,b1,b2,..." (24 ids).
// Ids are reported verbatim; a v1 STM may report garbage for unassigned
// sensors, so consumers resolve them against known ids.
struct ValveSensors {
  bool isList = false;
  uint8_t valve = 0;  // single form only
  OneWireId ids[kValveCount][2];  // single form: ids[valve][0..1] only
};

struct TargetReply {  // gtgtp v pos
  uint8_t valve = 0;
  uint8_t target = 0;  // 0..100
};

// gvlvx (v2), 19 fields after the command:
// idx status pos target meanCur oc cc dc cr moves calState earlyStops
// cmdRejected lastDir lastReq lastCnt lastStop lastPeak lastMs
// `status` uses the gvlvd encoding, but its bit 7 is not the calibration
// state in v2: the STM sets it only for staln and the movement trigger, and
// keeps it set until that calibration ends. calState (0..15 on the wire) is
// authoritative: bits 0..1 the phase, bits 2..3 sticky flags.
constexpr uint8_t kCalStateIdle = 0;
constexpr uint8_t kCalStateRequested = 1;
constexpr uint8_t kCalStateRunning = 2;
constexpr uint8_t kCalStateMask = 0x03;
constexpr uint8_t kCalFlagEarlyStop = 0x04;   // early end stop since the last good calibration
constexpr uint8_t kCalFlagLastFailed = 0x08;  // the last calibration did not succeed
constexpr uint8_t kCalFlagMask = kCalFlagEarlyStop | kCalFlagLastFailed;
enum class StopReason : uint8_t {
  None = 0,
  Target = 1,
  EndStop = 2,
  EarlyEndStop = 3,
  Timeout = 4,
  UnderCurrent = 5,
  SafetyOverCurrent = 6,
  Aborted = 7,
};
const char* stopReasonName(StopReason r);  // "none","target",... never null

struct MoveResult {
  MoveDir dir = MoveDir::Open;
  uint32_t requestedCounts = 0;
  uint32_t countedCounts = 0;
  StopReason stop = StopReason::None;
  uint16_t peakCurrent = 0;   // 0.1 mA
  uint32_t durationMs = 0;
};

// gvlvy (v3) field 20: valve flags (the STM's kVlvFlag* values).
constexpr uint16_t kStmFlagFsLease = 0x001;       // at its failsafe position: lease expired
constexpr uint16_t kStmFlagFsBlocked = 0x002;     // at its failsafe position: valve blocked
constexpr uint16_t kStmFlagUncalibrated = 0x004;  // no valid calibration counts
constexpr uint16_t kStmFlagNeedsRef = 0x008;      // next move goes to an end stop first
constexpr uint16_t kStmFlagRecal = 0x010;         // full calibration once the valve is present
constexpr uint16_t kStmFlagCalRestored = 0x020;   // counts restored from EEPROM
constexpr uint16_t kStmFlagRetry = 0x040;         // automatic calibration retry scheduled
constexpr uint16_t kStmFlagEarlyPending = 0x080;  // one early partial stop at this drive
constexpr uint16_t kStmFlagAssembly = 0x100;      // staop hold
constexpr uint16_t kStmFlagSvcHold = 0x200;       // left at a service move / sstop position
// "fsLease","fsBlocked","uncalibrated","needsRef","recal","calRestored",
// "retry","earlyPending","assembly","svcHold" for bits 0..9; "" for the
// reserved bits 10..15 and above.
const char* stmFlagName(uint8_t bit);

// gvlvy field 21: why a valve failed (status 4) or is blocked (status 9).
enum class ValveFault : uint8_t {
  None = 0,
  MoveTimeout = 1,
  StrokeTimeout = 2,
  Short = 3,            // presence test measured a short
  StrokesTooShort = 4,  // blocked
  InrushTrip = 5,       // the motor tripped the inrush limit at start
};
// "none","move_timeout","stroke_timeout","short","strokes_too_short",
// "inrush_trip"; "unknown" for other values.
const char* valveFaultName(uint8_t fault);

// gstax field 18: STM configuration load flags.
constexpr uint8_t kStmCfgLayoutCrc = 0x01;
constexpr uint8_t kStmCfgShadowMissing = 0x02;
constexpr uint8_t kStmCfgSettingsCorrupt = 0x04;
constexpr uint8_t kStmCfgSafetyCorrupt = 0x08;
constexpr uint8_t kStmCfgSensorSlot = 0x10;
constexpr uint8_t kStmCfgCalib = 0x20;
constexpr uint8_t kStmCfgUnverified = 0x40;
constexpr uint8_t kStmCfgReadFailed = 0x80;
// "layoutCrc","shadowMissing","settingsCorrupt","safetyCorrupt",
// "sensorSlot","calib","unverified","readFailed" for bits 0..7; "" above.
const char* stmCfgFlagName(uint8_t bit);

// gstax field 23: the common-mode protection guard tripped; the short and
// inrush limits are off until the next STM start.
constexpr uint8_t kStmSysProtectSuspended = 0x01;

struct ValveEx {
  uint8_t valve = 0;
  uint8_t status = 0;       // raw & 0x7F
  // A calibration runs (calState Running) or was asked for by staln or the
  // movement trigger (status bit 7). Requested alone (calState 1 without bit
  // 7: time trigger queued, or a valve found at start-up that calibrates on
  // its first target change) is not "calibrating": it can last for days.
  bool calibrating = false;
  uint8_t position = 0;     // 0..100
  uint8_t target = 0;       // 0..100, the STM's current target
  uint16_t meanCurrent = 0;
  uint32_t openCount = 0;
  uint32_t closeCount = 0;
  int32_t deadZone = 0;
  uint8_t calibRetries = 0;
  uint32_t moves = 0;
  uint8_t calState = 0;     // phase: raw & kCalStateMask (0 idle, 1 requested, 2 running)
  uint8_t calFlags = 0;     // raw & kCalFlagMask
  uint32_t earlyStops = 0;
  uint32_t cmdRejected = 0;
  MoveResult lastMove;
  // gvlvy (v3) only; gvlvx leaves the defaults.
  bool v3 = false;
  uint16_t flags = 0;               // field 20, kStmFlag*
  uint8_t fault = 0;                // field 21, ValveFault
  uint8_t fsPct = kFailsafeHold;    // field 22, 0..100 or kFailsafeHold
  uint8_t drive = 0;                // field 23, the target the STM drives to
  uint32_t retryS = 0;              // field 24, s to the next automatic retry (0 none)
  uint8_t retries = 0;              // field 25, automatic retries since the fault began
};

// gprof (v2): "gprof idx n c1:m1 ... cn:mn", n 0..32, exactly n pairs.
constexpr uint8_t kProfileMaxSamples = 32;
struct ProfileSample {
  uint32_t count = 0;     // motor pulse count at the sample
  uint16_t current = 0;   // 0.1 mA
};
struct Profile {
  uint8_t valve = 0;
  uint8_t count = 0;
  ProfileSample samples[kProfileMaxSamples];
};

// "<cmd> <idx> ok" or "<cmd> <idx> err <code>" (svmov v2; sfspo, sstop v3).
// index -1: the STM could not read the index; such a reply answers any
// outstanding request of that command. index 255 (sfspo, sstop): all valves.
struct IndexedResult {
  int16_t index = -1;
  bool ok = false;
  uint16_t errorCode = 0;
};

// gstat (v2): "gstat uptime_s resets bootReason rxOverflow parseErr eepState".
// gstax (v3) starts with the same 6 fields and adds 17 more.
struct StmStatus {
  uint32_t uptimeS = 0;
  uint32_t resets = 0;
  uint32_t bootReason = 0;
  uint32_t rxOverflow = 0;
  uint32_t parseErrors = 0;
  uint8_t eepState = 0;
  // gstax only; gstat leaves the defaults.
  bool v3 = false;
  LeaseState lease = LeaseState::Off;   // 7
  uint32_t leaseRemainS = 0;            // 8, while running
  bool leaseClient = false;             // 9, a lease command within the last 300 s
  uint16_t leaseTimeoutMin = 0;         // 10
  uint16_t failsafeMask = 0;            // 11, bit v: valve v at its lease failsafe
  bool safeMode = false;                // 12
  uint8_t wdgResets = 0;                // 13, watchdog resets in the current window
  uint32_t uartOre = 0, uartFe = 0, uartNe = 0, rxDropped = 0;  // 14..17, since start-up
  uint8_t cfgFlags = 0;                 // 18, kStmCfg*
  uint32_t cfgEvents = 0;               // 19, loads that repaired/defaulted a block
  uint32_t eepWrites = 0;               // 20
  uint32_t tempAgeS = 0;                // 21, s since the last complete temperature cycle
  uint32_t owScanAgeS = 0;              // 22, s since the last 1-Wire enumeration
  uint8_t sysFlags = 0;                 // 23, kStmSys*
};

// glcfg (v3): "glcfg <timeoutMin> <fs0> ... <fs11>".
struct LeaseConfigReply {
  uint16_t timeoutMin = 0;                     // 0..1440
  uint8_t failsafePct[kValveCount] = {};       // 0..100 or kFailsafeHold
};

// slhbt (v3) ok form: "slhbt <lease> <remainS>".
struct HeartbeatReply {
  LeaseState lease = LeaseState::Off;
  uint32_t remainS = 0;
};

// Replies without payload ("stgtp", "stons", "staln", "stlnm", "smotc",
// "staop ", "stdet ", "masns ", "reset ", "stvls v", "scalx ok", "stlnt",
// "slcfg ok", "ssafe ok"). error=true for the "smotc err" / "scalx err" /
// "slcfg err" / "ssafe err" / "slhbt err" forms.
struct Ack {
  bool error = false;
  uint8_t valve = kNoValve;  // stvls only
};

// A parsed reply. Plain struct (not a union) so every payload type stays
// trivially copyable; only the member selected by `cmd` is meaningful.
// sizeof(Reply) is ~1.4 KB: keep exactly one per task, never on small stacks.
struct Reply {
  Cmd cmd = Cmd::None;
  bool gvlonError = false;       // "goned error" (v1 gvlon error)
  Ack ack;                       // Stgtp Stons Stvls Masns Staop Staln Stdet Stlnm Smotc Reset
                                 // Scalx Stlnt Slcfg Ssafe, Slhbt error form
  ValveData valveData;           // Gvlvd
  ValveStates valveStates;       // Gvlst
  OneWireList oneWireList;       // Gonec, Gowvc
  TempData tempData;             // Goned
  VoltData voltData;             // Gowvd
  ValveSensors valveSensors;     // Gvlon
  TargetReply target;            // Gtgtp
  uint16_t learnMovements = 0;   // Gtlnm
  MotorChars motorChars;         // Gmotc
  Version version;               // Gvers
  uint32_t build = 0;            // Gvers, 0 when absent
  uint16_t hwId = 0;             // Ghwin (DBGMCU IDCODE & 0xFFF)
  bool eepromIdle = false;       // Eepst: "1" = nothing pending
  uint8_t proto = 0;             // Gproto
  ValveEx valveEx;               // Gvlvx, Gvlvy (v3 set)
  Profile profile;               // Gprof
  IndexedResult serviceMove;     // Svmov
  Breakaway breakaway;           // Gcalx
  StmStatus status;              // Gstat, Gstax (v3 set)
  LeaseConfigReply leaseConfig;  // Glcfg
  HeartbeatReply heartbeat;      // Slhbt ok form
  IndexedResult failsafe;        // Sfspo
  IndexedResult stop;            // Sstop
  uint32_t learnTime = 0;        // Gtlnt
};

// Parses one complete line (no CR/LF; `len` bytes, NUL not required).
// Tokens are separated by one or more spaces; trailing spaces are allowed.
// Every numeric field is strict decimal and range-checked; on any error the
// status says why and `out` is reset to Reply{}.
// gvlvx has exactly 19 fields and gstat exactly 6. The v3 replies with a
// fixed field list (gvlvy >= 25, gstax >= 23, glcfg >= 13, slhbt ok form
// >= 2) accept extra fields a later STM may append: each must be a strict
// decimal 0..2^32-1 and is ignored.
ParseStatus parseReply(const char* line, size_t len, Reply& out);

// True when `rep` is the answer to `req`: same command and, where the reply
// carries it, the same valve / bus index. Special cases:
//  - Gvlon request accepts a gvlonError reply.
//  - Goned/Gowvd requests accept the invalid form ("goned 0") -- index match
//    cannot be checked there; a valid reading must carry req.expect unless
//    that is zero (a late reply for another bus index is not taken).
//  - Gonec/Gowvc list requests accept a count-only reply with count 0.
//  - Svmov/Sfspo/Sstop accept index -1 (the STM could not read the index).
bool replyMatches(const RequestLine& req, const Reply& rep);

// Maps a gvlon/list id to a 1-based config slot using the configured slot
// ids (slotIds[i] for slot i+1; zero ids are "empty slot"). Returns 0 when the
// id is zero, fails CRC, or is not configured.
uint8_t resolveTempSlot(const OneWireId& id, const OneWireId* slotIds, uint8_t slotCount);

// Chip name for a DBGMCU/bootloader PID: 0x413 "STM32F40xx/41xx",
// 0x423 "STM32F401xB/C", 0x431 "STM32F411xx", 0x433 "STM32F401xD/E",
// else "Unknown Chip". Never null.
const char* stmChipName(uint16_t pid);

}  // namespace vdm
