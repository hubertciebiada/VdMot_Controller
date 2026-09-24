// Smoke test: the core links, the shared helpers and the JSON writer work.
// Module tests live in test_<module>.cpp next to this file.
#include <string.h>

#include <string>

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

TEST_CASE("Backoff: exponential, capped, due however old the last attempt is") {
  Backoff b(5000, 60000);
  CHECK(b.due(0));
  CHECK(b.due(0x90000000u));  // armed: due at any time
  b.onFailure(1000);
  CHECK_FALSE(b.due(5999));
  CHECK(b.due(6000));
  CHECK(b.delayMs() == 10000);
  b.onFailure(6000);
  CHECK_FALSE(b.due(15999));
  CHECK(b.due(16000));
  b.onFailure(16000);  // 20 s
  b.onFailure(36000);  // 40 s
  CHECK(b.delayMs() == 60000);
  b.onFailure(76000);  // 60 s, capped from here on
  CHECK_FALSE(b.due(135999));
  CHECK(b.due(136000));
  b.onFailure(136000);
  CHECK(b.delayMs() == 60000);
  CHECK_FALSE(b.due(195999));
  CHECK(b.due(196000));

  // Success at boot, connection lost 30 days later: due at once (a deadline
  // compared with timeReached() would read as 20 days in the future).
  b.reset();
  CHECK(b.delayMs() == 5000);
  const uint32_t day30 = 30u * 86400u * 1000u;
  CHECK(day30 > 0x80000000u);
  CHECK(b.due(day30));
  CHECK_FALSE(timeReached(day30, 0));  // the defect Backoff avoids
  b.onFailure(day30);
  CHECK_FALSE(b.due(day30 + 4999));
  CHECK(b.due(day30 + 5000));
  // Across the millis() wrap.
  b.reset();
  b.onFailure(0xFFFFF000u);
  CHECK_FALSE(b.due(0xFFFFFFFFu));
  CHECK(b.due(0x00000388u));  // 0x1000 + 0x388 = 5000 ms later

  // Degenerate parameters.
  Backoff z(0, 0);
  z.onFailure(10);
  CHECK(z.delayMs() == 1);
  CHECK_FALSE(z.due(10));
  CHECK(z.due(11));
  Backoff odd(7, 20);
  odd.onFailure(0);
  CHECK(odd.delayMs() == 14);
  odd.onFailure(7);
  CHECK(odd.delayMs() == 20);
  odd.onFailure(21);
  CHECK(odd.delayMs() == 20);
  CHECK_FALSE(odd.due(40));
  CHECK(odd.due(41));
}

TEST_CASE("UTF-8 text and the name policy") {
  struct Seq {
    const char* s;
    size_t avail;
    size_t len;
  };
  const Seq seqs[] = {
      {"\xc2\xa0", 2, 2},          // U+00A0, first after the C1 block
      {"\xc2\x9f", 2, 0},          // U+009F C1 control
      {"\xc2\x80", 2, 0},          // U+0080 C1 control
      {"\xc3\xa4", 2, 2},          // a-umlaut
      {"\xc3\xa4", 1, 0},          // truncated by avail
      {"\xc3" "a", 2, 0},           // bad continuation
      {"\xc0\xaf", 2, 0},          // overlong '/'
      {"\xc1\xbf", 2, 0},          // overlong
      {"\xdf\xbf", 2, 2},          // U+07FF
      {"\xe0\xa0\x80", 3, 3},      // U+0800
      {"\xe0\x9f\xbf", 3, 0},      // overlong U+07FF
      {"\xe2\x82\xac", 3, 3},      // euro sign
      {"\xed\x9f\xbf", 3, 3},      // U+D7FF
      {"\xed\xa0\x80", 3, 0},      // U+D800 surrogate
      {"\xed\xbf\xbf", 3, 0},      // U+DFFF surrogate
      {"\xee\x80\x80", 3, 3},      // U+E000
      {"\xef\xbf\xbf", 3, 3},      // U+FFFF
      {"\xf0\x90\x80\x80", 4, 4},  // U+10000
      {"\xf0\x8f\xbf\xbf", 4, 0},  // overlong
      {"\xf0\x9f\x98\x80", 4, 4},  // emoji
      {"\xf0\x9f\x98\x80", 3, 0},  // truncated
      {"\xf4\x8f\xbf\xbf", 4, 4},  // U+10FFFF
      {"\xf4\x90\x80\x80", 4, 0},  // above U+10FFFF
      {"\xf5\x80\x80\x80", 4, 0},
      {"\xff", 1, 0},
      {"\x80", 1, 0},               // stray continuation
      {"a", 1, 0},                   // ASCII is not a sequence
  };
  for (const Seq& q : seqs) {
    CAPTURE(q.s);
    CHECK(utf8SequenceLength(q.s, q.avail) == q.len);
  }
  CHECK(utf8SequenceLength(nullptr, 4) == 0);
  CHECK(utf8SequenceLength("\xc3\xa4", 0) == 0);

  CHECK(isPrintableText("", 0));
  CHECK(isPrintableText(nullptr, 0));
  CHECK_FALSE(isPrintableText(nullptr, 1));
  CHECK(isPrintableText(" ~", 2));
  CHECK(isPrintableText("K\xc3\xbc" "che \xe2\x82\xac", 10));
  CHECK_FALSE(isPrintableText("a\x1f", 2));
  CHECK_FALSE(isPrintableText("a\x7f", 2));
  CHECK_FALSE(isPrintableText("a\xc3", 2));
  CHECK_FALSE(isPrintableText("\xc3\xa4\xa4", 3));
  CHECK(isPrintableText("a\x01", 1));  // only `len` bytes count

  CHECK(isSafeName("K\xc3\xbc" "che", 10, false));
  CHECK(isSafeName(" Bad ", 10, false));
  CHECK(isSafeName("\xc2\xb0" "C", 8, false));
  CHECK(isSafeName("", 10, true));
  CHECK_FALSE(isSafeName("", 10, false));
  CHECK_FALSE(isSafeName(nullptr, 10, true));
  CHECK_FALSE(isSafeName("\xc3\xa4\xc3\xa4\xc3\xa4\xc3\xa4\xc3\xa4\xc3\xa4", 11, false));  // 12 bytes
  CHECK(isSafeName("\xc3\xa4\xc3\xa4\xc3\xa4\xc3\xa4\xc3\xa4", 10, false));
  for (const char* bad : {"a+", "#", "a/b", "q\"", "b\\", "a\tb", "a\x7f", "a\xc2\x85", "a\xe4"}) {
    CAPTURE(bad);
    CHECK_FALSE(isSafeName(bad, 10, false));
  }
}

TEST_CASE("buildHostname: a DHCP/mDNS name from the station name") {
  const char* const cases[][2] = {
      {"VdMot", "VdMot"},
      {"VdMot_FBH-2", "VdMot_FBH-2"},
      {"My Station", "My-Station"},
      {"  Haus  Ost  ", "Haus-Ost"},
      {"Fu\xc3\x9f" "boden", "Fu-boden"},
      {"K\xc3\xbc" "che OG", "K-che-OG"},
      {"a - b", "a-b"},
      {"-x-", "x"},
      {"--", "VdMot"},
      {"\xc3\xa4\xc3\xb6", "VdMot"},
      {"", "VdMot"},
      {"a.b,c", "a-b-c"},
  };
  char out[21];
  for (const auto& c : cases) {
    CAPTURE(c[0]);
    CHECK(buildHostname(c[0], out, sizeof out) == strlen(c[1]));
    CHECK(std::string(out) == c[1]);
  }
  CHECK(buildHostname(nullptr, out, sizeof out) == 5);
  CHECK(std::string(out) == "VdMot");
  // Bounded by cap, never ends with '-'.
  char small[6];
  CHECK(buildHostname("abcd efgh", small, sizeof small) == 4);
  CHECK(std::string(small) == "abcd");
  CHECK(buildHostname("abcdefgh", small, sizeof small) == 5);
  CHECK(std::string(small) == "abcde");
  char tiny[5] = "xxxx";
  CHECK(buildHostname("abc", tiny, sizeof tiny) == 0);
  CHECK(tiny[0] == '\0');
  CHECK(buildHostname("abc", nullptr, 8) == 0);
  CHECK(buildHostname("abc", tiny, 0) == 0);
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
