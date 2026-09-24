#include "vdm/stm_codec.h"

#include <string.h>

#include "vdm/line_assembler.h"

namespace vdm {

namespace {

// Indexed by Cmd; kCmdCount entries including None.
const char* const kCmdNames[kCmdCount] = {
    "",      "stgtp", "gtgtp", "gvlvd", "gvlst", "gonec", "goned", "gvlon",  "gowvc", "gowvd", "stons",
    "stvls", "masns", "staop", "staln", "stdet", "stlnm", "gtlnm", "smotc",  "gmotc", "gvers", "ghwin",
    "eepst", "reset", "gproto", "gvlvx", "gprof", "svmov", "scalx", "gcalx", "gstat",
};
static_assert(static_cast<uint8_t>(Cmd::Gstat) + 1 == kCmdCount, "kCmdNames out of sync with Cmd");

// Copy source for resetting a Reply without a 1.3 KB temporary on the stack.
const Reply kEmptyReply{};

constexpr uint8_t kMaxTempBusIndex = kTempSlotCount - 1;
constexpr uint8_t kMaxVoltBusIndex = kVoltSlotCount - 1;

// ---------------------------------------------------------------- builders

class LineBuilder {
 public:
  explicit LineBuilder(RequestLine& out) : out_(out) {}

  void text(const char* s) {
    while (*s) put(*s++);
  }
  void token(const char* s) {
    text(s);
    put(' ');
  }
  void number(uint32_t v) {
    char digits[10];  // NOMUTATE: UINT32_MAX has 10 digits; no builder sends that many
    size_t n = 0;
    do {
      digits[n++] = static_cast<char>('0' + v % 10);
      v /= 10;
    } while (v != 0);
    while (n > 0) put(digits[--n]);
    put(' ');
  }
  void id(const OneWireId& v) {
    char buf[kOneWireIdTextLen + 1];  // NOMUTATE: exact size; larger is equivalent
    formatOneWireId(v, buf, sizeof buf);
    token(buf);
  }
  // Appends CR LF; false when anything did not fit.
  bool finish() {
    put('\r');
    put('\n');
    if (overflow_) return false;  // NOMUTATE: unreachable guard, longest request is 59 chars
    out_.text[len_] = '\0';
    out_.len = static_cast<uint8_t>(len_);
    return true;
  }

 private:
  void put(char c) {
    if (len_ >= kRequestMaxLen) {  // NOMUTATE: unreachable guard (see finish())
      overflow_ = true;            // NOMUTATE: unreachable guard (see finish())
      return;
    }
    out_.text[len_++] = c;
  }

  RequestLine& out_;
  size_t len_ = 0;
  bool overflow_ = false;
};

bool fail(RequestLine& out) {
  out = RequestLine{};
  return false;
}

// "<cmd> <n0> <n1> ... \r\n". `valve` and `arg` only fill the metadata.
bool buildNumeric(Cmd cmd, uint8_t valve, uint16_t arg, const uint32_t* nums, size_t n,
                  RequestLine& out) {
  out = RequestLine{};
  LineBuilder b(out);
  b.token(cmdName(cmd));
  for (size_t i = 0; i < n; ++i) b.number(nums[i]);
  if (!b.finish()) return fail(out);  // NOMUTATE: unreachable guard (see finish())
  out.cmd = cmd;
  out.valve = valve;
  out.arg = arg;
  return true;
}

bool buildBare(Cmd cmd, RequestLine& out) { return buildNumeric(cmd, kNoValve, 0, nullptr, 0, out); }

bool buildValveArg(Cmd cmd, uint8_t valve, RequestLine& out) {
  if (valve >= kValveCount) return fail(out);
  const uint32_t v = valve;
  return buildNumeric(cmd, valve, 0, &v, 1, out);
}

bool buildValveOrAll(Cmd cmd, uint8_t valve, RequestLine& out) {
  if (valve >= kValveCount && valve != kAllValves) return fail(out);
  const uint32_t v = valve;
  return buildNumeric(cmd, valve, 0, &v, 1, out);
}

bool buildIndexArg(Cmd cmd, uint8_t index, uint8_t maxIndex, RequestLine& out) {
  if (index > maxIndex) return fail(out);
  const uint32_t v = index;
  return buildNumeric(cmd, kNoValve, index, &v, 1, out);
}

bool buildListRequest(Cmd cmd, RequestLine& out) {
  const uint32_t v = kAllValves;
  return buildNumeric(cmd, kNoValve, kAllValves, &v, 1, out);
}

// ---------------------------------------------------------------- tokens

constexpr size_t kMaxTokens = 40;

struct Token {
  const char* p;
  size_t n;
};

bool tokenIs(const Token& t, const char* s) {
  const size_t n = strlen(s);
  return t.n == n && memcmp(t.p, s, n) == 0;
}

bool allDigits(const char* s, size_t n) {
  if (n == 0 || n > 10) return false;
  for (size_t i = 0; i < n; ++i) {
    if (s[i] < '0' || s[i] > '9') return false;
  }
  return true;
}

// Unsigned decimal in [min, max]. Non-digits -> BadNumber, digits whose value
// is outside the range (including > 2^32-1) -> OutOfRange.
ParseStatus readU(const char* s, size_t n, uint32_t min, uint32_t max, uint32_t& out) {
  if (!allDigits(s, n)) return ParseStatus::BadNumber;
  uint32_t v{};
  if (!parseUint(s, n, max, v) || v < min) return ParseStatus::OutOfRange;
  out = v;
  return ParseStatus::Ok;
}

ParseStatus readU(const Token& t, uint32_t min, uint32_t max, uint32_t& out) {
  return readU(t.p, t.n, min, max, out);
}

ParseStatus readI(const Token& t, int32_t min, int32_t max, int32_t& out) {
  const bool neg = t.p[0] == '-';  // tokens are never empty
  if (!allDigits(neg ? t.p + 1 : t.p, neg ? t.n - 1 : t.n)) return ParseStatus::BadNumber;
  int32_t v{};
  if (!parseInt(t.p, t.n, min, max, v)) return ParseStatus::OutOfRange;
  out = v;
  return ParseStatus::Ok;
}

ParseStatus readId(const char* s, size_t n, OneWireId& out) {
  return parseOneWireId(s, n, out) ? ParseStatus::Ok : ParseStatus::BadOneWireId;
}

ParseStatus readId(const Token& t, OneWireId& out) { return readId(t.p, t.n, out); }

// Reads a run of numeric fields described by a table. Stops at the first
// error. Each target is written through its own typed pointer.
enum class FieldType : uint8_t { U8, U16, U32, I16, I32, Bool };
struct Field {
  FieldType type;
  void* target;
  int32_t min;
  uint32_t max;
};

ParseStatus readFields(const Token* tok, const Field* fields, size_t n) {
  for (size_t i = 0; i < n; ++i) {
    const Field& f = fields[i];
    ParseStatus st;
    if (f.type == FieldType::I16 || f.type == FieldType::I32) {
      int32_t v{};
      st = readI(tok[i], f.min, static_cast<int32_t>(f.max), v);
      if (st != ParseStatus::Ok) return st;
      if (f.type == FieldType::I16) {
        *static_cast<int16_t*>(f.target) = static_cast<int16_t>(v);
      } else {
        *static_cast<int32_t*>(f.target) = v;
      }
      continue;
    }
    uint32_t v{};
    st = readU(tok[i], static_cast<uint32_t>(f.min), f.max, v);
    if (st != ParseStatus::Ok) return st;
    switch (f.type) {
      case FieldType::U8:
        *static_cast<uint8_t*>(f.target) = static_cast<uint8_t>(v);
        break;
      case FieldType::U16:
        *static_cast<uint16_t*>(f.target) = static_cast<uint16_t>(v);
        break;
      case FieldType::Bool:
        *static_cast<bool*>(f.target) = v != 0;
        break;
      default:
        *static_cast<uint32_t*>(f.target) = v;
        break;
    }
  }
  return ParseStatus::Ok;
}

constexpr uint32_t kU8 = 0xFF;
constexpr uint32_t kU16 = 0xFFFF;
constexpr uint32_t kU32 = 0xFFFFFFFFu;
constexpr int32_t kI16Min = -32768;
constexpr uint32_t kI16Max = 32767;
constexpr int32_t kI32Min = INT32_MIN;
constexpr uint32_t kI32Max = 0x7FFFFFFF;
constexpr uint32_t kValveMax = kValveCount - 1;

// Splits a comma list "a,b,c" (one trailing comma allowed) and calls
// visit(index, ptr, len) for each element. The element count must be exactly
// `expected`; empty elements are BadFormat.
template <typename Visit>
ParseStatus forEachListItem(const Token& t, size_t expected, Visit visit) {
  size_t n = t.n;  // >= 1: tokens are never empty
  if (t.p[n - 1] == ',') --n;  // tolerated v1 trailing comma
  size_t count = 0;
  size_t start = 0;
  for (size_t i = 0; i <= n; ++i) {
    if (i < n && t.p[i] != ',') continue;
    if (i == start || count == expected) return ParseStatus::BadFormat;
    const ParseStatus st = visit(count, t.p + start, i - start);
    if (st != ParseStatus::Ok) return st;
    ++count;
    start = i + 1;
  }
  return count == expected ? ParseStatus::Ok : ParseStatus::BadFormat;
}

// ---------------------------------------------------------------- replies

ParseStatus parseAck(size_t argc) {
  return argc == 0 ? ParseStatus::Ok : ParseStatus::BadArgCount;
}

ParseStatus parseValveData(const Token* a, size_t argc, ValveData& d) {
  if (argc != 11) return ParseStatus::BadArgCount;
  uint32_t raw{};
  const Field f[] = {
      {FieldType::U8, &d.valve, 0, kValveMax},     {FieldType::U8, &d.position, 0, 100},
      {FieldType::U16, &d.meanCurrent, 0, kU16},   {FieldType::U32, &raw, 0, kU8},
      {FieldType::I16, &d.temp1, kI16Min, kI16Max}, {FieldType::I16, &d.temp2, kI16Min, kI16Max},
      {FieldType::U32, &d.moves, 0, kU32},         {FieldType::U32, &d.openCount, 0, kU32},
      {FieldType::U32, &d.closeCount, 0, kU32},    {FieldType::I32, &d.deadZone, kI32Min, kI32Max},
      {FieldType::U8, &d.calibRetries, 0, kU8},
  };
  const ParseStatus st = readFields(a, f, 11);
  d.status = static_cast<uint8_t>(raw & 0x7F);
  d.calibrating = (raw & 0x80) != 0;
  return st;
}

ParseStatus parseValveStates(const Token* a, size_t argc, ValveStates& s) {
  if (argc != 2) return ParseStatus::BadArgCount;
  uint32_t n{};
  ParseStatus st = readU(a[0], kValveCount, kValveCount, n);
  if (st != ParseStatus::Ok) return st;
  return forEachListItem(a[1], kValveCount, [&s](size_t i, const char* p, size_t len) {
    uint32_t v{};
    const ParseStatus r = readU(p, len, 0, kU8, v);
    s.status[i] = static_cast<uint8_t>(v);
    return r;
  });
}

ParseStatus parseOneWireList(const Token* a, size_t argc, uint8_t maxCount, OneWireList& l) {
  if (argc != 1 && argc != 2) return ParseStatus::BadArgCount;
  uint32_t n{};
  ParseStatus st = readU(a[0], 0, maxCount, n);
  if (st != ParseStatus::Ok) return st;
  l.count = static_cast<uint8_t>(n);
  if (argc == 1) return ParseStatus::Ok;
  l.hasList = true;
  return forEachListItem(a[1], n, [&l](size_t i, const char* p, size_t len) {
    return readId(p, len, l.ids[i]);
  });
}

// "goned id temp" / "goned 0" (and "gowvd ..."). `value` range differs.
template <typename T>
ParseStatus parseSensorData(const Token* a, size_t argc, int32_t min, uint32_t max, bool& valid,
                            OneWireId& id, T& value) {
  if (argc == 1) return tokenIs(a[0], "0") ? ParseStatus::Ok : ParseStatus::BadFormat;
  if (argc != 2) return ParseStatus::BadArgCount;
  ParseStatus st = readId(a[0], id);
  if (st != ParseStatus::Ok) return st;
  int32_t v{};
  st = readI(a[1], min, static_cast<int32_t>(max), v);
  if (st != ParseStatus::Ok) return st;
  value = static_cast<T>(v);
  valid = true;
  return ParseStatus::Ok;
}

ParseStatus parseValveSensors(const Token* a, size_t argc, ValveSensors& s) {
  uint32_t v{};
  if (argc == 3) {
    ParseStatus st = readU(a[0], 0, kValveMax, v);
    if (st != ParseStatus::Ok) return st;
    s.valve = static_cast<uint8_t>(v);
    st = readId(a[1], s.ids[v][0]);
    if (st != ParseStatus::Ok) return st;
    return readId(a[2], s.ids[v][1]);
  }
  if (argc != 2) return ParseStatus::BadArgCount;
  ParseStatus st = readU(a[0], kValveCount, kValveCount, v);
  if (st != ParseStatus::Ok) return st;
  s.isList = true;
  return forEachListItem(a[1], 2u * kValveCount, [&s](size_t i, const char* p, size_t len) {
    return readId(p, len, s.ids[i / 2][i % 2]);
  });
}

ParseStatus parseMotorChars(const Token* a, size_t argc, MotorChars& m) {
  if (argc < 3 || argc > 5) return ParseStatus::BadArgCount;
  const Field f[] = {
      {FieldType::U8, &m.lowFactor, 0, kU8},  {FieldType::U8, &m.highFactor, 0, kU8},
      {FieldType::U8, &m.startOnPower, 0, kU8}, {FieldType::U16, &m.minCounts, 0, kU16},
      {FieldType::U8, &m.maxCalibRetries, 0, kU8},
  };
  m.fieldCount = static_cast<uint8_t>(argc);
  return readFields(a, f, argc);
}

ParseStatus parseVersionReply(const Token* a, size_t argc, Reply& r) {
  if (argc != 1 && argc != 2) return ParseStatus::BadArgCount;
  if (!parseVersion(a[0].p, a[0].n, r.version)) return ParseStatus::BadFormat;
  return argc == 2 ? readU(a[1], 0, kU32, r.build) : ParseStatus::Ok;
}

constexpr size_t kValveExFields = 19;

ParseStatus parseValveEx(const Token* a, size_t argc, ValveEx& x) {
  if (argc != kValveExFields) return ParseStatus::BadArgCount;
  uint32_t raw{};
  uint32_t cal{};
  uint32_t dir{};
  uint32_t stop{};
  MoveResult& m = x.lastMove;
  const Field f[] = {
      {FieldType::U8, &x.valve, 0, kValveMax},
      {FieldType::U32, &raw, 0, kU8},
      {FieldType::U8, &x.position, 0, 100},
      {FieldType::U8, &x.target, 0, 100},
      {FieldType::U16, &x.meanCurrent, 0, kU16},
      {FieldType::U32, &x.openCount, 0, kU32},
      {FieldType::U32, &x.closeCount, 0, kU32},
      {FieldType::I32, &x.deadZone, kI32Min, kI32Max},
      {FieldType::U8, &x.calibRetries, 0, kU8},
      {FieldType::U32, &x.moves, 0, kU32},
      {FieldType::U32, &cal, 0, kCalStateMask | kCalFlagMask},
      {FieldType::U32, &x.earlyStops, 0, kU32},
      {FieldType::U32, &x.cmdRejected, 0, kU32},
      {FieldType::U32, &dir, 0, 1},
      {FieldType::U32, &m.requestedCounts, 0, kU32},
      {FieldType::U32, &m.countedCounts, 0, kU32},
      {FieldType::U32, &stop, 0, static_cast<uint32_t>(StopReason::Aborted)},
      {FieldType::U16, &m.peakCurrent, 0, kU16},
      {FieldType::U32, &m.durationMs, 0, kU32},
  };
  static_assert(sizeof f / sizeof *f == kValveExFields, "one table row per gvlvx field");
  const ParseStatus st = readFields(a, f, kValveExFields);
  if (st != ParseStatus::Ok) return st;
  x.status = static_cast<uint8_t>(raw & 0x7F);
  x.calState = static_cast<uint8_t>(cal & kCalStateMask);
  x.calFlags = static_cast<uint8_t>(cal & kCalFlagMask);
  x.calibrating = x.calState == kCalStateRunning || (raw & 0x80) != 0;
  m.dir = static_cast<MoveDir>(dir);
  m.stop = static_cast<StopReason>(stop);
  return ParseStatus::Ok;
}

ParseStatus parseProfile(const Token* a, size_t argc, Profile& p) {
  if (argc < 2) return ParseStatus::BadArgCount;
  uint32_t v{};
  uint32_t n{};
  ParseStatus st = readU(a[0], 0, kValveMax, v);
  if (st != ParseStatus::Ok) return st;
  st = readU(a[1], 0, kProfileMaxSamples, n);
  if (st != ParseStatus::Ok) return st;
  if (argc != 2 + n) return ParseStatus::BadArgCount;
  p.valve = static_cast<uint8_t>(v);
  for (size_t i = 0; i < n; ++i) {
    const Token& t = a[2 + i];
    const char* colon = static_cast<const char*>(memchr(t.p, ':', t.n));
    if (colon == nullptr) return ParseStatus::BadFormat;
    const size_t cn = static_cast<size_t>(colon - t.p);
    uint32_t count{};
    uint32_t cur{};
    st = readU(t.p, cn, 0, kU32, count);
    if (st != ParseStatus::Ok) return st;
    st = readU(colon + 1, t.n - cn - 1, 0, kU16, cur);
    if (st != ParseStatus::Ok) return st;
    p.samples[i].count = count;
    p.samples[i].current = static_cast<uint16_t>(cur);
  }
  p.count = static_cast<uint8_t>(n);
  return ParseStatus::Ok;
}

ParseStatus parseServiceMove(const Token* a, size_t argc, ServiceMoveReply& s) {
  if (argc != 2 && argc != 3) return ParseStatus::BadArgCount;
  uint32_t v{};
  ParseStatus st = readU(a[0], 0, kValveMax, v);
  if (st != ParseStatus::Ok) return st;
  s.valve = static_cast<uint8_t>(v);
  if (argc == 2) {
    if (!tokenIs(a[1], "ok")) return ParseStatus::BadFormat;
    s.ok = true;
    return ParseStatus::Ok;
  }
  if (!tokenIs(a[1], "err")) return ParseStatus::BadFormat;
  uint32_t code{};
  st = readU(a[2], 0, kU16, code);
  s.errorCode = static_cast<uint16_t>(code);
  return st;
}

ParseStatus parseBreakaway(const Token* a, size_t argc, Breakaway& b) {
  if (argc != 3) return ParseStatus::BadArgCount;
  const Field f[] = {
      {FieldType::Bool, &b.enable, 0, 1},
      {FieldType::U8, &b.stepPct, 0, 100},
      {FieldType::U8, &b.maxmA, 20, 60},
  };
  return readFields(a, f, 3);
}

ParseStatus parseStatus(const Token* a, size_t argc, StmStatus& s) {
  if (argc != 6) return ParseStatus::BadArgCount;
  const Field f[] = {
      {FieldType::U32, &s.uptimeS, 0, kU32},    {FieldType::U32, &s.resets, 0, kU32},
      {FieldType::U32, &s.bootReason, 0, kU32}, {FieldType::U32, &s.rxOverflow, 0, kU32},
      {FieldType::U32, &s.parseErrors, 0, kU32}, {FieldType::U8, &s.eepState, 0, kU8},
  };
  return readFields(a, f, 6);
}

// "<cmd>" or "<cmd> err" (smotc) / "<cmd> ok|err" (scalx).
ParseStatus parseOkErr(const Token* a, size_t argc, bool bareIsOk, Ack& ack) {
  if (argc == 0 && bareIsOk) return ParseStatus::Ok;
  if (argc != 1) return ParseStatus::BadArgCount;
  if (tokenIs(a[0], "err")) {
    ack.error = true;
    return ParseStatus::Ok;
  }
  return !bareIsOk && tokenIs(a[0], "ok") ? ParseStatus::Ok : ParseStatus::BadFormat;
}

template <typename T>
ParseStatus parseSingle(const Token* a, size_t argc, uint32_t min, uint32_t max, T& out) {
  if (argc != 1) return ParseStatus::BadArgCount;
  uint32_t v{};
  const ParseStatus st = readU(a[0], min, max, v);
  out = static_cast<T>(v);
  return st;
}

ParseStatus parsePayload(Cmd cmd, const Token* a, size_t argc, Reply& r) {
  switch (cmd) {
    case Cmd::Stgtp:
    case Cmd::Stons:
    case Cmd::Masns:
    case Cmd::Staop:
    case Cmd::Staln:
    case Cmd::Stdet:
    case Cmd::Stlnm:
    case Cmd::Reset:
      return parseAck(argc);
    case Cmd::Smotc:
      return parseOkErr(a, argc, true, r.ack);
    case Cmd::Scalx:
      return parseOkErr(a, argc, false, r.ack);
    case Cmd::Stvls:
      return parseSingle(a, argc, 0, kValveMax, r.ack.valve);
    case Cmd::Gtgtp: {
      if (argc != 2) return ParseStatus::BadArgCount;
      const Field f[] = {{FieldType::U8, &r.target.valve, 0, kValveMax},
                         {FieldType::U8, &r.target.target, 0, 100}};
      return readFields(a, f, 2);
    }
    case Cmd::Gvlvd:
      return parseValveData(a, argc, r.valveData);
    case Cmd::Gvlst:
      return parseValveStates(a, argc, r.valveStates);
    case Cmd::Gonec:
      return parseOneWireList(a, argc, kTempSlotCount, r.oneWireList);
    case Cmd::Gowvc:
      return parseOneWireList(a, argc, kVoltSlotCount, r.oneWireList);
    case Cmd::Goned:
      return parseSensorData(a, argc, kI16Min, kI16Max, r.tempData.valid, r.tempData.id,
                             r.tempData.value);
    case Cmd::Gowvd:
      return parseSensorData(a, argc, kI32Min, kI32Max, r.voltData.valid, r.voltData.id,
                             r.voltData.vad);
    case Cmd::Gvlon:
      return parseValveSensors(a, argc, r.valveSensors);
    case Cmd::Gtlnm:
      return parseSingle(a, argc, 0, kU16, r.learnMovements);
    case Cmd::Gmotc:
      return parseMotorChars(a, argc, r.motorChars);
    case Cmd::Gvers:
      return parseVersionReply(a, argc, r);
    case Cmd::Ghwin:
      return parseSingle(a, argc, 0, 0xFFF, r.hwId);
    case Cmd::Eepst:
      return parseSingle(a, argc, 0, 1, r.eepromIdle);
    case Cmd::Gproto:
      return parseSingle(a, argc, 1, kU8, r.proto);
    case Cmd::Gvlvx:
      return parseValveEx(a, argc, r.valveEx);
    case Cmd::Gprof:
      return parseProfile(a, argc, r.profile);
    case Cmd::Svmov:
      return parseServiceMove(a, argc, r.serviceMove);
    case Cmd::Gcalx:
      return parseBreakaway(a, argc, r.breakaway);
    case Cmd::Gstat:
      return parseStatus(a, argc, r.status);
    default:
      return ParseStatus::UnknownCommand;
  }
}

}  // namespace

// ---------------------------------------------------------------- commands

const char* cmdName(Cmd c) {
  const uint8_t i = static_cast<uint8_t>(c);
  return i < kCmdCount ? kCmdNames[i] : "";
}

Cmd cmdFromName(const char* s, size_t len) {
  if (s == nullptr) return Cmd::None;
  for (uint8_t i = static_cast<uint8_t>(Cmd::Stgtp); i < kCmdCount; ++i) {
    if (strlen(kCmdNames[i]) == len && memcmp(kCmdNames[i], s, len) == 0) {
      return static_cast<Cmd>(i);
    }
  }
  return Cmd::None;
}

bool cmdIsV2(Cmd c) {
  const uint8_t i = static_cast<uint8_t>(c);
  return i >= static_cast<uint8_t>(Cmd::Gproto) && i < kCmdCount;
}

bool cmdIsIdempotent(Cmd c) {
  switch (c) {
    case Cmd::Stgtp:
    case Cmd::Gtgtp:
    case Cmd::Gvlvd:
    case Cmd::Gvlst:
    case Cmd::Gonec:
    case Cmd::Goned:
    case Cmd::Gvlon:
    case Cmd::Gowvc:
    case Cmd::Gowvd:
    case Cmd::Stvls:
    case Cmd::Stlnm:
    case Cmd::Gtlnm:
    case Cmd::Smotc:
    case Cmd::Gmotc:
    case Cmd::Gvers:
    case Cmd::Ghwin:
    case Cmd::Eepst:
    case Cmd::Gproto:
    case Cmd::Gvlvx:
    case Cmd::Gprof:
    case Cmd::Scalx:
    case Cmd::Gcalx:
    case Cmd::Gstat:
      return true;
    default:
      return false;
  }
}

bool motorCharsValid(const MotorChars& m) {
  return m.lowFactor >= 10 && m.lowFactor <= 40 && m.highFactor >= 10 && m.highFactor <= 40 &&
         m.startOnPower <= 100 && m.minCounts <= 60000 && m.maxCalibRetries <= 2;
}

bool breakawayValid(const Breakaway& b) {
  return b.stepPct <= 100 && b.maxmA >= 20 && b.maxmA <= 60;
}

bool learnMovementsValid(uint32_t n) { return n == 0 || (n >= 50 && n <= 65534); }

// ---------------------------------------------------------------- builders

bool buildSetTarget(uint8_t valve, uint8_t pos, RequestLine& out) {
  if (valve >= kValveCount || pos > 100) return fail(out);
  const uint32_t n[] = {valve, pos};
  return buildNumeric(Cmd::Stgtp, valve, pos, n, 2, out);
}

bool buildGetTarget(uint8_t valve, RequestLine& out) { return buildValveArg(Cmd::Gtgtp, valve, out); }
bool buildValveData(uint8_t valve, RequestLine& out) { return buildValveArg(Cmd::Gvlvd, valve, out); }
bool buildValveStates(RequestLine& out) { return buildBare(Cmd::Gvlst, out); }
bool buildTempCount(RequestLine& out) { return buildBare(Cmd::Gonec, out); }
bool buildTempList(RequestLine& out) { return buildListRequest(Cmd::Gonec, out); }
bool buildTempData(uint8_t busIndex, RequestLine& out) {
  return buildIndexArg(Cmd::Goned, busIndex, kMaxTempBusIndex, out);
}
bool buildValveSensors(uint8_t valveOrAll, RequestLine& out) {
  return buildValveOrAll(Cmd::Gvlon, valveOrAll, out);
}
bool buildVoltCount(RequestLine& out) { return buildBare(Cmd::Gowvc, out); }
bool buildVoltList(RequestLine& out) { return buildListRequest(Cmd::Gowvc, out); }
bool buildVoltData(uint8_t busIndex, RequestLine& out) {
  return buildIndexArg(Cmd::Gowvd, busIndex, kMaxVoltBusIndex, out);
}
bool buildScanOneWire(RequestLine& out) { return buildBare(Cmd::Stons, out); }

bool buildSetValveSensors(uint8_t valve, const OneWireId& s1, const OneWireId& s2,
                          RequestLine& out) {
  if (valve >= kValveCount) return fail(out);
  if ((!isZero(s1) && !crcValid(s1)) || (!isZero(s2) && !crcValid(s2))) return fail(out);
  out = RequestLine{};
  LineBuilder b(out);
  b.token(cmdName(Cmd::Stvls));
  b.number(valve);
  b.id(s1);
  b.id(s2);
  if (!b.finish()) return fail(out);  // NOMUTATE: unreachable guard (see finish())
  out.cmd = Cmd::Stvls;
  out.valve = valve;
  return true;
}

bool buildMatchSensors(RequestLine& out) { return buildBare(Cmd::Masns, out); }
bool buildAssembly(uint8_t valveOrAll, RequestLine& out) {
  return buildValveOrAll(Cmd::Staop, valveOrAll, out);
}
bool buildCalibrate(uint8_t valveOrAll, RequestLine& out) {
  return buildValveOrAll(Cmd::Staln, valveOrAll, out);
}
bool buildDetect(RequestLine& out) { return buildValveOrAll(Cmd::Stdet, kAllValves, out); }

bool buildSetLearnMovements(uint32_t n, RequestLine& out) {
  if (!learnMovementsValid(n)) return fail(out);
  return buildNumeric(Cmd::Stlnm, kNoValve, static_cast<uint16_t>(n), &n, 1, out);
}

bool buildGetLearnMovements(RequestLine& out) { return buildBare(Cmd::Gtlnm, out); }

bool buildSetMotorChars(const MotorChars& m, RequestLine& out) {
  if (!motorCharsValid(m)) return fail(out);
  const uint32_t n[] = {m.lowFactor, m.highFactor, m.startOnPower, m.minCounts,
                        m.maxCalibRetries};
  return buildNumeric(Cmd::Smotc, kNoValve, m.lowFactor, n, 5, out);
}

bool buildGetMotorChars(RequestLine& out) { return buildBare(Cmd::Gmotc, out); }
bool buildGetVersion(RequestLine& out) { return buildBare(Cmd::Gvers, out); }
bool buildGetHwId(RequestLine& out) { return buildBare(Cmd::Ghwin, out); }
bool buildEepromState(RequestLine& out) { return buildBare(Cmd::Eepst, out); }
bool buildSoftReset(RequestLine& out) { return buildBare(Cmd::Reset, out); }
bool buildGetProto(RequestLine& out) { return buildBare(Cmd::Gproto, out); }
bool buildValveEx(uint8_t valve, RequestLine& out) { return buildValveArg(Cmd::Gvlvx, valve, out); }
bool buildProfile(uint8_t valve, RequestLine& out) { return buildValveArg(Cmd::Gprof, valve, out); }

bool buildServiceMove(uint8_t valve, MoveDir dir, uint16_t counts, uint8_t maxmA,
                      RequestLine& out) {
  const uint8_t d = static_cast<uint8_t>(dir);
  if (valve >= kValveCount || d > 1 || counts < 1 || counts > 10000 || maxmA < 5 || maxmA > 60) {
    return fail(out);
  }
  const uint32_t n[] = {valve, d, counts, maxmA};
  return buildNumeric(Cmd::Svmov, valve, d, n, 4, out);
}

bool buildSetBreakaway(const Breakaway& b, RequestLine& out) {
  if (!breakawayValid(b)) return fail(out);
  const uint32_t n[] = {b.enable ? 1u : 0u, b.stepPct, b.maxmA};
  return buildNumeric(Cmd::Scalx, kNoValve, static_cast<uint16_t>(n[0]), n, 3, out);
}

bool buildGetBreakaway(RequestLine& out) { return buildBare(Cmd::Gcalx, out); }
bool buildGetStatus(RequestLine& out) { return buildBare(Cmd::Gstat, out); }

// ---------------------------------------------------------------- replies

const char* parseStatusName(ParseStatus s) {
  switch (s) {
    case ParseStatus::Ok: return "ok";
    case ParseStatus::Empty: return "empty";
    case ParseStatus::UnknownCommand: return "unknown_command";
    case ParseStatus::BadArgCount: return "bad_arg_count";
    case ParseStatus::BadNumber: return "bad_number";
    case ParseStatus::OutOfRange: return "out_of_range";
    case ParseStatus::BadOneWireId: return "bad_onewire_id";
    case ParseStatus::BadFormat: return "bad_format";
    case ParseStatus::TooLong: return "too_long";
  }
  return "unknown";
}

const char* stopReasonName(StopReason r) {
  switch (r) {
    case StopReason::None: return "none";
    case StopReason::Target: return "target";
    case StopReason::EndStop: return "endstop";
    case StopReason::EarlyEndStop: return "early_endstop";
    case StopReason::Timeout: return "timeout";
    case StopReason::UnderCurrent: return "undercurrent";
    case StopReason::SafetyOverCurrent: return "safety_overcurrent";
    case StopReason::Aborted: return "aborted";
  }
  return "unknown";
}

ParseStatus parseReply(const char* line, size_t len, Reply& out) {
  out = kEmptyReply;
  if (line == nullptr) return ParseStatus::Empty;
  if (len > kStmMaxLineLen) return ParseStatus::TooLong;

  Token tok[kMaxTokens];
  size_t count = 0;
  size_t i = 0;
  while (i < len) {
    if (line[i] == ' ') {
      ++i;
      continue;
    }
    if (count == kMaxTokens) return ParseStatus::TooLong;
    const size_t start = i;
    while (i < len && line[i] != ' ') ++i;
    tok[count++] = Token{line + start, i - start};
  }
  if (count == 0) return ParseStatus::Empty;

  const Cmd cmd = cmdFromName(tok[0].p, tok[0].n);
  if (cmd == Cmd::None) return ParseStatus::UnknownCommand;
  const Token* args = tok + 1;
  const size_t argc = count - 1;

  // v1 quirk: the gvlon error reply carries the goned prefix.
  if (cmd == Cmd::Goned && argc == 1 && tokenIs(args[0], "error")) {
    out.cmd = Cmd::Gvlon;
    out.gvlonError = true;
    return ParseStatus::Ok;
  }

  const ParseStatus st = parsePayload(cmd, args, argc, out);
  if (st != ParseStatus::Ok) {
    out = kEmptyReply;
    return st;
  }
  out.cmd = cmd;
  return ParseStatus::Ok;
}

bool replyMatches(const RequestLine& req, const Reply& rep) {
  if (req.cmd == Cmd::None || req.len == 0) return false;
  if (rep.cmd != req.cmd) return false;
  switch (req.cmd) {
    case Cmd::Gvlvd:
      return rep.valveData.valve == req.valve;
    case Cmd::Gvlvx:
      return rep.valveEx.valve == req.valve;
    case Cmd::Gtgtp:
      return rep.target.valve == req.valve;
    case Cmd::Gprof:
      return rep.profile.valve == req.valve;
    case Cmd::Svmov:
      return rep.serviceMove.valve == req.valve;
    case Cmd::Stvls:
      return rep.ack.valve == req.valve;
    case Cmd::Gvlon:
      if (rep.gvlonError) return true;
      if (req.valve == kAllValves) return rep.valveSensors.isList;
      return !rep.valveSensors.isList && rep.valveSensors.valve == req.valve;
    case Cmd::Gonec:
    case Cmd::Gowvc:
      if (req.arg == kAllValves) return rep.oneWireList.hasList || rep.oneWireList.count == 0;
      return !rep.oneWireList.hasList;
    default:
      return true;
  }
}

uint8_t resolveTempSlot(const OneWireId& id, const OneWireId* slotIds, uint8_t slotCount) {
  if (slotIds == nullptr || isZero(id) || !crcValid(id)) return 0;
  for (uint8_t i = 0; i < slotCount; ++i) {
    if (slotIds[i] == id) return static_cast<uint8_t>(i + 1);
  }
  return 0;
}

const char* stmChipName(uint16_t pid) {
  switch (pid) {
    case 0x413: return "STM32F40xx/41xx";
    case 0x423: return "STM32F401xB/C";
    case 0x431: return "STM32F411xx";
    case 0x433: return "STM32F401xD/E";
    default: return "Unknown Chip";
  }
}

}  // namespace vdm
