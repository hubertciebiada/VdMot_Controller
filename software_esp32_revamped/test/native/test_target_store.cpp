// Desired-target persistence: record codec, boot choice, capture/restore and
// the debounced NVS saver.
#include <string.h>

#include "doctest.h"
#include "vdm/config.h"
#include "vdm/target_store.h"

using namespace vdm;

namespace {

// valve 0: 42 from MQTT, valve 11: 100 from an assembly, the rest invalid.
const uint8_t kGolden[kPersistedTargetsSize] = {
    0x56, 0x44, 0x54, 0x47, 0x01, 0x0C, 0x01, 0x2A, 0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x64, 0x05, 0x44, 0x80, 0xBC, 0x76};

PersistedTargets golden() {
  PersistedTargets t;
  t.valid[0] = true;
  t.pos[0] = 42;
  t.source[0] = TargetSource::Mqtt;
  t.valid[11] = true;
  t.pos[11] = 100;
  t.source[11] = TargetSource::Assembly;
  return t;
}

// Rewrites the CRC after a deliberate change.
void fixCrc(uint8_t (&b)[kPersistedTargetsSize]) {
  const uint32_t crc = crc32(b, 42);
  for (int i = 0; i < 4; ++i) b[42 + i] = static_cast<uint8_t>(crc >> (8 * i));
}

bool allInvalid(const PersistedTargets& t) {
  for (uint8_t v = 0; v < kValveCount; ++v) {
    if (t.valid[v] || t.pos[v] != 0 || t.source[v] != TargetSource::None) return false;
  }
  return true;
}

}  // namespace

TEST_CASE("targets: golden encoding and round trip") {
  uint8_t out[kPersistedTargetsSize];
  memset(out, 0xEE, sizeof out);
  CHECK(encodeTargets(golden(), out) == kPersistedTargetsSize);
  CHECK(memcmp(out, kGolden, sizeof out) == 0);
  PersistedTargets back;
  REQUIRE(decodeTargets(kGolden, sizeof kGolden, back));
  CHECK(back == golden());
  CHECK(back.pos[0] == 42);
  CHECK(back.source[11] == TargetSource::Assembly);
  CHECK_FALSE(back.valid[5]);
  // Every valve, every source, the boundary positions.
  PersistedTargets full;
  for (uint8_t v = 0; v < kValveCount; ++v) {
    full.valid[v] = true;
    full.pos[v] = static_cast<uint8_t>(v * 9 + 1);
    full.source[v] = static_cast<TargetSource>(v % 6);
  }
  full.pos[0] = 0;
  full.pos[1] = 100;
  encodeTargets(full, out);
  REQUIRE(decodeTargets(out, sizeof out, back));
  CHECK(back == full);
}

TEST_CASE("targets: invalid entries are written as zeros whatever they hold") {
  PersistedTargets t = golden();
  t.pos[5] = 77;
  t.source[5] = TargetSource::Web;
  uint8_t out[kPersistedTargetsSize];
  encodeTargets(t, out);
  CHECK(memcmp(out, kGolden, sizeof out) == 0);
  CHECK(t == golden());  // invalid entries do not compare
  PersistedTargets u = golden();
  u.pos[0] = 43;
  CHECK_FALSE(u == golden());
  CHECK(u != golden());
  u = golden();
  u.source[0] = TargetSource::Web;
  CHECK_FALSE(u == golden());
  u = golden();
  u.valid[3] = true;
  CHECK_FALSE(u == golden());
}

TEST_CASE("targets: every single-bit flip is rejected and clears the output") {
  for (size_t byte = 0; byte < kPersistedTargetsSize; ++byte) {
    for (int bit = 0; bit < 8; ++bit) {
      uint8_t b[kPersistedTargetsSize];
      memcpy(b, kGolden, sizeof b);
      b[byte] = static_cast<uint8_t>(b[byte] ^ (1u << bit));
      PersistedTargets out = golden();
      CAPTURE(byte);
      CAPTURE(bit);
      CHECK_FALSE(decodeTargets(b, sizeof b, out));
      CHECK(allInvalid(out));
    }
  }
}

TEST_CASE("targets: wrong length, null, bad fields with a correct CRC") {
  PersistedTargets out;
  CHECK_FALSE(decodeTargets(kGolden, 45, out));
  uint8_t longer[47] = {};
  memcpy(longer, kGolden, sizeof kGolden);
  CHECK_FALSE(decodeTargets(longer, 47, out));
  CHECK_FALSE(decodeTargets(nullptr, 46, out));
  uint8_t b[kPersistedTargetsSize];
  memcpy(b, kGolden, sizeof b);
  b[7] = 101;  // pos of valve 0
  fixCrc(b);
  CHECK_FALSE(decodeTargets(b, sizeof b, out));
  memcpy(b, kGolden, sizeof b);
  b[7] = 100;
  fixCrc(b);
  CHECK(decodeTargets(b, sizeof b, out));
  memcpy(b, kGolden, sizeof b);
  b[8] = 6;  // source above Assembly
  fixCrc(b);
  CHECK_FALSE(decodeTargets(b, sizeof b, out));
  memcpy(b, kGolden, sizeof b);
  b[6] = 3;  // unknown flag bit
  fixCrc(b);
  CHECK_FALSE(decodeTargets(b, sizeof b, out));
  memcpy(b, kGolden, sizeof b);
  b[4] = 2;  // version
  fixCrc(b);
  CHECK_FALSE(decodeTargets(b, sizeof b, out));
  memcpy(b, kGolden, sizeof b);
  b[5] = 11;  // count
  fixCrc(b);
  CHECK_FALSE(decodeTargets(b, sizeof b, out));
  memcpy(b, kGolden, sizeof b);
  b[3] = 'H';  // magic
  fixCrc(b);
  CHECK_FALSE(decodeTargets(b, sizeof b, out));
  memcpy(b, kGolden, sizeof b);
  b[0] = 'X';
  fixCrc(b);
  CHECK_FALSE(decodeTargets(b, sizeof b, out));
  // An invalid entry (flag byte 0) may carry any value bytes: they are ignored.
  memcpy(b, kGolden, sizeof b);
  b[10] = 200;
  b[11] = 9;
  fixCrc(b);
  REQUIRE(decodeTargets(b, sizeof b, out));
  CHECK_FALSE(out.valid[1]);
  CHECK(out.pos[1] == 0);
}

TEST_CASE("targets: the RTC copy wins, else NVS, else none") {
  uint8_t other[kPersistedTargetsSize];
  PersistedTargets t;
  t.valid[4] = true;
  t.pos[4] = 7;
  t.source[4] = TargetSource::Web;
  encodeTargets(t, other);
  uint8_t garbage[kPersistedTargetsSize];
  memset(garbage, 0xA5, sizeof garbage);
  PersistedTargets out;
  CHECK(chooseTargets(kGolden, sizeof kGolden, other, sizeof other, out) == RestoreSource::Rtc);
  CHECK(out == golden());
  CHECK(chooseTargets(garbage, sizeof garbage, other, sizeof other, out) == RestoreSource::Nvs);
  CHECK(out == t);
  CHECK(chooseTargets(garbage, sizeof garbage, other, 0, out) == RestoreSource::None);
  CHECK(allInvalid(out));
  CHECK(chooseTargets(nullptr, 0, nullptr, 0, out) == RestoreSource::None);
}

TEST_CASE("targets: capture and restore through the model") {
  ValveModel m;
  m.setActiveMask(0x00D);  // valves 0, 2, 3
  TargetReply r;
  r.valve = 0;
  r.target = 30;
  m.applyTarget(r, 0);  // adopted: source Stm
  REQUIRE(m.setDesiredTarget(2, 40, TargetSource::Mqtt, 0));
  m.setAssembly(3, 0);
  PersistedTargets t;
  t.valid[7] = true;  // cleared by the capture
  captureTargets(m, t);
  CHECK(t.valid[0]);
  CHECK(t.pos[0] == 30);
  CHECK(t.source[0] == TargetSource::Stm);
  CHECK(t.valid[2]);
  CHECK(t.source[2] == TargetSource::Mqtt);
  CHECK(t.pos[3] == 100);
  CHECK(t.source[3] == TargetSource::Assembly);
  CHECK_FALSE(t.valid[1]);
  CHECK_FALSE(t.valid[7]);

  t.valid[1] = true;  // valve 1 is inactive in the new model
  t.pos[1] = 5;
  ValveModel n;
  n.setActiveMask(0x00D);
  CHECK(restoreTargets(n, t) == 3);
  CHECK(n.valve(0).source == TargetSource::Restored);
  CHECK(n.valve(2).source == TargetSource::Restored);
  CHECK(n.valve(3).source == TargetSource::Assembly);
  CHECK(n.valve(2).desired == 40);
  CHECK(n.valve(2).sync == TargetSync::Pending);
  CHECK_FALSE(n.valve(1).desiredValid);
  CHECK(restoreTargets(n, PersistedTargets{}) == 0);
}

// ================================================================ saver

TEST_CASE("saver: 5 min after one change") {
  TargetSaver s;
  s.primeStored(PersistedTargets{});
  CHECK_FALSE(s.dirty());
  s.update(golden(), 1000);
  CHECK(s.dirty());
  CHECK_FALSE(s.due(1000 + 299999));
  CHECK(s.due(1000 + 300000));
  CHECK(memcmp(s.bytes(), kGolden, kPersistedTargetsSize) == 0);
  s.saved(true, 301000);
  CHECK_FALSE(s.dirty());
  CHECK_FALSE(s.due(10000000));
  s.update(golden(), 400000);  // equal to the stored copy: never dirty
  CHECK_FALSE(s.dirty());
}

TEST_CASE("saver: a change every minute is written at most 30 min after the first") {
  TargetSaver s;
  s.primeStored(PersistedTargets{});
  PersistedTargets t;
  t.valid[0] = true;
  const uint32_t t0 = 5000;
  for (uint32_t k = 0; k < 30; ++k) {
    t.pos[0] = static_cast<uint8_t>(k + 1);
    s.update(t, t0 + k * 60000);
    CHECK_FALSE(s.due(t0 + k * 60000 + 59999));
  }
  CHECK_FALSE(s.due(t0 + 1799999));
  CHECK(s.due(t0 + 1800000));
}

TEST_CASE("saver: a change back to the stored value is clean again") {
  TargetSaver s;
  s.primeStored(golden());
  CHECK(memcmp(s.bytes(), kGolden, kPersistedTargetsSize) == 0);
  s.update(golden(), 0);
  CHECK_FALSE(s.dirty());
  s.update(PersistedTargets{}, 100);
  CHECK(s.dirty());
  s.update(golden(), 200);
  CHECK_FALSE(s.dirty());
  CHECK_FALSE(s.due(10000000));
  CHECK(memcmp(s.bytes(), kGolden, kPersistedTargetsSize) == 0);
}

TEST_CASE("saver: a failed write is retried one debounce later") {
  TargetSaver s;
  s.primeStored(PersistedTargets{});
  s.update(golden(), 0);
  REQUIRE(s.due(300000));
  s.saved(false, 400000);
  CHECK(s.dirty());
  CHECK_FALSE(s.due(400000 + 299999));
  CHECK(s.due(400000 + 300000));
  s.saved(true, 700000);
  CHECK_FALSE(s.dirty());
  // The stored copy is the saved value now.
  s.update(golden(), 800000);
  CHECK_FALSE(s.dirty());
}

TEST_CASE("saver: the first-change time is kept while dirty") {
  TargetSaver s;
  s.primeStored(PersistedTargets{});
  PersistedTargets t;
  t.valid[0] = true;
  t.pos[0] = 1;
  s.update(t, 1000);
  t.pos[0] = 2;
  s.update(t, 1000 + 1600000);
  CHECK_FALSE(s.due(1000 + 1799999));
  CHECK(s.due(1000 + 1800000));
  s.saved(true, 1000 + 1800000);
  t.pos[0] = 3;
  s.update(t, 2000000);  // a new dirty period starts its own maximum
  CHECK_FALSE(s.due(2000000 + 299999));
  CHECK(s.due(2000000 + 300000));
}
