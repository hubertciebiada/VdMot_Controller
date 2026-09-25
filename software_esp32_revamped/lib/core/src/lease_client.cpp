#include "vdm/lease_client.h"

#include <string.h>

namespace vdm {

namespace {

constexpr uint8_t kReasonNoReply = 1;
constexpr uint8_t kReasonRejected = 2;
constexpr uint8_t kReasonDiffers = 3;
constexpr int32_t kSourceStm = 1;
constexpr int32_t kSourceEsp = 2;
constexpr uint8_t kLeaseMagic[4] = {'V', 'D', 'L', 'E'};

bool sameAsStm(const LeaseConfig& c, const LeaseConfigReply& r) {
  if (c.timeoutMin != r.timeoutMin) return false;
  for (uint8_t v = 0; v < kValveCount; ++v) {
    if (c.failsafePct[v] != r.failsafePct[v]) return false;
  }
  return true;
}

// Bounded event sink for one call.
class Sink {
 public:
  Sink(Event* out, size_t maxOut) : out_(out), max_(out == nullptr ? 0 : maxOut) {}
  void add(EventCode code, int32_t a1 = 0, int32_t a2 = 0) {
    if (n_ < max_) out_[n_++] = makeEvent(code, eventDefaultSeverity(code), kNoValve, a1, a2, "");
  }
  size_t count() const { return n_; }

 private:
  Event* out_;
  size_t max_;
  size_t n_ = 0;
};

}  // namespace

bool operator==(const LeaseConfig& a, const LeaseConfig& b) {
  if (a.timeoutMin != b.timeoutMin) return false;
  for (uint8_t v = 0; v < kValveCount; ++v) {
    if (a.failsafePct[v] != b.failsafePct[v]) return false;
  }
  return true;
}

void effectiveLeaseConfig(const Config& cfg, LeaseConfig& out) {
  out.timeoutMin = cfg.failsafe.timeoutMin;
  for (uint8_t v = 0; v < kValveCount; ++v) {
    out.failsafePct[v] = cfg.valves[v].active ? cfg.valves[v].failsafePct : kFailsafeHold;
  }
}

// ---------------------------------------------------------------- inputs

void LeaseClient::setConfig(const LeaseConfig& c) {
  if (c == config_) return;
  config_ = c;
  attempts_ = 0;
  reread();
}

void LeaseClient::setConfigTrusted(bool trusted) {
  if (trusted == trusted_) return;
  trusted_ = trusted;
  if (trusted) reread();
}

void LeaseClient::setProtocol(uint8_t proto, uint32_t nowMs) {
  (void)nowMs;
  if (proto > 3) proto = 3;
  if (proto == proto_) return;
  proto_ = proto;
  if (proto != 0) lastProto_ = proto;
  inFlight_ = Kind::None;
  if (proto == 3) startSession();
}

void LeaseClient::startSession() {
  inFlight_ = Kind::None;
  hbSent_ = false;
  hbOk_ = false;
  haveCfgEvents_ = false;
  attempts_ = 0;
  reread();
}

void LeaseClient::onStmReboot() { startSession(); }

void LeaseClient::reread() {
  synced_ = false;
  cfgStep_ = Kind::Read;
  cfgDueNow_ = true;
  // The push flags are read in the Push step only; onConfigReply() sets them before it.
  // A config request in flight belongs to the old round: its result is dropped.
  if (inFlight_ != Kind::Heartbeat) inFlight_ = Kind::None;
}

void LeaseClient::setRegulator(RegulatorCause c, uint32_t commandSeq, uint32_t nowMs) {
  const bool seqChanged = regInit_ && commandSeq != lastSeq_;
  lastSeq_ = commandSeq;
  if (c != RegulatorCause::Alive) {
    aliveSinceValid_ = false;
    cause_ = c;
    if (effAlive_ || !lostValid_) {
      lostValid_ = true;
      lostSinceMs_ = nowMs;
      lostReported_ = false;
    }
    effAlive_ = false;
    regInit_ = true;
    return;
  }
  regInit_ = true;
  if (effAlive_) return;
  if (!aliveSinceValid_) {
    aliveSinceValid_ = true;
    aliveSinceMs_ = nowMs;
  }
  if (failsafeActive(nowMs) && !seqChanged &&
      elapsedMs(nowMs, aliveSinceMs_) < kRenewHoldMs) {
    return;  // a short blip does not end a failsafe
  }
  if (lostReported_) {
    backPending_ = true;
    backSeconds_ = elapsedMs(nowMs, lostSinceMs_) / 1000;
  }
  // lostReported_ may stay set: the next loss (effAlive_) clears it before tick() reads it
  effAlive_ = true;
  cause_ = RegulatorCause::Alive;
}

void LeaseClient::onStatus(const StmStatus& s, uint32_t nowMs) {
  if (!s.v3) return;
  stmLease_ = s.lease;
  stmRemainS_ = s.leaseRemainS;
  stmRemainAtMs_ = nowMs;
  stmMask_ = s.failsafeMask;
  const bool drift = synced_ && s.leaseTimeoutMin != config_.timeoutMin;
  const bool repaired = haveCfgEvents_ && s.cfgEvents != cfgEvents_;
  haveCfgEvents_ = true;
  cfgEvents_ = s.cfgEvents;
  if (drift || repaired) reread();
}

// ---------------------------------------------------------------- state

LeaseMode LeaseClient::mode() const {
  const uint8_t p = proto_ != 0 ? proto_ : lastProto_;
  if (p == 3) return LeaseMode::Stm;
  if (p == 0) return restoredActive_ ? LeaseMode::Emulated : LeaseMode::None;
  return config_.timeoutMin > 0 ? LeaseMode::Emulated : LeaseMode::None;
}

bool LeaseClient::emulationActive(uint32_t nowMs) const {
  if (proto_ == 0 && lastProto_ == 0) return restoredActive_;
  if (mode() != LeaseMode::Emulated || effAlive_ || !lostValid_) return false;
  return elapsedMs(nowMs, lostSinceMs_) >= uint32_t{config_.timeoutMin} * 60000u;
}

bool LeaseClient::failsafeActive(uint32_t nowMs) const {
  if (mode() == LeaseMode::Stm) return stmLease_ == LeaseState::Expired;
  return emulationActive(nowMs);
}

uint16_t LeaseClient::holdMask() const {
  uint16_t m = 0;
  for (uint8_t v = 0; v < kValveCount; ++v) {
    if (config_.failsafePct[v] != kFailsafeHold) m = static_cast<uint16_t>(m | (1u << v));
  }
  return m;
}

uint16_t LeaseClient::emulatedMask(uint32_t nowMs) const {
  if (!emulationActive(nowMs)) return 0;
  if (proto_ == 0 && lastProto_ == 0) return restoredMask_;
  return holdMask();
}

LeaseStatus LeaseClient::status(uint32_t nowMs) const {
  LeaseStatus s;
  s.mode = mode();
  s.timeoutMin = config_.timeoutMin;
  s.regulator = cause_;
  s.regulatorLostS = !effAlive_ && lostValid_ ? elapsedMs(nowMs, lostSinceMs_) / 1000 : 0;
  s.configSynced = synced_;
  s.configFailed = configFailed_;
  s.configTrusted = trusted_;
  if (s.mode == LeaseMode::Stm) {
    s.state = stmLease_;
    s.failsafeMask = stmMask_;
    if (stmLease_ == LeaseState::Running) {
      const uint32_t aged = elapsedMs(nowMs, stmRemainAtMs_) / 1000;
      s.remainS = stmRemainS_ > aged ? stmRemainS_ - aged : 0;
    }
  } else if (s.mode == LeaseMode::Emulated) {
    const bool active = emulationActive(nowMs);
    s.state = active ? LeaseState::Expired : LeaseState::Running;
    s.failsafeMask = emulatedMask(nowMs);
    const uint32_t total = uint32_t{config_.timeoutMin} * 60u;
    if (!active) s.remainS = total > s.regulatorLostS ? total - s.regulatorLostS : 0;
  }
  return s;
}

// ---------------------------------------------------------------- requests

bool LeaseClient::next(uint32_t nowMs, RequestLine& out) {
  out = RequestLine{};
  if (proto_ != 3) return false;
  if (inFlight_ != Kind::None) {
    if (elapsedMs(nowMs, inFlightAtMs_) < kLostRequestMs) return false;
    const RequestLine lost = inFlightReq_;
    onCompletion(lost, Outcome::Timeout, nullptr, nowMs);
  }
  const bool hbDue = !hbSent_ || effAlive_ != lastSentAlive_ ||
                     elapsedMs(nowMs, hbDoneMs_) >= kHeartbeatMs;
  if (hbDue) {
    buildHeartbeat(effAlive_, out);
    lastSentAlive_ = effAlive_;
    hbSent_ = true;
    inFlight_ = Kind::Heartbeat;
  } else {
    if (!hbOk_) return false;
    if (!cfgDueNow_ && elapsedMs(nowMs, cfgWaitFromMs_) < cfgWaitMs_) return false;
    if (cfgStep_ == Kind::Push && pushTimeout_) {
      buildSetLeaseTimeout(config_.timeoutMin, out);
    } else if (cfgStep_ == Kind::Push && pushAll_) {
      buildSetFailsafe(kAllValves, config_.failsafePct[0], out);
    } else if (cfgStep_ == Kind::Push && pushMask_ != 0) {
      const uint8_t v = static_cast<uint8_t>(__builtin_ctz(pushMask_));
      buildSetFailsafe(v, config_.failsafePct[v], out);
    } else {
      if (cfgStep_ == Kind::Push) cfgStep_ = Kind::Verify;
      buildGetLeaseConfig(out);
    }
    inFlight_ = cfgStep_;
  }
  inFlightReq_ = out;
  inFlightAtMs_ = nowMs;
  return true;
}

void LeaseClient::configDone(uint32_t nowMs) {
  synced_ = true;
  attempts_ = 0;
  configFailed_ = false;
  cfgStep_ = Kind::Read;
  cfgDueNow_ = false;
  cfgWaitFromMs_ = nowMs;
  cfgWaitMs_ = kConfigCheckMs;
}

void LeaseClient::failAttempt(uint8_t reason, uint32_t nowMs) {
  synced_ = false;
  if (attempts_ < UINT8_MAX) ++attempts_;
  const bool exhausted = attempts_ >= kConfigMaxAttempts;
  if (exhausted && !configFailed_) {
    configFailed_ = true;
    failEventPending_ = true;
    failReason_ = reason;
  }
  cfgStep_ = Kind::Read;
  cfgDueNow_ = false;
  cfgWaitFromMs_ = nowMs;
  cfgWaitMs_ = exhausted ? kConfigCheckMs : kConfigRetryMs;
}

void LeaseClient::onConfigReply(const LeaseConfigReply& r, bool verify, uint32_t nowMs) {
  if (sameAsStm(config_, r)) {
    configDone(nowMs);
    return;
  }
  if (verify) {
    failAttempt(kReasonDiffers, nowMs);
    return;
  }
  synced_ = false;
  if (!trusted_) {
    // Defaults nobody saved must not overwrite what the STM holds.
    cfgDueNow_ = false;
    cfgWaitFromMs_ = nowMs;
    cfgWaitMs_ = kConfigCheckMs;
    return;
  }
  pushTimeout_ = r.timeoutMin != config_.timeoutMin;
  uint16_t diff = 0;
  bool allEqual = true;
  for (uint8_t v = 0; v < kValveCount; ++v) {
    if (r.failsafePct[v] != config_.failsafePct[v]) diff = static_cast<uint16_t>(diff | (1u << v));
    if (config_.failsafePct[v] != config_.failsafePct[0]) allEqual = false;
  }
  // One sfspo 255 when every valve wants the same value and several differ.
  pushAll_ = allEqual && __builtin_popcount(diff) >= 2;
  pushMask_ = pushAll_ ? 0 : diff;
  cfgStep_ = Kind::Push;
  cfgDueNow_ = true;
}

void LeaseClient::onCompletion(const RequestLine& req, Outcome o, const Reply* rep,
                               uint32_t nowMs) {
  if (inFlight_ == Kind::None || req.cmd != inFlightReq_.cmd || req.len != inFlightReq_.len ||
      memcmp(req.text, inFlightReq_.text, req.len) != 0) {
    return;
  }
  const Kind kind = inFlight_;
  inFlight_ = Kind::None;
  const bool ok = o == Outcome::Ok && rep != nullptr;
  if (kind == Kind::Heartbeat) {
    hbDoneMs_ = nowMs;
    if (!ok) return;
    hbOk_ = true;
    stmLease_ = rep->heartbeat.lease;
    stmRemainS_ = rep->heartbeat.remainS;
    stmRemainAtMs_ = nowMs;
    return;
  }
  if (kind == Kind::Push) {
    if (!ok) {
      failAttempt(o == Outcome::Rejected ? kReasonRejected : kReasonNoReply, nowMs);
      return;
    }
    if (req.cmd == Cmd::Slcfg) {
      pushTimeout_ = false;
    } else if (req.valve == kAllValves) {
      pushAll_ = false;
      pushMask_ = 0;
    } else {
      pushMask_ = static_cast<uint16_t>(pushMask_ & ~(1u << req.valve));
    }
    return;
  }
  if (!ok) {
    failAttempt(kReasonNoReply, nowMs);
    return;
  }
  onConfigReply(rep->leaseConfig, kind == Kind::Verify, nowMs);
}

// ---------------------------------------------------------------- events

size_t LeaseClient::tick(uint32_t nowMs, Event* out, size_t maxOut) {
  Sink sink(out, maxOut);
  if (!effAlive_ && lostValid_ && !lostReported_ &&
      elapsedMs(nowMs, lostSinceMs_) >= kRegulatorEventMs) {
    lostReported_ = true;
    sink.add(EventCode::RegulatorLost, static_cast<int32_t>(cause_));
  }
  if (backPending_) {
    backPending_ = false;
    sink.add(EventCode::RegulatorBack, static_cast<int32_t>(backSeconds_));
  }
  const bool stmActive = mode() == LeaseMode::Stm && stmLease_ == LeaseState::Expired;
  if (stmActive && !stmFsReported_) {
    stmFsReported_ = true;
    stmFsSinceMs_ = nowMs;
    sink.add(EventCode::FailsafeActive, stmMask_, kSourceStm);
  } else if (!stmActive && stmFsReported_) {
    stmFsReported_ = false;
    sink.add(EventCode::FailsafeEnded, static_cast<int32_t>(elapsedMs(nowMs, stmFsSinceMs_) / 1000),
             kSourceStm);
  }
  const bool emuActive = emulationActive(nowMs);
  if (emuActive && !emuReported_) {
    emuReported_ = true;
    emuSinceMs_ = nowMs;
    sink.add(EventCode::FailsafeActive, emulatedMask(nowMs), kSourceEsp);
  } else if (!emuActive && emuReported_) {
    emuReported_ = false;
    sink.add(EventCode::FailsafeEnded, static_cast<int32_t>(elapsedMs(nowMs, emuSinceMs_) / 1000),
             kSourceEsp);
  }
  if (failEventPending_) {
    failEventPending_ = false;
    sink.add(EventCode::LeaseConfigFailed, failReason_, attempts_);
  }
  return sink.count();
}

// ---------------------------------------------------------------- restarts

LeaseClient::Snapshot LeaseClient::snapshot(uint32_t nowMs) const {
  Snapshot s;
  s.lost = !effAlive_ && lostValid_;
  s.active = emulationActive(nowMs);
  s.mask = emulatedMask(nowMs);
  s.lostElapsedMs = s.lost ? elapsedMs(nowMs, lostSinceMs_) : 0;
  return s;
}

void LeaseClient::restore(const Snapshot& s, uint32_t nowMs) {
  if (s.lost) {
    lostValid_ = true;
    lostSinceMs_ = nowMs - s.lostElapsedMs;
    lostReported_ = s.lostElapsedMs >= kRegulatorEventMs;
  }
  if (s.active) {
    restoredActive_ = true;
    restoredMask_ = s.mask;
    emuReported_ = true;
    emuSinceMs_ = nowMs;
  }
}

size_t encodeLeaseRecord(const LeaseClient::Snapshot& s, uint8_t (&out)[kLeaseRecordSize]) {
  for (size_t i = 0; i < 4; ++i) out[i] = kLeaseMagic[i];
  out[4] = s.lost ? 1 : 0;
  out[5] = s.active ? 1 : 0;
  out[6] = static_cast<uint8_t>(s.mask);
  out[7] = static_cast<uint8_t>(s.mask >> 8);
  for (size_t i = 0; i < 4; ++i) out[8 + i] = static_cast<uint8_t>(s.lostElapsedMs >> (8 * i));
  const uint32_t crc = crc32(out, 12);
  out[12] = static_cast<uint8_t>(crc);
  out[13] = static_cast<uint8_t>(crc >> 8);
  return kLeaseRecordSize;
}

bool decodeLeaseRecord(const uint8_t* data, size_t len, LeaseClient::Snapshot& out) {
  out = LeaseClient::Snapshot{};
  if (data == nullptr || len != kLeaseRecordSize) return false;
  for (size_t i = 0; i < 4; ++i) {
    if (data[i] != kLeaseMagic[i]) return false;
  }
  const uint32_t crc = crc32(data, 12);
  if (data[12] != static_cast<uint8_t>(crc) || data[13] != static_cast<uint8_t>(crc >> 8)) {
    return false;
  }
  if (data[4] > 1 || data[5] > 1) return false;
  LeaseClient::Snapshot s;
  s.lost = data[4] == 1;
  s.active = data[5] == 1;
  s.mask = static_cast<uint16_t>(data[6] | data[7] << 8);
  for (size_t i = 0; i < 4; ++i) s.lostElapsedMs |= static_cast<uint32_t>(data[8 + i]) << (8 * i);
  out = s;
  return true;
}

}  // namespace vdm
