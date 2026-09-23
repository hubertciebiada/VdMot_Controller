// STM application protocol codec (ESP side): builds request lines and parses
// reply lines into typed structs. Hardware-free, no state.
//
// Wire format (both protocol v1 = STM 1.4.x and v2 = revamped STM 2.x):
//   request : "<cmd>" { " " <arg> } " " "\r\n"  -- EVERY token, including the
//             last, is followed by one space (the v1 STM tokenizer needs it);
//             numbers are non-negative decimal; max 5 args; line <= 63 chars.
//   reply   : "<cmd>" { " " <arg> } [" "] and CR/LF (one line per request).
// Replies are matched to requests by their 5-char command (see replyMatches).
// References: specs/01-uart-protocol.md §5, architecture §2.3 (v2 commands).
#pragma once

#include <stddef.h>
#include <stdint.h>

#include "vdm/common.h"
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
};
constexpr uint8_t kCmdCount = 31;  // including None

// "stgtp" etc.; "" for None/out of range. Never null.
const char* cmdName(Cmd c);
// Exact match of `len` bytes against the 30 names; None if unknown.
Cmd cmdFromName(const char* s, size_t len);
// True for the v2-only commands (Gproto..Gstat).
bool cmdIsV2(Cmd c);
// True when sending the same request twice has the same effect as once
// (all get*, stgtp, stvls, stlnm, smotc, scalx). Only these are retried by
// LinkPolicy. Actions (staln, staop, stdet, stons, masns, svmov, reset) are
// never retried automatically.
bool cmdIsIdempotent(Cmd c);

// ---------------------------------------------------------------- requests

constexpr size_t kRequestMaxLen = 63;  // chars incl. "\r\n", excl. NUL

// One encoded request. `valve` is the valve the request is about (0..11),
// kAllValves for "255" requests, or kNoValve. `arg` is the first numeric
// argument when there is one that is not a valve (sensor bus index, count),
// else 0; LinkPolicy uses (cmd, valve, arg) to match replies.
struct RequestLine {
  char text[kRequestMaxLen + 1] = {0};
  uint8_t len = 0;
  Cmd cmd = Cmd::None;
  uint8_t valve = kNoValve;
  uint16_t arg = 0;
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
// (spec 01 §5.18 boot acceptance): factors 10..40, startOnPower 0..100,
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
// sensors (spec 01 §5.7), so consumers resolve them against known ids.
struct ValveSensors {
  bool isList = false;
  uint8_t valve = 0;  // single form only
  OneWireId ids[kValveCount][2];  // single form: ids[valve][0..1] only
};

struct TargetReply {  // gtgtp v pos
  uint8_t valve = 0;
  uint8_t target = 0;  // 0..100
};

// gvlvx (v2), 20 fields after the command:
// idx status pos target meanCur oc cc dc cr moves calState earlyStops
// cmdRejected lastDir lastReq lastCnt lastStop lastPeak lastMs
// `status` uses the gvlvd encoding (bit 7 = calibrating).
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

struct ValveEx {
  uint8_t valve = 0;
  uint8_t status = 0;
  bool calibrating = false;
  uint8_t position = 0;     // 0..100
  uint8_t target = 0;       // 0..100, the STM's current target
  uint16_t meanCurrent = 0;
  uint32_t openCount = 0;
  uint32_t closeCount = 0;
  int32_t deadZone = 0;
  uint8_t calibRetries = 0;
  uint32_t moves = 0;
  uint8_t calState = 0;     // 0 idle, 1 started, 2 in progress
  uint32_t earlyStops = 0;
  uint32_t cmdRejected = 0;
  MoveResult lastMove;
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

// svmov (v2): "svmov idx ok" or "svmov idx err <code>".
struct ServiceMoveReply {
  uint8_t valve = 0;
  bool ok = false;
  uint16_t errorCode = 0;
};

// gstat (v2): "gstat uptime_s resets bootReason rxOverflow parseErr eepState".
struct StmStatus {
  uint32_t uptimeS = 0;
  uint32_t resets = 0;
  uint32_t bootReason = 0;
  uint32_t rxOverflow = 0;
  uint32_t parseErrors = 0;
  uint8_t eepState = 0;
};

// Replies without payload ("stgtp", "stons", "staln", "stlnm", "smotc",
// "staop ", "stdet ", "masns ", "reset ", "stvls v", "scalx ok").
// error=true for the v2 "smotc err" / "scalx err" forms.
struct Ack {
  bool error = false;
  uint8_t valve = kNoValve;  // stvls only
};

// A parsed reply. Plain struct (not a union) so every payload type stays
// trivially copyable; only the member selected by `cmd` is meaningful.
// sizeof(Reply) is ~1.3 KB: keep exactly one per task, never on small stacks.
struct Reply {
  Cmd cmd = Cmd::None;
  bool gvlonError = false;       // "goned error" (v1 gvlon error)
  Ack ack;                       // Stgtp Stons Stvls Masns Staop Staln Stdet Stlnm Smotc Reset Scalx
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
  ValveEx valveEx;               // Gvlvx
  Profile profile;               // Gprof
  ServiceMoveReply serviceMove;  // Svmov
  Breakaway breakaway;           // Gcalx
  StmStatus status;              // Gstat
};

// Parses one complete line (no CR/LF; `len` bytes, NUL not required).
// Tokens are separated by one or more spaces; trailing spaces are allowed.
// Every numeric field is strict decimal and range-checked; on any error the
// status says why and `out` is reset to Reply{}.
ParseStatus parseReply(const char* line, size_t len, Reply& out);

// True when `rep` is the answer to `req`: same command and, where the reply
// carries it, the same valve / bus index. Special cases:
//  - Gvlon request accepts a gvlonError reply.
//  - Goned/Gowvd requests accept the invalid form ("goned 0") -- index match
//    cannot be checked there.
//  - Gonec/Gowvc list requests accept a count-only reply with count 0.
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
