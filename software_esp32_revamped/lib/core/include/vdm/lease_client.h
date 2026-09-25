// Failsafe lease, ESP side: tells the STM whether the regulator (MQTT/HA) is
// alive (slhbt), keeps the STM's lease timeout and failsafe positions equal
// to the ESP config (glcfg / slcfg / sfspo) on protocol 3, and emulates the
// lease on protocols 1/2 by naming the valves the ESP drives to their
// failsafe position itself. Reports the lease status and its events.
// Hardware-free.
#pragma once

#include <stddef.h>
#include <stdint.h>

#include "vdm/config.h"
#include "vdm/event_log.h"
#include "vdm/failsafe.h"
#include "vdm/link_policy.h"
#include "vdm/stm_codec.h"

namespace vdm {

// What the STM must hold.
struct LeaseConfig {
  uint16_t timeoutMin = kFailsafeTimeoutDefaultMin;
  uint8_t failsafePct[kValveCount] = {kFailsafePctDefault, kFailsafePctDefault,
                                      kFailsafePctDefault, kFailsafePctDefault,
                                      kFailsafePctDefault, kFailsafePctDefault,
                                      kFailsafePctDefault, kFailsafePctDefault,
                                      kFailsafePctDefault, kFailsafePctDefault,
                                      kFailsafePctDefault, kFailsafePctDefault};
};
bool operator==(const LeaseConfig& a, const LeaseConfig& b);
inline bool operator!=(const LeaseConfig& a, const LeaseConfig& b) { return !(a == b); }
// timeoutMin = cfg.failsafe.timeoutMin; failsafePct[v] = cfg.valves[v].failsafePct
// for active valves, kFailsafeHold for inactive ones (the ESP never moves an
// inactive valve).
void effectiveLeaseConfig(const Config& cfg, LeaseConfig& out);

class LeaseClient {
 public:
  static constexpr uint32_t kHeartbeatMs = 60000;       // slhbt period
  static constexpr uint32_t kConfigCheckMs = 600000;    // glcfg compare while synced
  static constexpr uint32_t kConfigRetryMs = 60000;     // after a failed attempt
  static constexpr uint8_t kConfigMaxAttempts = 3;      // then LeaseConfigFailed; retries every kConfigCheckMs
  static constexpr uint32_t kRegulatorEventMs = 60000;  // RegulatorLost after this long without the regulator
  static constexpr uint32_t kLostRequestMs = 10000;     // a handed-out request without completion counts as Timeout
  static constexpr uint32_t kRenewHoldMs = 120000;      // during a failsafe: alive this long before renewing

  // A change re-reads glcfg at once (protocol 3).
  void setConfig(const LeaseConfig& c);
  const LeaseConfig& config() const { return config_; }
  // False while the ESP runs on a default config nobody saved: glcfg is
  // still compared, slcfg/sfspo are never sent. Becoming true re-reads.
  void setConfigTrusted(bool trusted);
  // 0..3 (higher counts as 3). 0 pauses (the mode stays what the last known
  // protocol gave); a change to 3 starts a session.
  void setProtocol(uint8_t proto, uint32_t nowMs);
  // Once per second from the MQTT view. The first call starts the lost
  // timer when the cause is dead (at ESP boot the regulator counts as lost
  // until MQTT reports it alive). While a failsafe is active (STM lease
  // expired or ESP emulation) a dead -> alive change counts only after
  // kRenewHoldMs of continuous life, or at once when commandSeq changed (an
  // MQTT command arrived).
  void setRegulator(RegulatorCause c, uint32_t commandSeq, uint32_t nowMs);
  // New STM session: heartbeat due, glcfg re-read.
  void onStmReboot();
  // Next request, to be queued at Priority::Config; false when nothing is
  // due. One request in flight at most; a heartbeat goes before any config
  // request, config requests only after the session's first heartbeat was
  // answered.
  bool next(uint32_t nowMs, RequestLine& out);
  // Completion of a request from next(); rep is the matched reply for
  // Ok/Rejected, else nullptr. Other requests are ignored.
  void onCompletion(const RequestLine& req, Outcome o, const Reply* rep, uint32_t nowMs);
  // gstax (protocol 3): lease state and mask; a leaseTimeoutMin that drifted
  // from a synced config, or a changed cfgEvents, re-reads glcfg.
  void onStatus(const StmStatus& s, uint32_t nowMs);
  // Valves the ESP drives to their failsafe position (protocols 1/2): those
  // with a failsafePct other than hold once the regulator has been lost for
  // timeoutMin; 0 on protocol 3, with timeout 0 and while the regulator is
  // alive.
  uint16_t emulatedMask(uint32_t nowMs) const;
  LeaseStatus status(uint32_t nowMs) const;
  // Once per second: RegulatorLost/Back, FailsafeActive/Ended,
  // LeaseConfigFailed.
  size_t tick(uint32_t nowMs, Event* out, size_t maxOut);

  // The ESP emulation across ESP software restarts (RTC record).
  struct Snapshot {
    bool lost = false;          // regulator dead, lostElapsedMs meaningful
    bool active = false;        // the ESP emulation drives valves
    uint16_t mask = 0;          // emulatedMask()
    uint32_t lostElapsedMs = 0;
  };
  Snapshot snapshot(uint32_t nowMs) const;
  // Boot, before the first setRegulator(): a lost record continues the lost
  // timer; an active record drives its mask at once (also before the
  // protocol is known) and does not report failsafe_active again.
  void restore(const Snapshot& s, uint32_t nowMs);

 private:
  enum class Kind : uint8_t { None, Heartbeat, Read, Push, Verify };
  LeaseMode mode() const;
  bool emulationActive(uint32_t nowMs) const;
  bool failsafeActive(uint32_t nowMs) const;
  void startSession();
  void reread();
  void configDone(uint32_t nowMs);
  void failAttempt(uint8_t reason, uint32_t nowMs);
  void onConfigReply(const LeaseConfigReply& r, bool verify, uint32_t nowMs);
  uint16_t holdMask() const;

  LeaseConfig config_;
  bool trusted_ = true;
  uint8_t proto_ = 0;
  uint8_t lastProto_ = 0;

  // regulator
  bool regInit_ = false;
  bool effAlive_ = false;
  RegulatorCause cause_ = RegulatorCause::BrokerDown;
  uint32_t lastSeq_ = 0;
  bool lostValid_ = false;
  uint32_t lostSinceMs_ = 0;
  bool aliveSinceValid_ = false;
  uint32_t aliveSinceMs_ = 0;
  bool lostReported_ = false;
  bool backPending_ = false;
  uint32_t backSeconds_ = 0;

  // requests
  Kind inFlight_ = Kind::None;
  RequestLine inFlightReq_;
  uint32_t inFlightAtMs_ = 0;
  bool hbSent_ = false;       // in this session
  bool hbOk_ = false;         // a heartbeat of this session was answered
  uint32_t hbDoneMs_ = 0;    // completion of the last heartbeat (meaningful once hbSent_)
  bool lastSentAlive_ = false;

  // config sync
  Kind cfgStep_ = Kind::Read;
  bool cfgDueNow_ = true;     // else due cfgWaitMs_ after cfgWaitFromMs_
  uint32_t cfgWaitFromMs_ = 0;
  uint32_t cfgWaitMs_ = 0;
  bool pushTimeout_ = false;
  bool pushAll_ = false;
  uint16_t pushMask_ = 0;
  bool synced_ = false;
  bool configFailed_ = false;
  uint8_t attempts_ = 0;
  bool failEventPending_ = false;
  uint8_t failReason_ = 0;

  // STM lease (protocol 3)
  LeaseState stmLease_ = LeaseState::Off;
  uint32_t stmRemainS_ = 0;
  uint32_t stmRemainAtMs_ = 0;
  uint16_t stmMask_ = 0;
  bool haveCfgEvents_ = false;
  uint32_t cfgEvents_ = 0;

  // failsafe events
  bool stmFsReported_ = false;
  uint32_t stmFsSinceMs_ = 0;
  bool emuReported_ = false;
  uint32_t emuSinceMs_ = 0;
  bool restoredActive_ = false;
  uint16_t restoredMask_ = 0;
};

// RTC record of a LeaseClient::Snapshot: "VDLE" (4), lost (1), active (1),
// mask u16 LE (2), lostElapsedMs u32 LE (4), CRC u16 LE = low 16 bits of
// vdm::crc32 over bytes 0..11 (2).
constexpr size_t kLeaseRecordSize = 14;
size_t encodeLeaseRecord(const LeaseClient::Snapshot& s, uint8_t (&out)[kLeaseRecordSize]);
// false (out = {}) for a wrong length, magic or CRC, or flag bytes above 1.
bool decodeLeaseRecord(const uint8_t* data, size_t len, LeaseClient::Snapshot& out);

}  // namespace vdm
