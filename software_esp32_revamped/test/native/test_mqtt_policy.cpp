// MQTT task decisions: HA status, reconnect pacing, client id, inbound
// commands, reject throttle, button confirmation, target latch.
#include <stdlib.h>
#include <string.h>

#include <string>
#include <vector>

#include "doctest.h"
#include "vdm/mqtt_policy.h"

using namespace vdm;

namespace {

using Change = RegulatorWatch::Change;

Change ha(RegulatorWatch& w, const char* p) { return w.onHaStatus(p, strlen(p)); }

std::string clientId(const char* station, uint8_t a = 0xa1, uint8_t b = 0xb2, uint8_t c = 0xc3) {
  const uint8_t mac[6] = {0x24, 0x0a, 0xc4, a, b, c};
  char out[32];
  memset(out, 'X', sizeof out);
  const size_t n = buildMqttClientId(station, mac, out, sizeof out);
  CHECK(n == strlen(out));
  return out;
}

using Segments = char[kValveCount][kSegmentMax + 1];

struct Inbound {
  TopicContext topics;
  Segments seg = {};
  EchoFilter echo;
  InboundContext ctx;
  Inbound(bool separate = true, MqttMode mode = MqttMode::MqttHa) {
    copyString(topics.station, sizeof topics.station, "VdMot");
    topics.separate = separate;
    copyString(seg[0], sizeof seg[0], "Bad_1");
    for (uint8_t i = 1; i < kValveCount; ++i) snprintf(seg[i], sizeof seg[i], "%u", i + 1u);
    ctx.topics = &topics;
    ctx.haPrefix = "ha";
    ctx.segments = seg;
    ctx.activeMask = 0x00FF;  // valves 1..8 active
    ctx.mode = mode;
    ctx.stmV3 = true;
    ctx.echo = &echo;
  }
  InboundDecision decide(const char* topic, const char* payload) const {
    return decideInbound(ctx, topic, strlen(topic), reinterpret_cast<const uint8_t*>(payload),
                         strlen(payload));
  }
  bool clearEcho(const char* topic, const char* payload) const {
    return inboundIsClearEcho(ctx, topic, strlen(topic),
                              reinterpret_cast<const uint8_t*>(payload), strlen(payload));
  }
};

// action/valve/pos/reason/detail/clear as one comparable string.
std::string str(const InboundDecision& d) {
  static const char* const kActions[] = {"Ignore",       "Reject",   "SetTarget", "StopValve",
                                         "CalibrateValve", "CalibrateAll", "Restart", "StmReset",
                                         "Detect",       "StopAll",  "StmSafeExit", "HaOnline",
                                         "HaOffline"};
  std::string s = kActions[static_cast<int>(d.action)];
  s += " v" + std::to_string(d.valve) + " p" + std::to_string(d.pos);
  if (d.reason != RejectReason::None) s += std::string(" '") + rejectReasonName(d.reason) + "'";
  if (d.detail != 0) s += " d" + std::to_string(d.detail);
  if (d.clearRetained) s += " clear";
  return s;
}

InboundDecision decision(InboundAction a, uint8_t valve = kNoValve) {
  InboundDecision d;
  d.action = a;
  d.valve = valve;
  return d;
}

}  // namespace

TEST_CASE("RegulatorWatch: HA status transitions") {
  RegulatorWatch w;
  CHECK(w.haStatus() == HaStatus::Unknown);
  CHECK(ha(w, "online") == Change::None);  // Unknown -> Online is silent
  CHECK(w.haStatus() == HaStatus::Online);
  CHECK(ha(w, "online") == Change::None);
  CHECK(ha(w, "offline") == Change::WentOffline);
  CHECK(w.haStatus() == HaStatus::Offline);
  CHECK(ha(w, "offline") == Change::None);
  CHECK(w.haStatus() == HaStatus::Offline);
  for (const char* ignored : {"ONLINE", "offline ", " online", "onlin", "", "offlinex"}) {
    CAPTURE(ignored);
    CHECK(ha(w, ignored) == Change::None);
    CHECK(w.haStatus() == HaStatus::Offline);
  }
  CHECK(w.onHaStatus(nullptr, 6) == Change::None);
  CHECK(w.onHaStatus("onlinex", 6) == Change::CameOnline);  // len is authoritative
  CHECK(w.haStatus() == HaStatus::Online);
  RegulatorWatch u;
  CHECK(ha(u, "offline") == Change::WentOffline);  // Unknown -> Offline
}

TEST_CASE("RegulatorWatch: a command brings an offline HA back, restore and snapshot") {
  RegulatorWatch w;
  CHECK(w.onInboundCommand() == Change::None);
  CHECK(w.haStatus() == HaStatus::Unknown);
  ha(w, "online");
  CHECK(w.onInboundCommand() == Change::None);
  ha(w, "offline");
  CHECK(w.onInboundCommand() == Change::CameOnlineByCommand);
  CHECK(w.haStatus() == HaStatus::Online);
  CHECK(w.snapshot() == HaStatus::Online);
  w.restore(HaStatus::Offline);
  CHECK(w.haStatus() == HaStatus::Offline);
  CHECK(w.snapshot() == HaStatus::Offline);
  w.restore(HaStatus::Unknown);
  CHECK(w.haStatus() == HaStatus::Unknown);
  w.restore(HaStatus::Online);
  CHECK(w.haStatus() == HaStatus::Online);
  w.restore(static_cast<HaStatus>(3));
  CHECK(w.haStatus() == HaStatus::Unknown);
}

TEST_CASE("HA status RTC record") {
  HaStatusRecord r;
  memset(&r, 0xA5, sizeof r);  // power-on garbage
  CHECK(decodeHaStatusRecord(r) == HaStatus::Unknown);
  for (HaStatus s : {HaStatus::Unknown, HaStatus::Online, HaStatus::Offline}) {
    encodeHaStatusRecord(s, r);
    CHECK(r.magic == 0x41484456u);
    CHECK(r.status == static_cast<uint8_t>(s));
    CHECK(r.pad == 0);
    CHECK(decodeHaStatusRecord(r) == s);
  }
  encodeHaStatusRecord(HaStatus::Offline, r);
  HaStatusRecord bad = r;
  bad.magic ^= 1;
  CHECK(decodeHaStatusRecord(bad) == HaStatus::Unknown);
  bad = r;
  bad.crc ^= 0x8000;
  CHECK(decodeHaStatusRecord(bad) == HaStatus::Unknown);
  bad = r;
  bad.pad = 1;  // covered by the CRC
  CHECK(decodeHaStatusRecord(bad) == HaStatus::Unknown);
  bad = r;
  bad.status = 1;  // a valid status with a stale CRC
  CHECK(decodeHaStatusRecord(bad) == HaStatus::Unknown);
  // A status out of range with a matching CRC.
  HaStatusRecord three;
  encodeHaStatusRecord(static_cast<HaStatus>(3), three);
  CHECK(three.status == 3);
  CHECK(decodeHaStatusRecord(three) == HaStatus::Unknown);
  // The CRC is the low half of crc32 over the first six bytes.
  encodeHaStatusRecord(HaStatus::Online, r);
  const uint8_t b[6] = {0x56, 0x44, 0x48, 0x41, 1, 0};
  CHECK(r.crc == static_cast<uint16_t>(crc32(b, sizeof b) & 0xFFFFu));
}

TEST_CASE("ReconnectPacer: back-off reset only after a stable connection") {
  ReconnectPacer p(2000, 60000);
  CHECK(p.due(0));
  p.onConnected(0);
  p.onDropped(100);  // dropped at once: a failed attempt
  CHECK_FALSE(p.due(100));
  CHECK_FALSE(p.due(2099));
  CHECK(p.due(2100));
  uint32_t t = 2100;
  for (uint32_t wait : {4000u, 8000u, 16000u, 32000u, 60000u, 60000u}) {
    CAPTURE(wait);
    p.onConnected(t);
    p.tick(t + 50, true);
    p.onDropped(t + 100);
    CHECK_FALSE(p.due(t + 100 + wait - 1));
    CHECK(p.due(t + 100 + wait));
    t += 100 + wait;
  }
  // Up for 59 999 ms: still backing off.
  p.onConnected(t);
  p.tick(t + ReconnectPacer::kStableMs - 1, true);
  p.onDropped(t + ReconnectPacer::kStableMs - 1);
  CHECK_FALSE(p.due(t + ReconnectPacer::kStableMs));
  CHECK(p.delayMs() == 60000);
  // Up for 60 000 ms (tick): the drop is due at once, delay back to min.
  t += 200000;
  p.onConnected(t);
  p.tick(t + ReconnectPacer::kStableMs, true);
  CHECK(p.delayMs() == 2000);
  p.onDropped(t + 70000);
  CHECK(p.due(t + 70000));
  // tick while not connected (or before any connect) changes nothing.
  ReconnectPacer q(2000, 60000);
  q.onAttemptFailed(0);
  q.tick(100000, false);
  q.tick(100000, true);  // never connected
  CHECK(q.delayMs() == 4000);
  q.onDropped(5);  // not connected: nothing
  CHECK(q.due(2000));
  CHECK_FALSE(q.due(1999));
  // A second tick after stability does not reset a later back-off twice.
  ReconnectPacer r(2000, 60000);
  r.onConnected(0);
  r.tick(60000, true);
  r.onAttemptFailed(60001);
  r.tick(70000, true);
  CHECK(r.delayMs() == 4000);
  // tick with connected false while the pacer thinks it is connected.
  ReconnectPacer s(2000, 60000);
  s.onConnected(0);
  s.tick(60000, false);
  s.onDropped(60001);
  CHECK_FALSE(s.due(60001));
  // forceNow: due at once.
  s.forceNow();
  CHECK(s.due(60002));
  CHECK(s.delayMs() == 2000);
  // Failed attempts double as well.
  ReconnectPacer f(2000, 60000);
  f.onAttemptFailed(0);
  CHECK_FALSE(f.due(1999));
  CHECK(f.due(2000));
  f.onAttemptFailed(2000);
  CHECK_FALSE(f.due(5999));
  CHECK(f.due(6000));
  // Across a millis() wrap.
  ReconnectPacer w(2000, 60000);
  w.onConnected(0xFFFFFF00u);
  w.tick(0xFFFFFF00u + ReconnectPacer::kStableMs, true);
  CHECK(w.delayMs() == 2000);
}

TEST_CASE("MQTT client id") {
  CHECK(clientId("VdMot") == "VdMot-a1b2c3");
  CHECK(clientId("Dom 1 \xc5\x81" "azienka EG") == "Dom-1-azienka-EG-a1b2c3");
  CHECK(clientId("Wohnung-Ost-EG-Bad") == "Wohnung-Ost-EG-B-a1b2c3");
  CHECK(clientId("abcdefghijklmno-x") == "abcdefghijklmno-a1b2c3");
  CHECK(clientId("abcdefghijklmn--x") == "abcdefghijklmn-a1b2c3");
  CHECK(clientId("") == "VdMot-a1b2c3");
  CHECK(clientId("VdMot", 0x00, 0x0f, 0xf0) == "VdMot-000ff0");
  CHECK(clientId("abcdefghijklmnopqrst") == "abcdefghijklmnop-a1b2c3");
  const uint8_t mac[6] = {1, 2, 3, 4, 5, 6};
  char out[24];
  CHECK(buildMqttClientId("VdMot", mac, out, sizeof out) == 12);
  CHECK(std::string(out) == "VdMot-040506");
  char small[23];
  memset(small, 'X', sizeof small);
  CHECK(buildMqttClientId("VdMot", mac, small, sizeof small) == 0);
  CHECK(small[0] == '\0');
  CHECK(buildMqttClientId("VdMot", mac, nullptr, 30) == 0);
  CHECK(buildMqttClientId("VdMot", mac, small, 0) == 0);
  // Length <= 23 for every station of 1..20 bytes (fixed seed).
  srand(4242);
  for (int iter = 0; iter < 5000; ++iter) {
    char st[21];
    const size_t len = 1 + static_cast<size_t>(rand() % 20);
    for (size_t i = 0; i < len; ++i) st[i] = static_cast<char>(1 + rand() % 255);
    st[len] = '\0';
    char id[64];
    const size_t n = buildMqttClientId(st, mac, id, sizeof id);
    CHECK(n <= 23);
    CHECK(n >= 8);
    CHECK(n == strlen(id));
    CHECK(id[n - 7] == '-');
  }
}

TEST_CASE("EchoFilter") {
  EchoFilter e;
  CHECK_FALSE(e.isEcho(2, 0));
  e.published(2, 40);
  CHECK(e.isEcho(2, 40));
  CHECK_FALSE(e.isEcho(2, 41));
  CHECK_FALSE(e.isEcho(3, 40));
  e.published(2, 41);
  CHECK_FALSE(e.isEcho(2, 40));
  CHECK(e.isEcho(2, 41));
  e.published(11, 0);
  CHECK(e.isEcho(11, 0));
  e.published(12, 5);  // out of range: ignored
  CHECK_FALSE(e.isEcho(12, 5));
  e.reset();
  CHECK_FALSE(e.isEcho(2, 41));
  CHECK_FALSE(e.isEcho(11, 0));
}

TEST_CASE("reject reason names") {
  CHECK(std::string(rejectReasonName(RejectReason::None)) == "");
  CHECK(std::string(rejectReasonName(RejectReason::Payload)) == "payload");
  CHECK(std::string(rejectReasonName(RejectReason::UnknownValve)) == "unknown valve");
  CHECK(std::string(rejectReasonName(RejectReason::Inactive)) == "inactive");
  CHECK(std::string(rejectReasonName(RejectReason::Unsupported)) == "unsupported");
  CHECK(std::string(rejectReasonName(RejectReason::UnknownCommand)) == "unknown command");
  CHECK(std::string(rejectReasonName(RejectReason::QueueFull)) == "queue full");
  CHECK(std::string(rejectReasonName(RejectReason::ClearNotConfirmed)) == "clear not confirmed");
  CHECK(std::string(rejectReasonName(static_cast<RejectReason>(99))) == "");
}

TEST_CASE("inbound: button actions") {
  for (int a = 0; a <= static_cast<int>(InboundAction::HaOffline); ++a) {
    const InboundAction x = static_cast<InboundAction>(a);
    const bool button = x == InboundAction::CalibrateValve || x == InboundAction::CalibrateAll ||
                        x == InboundAction::Restart || x == InboundAction::StmReset ||
                        x == InboundAction::Detect || x == InboundAction::StopAll ||
                        x == InboundAction::StmSafeExit;
    CAPTURE(a);
    CHECK(inboundIsButton(x) == button);
  }
}

TEST_CASE("decideInbound: targets") {
  Inbound in;
  CHECK(str(in.decide("VdMot/valves/1/target/set", "40")) == "SetTarget v0 p40 clear");
  CHECK(str(in.decide("VdMot/valves/Bad_1/target/set/set", "43,7")) == "SetTarget v0 p44 clear");
  CHECK(str(in.decide("VdMot/valves/8/target/set", "OPEN")) == "SetTarget v7 p100 clear");
  CHECK(str(in.decide("VdMot/valves/9/target/set", "50")) == "Reject v8 p0 'inactive' clear");
  CHECK(str(in.decide("VdMot/valves/Old/target/set", "50")) ==
        "Reject v254 p0 'unknown valve' clear");
  CHECK(str(in.decide("VdMot/valves/1/target/set", "abc")) == "Reject v0 p0 'payload' d2 clear");
  CHECK(str(in.decide("VdMot/valves/1/target/set", "101")) == "Reject v0 p0 'payload' d3 clear");
  CHECK(str(in.decide("VdMot/valves/1/target/set", "STOP")) == "StopValve v0 p0 clear");
  in.ctx.stmV3 = false;
  CHECK(str(in.decide("VdMot/valves/1/target/set", "STOP")) ==
        "Reject v0 p0 'unsupported' clear");
  // Empty payloads (a retained clear coming back) are ignored everywhere.
  for (const char* t : {"VdMot/valves/1/target/set", "VdMot/valves/9/target/set",
                        "VdMot/valves/Old/target/set", "VdMot/cmd/restart", "VdMot/cmd/foo",
                        "VdMot/cmd/valves/1/calibrate"}) {
    CAPTURE(t);
    CHECK(str(in.decide(t, "")) == "Ignore v254 p0");
    CHECK(str(in.decide(t, " \r\n\t")) == "Ignore v254 p0");
    CHECK(in.clearEcho(t, ""));
    CHECK(in.clearEcho(t, " \n"));
    CHECK_FALSE(in.clearEcho(t, "x"));
  }
  CHECK(decideInbound(in.ctx, "VdMot/cmd/restart", 17, nullptr, 5).action == InboundAction::Ignore);
  CHECK(inboundIsClearEcho(in.ctx, "VdMot/cmd/restart", 17, nullptr, 5));
  // Not our topics.
  CHECK(str(in.decide("VdMot/common/state", "1")) == "Ignore v254 p0");
  CHECK(str(in.decide("Other/valves/1/target/set", "1")) == "Ignore v254 p0");
  CHECK_FALSE(in.clearEcho("Other/valves/1/target/set", ""));
  CHECK_FALSE(in.clearEcho("homeassistant/status", ""));
  InboundContext none;
  CHECK(decideInbound(none, "VdMot/cmd/restart", 17, reinterpret_cast<const uint8_t*>("PRESS"), 5)
            .action == InboundAction::Ignore);
  CHECK_FALSE(inboundIsClearEcho(none, "VdMot/cmd/restart", 17, nullptr, 0));
}

TEST_CASE("decideInbound: the state form without separate") {
  Inbound in(false);
  CHECK(str(in.decide("VdMot/valves/2/target", "40")) == "SetTarget v1 p40");
  CHECK(str(in.decide("VdMot/valves/2/target/set", "40")) == "SetTarget v1 p40 clear");
  in.echo.published(1, 40);
  CHECK(str(in.decide("VdMot/valves/2/target", "40")) == "Ignore v1 p40");   // own echo
  CHECK(str(in.decide("VdMot/valves/2/target", " 40.0")) == "Ignore v1 p40");
  CHECK(str(in.decide("VdMot/valves/2/target", "41")) == "SetTarget v1 p41");
  CHECK(str(in.decide("VdMot/valves/2/target/set", "40")) == "SetTarget v1 p40 clear");
  CHECK(str(in.decide("VdMot/valves/3/target", "40")) == "SetTarget v2 p40");
  CHECK(str(in.decide("VdMot/valves/9/target", "40")) == "Reject v8 p0 'inactive'");
  CHECK(str(in.decide("VdMot/valves/2/target", "x")) == "Reject v1 p0 'payload' d2");
  in.ctx.echo = nullptr;
  CHECK(str(in.decide("VdMot/valves/2/target", "40")) == "SetTarget v1 p40");
}

TEST_CASE("decideInbound: cmd topics") {
  Inbound in;
  CHECK(str(in.decide("VdMot/cmd/valves/Bad_1/calibrate", "PRESS")) == "CalibrateValve v0 p0 clear");
  CHECK(str(in.decide("VdMot/cmd/valves/2/calibrate", "PRESS")) == "CalibrateValve v1 p0 clear");
  CHECK(str(in.decide("VdMot/cmd/valves/9/calibrate", "PRESS")) == "Reject v8 p0 'inactive' clear");
  CHECK(str(in.decide("VdMot/cmd/valves/Old/calibrate", "PRESS")) ==
        "Reject v254 p0 'unknown valve' clear");
  CHECK(str(in.decide("VdMot/cmd/valves/1/calibrate", "press")) == "Reject v0 p0 'payload' clear");
  CHECK(str(in.decide("VdMot/cmd/calibrate", "PRESS")) == "CalibrateAll v254 p0 clear");
  CHECK(str(in.decide("VdMot/cmd/restart", "PRESS")) == "Restart v254 p0 clear");
  CHECK(str(in.decide("VdMot/cmd/stmReset", "PRESS")) == "StmReset v254 p0 clear");
  CHECK(str(in.decide("VdMot/cmd/detect", "PRESS")) == "Detect v254 p0 clear");
  CHECK(str(in.decide("VdMot/cmd/stop", "PRESS")) == "StopAll v254 p0 clear");
  CHECK(str(in.decide("VdMot/cmd/stmSafeExit", "PRESS")) == "StmSafeExit v254 p0 clear");
  CHECK(str(in.decide("VdMot/cmd/restart", "press")) == "Reject v254 p0 'payload' clear");
  CHECK(str(in.decide("VdMot/cmd/foo", "PRESS")) == "Reject v254 p0 'unknown command' clear");
  CHECK(str(in.decide("VdMot/cmd/foo", "x")) == "Reject v254 p0 'unknown command' clear");
  in.ctx.stmV3 = false;
  CHECK(str(in.decide("VdMot/cmd/stop", "PRESS")) == "Reject v254 p0 'unsupported' clear");
  CHECK(str(in.decide("VdMot/cmd/stmSafeExit", "PRESS")) == "Reject v254 p0 'unsupported' clear");
  CHECK(str(in.decide("VdMot/cmd/stop", "x")) == "Reject v254 p0 'payload' clear");
  CHECK(str(in.decide("VdMot/cmd/restart", "PRESS")) == "Restart v254 p0 clear");
  // The mode does not matter for commands (Off has no subscriptions).
  in.ctx.mode = MqttMode::Mqtt;
  CHECK(str(in.decide("VdMot/cmd/detect", "PRESS")) == "Detect v254 p0 clear");
}

TEST_CASE("decideInbound: HA status") {
  Inbound in;
  CHECK(str(in.decide("homeassistant/status", "online")) == "HaOnline v254 p0");
  CHECK(str(in.decide("homeassistant/status", "offline")) == "HaOffline v254 p0");
  CHECK(str(in.decide("ha/status", "offline")) == "HaOffline v254 p0");
  CHECK(str(in.decide("homeassistant/status", "ONLINE")) == "Ignore v254 p0");
  CHECK(str(in.decide("homeassistant/status", "")) == "Ignore v254 p0");
  CHECK(str(in.decide("homeassistant/status", "offline ")) == "Ignore v254 p0");
  CHECK(decideInbound(in.ctx, "homeassistant/status", 20, nullptr, 6).action ==
        InboundAction::Ignore);
  in.ctx.mode = MqttMode::Mqtt;
  CHECK(str(in.decide("homeassistant/status", "offline")) == "Ignore v254 p0");
  in.ctx.mode = MqttMode::Off;
  CHECK(str(in.decide("homeassistant/status", "online")) == "Ignore v254 p0");
}

TEST_CASE("RejectLog: one log per (valve, reason) per 10 s") {
  RejectLog r;
  CHECK(r.shouldLog(9, RejectReason::Inactive, 0));
  CHECK_FALSE(r.shouldLog(9, RejectReason::Inactive, 9999));
  CHECK(r.shouldLog(9, RejectReason::Inactive, 10000));
  CHECK_FALSE(r.shouldLog(9, RejectReason::Inactive, 10001));
  CHECK(r.shouldLog(9, RejectReason::Payload, 10002));   // other reason
  CHECK(r.shouldLog(8, RejectReason::Payload, 10003));   // other valve
  CHECK_FALSE(r.shouldLog(8, RejectReason::Payload, 10004));
  CHECK(r.shouldLog(9, RejectReason::Inactive, 10005));  // not the last logged one any more
  RejectLog w;
  CHECK(w.shouldLog(0, RejectReason::None, 0xFFFFFF00u));
  CHECK_FALSE(w.shouldLog(0, RejectReason::None, 0xFFFFFF00u + 9999u));
  CHECK(w.shouldLog(0, RejectReason::None, 0xFFFFFF00u + 10000u));
}

TEST_CASE("ButtonGate: hold, confirm by the empty echo, expire") {
  ButtonGate g;
  InboundDecision out;
  CHECK_FALSE(g.confirm("VdMot/cmd/restart", 17, out));
  CHECK(g.hold(decision(InboundAction::Restart), "VdMot/cmd/restart", 17, 100));
  CHECK(g.hold(decision(InboundAction::CalibrateValve, 3), "VdMot/cmd/valves/4/calibrate", 28, 200));
  CHECK_FALSE(g.confirm("VdMot/cmd/restar", 16, out));
  CHECK_FALSE(g.confirm("VdMot/cmd/detect", 16, out));
  CHECK_FALSE(g.confirm(nullptr, 17, out));
  CHECK_FALSE(g.expire(100 + ButtonGate::kConfirmMs - 1, out));
  REQUIRE(g.confirm("VdMot/cmd/valves/4/calibrate", 28, out));
  CHECK(out.action == InboundAction::CalibrateValve);
  CHECK(out.valve == 3);
  CHECK_FALSE(g.confirm("VdMot/cmd/valves/4/calibrate", 28, out));  // taken
  REQUIRE(g.expire(100 + ButtonGate::kConfirmMs, out));
  CHECK(out.action == InboundAction::Restart);
  CHECK_FALSE(g.expire(100000, out));
  // Four slots; reset drops them.
  for (int i = 0; i < 4; ++i) CHECK(g.hold(decision(InboundAction::Detect), "t", 1, 0));
  CHECK_FALSE(g.hold(decision(InboundAction::Detect), "t", 1, 0));
  g.reset();
  CHECK_FALSE(g.confirm("t", 1, out));
  CHECK(g.hold(decision(InboundAction::Detect), "t", 1, 0));
  // A topic longer than kTopicMax is not held; exactly kTopicMax is.
  std::string longest(kTopicMax, 'a');
  CHECK(g.hold(decision(InboundAction::Detect), longest.c_str(), longest.size(), 0));
  CHECK(g.confirm(longest.c_str(), longest.size(), out));
  CHECK_FALSE(g.hold(decision(InboundAction::Detect), longest.c_str(), longest.size() + 1, 0));
  CHECK_FALSE(g.hold(decision(InboundAction::Detect), nullptr, 1, 0));
  // Same topic twice: confirmed one by one.
  ButtonGate h;
  CHECK(h.hold(decision(InboundAction::Detect), "x", 1, 0));
  CHECK(h.hold(decision(InboundAction::Restart), "x", 1, 1));
  CHECK(h.confirm("x", 1, out));
  CHECK(out.action == InboundAction::Detect);
  CHECK(h.confirm("x", 1, out));
  CHECK(out.action == InboundAction::Restart);
  // Expire across a millis() wrap.
  ButtonGate w;
  CHECK(w.hold(decision(InboundAction::Detect), "x", 1, 0xFFFFF000u));
  CHECK_FALSE(w.expire(0xFFFFF000u + 4999u, out));
  CHECK(w.expire(0xFFFFF000u + 5000u, out));
}

TEST_CASE("TargetLatch: the newest refused target per valve") {
  TargetLatch l;
  uint8_t v = 99;
  uint8_t p = 99;
  CHECK_FALSE(l.next(0, v, p));
  CHECK(v == 99);
  l.set(3, 40);
  l.set(3, 41);  // newer wins
  l.set(7, 10);
  CHECK(l.pending(3));
  CHECK(l.pending(7));
  CHECK_FALSE(l.pending(4));
  CHECK_FALSE(l.pending(12));
  REQUIRE(l.next(0, v, p));
  CHECK(v == 3);
  CHECK(p == 41);
  REQUIRE(l.next(4, v, p));
  CHECK(v == 7);
  CHECK(p == 10);
  REQUIRE(l.next(8, v, p));  // wraps
  CHECK(v == 3);
  REQUIRE(l.next(3, v, p));
  CHECK(v == 3);
  l.clear(3);
  CHECK_FALSE(l.pending(3));
  REQUIRE(l.next(0, v, p));
  CHECK(v == 7);
  l.clear(7);
  CHECK_FALSE(l.next(0, v, p));
  l.set(12, 5);  // out of range: ignored
  l.clear(12);
  CHECK_FALSE(l.next(0, v, p));
  l.set(11, 0);
  REQUIRE(l.next(11, v, p));
  CHECK(v == 11);
  CHECK(p == 0);
  REQUIRE(l.next(0, v, p));
  CHECK(v == 11);
}
