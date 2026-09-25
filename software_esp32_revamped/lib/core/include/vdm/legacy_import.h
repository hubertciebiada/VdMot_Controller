// One-shot import of the legacy (1.4.x) NVS configuration into the new
// Config (architecture R5, specs/04 §5). Hardware-free: the glue implements
// LegacyNvsReader on top of Preferences (read-only), tests use a map.
// The import never writes or deletes legacy keys, so it is idempotent and a
// downgrade to the legacy firmware still finds its config.
#pragma once

#include <stddef.h>
#include <stdint.h>

#include "vdm/config.h"

namespace vdm {

// Read-only access to one legacy NVS key. Namespaces: "sysCfg", "netCfg",
// "tZCfg", "protCfg", "valvesCfg", "tempsCfg", "voltsCfg", "Misc"; the
// dropped "valvesCtrlCfg", "msgCfg" and "motorCfg" keys are only probed to
// count them in ImportReport::ignored.
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

// Legacy blob sizes (Xtensa GCC layout, specs/04 §5 table).
constexpr size_t kLegacyValvesBlob = 144;   // 12 x {char name[11]; bool active;}
constexpr size_t kLegacyTempsBlob = 1496;   // 34 x {name[11], active, int offset@12, ID[25]@16, pad}
constexpr size_t kLegacyVoltsBlob = 480;    // 8 x 56 {name,active,float offset@12,float factor@16,unit[9]@20,ID[25]@29} + 8 x 4

struct ImportReport {
  bool anyLegacy = false;     // at least one legacy namespace had a key
  uint16_t imported = 0;      // keys applied (a blob counts once)
  uint16_t rejected = 0;      // keys/blob fields present but invalid, plus repairs
  uint16_t ignored = 0;       // DROP keys present (PI, messenger, ...), not imported
  // "<ns>/<key>" of the first rejected key; blob fields as
  // "<ns>/<key>.<n>.<field>" (n 1-based, e.g. "tempsCfg/temps.3.id").
  char firstRejected[40] = {0};
  int64_t lastCalibEpoch = 0;    // Misc/MiscLC when valid (>= 2020-01-01), else 0
};

// Builds `out` from defaults plus every legacy key that validates.
// Mapping (binding, DESIGN.md "Legacy import"):
//  sysCfg/stName -> station (isSafeName; invalid -> default "VdMot")
//  sysCfg/CF -> ignored (°C only)
//  netCfg/ethwifi, dhcp, staticIp, mask, gw, dnsIp, ssid, pwd, userName,
//    userPwd, timeServer, syslogEnable, sysLogIp, sysLogPort (0 -> 514),
//    netConnTO -> net.*, web.*, time.ntpServer, syslog.*
//  tZCfg/tZ, tZCode -> time.tzName, time.tzPosix
//  protCfg/dataProt, brokerIp (uint32 -> dotted host), brokerPort (0 -> 1883),
//    publishInterval (clamped 2..3600), brokerUser, brokerPwd, brokerPF
//    (missing -> 7 like the legacy read fallback, applied only when
//    protCfg/dataProt exists, so a device without legacy MQTT settings keeps
//    the new defaults), brokerKAT, brokerMD,
//    brokerMQF bit2 -> mqtt.germanDecimal; brokerInterval, brokerMQTO,
//    brokerMQToPos and brokerMQF bits 0/1 -> ignored.
//    dataProt 2 with publishSeparate 0: imported as mode Mqtt (HA needs
//    separate topics) and reported as rejected "protCfg/dataProt".
//  valvesCfg/valves (blob 144), dayOfCalib, hourOfCalib (24..255 meant
//    "never" in the legacy firmware -> calib.dayMask 0, both keys imported)
//  tempsCfg/temps (blob 1496): name, active, offset (clamped to +-10.0 C ->
//    rejected if outside), ID (parseOneWireId; "" or all-zero -> empty slot)
//  voltsCfg/volts (blob 480): name, active, offset, factor, unit, ID
//  Misc/MiscLC -> report.lastCalibEpoch (2020-01-01 <= t < 2100-01-01; the
//    legacy firmware also stored the unsynced 1970 clock -> rejected)
// Strings longer than their field and blob strings without a NUL inside
// their char[] (legacy strncpy) are rejected. Per-key validation uses
// setConfigValue itself; a key that fails keeps the default. Afterwards
// cross-field rules are enforced by clearing the offending optional feature,
// each counted as rejected: incomplete static IP -> DHCP ("netCfg/dhcp");
// ssid with a password of 1..7 chars -> WiFi credentials cleared
// ("netCfg/pwd"; an empty password is an open network and kept); WiFi-only without ssid -> auto ("netCfg/ethwifi"); syslog
// level without server -> 0; web user or password alone -> both cleared;
// MQTT without broker -> off ("protCfg/brokerIp"); minDelay above the
// publish interval -> clamped ("protCfg/brokerMD"); HA without separate
// topics -> MQTT ("protCfg/dataProt"); HA with the decimal comma -> dot
// ("protCfg/brokerMQF"); duplicate valve segments or HA ids and names
// equal to another unnamed valve's number -> later names cleared (repeated
// until stable); duplicate sensor ids -> later id cleared; active slots
// without id -> inactive; active slots with one HA id -> the later name
// cleared, or the earlier one when the later slot is unnamed (repeated
// until stable). As a last resort (never reached by the rules
// above) a result that still fails validateConfig() is replaced by the
// defaults and reported as "validate/<path>". The result always passes
// validateConfig(). The temps blob buffer (1496 B) is static, so the
// function is not reentrant (it runs once at boot under the storage lock).
ImportReport importLegacyConfig(LegacyNvsReader& nvs, Config& out);

}  // namespace vdm
