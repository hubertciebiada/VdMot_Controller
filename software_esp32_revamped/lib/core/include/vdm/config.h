// Configuration schema of the new ESP firmware: one plain struct, defaults,
// validation, a key-path setter used by HTTP/JSON import (glue parses JSON
// with ArduinoJson and feeds values here), JSON export, and a versioned
// binary encoding for NVS. Hardware-free.
//
// DESIGN.md "Config schema" is the binding table (key, type, range, default,
// legacy NVS source). Every change to this struct must update that table,
// kConfigSchemaVersion handling in decodeConfig(), and the tests.
#pragma once

#include <stddef.h>
#include <stdint.h>

#include "vdm/common.h"

namespace vdm {

class JsonWriter;

constexpr uint16_t kConfigSchemaVersion = 1;
constexpr size_t kSecretMax = 64;     // passwords (legacy char[65])
constexpr size_t kHostMax = 64;       // broker host, NTP server
constexpr size_t kTzNameMax = 49;     // legacy char[50]
constexpr size_t kTzPosixMax = 49;

enum class NetInterface : uint8_t { Auto = 0, Ethernet = 1, Wifi = 2 };
enum class MqttMode : uint8_t { Off = 0, Mqtt = 1, MqttHa = 2 };

struct NetConfig {
  NetInterface iface = NetInterface::Auto;
  bool dhcp = true;
  uint32_t ip = 0, mask = 0, gateway = 0, dns = 0;  // legacy uint32 layout (parseIpv4)
  char ssid[33] = {0};                // 1..32 chars (802.11 limit) or "" = WiFi off
  char wifiPassword[kSecretMax + 1] = {0};  // 8..63 chars (WPA2) when ssid set
  uint8_t reconnectTimeoutMin = 5;    // legacy netConnTO; 0 = never restart the ESP
};

struct TimeConfig {
  char ntpServer[kHostMax + 1] = "pool.ntp.org";  // "" disables SNTP
  char tzName[kTzNameMax + 1] = "Europe/Berlin";
  char tzPosix[kTzPosixMax + 1] = "CET-1CEST,M3.5.0,M10.5.0/3";
};

struct SyslogConfig {
  uint8_t level = 0;       // 0 off, 1 errors+warnings, 2 +info, 3 +debug
  uint32_t server = 0;     // IPv4; required non-zero when level > 0
  uint16_t port = 514;
};

struct WebConfig {
  char user[kSecretMax + 1] = {0};      // auth enabled iff user AND password non-empty
  char password[kSecretMax + 1] = {0};
  bool protectRead = false;             // also require auth for GET /api/* (not "/")
};

struct MqttConfig {
  MqttMode mode = MqttMode::Off;
  char host[kHostMax + 1] = {0};  // hostname or dotted IPv4
  uint16_t port = 1883;
  char user[kSecretMax + 1] = {0};  // used only when user AND password non-empty (legacy)
  char password[kSecretMax + 1] = {0};
  uint16_t keepAliveS = 60;          // 5..300
  uint16_t publishIntervalS = 10;    // 2..3600
  uint16_t minDelayS = 5;            // 0..publishIntervalS
  // legacy protocolFlags (brokerPF bit order)
  bool separate = true;       // bit0
  bool allTemps = true;       // bit1
  bool pathAsRoot = false;    // bit2
  bool upTime = true;         // bit3
  bool onChange = true;       // bit4
  bool retained = true;       // bit5
  bool plainText = true;      // bit6
  bool diag = true;           // bit7
  bool germanDecimal = false; // legacy brokerMQF bit2 numFormat
  // new
  bool newDiag = true;        // diag/* topics + HA entities
  bool events = true;         // <main>events
  bool haDiscoveryOnConnect = true;  // re-send discovery on connect and HA birth
};

struct ValveConfig {
  char name[kItemNameMax + 1] = {0};
  bool active = false;
};

struct TempSlotConfig {
  char name[kItemNameMax + 1] = {0};
  bool active = false;
  int16_t offset = 0;       // 0.1 C, -100..100 (+-10.0 C)
  OneWireId id;             // zero = empty slot
};

struct VoltSlotConfig {
  char name[kItemNameMax + 1] = {0};
  bool active = false;
  float offset = 0.0f;      // finite, -1000..1000
  float factor = 1.0f;      // finite, -1000..1000, != 0
  char unit[kUnitMax + 1] = {0};
  OneWireId id;
};

struct CalibScheduleConfig {
  uint8_t dayMask = 9;   // bit i = tm_wday i (bit0 Sunday); 0 disables; default Sun+Wed
  uint8_t hour = 0;      // 0..23 local time
  uint8_t minute = 0;    // 0..59 (new; legacy always :00)
};

struct Config {
  uint16_t schema = kConfigSchemaVersion;
  char station[kStationNameMax + 1] = "VdMot";  // hostname, MQTT client id/root, HA device
  NetConfig net;
  TimeConfig time;
  SyslogConfig syslog;
  WebConfig web;
  MqttConfig mqtt;
  ValveConfig valves[kValveCount];
  TempSlotConfig temps[kTempSlotCount];
  VoltSlotConfig volts[kVoltSlotCount];
  CalibScheduleConfig calib;
  bool persistLog = true;  // write the event log to LittleFS (2 x 64 KB rotation)
};

// Factory defaults (the member initialisers above).
void setDefaults(Config& c);

// Validation of a whole config. Returns true when every field is within its
// documented range and the cross-field rules hold:
//  - static IP: when !dhcp, ip/mask/gateway non-zero and mask contiguous;
//  - ssid set -> wifiPassword 8..63 chars; iface Wifi -> ssid set;
//  - syslog level > 0 -> server != 0 and port != 0;
//  - web: user and password both empty or both non-empty; user without ':'
//    (HTTP Basic); ssid printable ASCII;
//  - mqtt mode != Off -> host valid (isHostName or IPv4), port != 0;
//    minDelayS <= publishIntervalS; mode MqttHa -> separate == true;
//  - names: isSafeName (station 1..20, others 0..10); duplicate non-empty
//    valve names are rejected (MQTT segment collision), also a valve named
//    like the number segment of another valve ("3" vs unnamed valve 3);
//  - temp/volt slots: two slots must not share a non-zero id; active slot
//    needs an id.
// On failure `path` receives the first offending key path ("mqtt.port",
// "valves.3.name"; indices 1-based) when non-null.
bool validateConfig(const Config& c, char* path, size_t pathCap);

// ---------------------------------------------------------------- key paths

// A JSON scalar handed over by glue (ArduinoJson variant -> ConfigValue).
struct ConfigValue {
  enum class Type : uint8_t { Null, Bool, Int, Float, String } type = Type::Null;
  bool b = false;
  int64_t i = 0;
  double f = 0.0;
  const char* s = nullptr;  // not owned, NUL-terminated
  size_t len = 0;
};

enum class SetResult : uint8_t {
  Ok,
  UnknownKey,
  WrongType,    // e.g. string for a bool; numbers given as strings are accepted
                // for numeric keys (legacy UI sends "3"), bools accept 0/1
  OutOfRange,   // violates the per-field range (cross-field rules are checked
                // by validateConfig after the whole patch)
  ReadOnly,     // "schema" with a value other than kConfigSchemaVersion
};
const char* setResultName(SetResult r);

// Sets one field by dotted path, 1-based array indices:
//   "station", "net.dhcp", "net.ip" (dotted string), "mqtt.port",
//   "valves.3.name", "temps.12.offset" (float C, rounded to 0.1),
//   "temps.12.id" ("hh-..." or "" to clear), "calib.dayMask", ...
// The complete key list is DESIGN.md "Config schema". Secrets
// ("*.password", "net.wifiPassword") are write-only; an empty string for a
// secret means "unchanged" unless `clearSecrets` is true.
// So that an exported document can be posted back unchanged, "schema" equal
// to kConfigSchemaVersion and the export's "<secret>Set" booleans
// ("net.wifiPasswordSet", "web.passwordSet", "mqtt.passwordSet") are
// accepted as no-ops. Paths are at most 64 chars; array indices are
// 1..N without leading zeros.
SetResult setConfigValue(Config& c, const char* path, const ConfigValue& v, bool clearSecrets);

// ---------------------------------------------------------------- JSON patch

constexpr uint8_t kConfigJsonMaxDepth = 8;  // nesting limit of a patch document

enum class PatchResult : uint8_t {
  Ok,
  Malformed,   // not a JSON object / syntax error / nested deeper than kConfigJsonMaxDepth
  UnknownKey,  // setConfigValue results for the first failing key
  WrongType,
  OutOfRange,
  ReadOnly,
  Invalid,     // every key applied, but validateConfig failed
};
// "ok","malformed","unknown_key","wrong_type","out_of_range","read_only","invalid"
const char* patchResultName(PatchResult r);

// POST /api/config: applies a partial config document to `c` in place, so
// the caller passes a copy of the active config and commits it only on Ok.
// Structure: the one writeConfigJson emits (nested objects, arrays with
// element i -> path index i+1), nested objects keyed by index
// ({"valves":{"3":{"name":"x"}}}) or dotted keys ({"valves.3.name":"x"}),
// in any mix. A root "clearSecrets": true (bool only) makes empty secrets
// clear the stored ones. Pass 1 checks the whole syntax (strict RFC 8259:
// escapes incl. surrogate pairs, number grammar, no trailing data) before
// anything is changed; pass 2 applies every scalar with setConfigValue and
// stops at the first failure; then validateConfig runs. On failure `path`
// receives the offending key path, or "@<byte offset>" for Malformed.
// No heap, bounded recursion, strings longer than 96 bytes are truncated
// (and then rejected as out of range by every string field). Escapes of
// non-ASCII characters are validated but not decoded (no field accepts
// them); they are rejected as out of range like raw non-ASCII bytes.
PatchResult applyConfigJson(Config& c, const char* json, size_t len, char* path, size_t pathCap);

// JSON export in the same key structure as setConfigValue, e.g.
// {"schema":1,"station":"VdMot","net":{...},"valves":[{"name":..},...],...}.
// Secrets are never written; instead "<key>Set":true/false is emitted
// (e.g. "passwordSet"). Returns jw.ok().
bool writeConfigJson(JsonWriter& jw, const Config& c);

// ---------------------------------------------------------------- binary

// NVS encoding: magic "VDMC", u16 schema, u16 payload length, payload
// (explicit little-endian field-by-field encoding, strings as u8 length +
// bytes), u32 CRC32 of everything before it. Never a raw struct dump.
constexpr size_t kConfigBlobMax = 4096;
// Returns bytes written, 0 when cap is too small.
size_t encodeConfig(const Config& c, uint8_t* out, size_t cap);

enum class DecodeResult : uint8_t { Ok, TooShort, BadMagic, BadCrc, UnsupportedSchema, Invalid };
// Decodes into `out` (defaults first, then fields). Older schema versions
// are migrated; a newer schema is rejected (UnsupportedSchema) so a
// downgrade keeps the NVS blob untouched. The decoded config must pass
// validateConfig, else Invalid.
DecodeResult decodeConfig(const uint8_t* data, size_t len, Config& out);

// CRC-32 (IEEE 802.3, reflected, init/xorout 0xFFFFFFFF; zlib crc32()
// semantics: pass the previous result as `crc` to continue over more data).
// Also used by the flasher.
uint32_t crc32(const uint8_t* data, size_t len, uint32_t crc = 0);

}  // namespace vdm
