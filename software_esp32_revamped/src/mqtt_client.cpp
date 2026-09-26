#include "mqtt_client.h"

#include <Arduino.h>
#include <LittleFS.h>
#include <PubSubClient.h>
#include <WiFiClient.h>
#include <esp_attr.h>
#include <esp_mac.h>
#include <esp_task_wdt.h>
#include <freertos/FreeRTOS.h>
#include <string.h>

#include <vdm/config.h>
#include <vdm/event_limiter.h>
#include <vdm/ha_discovery.h>
#include <vdm/json_writer.h>
#include <vdm/mqtt_policy.h>
#include <vdm/mqtt_topics.h>
#include <vdm/mqtt_values.h>
#include <vdm/version.h>

#include "app.h"
#include "boot_alloc.h"
#include "logger.h"
#include "net.h"
#include "ota.h"
#include "storage.h"

namespace mqtt {

namespace {

// PublishScheduler slots (DESIGN.md "MQTT topics").
constexpr uint8_t kSlotCommon = 0;
constexpr uint8_t kSlotValve0 = 1;
constexpr uint8_t kSlotTemp0 = kSlotValve0 + vdm::kValveCount;  // 13
constexpr uint8_t kSlotVolt0 = kSlotTemp0 + vdm::kTempSlotCount;  // 47
constexpr uint8_t kSlotStm = kSlotVolt0 + vdm::kVoltSlotCount;    // 55
constexpr uint8_t kSlotSystem = kSlotStm + 1;                     // 56: stm/status, failsafe
constexpr uint8_t kSlotCount = kSlotSystem + 1;
static_assert(kSlotCount <= vdm::PublishScheduler::kSlots, "scheduler slots");

constexpr uint8_t kMaxSlotsPerPass = 2;       // on-change items per loop pass
constexpr uint8_t kMaxDiagPerPass = 4;        // diag messages per loop pass
constexpr size_t kEventsPerPass = 4;
constexpr uint32_t kSensorStaleMs = 60000;
constexpr uint32_t kCounterPaceMs = 10000;    // diag/mqtt/* at most this often
constexpr uint32_t kStartedToleranceS = 60;   // diag/stm/started moves by more than this
constexpr size_t kInboundSlots = 4;
constexpr size_t kInboundPayloadMax = 33;     // longer payloads are cut (every valid one is shorter)
constexpr const char* kListFile = "/HADiscovery.cfg";
constexpr const char* kListTmp = "/HADiscovery.cfg.tmp";

// Fields of a valve that trigger an on-change publish.
constexpr uint32_t kValveCompatMask = vdm::kChangeStatus | vdm::kChangePosition |
                                      vdm::kChangeTarget | vdm::kChangeMeanCurrent |
                                      vdm::kChangeTemp1 | vdm::kChangeTemp2 |
                                      vdm::kChangeCounters | vdm::kChangeCalibRetries |
                                      vdm::kChangeSensors | vdm::kChangeKnown |
                                      vdm::kChangeSync | vdm::kChangeHealth |
                                      vdm::kChangeFailsafe;

WiFiClient gNet;
PubSubClient gClient(gNet);

// Task-owned copies (static: too large for the stack).
vdm::Config& gCfg = bootAlloc<vdm::Config>();
vdm::Config& gNextCfg = bootAlloc<vdm::Config>();  // reloadConfig() scratch
uint32_t gCfgRevision = 0;
app::StmSnapshot& gSnap = bootAlloc<app::StmSnapshot>();
uint32_t gSnapRevision = UINT32_MAX;
vdm::TopicContext gTopics;
char gSegments[vdm::kValveCount][vdm::kSegmentMax + 1];
char gHost[vdm::kHostMax + 1];  // PubSubClient keeps the pointer
char gClientId[vdm::kClientIdMax + 1];
char gLwtTopic[vdm::kTopicMax + 1];
bool gCleanNext = false;  // the next connect drops the broker session (topic config changed)
vdm::PublishScheduler gScheduler;
vdm::EventRateLimiter gEventLimiter;
vdm::EventAggregator gAggregator;
uint32_t gEventCursor = 0;

// Last published values (on-change detection).
using ValveStates = ObjArray<vdm::ValveState, vdm::kValveCount>;
ValveStates& gPubValve = bootAlloc<ValveStates>();
bool gPubValveValid[vdm::kValveCount];
struct SensorPub {
  bool valid = false;  // an entry was published since connect
  bool ok = false;
  int32_t value = 0;
};
SensorPub gPubTemp[vdm::kTempSlotCount];
SensorPub gPubVolt[vdm::kVoltSlotCount];
uint8_t gPubState = 0xFF;
uint32_t gPubUptime = UINT32_MAX;
int8_t gPubStmOnline = -1;
int8_t gPubFailsafe = -1;
char gMessage[120] = "";        // message of the latest Warning+ event
bool gMessageChanged = true;
bool gFirstPublish = true;      // common/ip only on the first full publish per connection
bool gFullRunning = false;
uint8_t gFullCursor = 0;
uint8_t gChangeCursor = 0;

// Calibration end per valve (legacy calibration/date), observed from the
// snapshots regardless of the connection state.
vdm::CalibEndTracker gCalib;

// New diag topics: last published values (valid after connect).
struct DiagPub {
  bool valid = false;
  uint32_t moveSeq = 0;
  uint32_t earlyStops = 0;
  uint32_t cmdRejected = 0;
  uint8_t calState = 0;
  uint32_t profileCrc = 0;
};
DiagPub gPubDiag[vdm::kValveCount];
struct StmPub {
  bool valid = false;
  uint8_t proto = 0;
  vdm::LinkState link = vdm::LinkState::Unknown;
  uint32_t resets = 0, rxOverflow = 0, parseErr = 0;
  bool statusValid = false;
  int8_t calibActive = -1;
  char version[32] = "";
  int64_t started = -1;
  int8_t lease = -1;
  int8_t safeMode = -1;
  bool nextValid = false;
  int64_t next = 0;
  bool suppressedValid = false;
  uint32_t suppressed = 0;
  uint32_t suppressedMs = 0;
  bool rejectedValid = false;
  uint32_t rejected = 0;
  uint32_t rejectedMs = 0;
} gPubStm;

// Discovery.
class ListPort : public vdm::DiscoveryPort {
 public:
  bool publish(const char* topic, const char* payload) override;
  bool listOpen() override {
    if (!storage::fsReady()) return false;
    read_ = LittleFS.open(kListFile, FILE_READ);
    return static_cast<bool>(read_);
  }
  int listRead() override { return read_ ? read_.read() : -1; }
  void listClose() override { read_.close(); }
  bool listBegin() override {
    if (!storage::fsReady()) return false;
    write_ = LittleFS.open(kListTmp, FILE_WRITE);
    return static_cast<bool>(write_);
  }
  bool listWrite(const char* topic) override {
    const size_t n = strlen(topic);
    return write_.write(reinterpret_cast<const uint8_t*>(topic), n) == n &&
           write_.write(reinterpret_cast<const uint8_t*>("\n"), 1) == 1;
  }
  bool listCommit() override {
    write_.close();
    return LittleFS.rename(kListTmp, kListFile);
  }
  void listAbort() override {
    write_.close();
    if (storage::fsReady()) LittleFS.remove(kListTmp);
  }
  void close() {
    read_.close();
    write_.close();
  }

 private:
  fs::File read_;
  fs::File write_;
};

vdm::DiscoveryContext& gDiscCtx = bootAlloc<vdm::DiscoveryContext>();
vdm::DiscoveryRun gRun;
ListPort gPort;
uint32_t gDiscKey = 0;
uint32_t gDiscKeyRevision = UINT32_MAX;  // snapshot revision the key was checked at
using PayloadBuf = ObjArray<char, vdm::kDiscoveryPayloadMax + 1>;
PayloadBuf& gDiscPayload = bootAlloc<PayloadBuf>();  // also the profile payload

// Inbound messages: copied by the PubSubClient callback, handled after loop().
struct Inbound {
  char topic[vdm::kTopicMax + 1];
  size_t topicLen;
  uint8_t payload[kInboundPayloadMax];
  size_t len;
};
Inbound gInbound[kInboundSlots];
size_t gInboundCount = 0;
uint32_t gInboundOverflow = 0;  // messages lost to a full queue since the last drain
vdm::EchoFilter gEcho;
vdm::RejectLog gRejectLog;
vdm::ButtonGate gButtons;
vdm::TargetLatch gLatch;
uint8_t gLatchCursor = 0;
vdm::RegulatorWatch gRegulator;
RTC_NOINIT_ATTR vdm::HaStatusRecord gRtcHaStatus;

portMUX_TYPE gMux = portMUX_INITIALIZER_UNLOCKED;
Status gStatus;
vdm::RegulatorInput gRegulatorState;
volatile bool gReconnectRequested = false;
volatile bool gDiscoveryRequested = false;
volatile DiscoveryAction gDiscoveryAction = DiscoveryAction::Publish;
bool gOfflineSent = false;
bool gConnected = false;

vdm::ReconnectPacer gPacer(kBackoffMinMs, kBackoffMaxMs);
bool gConfigLoaded = false;

void onDisconnected();

void setState(vdm::MqttState s, int8_t rc = 0) {
  portENTER_CRITICAL(&gMux);
  gStatus.state = s;
  gStatus.rc = rc;
  portEXIT_CRITICAL(&gMux);
}

void count(uint32_t Status::*field) {
  portENTER_CRITICAL(&gMux);
  ++(gStatus.*field);
  portEXIT_CRITICAL(&gMux);
}

uint32_t readCount(uint32_t Status::*field) {
  portENTER_CRITICAL(&gMux);
  const uint32_t v = gStatus.*field;
  portEXIT_CRITICAL(&gMux);
  return v;
}

void setDiscoveryRunning(bool on) {
  portENTER_CRITICAL(&gMux);
  gStatus.discoveryRunning = on;
  portEXIT_CRITICAL(&gMux);
}

// The HA status goes to the status, the regulator state and the RTC record.
void storeHaStatus() {
  const vdm::HaStatus s = gRegulator.haStatus();
  encodeHaStatusRecord(s, gRtcHaStatus);
  portENTER_CRITICAL(&gMux);
  gStatus.haStatus = s;
  gRegulatorState.ha = s;
  portEXIT_CRITICAL(&gMux);
}

// Regulator view for the lease: mode, broker session, HA status.
void updateRegulator() {
  portENTER_CRITICAL(&gMux);
  gRegulatorState.mode = gCfg.mqtt.mode;
  gRegulatorState.brokerConnected = gConnected;
  gRegulatorState.ha = gRegulator.haStatus();
  portEXIT_CRITICAL(&gMux);
}

void reloadConfig() {
  vdm::Config& next = gNextCfg;
  gCfgRevision = storage::configRevision();
  storage::getConfig(next);
  const bool reconnect = vdm::mqttTopicConfigChanged(next, gCfg);
  gCfg = next;
  vdm::copyString(gTopics.station, sizeof gTopics.station, vdm::mqttRootTopic(gCfg));
  gTopics.pathAsRoot = gCfg.mqtt.pathAsRoot;
  gTopics.separate = gCfg.mqtt.separate;
  for (uint8_t i = 0; i < vdm::kValveCount; ++i) {
    vdm::itemSegment(gCfg, vdm::ItemKind::Valve, i, gSegments[i], sizeof gSegments[i]);
  }
  if (gCfg.mqtt.clientId[0] != '\0') {
    vdm::copyString(gClientId, sizeof gClientId, gCfg.mqtt.clientId);
  } else {
    uint8_t mac[6] = {0};
    esp_efuse_mac_get_default(mac);
    vdm::buildMqttClientId(gCfg.station, mac, gClientId, sizeof gClientId);
  }
  vdm::PublishScheduler::Params p;
  p.onChange = gCfg.mqtt.onChange;
  p.publishIntervalMs = static_cast<uint32_t>(gCfg.mqtt.publishIntervalS) * 1000u;
  p.minDelayMs = static_cast<uint32_t>(gCfg.mqtt.minDelayS) * 1000u;
  gScheduler.configure(p);
  vdm::buildTopic(gTopics, vdm::Topic::Status, nullptr, gLwtTopic, sizeof gLwtTopic);
  // The config found at boot is the one the broker session belongs to.
  if (reconnect && gConfigLoaded) {
    gCleanNext = true;
    if (gClient.connected()) {
      gClient.disconnect();  // clean: the broker does not send the LWT
      onDisconnected();
      gPacer.forceNow();     // not a failed attempt
    }
  }
  gConfigLoaded = true;
}

// PubSubClient needs 5 (fixed header) + 2 (topic length) bytes besides the
// topic and payload. Every message built here fits, so a publish only fails
// on a dead connection or a socket write that stalled.
static_assert(5 + 2 + vdm::kTopicMax + vdm::kDiscoveryPayloadMax <= kBufferSize,
              "largest MQTT message must fit the PubSubClient buffer");

bool publishRaw(const char* topic, const char* payload, bool retained) {
  const bool ok = gClient.publish(topic, payload, retained);
  esp_task_wdt_reset();  // a pass may publish many messages
  if (ok) return true;
  count(&Status::publishFailures);
  // A failed publish on a live connection is a socket write that WiFiClient
  // already retried for up to 10 s (broker gone without FIN/RST). Every
  // further publish of this pass would stall as long, past the task
  // watchdog: drop the connection and let the reconnect back-off take over.
  if (gClient.connected()) gNet.stop();
  return false;
}

bool ListPort::publish(const char* topic, const char* payload) {
  return publishRaw(topic, payload, true);
}

bool publish(vdm::Topic t, const char* segment, const char* payload) {
  char topic[vdm::kTopicMax + 1];
  if (vdm::buildTopic(gTopics, t, segment, topic, sizeof topic) == 0) return false;
  return publishRaw(topic, payload, vdm::topicRetained(t, gCfg.mqtt.retained));
}

bool publishUint(vdm::Topic t, const char* segment, uint32_t v) {
  char buf[12];
  snprintf(buf, sizeof buf, "%lu", static_cast<unsigned long>(v));
  return publish(t, segment, buf);
}

bool publishCounter(vdm::Topic t, const char* segment, uint32_t v) {
  char buf[12];
  vdm::formatLegacyCounter(v, buf, sizeof buf);
  return publish(t, segment, buf);
}

// ---------------------------------------------------------------- discovery

bool stmV3() { return gSnap.proto >= 3; }

vdm::DiscoveryInputs discoveryInputs() {
  vdm::DiscoveryInputs in;
  in.cfg = &gCfg;
  in.valves = gSnap.valves;
  in.temps = gSnap.temps;
  in.tempCount = gSnap.tempCount;
  in.volts = gSnap.volts;
  in.voltCount = gSnap.voltCount;
  in.sensorsSettled = gSnap.sensorsSettled;
  in.stmProto = gSnap.proto;
  in.stmHw = gSnap.version.hw;
  in.ip = net::info().ip;
  in.swVersion = vdm::firmwareVersion();
  return in;
}

void startRun(const vdm::DiscoveryPlan& plan) {
  if (gRun.running()) gRun.abort(gPort);
  const vdm::DiscoveryInputs in = discoveryInputs();
  vdm::buildDiscoveryContext(in, gDiscCtx);
  gDiscKey = vdm::discoveryInputKey(in);
  gDiscKeyRevision = gSnapRevision;
  gRun.start(gDiscCtx, plan);
  setDiscoveryRunning(true);
}

// A publish run (with the first-run cleanup and the 2.0.0 migration when due).
vdm::DiscoveryPlan publishPlan() {
  vdm::DiscoveryPlan p;
  p.publish = true;
  p.dropLegacy = !storage::haCleanupDone();
  p.retire20 = storage::haLayout() < 2;
  return p;
}

// Manual requests run in modes 1 and 2.
void startRequested(DiscoveryAction a) {
  if (gCfg.mqtt.mode == vdm::MqttMode::Off) return;
  vdm::DiscoveryPlan p = a == DiscoveryAction::Delete ? vdm::DiscoveryPlan{} : publishPlan();
  p.removeAll = a != DiscoveryAction::Publish;
  p.prune = a != DiscoveryAction::Delete;
  startRun(p);
}

// Automatic run after a connect.
void startOnConnect() {
  const bool ha = gCfg.mqtt.mode == vdm::MqttMode::MqttHa;
  if (ha && (gCfg.mqtt.haDiscoveryOnConnect || storage::haLayout() < 2)) {
    startRun(publishPlan());
  } else if (!storage::haCleanupDone()) {
    vdm::DiscoveryPlan p;
    p.dropLegacy = true;
    startRun(p);
  }
}

void serviceDiscovery() {
  if (!gRun.running()) return;
  vdm::JsonWriter jw(gDiscPayload.data(), sizeof gDiscPayload.items);
  const vdm::DiscoveryRun::Phase ph = gRun.step(gPort, jw);
  if (ph == vdm::DiscoveryRun::Phase::Aborted) {
    setDiscoveryRunning(false);
    return;
  }
  if (ph != vdm::DiscoveryRun::Phase::Done) return;
  setDiscoveryRunning(false);
  const vdm::DiscoveryRun::Stats& s = gRun.stats();
  logger::log(vdm::EventCode::HaDiscoverySent, vdm::kNoValve, s.configs, s.deletes,
              s.skipped ? "skipped entities" : nullptr);
  if (gRun.plan().dropLegacy) storage::setHaCleanupDone();
  if (gRun.plan().publish) storage::setHaLayout(2);
}

// A valve sensor or a published sensor segment changed after the last run
// (the STM reports assignments only after its first 1-Wire read).
void checkDiscoveryInputs() {
  if (gRun.running() || gCfg.mqtt.mode != vdm::MqttMode::MqttHa ||
      !gCfg.mqtt.haDiscoveryOnConnect || gDiscKeyRevision == gSnapRevision) {
    return;
  }
  gDiscKeyRevision = gSnapRevision;
  if (vdm::discoveryInputKey(discoveryInputs()) != gDiscKey) startRun(publishPlan());
}

// ---------------------------------------------------------------- inbound

void onMessage(char* topic, uint8_t* payload, unsigned int len) {
  // Only copies: publishing here would overwrite PubSubClient's buffer.
  if (gInboundCount >= kInboundSlots) {
    ++gInboundOverflow;
    return;
  }
  Inbound& m = gInbound[gInboundCount++];
  m.topicLen = strnlen(topic, vdm::kTopicMax + 1);
  if (m.topicLen > vdm::kTopicMax) m.topicLen = 0;  // not one of ours
  memcpy(m.topic, topic, m.topicLen);
  m.topic[m.topicLen] = '\0';
  m.len = len < kInboundPayloadMax ? len : kInboundPayloadMax;
  memcpy(m.payload, payload, m.len);
}

void rejectCommand(vdm::RejectReason reason, uint8_t valve, int32_t detail) {
  count(&Status::commandsRejected);
  const uint8_t key = valve < vdm::kValveCount ? static_cast<uint8_t>(valve + 1) : 0;
  if (!gRejectLog.shouldLog(key, reason, app::nowMs())) return;
  logger::log(vdm::EventCode::MqttCommandRejected, vdm::kNoValve, key, detail,
              vdm::rejectReasonName(reason));
}

void accepted() {
  portENTER_CRITICAL(&gMux);
  ++gRegulatorState.commandSeq;
  portEXIT_CRITICAL(&gMux);
  if (gCfg.mqtt.mode == vdm::MqttMode::MqttHa &&
      gRegulator.onInboundCommand() != vdm::RegulatorWatch::Change::None) {
    storeHaStatus();
  }
}

bool submit(app::CommandType type, uint8_t valve) {
  app::Command c;
  c.type = type;
  c.valve = valve;
  return app::submit(c);
}

void submitTarget(uint8_t valve, uint8_t pos) {
  app::Command c;
  c.type = app::CommandType::SetTarget;
  c.valve = valve;
  c.pos = pos;
  c.source = vdm::TargetSource::Mqtt;
  if (app::submit(c)) {
    gLatch.clear(valve);
  } else {
    gLatch.set(valve, pos);  // re-submitted until the queue takes it
  }
}

void act(const vdm::InboundDecision& d) {
  bool ok = true;
  switch (d.action) {
    case vdm::InboundAction::SetTarget:
      submitTarget(d.valve, d.pos);
      break;
    case vdm::InboundAction::StopValve:
      ok = submit(app::CommandType::StopValve, d.valve);
      break;
    case vdm::InboundAction::CalibrateValve:
      ok = submit(app::CommandType::Calibrate, d.valve);
      break;
    case vdm::InboundAction::CalibrateAll:
      ok = submit(app::CommandType::Calibrate, vdm::kAllValves);
      break;
    case vdm::InboundAction::Restart:
      ota::requestRestart(0, 1000);
      break;
    case vdm::InboundAction::StmReset:
      ok = submit(app::CommandType::ResetStm, vdm::kNoValve);
      break;
    case vdm::InboundAction::Detect:
      ok = submit(app::CommandType::Detect, vdm::kAllValves);
      break;
    case vdm::InboundAction::StopAll:
      ok = submit(app::CommandType::StopValve, vdm::kAllValves);
      break;
    case vdm::InboundAction::StmSafeExit:
      ok = submit(app::CommandType::LeaveSafeMode, vdm::kNoValve);
      break;
    default:
      return;
  }
  if (ok) {
    accepted();
  } else {
    rejectCommand(vdm::RejectReason::QueueFull, d.valve, 0);
  }
}

void onHaStatus(const vdm::InboundDecision& d) {
  const char* p = d.action == vdm::InboundAction::HaOnline ? "online" : "offline";
  const vdm::RegulatorWatch::Change ch = gRegulator.onHaStatus(p, strlen(p));
  storeHaStatus();
  if (ch == vdm::RegulatorWatch::Change::CameOnline && gCfg.mqtt.haDiscoveryOnConnect &&
      !gRun.running()) {
    startRun(publishPlan());
  }
}

void handleInbound(const Inbound& m) {
  vdm::InboundContext ic;
  ic.topics = &gTopics;
  ic.haPrefix = gCfg.mqtt.discoveryPrefix;
  ic.segments = gSegments;
  ic.activeMask = vdm::activeValveMask(gCfg);
  ic.mode = gCfg.mqtt.mode;
  ic.stmV3 = stmV3();
  ic.echo = &gEcho;
  vdm::InboundDecision d;
  if (vdm::inboundIsClearEcho(ic, m.topic, m.topicLen, m.payload, m.len)) {
    if (gButtons.confirm(m.topic, m.topicLen, d)) act(d);
    return;
  }
  d = vdm::decideInbound(ic, m.topic, m.topicLen, m.payload, m.len);
  if (d.action == vdm::InboundAction::Ignore) return;
  if (d.action == vdm::InboundAction::HaOnline || d.action == vdm::InboundAction::HaOffline) {
    onHaStatus(d);
    return;
  }
  const bool cleared = !d.clearRetained || publishRaw(m.topic, "", true);
  if (d.action == vdm::InboundAction::Reject) {
    rejectCommand(d.reason, d.valve, d.detail);
  } else if (!vdm::inboundIsButton(d.action)) {
    act(d);
  } else if (!cleared) {
    rejectCommand(vdm::RejectReason::ClearNotConfirmed, d.valve, 0);
  } else if (!gButtons.hold(d, m.topic, m.topicLen, app::nowMs())) {
    rejectCommand(vdm::RejectReason::QueueFull, d.valve, 0);
  }
}

void drainInbound() {
  // Copy out first: publishing below may run the callback again (it never
  // does in PubSubClient 2.8, which reads only in loop()).
  for (size_t i = 0; i < gInboundCount; ++i) handleInbound(gInbound[i]);
  gInboundCount = 0;
  for (; gInboundOverflow > 0; --gInboundOverflow) {
    rejectCommand(vdm::RejectReason::QueueFull, vdm::kNoValve, 0);
  }
  vdm::InboundDecision d;
  while (gButtons.expire(app::nowMs(), d)) {
    rejectCommand(vdm::RejectReason::ClearNotConfirmed, d.valve, 0);
  }
  uint8_t valve = 0;
  uint8_t pos = 0;
  if (gLatch.next(gLatchCursor, valve, pos)) {
    gLatchCursor = static_cast<uint8_t>((valve + 1) % vdm::kValveCount);
    submitTarget(valve, pos);
  }
}

// loop() plus the handling of what it delivered.
void pump() {
  gClient.loop();
  drainInbound();
}

// ---------------------------------------------------------------- snapshot

// Reads a new snapshot when there is one and tracks calibration ends
// (needed for calibration/date even while disconnected).
void observe() {
  const uint32_t rev = app::stmSnapshotRevision();
  if (rev == gSnapRevision) return;
  gSnapRevision = rev;
  app::readStmSnapshot(gSnap);
  const vdm::LocalTime now = net::localTime();
  portENTER_CRITICAL(&gMux);  // calibrationEnd() reads it from other tasks
  gCalib.observe(gSnap.valves, now);
  portEXIT_CRITICAL(&gMux);
}

bool failsafeActive() { return gSnap.lease.state == vdm::LeaseState::Expired; }

bool safeMode() { return gSnap.haveStatus && gSnap.status.v3 && gSnap.status.safeMode; }

uint8_t currentSystemState() {
  vdm::SystemFlags f;
  f.safeMode = safeMode();
  f.failsafe = failsafeActive();
  return vdm::systemState(gSnap.link, gSnap.valves, vdm::kValveCount, vdm::activeValveMask(gCfg),
                          f);
}

bool slotTemp(uint8_t slot1, int32_t& tenths) {
  return vdm::slotTempTenths(gCfg, gSnap.temps, gSnap.tempCount, slot1, app::nowMs(),
                             kSensorStaleMs, tenths);
}

bool slotVolt(uint8_t i, double& value) {
  return vdm::slotVoltValue(gCfg, gSnap.volts, gSnap.voltCount, i, app::nowMs(), kSensorStaleMs,
                            value);
}

// ---------------------------------------------------------------- compat slots

void publishCommon(bool full) {
  char buf[vdm::kTopicMax + 1];
  if (full && gFirstPublish) {
    vdm::formatIpv4(net::info().ip, buf, sizeof buf);
    if (publish(vdm::Topic::CommonIp, nullptr, buf)) gFirstPublish = false;
  }
  const uint8_t st = currentSystemState();
  vdm::formatSystemState(st, gCfg.mqtt.plainText, buf, sizeof buf);
  if (publish(vdm::Topic::CommonState, nullptr, buf)) gPubState = st;
  if (gCfg.mqtt.upTime) {
    const uint32_t up = app::uptimeS();
    vdm::formatUptime(up, buf, sizeof buf);
    if (publish(vdm::Topic::CommonUptime, nullptr, buf)) gPubUptime = up;
  }
  if (publish(vdm::Topic::CommonMessage, nullptr, gMessage)) gMessageChanged = false;
}

void publishValveTemp(vdm::Topic t, const char* seg, int16_t raw, uint8_t slot1) {
  if (raw == vdm::kTempUnassigned) return;
  int32_t tenths = raw;
  if (slot1 >= 1 && slot1 <= vdm::kTempSlotCount) tenths += gCfg.temps[slot1 - 1].offset;
  char buf[16];
  vdm::formatTemp(tenths, vdm::tempRawValid(raw), gCfg.mqtt.germanDecimal, buf, sizeof buf);
  publish(t, seg, buf);
}

void publishValve(uint8_t i) {
  const vdm::ValveState& v = gSnap.valves[i];
  const char* seg = gSegments[i];
  gPubValve[i] = v;
  gPubValveValid[i] = true;
  if (!gCfg.valves[i].active || !v.known) return;
  char buf[48];
  uint8_t target = 0;
  if (vdm::publishedTarget(v, gCfg.mqtt.separate, target) &&
      publishUint(vdm::Topic::ValveTarget, seg, target) && !gCfg.mqtt.separate) {
    gEcho.published(i, target);
  }
  if (v.desiredValid) publishUint(vdm::Topic::ValveRequested, seg, v.desired);
  publish(vdm::Topic::ValveSync, seg, vdm::targetSyncName(v.sync));
  publish(vdm::Topic::ValveFailsafe, seg, vdm::failsafeKindName(vdm::failsafeKind(v)));
  publish(vdm::Topic::ValveProblem, seg, vdm::valveProblem(v) ? "1" : "0");
  vdm::formatValveState(v.status, gCfg.mqtt.plainText, buf, sizeof buf);
  publish(vdm::Topic::ValveState, seg, buf);
  publishUint(vdm::Topic::ValveActual, seg, v.position);
  if (gCalib.ended(i)) {
    vdm::formatCalibDate(gCalib.end(i), buf, sizeof buf);
    if (publish(vdm::Topic::ValveCalibDate, seg, buf)) gCalib.clearDirty(i);
  }
  publishUint(vdm::Topic::ValveCalibRepetitions, seg, v.calibRetries);
  if (gCfg.mqtt.diag) {
    publishUint(vdm::Topic::ValveMeanCurrent, seg, v.meanCurrent);
    publishCounter(vdm::Topic::ValveOpenCount, seg, v.openCount);
    publishCounter(vdm::Topic::ValveCloseCount, seg, v.closeCount);
    snprintf(buf, sizeof buf, "%ld", static_cast<long>(v.deadZone));
    publish(vdm::Topic::ValveDeadZoneCount, seg, buf);
    publishCounter(vdm::Topic::ValveMoves, seg, v.moves);
  }
  publishValveTemp(vdm::Topic::ValveTemp1, seg, v.temp1, v.sensorSlot[0]);
  publishValveTemp(vdm::Topic::ValveTemp2, seg, v.temp2, v.sensorSlot[1]);
}

void publishTemp(uint8_t i) {
  SensorPub& p = gPubTemp[i];
  int32_t tenths = 0;
  p.ok = slotTemp(static_cast<uint8_t>(i + 1), tenths);
  p.value = tenths;
  p.valid = true;
  if (!vdm::tempPublished(gCfg, gSnap.valves, i)) return;
  const vdm::TempSlotConfig& s = gCfg.temps[i];
  char seg[vdm::kSegmentMax + 1];
  if (vdm::sensorTopicSegment(gCfg, vdm::ItemKind::Temp, i,
                              vdm::findTempBus(gSnap.temps, gSnap.tempCount, s.id), seg,
                              sizeof seg) == 0) {
    return;  // unnamed and not on the bus
  }
  char buf[vdm::kOneWireIdTextLen + 1];
  vdm::formatOneWireId(s.id, buf, sizeof buf);
  publish(vdm::Topic::TempId, seg, buf);
  vdm::formatTemp(tenths, p.ok, gCfg.mqtt.germanDecimal, buf, sizeof buf);
  publish(vdm::Topic::TempValue, seg, buf);
}

void publishVolt(uint8_t i) {
  SensorPub& p = gPubVolt[i];
  double value = 0;
  p.ok = slotVolt(i, value);
  p.value = p.ok ? static_cast<int32_t>(value * 1000.0) : 0;
  p.valid = true;
  if (!vdm::voltPublishedMqtt(gCfg, i)) return;
  const vdm::VoltSlotConfig& s = gCfg.volts[i];
  char seg[vdm::kSegmentMax + 1];
  if (vdm::sensorTopicSegment(gCfg, vdm::ItemKind::Volt, i,
                              vdm::findVoltBus(gSnap.volts, gSnap.voltCount, s.id), seg,
                              sizeof seg) == 0) {
    return;
  }
  char buf[vdm::kOneWireIdTextLen + 1];
  vdm::formatOneWireId(s.id, buf, sizeof buf);
  publish(vdm::Topic::VoltId, seg, buf);
  vdm::formatVolt(value, p.ok, gCfg.mqtt.germanDecimal, buf, sizeof buf);
  publish(vdm::Topic::VoltValue, seg, buf);
  publish(vdm::Topic::VoltUnit, seg, s.unit);
}

void publishStmFull() {
  if (!gCfg.mqtt.newDiag || gSnap.proto < 2 || !gSnap.haveStatus) return;
  publishUint(vdm::Topic::DiagStmUptime, nullptr, gSnap.status.uptimeS);
}

// stm/status and failsafe: always published, retained.
void publishSystem() {
  const bool online = vdm::stmOnline(gSnap.link);
  if (publish(vdm::Topic::StmStatus, nullptr, online ? "online" : "offline")) {
    gPubStmOnline = online ? 1 : 0;
  }
  const bool fs = failsafeActive();
  if (publish(vdm::Topic::Failsafe, nullptr, fs ? "1" : "0")) gPubFailsafe = fs ? 1 : 0;
}

void publishSlot(uint8_t slot, bool full) {
  if (slot == kSlotCommon) {
    publishCommon(full);
  } else if (slot < kSlotTemp0) {
    publishValve(slot - kSlotValve0);
  } else if (slot < kSlotVolt0) {
    publishTemp(slot - kSlotTemp0);
  } else if (slot < kSlotStm) {
    publishVolt(slot - kSlotVolt0);
  } else if (slot == kSlotStm) {
    publishStmFull();
  } else {
    publishSystem();
  }
}

bool systemChanged() {
  return gPubStmOnline != (vdm::stmOnline(gSnap.link) ? 1 : 0) ||
         gPubFailsafe != (failsafeActive() ? 1 : 0);
}

bool slotChanged(uint8_t slot) {
  if (slot == kSlotCommon) {
    // The uptime text changes every second; minDelayS spaces it further.
    return (gCfg.mqtt.upTime && app::uptimeS() != gPubUptime) || gMessageChanged ||
           currentSystemState() != gPubState;
  }
  if (slot < kSlotTemp0) {
    const uint8_t i = slot - kSlotValve0;
    return !gPubValveValid[i] || gCalib.dirty(i) ||
           (vdm::diffValve(gPubValve[i], gSnap.valves[i]) & kValveCompatMask) != 0;
  }
  if (slot < kSlotVolt0) {
    const uint8_t i = slot - kSlotTemp0;
    int32_t tenths = 0;
    const bool ok = slotTemp(static_cast<uint8_t>(i + 1), tenths);
    const SensorPub& p = gPubTemp[i];
    return !p.valid || p.ok != ok || (ok && p.value != tenths);
  }
  if (slot < kSlotStm) {
    const uint8_t i = slot - kSlotVolt0;
    double value = 0;
    const bool ok = slotVolt(i, value);
    const SensorPub& p = gPubVolt[i];
    return !p.valid || p.ok != ok || (ok && p.value != static_cast<int32_t>(value * 1000.0));
  }
  return false;  // STM uptime goes out with the full publish only
}

// Full publish, spread over passes: one valve (or a few sensors) per pass
// with loop() in between, so inbound commands are never starved.
void serviceFullPublish(uint32_t now) {
  if (gScheduler.takeFullPublish(now)) {
    gFullRunning = true;
    gFullCursor = 0;
  }
  if (!gFullRunning) return;
  uint8_t budget = 4;
  while (budget > 0 && gFullCursor < kSlotCount) {
    const uint8_t slot = gFullCursor++;
    publishSlot(slot, true);
    budget = (slot >= kSlotValve0 && slot < kSlotTemp0) ? 0 : budget - 1;
  }
  if (gFullCursor >= kSlotCount) {
    gFullRunning = false;
    gScheduler.markAllPublished(now);
  }
}

void serviceOnChange(uint32_t now) {
  if (gFullRunning) return;
  // stm/status and failsafe go out on every change, whatever onChange says.
  if (systemChanged()) publishSystem();
  if (!gCfg.mqtt.onChange) return;
  uint8_t sent = 0;
  for (uint8_t n = 0; n < kSlotSystem && sent < kMaxSlotsPerPass; ++n) {
    const uint8_t slot = gChangeCursor;
    gChangeCursor = static_cast<uint8_t>((gChangeCursor + 1) % kSlotSystem);
    if (gScheduler.takeItem(slot, slotChanged(slot), now)) {
      publishSlot(slot, false);
      ++sent;
    }
  }
}

// ---------------------------------------------------------------- new diag

void publishLastMove(uint8_t i, const vdm::ValveState& v) {
  char json[160];
  vdm::JsonWriter jw(json, sizeof json);
  jw.beginObject();
  jw.kv("dir", v.lastMove.dir == vdm::MoveDir::Open ? "open" : "close");
  jw.kv("req", v.lastMove.requestedCounts);
  jw.kv("cnt", v.lastMove.countedCounts);
  jw.kv("stop", vdm::stopReasonName(v.lastMove.stop));
  jw.kv("peak", static_cast<uint32_t>(v.lastMove.peakCurrent));
  jw.kv("ms", v.lastMove.durationMs);
  jw.endObject();
  if (jw.complete()) publish(vdm::Topic::DiagValveLastMove, gSegments[i], json);
}

// CRC-32 of the profile's bytes with zeros in the padding (after count and after each sample's
// current): a copy or a parse may leave anything there, only the fields may make a profile new.
uint32_t profileCrc(const vdm::Profile& p) {
  static const uint8_t kZeros[sizeof(vdm::ProfileSample)] = {};
  uint32_t crc = vdm::crc32(&p.valve, sizeof p.valve);
  crc = vdm::crc32(&p.count, sizeof p.count, crc);
  crc = vdm::crc32(kZeros, offsetof(vdm::Profile, samples) - sizeof p.valve - sizeof p.count, crc);
  for (const vdm::ProfileSample& s : p.samples) {
    crc = vdm::crc32(reinterpret_cast<const uint8_t*>(&s.count), sizeof s.count, crc);
    crc = vdm::crc32(reinterpret_cast<const uint8_t*>(&s.current), sizeof s.current, crc);
    crc = vdm::crc32(kZeros, sizeof s - sizeof s.count - sizeof s.current, crc);
  }
  return crc;
}

// A counter topic of diag/mqtt: on change, at most every kCounterPaceMs.
void publishPaced(vdm::Topic t, uint32_t value, bool& valid, uint32_t& last, uint32_t& lastMs,
                  uint32_t now) {
  if (valid && (value == last || vdm::elapsedMs(now, lastMs) < kCounterPaceMs)) return;
  if (!publishUint(t, nullptr, value)) return;
  valid = true;
  last = value;
  lastMs = now;
}

// STM version, start time, lease, safe mode, counters, next calibration.
void serviceSystemDiag(uint32_t now) {
  StmPub& s = gPubStm;
  char buf[32];
  if (gSnap.version.valid && vdm::formatVersion(gSnap.version, buf, sizeof buf) > 0 &&
      strcmp(buf, s.version) != 0 && publish(vdm::Topic::DiagStmVersion, nullptr, buf)) {
    vdm::copyString(s.version, sizeof s.version, buf);
  }
  const vdm::LocalTime t = net::localTime();
  if (gSnap.proto >= 2 && gSnap.haveStatus && t.valid && t.epoch > gSnap.status.uptimeS) {
    const int64_t started = t.epoch - static_cast<int64_t>(gSnap.status.uptimeS);
    const int64_t moved = started > s.started ? started - s.started : s.started - started;
    if (s.started < 0 || moved > static_cast<int64_t>(kStartedToleranceS)) {
      vdm::formatUtcTimestamp(static_cast<uint32_t>(started), buf, sizeof buf);
      if (publish(vdm::Topic::DiagStmStarted, nullptr, buf)) s.started = started;
    }
  }
  const int8_t lease = static_cast<int8_t>(gSnap.lease.state);
  if (lease != s.lease &&
      publish(vdm::Topic::DiagStmLease, nullptr, vdm::leaseStateName(gSnap.lease.state))) {
    s.lease = lease;
  }
  if (gSnap.haveStatus && gSnap.status.v3) {
    const int8_t sm = gSnap.status.safeMode ? 1 : 0;
    if (sm != s.safeMode && publish(vdm::Topic::DiagStmSafeMode, nullptr, sm ? "1" : "0")) {
      s.safeMode = sm;
    }
  }
  const int64_t next = app::calibInfo().nextEpoch;
  if (!s.nextValid || next != s.next) {
    buf[0] = '\0';
    if (next > 0) vdm::formatUtcTimestamp(static_cast<uint32_t>(next), buf, sizeof buf);
    if (publish(vdm::Topic::DiagCalibrationNext, nullptr, buf)) {
      s.nextValid = true;
      s.next = next;
    }
  }
  const uint32_t suppressed = gEventLimiter.suppressed() + gAggregator.duplicates();
  publishPaced(vdm::Topic::DiagMqttEventsSuppressed, suppressed, s.suppressedValid, s.suppressed,
               s.suppressedMs, now);
  publishPaced(vdm::Topic::DiagMqttCommandsRejected, readCount(&Status::commandsRejected),
               s.rejectedValid, s.rejected, s.rejectedMs, now);
}

// Publishes changed diag values; bounded messages per pass.
void serviceDiag(uint32_t now) {
  if (!gCfg.mqtt.newDiag) return;
  uint8_t budget = kMaxDiagPerPass;
  // STM level.
  StmPub& s = gPubStm;
  if (!s.valid || s.proto != gSnap.proto) {
    if (gSnap.proto != 0 && publishUint(vdm::Topic::DiagStmProto, nullptr, gSnap.proto)) {
      s.proto = gSnap.proto;
    }
    --budget;
  }
  if (!s.valid || s.link != gSnap.link) {
    if (publish(vdm::Topic::DiagStmLink, nullptr, vdm::linkStateName(gSnap.link))) {
      s.link = gSnap.link;
    }
    --budget;
  }
  s.valid = true;
  if (gSnap.proto >= 2 && gSnap.haveStatus) {
    const vdm::StmStatus& st = gSnap.status;
    if (!s.statusValid || s.resets != st.resets)
      publishUint(vdm::Topic::DiagStmResets, nullptr, st.resets);
    if (!s.statusValid || s.rxOverflow != st.rxOverflow) {
      publishUint(vdm::Topic::DiagStmRxOverflow, nullptr, st.rxOverflow);
    }
    if (!s.statusValid || s.parseErr != st.parseErrors) {
      publishUint(vdm::Topic::DiagStmParseErr, nullptr, st.parseErrors);
    }
    s.resets = st.resets;
    s.rxOverflow = st.rxOverflow;
    s.parseErr = st.parseErrors;
    s.statusValid = true;
  }
  bool calibActive = false;
  for (const vdm::ValveState& v : gSnap.valves) calibActive = calibActive || v.calibrating;
  if (s.calibActive != static_cast<int8_t>(calibActive)) {
    if (publish(vdm::Topic::DiagCalibrationActive, nullptr, calibActive ? "1" : "0")) {
      s.calibActive = static_cast<int8_t>(calibActive);
    }
  }
  serviceSystemDiag(now);
  // Per valve (v2 only).
  for (uint8_t i = 0; i < vdm::kValveCount && budget > 0; ++i) {
    const vdm::ValveState& v = gSnap.valves[i];
    if (!gCfg.valves[i].active || !v.hasExtended) continue;
    DiagPub& d = gPubDiag[i];
    const char* seg = gSegments[i];
    if (!d.valid || d.moveSeq != v.moveSeq) {
      if (v.moveSeq != 0) publishLastMove(i, v);
      d.moveSeq = v.moveSeq;
      --budget;
    }
    if (!d.valid || d.earlyStops != v.earlyStops) {
      publishUint(vdm::Topic::DiagValveEarlyStops, seg, v.earlyStops);
      d.earlyStops = v.earlyStops;
    }
    if (!d.valid || d.cmdRejected != v.cmdRejected) {
      publishUint(vdm::Topic::DiagValveCmdRejected, seg, v.cmdRejected);
      d.cmdRejected = v.cmdRejected;
    }
    if (!d.valid || d.calState != v.calState) {
      publishUint(vdm::Topic::DiagValveCalState, seg, v.calState);
      d.calState = v.calState;
    }
    const vdm::Profile& p = gSnap.profiles[i];
    const uint32_t crc = p.count > 0 ? profileCrc(p) : 0;
    // Not retained: only new profiles go out, not the one known at connect, whose CRC the first
    // pass of the valve stores whatever the budget.
    if (crc != d.profileCrc && (budget > 0 || !d.valid)) {
      if (d.valid && p.count > 0) {
        // Shares the discovery buffer: both run in this task, one at a time.
        vdm::JsonWriter jw(gDiscPayload.data(), sizeof gDiscPayload.items);
        if (vdm::writeProfileJson(jw, p) && jw.complete()) {
          publish(vdm::Topic::DiagValveProfile, seg, jw.c_str());
        }
        --budget;
      }
      d.profileCrc = crc;
    }
    d.valid = true;
    pump();
  }
}

// ---------------------------------------------------------------- events

void publishEvent(const vdm::PublishEvent& pe, uint32_t now) {
  if (!gEventLimiter.allow(pe.event, now)) return;
  char json[512];
  vdm::JsonWriter jw(json, sizeof json);
  if (vdm::writeMqttEventJson(jw, pe.event, pe.valveMask) && jw.complete()) {
    publish(vdm::Topic::Events, nullptr, json);
  }
}

void serviceEvents(uint32_t now) {
  vdm::Event ev[kEventsPerPass];
  uint32_t next = gEventCursor;
  const size_t n = logger::readSince(gEventCursor, ev, kEventsPerPass, next);
  gEventCursor = next;
  const bool out = gConnected && gCfg.mqtt.events;
  for (size_t i = 0; i < n; ++i) {
    if (ev[i].severity >= vdm::Severity::Warning) {
      vdm::formatEventMessage(ev[i], gMessage, sizeof gMessage);
      gMessageChanged = true;
    }
    if (!out || !vdm::eventReachesMqtt(ev[i])) continue;
    vdm::PublishEvent pe;
    if (ev[i].valve < vdm::kValveCount) {
      if (gAggregator.offer(ev[i], now, pe)) publishEvent(pe, now);
    } else {
      pe.event = ev[i];
      publishEvent(pe, now);
    }
  }
  vdm::PublishEvent pe;
  while (out && gAggregator.poll(now, pe)) publishEvent(pe, now);
  portENTER_CRITICAL(&gMux);
  gStatus.eventsSuppressed = gEventLimiter.suppressed() + gAggregator.duplicates();
  portEXIT_CRITICAL(&gMux);
}

// ---------------------------------------------------------------- connection

void resetPublishedState() {
  for (bool& b : gPubValveValid) b = false;
  for (SensorPub& p : gPubTemp) p = SensorPub{};
  for (SensorPub& p : gPubVolt) p = SensorPub{};
  for (DiagPub& d : gPubDiag) d = DiagPub{};
  gPubStm = StmPub{};
  gPubState = 0xFF;
  gPubUptime = UINT32_MAX;
  gPubStmOnline = -1;
  gPubFailsafe = -1;
  gMessageChanged = true;
  gFirstPublish = true;
  gFullRunning = false;
  gFullCursor = 0;
}

bool connect(uint32_t now) {
  if (gCfg.mqtt.host[0] == '\0' || gLwtTopic[0] == '\0') {
    setState(vdm::MqttState::Error);
    return false;
  }
  vdm::copyString(gHost, sizeof gHost, gCfg.mqtt.host);
  gClient.setServer(gHost, gCfg.mqtt.port);
  gClient.setKeepAlive(gCfg.mqtt.keepAliveS);
  gClient.setSocketTimeout(kSocketTimeoutS);
  gClient.setCallback(onMessage);
  const bool auth = gCfg.mqtt.user[0] != '\0' && gCfg.mqtt.password[0] != '\0';
  setState(vdm::MqttState::Connecting);
  esp_task_wdt_reset();  // connect may take up to 3 s TCP + 5 s CONNACK
  const bool ok = gClient.connect(gClientId, auth ? gCfg.mqtt.user : nullptr,
                                  auth ? gCfg.mqtt.password : nullptr, gLwtTopic, 0, true,
                                  "offline", gCleanNext);
  esp_task_wdt_reset();
  if (!ok) {
    setState(vdm::MqttState::Error, static_cast<int8_t>(gClient.state()));
    return false;
  }
  gCleanNext = false;
  gPacer.onConnected(now);
  resetPublishedState();
  gInboundCount = 0;
  gEcho.reset();
  gButtons.reset();
  publishRaw(gLwtTopic, "online", true);
  static vdm::Subscription subs[vdm::kMaxSubscriptions];
  const size_t n = vdm::buildSubscriptions(gTopics, gCfg.mqtt.mode, gCfg.mqtt.discoveryPrefix,
                                           gSegments, subs, vdm::kMaxSubscriptions);
  for (size_t i = 0; i < n; ++i) gClient.subscribe(subs[i].filter, subs[i].qos);
  gScheduler.onConnected(now);
  gConnected = true;
  gOfflineSent = false;
  portENTER_CRITICAL(&gMux);
  vdm::copyString(gStatus.clientId, sizeof gStatus.clientId, gClientId);
  portEXIT_CRITICAL(&gMux);
  setState(vdm::MqttState::Connected);
  count(&Status::reconnects);
  logger::log(vdm::EventCode::MqttConnected);
  startOnConnect();
  return true;
}

void onDisconnected() {
  if (!gConnected) return;
  gConnected = false;
  gPacer.onDropped(app::nowMs());
  if (gRun.running()) gRun.abort(gPort);
  gPort.close();
  setDiscoveryRunning(false);
  gAggregator.reset();
  gButtons.reset();
  gInboundCount = 0;
  logger::log(vdm::EventCode::MqttDisconnected, vdm::kNoValve, gClient.state());
  setState(vdm::MqttState::Connecting, static_cast<int8_t>(gClient.state()));
}

void disconnectClean() {
  if (gClient.connected()) {
    publishRaw(gLwtTopic, "offline", true);
    gClient.disconnect();
  }
  onDisconnected();
}

}  // namespace

void begin() {
  gClient.setBufferSize(kBufferSize);  // allocated once, never resized
  gRegulator.restore(vdm::decodeHaStatusRecord(gRtcHaStatus));
  reloadConfig();
  updateRegulator();
  storeHaStatus();
}

void task(void*) {
  esp_task_wdt_add(nullptr);
  gEventCursor = 0;  // events since boot feed common/message and <main>events
  for (;;) {
    esp_task_wdt_reset();
    const uint32_t now = app::nowMs();
    if (storage::configRevision() != gCfgRevision) reloadConfig();
    observe();
    updateRegulator();

    if (ota::restartPending()) {
      if (!gOfflineSent) {
        gOfflineSent = true;
        disconnectClean();
        setState(vdm::MqttState::Disabled);
      }
      vTaskDelay(pdMS_TO_TICKS(100));
      continue;
    }
    if (gCfg.mqtt.mode == vdm::MqttMode::Off || !net::isUp()) {
      disconnectClean();
      setState(gCfg.mqtt.mode == vdm::MqttMode::Off ? vdm::MqttState::Disabled
                                                     : vdm::MqttState::Connecting);
      serviceEvents(now);  // keep the message tracking and cursor current
      vTaskDelay(pdMS_TO_TICKS(500));
      continue;
    }
    if (gReconnectRequested) {
      gReconnectRequested = false;
      disconnectClean();
      gPacer.forceNow();
    }
    if (!gClient.connected()) {
      onDisconnected();
      if (gPacer.due(now)) {
        if (!connect(now)) gPacer.onAttemptFailed(app::nowMs());
      }
      updateRegulator();
      serviceEvents(now);
      vTaskDelay(pdMS_TO_TICKS(100));
      continue;
    }
    gPacer.tick(now, true);
    if (gDiscoveryRequested) {
      gDiscoveryRequested = false;
      startRequested(gDiscoveryAction);
    } else {
      checkDiscoveryInputs();
    }
    pump();
    serviceEvents(now);
    serviceFullPublish(now);
    pump();
    serviceOnChange(now);
    serviceDiag(now);
    serviceDiscovery();
    vTaskDelay(pdMS_TO_TICKS(kDiscoveryPaceMs));
  }
}

Status status() {
  portENTER_CRITICAL(&gMux);
  const Status s = gStatus;
  portEXIT_CRITICAL(&gMux);
  return s;
}

vdm::RegulatorInput regulatorState() {
  portENTER_CRITICAL(&gMux);
  const vdm::RegulatorInput r = gRegulatorState;
  portEXIT_CRITICAL(&gMux);
  return r;
}

bool calibrationEnd(uint8_t valve, vdm::LocalTime& out) {
  portENTER_CRITICAL(&gMux);
  const bool ended = gCalib.ended(valve);
  if (ended) out = gCalib.end(valve);
  portEXIT_CRITICAL(&gMux);
  return ended;
}

void requestReconnect() { gReconnectRequested = true; }

void requestDiscovery(DiscoveryAction a) {
  gDiscoveryAction = a;
  gDiscoveryRequested = true;
}

}  // namespace mqtt
