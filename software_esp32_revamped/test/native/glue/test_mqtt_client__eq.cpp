// Tests of src/mqtt_client.cpp for inputs that are rare but real: a publish that fails as the
// last one of the full publish, and valve profiles whose CRC-32 hits the values the diag
// comparison stores for "no profile" (every CRC value is reachable: the samples are STM data).
#include <stddef.h>
#include <string.h>

#include <string>
#include <vector>

#include <vdm/config.h>
#include <vdm/mqtt_topics.h>

#include "glue_test.h"
#include "mqtt_client.h"

namespace {

vdm::Config& useMqtt() {
  vdm::Config& c = sib::storage().active;
  c.mqtt.mode = vdm::MqttMode::Mqtt;
  vdm::copyString(c.mqtt.host, sizeof c.mqtt.host, "broker.lan");
  c.mqtt.port = 1884;
  c.mqtt.keepAliveS = 30;
  vdm::copyString(c.station, sizeof c.station, "VdMot");
  c.valves[0].active = true;
  ++sib::storage().revision;
  sib::storage().haCleanupDone = true;
  sib::storage().haLayout = 2;
  sib::net().up = true;
  fakes::fs().mounted = true;
  return c;
}

std::string topicOf(const vdm::Config& c, vdm::Topic t) {
  vdm::TopicContext tc;
  vdm::copyString(tc.station, sizeof tc.station, vdm::mqttRootTopic(c));
  tc.pathAsRoot = c.mqtt.pathAsRoot;
  tc.separate = c.mqtt.separate;
  char buf[vdm::kTopicMax + 1];
  vdm::buildTopic(tc, t, nullptr, buf, sizeof buf);
  return buf;
}

void runTask(long passes) {
  fakes::rtos().stopAfterYields(passes);
  CHECK_THROWS_AS(mqtt::task(nullptr), fakes::YieldLimit);
}

void settle(long passes = 40) {
  mqtt::begin();
  runTask(passes);
  REQUIRE(fakes::mqtt().connected);
}

std::vector<std::string> payloads(const std::string& topic) {
  std::vector<std::string> v;
  for (const fakes::MqttMessage& m : fakes::mqtt().publishedTo(topic)) v.push_back(m.payload);
  return v;
}

vdm::StmSnapshot& snap() { return sib::app().snapshot; }
void publishSnap() { ++sib::app().snapshotRevision; }

vdm::StmSnapshot& linkUp() {
  vdm::StmSnapshot& s = snap();
  s.link = vdm::LinkState::Up;
  s.proto = 2;
  s.valves[0].known = true;
  publishSnap();
  return s;
}

// ---- profiles with a chosen CRC-32 over the whole object (what the diag comparison hashes)

uint32_t crcOf(const vdm::Profile& p) {
  return vdm::crc32(reinterpret_cast<const uint8_t*>(&p), sizeof p);
}

// Every byte zero (padding too): the bytes the firmware hashes are the ones set here.
void clearProfile(vdm::Profile& p) { memset(static_cast<void*>(&p), 0, sizeof p); }

// CRC-32 is affine in the message bits, so the 32 bits of one sample count reach every value:
// solves for them over GF(2).
void forceCrc(vdm::Profile& p, size_t offset, uint32_t target) {
  uint8_t* b = reinterpret_cast<uint8_t*>(&p);
  memset(b + offset, 0, 4);
  const uint32_t base = crcOf(p);
  uint32_t basis[32] = {};  // basis[k]: a CRC change whose highest set bit is k
  uint32_t combo[32] = {};  // the window bits that make it
  for (int j = 0; j < 32; ++j) {
    const uint8_t bit = static_cast<uint8_t>(1u << (j % 8));
    b[offset + j / 8] ^= bit;
    uint32_t v = crcOf(p) ^ base;
    b[offset + j / 8] ^= bit;
    uint32_t c = 1u << j;
    for (int k = 31; k >= 0 && v != 0; --k) {
      if ((v >> k & 1u) == 0) continue;
      if (basis[k] == 0) {
        basis[k] = v;
        combo[k] = c;
        v = 0;
      } else {
        v ^= basis[k];
        c ^= combo[k];
      }
    }
  }
  uint32_t v = base ^ target;
  uint32_t x = 0;
  for (int k = 31; k >= 0; --k) {
    if ((v >> k & 1u) == 0) continue;
    REQUIRE(basis[k] != 0);
    v ^= basis[k];
    x ^= combo[k];
  }
  for (int j = 0; j < 32; ++j) {
    if (x >> j & 1u) b[offset + j / 8] ^= static_cast<uint8_t>(1u << (j % 8));
  }
  REQUIRE(crcOf(p) == target);
}

// Three samples; the count of the third one is solved for `crc`.
void setProfile(vdm::Profile& p, uint8_t valve, uint32_t crc) {
  clearProfile(p);
  p.valve = valve;
  p.count = 3;
  for (uint8_t i = 0; i < 3; ++i) {
    p.samples[i].count = 100u * (i + 1u);
    p.samples[i].current = static_cast<uint16_t>(40 + i);
  }
  forceCrc(p, offsetof(vdm::Profile, samples) + 2 * sizeof(vdm::ProfileSample) +
                  offsetof(vdm::ProfileSample, count),
           crc);
}

std::string profileJson(const vdm::Profile& p) {
  return "{\"valve\":" + std::to_string(p.valve + 1) +
         ",\"count\":3,\"samples\":[[100,40],[200,41],[" + std::to_string(p.samples[2].count) +
         ",42]]}";
}

}  // namespace

// ---------------------------------------------------------------- stm/status and failsafe

TEST_CASE("mqtt system: a failsafe publish that fails at the end of the full publish is retried") {
  glue::begin();
  const vdm::Config& c = useMqtt();
  linkUp();
  fakes::mqtt().failPublishTopic = topicOf(c, vdm::Topic::Failsafe);
  mqtt::begin();
  runTask(150);  // the first session, 2 s of back-off, the second session and its full publish
  // The full publish ends with stm/status (sent) and failsafe (failed: the session is dropped).
  // The on-change check of the same pass finds failsafe unpublished and sends both again, on
  // the dead session.
  CHECK(mqtt::status().publishFailures == 3);
  CHECK(fakes::mqtt().connects == 2);
  CHECK(payloads(topicOf(c, vdm::Topic::Failsafe)) == std::vector<std::string>{"0"});
}

// ---------------------------------------------------------------- profiles

TEST_CASE("mqtt diag: a profile after an empty one goes out, also with the CRC-32 1") {
  glue::begin();
  useMqtt();
  vdm::StmSnapshot& s = linkUp();
  s.valves[0].hasExtended = true;
  clearProfile(s.profiles[0]);
  publishSnap();
  settle();
  REQUIRE(payloads("VdMot/diag/valves/1/profile").empty());
  setProfile(s.profiles[0], 0, 1);
  publishSnap();
  runTask(5);
  CHECK(payloads("VdMot/diag/valves/1/profile") ==
        std::vector<std::string>{profileJson(s.profiles[0])});
}

TEST_CASE("mqtt diag: an empty profile is stored as CRC 0, not as the CRC of its bytes") {
  glue::begin();
  useMqtt();
  vdm::StmSnapshot& s = linkUp();
  s.valves[0].hasExtended = true;
  clearProfile(s.profiles[0]);
  const uint32_t emptyBytes = crcOf(s.profiles[0]);
  REQUIRE(emptyBytes != 0);
  publishSnap();
  settle();
  setProfile(s.profiles[0], 0, emptyBytes);
  publishSnap();
  runTask(5);
  CHECK(payloads("VdMot/diag/valves/1/profile") ==
        std::vector<std::string>{profileJson(s.profiles[0])});
}

TEST_CASE("mqtt diag: a valve whose first pass ran out of budget compares its profile with 0") {
  glue::begin();
  vdm::Config& c = useMqtt();
  c.valves[1].active = true;
  vdm::StmSnapshot& s = linkUp();
  s.valves[0].hasExtended = true;
  s.valves[1].hasExtended = true;
  clearProfile(s.profiles[0]);
  setProfile(s.profiles[1], 1, 1);  // known at the connect, CRC-32 1
  publishSnap();
  settle();
  // First diag pass: protocol and link take two of the four messages, the last-move checks of
  // valves 1 and 2 the others, so valve 2 stores no CRC. The next pass compares its profile
  // with the default 0.
  CHECK(payloads("VdMot/diag/valves/2/profile") ==
        std::vector<std::string>{profileJson(s.profiles[1])});
}
