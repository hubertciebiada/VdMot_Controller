// One-shot import of the legacy (1.4.x) NVS configuration into the new
// Config (DESIGN.md "Legacy import"). Hardware-free: the glue implements
// LegacyNvsReader on top of Preferences (read-only), tests use a map.
// The import never writes or deletes legacy keys, so it is idempotent and a
// downgrade to the legacy firmware still finds its config.
#pragma once

#include <stddef.h>
#include <stdint.h>

#include "vdm/config.h"

namespace vdm {

class JsonWriter;

// Read-only access to one legacy NVS key. Namespaces: "sysCfg", "netCfg",
// "tZCfg", "protCfg", "valvesCfg", "tempsCfg", "voltsCfg", "Misc"; the
// dropped "valvesCtrlCfg", "msgCfg" and "motorCfg" keys are counted in
// ImportReport::ignored (valvesCtrl and msgFlags are also read for the
// report).
// Every method returns false when the namespace or key does not exist or
// the stored type differs; outputs are untouched then.
class LegacyNvsReader {
 public:
  virtual ~LegacyNvsReader() = default;
  // Integer keys of any width (UChar, UShort, ULong, Long, Short). Signed
  // types are sign-extended, unsigned zero-extended.
  virtual bool readInt(const char* ns, const char* key, int64_t& out) = 0;
  // String keys. Copies at most cap-1 chars + NUL; `truncated` is set when the
  // stored string was longer (the importer then rejects the key).
  virtual bool readString(const char* ns, const char* key, char* out, size_t cap,
                          bool& truncated) = 0;
  // Blob keys. Copies min(stored, cap) bytes; `storedLen` receives the stored
  // length so the importer can reject unexpected sizes.
  virtual bool readBlob(const char* ns, const char* key, uint8_t* out, size_t cap,
                        size_t& storedLen) = 0;
};

// Legacy blob sizes (Xtensa GCC layout, DESIGN.md "Legacy import").
constexpr size_t kLegacyValvesBlob = 144;   // 12 x {char name[11]; bool active;}
constexpr size_t kLegacyTempsBlob = 1496;   // 34 x {name[11], active, int offset@12, ID[25]@16, pad}
constexpr size_t kLegacyVoltsBlob = 480;    // 8 x 56 {name,active,float offset@12,float factor@16,unit[9]@20,ID[25]@29} + 8 x 4
constexpr size_t kLegacyVoltsBlob140 = 448; // 1.4.0: the same 8 x 56 elements without the 8 x 4 tail
constexpr size_t kLegacyValvesCtrlMax = 12 * 64;  // valvesCtrl: 12 elements, byte 0 = controlFlags

// ImportReport::dropped: legacy features the new firmware does not have and
// that were in use (event import_dropped, report "dropped").
constexpr uint8_t kDroppedPi = 0x01;             // PI control active on a valve
constexpr uint8_t kDroppedWindow = 0x02;         // window contact installed on a valve
constexpr uint8_t kDroppedMessenger = 0x04;      // PushOver or e-mail messages
constexpr uint8_t kDroppedDs18Timeout = 0x08;    // brokerMQF bit1: DS18 value timeout
constexpr uint8_t kDroppedLegacyFailsafe = 0x10; // brokerMQF bit0: tValue timeout failsafe

struct ImportReport {
  bool anyLegacy = false;     // at least one legacy namespace had a key
  uint16_t imported = 0;      // keys applied (a blob counts once)
  uint16_t rejected = 0;      // keys/blob fields present but invalid, plus repairs
  uint16_t ignored = 0;       // DROP keys present (PI, messenger, ...), not imported
  // "<ns>/<key>" of the first rejected key; blob fields as
  // "<ns>/<key>.<n>.<field>" (n 1-based, e.g. "tempsCfg/temps.3.id").
  char firstRejected[40] = {0};
  int64_t lastCalibEpoch = 0;    // Misc/MiscLC when valid (>= 2020-01-01), else 0
  uint8_t piValves = 0;          // valves with PI control (valvesCtrl)
  uint8_t windowValves = 0;      // valves with a window contact (valvesCtrl)
  uint8_t dropped = 0;           // kDropped* bits
  // bit i: item i got a new name (characters MQTT cannot carry replaced, or
  // cleared as a duplicate); the config holds the result.
  uint16_t renamedValves = 0;
  uint64_t renamedTemps = 0;
  uint8_t renamedVolts = 0;
  // Legacy failsafe keys (not mapped: the new config keeps 60 min / 50 %).
  bool legacyFailsafeValid = false;    // brokerMQTO or brokerMQToPos stored
  bool legacyFailsafeEnabled = false;  // brokerMQF bit0
  int32_t legacyFailsafeTimeoutMin = 0;
  int32_t legacyFailsafePct = 0;
  bool syslogDebug = false;      // syslogEnable 1..3 (debug verbosity) imported as level 3
  bool voltsBlob448 = false;     // the volts blob of 1.4.0 (448 bytes)
};

// Builds `out` from defaults plus every legacy key that validates.
// Mapping (binding, DESIGN.md "Legacy import"):
//  sysCfg/stName -> station (isSafeName; invalid -> default "VdMot"); ""
//    (legacy topics under "VdMotFBH/") -> station "VdMot", mqtt.rootTopic
//    "VdMotFBH", counted as imported
//  sysCfg/CF -> ignored (°C only)
//  netCfg/ethwifi, dhcp, staticIp, mask, gw, dnsIp, ssid, pwd, userName,
//    userPwd, timeServer, sysLogIp, sysLogPort (0 -> 514), netConnTO ->
//    net.*, web.*, time.ntpServer, syslog.*; syslogEnable 0 -> 0, 1..3 (the
//    legacy debug verbosity) -> 3 and report.syslogDebug, others rejected
//  tZCfg/tZ, tZCode -> time.tzName, time.tzPosix
//  protCfg/dataProt, brokerIp (uint32 -> dotted host), brokerPort (0 -> 1883),
//    publishInterval (clamped 2..3600), brokerUser, brokerPwd, brokerPF
//    (missing -> 7 like the legacy read fallback, applied only when
//    protCfg/dataProt exists, so a device without legacy MQTT settings keeps
//    the new defaults), brokerKAT, brokerMD,
//    brokerMQF bit2 -> mqtt.germanDecimal; bit0 (tValue failsafe) ->
//    dropped kDroppedLegacyFailsafe, bit1 -> kDroppedDs18Timeout, one
//    ignored count for either. brokerInterval, brokerMQTO and brokerMQToPos
//    are ignored; MQTO/ToPos and bit0 are copied into report.legacyFailsafe*.
//    dataProt 2 with publishSeparate 0: imported as mode Mqtt (HA needs
//    separate topics) and reported as rejected "protCfg/dataProt".
//  valvesCfg/valves (blob 144), dayOfCalib, hourOfCalib (24..255 meant
//    "never" in the legacy firmware -> calib.dayMask 0, both keys imported)
//  tempsCfg/temps (blob 1496): name, active, offset (clamped to +-10.0 C ->
//    rejected if outside), ID (parseOneWireId; "" or all-zero -> empty slot)
//  voltsCfg/volts (blob 480, or 448 from 1.4.0 -> report.voltsBlob448):
//    name, active, offset, factor, unit, ID
//  valvesCtrlCfg/valvesCtrl (ignored; 12 elements of L / 12 bytes, 12..768
//    bytes, byte 0 bit0 PI active, bit4 window contact) -> report.piValves,
//    windowValves, dropped; other lengths rejected
//  msgCfg/msgFlags (ignored) bit0 PushOver or bit1 e-mail -> dropped
//    kDroppedMessenger
//  Misc/MiscLC -> report.lastCalibEpoch (2020-01-01 <= t < 2100-01-01; the
//    legacy firmware also stored the unsynced 1970 clock -> rejected)
// Blob names that are printable text but contain '/', '+', '#', '"' or '\'
// get those replaced by '_' (report.renamed*); when the original contains
// '/', '"' or '\' and no '+' or '#', the original with ' ' -> '_' becomes
// the item's MQTT topic override (the segment the legacy firmware used),
// when it is a valid segment. Strings longer than their field and blob
// strings without a NUL inside their char[] (legacy strncpy) are rejected.
// Per-key validation uses setConfigValue itself; a key that fails keeps the
// default. Afterwards sanitizeConfig() makes the result valid, each repair
// counted as rejected under its legacy key: incomplete static IP
// ("netCfg/dhcp"), a 1..7 char WiFi password ("netCfg/pwd"), WiFi-only
// without ssid ("netCfg/ethwifi"), syslog without server
// ("netCfg/syslogEnable"), web user or password alone ("netCfg/userPwd",
// "netCfg/userName"), MQTT without broker ("protCfg/brokerIp"), minDelay
// above the publish interval ("protCfg/brokerMD"), HA without separate
// topics ("protCfg/dataProt"), HA with the decimal comma
// ("protCfg/brokerMQF"), cleared valve names and overrides
// ("valvesCfg/valves.<n>.name|topic"), sensor ids, active flags and names
// ("tempsCfg/temps.<n>.id|active|name", volts alike). The result always
// passes validateConfig(). The temps blob buffer (1496 B) is static, so the
// function is not reentrant (it runs once at boot under the storage lock);
// the valvesCtrl blob (up to 768 B) is read on the stack.
ImportReport importLegacyConfig(LegacyNvsReader& nvs, Config& out);

// The import report document (/sys/import.json):
// {"imported":57,"rejected":2,"ignored":14,"firstRejected":"valvesCfg/valves.5.name",
//  "piValves":3,"windowValves":1,"dropped":["pi","window","messenger","ds18Timeout"],
//  "legacyFailsafe":{"enabled":true,"timeoutMin":120,"pct":10},"rootTopic":"VdMotFBH",
//  "renamed":[{"kind":"valve","n":3,"name":"Bad_WC","topic":"Bad/WC"}],
//  "syslogDebug":true,"voltsBlob448":false}
// "legacyFailsafe" and "rootTopic" are null when not set; "renamed" lists
// valves, temps, volts (kind "valve", "temp", "volt") by the renamed bits
// with the names and overrides of `c`. Returns jw.ok().
bool writeImportReportJson(JsonWriter& jw, const ImportReport& r, const Config& c);

}  // namespace vdm
