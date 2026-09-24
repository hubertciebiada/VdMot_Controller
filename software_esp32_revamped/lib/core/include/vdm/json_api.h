// HTTP API document builders (/api/*). They take plain snapshot structs so
// they are host-testable; the web glue fills the snapshots under the
// respective locks and streams the result. Hardware-free.
// DESIGN.md "HTTP API" is the binding field list.
#pragma once

#include <stddef.h>
#include <stdint.h>

#include "vdm/config.h"
#include "vdm/event_log.h"
#include "vdm/json_writer.h"
#include "vdm/link_policy.h"
#include "vdm/stm_flasher.h"
#include "vdm/valve_model.h"
#include "vdm/version.h"

namespace vdm {

enum class NetState : uint8_t { Down = 0, Ethernet = 1, Wifi = 2 };
enum class MqttState : uint8_t { Disabled = 0, Connecting = 1, Connected = 2, Error = 3 };
const char* netStateName(NetState s);    // "down","ethernet","wifi"
const char* mqttStateName(MqttState s);  // "disabled","connecting","connected","error"

// Everything /api/status shows. Filled by the web glue from the app, net,
// mqtt and stm_link snapshots.
struct StatusSnapshot {
  // ESP
  const char* espVersion = "";      // firmwareVersion()
  uint32_t buildEpoch = 0;          // VDM_BUILD_EPOCH (dev builds), 0 = none
  uint32_t uptimeS = 0;
  uint8_t resetReason = 0;          // esp_reset_reason()
  uint32_t bootCount = 0;
  uint32_t freeHeap = 0, minFreeHeap = 0, largestFreeBlock = 0;
  uint32_t sketchSize = 0, sketchSpace = 0;  // app image size / partition size
  // time
  bool timeValid = false;
  int64_t epoch = 0;
  LocalTime local;
  uint32_t lastSyncEpoch = 0;
  // network
  NetState net = NetState::Down;
  uint32_t ip = 0, mask = 0, gateway = 0, dns = 0;
  char mac[18] = {0};
  int8_t wifiRssi = 0;
  char hostname[kStationNameMax + 1] = {0};
  // mqtt
  MqttState mqtt = MqttState::Disabled;
  int8_t mqttRc = 0;                // PubSubClient state()
  uint32_t mqttReconnects = 0;
  uint32_t mqttPublishFailures = 0;
  // stm
  LinkState link = LinkState::Unknown;
  LinkStats linkStats;
  uint8_t stmProto = 0;
  Version stmVersion;
  uint32_t stmBuild = 0;
  uint16_t stmHwId = 0;
  bool stmCompatible = true;        // >= minStmVersion() or unknown
  bool haveStmStatus = false;
  StmStatus stmStatus;              // v2 gstat
  uint32_t espLineOverflows = 0, espLineMalformed = 0;
  bool calibrationActive = false;
  int64_t lastScheduledCalibEpoch = 0;
  uint32_t nextCalibSlot = 0;       // yyyymmdd, 0 = none
  bool authEnabled = false;
  uint32_t lastEventSeq = 0;
};
// {"esp":{"version":..,"build":..,"uptime":..,"resetReason":"..","boots":..,
//  "heap":{"free":..,"min":..,"largest":..},"flash":{"used":..,"size":..}},
//  "time":{"valid":..,"epoch":..,"local":"2026-09-23T14:03:05","lastSync":..},
//  "net":{"state":"ethernet","ip":"..","mask":"..","gw":"..","dns":"..","mac":"..","rssi":..,"hostname":".."},
//  "mqtt":{"state":"connected","rc":0,"reconnects":..,"publishFailures":..},
//  "stm":{"link":"up","proto":2,"version":"2.0.0-revamped_C2","build":..,"hwId":"0x431",
//         "chip":"STM32F411xx","compatible":true,"minVersion":"1.4.0",
//         "stats":{...LinkStats...},"status":{...gstat...}|null,
//         "espRx":{"overflow":..,"malformed":..}},
//  "calibration":{"active":..,"lastScheduled":..,"nextSlot":..},
//  "auth":..,"lastEventSeq":..}
// Unknown values are null: esp.build and stm.build 0, time.epoch/local
// while !timeValid, lastSync 0, net.rssi unless on WiFi, stm.proto 0, an
// invalid stm.version, hwId/chip for hwId 0, lastScheduled <= 0, nextSlot 0.
// resetReason is the esp_reset_reason_t name ("poweron","task_wdt",...).
// Builders return jw.ok() (writeErrorJson: jw.complete()).
bool writeStatusJson(JsonWriter& jw, const StatusSnapshot& s);

// Per-valve view: model state + config + sensor names/values resolved.
struct ValveView {
  const ValveState* state = nullptr;
  const ValveConfig* config = nullptr;
  // Resolved sensors (slot 1-based, 0 = none); temp in tenths when valid.
  uint8_t sensorSlot[2] = {0, 0};
  const char* sensorName[2] = {"", ""};
  bool sensorValid[2] = {false, false};
  int32_t sensorTenths[2] = {0, 0};   // raw + slot offset
};
// {"valves":[{"idx":1..12,"name":"..","active":..,"known":..,"state":<n>,
//  "stateKey":"idle","calibrating":..,"pos":..,"target":..|null,
//  "targetSource":"web","sync":"synced","stmTarget":..|null,"meanCur":..,
//  "moves":..,"oc":..,"cc":..,"dc":..,"cr":..,"health":[..flag names..],
//  "age":<s since last data>|null,
//  "sensors":[{"slot":..,"name":"..","temp":21.5|null}, ...],
//  "ext":null|{"calState":0..3,"calEarlyStop":..,"calLastFailed":..,
//        "earlyStops":..,"cmdRejected":..,
//        "lastMove":{"dir":"open","req":..,"cnt":..,"stop":"endstop","peak":..,"ms":..},
//        "moveSeq":..}}, ... 12 entries always]}
// One entry per view (the glue passes all 12), idx = array position + 1.
// "health" lists HealthFlag names in bit order: "blocked","failed","noValve",
// "calibRetries","earlyStop","cmdRejected","stale","targetUnconfirmed",
// "tempFailed". "sensors" lists only assigned slots (slot != 0). "peak" is
// in mA with one decimal. A null state/config pointer renders as empty.
bool writeValvesJson(JsonWriter& jw, const ValveView* views, uint8_t count, uint32_t nowMs);

// {"valve":n,"count":k,"samples":[[count,current_mA_x10],...]}
bool writeProfileJson(JsonWriter& jw, const Profile& p);

// Sensors: every discovered bus sensor + every configured slot.
struct SensorView {
  uint8_t slot = 0;                 // 1-based config slot, 0 = not configured
  const char* name = "";
  bool active = false;
  bool onBus = false;               // present in the last gonec/gowvc list
  OneWireId id;
  bool valid = false;
  int32_t raw = 0;                  // STM raw (tenths / 10 mV)
  int32_t value = 0;                // temps: tenths incl. offset; volts: milli-units
  uint32_t ageS = 0;
  uint8_t valve = kNoValve;         // temps: valve using it (0-based), else kNoValve
  const char* unit = "";            // volts
};
// {"temps":[{"slot":..,"name":..,"id":"28-..","active":..,"onBus":..,
//   "temp":21.5|null,"raw":..,"age":..,"valve":n|null}],
//  "volts":[{"slot":..,"name":..,"id":..,"active":..,"onBus":..,
//   "value":12.345|null,"unit":"V","raw":..,"age":..}]}
// "slot" is null for bus sensors without a config slot, "id" "" when zero.
bool writeSensorsJson(JsonWriter& jw, const SensorView* temps, uint8_t tempCount,
                      const SensorView* volts, uint8_t voltCount);

// {"first":..,"last":..,"next":..,"dropped":..,"events":[writeEventJson...]}
// `events` are already filtered (EventLog::read).
bool writeEventsJson(JsonWriter& jw, const Event* events, size_t count, uint32_t firstSeq,
                     uint32_t lastSeq, uint32_t nextSince, uint32_t dropped);

// {"phase":"writing","status":4,"percent":..,"bytesDone":..,"bytesTotal":..,
//  "chipId":"0x431","chipName":"..","bootloaderVersion":"3.1","attempt":..,
//  "error":null|{"code":"nack","phase":"writing","addr":"0x08000100"},
//  "startedMs":..,"finishedMs":..,"image":{"name":..,"size":..,"crc32":"0x..","version":".."},
//  "appVersion":".."|null}
// chipId/chipName null before GetId; bootloaderVersion "<hi>.<lo>" nibbles
// of the GET byte (0x31 -> "3.1"), null when 0; "image" null when neither a
// name nor a validated image is known; image.version null when empty.
bool writeFlashStatusJson(JsonWriter& jw, const FlashStatus& s, const char* imageName);

// Motor/STM parameters: {"motor":{"lowC":..,"highC":..,"startOnPower":..,
// "noOfMinCount":..,"maxCalReps":..},"learnMovements":..,
// "breakaway":null|{"enable":..,"stepPct":..,"maxmA":..},"known":..}
bool writeMotorJson(JsonWriter& jw, const MotorChars& m, uint16_t learnMovements,
                    const Breakaway* breakaway, bool known);

// Uniform error body: {"error":"<code>","detail":"<text>"} (HTTP 4xx/5xx).
bool writeErrorJson(JsonWriter& jw, const char* code, const char* detail);

// ---------------------------------------------------------------- routing

enum class HttpMethod : uint8_t { Get, Post, Delete, Other };

// Every /api endpoint (DESIGN.md "HTTP API" is the binding table).
enum class ApiRoute : uint8_t {
  NotFound,          // 404
  MethodNotAllowed,  // 405 (path known, method not)
  Status,            // GET  /api/status
  Valves,            // GET  /api/valves
  ValveTarget,       // POST /api/valves/{n}/target        {"target":0..100}
  ValveCalibrate,    // POST /api/valves/{n}/calibrate
  ValveAssembly,     // POST /api/valves/{n}/assembly
  ValveServiceMove,  // POST /api/valves/{n}/service-move  {"dir":"open|close","counts":1..10000,"maxmA":5..60}
  ValveSensors,      // POST /api/valves/{n}/sensors       {"slot1":0..34,"slot2":0..34}
  ValveProfile,      // GET  /api/valves/{n}/profile       last gprof (v2)
  ValveProfileRefresh,  // POST /api/valves/{n}/profile    request a fresh gprof (v2)
  CalibrateAll,      // POST /api/valves/calibrate
  AssemblyAll,       // POST /api/valves/assembly
  Detect,            // POST /api/valves/detect
  Sensors,           // GET  /api/sensors
  SensorsScan,       // POST /api/sensors/scan
  Events,            // GET  /api/events?since=&minSeverity=&valve=&limit=
  ConfigGet,         // GET  /api/config
  ConfigPatch,       // POST /api/config                   partial config JSON
  ConfigExport,      // GET  /api/config/export            full config (no secrets) as download
  Motor,             // GET  /api/stm/motor
  MotorSet,          // POST /api/stm/motor
  StmReset,          // POST /api/stm/reset
  StmImages,         // GET  /api/stm/images
  StmImageUpload,    // POST /api/stm/images               multipart
  StmImageDelete,    // DELETE /api/stm/images/{name}
  StmFlash,          // POST /api/stm/flash                {"image":"..","mode":"normal|blank","force":false}
  StmFlashStatus,    // GET  /api/stm/flash
  StmFlashAbort,     // POST /api/stm/flash/abort
  EspOta,            // POST /api/ota/esp                  multipart
  Reboot,            // POST /api/system/reboot
  FactoryReset,      // POST /api/system/factory-reset     {"confirm":"factory-reset"}
  MqttReconnect,     // POST /api/mqtt/reconnect
  MqttDiscovery,     // POST /api/mqtt/discovery           {"action":"publish|delete|republish"}
  LogDownload,       // GET  /api/log                      current + previous log file, text/plain
};

struct RouteMatch {
  ApiRoute route = ApiRoute::NotFound;
  uint8_t valve = kNoValve;  // 0-based, from {n} = 1..12
  char name[32] = {0};       // {name}: [A-Za-z0-9._-]{1,31}, no leading '.'
  bool needsAuth = true;     // when web auth is enabled
};

// Matches "/api/..." (no query string; a trailing '/' is not accepted).
// {n} must be 1..12 without leading zeros, else NotFound. needsAuth is false
// only for the read-only GET routes (Status, Valves, ValveProfile, Sensors,
// Events, Motor, StmFlashStatus; DESIGN.md "HTTP API" auth column "read")
// and only when protectRead is false. MethodNotAllowed keeps needsAuth true.
// A path containing NUL bytes is NotFound.
RouteMatch matchApiRoute(HttpMethod m, const char* path, size_t len, bool protectRead);

}  // namespace vdm
