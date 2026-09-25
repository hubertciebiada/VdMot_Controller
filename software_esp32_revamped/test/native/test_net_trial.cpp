// Network trial: record codec, boot rule, trial window, field helpers.
#include <string.h>

#include <string>
#include <vector>

#include "doctest.h"
#include "vdm/net_trial.h"

using namespace vdm;

namespace {

NetConfig staticNet() {
  NetConfig n;
  n.iface = NetInterface::Ethernet;
  n.dhcp = false;
  n.ip = 0x3201A8C0;       // 192.168.1.50
  n.mask = 0x00FFFFFF;     // 255.255.255.0
  n.gateway = 0x0101A8C0;  // 192.168.1.1
  n.dns = 0x0201A8C0;
  n.reconnectTimeoutMin = 7;
  return n;
}

NetConfig wifiNet() {
  NetConfig n;
  n.iface = NetInterface::Wifi;
  n.dhcp = true;
  memset(n.ssid, 'S', 32);
  n.ssid[32] = '\0';
  memset(n.wifiPassword, 'p', 64);
  n.wifiPassword[64] = '\0';
  return n;
}

void addU32(std::vector<uint8_t>& v, uint32_t x) {
  for (int i = 0; i < 4; ++i) v.push_back(static_cast<uint8_t>(x >> (8 * i)));
}

std::vector<uint8_t> encode(const NetTrialRecord& r) {
  std::vector<uint8_t> v(kNetTrialBlobMax);
  v.resize(encodeNetTrial(r, v.data(), v.size()));
  return v;
}

// A blob with its CRC fixed after a change of the bytes before it.
std::vector<uint8_t> recrc(std::vector<uint8_t> v) {
  const uint32_t c = crc32(v.data(), v.size() - 4);
  for (int i = 0; i < 4; ++i) v[v.size() - 4 + i] = static_cast<uint8_t>(c >> (8 * i));
  return v;
}

bool decodes(const std::vector<uint8_t>& v) {
  NetTrialRecord r;
  return decodeNetTrial(v.data(), v.size(), r);
}

bool sameTrialFields(const NetConfig& a, const NetConfig& b) {
  return a.iface == b.iface && a.dhcp == b.dhcp && a.ip == b.ip && a.mask == b.mask &&
         a.gateway == b.gateway && a.dns == b.dns && strcmp(a.ssid, b.ssid) == 0 &&
         strcmp(a.wifiPassword, b.wifiPassword) == 0;
}

}  // namespace

TEST_CASE("encodeNetTrial: the exact layout of a static record") {
  NetTrialRecord r;
  r.state = NetTrialState::Running;
  r.previous = staticNet();
  vdm::copyString(r.previous.ssid, sizeof r.previous.ssid, "ab");
  vdm::copyString(r.previous.wifiPassword, sizeof r.previous.wifiPassword, "xyz");
  r.trialCrc = 0x11223344;
  std::vector<uint8_t> want = {'V', 'D', 'N', 'T', 1, 2, 1, 0};
  addU32(want, 0x3201A8C0);
  addU32(want, 0x00FFFFFF);
  addU32(want, 0x0101A8C0);
  addU32(want, 0x0201A8C0);
  want.push_back(2);
  want.push_back('a');
  want.push_back('b');
  want.push_back(3);
  want.push_back('x');
  want.push_back('y');
  want.push_back('z');
  addU32(want, 0x11223344);
  addU32(want, crc32(want.data(), want.size()));
  CHECK(encode(r) == want);
  // The fields CRC covers exactly the bytes from iface to the password.
  CHECK(netTrialFieldsCrc(r.previous) == crc32(want.data() + 6, want.size() - 6 - 8));
}

TEST_CASE("encodeNetTrial / decodeNetTrial: round trips") {
  NetTrialRecord in;
  in.previous = staticNet();
  in.trialCrc = 0xDEADBEEF;
  std::vector<uint8_t> b = encode(in);
  CHECK(b.size() == 34);
  NetTrialRecord out;
  out.previous.reconnectTimeoutMin = 42;
  REQUIRE(decodeNetTrial(b.data(), b.size(), out));
  CHECK(out.state == NetTrialState::Armed);
  CHECK(sameTrialFields(out.previous, in.previous));
  CHECK(out.previous.reconnectTimeoutMin == 42);  // not a trial field
  CHECK(out.trialCrc == 0xDEADBEEF);

  NetTrialRecord dhcp;
  dhcp.state = NetTrialState::Running;
  dhcp.previous.iface = NetInterface::Auto;
  b = encode(dhcp);
  NetTrialRecord out2;
  out2.previous = staticNet();
  vdm::copyString(out2.previous.ssid, sizeof out2.previous.ssid, "old");
  REQUIRE(decodeNetTrial(b.data(), b.size(), out2));
  CHECK(out2.state == NetTrialState::Running);
  CHECK(sameTrialFields(out2.previous, dhcp.previous));

  NetTrialRecord wifi;
  wifi.previous = wifiNet();
  b = encode(wifi);
  CHECK(b.size() == 130);
  NetTrialRecord out3;
  REQUIRE(decodeNetTrial(b.data(), b.size(), out3));
  CHECK(sameTrialFields(out3.previous, wifi.previous));
  CHECK(strlen(out3.previous.ssid) == 32);
  CHECK(strlen(out3.previous.wifiPassword) == 64);
}

TEST_CASE("encodeNetTrial: capacity") {
  NetTrialRecord r;
  r.previous = wifiNet();
  uint8_t buf[kNetTrialBlobMax];
  CHECK(encodeNetTrial(r, buf, 129) == 0);
  CHECK(encodeNetTrial(r, buf, 130) == 130);
  CHECK(encodeNetTrial(r, nullptr, 130) == 0);
  CHECK(kNetTrialBlobMax == 136);
}

TEST_CASE("decodeNetTrial: rejects") {
  NetTrialRecord r;
  r.previous = staticNet();
  vdm::copyString(r.previous.ssid, sizeof r.previous.ssid, "net");
  vdm::copyString(r.previous.wifiPassword, sizeof r.previous.wifiPassword, "password");
  const std::vector<uint8_t> good = encode(r);
  REQUIRE(decodes(good));
  NetTrialRecord out;
  out.trialCrc = 77;
  CHECK_FALSE(decodeNetTrial(nullptr, good.size(), out));
  for (size_t n = 0; n < good.size(); ++n) {
    INFO(n);
    CHECK_FALSE(decodeNetTrial(good.data(), n, out));
  }
  CHECK(out.trialCrc == 77);  // unchanged
  std::vector<uint8_t> v = good;
  v[0] = 'X';
  CHECK_FALSE(decodes(recrc(v)));
  v = good;
  v[3] = 'X';
  CHECK_FALSE(decodes(recrc(v)));
  v = good;
  v[4] = 2;  // version
  CHECK_FALSE(decodes(recrc(v)));
  v = good;
  v[4] = 0;
  CHECK_FALSE(decodes(recrc(v)));
  for (uint8_t st : {0, 3}) {
    v = good;
    v[5] = st;
    CHECK_FALSE(decodes(recrc(v)));
  }
  v = good;
  v[6] = 3;  // iface
  CHECK_FALSE(decodes(recrc(v)));
  v[6] = 2;
  CHECK(decodes(recrc(v)));
  v = good;
  v[7] = 2;  // dhcp
  CHECK_FALSE(decodes(recrc(v)));
  v[7] = 1;
  CHECK(decodes(recrc(v)));
  v = good;
  v.push_back(0);  // trailing byte
  CHECK_FALSE(decodes(recrc(v)));
  v = good;
  v.pop_back();
  CHECK_FALSE(decodes(v));
  // Every single-bit flip of a valid blob.
  for (size_t i = 0; i < good.size(); ++i) {
    for (int bit = 0; bit < 8; ++bit) {
      v = good;
      v[i] = static_cast<uint8_t>(v[i] ^ (1u << bit));
      INFO(i << ":" << bit);
      CHECK_FALSE(decodes(v));
    }
  }
}

TEST_CASE("decodeNetTrial: ssid and password length limits") {
  NetTrialRecord r;
  r.previous = wifiNet();
  const std::vector<uint8_t> good = encode(r);
  // ssid length 33: one more byte in the blob (still consistent), rejected.
  std::vector<uint8_t> v = good;
  v[24] = 33;
  v.insert(v.begin() + 25, 'S');
  CHECK_FALSE(decodes(recrc(v)));
  // password length 65.
  v = good;
  v[25 + 32] = 65;
  v.insert(v.begin() + 26 + 32, 'p');
  CHECK_FALSE(decodes(recrc(v)));
  // A length byte pointing past the end.
  v = good;
  v[24] = 32;
  v.resize(34 + 10);
  v[24] = 30;
  CHECK_FALSE(decodes(recrc(v)));
  // A shorter ssid with the password length adjusted: consistent and accepted.
  NetTrialRecord s;
  s.previous = staticNet();
  vdm::copyString(s.previous.ssid, sizeof s.previous.ssid, "a");
  const std::vector<uint8_t> one = encode(s);
  CHECK(one.size() == 35);
  CHECK(decodes(one));
}

TEST_CASE("netTrialFieldsCrc changes with every trial field, not with reconnectTimeoutMin") {
  const NetConfig base = staticNet();
  const uint32_t c = netTrialFieldsCrc(base);
  NetConfig n = base;
  n.reconnectTimeoutMin = 0;
  CHECK(netTrialFieldsCrc(n) == c);
  n = base;
  n.iface = NetInterface::Auto;
  CHECK(netTrialFieldsCrc(n) != c);
  n = base;
  n.dhcp = true;
  CHECK(netTrialFieldsCrc(n) != c);
  n = base;
  n.ip ^= 1;
  CHECK(netTrialFieldsCrc(n) != c);
  n = base;
  n.mask ^= 0x80000000u;
  CHECK(netTrialFieldsCrc(n) != c);
  n = base;
  n.gateway ^= 0x00010000u;
  CHECK(netTrialFieldsCrc(n) != c);
  n = base;
  n.dns ^= 0x00000100u;
  CHECK(netTrialFieldsCrc(n) != c);
  n = base;
  vdm::copyString(n.ssid, sizeof n.ssid, "x");
  CHECK(netTrialFieldsCrc(n) != c);
  n = base;
  vdm::copyString(n.wifiPassword, sizeof n.wifiPassword, "x");
  const uint32_t pw = netTrialFieldsCrc(n);
  CHECK(pw != c);
  // "x" as ssid and as password are different settings.
  NetConfig s = base;
  vdm::copyString(s.ssid, sizeof s.ssid, "x");
  CHECK(netTrialFieldsCrc(s) != pw);
}

TEST_CASE("netTrialAtBoot") {
  const NetConfig cur = staticNet();
  CHECK(netTrialAtBoot(nullptr, cur) == NetTrialBoot::None);
  NetTrialRecord r;
  r.previous.dhcp = true;
  r.trialCrc = netTrialFieldsCrc(cur) + 1;
  CHECK(netTrialAtBoot(&r, cur) == NetTrialBoot::Stale);
  r.state = NetTrialState::Running;
  CHECK(netTrialAtBoot(&r, cur) == NetTrialBoot::Stale);
  r.trialCrc = netTrialFieldsCrc(cur);
  CHECK(netTrialAtBoot(&r, cur) == NetTrialBoot::RevertNow);
  r.state = NetTrialState::Armed;
  CHECK(netTrialAtBoot(&r, cur) == NetTrialBoot::Start);
}

TEST_CASE("applyNetTrialFields copies the 8 fields and keeps reconnectTimeoutMin") {
  NetConfig dst;
  dst.reconnectTimeoutMin = 3;
  const NetConfig prev = wifiNet();
  NetConfig p2 = prev;
  p2.ip = 1;
  p2.mask = 2;
  p2.gateway = 3;
  p2.dns = 4;
  p2.dhcp = false;
  p2.reconnectTimeoutMin = 99;
  applyNetTrialFields(dst, p2);
  CHECK(sameTrialFields(dst, p2));
  CHECK(dst.reconnectTimeoutMin == 3);
}

TEST_CASE("formatNetAddress") {
  char out[32];
  CHECK(formatNetAddress(staticNet(), out, sizeof out) == 12);
  CHECK(std::string(out) == "192.168.1.50");
  NetConfig d;
  CHECK(formatNetAddress(d, out, sizeof out) == 4);
  CHECK(std::string(out) == "dhcp");
  CHECK(formatNetAddress(staticNet(), out, 5) == 4);
  CHECK(std::string(out) == "192.");
  CHECK(formatNetAddress(staticNet(), out, 1) == 0);
  CHECK(std::string(out).empty());
  out[0] = 'q';
  CHECK(formatNetAddress(staticNet(), out, 0) == 0);
  CHECK(out[0] == 'q');
  CHECK(formatNetAddress(staticNet(), nullptr, 10) == 0);
}

TEST_CASE("NetTrial: no network within the window -> Revert NoNetwork at 120 s") {
  NetTrial t;
  CHECK_FALSE(t.active());
  CHECK(t.update(false, 0) == NetTrial::Decision::None);
  t.start(0);
  CHECK(t.active());
  CHECK(t.reason() == NetTrialRevert::NotConfirmed);
  CHECK(t.remainingMs(0) == 120000);
  CHECK(t.remainingMs(119999) == 1);
  CHECK(t.upForMs(50000) == 0);
  CHECK(t.update(false, 119999) == NetTrial::Decision::None);
  CHECK(t.update(false, 120000) == NetTrial::Decision::Revert);
  CHECK(t.reason() == NetTrialRevert::NoNetwork);
  CHECK_FALSE(t.active());
  CHECK(t.remainingMs(120000) == 0);
  CHECK(t.update(false, 130000) == NetTrial::Decision::None);  // once
  t.start(200000);
  CHECK(t.reason() == NetTrialRevert::NotConfirmed);
}

TEST_CASE("NetTrial: the window counts from the first network, a later IP loss does not reset it") {
  NetTrial t;
  t.start(0);
  CHECK(t.update(false, 29000) == NetTrial::Decision::None);
  CHECK(t.update(true, 30000) == NetTrial::Decision::None);
  CHECK(t.remainingMs(30000) == 120000);
  CHECK(t.upForMs(30000) == 0);
  CHECK(t.upForMs(40000) == 10000);
  CHECK(t.update(false, 100000) == NetTrial::Decision::None);
  CHECK(t.update(true, 110000) == NetTrial::Decision::None);
  CHECK(t.remainingMs(110000) == 40000);
  CHECK(t.update(false, 149999) == NetTrial::Decision::None);
  CHECK(t.remainingMs(149999) == 1);
  CHECK(t.update(false, 150000) == NetTrial::Decision::Revert);
  CHECK(t.reason() == NetTrialRevert::NotConfirmed);
  CHECK(t.remainingMs(150000) == 0);
}

TEST_CASE("NetTrial: confirm ends the trial once") {
  NetTrial t;
  CHECK_FALSE(t.confirm());
  t.start(1000);
  t.update(true, 2000);
  CHECK(t.confirm());
  CHECK_FALSE(t.active());
  CHECK_FALSE(t.confirm());
  CHECK(t.update(true, 500000) == NetTrial::Decision::None);
  CHECK(t.remainingMs(3000) == 0);
  CHECK(t.upForMs(62000) == 60000);
}

TEST_CASE("NetTrial: start near the 32-bit wrap and a custom window") {
  NetTrial t;
  const uint32_t s = 0xFFFFF000u;
  t.start(s);
  CHECK(t.remainingMs(s + 1000u) == 119000);
  CHECK(t.update(false, s + 119999u) == NetTrial::Decision::None);
  CHECK(t.update(false, s + 120000u) == NetTrial::Decision::Revert);
  NetTrial w(10);
  w.start(5);
  CHECK(w.update(true, 6) == NetTrial::Decision::None);
  CHECK(w.update(true, 15) == NetTrial::Decision::None);
  CHECK(w.update(true, 16) == NetTrial::Decision::Revert);
  CHECK(kNetTrialWindowMs == 120000);
}

TEST_CASE("NetTrialRevert and state numbers") {
  CHECK(static_cast<uint8_t>(NetTrialRevert::NotConfirmed) == 1);
  CHECK(static_cast<uint8_t>(NetTrialRevert::NoNetwork) == 2);
  CHECK(static_cast<uint8_t>(NetTrialRevert::Interrupted) == 3);
  CHECK(static_cast<uint8_t>(NetTrialRevert::User) == 4);
  CHECK(static_cast<uint8_t>(NetTrialState::Armed) == 1);
  CHECK(static_cast<uint8_t>(NetTrialState::Running) == 2);
}
