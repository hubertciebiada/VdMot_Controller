// Decisions of the MQTT task: Home Assistant status, reconnect pacing, client
// id, inbound commands (echoes, retained leftovers, rejections), button
// confirmation and the latest target per valve. Hardware-free.
#pragma once

#include <stddef.h>
#include <stdint.h>

#include "vdm/common.h"
#include "vdm/config.h"
#include "vdm/failsafe.h"
#include "vdm/mqtt_topics.h"

namespace vdm {

// K1: what Home Assistant said on its status topic. The status survives
// broker reconnects (HA's birth and last will are often not retained, so a
// reconnect must not turn a known Offline into Unknown) and ESP software
// restarts (RTC record); power-on starts Unknown.
class RegulatorWatch {
 public:
  enum class Change : uint8_t { None, WentOffline, CameOnline, CameOnlineByCommand };
  // Exact payload bytes: "offline" -> Offline (WentOffline unless it was
  // Offline); "online" -> Online (CameOnline only when it was Offline; Unknown
  // -> Online is silent); anything else is ignored.
  Change onHaStatus(const char* payload, size_t len);
  // An accepted inbound command in MqttHa mode: somebody is controlling, so an
  // Offline HA reads as Online again (CameOnlineByCommand: no discovery run).
  Change onInboundCommand();
  HaStatus haStatus() const { return status_; }
  HaStatus snapshot() const { return status_; }
  void restore(HaStatus s);  // out-of-range values restore Unknown

 private:
  HaStatus status_ = HaStatus::Unknown;
};

// RTC record of the HA status: {u32 magic "VDHA", u8 status, u8 pad, u16 crc}
// with crc = low 16 bits of crc32 over the first 6 bytes.
struct HaStatusRecord {
  uint32_t magic;
  uint8_t status;
  uint8_t pad;
  uint16_t crc;
};
constexpr uint32_t kHaStatusMagic = 0x41484456u;  // "VDHA" little endian
void encodeHaStatusRecord(HaStatus s, HaStatusRecord& out);
// Unknown for a record with a wrong magic, CRC or status (power-on garbage).
HaStatus decodeHaStatusRecord(const HaStatusRecord& r);

// W20: reconnect pacing. The back-off (minMs doubling to maxMs) is reset only
// after a connection stayed up for kStableMs; a drop before that counts as a
// failed attempt.
class ReconnectPacer {
 public:
  static constexpr uint32_t kStableMs = 60000;
  ReconnectPacer(uint32_t minMs = 2000, uint32_t maxMs = 60000);
  bool due(uint32_t nowMs) const { return backoff_.due(nowMs); }
  void onAttemptFailed(uint32_t nowMs) { backoff_.onFailure(nowMs); }
  void onConnected(uint32_t nowMs);
  void onDropped(uint32_t nowMs);             // connection lost: failure unless it was stable
  void tick(uint32_t nowMs, bool connected);  // stable after kStableMs -> back-off reset
  void forceNow();                            // user reconnect: due at once, delay back to min
  uint32_t delayMs() const { return backoff_.delayMs(); }

 private:
  Backoff backoff_;
  bool connected_ = false;
  bool stable_ = false;
  uint32_t connectedMs_ = 0;
};

// W20: "<host>-<mac>" = buildHostname(station) cut to 16 chars (a '-' left at
// the end of the cut is dropped), '-', mac[3..5] as 6 lowercase hex digits.
// At most 23 chars (MQTT 3.1.1 guaranteed length). Returns the length; 0 (and
// "" when cap > 0) when cap < 24.
size_t buildMqttClientId(const char* station, const uint8_t (&mac)[6], char* out, size_t cap);

// Without `separate` the target state topic is also a command topic: the ESP
// remembers the value it published there last per valve (since connect); an
// inbound message on the state form with that value is its own echo.
class EchoFilter {
 public:
  void reset();
  void published(uint8_t valve, uint8_t value);
  bool isEcho(uint8_t valve, uint8_t value) const;

 private:
  uint16_t valid_ = 0;
  uint8_t value_[kValveCount] = {};
};

enum class RejectReason : uint8_t {
  None, Payload, UnknownValve, Inactive, Unsupported, UnknownCommand, QueueFull, ClearNotConfirmed,
};
// "", "payload", "unknown valve", "inactive", "unsupported", "unknown command",
// "queue full", "clear not confirmed"; "" out of range.
const char* rejectReasonName(RejectReason r);

enum class InboundAction : uint8_t {
  Ignore, Reject, SetTarget, StopValve, CalibrateValve, CalibrateAll, Restart, StmReset, Detect,
  StopAll, StmSafeExit, HaOnline, HaOffline,
};
// Button actions: run only after the broker echoed the clearing publish.
bool inboundIsButton(InboundAction a);

struct InboundDecision {
  InboundAction action = InboundAction::Ignore;
  uint8_t valve = kNoValve;     // valve actions and valve rejects (0..11)
  uint8_t pos = 0;              // SetTarget
  RejectReason reason = RejectReason::None;
  int32_t detail = 0;           // Reject Payload: the TargetPayload value
  bool clearRetained = false;   // publish "" retained to the topic first
};
struct InboundContext {
  const TopicContext* topics = nullptr;
  const char* haPrefix = "homeassistant";
  const char (*segments)[kSegmentMax + 1] = nullptr;   // kValveCount entries
  uint16_t activeMask = 0;
  MqttMode mode = MqttMode::Off;
  bool stmV3 = false;                 // STM protocol >= 3 (sstop, ssafe)
  const EchoFilter* echo = nullptr;
};
// Rules:
//  - not our topic -> Ignore; an empty payload (after trimming blanks) on any
//    command topic -> Ignore (the echo of a retained clear, or someone
//    clearing);
//  - HaStatus: mode != MqttHa -> Ignore; exactly "online"/"offline" ->
//    HaOnline/HaOffline; else Ignore; never cleared;
//  - Target: valve -1 -> Reject UnknownValve; inactive -> Reject Inactive;
//    payload Ok -> SetTarget (state form && echo->isEcho(valve, pos) ->
//    Ignore); Stop -> StopValve with stmV3, else Reject Unsupported;
//    NotNumber/OutOfRange -> Reject Payload (detail = the TargetPayload);
//  - CalibrateValve: valve and inactive as Target, payload must be PRESS
//    (else Reject Payload);
//  - other cmd topics: payload PRESS (else Reject Payload); StopAll and
//    StmSafeExit need stmV3 (else Unsupported); UnknownCommand -> Reject;
//  - clearRetained: every non-empty message on a command topic (actions and
//    rejects), except the state form "<main>valves/<seg>/target" without
//    separate (the ESP's own retained state).
InboundDecision decideInbound(const InboundContext& c, const char* topic, size_t topicLen,
                              const uint8_t* payload, size_t len);
// True for an empty (blank) payload on a command topic: the broker echoing a
// retained clear back (ButtonGate::confirm).
bool inboundIsClearEcho(const InboundContext& c, const char* topic, size_t topicLen,
                        const uint8_t* payload, size_t len);

// E28: logging throttle for rejected commands: a rejection is logged when its
// (valve, reason) differs from the last logged one or kRepeatMs passed since
// it; the caller counts every rejection.
class RejectLog {
 public:
  static constexpr uint32_t kRepeatMs = 10000;
  // valveKey 0 = no valve, 1..12.
  bool shouldLog(uint8_t valveKey, RejectReason reason, uint32_t nowMs);

 private:
  bool valid_ = false;
  uint8_t key_ = 0;
  RejectReason reason_ = RejectReason::None;
  uint32_t lastMs_ = 0;
};

// Button actions wait for the broker to echo the empty retained message the
// ESP published to their topic (then a retained PRESS cannot repeat at the
// next connect); without the echo within kConfirmMs they are rejected.
class ButtonGate {
 public:
  static constexpr size_t kSlots = 4;
  static constexpr uint32_t kConfirmMs = 5000;
  // Queues a decided button action for `topic`; false when every slot is busy
  // (the caller rejects it: queue full).
  bool hold(const InboundDecision& d, const char* topic, size_t len, uint32_t nowMs);
  // The empty message arrived on `topic`: true and the oldest held action of
  // that topic in `out` (removed).
  bool confirm(const char* topic, size_t len, InboundDecision& out);
  // A held action older than kConfirmMs: true and that action in `out`
  // (removed); the caller rejects it (clear not confirmed).
  bool expire(uint32_t nowMs, InboundDecision& out);
  void reset();  // disconnect: held actions are dropped

 private:
  struct Slot {
    bool used = false;
    InboundDecision d;
    uint32_t sinceMs = 0;
    uint8_t len = 0;
    char topic[kTopicMax + 1] = {0};
  };
  Slot slots_[kSlots];
};

// H14: the newest target per valve that app::submit() refused (full queue);
// the task re-submits it until the queue takes it, a newer one replaces it.
class TargetLatch {
 public:
  void set(uint8_t valve, uint8_t pos);
  void clear(uint8_t valve);
  // The first latched valve at or after `from` (wrapping); false when none.
  bool next(uint8_t from, uint8_t& valve, uint8_t& pos) const;
  bool pending(uint8_t valve) const;

 private:
  uint16_t mask_ = 0;
  uint8_t pos_[kValveCount] = {};
};

}  // namespace vdm
