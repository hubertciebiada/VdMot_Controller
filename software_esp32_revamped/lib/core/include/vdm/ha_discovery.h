// Home Assistant MQTT discovery: the deterministic list of entity configs
// (KEEP entities with their legacy object ids and unique_ids, plus the 2.1
// entities), the legacy DROP entities to delete, the classification of the
// lines of the discovery list /HADiscovery.cfg and the discovery run that
// prunes, publishes and rewrites that list. Hardware-free. The MQTT glue
// steps the run once per loop pass (no blocking, no delay()).
#pragma once

#include <stddef.h>
#include <stdint.h>

#include "vdm/common.h"
#include "vdm/config.h"
#include "vdm/mqtt_topics.h"
#include "vdm/valve_model.h"

namespace vdm {

class JsonWriter;

constexpr size_t kDiscoveryTopicMax = 127;
// The event entity lists every event type that reaches MQTT (about 1.8 KB
// with the 2.1 registry). PubSubClient buffer: mqtt::kBufferSize (2304).
constexpr size_t kDiscoveryPayloadMax = 2047;

// Everything discovery needs, filled by glue (buildDiscoveryContext) from the
// config and the STM snapshot.
struct DiscoveryContext {
  TopicContext topics;               // MQTT root, pathAsRoot, separate
  // Station name (device, node id, unique_ids); "" = topics.station.
  char station[kStationNameMax + 1] = {0};
  char discoveryPrefix[kTopicPrefixMax + 1] = "homeassistant";  // config topics
  bool plainText = true;             // legacy publishPlainText
  bool publishDiag = true;           // legacy publishDiag (valves/<V>/diag/*)
  bool publishUptime = true;         // legacy publishUpTime
  bool publishAllTemps = true;       // legacy publishAllTemps
  bool newDiag = true;               // new diag/* entities
  bool events = true;                // <main>events and its event entity
  uint16_t publishIntervalS = 10;    // expire_after of the numeric sensors
  bool stmV3 = false;                // STM protocol >= 3: payload_stop, stop and safe-exit buttons
  char ip[16] = {0};                 // for configuration_url
  char swVersion[32] = {0};          // firmwareVersion()
  char hwVersion[4] = {0};           // STM board tag ("C1", "C2"), "" = unknown (key omitted)
  struct Valve {
    bool active = false;
    char segment[kSegmentMax + 1] = {0};  // itemSegment()
    char name[kItemNameMax + 1] = {0};    // configured name (entity names), "" = "Valve <n>"
    bool hasTemp1 = false;           // STM reports an assigned sensor 1
    bool hasTemp2 = false;
    // False while the STM has not reported this valve's sensors yet (link
    // re-sync not settled): hasTemp1/2 are then a guess, and their configs are
    // KeptUnknown (never deleted on a guess).
    bool tempsKnown = true;
  } valves[kValveCount];
  struct Sensor {
    bool active = false;             // configured, active, id present
    bool published = false;          // will actually be published (allTemps / unassigned)
    char segment[kSegmentMax + 1] = {0};       // itemSegment(): object id (legacy slot form)
    char topicSegment[kSegmentMax + 1] = {0};  // published segment (E22: bus index when unnamed)
    bool topicKnown = true;          // false: unnamed and not on the bus (no config, KeptUnknown)
    char name[kItemNameMax + 1] = {0};
    char id[kOneWireIdTextLen + 1] = {0};  // unique_id source
    char unit[kUnitMax + 1] = {0};   // volts only
  } temps[kTempSlotCount], volts[kVoltSlotCount];
};

// What the glue feeds into buildDiscoveryContext().
struct DiscoveryInputs {
  const Config* cfg = nullptr;
  const ValveState* valves = nullptr;  // kValveCount entries
  const TempReading* temps = nullptr;
  uint8_t tempCount = 0;
  const VoltReading* volts = nullptr;
  uint8_t voltCount = 0;
  bool sensorsSettled = false;
  uint8_t stmProto = 0;
  const char* stmHw = "";              // Version::hw of gvers
  uint32_t ip = 0;                     // legacy NVS layout (formatIpv4)
  const char* swVersion = "";
};
// Fills every field of `out` (false and an empty context without cfg).
bool buildDiscoveryContext(const DiscoveryInputs& in, DiscoveryContext& out);
// CRC32 over the snapshot-dependent parts of the context (per valve
// hasTemp1/hasTemp2/tempsKnown, per sensor slot published/topicKnown/topic
// segment, hw, stmV3): the glue re-runs discovery when it changes.
uint32_t discoveryInputKey(const DiscoveryInputs& in);

// HA components used.
enum class HaComponent : uint8_t {
  Sensor, BinarySensor, Text, Valve, Number, Select, Switch, Climate, Button, Event,
};
const char* haComponentName(HaComponent c);  // "sensor","binary_sensor",...; "" out of range

// One discovery message: topic "<prefix>/<component>/<node>/<objectId>/config",
// retained, payload JSON (empty payload = delete).
struct DiscoveryMessage {
  char topic[kDiscoveryTopicMax + 1] = {0};
  bool remove = false;  // true -> publish empty retained payload
};

// Iterates the current entity set in a fixed order: common (state, message,
// uptime*, ip), per active valve 20 kinds (state, target, actual, temp1*,
// temp2*, calibration date and repetitions, diag x5*, early stops*, rejected
// commands*, last stop*, calibration state*, problem, failsafe, target
// delivery, calibrate button), temps*, volts*, then the device entities
// (ESP/STM online, failsafe, lease*, safe mode*, link*, protocol*, version*,
// started*, resets*, overflows*, parse errors*, calibration running*, next
// calibration*, suppressed events*, rejected commands*, events*, buttons
// calibrate all, detect, stop*, reset STM, restart ESP, leave safe mode*).
// (* = gated.)
//
// Node id = buildHaId(station), object ids from buildHaId(segment); names
// and unique_ids ("<station with ' ' -> '_'>.<uid>") keep the raw segment.
// KEEP entities (legacy component, object id, name and unique_id) carry no
// availability, so they keep working under the legacy firmware after a
// rollback; new entities name <main>status (esp) or <main>status and
// <main>stm/status (esp+stm).
class DiscoveryIterator {
 public:
  explicit DiscoveryIterator(const DiscoveryContext& ctx);
  // Writes the next message: topic into msg, JSON payload into jw. Returns
  // false when the list is exhausted or a message does not fit (then
  // jw.ok() is false, msg.topic is set when the entity got that far, and the
  // caller skips it).
  bool next(DiscoveryMessage& msg, JsonWriter& jw);
  // Next config topic only (no JSON); entities that cannot be built are
  // passed over. false at the end.
  bool nextTopic(DiscoveryMessage& msg);
  // The 2.0.0 form of the entity just produced by next()/nextTopic():
  // prefix "homeassistant", raw station, raw segment in the object id. false
  // when it equals the current form.
  bool v20Topic(DiscoveryMessage& msg) const;
  void restart();
  void reset(const DiscoveryContext& ctx);  // another context, position 0
  uint16_t position() const { return pos_; }

 private:
  const DiscoveryContext* ctx_;
  uint16_t pos_ = 0;
};

// Iterates the legacy DROP entities to delete once (empty retained payload),
// for every valve index whether active or not and for both possible valve
// segments (name-based and 1-based index), with the literal legacy prefix
// "homeassistant/" and the raw station:
//   climate/<st>/climate_<R>, number/<st>/valves_control_dynOffs_<R>,
//   number/<st>/valves_control_min_<R>, number/<st>/valves_control_max_<R>,
//   select/<st>/valves_window_state_<R>, switch/<st>/valves_window_state_<R>,
//   number/<st>/valves_window_target_<R>, select/<st>/heatControl,
//   number/<st>/parkPosition.
class DropListIterator {
 public:
  explicit DropListIterator(const DiscoveryContext& ctx);
  bool next(DiscoveryMessage& msg);  // msg.remove is always true
  void restart();
  void reset(const DiscoveryContext& ctx);

 private:
  const DiscoveryContext* ctx_;
  uint16_t pos_ = 0;
};

// What a line of the discovery list is to the current context.
enum class TopicClass : uint8_t {
  Foreign,      // not "<homeassistant|prefix>/<component>/<id>/<id>/config", wildcards,
                // control chars, > kDiscoveryTopicMax: dropped from the list, never published to
  Current,      // the config topic of an entity the iterator produces now
  KeptUnknown,  // the config topic of an entity kept on a guess: valve temp1/temp2 while
                // !tempsKnown, a temp/volt entity while !topicKnown (never deleted)
  Stale,        // anything else: deleted (renamed, deactivated, gated off, old prefix/station)
};
// Trailing CR, LF, space and tab of the line are ignored.
TopicClass classifyDiscoveryTopic(const DiscoveryContext& ctx, const char* topic, size_t len);

// Byte source and list files of a discovery run (glue: PubSubClient + LittleFS).
class DiscoveryPort {
 public:
  virtual bool publish(const char* topic, const char* payload) = 0;  // retained; false = conn lost
  virtual bool listOpen() = 0;                 // /HADiscovery.cfg for reading; false = missing
  virtual int listRead() = 0;                  // next byte, -1 at the end
  virtual void listClose() = 0;
  virtual bool listBegin() = 0;                // create/truncate /HADiscovery.cfg.tmp
  virtual bool listWrite(const char* topic) = 0;   // topic + '\n'
  virtual bool listCommit() = 0;               // rename tmp over the list (replace)
  virtual void listAbort() = 0;                // close and remove tmp

 protected:
  ~DiscoveryPort() = default;
};
// One line of the list from port.listRead(): CR/LF separated, empty lines
// skipped, lines longer than kDiscoveryTopicMax dropped; false at the end.
bool readListLine(DiscoveryPort& port, char (&out)[kDiscoveryTopicMax + 1]);

struct DiscoveryPlan {
  bool removeAll = false;   // actions Delete and DeleteAndPublish
  bool publish = false;     // Publish, DeleteAndPublish, automatic runs in mode 2
  bool dropLegacy = false;  // legacy DROP entities (first run)
  bool retire20 = false;    // 2.0.0 -> 2.1 migration (publish runs only)
  bool prune = true;        // false only for a pure Delete
};

// One discovery run: phases in this order, each only when the plan asks:
//  RemoveList (every usable list line -> ""), RemoveCurrent (every current
//  topic -> ""), ClearList (empty list; a pure Delete ends here), Retired
//  (diag_stm_uptime and the 2.0.0 forms of changed ids -> ""), DropList,
//  Prune (Stale lines -> "", Foreign dropped, KeptUnknown and, without
//  publish, Current lines carried), Publish (every config), then WriteKept /
//  WriteCurrent / Commit when the list changed (missing, a Stale or Foreign
//  line, other CRC32 or line count): the carried lines and then every
//  current topic into the tmp file, renamed over the list.
// A failed publish aborts the run (Aborted); a failed list write abandons
// only the list (listAbort), the run still ends Done. A context without a
// valid station ends the run at its first step (Done, nothing touched).
class DiscoveryRun {
 public:
  enum class Phase : uint8_t {
    Idle, RemoveList, RemoveCurrent, ClearList, Retired, DropList, Prune, Publish, WriteKept,
    WriteCurrent, Commit, Done, Aborted,
  };
  static constexpr uint8_t kLinesPerStep = 8;
  struct Stats {
    uint16_t configs = 0;
    uint16_t deletes = 0;
    uint16_t skipped = 0;
    bool listWritten = false;
  };
  DiscoveryRun();
  // ctx must stay unchanged until the run ends.
  void start(const DiscoveryContext& ctx, const DiscoveryPlan& plan);
  // At most one publish and at most kLinesPerStep list lines per call.
  Phase step(DiscoveryPort& port, JsonWriter& payload);
  void abort(DiscoveryPort& port);
  Phase phase() const { return phase_; }
  bool running() const;
  const Stats& stats() const { return stats_; }
  const DiscoveryPlan& plan() const { return plan_; }

 private:
  void enter(Phase p);
  void advance();
  void afterPublish();
  bool remove(DiscoveryPort& port, const char* topic);
  void finishList(DiscoveryPort& port, bool commit);

  const DiscoveryContext* ctx_ = nullptr;
  DiscoveryPlan plan_;
  Phase phase_ = Phase::Idle;
  bool entered_ = false;
  bool reading_ = false;   // the list is open for reading
  bool writing_ = false;   // the tmp list is open for writing
  bool uptimeDone_ = false;
  bool changed_ = false;
  uint32_t crcOld_ = 0;
  uint32_t crcNew_ = 0;
  uint16_t linesOld_ = 0;
  uint16_t linesNew_ = 0;
  DiscoveryIterator it_;
  DropListIterator drop_;
  Stats stats_;
};

}  // namespace vdm
