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
  n.dhcp = false;
  n.ip = 0x3201A8C0;       // 192.168.1.50
  n.mask = 0x00FFFFFF;     // 255.255.255.0
  n.gateway = 0x0101A8C0;  // 192.168.1.1
  n.dns = 0x0201A8C0;
  n.reconnectTimeoutMin = 7;
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

// A record in the layout of the firmware with WiFi (the one kept here): the
// interface choice `iface`, the previous settings of `n`, an ssid and a password.
std::vector<uint8_t> wifiRecord(NetTrialState st, uint8_t iface, const NetConfig& n,
                                const std::string& ssid, const std::string& pwd,
                                uint32_t trialCrc) {
  std::vector<uint8_t> v = {'V', 'D', 'N', 'T', 1, static_cast<uint8_t>(st), iface,
                            static_cast<uint8_t>(n.dhcp ? 1 : 0)};
  addU32(v, n.ip);
  addU32(v, n.mask);
  addU32(v, n.gateway);
  addU32(v, n.dns);
  v.push_back(static_cast<uint8_t>(ssid.size()));
  v.insert(v.end(), ssid.begin(), ssid.end());
  v.push_back(static_cast<uint8_t>(pwd.size()));
  v.insert(v.end(), pwd.begin(), pwd.end());
  addU32(v, trialCrc);
  addU32(v, crc32(v.data(), v.size()));
  return v;
}

// netTrialFieldsCrc() as the firmware with WiFi computes it: over iface .. password.
uint32_t wifiFieldsCrc(uint8_t iface, const NetConfig& n, const std::string& ssid,
                       const std::string& pwd) {
  const std::vector<uint8_t> r = wifiRecord(NetTrialState::Armed, iface, n, ssid, pwd, 0);
  return crc32(r.data() + 6, r.size() - 6 - 8);
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
  return a.dhcp == b.dhcp && a.ip == b.ip && a.mask == b.mask && a.gateway == b.gateway &&
         a.dns == b.dns;
}

}  // namespace

TEST_CASE("encodeNetTrial: the layout of the firmware with WiFi, its fields neutral") {
  NetTrialRecord r;
  r.state = NetTrialState::Running;
  r.previous = staticNet();
  r.trialCrc = 0x11223344;
  const std::vector<uint8_t> b = encode(r);
  std::vector<uint8_t> want = {'V', 'D', 'N', 'T', 1, 2, 0, 0};
  addU32(want, 0x3201A8C0);
  addU32(want, 0x00FFFFFF);
  addU32(want, 0x0101A8C0);
  addU32(want, 0x0201A8C0);
  want.push_back(0);  // ssid ""
  want.push_back(0);  // password ""
  addU32(want, 0x11223344);
  addU32(want, crc32(want.data(), want.size()));
  CHECK(b == want);
  CHECK(b == wifiRecord(NetTrialState::Running, 0, staticNet(), "", "", 0x11223344));
  // The fields CRC covers the bytes from iface to the password, as the firmware with WiFi
  // computes it for a config saved here (iface auto, no WiFi).
  CHECK(netTrialFieldsCrc(r.previous) == crc32(want.data() + 6, want.size() - 6 - 8));
  CHECK(netTrialFieldsCrc(r.previous) == wifiFieldsCrc(0, r.previous, "", ""));
  NetConfig dhcp;
  CHECK(netTrialFieldsCrc(dhcp) == wifiFieldsCrc(0, dhcp, "", ""));
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
  b = encode(dhcp);
  NetTrialRecord out2;
  out2.previous = staticNet();
  REQUIRE(decodeNetTrial(b.data(), b.size(), out2));
  CHECK(out2.state == NetTrialState::Running);
  CHECK(sameTrialFields(out2.previous, dhcp.previous));
}

TEST_CASE("decodeNetTrial: a record of the firmware with WiFi is read, its WiFi skipped") {
  // The longest one: WiFi only, a 32-byte ssid and a 64-byte password.
  const std::vector<uint8_t> w = wifiRecord(NetTrialState::Running, 2, staticNet(),
                                            std::string(32, 'S'), std::string(64, 'p'), 0xCAFE);
  REQUIRE(w.size() == 130);
  NetTrialRecord out;
  REQUIRE(decodeNetTrial(w.data(), w.size(), out));
  CHECK(out.state == NetTrialState::Running);
  CHECK(sameTrialFields(out.previous, staticNet()));
  CHECK(out.trialCrc == 0xCAFE);
  // Any interface byte, and lengths that add up, are skipped.
  NetConfig d;
  const struct {
    uint8_t iface;
    const char* ssid;
    const char* pwd;
  } rows[] = {{1, "", ""}, {7, "home", ""}, {0, "", "x"}, {255, "a", "12345678"}};
  for (const auto& row : rows) {
    CAPTURE(row.iface);
    const std::vector<uint8_t> v = wifiRecord(NetTrialState::Armed, row.iface, d, row.ssid, row.pwd, 5);
    NetTrialRecord o;
    o.previous = staticNet();
    REQUIRE(decodeNetTrial(v.data(), v.size(), o));
    CHECK(o.state == NetTrialState::Armed);
    CHECK(sameTrialFields(o.previous, d));
    CHECK(o.trialCrc == 5);
  }
}

TEST_CASE("encodeNetTrial: capacity") {
  NetTrialRecord r;
  r.previous = staticNet();
  uint8_t buf[kNetTrialBlobMax];
  CHECK(encodeNetTrial(r, buf, 33) == 0);
  CHECK(encodeNetTrial(r, buf, 34) == 34);
  CHECK(encodeNetTrial(r, nullptr, 34) == 0);
  CHECK(kNetTrialBlobMax == 136);  // a record of the firmware with WiFi has up to 130 bytes
}

TEST_CASE("decodeNetTrial: rejects") {
  const std::vector<uint8_t> good =
      wifiRecord(NetTrialState::Armed, 1, staticNet(), "net", "password", 77777);
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

TEST_CASE("decodeNetTrial: the ssid and password lengths must add up to the blob") {
  NetTrialRecord r;
  r.previous = staticNet();
  const std::vector<uint8_t> good = encode(r);  // 34 bytes, both lengths 0
  // An ssid length that puts the password length at the blob end, or past it: nothing is read
  // beyond the blob (exact-size buffers).
  for (uint8_t ssid : {9, 10, 255}) {
    std::vector<uint8_t> v = good;
    v[24] = ssid;
    const std::vector<uint8_t> exact = recrc(v);
    INFO(static_cast<int>(ssid));
    CHECK_FALSE(decodes(exact));
  }
  // One byte short of the end: the password length is read, the total does not add up.
  std::vector<uint8_t> v = good;
  v[24] = 8;
  CHECK_FALSE(decodes(recrc(v)));
  // A password length that does not fit.
  v = good;
  v[25] = 1;
  CHECK_FALSE(decodes(recrc(v)));
  // Consistent lengths are accepted, whatever they are.
  const std::vector<uint8_t> one = wifiRecord(NetTrialState::Armed, 0, staticNet(), "a", "", 1);
  CHECK(one.size() == 35);
  CHECK(decodes(one));
  const std::vector<uint8_t> big =
      wifiRecord(NetTrialState::Armed, 0, staticNet(), std::string(40, 's'), std::string(62, 'p'), 1);
  CHECK(big.size() == 136);
  CHECK(decodes(big));
}

TEST_CASE("netTrialFieldsCrc changes with every trial field, not with reconnectTimeoutMin") {
  const NetConfig base = staticNet();
  const uint32_t c = netTrialFieldsCrc(base);
  NetConfig n = base;
  n.reconnectTimeoutMin = 0;
  CHECK(netTrialFieldsCrc(n) == c);
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

TEST_CASE("netTrialAtBoot: a record of the firmware with WiFi (an upgrade during a trial)") {
  const NetConfig cur = staticNet();
  // Settings on trial with the interface auto and no WiFi: the trial goes on here.
  std::vector<uint8_t> v = wifiRecord(NetTrialState::Running, 2, NetConfig{}, "home", "password",
                                      wifiFieldsCrc(0, cur, "", ""));
  NetTrialRecord r;
  REQUIRE(decodeNetTrial(v.data(), v.size(), r));
  CHECK(netTrialAtBoot(&r, cur) == NetTrialBoot::RevertNow);
  CHECK(r.previous.dhcp);
  // An interface choice or WiFi on trial: gone here, so the record is stale.
  const uint32_t gone[] = {wifiFieldsCrc(1, cur, "", ""), wifiFieldsCrc(2, cur, "home", ""),
                           wifiFieldsCrc(0, cur, "", "password")};
  for (uint32_t crc : gone) {
    v = wifiRecord(NetTrialState::Running, 0, NetConfig{}, "", "", crc);
    REQUIRE(decodeNetTrial(v.data(), v.size(), r));
    CHECK(netTrialAtBoot(&r, cur) == NetTrialBoot::Stale);
  }
}

TEST_CASE("applyNetTrialFields copies the 5 fields and keeps reconnectTimeoutMin") {
  NetConfig dst;
  dst.reconnectTimeoutMin = 3;
  NetConfig p2;
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
