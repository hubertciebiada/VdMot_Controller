// Network trial: a change of the settings that decide whether and where the
// device is reachable runs on trial after the restart and is reverted unless
// the user confirms it from the new address. Record format (NVS "netTrial"),
// boot rule and the trial window. Hardware-free.
#pragma once

#include <stddef.h>
#include <stdint.h>

#include "vdm/config.h"

namespace vdm {

constexpr uint32_t kNetTrialWindowMs = 120000;
constexpr size_t kNetTrialBlobMax = 136;

enum class NetTrialState : uint8_t { Armed = 1, Running = 2 };
struct NetTrialRecord {
  NetTrialState state = NetTrialState::Armed;
  NetConfig previous;     // only the trial fields are encoded; restored on revert
  uint32_t trialCrc = 0;  // netTrialFieldsCrc() of the settings on trial
};

// CRC-32 (vdm::crc32) over the trial fields of `n` (iface, dhcp, ip, mask,
// gateway, dns, ssid, wifiPassword), encoded as in the blob.
uint32_t netTrialFieldsCrc(const NetConfig& n);
// Blob: "VDNT", u8 version 1, u8 state, u8 iface, u8 dhcp, u32 ip, mask,
// gateway, dns (LE, legacy layout), u8 len + ssid, u8 len + wifiPassword,
// u32 trialCrc, u32 CRC-32 over all bytes before it. Max 130 bytes. Returns
// the bytes written, 0 when cap is too small.
size_t encodeNetTrial(const NetTrialRecord& r, uint8_t* out, size_t cap);
// false: short, magic, version != 1, state not 1/2, iface > 2, dhcp > 1,
// ssid > 32, password > 64, length mismatch, CRC. On false `out` is unchanged.
// Only the trial fields of out.previous are written.
bool decodeNetTrial(const uint8_t* data, size_t len, NetTrialRecord& out);

enum class NetTrialBoot : uint8_t { None, Stale, Start, RevertNow };
// null -> None; r->trialCrc != netTrialFieldsCrc(current) -> Stale (the stored
// config is no longer the one on trial: erase, no action); Armed -> Start;
// Running (the previous boot ended during the trial: crash, power loss,
// restart) -> RevertNow.
NetTrialBoot netTrialAtBoot(const NetTrialRecord* r, const NetConfig& current);

// Revert: copies iface, dhcp, ip, mask, gateway, dns, ssid, wifiPassword of
// `prev` into `dst`; reconnectTimeoutMin stays.
void applyNetTrialFields(NetConfig& dst, const NetConfig& prev);
// "192.168.1.50" for a static configuration, "dhcp" otherwise (event texts).
// Returns the length (NUL-terminated, truncated to cap - 1; 0 for cap 0).
size_t formatNetAddress(const NetConfig& n, char* out, size_t cap);

enum class NetTrialRevert : uint8_t { NotConfirmed = 1, NoNetwork = 2, Interrupted = 3, User = 4 };

class NetTrial {
 public:
  enum class Decision : uint8_t { None, Revert };
  explicit NetTrial(uint32_t windowMs = kNetTrialWindowMs) : windowMs_(windowMs) {}
  void start(uint32_t nowMs);  // boot with an Armed record
  bool active() const { return active_; }
  // Every second: Revert once when the network was not up within windowMs of
  // start() (NoNetwork), or windowMs passed since it first came up without
  // confirm() (NotConfirmed). The window is not restarted by a later IP loss.
  Decision update(bool netUp, uint32_t nowMs);
  NetTrialRevert reason() const { return reason_; }
  bool confirm();                               // true when a trial was active (then inactive)
  uint32_t remainingMs(uint32_t nowMs) const;   // 0 when inactive
  uint32_t upForMs(uint32_t nowMs) const;       // since the network came up, 0 before

 private:
  uint32_t windowMs_;
  bool active_ = false;
  bool up_ = false;
  uint32_t startMs_ = 0;
  uint32_t upMs_ = 0;
  NetTrialRevert reason_ = NetTrialRevert::NotConfirmed;
};

}  // namespace vdm
