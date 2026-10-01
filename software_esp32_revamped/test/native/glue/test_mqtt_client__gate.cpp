// Tests of src/mqtt_client.cpp for the memory of a discovery run and its start: the automatic
// runs wait for settled STM inputs (vdm::DiscoveryGate, at most 120 s), a manual request runs at
// once; the context and the payload buffer of a run, and the copy of a profile, live on the heap
// only while they are used, and a run without memory waits for the next pass.
#include <stddef.h>

#include <string>
#include <vector>

#include <vdm/config.h>
#include <vdm/ha_discovery.h>
#include <vdm/mqtt_policy.h>

#include "glue_test.h"
#include "mqtt_client.h"

namespace {

vdm::Config& useHa() {
  vdm::Config& c = sib::storage().active;
  c.mqtt.mode = vdm::MqttMode::MqttHa;
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

void runTask(long passes) {
  fakes::rtos().stopAfterYields(passes);
  CHECK_THROWS_AS(mqtt::task(nullptr), fakes::YieldLimit);
}

vdm::StmSnapshot& snap() { return sib::app().snapshot; }
void publishSnap() { ++sib::app().snapshotRevision; }

// The STM link up, its inputs not settled yet.
vdm::StmSnapshot& linkUp() {
  vdm::StmSnapshot& s = snap();
  s.link = vdm::LinkState::Up;
  s.proto = 3;
  s.valves[0].known = true;
  publishSnap();
  return s;
}

size_t runs() { return sib::logger().withCode(vdm::EventCode::HaDiscoverySent).size(); }

size_t countPrefix(const std::string& prefix) {
  size_t n = 0;
  for (const fakes::MqttMessage& m : fakes::mqtt().published) {
    n += m.topic.compare(0, prefix.size(), prefix) == 0;
  }
  return n;
}

// Heap blocks of a discovery run: its context and payload buffer in one block.
size_t runBlocks() {
  size_t n = 0;
  for (size_t a : fakes::heap().allocated) {
    n += a >= sizeof(vdm::DiscoveryContext) + vdm::kDiscoveryPayloadMax + 1 && a < 8192;
  }
  return n;
}

constexpr long kPassesPerSecond = 50;  // one pass every kDiscoveryPaceMs (20 ms)

}  // namespace

// ---------------------------------------------------------------- the gate

TEST_CASE("mqtt discovery gate: a connect before the STM settled waits, then one run") {
  glue::begin();
  useHa();
  vdm::StmSnapshot& s = linkUp();
  mqtt::begin();
  runTask(40);
  REQUIRE(fakes::mqtt().connected);
  CHECK_FALSE(mqtt::status().discoveryRunning);
  CHECK(countPrefix("homeassistant/") == 0);
  // the STM start-up: sensor assignments come in step by step (each changes the inputs)
  for (uint8_t slot = 1; slot <= 3; ++slot) {
    s.valves[0].sensorSlot[0] = slot;
    publishSnap();
    runTask(5 * kPassesPerSecond);
  }
  CHECK(countPrefix("homeassistant/") == 0);
  CHECK(runs() == 0);
  s.sensorsSettled = true;
  publishSnap();
  runTask(1);
  CHECK(mqtt::status().discoveryRunning);
  runTask(1000);
  CHECK(runs() == 1);
  CHECK_FALSE(mqtt::status().discoveryRunning);
  // nothing changed since: no further run
  publishSnap();
  runTask(500);
  CHECK(runs() == 1);
  // a change of the inputs after the STM settled runs at once (through the gate, not held)
  vdm::copyString(s.version.hw, sizeof s.version.hw, "C2");
  publishSnap();
  runTask(1);
  CHECK(mqtt::status().discoveryRunning);
}

TEST_CASE("mqtt discovery gate: without a settling STM the run starts 120 s after the connect") {
  glue::begin();
  useHa();
  linkUp();
  mqtt::begin();
  runTask(1);  // the connect
  REQUIRE(fakes::mqtt().connected);
  runTask(vdm::DiscoveryGate::kMaxWaitMs / 1000 * kPassesPerSecond - 10);
  CHECK(countPrefix("homeassistant/") == 0);
  CHECK_FALSE(mqtt::status().discoveryRunning);
  runTask(20);
  CHECK(mqtt::status().discoveryRunning);
  runTask(1000);
  CHECK(runs() == 1);
}

TEST_CASE("mqtt discovery gate: a manual request runs at once and answers the waiting run") {
  glue::begin();
  useHa();
  linkUp();
  mqtt::begin();
  runTask(40);
  REQUIRE(fakes::mqtt().connected);
  mqtt::requestDiscovery(mqtt::DiscoveryAction::Delete);
  runTask(1);
  CHECK(mqtt::status().discoveryRunning);
  runTask(1000);
  REQUIRE(runs() == 1);
  // the waiting automatic run is gone: nothing publishes the configs the user deleted
  const size_t before = fakes::mqtt().published.size();
  runTask(vdm::DiscoveryGate::kMaxWaitMs / 1000 * kPassesPerSecond);
  CHECK(runs() == 1);
  for (size_t i = before; i < fakes::mqtt().published.size(); ++i) {
    CHECK(fakes::mqtt().published[i].topic.compare(0, 14, "homeassistant/") != 0);
  }
}

TEST_CASE("mqtt discovery gate: a session that ends drops the waiting run, the next one asks") {
  glue::begin();
  useHa();
  linkUp();
  mqtt::begin();
  runTask(1);  // the connect
  REQUIRE(fakes::mqtt().connects == 1);
  runTask(60 * kPassesPerSecond);
  fakes::mqtt().dropConnection();
  runTask(100);  // the reconnect after the back-off
  REQUIRE(fakes::mqtt().connects == 2);
  // 120 s after the first connect: the wait of the first session is gone with it
  runTask(60 * kPassesPerSecond);
  CHECK_FALSE(mqtt::status().discoveryRunning);
  CHECK(runs() == 0);
  // 120 s after the second connect (about 60 s after the first)
  runTask(55 * kPassesPerSecond);
  CHECK_FALSE(mqtt::status().discoveryRunning);
  CHECK(runs() == 0);
  runTask(5 * kPassesPerSecond);
  CHECK((mqtt::status().discoveryRunning || runs() == 1));
}

TEST_CASE("mqtt discovery gate: the cleanup of mode 1 runs at once, without memory later") {
  glue::begin();
  vdm::Config& c = useHa();
  c.mqtt.mode = vdm::MqttMode::Mqtt;
  sib::storage().haCleanupDone = false;
  linkUp();
  mqtt::begin();
  runTask(1);  // the connect: the cleanup starts with it
  CHECK(mqtt::status().discoveryRunning);
  runTask(1000);
  CHECK(sib::storage().haCleanupMarks == 1);
  CHECK(runs() == 1);
  // the next boot, no memory at the connect: the gate takes the cleanup
  sib::storage().haCleanupDone = false;
  fakes::mqtt().dropConnection();
  fakes::heap().failAll = true;
  runTask(100);
  REQUIRE(fakes::mqtt().connects == 2);
  CHECK_FALSE(mqtt::status().discoveryRunning);
  fakes::heap().failAll = false;
  runTask(1);
  CHECK_FALSE(mqtt::status().discoveryRunning);  // the STM inputs are not settled
  snap().sensorsSettled = true;
  publishSnap();
  runTask(1);
  CHECK(mqtt::status().discoveryRunning);
  runTask(1000);
  CHECK(sib::storage().haCleanupMarks == 2);
}

// ---------------------------------------------------------------- memory

TEST_CASE("mqtt discovery: a run takes its context and payload from the heap, one block a run") {
  glue::begin();
  useHa();
  vdm::StmSnapshot& s = linkUp();
  s.sensorsSettled = true;
  publishSnap();
  mqtt::begin();
  CHECK(runBlocks() == 0);  // nothing at boot
  runTask(1000);
  REQUIRE(runs() == 1);
  CHECK(runBlocks() == 1);
  mqtt::requestDiscovery(mqtt::DiscoveryAction::Publish);
  runTask(1000);
  CHECK(runs() == 2);
  CHECK(runBlocks() == 2);
  // a request during a run restarts it in the block it has
  mqtt::requestDiscovery(mqtt::DiscoveryAction::Publish);
  runTask(5);
  REQUIRE(mqtt::status().discoveryRunning);
  mqtt::requestDiscovery(mqtt::DiscoveryAction::Publish);
  runTask(1000);
  CHECK(runs() == 3);
  CHECK(runBlocks() == 3);
}

TEST_CASE("mqtt discovery: without memory a run waits for the next pass, automatic or manual") {
  glue::begin();
  useHa();
  vdm::StmSnapshot& s = linkUp();
  s.sensorsSettled = true;
  publishSnap();
  mqtt::begin();
  fakes::heap().failAll = true;
  runTask(40);
  REQUIRE(fakes::mqtt().connected);
  CHECK_FALSE(mqtt::status().discoveryRunning);
  CHECK(countPrefix("homeassistant/") == 0);
  fakes::heap().failAll = false;
  runTask(1);
  CHECK(mqtt::status().discoveryRunning);
  runTask(1000);
  REQUIRE(runs() == 1);
  fakes::heap().failAll = true;
  mqtt::requestDiscovery(mqtt::DiscoveryAction::Delete);
  runTask(50);
  CHECK_FALSE(mqtt::status().discoveryRunning);
  CHECK(runs() == 1);
  fakes::heap().failAll = false;
  runTask(1);
  CHECK(mqtt::status().discoveryRunning);
  runTask(1000);
  CHECK(runs() == 2);
}

TEST_CASE("mqtt discovery: HA back online without memory for the run gets it later") {
  glue::begin();
  useHa();
  vdm::StmSnapshot& s = linkUp();
  s.sensorsSettled = true;
  publishSnap();
  mqtt::begin();
  runTask(1000);
  REQUIRE(runs() == 1);
  fakes::mqtt().inbox.push_back({"homeassistant/status", "offline", false});
  runTask(2);
  fakes::heap().failAll = true;
  fakes::mqtt().inbox.push_back({"homeassistant/status", "online", false});
  runTask(2);
  CHECK(mqtt::regulatorState().ha == vdm::HaStatus::Online);
  CHECK_FALSE(mqtt::status().discoveryRunning);
  fakes::heap().failAll = false;
  runTask(1);
  CHECK(mqtt::status().discoveryRunning);
  runTask(1000);
  CHECK(runs() == 2);
}

TEST_CASE("mqtt diag: the largest profile fits its JSON buffer") {
  glue::begin();
  vdm::Config& c = useHa();
  c.mqtt.mode = vdm::MqttMode::Mqtt;
  c.valves[11].active = true;
  vdm::StmSnapshot& s = linkUp();
  s.valves[11].known = true;
  s.valves[11].hasExtended = true;
  publishSnap();
  mqtt::begin();
  runTask(40);
  REQUIRE(fakes::mqtt().connected);
  vdm::Profile& p = sib::app().profiles[11];
  p.valve = 11;
  p.count = vdm::kProfileMaxSamples;
  for (vdm::ProfileSample& x : p.samples) {
    x.count = UINT32_MAX;
    x.current = UINT16_MAX;
  }
  ++s.profileSeq[11];
  publishSnap();
  runTask(5);
  const std::vector<fakes::MqttMessage> m = fakes::mqtt().publishedTo("VdMot/diag/valves/12/profile");
  REQUIRE(m.size() == 1);
  CHECK(m[0].payload.size() == 643);
  CHECK(m[0].payload.rfind("{\"valve\":12,\"count\":32,\"samples\":[[4294967295,65535],", 0) == 0);
}
