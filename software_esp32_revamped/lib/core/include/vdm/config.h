// Configuration schema of the new ESP firmware: one plain struct, defaults,
// validation, a key-path setter used by HTTP/JSON import (glue parses JSON
// with ArduinoJson and feeds values here), JSON export, and versioned
// binary encodings for NVS. Hardware-free.
//
// DESIGN.md "Config schema" is the binding table (key, type, range, default,
// legacy NVS source). Every change to this struct must update that table,
// the schema numbers below and the tests.
#pragma once

#include <stddef.h>
#include <stdint.h>

#include "vdm/common.h"
#include "vdm/failsafe.h"

namespace vdm {

class JsonWriter;

// Header of the NVS `cfg` blob: frozen forever (ESP 2.0.0 reads it). New keys
// go to the `cfgx` blob with a new, never reused tag.
constexpr uint16_t kConfigBaseSchema = 1;
// "schema" of JSON documents and Config::schema.
constexpr uint16_t kConfigJsonSchema = 2;
constexpr size_t kSecretMax = 64;     // passwords (legacy char[65])
constexpr size_t kHostMax = 64;       // broker host, NTP server
constexpr size_t kTzNameMax = 49;     // legacy char[50]
constexpr size_t kTzPosixMax = 49;
constexpr size_t kAllowedHostsMax = 80;  // below the patch reader's 96: a cut string stays invalid
constexpr size_t kClientIdMax = 64;
constexpr size_t kTopicPrefixMax = 32;

enum class NetInterface : uint8_t { Auto = 0, Ethernet = 1, Wifi = 2 };
enum class MqttMode : uint8_t { Off = 0, Mqtt = 1, MqttHa = 2 };

struct NetConfig {
  NetInterface iface = NetInterface::Auto;
  bool dhcp = true;
  uint32_t ip = 0, mask = 0, gateway = 0, dns = 0;  // legacy uint32 layout (parseIpv4)
  char ssid[33] = {0};                // 1..32 chars (802.11 limit) or "" = WiFi off
  char wifiPassword[kSecretMax + 1] = {0};  // "" (open network) or 8..63 chars (WPA2)
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
  // Host header values the request guard accepts besides the interface IP
  // and the host name: "" or 1..4 entries separated by ','.
  char allowedHosts[kAllowedHostsMax + 1] = {0};
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
  char rootTopic[kStationNameMax + 1] = {0};  // "" = the station name (mqttRootTopic())
  char clientId[kClientIdMax + 1] = {0};      // "" = automatic
  char discoveryPrefix[kTopicPrefixMax + 1] = "homeassistant";
};

struct ValveConfig {
  char name[kItemNameMax + 1] = {0};
  bool active = false;
  uint8_t failsafePct = kFailsafePctDefault;  // 0..100 or kFailsafeHold
  char topic[kItemNameMax + 1] = {0};         // MQTT segment override, "" = from the name
};

struct TempSlotConfig {
  char name[kItemNameMax + 1] = {0};
  bool active = false;
  int16_t offset = 0;       // 0.1 C, -100..100 (+-10.0 C)
  OneWireId id;             // zero = empty slot
  char topic[kItemNameMax + 1] = {0};
};

struct VoltSlotConfig {
  char name[kItemNameMax + 1] = {0};
  bool active = false;
  float offset = 0.0f;      // finite, -1000..1000
  float factor = 1.0f;      // finite, -1000..1000, != 0
  char unit[kUnitMax + 1] = {0};
  OneWireId id;
  char topic[kItemNameMax + 1] = {0};
};

struct CalibScheduleConfig {
  uint8_t dayMask = 9;   // bit i = tm_wday i (bit0 Sunday); 0 disables; default Sun+Wed
  uint8_t hour = 0;      // 0..23 local time
  uint8_t minute = 0;    // 0..59 (new; legacy always :00)
};

struct FailsafeConfig {
  uint16_t timeoutMin = kFailsafeTimeoutDefaultMin;  // 0 (off) or 5..1440
};

struct Config {
  uint16_t schema = kConfigJsonSchema;
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
  FailsafeConfig failsafe;
  bool persistLog = true;  // write the event log to LittleFS (2 x 64 KB rotation)
};

// Factory defaults (the member initialisers above).
void setDefaults(Config& c);

// Validation of a whole config. Returns true when every field is within its
// documented range and the cross-field rules hold:
//  - static IP: when !dhcp, ip/mask/gateway non-zero and mask contiguous;
//  - ssid set -> wifiPassword "" (open network) or 8..63 chars; iface Wifi
//    -> ssid set;
//  - syslog level > 0 -> server != 0 and port != 0;
//  - web: user and password both empty or both non-empty; user without ':'
//    (HTTP Basic); ssid and secrets printable text (ASCII or UTF-8);
//  - mqtt mode != Off -> host valid (isHostName or IPv4), port != 0;
//    minDelayS <= publishIntervalS; mode MqttHa -> separate == true;
//  - names: isSafeName (station 1..20, others 0..10); duplicate non-empty
//    valve names are rejected (MQTT segment collision), also a valve named
//    like the number segment of another valve ("3" vs unnamed valve 3);
//  - temp/volt slots: two slots must not share a non-zero id; active slot
//    needs an id;
//  - V1: mode MqttHa -> germanDecimal false (HA reads a decimal point);
//  - V2: the valves' MQTT segments (itemSegment) are pairwise different
//    (path: the later valve's topic when it has one, else its name);
//  - V3: buildHaId() of the segments is unique among the valves, among the
//    active temp slots with an id and among the active volt slots with an id
//    (path as V2).
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
  ReadOnly,     // "schema" with a value other than 1..kConfigJsonSchema
};
const char* setResultName(SetResult r);

// Sets one field by dotted path, 1-based array indices:
//   "station", "net.dhcp", "net.ip" (dotted string), "mqtt.port",
//   "valves.3.name", "temps.12.offset" (float C, rounded to 0.1),
//   "temps.12.id" ("hh-..." or "" to clear), "calib.dayMask", ...
// The complete key list is DESIGN.md "Config schema". Secrets
// ("*.password", "net.wifiPassword") are write-only; an empty string for a
// secret means "unchanged" unless `clearSecrets` is true.
// So that an exported document can be posted back unchanged, "schema" 1..
// kConfigJsonSchema (a 2.0.0 export says 1) and the export's "<secret>Set"
// booleans ("net.wifiPasswordSet", "web.passwordSet", "mqtt.passwordSet")
// are accepted as no-ops. Paths are at most 64 chars; array indices are
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
// (and then rejected as out of range by every string field). \uXXXX escapes
// (surrogate pairs included) are decoded to UTF-8 like raw UTF-8 bytes; the
// field rules decide (names, SSIDs and secrets accept UTF-8 text, control
// characters and invalid sequences are out of range).
PatchResult applyConfigJson(Config& c, const char* json, size_t len, char* path, size_t pathCap);

// Secrets in the export: Flags writes "<key>Set":true/false instead of the
// value (e.g. "passwordSet"); Clear writes the secret itself and no
// "...Set" member (downloaded backups).
enum class SecretMode : uint8_t { Flags, Clear };
// What POST /api/config reports about a saved document.
struct ApplyInfo {
  bool restartRequired = false;  // configRestartReasons() != 0
  bool netTrial = false;         // netTrialRequired()
};
// JSON export in the same key structure as setConfigValue, e.g.
// {"schema":2,"station":"VdMot","net":{...},"valves":[{"name":..},...],...}.
// apply != nullptr appends "restartRequired" and "netTrial" as the last two
// members of the root object. Returns jw.ok().
bool writeConfigJson(JsonWriter& jw, const Config& c, SecretMode secrets = SecretMode::Flags,
                     const ApplyInfo* apply = nullptr);

// ---------------------------------------------------------------- helpers

enum class ItemKind : uint8_t { Valve, Temp, Volt };
// MQTT topic segment of one item: the `topic` override when set, else the
// name with ' ' -> '_', else the 1-based index ("3"). Returns chars
// written; 0 and out = "" when it does not fit or idx0 is out of range.
size_t itemSegment(const Config& c, ItemKind kind, uint8_t idx0, char* out, size_t cap);
// MQTT root: mqtt.rootTopic when set, else the station name.
const char* mqttRootTopic(const Config& c);
// DNS server of the network settings: with a static IP net.dns, or the
// gateway when dns is 0; with DHCP net.dns.
uint32_t effectiveDns(const NetConfig& n);
// A change that can make the device unreachable (network trial): iface or
// dhcp differ; with !after.dhcp ip, mask, gateway or effectiveDns differ;
// ssid or wifiPassword differ unless iface is Ethernet in both.
bool netTrialRequired(const NetConfig& before, const NetConfig& after);
enum RestartReason : uint8_t { kRestartNetwork = 0x01, kRestartHostname = 0x02 };
// Why applying `after` over `before` needs an ESP restart:
// kRestartNetwork = netTrialRequired(), kRestartHostname = buildHostname()
// of the stations differs. The single rule for net::reconfigure (restart
// iff != 0) and the POST /api/config answer.
uint8_t configRestartReasons(const Config& before, const Config& after);
// True when the MQTT topics or the session differ: station, every mqtt.*
// field, and per valve name/active/topic, per temp and volt slot
// name/active/topic/id. Not failsafe.timeoutMin, not valves.N.failsafePct
// (they apply live). True -> reconnect with a clean session.
bool mqttTopicConfigChanged(const Config& a, const Config& b);

// ---------------------------------------------------------------- binary

// NVS `cfg` blob: magic "VDMC", u16 kConfigBaseSchema, u16 payload length,
// payload (explicit little-endian field-by-field encoding, strings as u8
// length + bytes), u32 CRC32 of everything before it. Never a raw struct
// dump. Holds exactly the fields of ESP 2.0.0 in their order.
constexpr size_t kConfigBlobMax = 4096;
// Returns bytes written, 0 when cap is too small.
size_t encodeConfig(const Config& c, uint8_t* out, size_t cap);

enum class DecodeResult : uint8_t { Ok, TooShort, BadMagic, BadCrc, UnsupportedSchema, Invalid };

// Repairs that make a loaded config valid (sanitizeConfig).
enum RepairBit : uint32_t {
  kRepairField = 1u << 0,         // a field outside its per-field rule was reset to its default
  kRepairStaticIp = 1u << 1,      // incomplete static IP -> dhcp
  kRepairWifiPassword = 1u << 2,  // ssid with a 1..7 byte password -> WiFi credentials cleared
  kRepairWifiIface = 1u << 3,     // iface WiFi without ssid -> auto
  kRepairSyslog = 1u << 4,        // level > 0 without server -> 0
  kRepairWebNoPassword = 1u << 5, // web user without password -> both cleared
  kRepairMqttHost = 1u << 6,      // mode != off without host -> off
  kRepairMinDelay = 1u << 7,      // minDelayS > publishIntervalS -> publishIntervalS
  kRepairHaSeparate = 1u << 8,    // HA mode without separate -> mode MQTT
  kRepairHaDecimal = 1u << 9,     // HA mode with germanDecimal -> germanDecimal false (V1)
  kRepairValveNames = 1u << 10,   // duplicate / number-clashing valve names -> later name cleared
  kRepairSlotIds = 1u << 11,      // duplicate temp/volt ids -> later id cleared
  kRepairSlotActive = 1u << 12,   // active slot without id -> inactive
  kRepairTopics = 1u << 13,       // duplicate valve segment by an override -> later override cleared
  kRepairWebNoUser = 1u << 14,    // web password without user -> both cleared
  kRepairHaIds = 1u << 15,        // equal HA ids (V3) -> the later name (or override) cleared
};
struct Repairs {
  uint32_t mask = 0;          // RepairBit values applied
  uint16_t count = 0;         // repairs applied (saturating)
  uint16_t valveNames = 0;    // bit i: valve i name cleared
  uint16_t valveTopics = 0;   // bit i: valve i topic override cleared
  uint64_t tempIds = 0, tempActive = 0;  // bit i: temp slot i id cleared / set inactive
  uint8_t voltIds = 0, voltActive = 0;
  char first[40] = {0};       // key path of the first repair ("calib.hour", "valves.3.name")
};
// Resets every field that fails its per-field rule to its default, then
// applies the cross-field repairs in validateConfig order. Postcondition:
// validateConfig(c). out may be null. Returns the repair mask.
uint32_t sanitizeConfig(Config& c, Repairs* out);

struct DecodeInfo {
  uint16_t schema = 0;       // header schema
  bool newerSchema = false;  // schema > kConfigBaseSchema: its known prefix was decoded
  Repairs repairs;           // sanitizeConfig() result
};
// Decodes a `cfg` blob into `out` (defaults first, then the fields), then
// makes it valid with sanitizeConfig() (info->repairs):
//  - schema 0 -> UnsupportedSchema, defaults;
//  - schema 1 -> the payload must end exactly after the last field;
//  - schema > 1 (a newer firmware) -> the schema-1 fields are read from the
//    payload start, the rest is ignored; newerSchema; Ok;
//  - structural damage (bool byte > 1, string length >= cap, NUL inside a
//    string, payload too short) -> Invalid, defaults;
//  - value damage -> Ok with repairs (station "" -> "VdMot", ...).
// `info` may be null.
DecodeResult decodeConfig(const uint8_t* data, size_t len, Config& out,
                          DecodeInfo* info = nullptr);

// NVS `cfgx` blob (the keys added after 2.0.0): magic "VDMX", u8 format
// version 1 (a higher one is read like 1), u16 payload length L, records
// {u8 tag, u8 element (0-based array index, 0 for objects), u8 len, len
// bytes} (L bytes), u32 CRC-32 over bytes 0..6+L; bytes after the CRC are
// ignored. Values: PctHold 1 byte, OffOrRange u16, strings without NUL.
// The encoder writes every ext field in table order (strings also when
// empty), so a missing record means "default".
constexpr size_t kConfigExtBlobMax = 1536;
constexpr size_t kConfigExtKeepMax = 512;  // unknown records carried over (a newer firmware's keys)
enum class ExtResult : uint8_t { Absent, Ok, TooShort, BadMagic, BadCrc };
struct ExtInfo {
  uint16_t applied = 0, unknown = 0, bad = 0;
  size_t keepLen = 0;  // bytes of unknown records copied to `keep`
};
// Appends `keep` (unknown records read from the stored cfgx) verbatim after
// its own records, so a save does not destroy keys of a newer firmware.
// Returns bytes written, 0 when cap is too small.
size_t encodeConfigExt(const Config& c, uint8_t* out, size_t cap, const uint8_t* keep = nullptr,
                       size_t keepLen = 0);
// Applies the records to `inout` (defaults stay for missing ones). Unknown
// tag -> unknown + 1 and the record is copied to `keep` while it fits (at
// most kConfigExtKeepMax bytes); known tag with element >= count, a wrong
// length or a value that fails its field rule -> bad + 1, field unchanged;
// a later record for the same field wins. data == nullptr or len == 0 ->
// Absent. Damaged header or CRC -> nothing applied.
ExtResult decodeConfigExt(const uint8_t* data, size_t len, Config& inout, ExtInfo* info = nullptr,
                          uint8_t* keep = nullptr, size_t keepCap = 0);

struct StoredBlobs {
  const uint8_t* base = nullptr;
  size_t baseLen = 0;
  const uint8_t* ext = nullptr;
  size_t extLen = 0;
};
struct LoadInfo {
  DecodeResult base = DecodeResult::TooShort;
  ExtResult ext = ExtResult::Absent;
  DecodeInfo decode;  // base decode incl. its repairs
  ExtInfo extInfo;
  Repairs repairs;    // final sanitizeConfig() after the ext records
};
// The base blob must be usable (decodeConfig Ok); then the ext records; then
// sanitizeConfig. True when `out` holds a usable config (never false
// because of the ext blob).
bool loadConfigBlobs(const StoredBlobs& b, Config& out, LoadInfo& info, uint8_t* keep = nullptr,
                     size_t keepCap = 0);

// CRC-32 (IEEE 802.3, reflected, init/xorout 0xFFFFFFFF; zlib crc32()
// semantics: pass the previous result as `crc` to continue over more data).
// Also used by the flasher.
uint32_t crc32(const uint8_t* data, size_t len, uint32_t crc = 0);

}  // namespace vdm
