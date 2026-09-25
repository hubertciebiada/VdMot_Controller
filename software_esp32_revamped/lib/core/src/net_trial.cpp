#include "vdm/net_trial.h"

#include <string.h>

#include "vdm/common.h"

namespace vdm {

namespace {

constexpr uint8_t kMagic[4] = {'V', 'D', 'N', 'T'};
constexpr uint8_t kVersion = 1;
constexpr size_t kSsidMax = sizeof(NetConfig::ssid) - 1;              // 32
constexpr size_t kPasswordMax = sizeof(NetConfig::wifiPassword) - 1;  // 64
// magic, version, state, iface, dhcp, 4 addresses, 2 lengths, trialCrc, crc
constexpr size_t kFixedBytes = 4 + 1 + 1 + 1 + 1 + 16 + 1 + 1 + 4 + 4;

void putU32(uint8_t* p, uint32_t v) {
  p[0] = static_cast<uint8_t>(v);
  p[1] = static_cast<uint8_t>(v >> 8);
  p[2] = static_cast<uint8_t>(v >> 16);
  p[3] = static_cast<uint8_t>(v >> 24);
}

uint32_t getU32(const uint8_t* p) {
  return static_cast<uint32_t>(p[0]) | (static_cast<uint32_t>(p[1]) << 8) |
         (static_cast<uint32_t>(p[2]) << 16) | (static_cast<uint32_t>(p[3]) << 24);
}

// Bounded strlen (the arrays are NUL-terminated by construction; this keeps
// a corrupt struct from reading past them).
size_t textLen(const char* s, size_t max) {
  size_t n = 0;
  while (n < max && s[n] != '\0') ++n;
  return n;
}

// The trial fields (iface .. password) as in the blob; returns the bytes.
size_t putFields(const NetConfig& n, uint8_t* p) {
  size_t at = 0;
  p[at++] = static_cast<uint8_t>(n.iface);
  p[at++] = n.dhcp ? 1 : 0;
  putU32(p + at, n.ip);
  putU32(p + at + 4, n.mask);
  putU32(p + at + 8, n.gateway);
  putU32(p + at + 12, n.dns);
  at += 16;
  const size_t ssid = textLen(n.ssid, kSsidMax);
  p[at++] = static_cast<uint8_t>(ssid);
  memcpy(p + at, n.ssid, ssid);
  at += ssid;
  const size_t pass = textLen(n.wifiPassword, kPasswordMax);
  p[at++] = static_cast<uint8_t>(pass);
  memcpy(p + at, n.wifiPassword, pass);
  at += pass;
  return at;
}

}  // namespace

uint32_t netTrialFieldsCrc(const NetConfig& n) {
  uint8_t buf[kNetTrialBlobMax];
  return crc32(buf, putFields(n, buf));
}

size_t encodeNetTrial(const NetTrialRecord& r, uint8_t* out, size_t cap) {
  uint8_t buf[kNetTrialBlobMax];
  memcpy(buf, kMagic, 4);
  buf[4] = kVersion;
  buf[5] = static_cast<uint8_t>(r.state);
  size_t len = 6 + putFields(r.previous, buf + 6);
  putU32(buf + len, r.trialCrc);
  len += 4;
  putU32(buf + len, crc32(buf, len));
  len += 4;
  if (out == nullptr || cap < len) return 0;
  memcpy(out, buf, len);
  return len;
}

bool decodeNetTrial(const uint8_t* data, size_t len, NetTrialRecord& out) {
  if (data == nullptr || len < kFixedBytes) return false;
  if (memcmp(data, kMagic, 4) != 0 || data[4] != kVersion) return false;
  const uint8_t state = data[5];
  if (state != static_cast<uint8_t>(NetTrialState::Armed) &&
      state != static_cast<uint8_t>(NetTrialState::Running)) {
    return false;
  }
  if (data[6] > static_cast<uint8_t>(NetInterface::Wifi) || data[7] > 1) return false;
  const size_t ssid = data[24];
  if (ssid > kSsidMax || 25 + ssid >= len) return false;
  const size_t pass = data[25 + ssid];
  if (pass > kPasswordMax || len != kFixedBytes + ssid + pass) return false;
  const size_t crcAt = len - 4;
  if (crc32(data, crcAt) != getU32(data + crcAt)) return false;
  out.state = static_cast<NetTrialState>(state);
  NetConfig& n = out.previous;
  n.iface = static_cast<NetInterface>(data[6]);
  n.dhcp = data[7] != 0;
  n.ip = getU32(data + 8);
  n.mask = getU32(data + 12);
  n.gateway = getU32(data + 16);
  n.dns = getU32(data + 20);
  memcpy(n.ssid, data + 25, ssid);
  n.ssid[ssid] = '\0';
  memcpy(n.wifiPassword, data + 26 + ssid, pass);
  n.wifiPassword[pass] = '\0';
  out.trialCrc = getU32(data + crcAt - 4);
  return true;
}

NetTrialBoot netTrialAtBoot(const NetTrialRecord* r, const NetConfig& current) {
  if (r == nullptr) return NetTrialBoot::None;
  if (r->trialCrc != netTrialFieldsCrc(current)) return NetTrialBoot::Stale;
  return r->state == NetTrialState::Running ? NetTrialBoot::RevertNow : NetTrialBoot::Start;
}

void applyNetTrialFields(NetConfig& dst, const NetConfig& prev) {
  const uint8_t keep = dst.reconnectTimeoutMin;
  dst = prev;
  dst.reconnectTimeoutMin = keep;
}

size_t formatNetAddress(const NetConfig& n, char* out, size_t cap) {
  if (out == nullptr || cap == 0) return 0;
  char ip[16] = "dhcp";
  if (!n.dhcp) formatIpv4(n.ip, ip, sizeof ip);
  copyString(out, cap, ip);
  return strlen(out);
}

void NetTrial::start(uint32_t nowMs) {
  active_ = true;
  up_ = false;
  startMs_ = nowMs;
  reason_ = NetTrialRevert::NotConfirmed;
}

NetTrial::Decision NetTrial::update(bool netUp, uint32_t nowMs) {
  if (!active_) return Decision::None;
  if (netUp && !up_) {
    up_ = true;
    upMs_ = nowMs;
  }
  if (up_) {
    if (elapsedMs(nowMs, upMs_) < windowMs_) return Decision::None;
    reason_ = NetTrialRevert::NotConfirmed;
  } else {
    if (elapsedMs(nowMs, startMs_) < windowMs_) return Decision::None;
    reason_ = NetTrialRevert::NoNetwork;
  }
  active_ = false;
  return Decision::Revert;
}

bool NetTrial::confirm() {
  if (!active_) return false;
  active_ = false;
  return true;
}

uint32_t NetTrial::remainingMs(uint32_t nowMs) const {
  if (!active_) return 0;
  const uint32_t e = elapsedMs(nowMs, up_ ? upMs_ : startMs_);
  return e >= windowMs_ ? 0 : windowMs_ - e;
}

uint32_t NetTrial::upForMs(uint32_t nowMs) const { return up_ ? elapsedMs(nowMs, upMs_) : 0; }

}  // namespace vdm
