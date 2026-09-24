// MQTT topic tree, command parsing, payload formatters and publish cadence.
// Golden strings follow specs/03-mqtt-ha-compat.md (legacy byte format).
#include <math.h>
#include <stdlib.h>
#include <string.h>

#include <initializer_list>
#include <string>
#include <vector>

#include "doctest.h"
#include "vdm/mqtt_topics.h"

using namespace vdm;

namespace {

TopicContext ctxOf(const char* station, bool separate = true, bool pathAsRoot = false) {
  TopicContext c;
  copyString(c.station, sizeof c.station, station);
  c.separate = separate;
  c.pathAsRoot = pathAsRoot;
  return c;
}

std::string topic(const TopicContext& c, Topic t, const char* seg = nullptr) {
  char buf[kTopicMax + 1];
  const size_t n = buildTopic(c, t, seg, buf, sizeof buf);
  CHECK(n == strlen(buf));
  return buf;
}

using Segments = char[kValveCount][kSegmentMax + 1];

void fillSegments(Segments& s, const char* const names[kValveCount]) {
  for (uint8_t i = 0; i < kValveCount; ++i) buildSegment(names ? names[i] : "", i, s[i], sizeof s[i]);
}

int parse(const TopicContext& c, const char* t, const Segments* s) {
  return parseTargetCommandTopic(c, t, strlen(t), s ? *s : nullptr);
}

TargetPayload payload(const char* p, uint8_t& out) { return parseTargetPayload(p, strlen(p), out); }

std::string fmtTemp(int32_t t, bool valid = true, bool german = false) {
  char buf[16];
  const size_t n = formatTemp(t, valid, german, buf, sizeof buf);
  CHECK(n == strlen(buf));
  return buf;
}

std::string fmtVolt(double v, bool valid = true, bool german = false) {
  char buf[32];
  const size_t n = formatVolt(v, valid, german, buf, sizeof buf);
  CHECK(n == strlen(buf));
  return buf;
}

}  // namespace

TEST_CASE("main topic") {
  char buf[kTopicMax + 1];
  CHECK(buildMainTopic(ctxOf("VdMot"), buf, sizeof buf) == 6);
  CHECK(std::string(buf) == "VdMot/");
  CHECK(buildMainTopic(ctxOf("VdMot", true, true), buf, sizeof buf) == 7);
  CHECK(std::string(buf) == "/VdMot/");
  CHECK(buildMainTopic(ctxOf(""), buf, sizeof buf) == 9);
  CHECK(std::string(buf) == "VdMotFBH/");
  CHECK(buildMainTopic(ctxOf("", true, true), buf, sizeof buf) == 10);
  CHECK(std::string(buf) == "/VdMotFBH/");
  CHECK(buildMainTopic(ctxOf("A"), buf, sizeof buf) == 2);
  CHECK(std::string(buf) == "A/");
  buildMainTopic(ctxOf("My Station"), buf, sizeof buf);
  CHECK(std::string(buf) == "My Station/");  // raw, like legacy
  buildMainTopic(ctxOf("abcdefghijklmnopqrst"), buf, sizeof buf);
  CHECK(std::string(buf) == "abcdefghijklmnopqrst/");
  for (const char* bad : {"a+b", "a#", "a/b", " a", "a ", "q\"", "b\\"}) {
    CHECK(buildMainTopic(ctxOf(bad), buf, sizeof buf) == 0);
    CHECK(buf[0] == '\0');
  }
  TopicContext unterminated;
  memset(unterminated.station, 'x', sizeof unterminated.station);
  CHECK(buildMainTopic(unterminated, buf, sizeof buf) == 0);
  // Capacity: exactly fits / one short.
  char seven[7];
  CHECK(buildMainTopic(ctxOf("VdMot"), seven, sizeof seven) == 6);
  char six[6];
  CHECK(buildMainTopic(ctxOf("VdMot"), six, sizeof six) == 0);
  CHECK(six[0] == '\0');
  CHECK(buildMainTopic(ctxOf("VdMot"), nullptr, 10) == 0);
  CHECK(buildMainTopic(ctxOf("VdMot"), buf, 0) == 0);
}

TEST_CASE("segments") {
  char buf[kSegmentMax + 1];
  CHECK(buildSegment("Bad 1", 0, buf, sizeof buf) == 5);
  CHECK(std::string(buf) == "Bad_1");
  CHECK(buildSegment("a b c", 0, buf, sizeof buf) == 5);
  CHECK(std::string(buf) == "a_b_c");
  CHECK(buildSegment("", 2, buf, sizeof buf) == 1);
  CHECK(std::string(buf) == "3");
  CHECK(buildSegment(nullptr, 0, buf, sizeof buf) == 1);
  CHECK(std::string(buf) == "1");
  CHECK(buildSegment("", 33, buf, sizeof buf) == 2);
  CHECK(std::string(buf) == "34");
  CHECK(buildSegment("", 255, buf, sizeof buf) == 3);
  CHECK(std::string(buf) == "256");
  CHECK(buildSegment("0123456789", 0, buf, sizeof buf) == 10);
  CHECK(buildSegment("0123456789a", 0, buf, sizeof buf) == 0);
  CHECK(buf[0] == '\0');
  for (const char* bad : {"a/b", "a+", "#", " a", "a ", "x\"", "x\\", "\x7f"}) {
    CHECK(buildSegment(bad, 0, buf, sizeof buf) == 0);
  }
  char small[3];
  CHECK(buildSegment("abc", 0, small, sizeof small) == 0);
  CHECK(small[0] == '\0');
  CHECK(buildSegment("ab", 0, small, sizeof small) == 2);
  CHECK(buildSegment("", 99, small, sizeof small) == 0);
  CHECK(buildSegment("", 8, small, sizeof small) == 1);
  CHECK(buildSegment("ab", 0, nullptr, 3) == 0);
  CHECK(buildSegment("ab", 0, small, 0) == 0);
}

TEST_CASE("compat and new topics, byte exact") {
  const TopicContext s = ctxOf("VdMot");
  const TopicContext p = ctxOf("VdMot", false);
  struct Row {
    Topic t;
    const char* seg;
    const char* separate;
    const char* plain;
  };
  const Row rows[] = {
      {Topic::CommonIp, nullptr, "VdMot/common/ip/value", "VdMot/common/ip"},
      {Topic::CommonState, nullptr, "VdMot/common/state/value", "VdMot/common/state"},
      {Topic::CommonUptime, nullptr, "VdMot/common/uptime/value", "VdMot/common/uptime"},
      {Topic::CommonMessage, nullptr, "VdMot/common/message/value", "VdMot/common/message"},
      {Topic::ValveTarget, "Bad_1", "VdMot/valves/Bad_1/target/value", "VdMot/valves/Bad_1/target"},
      {Topic::ValveState, "3", "VdMot/valves/3/state/value", "VdMot/valves/3/state"},
      {Topic::ValveCalibDate, "3", "VdMot/valves/3/calibration/date/value", "VdMot/valves/3/calibration/date"},
      {Topic::ValveCalibRepetitions, "3", "VdMot/valves/3/calibration/repetitions/value", "VdMot/valves/3/calibration/repetitions"},
      {Topic::ValveMeanCurrent, "3", "VdMot/valves/3/diag/meanCurrrent/value", "VdMot/valves/3/diag/meanCurrrent"},
      {Topic::ValveOpenCount, "3", "VdMot/valves/3/diag/openCount/value", "VdMot/valves/3/diag/openCount"},
      {Topic::ValveCloseCount, "3", "VdMot/valves/3/diag/closeCount/value", "VdMot/valves/3/diag/closeCount"},
      {Topic::ValveDeadZoneCount, "3", "VdMot/valves/3/diag/deadZoneCount/value", "VdMot/valves/3/diag/deadZoneCount"},
      {Topic::ValveMoves, "3", "VdMot/valves/3/diag/moves/value", "VdMot/valves/3/diag/moves"},
      {Topic::ValveTemp1, "3", "VdMot/valves/3/temp1/value", "VdMot/valves/3/temp1"},
      {Topic::ValveTemp2, "3", "VdMot/valves/3/temp2/value", "VdMot/valves/3/temp2"},
      {Topic::ValveActual, "3", "VdMot/valves/3/actual/value", "VdMot/valves/3/actual"},
      {Topic::TempId, "T_1", "VdMot/temps/T_1/id/value", "VdMot/temps/T_1/id"},
      {Topic::TempValue, "T_1", "VdMot/temps/T_1/value/value", "VdMot/temps/T_1/value"},
      {Topic::VoltId, "5", "VdMot/sensors/5/id/value", "VdMot/sensors/5/id"},
      {Topic::VoltValue, "5", "VdMot/sensors/5/value/value", "VdMot/sensors/5/value"},
      {Topic::VoltUnit, "5", "VdMot/sensors/5/unit/value", "VdMot/sensors/5/unit"},
      {Topic::DiagValveLastMove, "3", "VdMot/diag/valves/3/lastMove", nullptr},
      {Topic::DiagValveEarlyStops, "3", "VdMot/diag/valves/3/earlyStops", nullptr},
      {Topic::DiagValveCmdRejected, "3", "VdMot/diag/valves/3/cmdRejected", nullptr},
      {Topic::DiagValveCalState, "3", "VdMot/diag/valves/3/calState", nullptr},
      {Topic::DiagValveProfile, "3", "VdMot/diag/valves/3/profile", nullptr},
      {Topic::DiagStmProto, nullptr, "VdMot/diag/stm/proto", nullptr},
      {Topic::DiagStmUptime, nullptr, "VdMot/diag/stm/uptime", nullptr},
      {Topic::DiagStmResets, nullptr, "VdMot/diag/stm/resets", nullptr},
      {Topic::DiagStmRxOverflow, nullptr, "VdMot/diag/stm/rxOverflow", nullptr},
      {Topic::DiagStmParseErr, nullptr, "VdMot/diag/stm/parseErr", nullptr},
      {Topic::DiagStmLink, nullptr, "VdMot/diag/stm/link", nullptr},
      {Topic::DiagCalibrationActive, nullptr, "VdMot/diag/calibration/active", nullptr},
      {Topic::Events, nullptr, "VdMot/events", nullptr},
      {Topic::Status, nullptr, "VdMot/status", nullptr},
  };
  static_assert(sizeof rows / sizeof rows[0] == kTopicCount, "every topic covered");
  for (const Row& r : rows) {
    CAPTURE(r.separate);
    CHECK(topic(s, r.t, r.seg) == r.separate);
    CHECK(topic(p, r.t, r.seg) == (r.plain ? r.plain : r.separate));
    CHECK(topicIsCompat(r.t) == (r.plain != nullptr));
  }
  CHECK(topic(ctxOf("VdMot", true, true), Topic::ValveTarget, "1") == "/VdMot/valves/1/target/value");
  CHECK(topic(ctxOf(""), Topic::Status) == "VdMotFBH/status");
  CHECK(topic(ctxOf("My St"), Topic::CommonIp) == "My St/common/ip/value");
  // Non-item topics ignore the segment.
  CHECK(topic(s, Topic::Status, "x") == "VdMot/status");
}

TEST_CASE("buildTopic rejects bad segments, topics and buffers") {
  const TopicContext s = ctxOf("VdMot");
  char buf[kTopicMax + 1];
  for (const char* bad : {static_cast<const char*>(nullptr), "", "a/b", "a+", "#", "01234567890"}) {
    CHECK(buildTopic(s, Topic::ValveTarget, bad, buf, sizeof buf) == 0);
    CHECK(buf[0] == '\0');
  }
  CHECK(buildTopic(s, Topic::ValveTarget, "0123456789", buf, sizeof buf) > 0);
  CHECK(buildTopic(s, static_cast<Topic>(kTopicCount), "1", buf, sizeof buf) == 0);
  CHECK(buildTopic(ctxOf("a+b"), Topic::Status, nullptr, buf, sizeof buf) == 0);
  // "VdMot/status" is 12 chars.
  char thirteen[13];
  CHECK(buildTopic(s, Topic::Status, nullptr, thirteen, sizeof thirteen) == 12);
  char twelve[12];
  CHECK(buildTopic(s, Topic::Status, nullptr, twelve, sizeof twelve) == 0);
  CHECK(twelve[0] == '\0');
  // The suffix must fit as well: "VdMot/common/ip/value" is 21 chars.
  char c21[21];
  CHECK(buildTopic(s, Topic::CommonIp, nullptr, c21, sizeof c21) == 0);
  char c22[22];
  CHECK(buildTopic(s, Topic::CommonIp, nullptr, c22, sizeof c22) == 21);
  CHECK(buildTopic(s, Topic::Status, nullptr, nullptr, 10) == 0);
  CHECK(buildTopic(s, Topic::Status, nullptr, buf, 0) == 0);
  // Longest possible topic fits in kTopicMax.
  const TopicContext longest = ctxOf("abcdefghijklmnopqrst", true, true);
  CHECK(topic(longest, Topic::ValveCalibRepetitions, "0123456789") ==
        "/abcdefghijklmnopqrst/valves/0123456789/calibration/repetitions/value");
}

TEST_CASE("retain flags") {
  for (uint8_t i = 0; i < kTopicCount; ++i) {
    const Topic t = static_cast<Topic>(i);
    CAPTURE(i);
    if (t == Topic::Status || t == Topic::DiagCalibrationActive) {
      CHECK(topicRetained(t, false));
      CHECK(topicRetained(t, true));
    } else if (t == Topic::Events || t == Topic::DiagValveProfile) {
      CHECK_FALSE(topicRetained(t, false));
      CHECK_FALSE(topicRetained(t, true));
    } else {
      CHECK_FALSE(topicRetained(t, false));
      CHECK(topicRetained(t, true));
    }
  }
  CHECK_FALSE(topicRetained(static_cast<Topic>(kTopicCount), true));
  CHECK_FALSE(topicIsCompat(static_cast<Topic>(kTopicCount)));
}

TEST_CASE("target command subscription topic") {
  char buf[kTopicMax + 1];
  CHECK(buildTargetCommandTopic(ctxOf("VdMot"), "Bad_1", buf, sizeof buf) == 29);
  CHECK(std::string(buf) == "VdMot/valves/Bad_1/target/set");
  buildTargetCommandTopic(ctxOf("VdMot", false), "Bad_1", buf, sizeof buf);
  CHECK(std::string(buf) == "VdMot/valves/Bad_1/target");
  buildTargetCommandTopic(ctxOf("VdMot", true, true), "2", buf, sizeof buf);
  CHECK(std::string(buf) == "/VdMot/valves/2/target/set");
  CHECK(buildTargetCommandTopic(ctxOf("VdMot"), "", buf, sizeof buf) == 0);
  CHECK(buildTargetCommandTopic(ctxOf("VdMot"), "a/b", buf, sizeof buf) == 0);
  char small[20];
  CHECK(buildTargetCommandTopic(ctxOf("VdMot"), "1", small, sizeof small) == 0);
  CHECK(small[0] == '\0');
  CHECK(buildTargetCommandTopic(ctxOf("VdMot"), "1", nullptr, 20) == 0);
}

TEST_CASE("parseTargetCommandTopic") {
  const char* names[kValveCount] = {"Bad 1", "", "Kitchen", "", "", "", "", "", "", "", "", "12"};
  Segments seg;
  fillSegments(seg, names);
  const TopicContext s = ctxOf("VdMot");
  CHECK(parse(s, "VdMot/valves/Bad_1/target/set", &seg) == 0);
  CHECK(parse(s, "VdMot/valves/Bad_1/target/set/set", &seg) == 0);
  CHECK(parse(s, "/VdMot/valves/Bad_1/target/set", &seg) == 0);
  CHECK(parse(s, "VdMot/valves/2/target/set", &seg) == 1);
  CHECK(parse(s, "VdMot/valves/Kitchen/target/set", &seg) == 2);
  CHECK(parse(s, "VdMot/valves/3/target/set", &seg) == 2);   // number of a named valve
  CHECK(parse(s, "VdMot/valves/1/target/set", &seg) == 0);
  CHECK(parse(s, "VdMot/valves/12/target/set", &seg) == 11);  // valve 12 named "12"
  CHECK(parse(s, "VdMot/valves/11/target/set", &seg) == 10);
  for (const char* bad : {
           "VdMot/valves/13/target/set", "VdMot/valves/0/target/set", "VdMot/valves/03/target/set",
           "VdMot/valves//target/set", "VdMot/valves/-1/target/set", "VdMot/valves/1a/target/set",
           "VdMot/valves/Unknown/target/set", "VdMot/valves/01234567890/target/set",
           "VdMot/valves/1/target", "VdMot/valves/1/target/value", "VdMot/valves/1/target/set/set/set",
           "VdMot/valves/1/target/", "VdMot/valves/1/targetx/set", "VdMot/valves/1/state/set",
           "VdMot/valves/1", "VdMot/valves/1/", "VdMot/valves/", "VdMot/valves", "VdMot/common/state/set",
           "Other/valves/1/target/set", "VdMo/valves/1/target/set", "VdMotX/valves/1/target/set",
           "vdmot/valves/1/target/set", "//VdMot/valves/1/target/set", "VdMot/Valves/1/target/set",
           "VdMot/valves/Bad 1/target/set", "VdMot/valves/4294967297/target/set",
           "VdMot/valves/1/set/target", ""}) {
    CAPTURE(bad);
    CHECK(parse(s, bad, &seg) == -1);
  }
  // Not separate: bare topic and one /set.
  const TopicContext p = ctxOf("VdMot", false);
  CHECK(parse(p, "VdMot/valves/2/target", &seg) == 1);
  CHECK(parse(p, "VdMot/valves/2/target/set", &seg) == 1);
  CHECK(parse(p, "VdMot/valves/2/target/set/set", &seg) == -1);
  // pathAsRoot: the leading '/' is optional on both sides.
  const TopicContext r = ctxOf("VdMot", true, true);
  CHECK(parse(r, "/VdMot/valves/2/target/set", &seg) == 1);
  CHECK(parse(r, "VdMot/valves/2/target/set", &seg) == 1);
  // Fallback main topic.
  CHECK(parse(ctxOf(""), "VdMotFBH/valves/2/target/set", &seg) == 1);
  CHECK(parse(ctxOf("a+b"), "a+b/valves/2/target/set", &seg) == -1);
  CHECK(parse(ctxOf("a+b"), "valves/2/target/set", &seg) == -1);
  CHECK(parse(ctxOf("a+b"), "/valves/2/target/set", &seg) == -1);
  // Without segment table: numbers only.
  CHECK(parse(s, "VdMot/valves/5/target/set", nullptr) == 4);
  CHECK(parse(s, "VdMot/valves/Bad_1/target/set", nullptr) == -1);
  // First name match wins; names beat numbers.
  const char* dup[kValveCount] = {"x", "", "", "", "x", "", "", "", "", "", "", "3"};
  Segments d;
  fillSegments(d, dup);
  CHECK(parse(s, "VdMot/valves/x/target/set", &d) == 0);
  CHECK(parse(s, "VdMot/valves/3/target/set", &d) == 2);  // valve 3's own segment "3" first
  const char* named3[kValveCount] = {"3", "", "Z", "", "", "", "", "", "", "", "", ""};
  Segments n3;
  fillSegments(n3, named3);
  CHECK(parse(s, "VdMot/valves/3/target/set", &n3) == 0);
  // An empty entry in the table (invalid name) never matches.
  Segments holes;
  fillSegments(holes, names);
  holes[4][0] = '\0';
  CHECK(parse(s, "VdMot/valves//target/set", &holes) == -1);
  CHECK(parse(s, "VdMot/valves/5/target/set", &holes) == 4);
  // Unterminated segment entries are read bounded.
  Segments raw;
  memset(raw, 'q', sizeof raw);
  CHECK(parse(s, "VdMot/valves/qqqqqqqqqq/target/set", &raw) == 0);
  // len is authoritative; NUL bytes and oversize input are rejected.
  const char withTail[] = "VdMot/valves/2/target/setGARBAGE";
  CHECK(parseTargetCommandTopic(s, withTail, strlen("VdMot/valves/2/target/set"), seg) == 1);
  const char withNul[] = "VdMot/valves/2\0/target/set";
  CHECK(parseTargetCommandTopic(s, withNul, sizeof withNul - 1, seg) == -1);
  CHECK(parseTargetCommandTopic(s, nullptr, 10, seg) == -1);
  CHECK(parseTargetCommandTopic(s, "VdMot/valves/2/target/set", 0, seg) == -1);
  std::string huge = "VdMot/valves/2/target/set";
  huge += std::string(kTopicMax, '/');
  CHECK(parseTargetCommandTopic(s, huge.c_str(), huge.size(), seg) == -1);
  std::string edge = "/VdMot/valves/2/target/set";
  CHECK(parseTargetCommandTopic(s, edge.c_str(), edge.size(), seg) == 1);
}

TEST_CASE("parseTargetCommandTopic fuzz (fixed seed)") {
  const char* names[kValveCount] = {"a", "b b", "", "", "", "", "", "", "", "", "", ""};
  Segments seg;
  fillSegments(seg, names);
  const TopicContext s = ctxOf("VdMot");
  srand(12345);
  const char alphabet[] = "VdMot/valves/target/set0123456789ab_ \x01\xff+#";
  char buf[160];
  for (int iter = 0; iter < 50000; ++iter) {
    const size_t len = static_cast<size_t>(rand() % 150);
    for (size_t i = 0; i < len; ++i) {
      buf[i] = (rand() % 4 == 0) ? static_cast<char>(rand() % 256)
                                 : alphabet[rand() % (sizeof alphabet - 1)];
    }
    const int r = parseTargetCommandTopic(s, buf, len, seg);
    CHECK((r >= -1 && r < kValveCount));
  }
  // Mutations of a valid topic stay in range and mostly fail.
  const std::string good = "VdMot/valves/b_b/target/set";
  int accepted = 0;
  for (int iter = 0; iter < 20000; ++iter) {
    std::string t = good;
    const size_t pos = static_cast<size_t>(rand()) % t.size();
    t[pos] = static_cast<char>(rand() % 256);
    const int r = parseTargetCommandTopic(s, t.data(), t.size(), seg);
    CHECK((r >= -1 && r < kValveCount));
    if (r >= 0) {
      ++accepted;
      CHECK((r == 1 || t == good));
    }
  }
  CHECK(accepted < 20000);
}

TEST_CASE("parseTargetPayload") {
  struct Ok {
    const char* p;
    uint8_t v;
  };
  const Ok oks[] = {{"55", 55},     {"0", 0},          {"100", 100},   {" 55\r\n", 55},
                    {"\t7 ", 7},    {"55.0", 55},      {"55.00", 55},  {"100.000", 100},
                    {"007", 7},     {"OPEN", 100},     {"CLOSE", 0},   {" OPEN\n", 100},
                    {"0000000000000100", 100},         {"1", 1},       {"99", 99}};
  for (const Ok& o : oks) {
    CAPTURE(o.p);
    uint8_t out = 200;
    CHECK(payload(o.p, out) == TargetPayload::Ok);
    CHECK(out == o.v);
  }
  struct Bad {
    const char* p;
    TargetPayload r;
  };
  const Bad bads[] = {
      {"", TargetPayload::Empty},          {"   ", TargetPayload::Empty},
      {"\r\n", TargetPayload::Empty},      {"101", TargetPayload::OutOfRange},
      {"1000", TargetPayload::OutOfRange}, {"255", TargetPayload::OutOfRange},
      {"256", TargetPayload::OutOfRange},  {"99999999999999", TargetPayload::OutOfRange},
      {"101.0", TargetPayload::OutOfRange},
      {"55.", TargetPayload::NotNumber},   {".5", TargetPayload::NotNumber},
      {"55.5", TargetPayload::NotNumber},  {"55.01", TargetPayload::NotNumber},
      {"-5", TargetPayload::NotNumber},    {"+5", TargetPayload::NotNumber},
      {"-0", TargetPayload::NotNumber},    {"1e2", TargetPayload::NotNumber},
      {"0x10", TargetPayload::NotNumber},  {"nan", TargetPayload::NotNumber},
      {"inf", TargetPayload::NotNumber},   {"5 5", TargetPayload::NotNumber},
      {"55,0", TargetPayload::NotNumber},  {"open", TargetPayload::NotNumber},
      {"Close", TargetPayload::NotNumber}, {"STOP", TargetPayload::NotNumber},
      {"OPENX", TargetPayload::NotNumber}, {"55a", TargetPayload::NotNumber},
      {"00000000000001000", TargetPayload::NotNumber},  // 17 chars
      {"  55            ", TargetPayload::Ok},
  };
  for (const Bad& b : bads) {
    CAPTURE(b.p);
    uint8_t out = 200;
    CHECK(payload(b.p, out) == b.r);
    if (b.r != TargetPayload::Ok) CHECK(out == 200);
  }
  uint8_t out = 200;
  CHECK(parseTargetPayload(nullptr, 3, out) == TargetPayload::Empty);
  CHECK(parseTargetPayload("55", 0, out) == TargetPayload::Empty);
  CHECK(parseTargetPayload("55\0", 3, out) == TargetPayload::NotNumber);
  CHECK(parseTargetPayload("5\0005", 3, out) == TargetPayload::NotNumber);
  CHECK(parseTargetPayload("559", 2, out) == TargetPayload::Ok);  // len bounded
  CHECK(out == 55);
  // Exact-size buffers (no terminator to lean on).
  struct Exact {
    const char* p;
    TargetPayload r;
  };
  for (const Exact& x : {Exact{"   ", TargetPayload::Empty}, Exact{" 5 ", TargetPayload::Ok},
                         Exact{"5", TargetPayload::Ok}, Exact{"OPEN", TargetPayload::Ok}}) {
    std::vector<char> exact(x.p, x.p + strlen(x.p));
    uint8_t v = 200;
    CHECK(parseTargetPayload(exact.data(), exact.size(), v) == x.r);
  }
  std::string seventeen(17, ' ');
  seventeen[8] = '5';
  CHECK(parseTargetPayload(seventeen.data(), seventeen.size(), out) == TargetPayload::NotNumber);

  // Fuzz: never out of range, never a crash.
  srand(777);
  char buf[24];
  for (int iter = 0; iter < 50000; ++iter) {
    const size_t len = static_cast<size_t>(rand() % 20);
    for (size_t i = 0; i < len; ++i) {
      buf[i] = (rand() % 3 == 0) ? static_cast<char>(rand() % 256) : "0123456789. \r\nOPENCLS"[rand() % 22];
    }
    uint8_t v = 200;
    const TargetPayload r = parseTargetPayload(buf, len, v);
    if (r == TargetPayload::Ok) {
      CHECK(v <= 100);
    } else {
      CHECK(v == 200);
    }
  }
}

TEST_CASE("every builder leaves cap 0 untouched and clears the output on failure") {
  const TopicContext s = ctxOf("VdMot");
  char buf[kTopicMax + 1];
  auto prefill = [&] { memset(buf, 'X', sizeof buf); };
  // cap 0: nothing written.
  prefill();
  CHECK(buildMainTopic(s, buf, 0) == 0);
  CHECK(buf[0] == 'X');
  CHECK(buildTopic(s, Topic::Status, nullptr, buf, 0) == 0);
  CHECK(buf[0] == 'X');
  CHECK(buildTargetCommandTopic(s, "1", buf, 0) == 0);
  CHECK(buf[0] == 'X');
  CHECK(buildSegment("a", 0, buf, 0) == 0);
  CHECK(buildSegment("", 0, buf, 0) == 0);
  CHECK(buf[0] == 'X');
  CHECK(formatTemp(1, true, false, buf, 0) == 0);
  CHECK(formatVolt(1, true, false, buf, 0) == 0);
  CHECK(formatUptime(1, buf, 0) == 0);
  CHECK(formatValveState(1, true, buf, 0) == 0);
  CHECK(formatSystemState(1, true, buf, 0) == 0);
  CHECK(formatLegacyCounter(1, buf, 0) == 0);
  LocalTime t;
  CHECK(formatCalibDate(t, buf, 0) == 0);
  CHECK(buf[0] == 'X');

  // cap 1: only the terminator fits.
  char one[1];
  auto one1 = [&] { one[0] = 'X'; };
  one1();
  CHECK(buildMainTopic(s, one, 1) == 0);
  CHECK(one[0] == '\0');
  one1();
  CHECK(buildTopic(s, Topic::Status, nullptr, one, 1) == 0);
  CHECK(one[0] == '\0');
  one1();
  CHECK(buildSegment("a", 0, one, 1) == 0);
  CHECK(one[0] == '\0');
  one1();
  CHECK(buildSegment("", 0, one, 1) == 0);
  CHECK(one[0] == '\0');
  one1();
  CHECK(formatTemp(1, true, false, one, 1) == 0);
  CHECK(one[0] == '\0');
  one1();
  CHECK(formatVolt(1, false, false, one, 1) == 0);
  CHECK(one[0] == '\0');
  one1();
  CHECK(formatValveState(0, true, one, 1) == 0);  // "" fits
  CHECK(one[0] == '\0');

  // Failures after partial output clear it.
  const TopicContext bad = ctxOf("a+b", true, true);
  prefill();
  CHECK(buildMainTopic(bad, buf, sizeof buf) == 0);
  CHECK(buf[0] == '\0');
  prefill();
  CHECK(buildTopic(ctxOf("a+b"), Topic::CommonIp, nullptr, buf, sizeof buf) == 0);
  CHECK(buf[0] == '\0');
  prefill();
  CHECK(buildTopic(s, Topic::ValveTarget, "a/b", buf, sizeof buf) == 0);
  CHECK(buf[0] == '\0');
  prefill();
  CHECK(buildTopic(s, Topic::ValveTarget, nullptr, buf, sizeof buf) == 0);
  CHECK(buf[0] == '\0');
  prefill();
  CHECK(buildTopic(s, static_cast<Topic>(200), "1", buf, sizeof buf) == 0);
  CHECK(buf[0] == '\0');
  prefill();
  CHECK(buildTargetCommandTopic(s, "", buf, sizeof buf) == 0);
  CHECK(buf[0] == '\0');
  prefill();
  CHECK(buildTargetCommandTopic(ctxOf("a#"), "1", buf, sizeof buf) == 0);
  CHECK(buf[0] == '\0');
  prefill();
  CHECK(buildSegment("a/b", 0, buf, sizeof buf) == 0);
  CHECK(buf[0] == '\0');
  char three[3] = {'X', 'X', 'X'};
  CHECK(buildSegment("abc", 0, three, sizeof three) == 0);
  CHECK(three[0] == '\0');
  memset(three, 'X', sizeof three);
  CHECK(buildSegment("", 199, three, sizeof three) == 0);
  CHECK(three[0] == '\0');
  memset(three, 'X', sizeof three);
  CHECK(formatTemp(215, true, false, three, sizeof three) == 0);
  CHECK(three[0] == '\0');
  memset(three, 'X', sizeof three);
  CHECK(formatCalibDate(t, three, sizeof three) == 0);
  CHECK(three[0] == '\0');
  memset(three, 'X', sizeof three);
  CHECK(formatLegacyCounter(4294967295u, three, sizeof three) == 2);
  CHECK(std::string(three) == "-1");
  // Exact fit.
  char five[5];
  CHECK(formatTemp(215, true, false, five, sizeof five) == 4);
  CHECK(std::string(five) == "21.5");
  char seg11[kSegmentMax + 1];
  CHECK(buildSegment("0123456789", 0, seg11, sizeof seg11) == 10);
  CHECK(buildSegment("", 0, seg11, 2) == 1);
}

TEST_CASE("temperature and volt payloads") {
  CHECK(fmtTemp(215) == "21.5");
  CHECK(fmtTemp(215, true, true) == "21,5");
  CHECK(fmtTemp(-5) == "-0.5");
  CHECK(fmtTemp(-5, true, true) == "-0,5");
  CHECK(fmtTemp(-15) == "-1.5");
  CHECK(fmtTemp(0) == "0.0");
  CHECK(fmtTemp(9) == "0.9");
  CHECK(fmtTemp(10) == "1.0");
  CHECK(fmtTemp(1250) == "125.0");
  CHECK(fmtTemp(-550) == "-55.0");
  CHECK(fmtTemp(INT32_MIN) == "-214748364.8");
  CHECK(fmtTemp(INT32_MAX) == "214748364.7");
  CHECK(fmtTemp(215, false) == "failed");
  CHECK(fmtTemp(215, false, true) == "failed");
  char small[4];
  CHECK(formatTemp(215, true, false, small, sizeof small) == 0);
  CHECK(small[0] == '\0');
  char five[5];
  CHECK(formatTemp(215, true, false, five, sizeof five) == 4);
  CHECK(formatTemp(215, false, false, small, sizeof small) == 0);
  CHECK(formatTemp(215, true, false, nullptr, 8) == 0);

  CHECK(fmtVolt(12.345) == "12.345");
  CHECK(fmtVolt(12.345, true, true) == "12,345");
  CHECK(fmtVolt(0.0) == "0.000");
  CHECK(fmtVolt(-1.5) == "-1.500");
  CHECK(fmtVolt(-1.5, true, true) == "-1,500");
  CHECK(fmtVolt(1234.5678) == "1234.568");
  CHECK(fmtVolt(12.0, false) == "failed");
  CHECK(fmtVolt(NAN) == "failed");
  CHECK(fmtVolt(INFINITY) == "failed");
  CHECK(fmtVolt(-INFINITY, true, true) == "failed");
  CHECK(fmtVolt(999999999999999.0) == "999999999999999.000");
  CHECK(fmtVolt(-1e15, true, true) == "-1000000000000000,000");
  char buf[20];
  memset(buf, 'X', sizeof buf);
  CHECK(formatVolt(-1e15, true, false, buf, sizeof buf) == 0);
  CHECK(buf[0] == '\0');
  memset(buf, 'X', sizeof buf);
  CHECK(formatVolt(1e300, true, true, buf, sizeof buf) == 0);
  CHECK(buf[0] == '\0');
  char six[6];
  CHECK(formatVolt(1.0, true, false, six, sizeof six) == 5);
  CHECK(formatVolt(10.0, true, false, six, sizeof six) == 0);
}

TEST_CASE("uptime, states, counters") {
  char buf[32];
  CHECK(formatUptime(0, buf, sizeof buf) == 10);
  CHECK(std::string(buf) == "0d 0:00:00");
  formatUptime(273909, buf, sizeof buf);
  CHECK(std::string(buf) == "3d 4:05:09");
  formatUptime(86399, buf, sizeof buf);
  CHECK(std::string(buf) == "0d 23:59:59");
  formatUptime(86400, buf, sizeof buf);
  CHECK(std::string(buf) == "1d 0:00:00");
  formatUptime(UINT32_MAX, buf, sizeof buf);
  CHECK(std::string(buf) == "49710d 6:28:15");
  char tiny[10];
  CHECK(formatUptime(0, tiny, sizeof tiny) == 0);

  const char* texts[] = {"",        "idle",     "opens",     "closes",    "failed",
                         "unknown", "no valve", "full open", "connected", "blocked"};
  for (uint8_t st = 0; st < 10; ++st) {
    CHECK(formatValveState(st, true, buf, sizeof buf) == strlen(texts[st]));
    CHECK(std::string(buf) == texts[st]);
    formatValveState(st, false, buf, sizeof buf);
    CHECK(std::string(buf) == std::to_string(st));
  }
  CHECK(formatValveState(10, true, buf, sizeof buf) == 0);
  CHECK(std::string(buf) == "");
  CHECK(formatValveState(255, false, buf, sizeof buf) == 3);
  CHECK(std::string(buf) == "255");
  char two[2];
  CHECK(formatValveState(12, false, two, sizeof two) == 0);
  CHECK(formatValveState(1, true, two, sizeof two) == 0);

  const char* sys[] = {"ok", "info", "error"};
  for (uint8_t st = 0; st < 3; ++st) {
    formatSystemState(st, true, buf, sizeof buf);
    CHECK(std::string(buf) == sys[st]);
    formatSystemState(st, false, buf, sizeof buf);
    CHECK(std::string(buf) == std::to_string(st));
  }
  CHECK(formatSystemState(3, true, buf, sizeof buf) == 0);
  CHECK(std::string(buf) == "");
  CHECK(formatSystemState(3, false, buf, sizeof buf) == 1);
  CHECK(std::string(buf) == "3");
  CHECK(formatSystemState(255, true, buf, sizeof buf) == 0);

  CHECK(formatLegacyCounter(0, buf, sizeof buf) == 1);
  CHECK(std::string(buf) == "0");
  formatLegacyCounter(3120, buf, sizeof buf);
  CHECK(std::string(buf) == "3120");
  formatLegacyCounter(2147483647u, buf, sizeof buf);
  CHECK(std::string(buf) == "2147483647");
  formatLegacyCounter(2147483648u, buf, sizeof buf);
  CHECK(std::string(buf) == "-2147483648");
  formatLegacyCounter(4294967295u, buf, sizeof buf);
  CHECK(std::string(buf) == "-1");
  CHECK(formatLegacyCounter(100, two, sizeof two) == 0);
  CHECK(formatLegacyCounter(100, nullptr, 4) == 0);
}

TEST_CASE("calibration date in the legacy strftime format") {
  LocalTime t;
  t.valid = true;
  t.year = 2026;
  t.month = 9;
  t.mday = 21;
  t.wday = 1;
  t.hour = 14;
  t.minute = 3;
  t.second = 5;
  char buf[48];
  CHECK(formatCalibDate(t, buf, sizeof buf) == 34);
  CHECK(std::string(buf) == "Monday, September 21.2026 14:03:05");
  t.mday = 5;
  t.hour = 0;
  t.minute = 0;
  t.second = 0;
  formatCalibDate(t, buf, sizeof buf);
  CHECK(std::string(buf) == "Monday, September 05.2026 00:00:00");
  const char* days[] = {"Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"};
  const char* months[] = {"January", "February", "March",     "April",   "May",      "June",
                          "July",    "August",   "September", "October", "November", "December"};
  for (uint8_t d = 0; d < 7; ++d) {
    t.wday = d;
    formatCalibDate(t, buf, sizeof buf);
    CHECK(std::string(buf).rfind(std::string(days[d]) + ", ", 0) == 0);
  }
  t.wday = 3;
  for (uint8_t m = 1; m <= 12; ++m) {
    t.month = m;
    formatCalibDate(t, buf, sizeof buf);
    CHECK(std::string(buf) == std::string("Wednesday, ") + months[m - 1] + " 05.2026 00:00:00");
  }
  t.month = 1;
  t.mday = 1;
  formatCalibDate(t, buf, sizeof buf);
  CHECK(std::string(buf) == "Wednesday, January 01.2026 00:00:00");
  t.month = 12;
  t.mday = 31;
  t.hour = 23;
  t.minute = 59;
  t.second = 60;
  formatCalibDate(t, buf, sizeof buf);
  CHECK(std::string(buf) == "Wednesday, December 31.2026 23:59:60");
  const std::string failed = "Failed to obtain time";
  LocalTime bad = t;
  bad.valid = false;
  CHECK(formatCalibDate(bad, buf, sizeof buf) == failed.size());
  CHECK(std::string(buf) == failed);
  struct Mut {
    void (*f)(LocalTime&);
  };
  const Mut muts[] = {
      {[](LocalTime& x) { x.wday = 7; }},   {[](LocalTime& x) { x.month = 0; }},
      {[](LocalTime& x) { x.month = 13; }}, {[](LocalTime& x) { x.mday = 0; }},
      {[](LocalTime& x) { x.mday = 32; }},  {[](LocalTime& x) { x.hour = 24; }},
      {[](LocalTime& x) { x.minute = 60; }}, {[](LocalTime& x) { x.second = 61; }},
  };
  for (const Mut& m : muts) {
    LocalTime x = t;
    m.f(x);
    formatCalibDate(x, buf, sizeof buf);
    CHECK(std::string(buf) == failed);
  }
  char small[10];
  CHECK(formatCalibDate(t, small, sizeof small) == 0);
  CHECK(small[0] == '\0');
  CHECK(formatCalibDate(bad, small, sizeof small) == 0);
}

TEST_CASE("publish scheduler: full publishes") {
  PublishScheduler s;
  PublishScheduler::Params p;
  p.onChange = false;
  p.publishIntervalMs = 10000;
  p.minDelayMs = 5000;
  s.configure(p);
  CHECK(s.takeFullPublish(123));   // first after start
  CHECK_FALSE(s.takeFullPublish(123));
  CHECK_FALSE(s.takeFullPublish(10122));
  CHECK(s.takeFullPublish(10123));
  CHECK_FALSE(s.takeFullPublish(10124));
  s.onConnected(15000);
  CHECK(s.takeFullPublish(15000));
  CHECK_FALSE(s.takeFullPublish(24999));
  CHECK(s.takeFullPublish(25000));
  // Periodic mode: items only with the full publish.
  s.markAllPublished(25000);
  CHECK_FALSE(s.takeItem(1, true, 99999));
  // Wrap-safe.
  PublishScheduler w;
  w.configure(p);
  CHECK(w.takeFullPublish(0xFFFFF000u));
  CHECK_FALSE(w.takeFullPublish(0xFFFFF000u + 9999u));
  CHECK(w.takeFullPublish(0xFFFFF000u + 10000u));
}

TEST_CASE("publish scheduler: on-change items") {
  PublishScheduler s;
  PublishScheduler::Params p;
  p.onChange = true;
  p.publishIntervalMs = 10000;
  p.minDelayMs = 5000;
  s.configure(p);
  // Not yet published since connect: goes out at once.
  CHECK(s.takeItem(3, false, 100));
  CHECK_FALSE(s.takeItem(3, true, 100));
  s.markAllPublished(1000);
  CHECK_FALSE(s.takeItem(1, true, 5999));
  CHECK(s.takeItem(1, true, 6000));
  CHECK_FALSE(s.takeItem(1, true, 6001));
  CHECK_FALSE(s.takeItem(2, false, 10999));  // heartbeat
  CHECK(s.takeItem(2, false, 11000));
  CHECK_FALSE(s.takeItem(2, false, 11001));
  CHECK(s.takeItem(63, false, 11000));
  CHECK_FALSE(s.takeItem(64, true, 99999));
  CHECK_FALSE(s.takeItem(255, true, 99999));
  // Reconnect forgets the per-item history.
  s.onConnected(12000);
  CHECK(s.takeItem(1, false, 12000));
  // minDelay 0: every change goes out.
  PublishScheduler z;
  p.minDelayMs = 0;
  z.configure(p);
  z.markAllPublished(0);
  CHECK(z.takeItem(0, true, 0));
  CHECK(z.takeItem(0, true, 0));
  CHECK_FALSE(z.takeItem(0, false, 0));
}

TEST_CASE("publish scheduler: parameter clamping") {
  PublishScheduler s;
  PublishScheduler::Params p;
  p.onChange = true;
  p.publishIntervalMs = 500;   // below the 2 s minimum
  p.minDelayMs = 9000;         // above the interval
  s.configure(p);
  CHECK(s.takeFullPublish(0));
  CHECK_FALSE(s.takeFullPublish(1999));
  CHECK(s.takeFullPublish(2000));
  s.markAllPublished(0);
  CHECK_FALSE(s.takeItem(0, true, 1999));
  CHECK(s.takeItem(0, true, 2000));  // minDelay clamped to the interval
  p.publishIntervalMs = 2000;
  p.minDelayMs = 2000;
  PublishScheduler e;
  e.configure(p);
  e.markAllPublished(0);
  CHECK_FALSE(e.takeItem(0, true, 1999));
  CHECK(e.takeItem(0, true, 2000));
  // Exactly the minimum is kept; one below is raised to it.
  PublishScheduler m2;
  p.publishIntervalMs = 2000;
  p.minDelayMs = 0;
  m2.configure(p);
  CHECK(m2.takeFullPublish(0));
  CHECK_FALSE(m2.takeFullPublish(1999));
  CHECK(m2.takeFullPublish(2000));
  PublishScheduler m1;
  p.publishIntervalMs = 1999;
  m1.configure(p);
  CHECK(m1.takeFullPublish(0));
  CHECK_FALSE(m1.takeFullPublish(1999));
  CHECK(m1.takeFullPublish(2000));
  PublishScheduler d;  // default parameters before configure()
  CHECK(d.takeFullPublish(0));
  CHECK_FALSE(d.takeFullPublish(9999));
  CHECK(d.takeFullPublish(10000));
}
