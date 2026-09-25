// json_api edge cases: the longest STM version that is still written, the sensor list of a valve
// stops at its two slots.
#include <string.h>

#include <new>
#include <string>

#include "doctest.h"
#include "vdm/json_api.h"

using namespace vdm;

namespace {

char gMutBuf[16384];

std::string statusWithVersion(const Version& v) {
  static StatusSnapshot s;
  s = StatusSnapshot{};
  s.stmVersion = v;
  JsonWriter jw(gMutBuf, sizeof gMutBuf);
  REQUIRE(writeStatusJson(jw, s));
  return std::string(gMutBuf, jw.length());
}

}  // namespace

TEST_CASE("status: an STM version of 39 chars is written, one of 40 chars is null") {
  Version v;
  v.valid = true;
  v.major = 65535;
  v.minor = 65535;
  v.patch = 65535;          // "65535.65535.65535": 17 chars
  v.hw[0] = '\0';
  strcpy(v.suffix, "-abcdefghijklmnopqrstu");  // 22 chars: 39 in total
  std::string j = statusWithVersion(v);
  CHECK(j.find("\"version\":\"65535.65535.65535-abcdefghijklmnopqrstu\"") != std::string::npos);
  strcpy(v.suffix, "-abcdefghijklmnopqrstuv");  // 40 chars
  j = statusWithVersion(v);
  CHECK(j.find("abcdefghijklmnopqrstuv") == std::string::npos);
  CHECK(j.find("\"proto\":null,\"version\":null") != std::string::npos);
}

TEST_CASE("valves: only the two sensor slots are listed, whatever follows them in memory") {
  ValveState st;
  // the padding after sensorSlot[] is not zero: a third slot would look assigned
  alignas(ValveView) unsigned char raw[sizeof(ValveView)];
  memset(raw, 0x5A, sizeof raw);
  ValveView* v = new (raw) ValveView;
  v->state = &st;
  v->sensorSlot[0] = 1;
  v->sensorSlot[1] = 2;
  JsonWriter jw(gMutBuf, sizeof gMutBuf);
  REQUIRE(writeValvesJson(jw, v, 1, 0));
  const std::string j(gMutBuf, jw.length());
  CHECK(j.find("\"sensors\":[{\"sensor\":1,\"slot\":1,\"name\":\"\",\"temp\":null},"
               "{\"sensor\":2,\"slot\":2,\"name\":\"\",\"temp\":null}]") != std::string::npos);
  v->~ValveView();
}
