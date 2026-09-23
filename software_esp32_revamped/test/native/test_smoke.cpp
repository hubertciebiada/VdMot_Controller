// Smoke test: the core links, the shared helpers and the JSON writer work.
// Module tests live in test_<module>.cpp next to this file.
#include <string.h>

#include "doctest.h"
#include "vdm/common.h"
#include "vdm/config.h"
#include "vdm/json_writer.h"
#include "vdm/version.h"

using namespace vdm;

TEST_CASE("version parses legacy and revamped STM strings") {
  Version v;
  const char* s = "1.4.9_Dev_C2";
  REQUIRE(parseVersion(s, strlen(s), v));
  CHECK(v.major == 1);
  CHECK(v.minor == 4);
  CHECK(v.patch == 9);
  CHECK(strcmp(v.suffix, "_Dev") == 0);
  CHECK(strcmp(v.hw, "C2") == 0);
  CHECK_FALSE(isRevamped(v));

  Version r;
  s = "2.0.0-revamped_C2";
  REQUIRE(parseVersion(s, strlen(s), r));
  CHECK(isRevamped(r));
  CHECK(strcmp(r.hw, "C2") == 0);
  CHECK(compareVersion(r, v) > 0);

  char out[32];
  CHECK(formatVersion(r, out, sizeof out) == strlen(s));
  CHECK(strcmp(out, s) == 0);

  Version bad;
  s = "1.4";
  CHECK_FALSE(parseVersion(s, strlen(s), bad));
  CHECK_FALSE(bad.valid);
  s = "1.4.9 C2";
  CHECK_FALSE(parseVersion(s, strlen(s), bad));
}

TEST_CASE("firmware version carries the revamped suffix on target builds") {
  // Native builds have no VDM_VERSION flag.
  CHECK(strcmp(firmwareVersion(), "0.0.0-native") == 0);
  CHECK(strcmp(minStmVersion(), "1.4.0") == 0);
}

TEST_CASE("strict number and address helpers") {
  uint32_t u = 7;
  CHECK(parseUint("100", 3, 100, u));
  CHECK(u == 100);
  CHECK_FALSE(parseUint("101", 3, 100, u));
  CHECK_FALSE(parseUint("+1", 2, 100, u));
  CHECK_FALSE(parseUint("", 0, 100, u));
  CHECK(u == 100);

  int32_t i = 0;
  CHECK(parseInt("-1270", 5, -32768, 32767, i));
  CHECK(i == -1270);
  CHECK_FALSE(parseInt("-", 1, -5, 5, i));

  uint32_t ip = 0;
  CHECK(parseIpv4("192.168.1.2", 11, ip));
  CHECK(ip == 0x0201A8C0u);
  char buf[16];
  CHECK(formatIpv4(ip, buf, sizeof buf) == 11);
  CHECK(strcmp(buf, "192.168.1.2") == 0);
  CHECK_FALSE(parseIpv4("192.168.1.256", 13, ip));
  CHECK_FALSE(parseIpv4("1.2.3", 5, ip));

  OneWireId id;
  const char* t = "28-84-37-94-97-FF-03-23";
  REQUIRE(parseOneWireId(t, strlen(t), id));
  char idText[24];
  CHECK(formatOneWireId(id, idText, sizeof idText) == 23);
  CHECK(strcmp(idText, "28-84-37-94-97-ff-03-23") == 0);
  CHECK(elapsedMs(5, 0xFFFFFFFBu) == 10);
  CHECK(timeReached(5, 0xFFFFFFFBu));
}

TEST_CASE("json writer escapes, nests and stays bounded") {
  char buf[64];
  JsonWriter jw(buf, sizeof buf);
  jw.beginObject();
  jw.kv("name", "a\"b\\c\n");
  jw.key("t");
  jw.fixed(-5, 1);
  jw.key("list");
  jw.beginArray();
  jw.value(true);
  jw.nullValue();
  jw.value(static_cast<uint32_t>(4000000000u));
  jw.endArray();
  jw.endObject();
  REQUIRE(jw.complete());
  CHECK(strcmp(buf, "{\"name\":\"a\\\"b\\\\c\\n\",\"t\":-0.5,\"list\":[true,null,4000000000]}") == 0);

  char tiny[8];
  JsonWriter small(tiny, sizeof tiny);
  small.beginArray();
  small.value("too long for this buffer");
  CHECK_FALSE(small.ok());
  CHECK(strcmp(tiny, "[") == 0);

  JsonWriter misuse(buf, sizeof buf);
  misuse.beginObject();
  misuse.value(1);  // value without key
  CHECK_FALSE(misuse.ok());
}

TEST_CASE("config defaults are the documented factory values") {
  Config c;
  setDefaults(c);
  CHECK(strcmp(c.station, "VdMot") == 0);
  CHECK(c.mqtt.port == 1883);
  CHECK(c.calib.dayMask == 9);
  CHECK(strcmp(c.time.tzPosix, "CET-1CEST,M3.5.0,M10.5.0/3") == 0);
}
