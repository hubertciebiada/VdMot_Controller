// MQTT policy edge cases: repeated drops, client id buffers, echo reset.
#include <string.h>

#include "doctest.h"
#include "vdm/mqtt_policy.h"

using namespace vdm;

TEST_CASE("ReconnectPacer: a second drop without a new connect is not another failure") {
  ReconnectPacer p(2000, 60000);
  p.onConnected(0);
  p.onDropped(100);
  CHECK(p.delayMs() == 4000);
  p.onDropped(200);
  CHECK(p.delayMs() == 4000);
  CHECK(p.due(2100));
}

TEST_CASE("buildMqttClientId: small buffers") {
  const uint8_t mac[6] = {0x24, 0x0a, 0xc4, 0xa1, 0xb2, 0xc3};
  char out[4] = {'X', 'X', 'X', 'X'};
  CHECK(buildMqttClientId("VdMot", mac, out, 0) == 0);
  CHECK(out[0] == 'X');
  CHECK(buildMqttClientId("VdMot", mac, out, 1) == 0);
  CHECK(out[0] == '\0');
  CHECK(out[1] == 'X');
}

TEST_CASE("buildMqttClientId: the host part is cut to 16 chars") {
  const uint8_t mac[6] = {0x24, 0x0a, 0xc4, 0xa1, 0xb2, 0xc3};
  char out[32];
  CHECK(buildMqttClientId("abcdefghijklmnopqrstuvwxyz", mac, out, sizeof out) == 23);
  CHECK(strcmp(out, "abcdefghijklmnop-a1b2c3") == 0);
  CHECK(buildMqttClientId("abcdefghijklmno", mac, out, sizeof out) == 22);
  CHECK(strcmp(out, "abcdefghijklmno-a1b2c3") == 0);
}

TEST_CASE("EchoFilter: reset forgets every published value") {
  EchoFilter f;
  f.published(0, 5);
  f.published(11, 7);
  CHECK(f.isEcho(0, 5));
  CHECK(f.isEcho(11, 7));
  f.reset();
  CHECK_FALSE(f.isEcho(0, 5));
  CHECK_FALSE(f.isEcho(11, 7));
}

TEST_CASE("TargetLatch: out-of-range valves are ignored") {
  TargetLatch l;
  l.set(kValveCount, 40);
  uint8_t v = 0xFF, pos = 0xFF;
  CHECK_FALSE(l.next(0, v, pos));
  CHECK_FALSE(l.pending(kValveCount));
  l.set(0, 30);
  l.clear(kValveCount);
  CHECK(l.next(5, v, pos));
  CHECK(v == 0);
  CHECK(pos == 30);
}
