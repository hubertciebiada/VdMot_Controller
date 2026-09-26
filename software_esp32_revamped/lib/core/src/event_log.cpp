#include "vdm/event_log.h"

#include <stdarg.h>
#include <stdio.h>
#include <string.h>

#include "vdm/json_writer.h"
#include "vdm/net_policy.h"
#include "vdm/stm_codec.h"
#include "vdm/valve_model.h"

namespace vdm {

namespace {

// Message templates: text plus these control bytes. The templates are
// constants of this file, always well formed (a table instead of one printf
// call per code keeps the registry small in flash).
#define ARG1 "\x01"      // arg1, decimal
#define ARG2 "\x02"      // arg2, decimal
#define TXT "\x03"       // the event text
#define IF_TXT "\x04"    // up to ELSE_TXT or END_TXT only when the event has a text
#define ELSE_TXT "\x05"
#define END_TXT "\x06"
#define NAME1 "\x07"     // + a set letter: arg1 through addName()
#define NAME2 "\x08"     // + a set letter: arg2 through addName()

struct CodeInfo {
  EventCode code;
  Severity severity;
  EventMqtt mqtt;
  const char* name;
  const char* msg;  // template; nullptr: the shape depends on the arguments (addSpecial)
};

constexpr EventMqtt kNo = EventMqtt::No;
constexpr EventMqtt kWarn = EventMqtt::WarnPlus;
constexpr EventMqtt kAlways = EventMqtt::Always;

// Registry order = the order of eventMqttNames().
const CodeInfo kCodes[] = {
    {EventCode::Boot, Severity::Info, kWarn, "boot",
     "boot (reset " NAME1 "r" ", count " ARG2 IF_TXT ", fw " TXT END_TXT ")"},
    {EventCode::ConfigImported, Severity::Info, kWarn, "config_imported",
     "legacy config imported (" ARG1 " keys, " ARG2 " rejected" IF_TXT ", first " TXT END_TXT
     ")"},
    {EventCode::ConfigSaved, Severity::Info, kWarn, "config_saved",
     "config saved (revision " ARG1 IF_TXT ", " TXT END_TXT ")"},
    {EventCode::ConfigDefaults, Severity::Error, kWarn, "config_defaults",
     "stored config unusable, using defaults (reason " ARG1 ")"},
    {EventCode::FsFormatted, Severity::Warning, kWarn, "fs_formatted", "file system" NAME1 "S"},
    {EventCode::EspOtaStarted, Severity::Info, kWarn, "esp_ota_started",
     "ESP update started (" ARG1 " bytes)"},
    {EventCode::EspOtaDone, Severity::Info, kWarn, "esp_ota_done",
     "ESP update done (" ARG1 " bytes" IF_TXT ", " TXT END_TXT ")"},
    {EventCode::EspOtaFailed, Severity::Error, kWarn, "esp_ota_failed",
     "ESP update failed (error " ARG1 ")"},
    {EventCode::AppMarkedValid, Severity::Info, kWarn, "app_marked_valid",
     "firmware marked valid after " ARG1 " s"},
    {EventCode::RebootRequested, Severity::Info, kWarn, "reboot_requested", nullptr},
    {EventCode::LowHeap, Severity::Warning, kWarn, "low_heap",
     "low heap (free " ARG1 ", min " ARG2 ")"},
    {EventCode::TimeSynced, Severity::Info, kWarn, "time_synced", "time synced (step " ARG1 " s)"},
    {EventCode::CalibTimeMissing, Severity::Warning, kWarn, "calib_time_missing",
     "scheduled calibration skipped, no valid time (slot " ARG1 ")"},
    {EventCode::FactoryResetSkipped, Severity::Warning, kWarn, "factory_reset_skipped",
     "factory reset pin still set: settings kept, remove the jumper"},
    {EventCode::StackLow, Severity::Warning, kWarn, "stack_low",
     "task " TXT ": stack low (" ARG1 " of " ARG2 " bytes free)"},
    {EventCode::HeapFragmented, Severity::Warning, kWarn, "heap_fragmented",
     "heap fragmented (largest block " ARG1 ", free " ARG2 ")"},
    {EventCode::LogWriteFailed, Severity::Warning, kWarn, "log_write_failed",
     "log file write failed (" NAME1 "l" ", " ARG2 " events lost)"},
    {EventCode::ImportDropped, Severity::Warning, kWarn, "import_dropped",
     "legacy import dropped " NAME2 "f" " (" ARG1 " PI valves" IF_TXT ", " TXT END_TXT ")"},
    {EventCode::ConfigRestored, Severity::Warning, kWarn, "config_restored",
     "configuration restored from the backup (" NAME1 "R" ")"},
    {EventCode::ConfigRepaired, Severity::Warning, kWarn, "config_repaired",
     "configuration repaired (" ARG2 " fields" IF_TXT ", first " TXT END_TXT ")"},
    {EventCode::ConfigNewerSchema, Severity::Warning, kWarn, "config_newer_schema",
     "configuration written by a newer firmware (schema " ARG1 ", " ARG2
     " unknown settings kept)"},
    {EventCode::FilesRemoved, Severity::Info, kNo, "files_removed",
     "removed " ARG1 " files (" ARG2 " KiB)" IF_TXT ": " TXT END_TXT},
    {EventCode::NetUp, Severity::Info, kWarn, "net_up",
     "network up (" NAME1 "i" IF_TXT ", " TXT END_TXT ")"},
    {EventCode::NetDown, Severity::Warning, kWarn, "net_down", "network down (" NAME1 "i" ")"},
    {EventCode::MqttConnected, Severity::Info, kWarn, "mqtt_connected", "MQTT connected"},
    {EventCode::MqttDisconnected, Severity::Warning, kWarn, "mqtt_disconnected",
     "MQTT disconnected (state " ARG1 ")"},
    {EventCode::MqttCommandRejected, Severity::Warning, kWarn, "mqtt_command_rejected",
     "MQTT command rejected (" NAME1 "m" IF_TXT ", " TXT END_TXT ")"},
    {EventCode::HaDiscoverySent, Severity::Info, kWarn, "ha_discovery_sent",
     "HA discovery sent (" ARG1 " configs, " ARG2 " deletes)"},
    {EventCode::AuthFailed, Severity::Warning, kWarn, "auth_failed",
     "authentication failed (" ARG1 " in window" IF_TXT ", " TXT END_TXT ")"},
    {EventCode::NetTrialStarted, Severity::Info, kNo, "net_trial_started",
     "network settings on trial for " ARG1 " s" IF_TXT " (" TXT ")" END_TXT},
    {EventCode::NetTrialConfirmed, Severity::Info, kNo, "net_trial_confirmed",
     "network settings confirmed after " ARG1 " s" NAME2 "n"},
    {EventCode::NetTrialReverted, Severity::Warning, kWarn, "net_trial_reverted", nullptr},
    {EventCode::NetUnreachable, Severity::Warning, kWarn, "net_unreachable",
     "network unreachable (nothing for " ARG1 " s, last " NAME2 "e" ")"},
    {EventCode::NetReachable, Severity::Info, kNo, "net_reachable",
     "network reachable again after " ARG1 " s"},
    {EventCode::NetInterfaceRestart, Severity::Warning, kWarn, "net_interface_restart",
     "network interface restarted (" NAME2 "j" ", after " ARG1 " s)"},
    {EventCode::RequestRefused, Severity::Warning, kWarn, "request_refused",
     "request refused (" NAME1 "q" ")" IF_TXT " from " TXT END_TXT},
    {EventCode::AuthLocked, Severity::Warning, kWarn, "auth_locked",
     "login locked for " ARG1 " s (lockout " ARG2 ")" IF_TXT " for " TXT END_TXT},
    {EventCode::LinkUp, Severity::Info, kWarn, "link_up", "STM link up"},
    {EventCode::LinkDegraded, Severity::Info, kWarn, "link_degraded",
     "STM link degraded (" ARG1 " timeouts)"},
    {EventCode::LinkDown, Severity::Error, kWarn, "link_down", "STM link down (" ARG1 " timeouts)"},
    {EventCode::StmResetByPolicy, Severity::Error, kWarn, "stm_reset_by_policy",
     "STM reset by link policy (" ARG1 " timeouts in " ARG2 " s)"},
    {EventCode::StmResetByUser, Severity::Info, kWarn, "stm_reset_by_user", "STM reset by user"},
    {EventCode::StmRebootDetected, Severity::Warning, kWarn, "stm_reboot_detected",
     "STM reboot detected (" NAME1 "c" ")"},
    {EventCode::StmVersion, Severity::Info, kWarn, "stm_version",
     "STM " IF_TXT TXT ELSE_TXT "version unknown" END_TXT " (protocol " ARG1 ", hw 0x" NAME2 "3"
     ")"},
    {EventCode::StmIncompatible, Severity::Error, kWarn, "stm_incompatible",
     "STM version " IF_TXT TXT ELSE_TXT "unknown" END_TXT " is not supported"},
    {EventCode::StmRxOverflow, Severity::Warning, kWarn, "stm_rx_overflow",
     "UART receive overflow (" ARG1 " total, " NAME2 "s" " side)"},
    {EventCode::StmParseErrors, Severity::Warning, kWarn, "stm_parse_errors",
     "UART parse errors (" ARG1 " total, " NAME2 "s" " side)"},
    {EventCode::StmQueueFull, Severity::Warning, kWarn, "stm_queue_full",
     "STM request queue full (command " ARG1 ")"},
    {EventCode::StmFlashStarted, Severity::Info, kWarn, "stm_flash_started",
     "STM flash started (" ARG1 " bytes" IF_TXT ", " TXT END_TXT ")"},
    {EventCode::StmFlashDone, Severity::Info, kWarn, "stm_flash_done",
     "STM flash done in " ARG1 " ms" IF_TXT " (" TXT ")" END_TXT},
    {EventCode::StmFlashFailed, Severity::Critical, kWarn, "stm_flash_failed",
     "STM flash failed (error " ARG1 " at 0x" NAME2 "8" IF_TXT ", " TXT END_TXT ")"},
    {EventCode::FailsafeActive, Severity::Warning, kAlways, "failsafe_active",
     "failsafe active on " NAME1 "p" " valves (" NAME2 "o" ")"},
    {EventCode::FailsafeEnded, Severity::Info, kAlways, "failsafe_ended",
     "failsafe ended after " ARG1 " s (" NAME2 "o" ")"},
    {EventCode::RegulatorLost, Severity::Warning, kWarn, "regulator_lost",
     "regulator lost (" NAME1 "g" ")"},
    {EventCode::RegulatorBack, Severity::Info, kAlways, "regulator_back",
     "regulator back after " ARG1 " s"},
    {EventCode::LeaseConfigFailed, Severity::Warning, kWarn, "lease_config_failed",
     "failsafe settings not accepted by the STM (" NAME1 "k" ", " ARG2 " attempts)"},
    {EventCode::StmSafeMode, Severity::Critical, kAlways, "stm_safe_mode",
     "STM in safe mode (" ARG1 " watchdog resets)"},
    {EventCode::StmSafeModeEnded, Severity::Info, kAlways, "stm_safe_mode_ended",
     "STM left safe mode"},
    {EventCode::StmConfigRepaired, Severity::Warning, kWarn, "stm_config_repaired",
     "STM configuration repaired (flags 0x" NAME1 "2" ", " ARG2 " repairs)"},
    {EventCode::StmUartErrors, Severity::Warning, kWarn, "stm_uart_errors",
     "STM UART errors (" ARG1 " total, " ARG2 " bytes dropped)"},
    {EventCode::StmEepromWaitTimeout, Severity::Warning, kWarn, "stm_eeprom_wait_timeout",
     "STM EEPROM write still pending after " ARG1 " ms (" NAME2 "w" ")" IF_TXT ": " TXT END_TXT},
    {EventCode::TargetsRestored, Severity::Info, kNo, "targets_restored",
     "desired targets restored for " ARG1 " valves (" NAME2 "t" ")"},
    {EventCode::StmProtectionSuspended, Severity::Error, kWarn, "stm_protection_suspended",
     "STM short-circuit and inrush limits suspended until the next STM start"},
    {EventCode::TargetSet, Severity::Info, kWarn, "target_set",
     "target " ARG1 " % (" NAME2 "u" ")"},
    {EventCode::ValveStateChanged, Severity::Debug, kWarn, "valve_state_changed",
     "state " NAME1 "v" " -> " NAME2 "v"},
    {EventCode::ValveBlocked, Severity::Error, kWarn, "valve_blocked",
     "blocked (calibration retries " ARG1 NAME2 "d" ")"},
    {EventCode::ValveFailed, Severity::Error, kWarn, "valve_failed", "failed" NAME2 "a"},
    {EventCode::ValveNoValve, Severity::Warning, kWarn, "valve_no_valve", "no valve detected"},
    {EventCode::ValveRecovered, Severity::Info, kWarn, "valve_recovered", nullptr},
    {EventCode::CalibStarted, Severity::Info, kWarn, "calib_started",
     "calibration started" NAME1 "y"},
    {EventCode::CalibOk, Severity::Info, kAlways, "calib_ok",
     "calibration ok (oc " ARG1 ", cc " ARG2 ")"},
    {EventCode::CalibRetry, Severity::Warning, kAlways, "calib_retry", "calibration retry " ARG1},
    {EventCode::CalibFailed, Severity::Error, kAlways, "calib_failed",
     "calibration failed after " ARG1 " retries" NAME2 "d"},
    {EventCode::EarlyStop, Severity::Warning, kWarn, "early_stop",
     "early stop (total " ARG1 ", " NAME2 "x" ")"},
    {EventCode::CmdRejected, Severity::Warning, kWarn, "cmd_rejected",
     "command rejected by the STM (total " ARG1 ")"},
    {EventCode::TargetNotConfirmed, Severity::Warning, kWarn, "target_not_confirmed",
     "target " ARG1 " % not confirmed after " ARG2 " attempts"},
    {EventCode::ValveStale, Severity::Warning, kWarn, "valve_stale", "no data for " ARG1 " s"},
    {EventCode::ServiceMoveDone, Severity::Info, kWarn, "service_move_done",
     "service move done (" ARG1 " counts, " NAME2 "x" ")"},
    {EventCode::CalibStrokeShort, Severity::Warning, kWarn, "calib_stroke_short",
     "calibration stroke " ARG1 " close to the minimum " ARG2},
    {EventCode::TempSensorFailed, Severity::Warning, kWarn, "temp_sensor_failed",
     "temp sensor " ARG1 " failed (raw " ARG2 IF_TXT ", " TXT END_TXT ")"},
    {EventCode::TempSensorRecovered, Severity::Info, kWarn, "temp_sensor_recovered",
     "temp sensor " ARG1 " recovered"},
    {EventCode::SensorCountChanged, Severity::Info, kWarn, "sensor_count_changed",
     NAME2 "z" " sensor count " ARG1},
    {EventCode::VoltSensorFailed, Severity::Warning, kWarn, "volt_sensor_failed",
     "volt sensor " ARG1 " failed (raw " ARG2 ")"},
    {EventCode::ScheduledCalibration, Severity::Info, kWarn, "scheduled_calibration",
     "scheduled calibration (slot " ARG1 ", " ARG2 " min late)"},
    {EventCode::ScheduledCalibrationFailed, Severity::Warning, kWarn,
     "scheduled_calibration_failed",
     "scheduled calibration not confirmed (slot " ARG1 ", " NAME2 "h" ")"},
    {EventCode::ScheduledCalibrationMissed, Severity::Error, kWarn,
     "scheduled_calibration_missed",
     "scheduled calibration missed (slot " ARG1 ", " ARG2 " attempts)"},
};

const CodeInfo* findCode(EventCode c) {
  for (const CodeInfo& ci : kCodes) {
    if (ci.code == c) return &ci;
  }
  return nullptr;
}

const char* const kSeverityNames[] = {"debug", "info", "warning", "error", "critical"};
const char* const kSeverityUpper[] = {"DEBUG", "INFO", "WARNING", "ERROR", "CRITICAL"};
constexpr uint8_t kSeverityCount = 5;

// Bounded appender: truncates, always NUL-terminated. cap > 0 (callers check).
class Text {
 public:
  Text(char* out, size_t cap) : out_(out), cap_(cap) { out_[0] = '\0'; }
  void add(const char* s) {
    if (s == nullptr) return;
    while (*s != '\0') addChar(*s++);
  }
  void addChar(char c) {
    if (len_ + 1 >= cap_) return;
    out_[len_++] = c;
    out_[len_] = '\0';
  }
  // Formats straight into the remaining space (truncating like add()).
  void addf(const char* fmt, ...) __attribute__((format(printf, 2, 3))) {  // NOMUTATE: attribute
    const size_t room = cap_ - len_;
    va_list ap;
    va_start(ap, fmt);
    const int n = vsnprintf(out_ + len_, room, fmt, ap);
    va_end(ap);
    if (n > 0) len_ += static_cast<size_t>(n) < room ? static_cast<size_t>(n) : room - 1;
    out_[len_] = '\0';
  }
  size_t length() const { return len_; }

 private:
  char* out_;
  size_t cap_;
  size_t len_ = 0;
};

const char* resetReasonName(int32_t r) {
  // esp_reset_reason_t (ESP-IDF 4.4)
  static const char* const kNames[] = {"unknown", "poweron",  "ext",     "sw",
                                       "panic",   "int_wdt",  "task_wdt", "wdt",
                                       "deepsleep", "brownout", "sdio"};
  const uint32_t i = static_cast<uint32_t>(r);
  return i < sizeof kNames / sizeof *kNames ? kNames[i] : "unknown";
}

const char* rebootReasonName(int32_t r) {
  static const char* const kNames[] = {"user",          "ota",      "net watchdog",
                                       "factory reset", "rollback", "network revert"};
  return (r >= 0 && r < 6) ? kNames[r] : "unknown";
}

// names[v - 1] for v in 1..N, else "unknown" (arguments that name a reason).
template <size_t N>
const char* oneBased(const char* const (&names)[N], int32_t v) {
  return (v >= 1 && v <= static_cast<int32_t>(N)) ? names[v - 1] : "unknown";
}

const char* const kLogSteps[] = {"open", "write", "rotate", "size limit"};
const char* const kImportFeatures[] = {"pi", "window", "messenger", "ds18Timeout", "legacyFailsafe"};
const char* const kTrialReverts[] = {"not confirmed", "no network", "interrupted", "user",
                                     "trial not stored"};
const char* const kIfaceSets[] = {"eth", "wifi", "eth+wifi"};
const char* const kRefusals[] = {"host", "origin", "header", "content type"};
const char* const kLeaseSources[] = {"STM lease", "ESP"};
const char* const kRegulatorLoss[] = {"MQTT broker disconnected", "Home Assistant offline"};
const char* const kLeaseConfigFailures[] = {"no reply", "rejected", "read-back differs"};
const char* const kEepromWaitActions[] = {"STM reset", "flash", "ESP restart"};
const char* const kRestoreSources[] = {"RTC", "NVS"};
const char* const kScheduleFailures[] = {"no reply", "not sent", "no result", "STM unsupported"};
const char* const kOtaChecks[] = {"net", "http", "stm"};
const char* const kRebootCauses[] = {"uptime", "resets", "v1 heuristic", "link recovered"};

// Event args are int32; the enum/status names take a byte. Values that do
// not fit must not wrap onto a valid name.
bool fitsByte(int32_t v) { return v == static_cast<uint8_t>(v); }

// Names of the set bits of `mask` (bit i = names[i]) joined by ", ";
// false when none of them is set.
template <size_t N>
bool addBitNames(Text& t, int32_t mask, const char* const (&names)[N]) {
  const char* sep = "";
  for (size_t i = 0; i < N; ++i) {
    if ((mask & (1 << i)) == 0) continue;
    t.add(sep);
    t.add(names[i]);
    sep = ", ";
  }
  return sep[0] != '\0';
}

// The name sets of NAME1/NAME2.
void addName(Text& t, char set, int32_t v) {
  const uint8_t b = static_cast<uint8_t>(v);
  const char* s = "unknown";
  switch (set) {
    case 'r': s = resetReasonName(v); break;
    case 'i': s = v == 1 ? "eth" : v == 2 ? "wifi" : "unknown"; break;
    case 'j': s = oneBased(kIfaceSets, v); break;
    case 'l': s = oneBased(kLogSteps, v); break;
    case 'q': s = oneBased(kRefusals, v); break;
    case 'c': s = oneBased(kRebootCauses, v); break;
    case 'o': s = oneBased(kLeaseSources, v); break;
    case 'g': s = oneBased(kRegulatorLoss, v); break;
    case 'k': s = oneBased(kLeaseConfigFailures, v); break;
    case 'w': s = oneBased(kEepromWaitActions, v); break;
    case 't': s = oneBased(kRestoreSources, v); break;
    case 'h': s = oneBased(kScheduleFailures, v); break;
    case 's': s = v == 0 ? "esp" : v == 1 ? "stm" : "unknown"; break;
    case 'z': s = v == 1 ? "volt" : "temp"; break;
    case 'S': s = v == -1 ? " format failed" : " formatted"; break;
    case 'n': s = v == 1 ? " (by a newer change)" : ""; break;
    case 'y': s = v == 1 ? " (scheduled)" : v == 2 ? " (automatic retry)" : ""; break;
    case 'e':
      if (fitsByte(v)) s = netEvidenceName(static_cast<NetEvidence>(b));
      break;
    case 'u':
      if (fitsByte(v)) s = targetSourceName(static_cast<TargetSource>(b));
      break;
    case 'x':
      if (fitsByte(v)) s = stopReasonName(static_cast<StopReason>(b));
      break;
    case 'v': s = fitsByte(v) ? valveStatusKey(b) : "invalid"; break;
    case 'a':  // " (<fault>)" when known
      if (v < 0) return;
      t.add(" (");
      t.add(fitsByte(v) ? valveFaultName(b) : "unknown");
      s = ")";
      break;
    case 'd':  // ", failsafe <v> %" when set
      if (v >= 0) t.addf(", failsafe %ld %%", static_cast<long>(v));
      return;
    case 'f':
      if (!addBitNames(t, v, kImportFeatures)) t.add("nothing");
      return;
    case 'm':
      if (v > 0) {
        t.addf("valve %ld", static_cast<long>(v));
        return;
      }
      s = "unknown valve";
      break;
    case 'R':
      if (v != 0) {
        t.addf("decode error %ld", static_cast<long>(v));
        return;
      }
      s = "NVS empty";
      break;
    case 'p': t.addf("%d", __builtin_popcount(static_cast<uint32_t>(v))); return;
    case '2':  // hex digits: the set letter is the width
    case '3':
    case '8':
      t.addf("%0*lx", set - '0', static_cast<unsigned long>(static_cast<uint32_t>(v)));
      return;
  }
  t.add(s);
}

void render(Text& t, const char* p, const Event& e, const char* txt) {
  for (; *p != '\0'; ++p) {
    switch (*p) {
      case ARG1[0]: t.addf("%ld", static_cast<long>(e.arg1)); break;
      case ARG2[0]: t.addf("%ld", static_cast<long>(e.arg2)); break;
      case TXT[0]: t.add(txt); break;
      case IF_TXT[0]:
        // Without a text continue after ELSE_TXT or END_TXT.
        if (txt[0] == '\0') {
          while (*p != ELSE_TXT[0] && *p != END_TXT[0]) ++p;
        }
        break;
      case ELSE_TXT[0]:
        // Reached from the text branch: skip the other one.
        while (*p != END_TXT[0]) ++p;
        break;
      case END_TXT[0]: break;
      case NAME1[0]: addName(t, *++p, e.arg1); break;
      case NAME2[0]: addName(t, *++p, e.arg2); break;
      default: t.addChar(*p); break;
    }
  }
}

// RebootRequested, NetTrialReverted and ValveRecovered.
void addSpecial(Text& t, const Event& e, const char* txt) {
  const int32_t a1 = e.arg1;
  const int32_t a2 = e.arg2;
  if (e.code == EventCode::RebootRequested) {
    t.add("restart requested (");
    t.add(rebootReasonName(a1));
    if (a1 == static_cast<int32_t>(RebootReason::NetWatchdog) && a2 > 0) {
      t.addf(", after %ld min", static_cast<long>(a2));
    } else if (a1 == static_cast<int32_t>(RebootReason::Rollback) && (a2 & 0x7) != 0) {
      t.add(", missing ");
      addBitNames(t, a2, kOtaChecks);
    }
  } else if (e.code == EventCode::NetTrialReverted) {
    t.add(a2 == -1 ? "network settings could not be reverted (" : "network settings reverted (");
    t.add(oneBased(kTrialReverts, a1));
    if (a2 != -1 && txt[0] != '\0') {
      t.add(", back to ");
      t.add(txt);
    }
  } else {
    t.add("recovered (");
    if (a1 != 0) {
      t.add("was ");
      addName(t, 'v', a1);
    } else {
      const char* sep = "";
      if (a2 & kHealthStale) {
        t.add("data again");
        sep = ", ";
      }
      if (a2 & kHealthTargetUnconfirmed) {
        t.add(sep);
        t.add("target confirmed");
      }
    }
  }
  t.add(")");
}

#undef ARG1
#undef ARG2
#undef TXT
#undef IF_TXT
#undef ELSE_TXT
#undef END_TXT
#undef NAME1
#undef NAME2

// Civil date from days since 1970-01-01 (H. Hinnant's algorithm, reduced to
// non-negative day counts: a uint32 epoch ends in 2106).
void civilFromDays(uint32_t days, uint32_t& y, unsigned& m, unsigned& d) {
  const uint32_t z = days + 719468;
  const uint32_t era = z / 146097;
  const unsigned doe = static_cast<unsigned>(z - era * 146097);
  const unsigned yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
  y = yoe + era * 400;
  const unsigned doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
  const unsigned mp = (5 * doy + 2) / 153;
  d = doy - (153 * mp + 2) / 5 + 1;
  m = mp < 10 ? mp + 3 : mp - 9;
  if (m <= 2) ++y;
}

// "YYYY-MM-DDTHH:MM:SS" of a UTC epoch, then `zone`.
void addUtc(Text& t, uint32_t epoch, const char* zone) {
  const uint32_t sod = epoch % 86400u;
  uint32_t y;
  unsigned m, d;
  civilFromDays(epoch / 86400u, y, m, d);
  t.addf("%04lu-%02u-%02uT%02u:%02u:%02u", static_cast<unsigned long>(y), m, d,
         static_cast<unsigned>(sod / 3600), static_cast<unsigned>(sod / 60 % 60),
         static_cast<unsigned>(sod % 60));
  t.add(zone);
}

constexpr uint16_t kValveMaskAll = (1u << kValveCount) - 1u;

// The valves of an aggregate: valveMask when it has 2 or more of the 12
// valve bits, else 0.
uint16_t aggregateMask(uint16_t valveMask) {
  const uint16_t m = valveMask & kValveMaskAll;
  return (m & (m - 1u)) != 0 ? m : 0;
}

size_t formatMessage(const Event& e, uint16_t multi, char* out, size_t cap) {
  if (out == nullptr || cap == 0) return 0;
  char txt[sizeof e.text];
  const size_t tl = boundedLength(e.text, kEventTextMax);
  memcpy(txt, e.text, tl);
  txt[tl] = '\0';
  Text t(out, cap);
  if (multi != 0) {
    const char* sep = "valves ";
    for (uint8_t v = 0; v < kValveCount; ++v) {
      if ((multi & (1u << v)) == 0) continue;
      t.addf("%s%u", sep, static_cast<unsigned>(v) + 1);
      sep = ", ";
    }
    t.add(": ");
  } else if (e.valve < kValveCount) {
    t.addf("valve %u: ", static_cast<unsigned>(e.valve) + 1);
  } else if (e.valve == kAllValves) {
    t.add("all valves: ");
  }
  const CodeInfo* ci = findCode(e.code);
  if (ci == nullptr) {
    t.addf("event %u", static_cast<unsigned>(e.code));
  } else if (ci->msg != nullptr) {
    render(t, ci->msg, e, txt);
  } else {
    addSpecial(t, e, txt);
  }
  return t.length();
}

// writeEventJson() and, with `mqtt`, writeMqttEventJson().
void writeEventObject(JsonWriter& jw, const Event& e, bool mqtt, uint16_t multi) {
  char msg[160];
  formatMessage(e, multi, msg, sizeof msg);
  jw.beginObject();
  jw.kv("seq", e.seq);
  jw.key("t");
  if (e.epoch != 0) {
    jw.value(e.epoch);
  } else {
    jw.nullValue();
  }
  jw.kv("up", e.uptimeS);
  jw.kv("sev", severityName(e.severity));
  jw.kv("code", static_cast<uint32_t>(e.code));
  jw.kv("name", eventCodeName(e.code));
  if (mqtt) jw.kv("event_type", eventCodeName(e.code));
  jw.key("valve");
  if (multi == 0 && e.valve < kValveCount) {
    jw.value(static_cast<uint32_t>(e.valve) + 1);
  } else {
    jw.nullValue();
  }
  if (multi != 0) {
    jw.key("valves");
    jw.beginArray();
    for (uint8_t v = 0; v < kValveCount; ++v) {
      if ((multi & (1u << v)) != 0) jw.value(static_cast<uint32_t>(v) + 1);
    }
    jw.endArray();
  }
  jw.kv("a1", e.arg1);
  jw.kv("a2", e.arg2);
  jw.key("text");
  jw.value(e.text, boundedLength(e.text, kEventTextMax));
  jw.kv("msg", static_cast<const char*>(msg));
  jw.endObject();
}

}  // namespace

const char* severityName(Severity s) {
  const uint8_t i = static_cast<uint8_t>(s);
  return i < kSeverityCount ? kSeverityNames[i] : "unknown";
}

bool parseSeverity(const char* s, size_t len, Severity& out) {
  if (s == nullptr) return false;
  for (uint8_t i = 0; i < kSeverityCount; ++i) {
    const char* name = kSeverityNames[i];
    if (strlen(name) != len) continue;
    // Names are lowercase letters only: `| 0x20` folds exactly 'A'..'Z'.
    size_t k = 0;
    while (k < len && (s[k] | 0x20) == name[k]) ++k;
    if (k == len) {
      out = static_cast<Severity>(i);
      return true;
    }
  }
  return false;
}

uint8_t syslogSeverity(Severity s) {
  switch (s) {
    case Severity::Debug: return 7;
    case Severity::Info: return 6;
    case Severity::Warning: return 4;
    case Severity::Error: return 3;
    case Severity::Critical: return 2;
  }
  return 7;
}

const char* eventCodeName(EventCode c) {
  const CodeInfo* ci = findCode(c);
  return ci ? ci->name : "unknown";
}

Severity eventDefaultSeverity(EventCode c) {
  const CodeInfo* ci = findCode(c);
  return ci ? ci->severity : Severity::Info;
}

bool eventIsCalibrationOutcome(EventCode c) {
  return c == EventCode::CalibOk || c == EventCode::CalibRetry || c == EventCode::CalibFailed;
}

Event makeEvent(EventCode code, Severity sev, uint8_t valve, int32_t arg1, int32_t arg2,
                const char* text) {
  Event e;
  e.code = code;
  e.severity = sev;
  e.valve = valve;
  e.arg1 = arg1;
  e.arg2 = arg2;
  copyString(e.text, sizeof e.text, text ? text : "");
  return e;
}

// ---------------------------------------------------------------- EventLog

EventLog::EventLog(Event* storage, size_t capacity) : buf_(storage), cap_(storage ? capacity : 0) {}

uint32_t EventLog::append(const Event& e) {
  if (cap_ == 0) {
    ++dropped_;
    return 0;
  }
  size_t idx;
  if (size_ == cap_) {
    idx = head_;
    head_ = (head_ + 1) % cap_;
    ++dropped_;
  } else {
    idx = (head_ + size_) % cap_;
    ++size_;
  }
  Event& slot = buf_[idx];
  slot = e;
  slot.text[kEventTextMax] = '\0';
  slot.seq = nextSeq_;
  if (++nextSeq_ == 0) nextSeq_ = 1;  // NOMUTATE: wraps after 2^32 events, not reachable in tests
  return slot.seq;
}

uint32_t EventLog::firstSeq() const { return size_ ? buf_[head_].seq : 0; }

uint32_t EventLog::lastSeq() const { return size_ ? buf_[(head_ + size_ - 1) % cap_].seq : 0; }

size_t EventLog::read(const EventFilter& f, Event* out, size_t maxOut, uint32_t& nextSince) const {
  nextSince = f.sinceSeq;
  if (out == nullptr || maxOut == 0) return 0;
  size_t n = 0;
  for (size_t k = 0; k < size_; ++k) {
    const Event& e = buf_[(head_ + k) % cap_];
    if (e.seq <= f.sinceSeq) continue;
    if (n == maxOut) break;
    nextSince = e.seq;
    if (e.severity < f.minSeverity) continue;
    if (f.valve != kNoValve && e.valve != f.valve && e.valve != kAllValves) continue;
    out[n++] = e;
  }
  return n;
}

bool EventLog::get(uint32_t seq, Event& out) const {
  // Stored events never carry seq 0, so get(0) finds nothing.
  for (size_t k = 0; k < size_; ++k) {
    const Event& e = buf_[(head_ + k) % cap_];
    if (e.seq == seq) {
      out = e;
      return true;
    }
  }
  return false;
}

void EventLog::clear() {
  // head_ may stay where it is: an empty ring starts wherever head_ points.
  size_ = 0;
}

// ---------------------------------------------------------------- formatting

size_t formatEventMessage(const Event& e, char* out, size_t cap) {
  return formatMessage(e, 0, out, cap);
}

size_t formatEventMessageMulti(const Event& e, uint16_t valveMask, char* out, size_t cap) {
  return formatMessage(e, aggregateMask(valveMask), out, cap);
}

size_t formatEventLine(const Event& e, char* out, size_t cap) {
  if (out == nullptr || cap == 0) return 0;
  Text t(out, cap);
  t.addf("#%lu ", static_cast<unsigned long>(e.seq));
  if (e.epoch != 0) {
    addUtc(t, e.epoch, "Z");
  } else {
    t.addf("+%lus", static_cast<unsigned long>(e.uptimeS));
  }
  const uint8_t sev = static_cast<uint8_t>(e.severity);
  t.add(" ");
  t.add(sev < kSeverityCount ? kSeverityUpper[sev] : "UNKNOWN");
  t.add(" ");
  t.add(eventCodeName(e.code));
  if (e.valve < kValveCount) t.addf(" v%u", static_cast<unsigned>(e.valve) + 1);
  t.add(" ");
  const size_t used = t.length();  // <= cap - 1, so at least the NUL fits
  return used + formatEventMessage(e, out + used, cap - used);
}

bool writeEventJson(JsonWriter& jw, const Event& e) {
  writeEventObject(jw, e, false, 0);
  return jw.ok();
}

bool writeMqttEventJson(JsonWriter& jw, const Event& e, uint16_t valveMask) {
  writeEventObject(jw, e, true, aggregateMask(valveMask));
  return jw.ok();
}

size_t formatUtcTimestamp(uint32_t epoch, char* out, size_t cap) {
  constexpr size_t kLen = 25;  // "2026-09-25T08:13:40+00:00"
  if (out == nullptr || cap == 0) return 0;
  if (cap < kLen + 1) {
    out[0] = '\0';
    return 0;
  }
  Text t(out, cap);
  addUtc(t, epoch, "+00:00");
  return t.length();
}

// ---------------------------------------------------------------- MQTT

EventMqtt eventMqtt(EventCode c) {
  const CodeInfo* ci = findCode(c);
  return ci ? ci->mqtt : EventMqtt::No;
}

bool eventReachesMqtt(const Event& e) {
  const EventMqtt m = eventMqtt(e.code);
  return m == EventMqtt::Always || (m == EventMqtt::WarnPlus && e.severity >= Severity::Warning);
}

size_t eventMqttNames(const char** out, size_t cap) {
  size_t n = 0;
  for (const CodeInfo& ci : kCodes) {
    if (ci.mqtt == EventMqtt::No) continue;
    if (out != nullptr && n < cap) out[n] = ci.name;
    ++n;
  }
  return n;
}

}  // namespace vdm
