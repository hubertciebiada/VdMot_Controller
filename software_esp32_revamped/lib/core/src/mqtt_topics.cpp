#include "vdm/mqtt_topics.h"

#include <math.h>
#include <stdarg.h>
#include <stdio.h>
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

// Valid topic segment: 1..kSegmentMax chars, none of NUL, '/', '+', '#'.
bool segmentValid(const char* seg, size_t& len) {
  if (seg == nullptr) return false;
  len = boundedLength(seg, kSegmentMax + 1);
  if (len == 0 || len > kSegmentMax) return false;
  for (size_t i = 0; i < len; ++i) {
    if (seg[i] == '/' || seg[i] == '+' || seg[i] == '#') return false;
  }
  return true;
}

struct TopicDef {
  const char* head;  // path before the segment (or the whole path)
  const char* tail;  // path after the segment; nullptr = no segment
};

// Indexed by Topic.
const TopicDef kTopics[kTopicCount] = {
    {"common/ip", nullptr},
    {"common/state", nullptr},
    {"common/uptime", nullptr},
    {"common/message", nullptr},
    {"valves/", "/target"},
    {"valves/", "/state"},
    {"valves/", "/calibration/date"},
    {"valves/", "/calibration/repetitions"},
    {"valves/", "/diag/meanCurrrent"},
    {"valves/", "/diag/openCount"},
    {"valves/", "/diag/closeCount"},
    {"valves/", "/diag/deadZoneCount"},
    {"valves/", "/diag/moves"},
    {"valves/", "/temp1"},
    {"valves/", "/temp2"},
    {"valves/", "/actual"},
    {"temps/", "/id"},
    {"temps/", "/value"},
    {"sensors/", "/id"},
    {"sensors/", "/value"},
    {"sensors/", "/unit"},
    {"diag/valves/", "/lastMove"},
    {"diag/valves/", "/earlyStops"},
    {"diag/valves/", "/cmdRejected"},
    {"diag/valves/", "/calState"},
    {"diag/valves/", "/profile"},
    {"diag/stm/proto", nullptr},
    {"diag/stm/uptime", nullptr},
    {"diag/stm/resets", nullptr},
    {"diag/stm/rxOverflow", nullptr},
    {"diag/stm/parseErr", nullptr},
    {"diag/stm/link", nullptr},
    {"diag/calibration/active", nullptr},
    {"events", nullptr},
    {"status", nullptr},
};
static_assert(static_cast<uint8_t>(Topic::Status) + 1 == kTopicCount, "kTopics out of sync");

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

bool topicIsCompat(Topic t) { return static_cast<uint8_t>(t) <= static_cast<uint8_t>(Topic::VoltUnit); }

bool topicRetained(Topic t, bool publishRetained) {
  switch (t) {
    case Topic::Status:
    case Topic::DiagCalibrationActive:
      return true;
    case Topic::Events:
    case Topic::DiagValveProfile:
      return false;
    default:
      return static_cast<uint8_t>(t) < kTopicCount && publishRetained;
  }
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

int parseTargetCommandTopic(const TopicContext& ctx, const char* topic, size_t len,
                            const char segments[kValveCount][kSegmentMax + 1]) {
  if (topic == nullptr || len == 0) return -1;
  if (memchr(topic, '\0', len) != nullptr) return -1;
  if (topic[0] == '/') {
    ++topic;
    --len;
  }
  char main[kTopicMax + 1];
  size_t ml = buildMainTopic(ctx, main, sizeof main);
  if (ml == 0) return -1;
  const char* mp = main;
  if (mp[0] == '/') {
    ++mp;
    --ml;
  }
  static const char kValves[] = "valves/";
  const size_t vl = sizeof kValves - 1;
  if (len < ml + vl || memcmp(topic, mp, ml) != 0 || memcmp(topic + ml, kValves, vl) != 0) {
    return -1;
  }
  const char* seg = topic + ml + vl;
  const char* end = topic + len;
  const char* slash = static_cast<const char*>(memchr(seg, '/', static_cast<size_t>(end - seg)));
  if (slash == nullptr) return -1;
  const size_t segLen = static_cast<size_t>(slash - seg);
  if (segLen == 0 || segLen > kSegmentMax) return -1;
  const char* rest = slash + 1;
  const size_t restLen = static_cast<size_t>(end - rest);
  const bool suffixOk =
      ctx.separate ? (bytesEqual(rest, restLen, "target/set") || bytesEqual(rest, restLen, "target/set/set"))
                   : (bytesEqual(rest, restLen, "target") || bytesEqual(rest, restLen, "target/set"));
  if (!suffixOk) return -1;
  if (segments != nullptr) {
    for (uint8_t i = 0; i < kValveCount; ++i) {
      const size_t l = boundedLength(segments[i], kSegmentMax);
      if (l == segLen && memcmp(segments[i], seg, segLen) == 0) return i;
    }
  }
  uint32_t n = 0;
  if (seg[0] == '0' || !parseUint(seg, segLen, kValveCount, n) || n == 0) return -1;
  return static_cast<int>(n - 1);
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
  size_t i = 0;
  uint32_t v = 0;
  while (i < n && s[i] >= '0' && s[i] <= '9') {
    if (v <= 100) v = v * 10 + static_cast<uint32_t>(s[i] - '0');
    ++i;
  }
  if (i == 0) return TargetPayload::NotNumber;
  if (i < n) {
    if (s[i] != '.' || i + 1 == n) return TargetPayload::NotNumber;
    for (size_t k = i + 1; k < n; ++k) {
      if (s[k] != '0') return TargetPayload::NotNumber;
    }
  }
  if (v > 100) return TargetPayload::OutOfRange;
  out = static_cast<uint8_t>(v);
  return TargetPayload::Ok;
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
