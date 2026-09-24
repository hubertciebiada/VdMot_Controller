// Home Assistant MQTT discovery: the deterministic list of entity configs
// (KEEP entities with their legacy object ids and unique_ids, plus new
// diagnostic entities) and the list of legacy DROP entities to delete.
// Hardware-free. The MQTT glue iterates the list one message per loop pass
// (no blocking, no delay()).
#pragma once

#include <stddef.h>
#include <stdint.h>

#include "vdm/common.h"
#include "vdm/mqtt_topics.h"

namespace vdm {

class JsonWriter;

constexpr size_t kDiscoveryTopicMax = 127;
constexpr size_t kDiscoveryPayloadMax = 1023;  // PubSubClient buffer is 1280

// Everything discovery needs, filled by glue from Config + model snapshots.
struct DiscoveryContext {
  TopicContext topics;               // station, pathAsRoot, separate
  bool plainText = true;             // legacy publishPlainText
  bool publishDiag = true;           // legacy publishDiag (valves/<V>/diag/*)
  bool publishUptime = true;         // legacy publishUpTime
  bool publishAllTemps = true;       // legacy publishAllTemps
  bool newDiag = true;               // new diag/* + events entities
  char ip[16] = {0};                 // for configuration_url
  char swVersion[32] = {0};          // firmwareVersion()
  struct Valve {
    bool active = false;
    char segment[kSegmentMax + 1] = {0};  // buildSegment()
    bool hasTemp1 = false;           // STM reports an assigned sensor 1
    bool hasTemp2 = false;
    // False while the STM has not reported this valve's sensors yet (link
    // re-sync not settled): hasTemp1/2 are then a guess, and
    // discoveryTopicIsCurrent() keeps the valve's temp1/temp2 configs.
    bool tempsKnown = true;
  } valves[kValveCount];
  struct Sensor {
    bool active = false;             // configured, active, id present
    bool published = false;          // will actually be published (allTemps / unassigned)
    char segment[kSegmentMax + 1] = {0};
    char name[kItemNameMax + 1] = {0};
    char id[kOneWireIdTextLen + 1] = {0};  // unique_id source
    char unit[kUnitMax + 1] = {0};   // volts only
  } temps[kTempSlotCount], volts[kVoltSlotCount];
};

// HA components used.
enum class HaComponent : uint8_t { Sensor, BinarySensor, Text, Valve, Number, Select, Switch, Climate };
const char* haComponentName(HaComponent c);  // "sensor","binary_sensor",...

// One discovery message: topic "homeassistant/<component>/<station>/<objectId>/config",
// retained, payload JSON (empty payload = delete).
struct DiscoveryMessage {
  char topic[kDiscoveryTopicMax + 1] = {0};
  bool remove = false;  // true -> publish empty retained payload
};

// Iterates the current entity set in a fixed order: common (state, message,
// uptime*, ip), per active valve (state, target, actual, temp1*, temp2*,
// calibration.date, calibration.repetitions, diag.* when publishDiag,
// new diag entities when newDiag), temps*, volts*, stm diag entities,
// calibration-active binary sensor, link-state sensor.
// (* = gated as described in DESIGN.md "HA discovery".)
//
// KEEP entities keep the legacy component, object id, name and unique_id
// (spec 03 §7.3-§7.7): unique_id = "<station>.<...>" with ' ' -> '_';
// state topic "<main><path>/value", command "<main><path>/set". Changes vs
// legacy (no entity-id change in HA): JSON is escaped, invalid device_class
// "volume"/state_class on text/number/valve entities is dropped,
// availability (Status topic, "online"/"offline") is added to every entity,
// device.sw_version = firmwareVersion(), temps unit fixed "°C".
class DiscoveryIterator {
 public:
  explicit DiscoveryIterator(const DiscoveryContext& ctx);
  // Writes the next message: topic into msg, JSON payload into jw (nothing
  // for remove). Returns false when the list is exhausted or a message does
  // not fit (then jw.ok() is false and the entity is skipped by the caller
  // after logging).
  bool next(DiscoveryMessage& msg, JsonWriter& jw);
  void restart();
  uint16_t position() const { return pos_; }

 private:
  const DiscoveryContext& ctx_;
  uint16_t pos_ = 0;
};

// Iterates the legacy DROP entities to delete once (empty retained payload),
// for every valve index whether active or not and for both possible valve
// segments (name-based and 1-based index):
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

 private:
  const DiscoveryContext& ctx_;
  uint16_t pos_ = 0;
};

// True when `topic` (a line of the legacy /HADiscovery.cfg) is produced by
// the current DiscoveryIterator; lines that are not get deleted on the next
// "delete stale" run (renamed station/valves, dropped entities). The temp1/
// temp2 configs of an active valve without tempsKnown count as current: a
// deletion on a guess would drop the HA registry entry (renames, areas).
bool discoveryTopicIsCurrent(const DiscoveryContext& ctx, const char* topic, size_t len);

}  // namespace vdm
