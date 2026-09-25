#include "vdm/mqtt_topics.h"

#include <math.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "vdm/valve_model.h"

namespace vdm {

namespace {

constexpr char kFallbackMain[] = "VdMotFBH/";
constexpr size_t kTargetPayloadMax = 16;

// Bounded topic builder: after an overflow or fail() the result is "" and
// length 0. cap may be 0 (nothing is written).
class Builder {
 public:
  Builder(char* out, size_t cap) : out_(out), cap_(cap), ok_(cap > 0) {
    if (ok_) out_[0] = '\0';
  }
  void add(const char* s, size_t n) {
    if (!ok_) return;
    if (n >= cap_ - len_) {
      ok_ = false;
      return;
    }
    memcpy(out_ + len_, s, n);
    len_ += n;
    out_[len_] = '\0';
  }
  void add(const char* s) {
    if (ok_) add(s, strlen(s));
  }
  void fail() { ok_ = false; }
  size_t finish() {
    if (ok_) return len_;
    if (cap_ > 0) out_[0] = '\0';
    return 0;
  }

 private:
  char* out_;
  size_t cap_;
  size_t len_ = 0;
  bool ok_;
};

// `out` is never null here (callers check).
size_t emptyOut(char* out, size_t cap) {
  if (cap > 0) out[0] = '\0';
  return 0;
}

// snprintf into `out`; 0 (and "") when it does not fit.
size_t putf(char* out, size_t cap, const char* fmt, ...) __attribute__((format(printf, 3, 4)));  // NOMUTATE: attribute
size_t putf(char* out, size_t cap, const char* fmt, ...) {
  if (out == nullptr || cap == 0) return 0;
  va_list ap;
  va_start(ap, fmt);
  const int n = vsnprintf(out, cap, fmt, ap);
  va_end(ap);
  if (static_cast<size_t>(n) < cap) return static_cast<size_t>(n);  // n < 0 fails here too
  return emptyOut(out, cap);
}

size_t putText(char* out, size_t cap, const char* text) { return putf(out, cap, "%s", text); }

// Valid topic segment (topicSegmentValid) of a NUL-terminated string.
bool segmentValid(const char* seg, size_t& len) {
  if (seg == nullptr) return false;
  len = boundedLength(seg, kSegmentMax + 1);
  return topicSegmentValid(seg, len);
}

enum class RetainRule : uint8_t { Setting, Always, Never };

struct TopicDef {
  const char* head;  // path before the segment (or the whole path)
  const char* tail;  // path after the segment; nullptr = no segment
  bool suffix;       // "/value" appended with `separate`
  RetainRule retain;
};

constexpr RetainRule S = RetainRule::Setting;
constexpr RetainRule A = RetainRule::Always;
constexpr RetainRule N = RetainRule::Never;

// Indexed by Topic.
const TopicDef kTopics[kTopicCount] = {
    {"common/ip", nullptr, true, S},
    {"common/state", nullptr, true, S},
    {"common/uptime", nullptr, true, S},
    {"common/message", nullptr, true, S},
    {"valves/", "/target", true, S},
    {"valves/", "/state", true, S},
    {"valves/", "/calibration/date", true, S},
    {"valves/", "/calibration/repetitions", true, S},
    {"valves/", "/diag/meanCurrrent", true, S},
    {"valves/", "/diag/openCount", true, S},
    {"valves/", "/diag/closeCount", true, S},
    {"valves/", "/diag/deadZoneCount", true, S},
    {"valves/", "/diag/moves", true, S},
    {"valves/", "/temp1", true, S},
    {"valves/", "/temp2", true, S},
    {"valves/", "/actual", true, S},
    {"temps/", "/id", true, S},
    {"temps/", "/value", true, S},
    {"sensors/", "/id", true, S},
    {"sensors/", "/value", true, S},
    {"sensors/", "/unit", true, S},
    {"diag/valves/", "/lastMove", false, S},
    {"diag/valves/", "/earlyStops", false, S},
    {"diag/valves/", "/cmdRejected", false, S},
    {"diag/valves/", "/calState", false, S},
    {"diag/valves/", "/profile", false, N},
    {"diag/stm/proto", nullptr, false, S},
    {"diag/stm/uptime", nullptr, false, S},
    {"diag/stm/resets", nullptr, false, S},
    {"diag/stm/rxOverflow", nullptr, false, S},
    {"diag/stm/parseErr", nullptr, false, S},
    {"diag/stm/link", nullptr, false, S},
    {"diag/calibration/active", nullptr, false, A},
    {"events", nullptr, false, N},
    {"status", nullptr, false, A},
    {"valves/", "/requested", true, S},
    {"valves/", "/sync", true, S},
    {"valves/", "/failsafe", true, S},
    {"valves/", "/problem", true, S},
    {"stm/status", nullptr, false, A},
    {"failsafe", nullptr, false, A},
    {"diag/stm/version", nullptr, false, S},
    {"diag/stm/started", nullptr, false, S},
    {"diag/stm/lease", nullptr, false, S},
    {"diag/stm/safeMode", nullptr, false, S},
    {"diag/mqtt/eventsSuppressed", nullptr, false, S},
    {"diag/mqtt/commandsRejected", nullptr, false, S},
    {"diag/calibration/next", nullptr, false, S},
    {"cmd/valves/", "/calibrate", false, N},
    {"cmd/calibrate", nullptr, false, N},
    {"cmd/restart", nullptr, false, N},
    {"cmd/stmReset", nullptr, false, N},
    {"cmd/detect", nullptr, false, N},
    {"cmd/stop", nullptr, false, N},
    {"cmd/stmSafeExit", nullptr, false, N},
};
static_assert(static_cast<uint8_t>(Topic::CmdStmSafeExit) + 1 == kTopicCount, "kTopics out of sync");

// Main topic + path (without the compat suffix); fails `b` on bad input.
void addBase(Builder& b, const TopicContext& ctx, Topic t, const char* segment) {
  const uint8_t i = static_cast<uint8_t>(t);
  if (i >= kTopicCount) {
    b.fail();
    return;
  }
  char main[kTopicMax + 1];
  const size_t ml = buildMainTopic(ctx, main, sizeof main);
  if (ml == 0) b.fail();
  b.add(main, ml);
  const TopicDef& d = kTopics[i];
  b.add(d.head);
  if (d.tail != nullptr) {
    size_t sl = 0;
    if (!segmentValid(segment, sl)) b.fail();
    b.add(segment, sl);
    b.add(d.tail);
  }
}

bool isSpace(char c) { return c == ' ' || c == '\t' || c == '\r' || c == '\n'; }

bool bytesEqual(const char* a, size_t alen, const char* b) {
  const size_t blen = strlen(b);
  return alen == blen && memcmp(a, b, alen) == 0;
}

}  // namespace

size_t buildMainTopic(const TopicContext& ctx, char* out, size_t cap) {
  if (out == nullptr) return 0;
  Builder b(out, cap);
  const bool named = ctx.station[0] != '\0';
  // isSafeName also bounds the length, so the add() below is safe.
  if (named && !isSafeName(ctx.station, kStationNameMax, false)) b.fail();
  if (ctx.pathAsRoot) b.add("/");
  if (named) {
    b.add(ctx.station);
    b.add("/");
  } else {
    b.add(kFallbackMain);
  }
  return b.finish();
}

size_t buildSegment(const char* name, uint8_t idx0, char* out, size_t cap) {
  if (out == nullptr || cap == 0) return 0;
  if (name == nullptr || name[0] == '\0') {
    const int n = snprintf(out, cap, "%u", static_cast<unsigned>(idx0) + 1u);
    if (static_cast<size_t>(n) < cap) return static_cast<size_t>(n);
    return emptyOut(out, cap);
  }
  if (!isSafeName(name, kSegmentMax, false)) return emptyOut(out, cap);
  const size_t len = strlen(name);
  if (len >= cap) return emptyOut(out, cap);
  for (size_t i = 0; i < len; ++i) out[i] = name[i] == ' ' ? '_' : name[i];
  out[len] = '\0';
  return len;
}

bool topicSegmentValid(const char* seg, size_t len) {
  if (seg == nullptr || len == 0 || len > kSegmentMax) return false;
  if (seg[0] == '/' || seg[len - 1] == '/') return false;
  for (size_t i = 0; i < len; ++i) {
    const char c = seg[i];
    if (c == '\0' || c == '+' || c == '#') return false;
    if (c == '/' && seg[i + 1] == '/') return false;  // i + 1 < len: the last byte is no '/'
  }
  return true;
}

bool topicIsCompat(Topic t) {
  const uint8_t i = static_cast<uint8_t>(t);
  return i < kTopicCount && kTopics[i].suffix;
}

bool topicRetained(Topic t, bool publishRetained) {
  const uint8_t i = static_cast<uint8_t>(t);
  if (i >= kTopicCount) return false;
  switch (kTopics[i].retain) {
    case RetainRule::Always: return true;
    case RetainRule::Never: return false;
    case RetainRule::Setting: break;
  }
  return publishRetained;
}

size_t buildTopic(const TopicContext& ctx, Topic t, const char* segment, char* out, size_t cap) {
  if (out == nullptr) return 0;
  Builder b(out, cap);
  addBase(b, ctx, t, segment);
  if (ctx.separate && topicIsCompat(t)) b.add("/value");
  return b.finish();
}

size_t buildTargetCommandTopic(const TopicContext& ctx, const char* segment, char* out,
                               size_t cap) {
  if (out == nullptr) return 0;
  Builder b(out, cap);
  addBase(b, ctx, Topic::ValveTarget, segment);
  if (ctx.separate) b.add("/set");
  return b.finish();
}

size_t buildHaStatusTopic(const char* prefix, char* out, size_t cap) {
  if (out == nullptr) return 0;
  Builder b(out, cap);
  if (prefix == nullptr || prefix[0] == '\0') b.fail();
  b.add(prefix);
  b.add("/status");
  return b.finish();
}

namespace {

constexpr char kHaDefaultPrefix[] = "homeassistant";

// Appends one entry (skipped when it does not fit or the filter is empty).
void addSubscription(Subscription* out, size_t cap, size_t& n, const char* filter, size_t len,
                     uint8_t qos) {
  if (n >= cap || len == 0) return;
  memcpy(out[n].filter, filter, len + 1);
  out[n].qos = qos;
  ++n;
}

// "<main>valves/<seg>/target[/set]" and the same + "/set".
void addTargetFilters(const TopicContext& ctx, const char* main, size_t ml, const char* seg,
                      Subscription* out, size_t cap, size_t& n) {
  char f[kTopicMax + 1];
  Builder b(f, sizeof f);
  b.add(main, ml);
  b.add("valves/");
  b.add(seg);
  b.add(ctx.separate ? "/target/set" : "/target");
  addSubscription(out, cap, n, f, b.finish(), 1);
  b.add("/set");
  addSubscription(out, cap, n, f, b.finish(), 1);
}

bool startsWith(const char* s, size_t n, const char* prefix, size_t pl) {
  return n >= pl && memcmp(s, prefix, pl) == 0;
}

bool endsWith(const char* s, size_t n, const char* suffix, size_t sl) {
  return n >= sl && memcmp(s + n - sl, suffix, sl) == 0;
}

// The valve a segment names: configured segments first, then 1..12.
int8_t matchSegment(const char* seg, size_t len, const char segments[kValveCount][kSegmentMax + 1]) {
  if (segments != nullptr) {
    for (uint8_t i = 0; i < kValveCount; ++i) {
      const size_t l = boundedLength(segments[i], kSegmentMax);
      if (l == len && memcmp(segments[i], seg, len) == 0) return static_cast<int8_t>(i);
    }
  }
  uint32_t n = 0;
  if (seg[0] == '0' || !parseUint(seg, len, kValveCount, n) || n == 0) return -1;
  return static_cast<int8_t>(n - 1);
}

struct CmdName {
  const char* name;
  InboundKind kind;
};

const CmdName kCmdNames[] = {
    {"calibrate", InboundKind::CalibrateAll}, {"restart", InboundKind::Restart},
    {"stmReset", InboundKind::StmReset},      {"detect", InboundKind::Detect},
    {"stop", InboundKind::StopAll},           {"stmSafeExit", InboundKind::StmSafeExit},
};

}  // namespace

size_t buildSubscriptions(const TopicContext& ctx, MqttMode mode, const char* haPrefix,
                          const char segments[kValveCount][kSegmentMax + 1], Subscription* out,
                          size_t cap) {
  if (out == nullptr || mode == MqttMode::Off) return 0;
  size_t n = 0;
  char main[kTopicMax + 1];
  const size_t ml = buildMainTopic(ctx, main, sizeof main);
  if (ml > 0) {
    addTargetFilters(ctx, main, ml, "+", out, cap, n);
    char f[kTopicMax + 1];
    Builder b(f, sizeof f);
    b.add(main, ml);
    b.add("cmd/#");
    addSubscription(out, cap, n, f, b.finish(), 0);
    for (uint8_t i = 0; segments != nullptr && i < kValveCount; ++i) {
      const size_t sl = boundedLength(segments[i], kSegmentMax);
      if (memchr(segments[i], '/', sl) == nullptr || !topicSegmentValid(segments[i], sl)) continue;
      char seg[kSegmentMax + 1];
      memcpy(seg, segments[i], sl);
      seg[sl] = '\0';
      addTargetFilters(ctx, main, ml, seg, out, cap, n);
    }
  }
  if (mode == MqttMode::MqttHa) {
    char f[kTopicMax + 1];
    addSubscription(out, cap, n, f, buildHaStatusTopic(kHaDefaultPrefix, f, sizeof f), 1);
    if (haPrefix != nullptr && strcmp(haPrefix, kHaDefaultPrefix) != 0) {
      addSubscription(out, cap, n, f, buildHaStatusTopic(haPrefix, f, sizeof f), 1);
    }
  }
  return n;
}

InboundTopic parseInboundTopic(const TopicContext& ctx, const char* haPrefix, const char* topic,
                               size_t len, const char segments[kValveCount][kSegmentMax + 1]) {
  InboundTopic r;
  if (topic == nullptr || len == 0 || len > kTopicMax || memchr(topic, '\0', len) != nullptr) {
    return r;
  }
  char ha[kTopicMax + 1];
  size_t hl = buildHaStatusTopic(kHaDefaultPrefix, ha, sizeof ha);
  const bool defaultHa = hl == len && memcmp(topic, ha, len) == 0;
  hl = buildHaStatusTopic(haPrefix, ha, sizeof ha);
  if (defaultHa || (hl == len && memcmp(topic, ha, len) == 0)) {
    r.kind = InboundKind::HaStatus;
    return r;
  }
  if (topic[0] == '/') {
    ++topic;
    --len;
  }
  char main[kTopicMax + 1];
  size_t ml = buildMainTopic(ctx, main, sizeof main);
  if (ml == 0) return r;
  const char* mp = main;
  if (mp[0] == '/') {
    ++mp;
    --ml;
  }
  if (!startsWith(topic, len, mp, ml)) return r;
  const char* rest = topic + ml;
  size_t n = len - ml;
  if (startsWith(rest, n, "valves/", 7)) {
    rest += 7;
    n -= 7;
    static const char* const kSeparate[] = {"/target/set", "/target/set/set"};
    static const char* const kPlain[] = {"/target", "/target/set"};
    const char* const* suffixes = ctx.separate ? kSeparate : kPlain;
    for (uint8_t k = 0; k < 2; ++k) {
      const size_t sl = strlen(suffixes[k]);
      if (n <= sl || !endsWith(rest, n, suffixes[k], sl)) continue;
      r.kind = InboundKind::Target;
      r.valve = matchSegment(rest, n - sl, segments);
      r.stateForm = !ctx.separate && k == 0;
      return r;
    }
    return r;
  }
  if (!startsWith(rest, n, "cmd/", 4)) return r;
  rest += 4;
  n -= 4;
  if (startsWith(rest, n, "valves/", 7) && n > 7 + 10 && endsWith(rest, n, "/calibrate", 10)) {
    r.kind = InboundKind::CalibrateValve;
    r.valve = matchSegment(rest + 7, n - 7 - 10, segments);
    return r;
  }
  r.kind = InboundKind::UnknownCommand;
  for (const CmdName& c : kCmdNames) {
    if (bytesEqual(rest, n, c.name)) r.kind = c.kind;
  }
  return r;
}

TargetPayload parseTargetPayload(const char* p, size_t len, uint8_t& out) {
  if (p == nullptr || len == 0) return TargetPayload::Empty;
  if (len > kTargetPayloadMax) return TargetPayload::NotNumber;
  size_t b = 0;
  size_t e = len;
  while (b < e && isSpace(p[b])) ++b;
  while (e > b && isSpace(p[e - 1])) --e;
  if (b == e) return TargetPayload::Empty;
  const char* s = p + b;
  const size_t n = e - b;
  if (bytesEqual(s, n, "OPEN")) {
    out = 100;
    return TargetPayload::Ok;
  }
  if (bytesEqual(s, n, "CLOSE")) {
    out = 0;
    return TargetPayload::Ok;
  }
  if (bytesEqual(s, n, "STOP")) return TargetPayload::Stop;
  char num[kTargetPayloadMax + 1];
  size_t i = 0;
  while (i < n && s[i] >= '0' && s[i] <= '9') {
    num[i] = s[i];
    ++i;
  }
  if (i == 0) return TargetPayload::NotNumber;
  if (i < n) {
    if (s[i] != '.' && s[i] != ',') return TargetPayload::NotNumber;
    num[i++] = '.';
    const size_t first = i;
    while (i < n && s[i] >= '0' && s[i] <= '9') {
      num[i] = s[i];
      ++i;
    }
    if (i == first || i < n) return TargetPayload::NotNumber;
  }
  num[i] = '\0';
  uint8_t v = 0;
  if (!roundTargetPercent(strtod(num, nullptr), v)) return TargetPayload::OutOfRange;
  out = v;
  return TargetPayload::Ok;
}

bool parseButtonPayload(const char* p, size_t len) {
  return p != nullptr && bytesEqual(p, len, "PRESS");
}

// ---------------------------------------------------------------- payloads

size_t formatTemp(int32_t tenths, bool valid, bool germanComma, char* out, size_t cap) {
  if (!valid) return putText(out, cap, "failed");
  const bool neg = tenths < 0;
  const uint32_t mag = neg ? 0u - static_cast<uint32_t>(tenths) : static_cast<uint32_t>(tenths);
  return putf(out, cap, "%s%lu%c%lu", neg ? "-" : "", static_cast<unsigned long>(mag / 10),
              germanComma ? ',' : '.', static_cast<unsigned long>(mag % 10));
}

size_t formatVolt(double value, bool valid, bool germanComma, char* out, size_t cap) {
  if (!valid || !isfinite(value)) return putText(out, cap, "failed");
  const size_t n = putf(out, cap, "%.3f", value);
  if (germanComma) {
    char* dot = strchr(out, '.');
    if (dot != nullptr) *dot = ',';
  }
  return n;
}

size_t formatUptime(uint32_t seconds, char* out, size_t cap) {
  return putf(out, cap, "%lud %lu:%02lu:%02lu", static_cast<unsigned long>(seconds / 86400u),
              static_cast<unsigned long>(seconds % 86400u / 3600u),
              static_cast<unsigned long>(seconds % 3600u / 60u),
              static_cast<unsigned long>(seconds % 60u));
}

size_t formatValveState(uint8_t status, bool plainText, char* out, size_t cap) {
  if (plainText) return putText(out, cap, valveStatusText(status));
  return putf(out, cap, "%u", static_cast<unsigned>(status));
}

size_t formatSystemState(uint8_t state, bool plainText, char* out, size_t cap) {
  if (plainText) {
    static const char* const kNames[] = {"ok", "info", "error"};
    return putText(out, cap, state < 3 ? kNames[state] : "");
  }
  return putf(out, cap, "%u", static_cast<unsigned>(state));
}

size_t formatCalibDate(const LocalTime& t, char* out, size_t cap) {
  static const char* const kDays[] = {"Sunday",   "Monday", "Tuesday", "Wednesday",
                                      "Thursday", "Friday", "Saturday"};
  static const char* const kMonths[] = {"January", "February", "March",     "April",
                                        "May",     "June",     "July",      "August",
                                        "September", "October", "November", "December"};
  if (!t.valid || t.wday > 6 || t.month < 1 || t.month > 12 || t.mday < 1 || t.mday > 31 ||
      t.hour > 23 || t.minute > 59 || t.second > 60) {
    return putText(out, cap, "Failed to obtain time");
  }
  return putf(out, cap, "%s, %s %02u.%u %02u:%02u:%02u", kDays[t.wday], kMonths[t.month - 1],
              static_cast<unsigned>(t.mday), static_cast<unsigned>(t.year),
              static_cast<unsigned>(t.hour), static_cast<unsigned>(t.minute),
              static_cast<unsigned>(t.second));
}

size_t formatLegacyCounter(uint32_t v, char* out, size_t cap) {
  // itoa(int) semantics: the bit pattern read as int32.
  const int64_t s = v <= 0x7FFFFFFFu ? static_cast<int64_t>(v)
                                     : static_cast<int64_t>(v) - 4294967296LL;
  return putf(out, cap, "%lld", static_cast<long long>(s));
}

// ---------------------------------------------------------------- scheduling

void PublishScheduler::configure(const Params& p) {
  p_ = p;
  if (p_.publishIntervalMs < 2000) p_.publishIntervalMs = 2000;
  if (p_.minDelayMs > p_.publishIntervalMs) p_.minDelayMs = p_.publishIntervalMs;
}

void PublishScheduler::onConnected(uint32_t nowMs) {
  (void)nowMs;
  forceFull_ = true;
  for (bool& s : itemSeen_) s = false;
}

bool PublishScheduler::takeFullPublish(uint32_t nowMs) {
  if (!forceFull_ && elapsedMs(nowMs, lastFullMs_) < p_.publishIntervalMs) return false;
  forceFull_ = false;
  lastFullMs_ = nowMs;
  return true;
}

bool PublishScheduler::takeItem(uint8_t slot, bool changed, uint32_t nowMs) {
  if (!p_.onChange || slot >= kSlots) return false;
  if (itemSeen_[slot]) {
    const uint32_t since = elapsedMs(nowMs, lastItemMs_[slot]);
    if (!(changed && since >= p_.minDelayMs) && since < p_.publishIntervalMs) return false;
  }
  itemSeen_[slot] = true;
  lastItemMs_[slot] = nowMs;
  return true;
}

void PublishScheduler::markAllPublished(uint32_t nowMs) {
  for (uint8_t i = 0; i < kSlots; ++i) {
    itemSeen_[i] = true;
    lastItemMs_[i] = nowMs;
  }
}

}  // namespace vdm
