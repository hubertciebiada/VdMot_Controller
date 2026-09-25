#include "vdm/mqtt_policy.h"

#include <string.h>

namespace vdm {

namespace {

bool bytesAre(const char* p, size_t len, const char* s) {
  const size_t n = strlen(s);
  return p != nullptr && len == n && memcmp(p, s, n) == 0;
}

bool isBlank(char c) { return c == ' ' || c == '\t' || c == '\r' || c == '\n'; }

// Payload without surrounding blanks is empty.
bool blankPayload(const uint8_t* p, size_t len) {
  for (size_t i = 0; i < len; ++i) {
    if (!isBlank(static_cast<char>(p[i]))) return false;
  }
  return true;
}

bool isCommand(InboundKind k) { return k != InboundKind::None && k != InboundKind::HaStatus; }

uint16_t haRecordCrc(const HaStatusRecord& r) {
  uint8_t b[6];
  memcpy(b, &r.magic, 4);
  b[4] = r.status;
  b[5] = r.pad;
  return static_cast<uint16_t>(crc32(b, sizeof b) & 0xFFFFu);
}

InboundDecision reject(RejectReason reason, int8_t valve, int32_t detail = 0) {
  InboundDecision d;
  d.action = InboundAction::Reject;
  d.reason = reason;
  d.valve = valve >= 0 ? static_cast<uint8_t>(valve) : kNoValve;
  d.detail = detail;
  return d;
}

}  // namespace

// ---------------------------------------------------------------- RegulatorWatch

RegulatorWatch::Change RegulatorWatch::onHaStatus(const char* payload, size_t len) {
  if (bytesAre(payload, len, "offline")) {
    const bool was = status_ == HaStatus::Offline;
    status_ = HaStatus::Offline;
    return was ? Change::None : Change::WentOffline;
  }
  if (bytesAre(payload, len, "online")) {
    const bool was = status_ == HaStatus::Offline;
    status_ = HaStatus::Online;
    return was ? Change::CameOnline : Change::None;
  }
  return Change::None;
}

RegulatorWatch::Change RegulatorWatch::onInboundCommand() {
  if (status_ != HaStatus::Offline) return Change::None;
  status_ = HaStatus::Online;
  return Change::CameOnlineByCommand;
}

void RegulatorWatch::restore(HaStatus s) {
  status_ = static_cast<uint8_t>(s) <= static_cast<uint8_t>(HaStatus::Offline) ? s
                                                                               : HaStatus::Unknown;
}

void encodeHaStatusRecord(HaStatus s, HaStatusRecord& out) {
  out.magic = kHaStatusMagic;
  out.status = static_cast<uint8_t>(s);
  out.pad = 0;
  out.crc = haRecordCrc(out);
}

HaStatus decodeHaStatusRecord(const HaStatusRecord& r) {
  if (r.magic != kHaStatusMagic || r.crc != haRecordCrc(r) ||
      r.status > static_cast<uint8_t>(HaStatus::Offline)) {
    return HaStatus::Unknown;
  }
  return static_cast<HaStatus>(r.status);
}

// ---------------------------------------------------------------- ReconnectPacer

ReconnectPacer::ReconnectPacer(uint32_t minMs, uint32_t maxMs) : backoff_(minMs, maxMs) {}

void ReconnectPacer::onConnected(uint32_t nowMs) {
  connected_ = true;
  stable_ = false;
  connectedMs_ = nowMs;
}

void ReconnectPacer::onDropped(uint32_t nowMs) {
  if (!connected_) return;
  connected_ = false;
  if (!stable_) backoff_.onFailure(nowMs);
}

void ReconnectPacer::tick(uint32_t nowMs, bool connected) {
  if (!connected || !connected_ || stable_) return;
  if (elapsedMs(nowMs, connectedMs_) < kStableMs) return;
  stable_ = true;
  backoff_.reset();
}

void ReconnectPacer::forceNow() { backoff_.reset(); }

// ---------------------------------------------------------------- client id

size_t buildMqttClientId(const char* station, const uint8_t (&mac)[6], char* out, size_t cap) {
  if (out == nullptr) return 0;
  if (cap < 24) {
    if (cap > 0) out[0] = '\0';
    return 0;
  }
  char host[65];
  size_t n = buildHostname(station, host, sizeof host);
  if (n > 16) n = 16;
  while (n > 0 && host[n - 1] == '-') --n;
  static const char kHex[] = "0123456789abcdef";
  memcpy(out, host, n);
  out[n++] = '-';
  for (uint8_t i = 3; i < 6; ++i) {
    out[n++] = kHex[mac[i] >> 4];
    out[n++] = kHex[mac[i] & 0x0F];
  }
  out[n] = '\0';
  return n;
}

// ---------------------------------------------------------------- EchoFilter

void EchoFilter::reset() { valid_ = 0; }

void EchoFilter::published(uint8_t valve, uint8_t value) {
  if (valve >= kValveCount) return;
  valid_ = static_cast<uint16_t>(valid_ | (1u << valve));
  value_[valve] = value;
}

bool EchoFilter::isEcho(uint8_t valve, uint8_t value) const {
  return valve < kValveCount && ((valid_ >> valve) & 1u) != 0 && value_[valve] == value;
}

// ---------------------------------------------------------------- inbound

const char* rejectReasonName(RejectReason r) {
  switch (r) {
    case RejectReason::None: return "";
    case RejectReason::Payload: return "payload";
    case RejectReason::UnknownValve: return "unknown valve";
    case RejectReason::Inactive: return "inactive";
    case RejectReason::Unsupported: return "unsupported";
    case RejectReason::UnknownCommand: return "unknown command";
    case RejectReason::QueueFull: return "queue full";
    case RejectReason::ClearNotConfirmed: return "clear not confirmed";
  }
  return "";
}

bool inboundIsButton(InboundAction a) {
  switch (a) {
    case InboundAction::CalibrateValve:
    case InboundAction::CalibrateAll:
    case InboundAction::Restart:
    case InboundAction::StmReset:
    case InboundAction::Detect:
    case InboundAction::StopAll:
    case InboundAction::StmSafeExit:
      return true;
    default:
      return false;
  }
}

InboundDecision decideInbound(const InboundContext& c, const char* topic, size_t topicLen,
                              const uint8_t* payload, size_t len) {
  InboundDecision d;
  if (c.topics == nullptr) return d;
  const InboundTopic t = parseInboundTopic(*c.topics, c.haPrefix, topic, topicLen, c.segments);
  if (t.kind == InboundKind::None) return d;
  const char* p = reinterpret_cast<const char*>(payload);
  if (payload == nullptr) len = 0;
  if (t.kind == InboundKind::HaStatus) {
    if (c.mode != MqttMode::MqttHa) return d;
    if (bytesAre(p, len, "online")) d.action = InboundAction::HaOnline;
    if (bytesAre(p, len, "offline")) d.action = InboundAction::HaOffline;
    return d;
  }
  if (blankPayload(payload, len)) return d;

  switch (t.kind) {
    case InboundKind::Target:
    case InboundKind::CalibrateValve: {
      if (t.valve < 0) {
        d = reject(RejectReason::UnknownValve, -1);
      } else if (((c.activeMask >> t.valve) & 1u) == 0) {
        d = reject(RejectReason::Inactive, t.valve);
      } else if (t.kind == InboundKind::CalibrateValve) {
        if (parseButtonPayload(p, len)) {
          d.action = InboundAction::CalibrateValve;
          d.valve = static_cast<uint8_t>(t.valve);
        } else {
          d = reject(RejectReason::Payload, t.valve);
        }
      } else {
        uint8_t pos = 0;
        const TargetPayload r = parseTargetPayload(p, len, pos);
        if (r == TargetPayload::Ok) {
          const bool echo = t.stateForm && c.echo != nullptr &&
                            c.echo->isEcho(static_cast<uint8_t>(t.valve), pos);
          d.action = echo ? InboundAction::Ignore : InboundAction::SetTarget;
          d.valve = static_cast<uint8_t>(t.valve);
          d.pos = pos;
        } else if (r == TargetPayload::Stop && c.stmV3) {
          d.action = InboundAction::StopValve;
          d.valve = static_cast<uint8_t>(t.valve);
        } else if (r == TargetPayload::Stop) {
          d = reject(RejectReason::Unsupported, t.valve);
        } else {
          d = reject(RejectReason::Payload, t.valve, static_cast<int32_t>(r));
        }
      }
      break;
    }
    case InboundKind::UnknownCommand:
      d = reject(RejectReason::UnknownCommand, -1);
      break;
    default: {
      static const InboundAction kActions[] = {
          InboundAction::CalibrateAll, InboundAction::Restart, InboundAction::StmReset,
          InboundAction::Detect,       InboundAction::StopAll, InboundAction::StmSafeExit,
      };
      const InboundAction a =
          kActions[static_cast<uint8_t>(t.kind) - static_cast<uint8_t>(InboundKind::CalibrateAll)];
      const bool v3Only = a == InboundAction::StopAll || a == InboundAction::StmSafeExit;
      if (!parseButtonPayload(p, len)) {
        d = reject(RejectReason::Payload, -1);
      } else if (v3Only && !c.stmV3) {
        d = reject(RejectReason::Unsupported, -1);
      } else {
        d.action = a;
      }
      break;
    }
  }
  d.clearRetained = !t.stateForm;
  return d;
}

bool inboundIsClearEcho(const InboundContext& c, const char* topic, size_t topicLen,
                        const uint8_t* payload, size_t len) {
  if (c.topics == nullptr || (payload != nullptr && !blankPayload(payload, len))) return false;
  return isCommand(parseInboundTopic(*c.topics, c.haPrefix, topic, topicLen, c.segments).kind);
}

// ---------------------------------------------------------------- RejectLog

bool RejectLog::shouldLog(uint8_t valveKey, RejectReason reason, uint32_t nowMs) {
  if (valid_ && key_ == valveKey && reason_ == reason && elapsedMs(nowMs, lastMs_) < kRepeatMs) {
    return false;
  }
  valid_ = true;
  key_ = valveKey;
  reason_ = reason;
  lastMs_ = nowMs;
  return true;
}

// ---------------------------------------------------------------- ButtonGate

bool ButtonGate::hold(const InboundDecision& d, const char* topic, size_t len, uint32_t nowMs) {
  if (topic == nullptr || len > kTopicMax) return false;
  for (Slot& s : slots_) {
    if (s.used) continue;
    s.used = true;
    s.d = d;
    s.sinceMs = nowMs;
    s.len = static_cast<uint8_t>(len);
    memcpy(s.topic, topic, len);
    s.topic[len] = '\0';
    return true;
  }
  return false;
}

bool ButtonGate::confirm(const char* topic, size_t len, InboundDecision& out) {
  if (topic == nullptr) return false;
  for (Slot& s : slots_) {
    if (!s.used || s.len != len || memcmp(s.topic, topic, len) != 0) continue;
    out = s.d;
    s.used = false;
    return true;
  }
  return false;
}

bool ButtonGate::expire(uint32_t nowMs, InboundDecision& out) {
  for (Slot& s : slots_) {
    if (!s.used || elapsedMs(nowMs, s.sinceMs) < kConfirmMs) continue;
    out = s.d;
    s.used = false;
    return true;
  }
  return false;
}

void ButtonGate::reset() {
  for (Slot& s : slots_) s.used = false;
}

// ---------------------------------------------------------------- TargetLatch

void TargetLatch::set(uint8_t valve, uint8_t pos) {
  if (valve >= kValveCount) return;
  mask_ = static_cast<uint16_t>(mask_ | (1u << valve));
  pos_[valve] = pos;
}

void TargetLatch::clear(uint8_t valve) {
  if (valve >= kValveCount) return;
  mask_ = static_cast<uint16_t>(mask_ & ~(1u << valve));
}

bool TargetLatch::next(uint8_t from, uint8_t& valve, uint8_t& pos) const {
  for (uint8_t k = 0; k < kValveCount; ++k) {
    const uint8_t v = static_cast<uint8_t>((from + k) % kValveCount);
    if (((mask_ >> v) & 1u) == 0) continue;
    valve = v;
    pos = pos_[v];
    return true;
  }
  return false;
}

bool TargetLatch::pending(uint8_t valve) const {
  return valve < kValveCount && ((mask_ >> valve) & 1u) != 0;
}

}  // namespace vdm
