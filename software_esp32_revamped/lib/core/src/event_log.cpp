#include "vdm/event_log.h"

#include <stdarg.h>
#include <stdio.h>
#include <string.h>

#include "vdm/json_writer.h"
#include "vdm/stm_codec.h"
#include "vdm/valve_model.h"

namespace vdm {

namespace {

struct CodeInfo {
  EventCode code;
  const char* name;
  Severity severity;
};

const CodeInfo kCodes[] = {
    {EventCode::Boot, "boot", Severity::Info},
    {EventCode::ConfigImported, "config_imported", Severity::Info},
    {EventCode::ConfigSaved, "config_saved", Severity::Info},
    {EventCode::ConfigDefaults, "config_defaults", Severity::Error},
    {EventCode::FsFormatted, "fs_formatted", Severity::Warning},
    {EventCode::EspOtaStarted, "esp_ota_started", Severity::Info},
    {EventCode::EspOtaDone, "esp_ota_done", Severity::Info},
    {EventCode::EspOtaFailed, "esp_ota_failed", Severity::Error},
    {EventCode::AppMarkedValid, "app_marked_valid", Severity::Info},
    {EventCode::RebootRequested, "reboot_requested", Severity::Info},
    {EventCode::LowHeap, "low_heap", Severity::Warning},
    {EventCode::TimeSynced, "time_synced", Severity::Info},
    {EventCode::CalibTimeMissing, "calib_time_missing", Severity::Warning},
    {EventCode::NetUp, "net_up", Severity::Info},
    {EventCode::NetDown, "net_down", Severity::Warning},
    {EventCode::MqttConnected, "mqtt_connected", Severity::Info},
    {EventCode::MqttDisconnected, "mqtt_disconnected", Severity::Warning},
    {EventCode::MqttCommandRejected, "mqtt_command_rejected", Severity::Warning},
    {EventCode::HaDiscoverySent, "ha_discovery_sent", Severity::Info},
    {EventCode::AuthFailed, "auth_failed", Severity::Warning},
    {EventCode::LinkUp, "link_up", Severity::Info},
    {EventCode::LinkDegraded, "link_degraded", Severity::Info},
    {EventCode::LinkDown, "link_down", Severity::Error},
    {EventCode::StmResetByPolicy, "stm_reset_by_policy", Severity::Error},
    {EventCode::StmResetByUser, "stm_reset_by_user", Severity::Info},
    {EventCode::StmRebootDetected, "stm_reboot_detected", Severity::Warning},
    {EventCode::StmVersion, "stm_version", Severity::Info},
    {EventCode::StmIncompatible, "stm_incompatible", Severity::Error},
    {EventCode::StmRxOverflow, "stm_rx_overflow", Severity::Warning},
    {EventCode::StmParseErrors, "stm_parse_errors", Severity::Warning},
    {EventCode::StmQueueFull, "stm_queue_full", Severity::Warning},
    {EventCode::StmFlashStarted, "stm_flash_started", Severity::Info},
    {EventCode::StmFlashDone, "stm_flash_done", Severity::Info},
    {EventCode::StmFlashFailed, "stm_flash_failed", Severity::Critical},
    {EventCode::TargetSet, "target_set", Severity::Info},
    {EventCode::ValveStateChanged, "valve_state_changed", Severity::Debug},
    {EventCode::ValveBlocked, "valve_blocked", Severity::Error},
    {EventCode::ValveFailed, "valve_failed", Severity::Error},
    {EventCode::ValveNoValve, "valve_no_valve", Severity::Warning},
    {EventCode::ValveRecovered, "valve_recovered", Severity::Info},
    {EventCode::CalibStarted, "calib_started", Severity::Info},
    {EventCode::CalibOk, "calib_ok", Severity::Info},
    {EventCode::CalibRetry, "calib_retry", Severity::Warning},
    {EventCode::CalibFailed, "calib_failed", Severity::Error},
    {EventCode::EarlyStop, "early_stop", Severity::Warning},
    {EventCode::CmdRejected, "cmd_rejected", Severity::Warning},
    {EventCode::TargetNotConfirmed, "target_not_confirmed", Severity::Warning},
    {EventCode::ValveStale, "valve_stale", Severity::Warning},
    {EventCode::ServiceMoveDone, "service_move_done", Severity::Info},
    {EventCode::TempSensorFailed, "temp_sensor_failed", Severity::Warning},
    {EventCode::TempSensorRecovered, "temp_sensor_recovered", Severity::Info},
    {EventCode::SensorCountChanged, "sensor_count_changed", Severity::Info},
    {EventCode::VoltSensorFailed, "volt_sensor_failed", Severity::Warning},
    {EventCode::ScheduledCalibration, "scheduled_calibration", Severity::Info},
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
    while (*s != '\0') put(*s++);
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
  void put(char c) {
    if (len_ + 1 >= cap_) return;
    out_[len_++] = c;
    out_[len_] = '\0';
  }
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
  static const char* const kNames[] = {"user", "ota", "net watchdog", "factory reset", "rollback"};
  return (r >= 0 && r < 5) ? kNames[r] : "unknown";
}

const char* rebootCauseName(int32_t c) {
  static const char* const kNames[] = {"uptime", "resets", "v1 heuristic", "link recovered"};
  return (c >= 1 && c <= 4) ? kNames[c - 1] : "unknown";
}

const char* ifaceName(int32_t i) { return i == 1 ? "eth" : i == 2 ? "wifi" : "unknown"; }
const char* sideName(int32_t s) { return s == 0 ? "esp" : s == 1 ? "stm" : "unknown"; }

// Event args are int32; the enum/status names take a byte. Values that do
// not fit must not wrap onto a valid name.
const char* stopName(int32_t s) {
  const uint8_t b = static_cast<uint8_t>(s);
  return s == b ? stopReasonName(static_cast<StopReason>(b)) : "unknown";
}

const char* statusKey(int32_t s) {
  const uint8_t b = static_cast<uint8_t>(s);
  return s == b ? valveStatusKey(b) : "invalid";
}

const char* sourceName(int32_t s) {
  const uint8_t b = static_cast<uint8_t>(s);
  return s == b ? targetSourceName(static_cast<TargetSource>(b)) : "unknown";
}

void addRecoveredFlags(Text& t, int32_t flags) {
  const char* sep = "";
  if (flags & kHealthStale) {
    t.add("data again");
    sep = ", ";
  }
  if (flags & kHealthTargetUnconfirmed) {
    t.add(sep);
    t.add("target confirmed");
  }
}

// Message body per code. `txt` is the event text (bounded, NUL-terminated).
void addMessage(Text& t, const Event& e, const char* txt) {
  const int32_t a1 = e.arg1;
  const int32_t a2 = e.arg2;
  const bool hasTxt = txt[0] != '\0';
  switch (e.code) {
    case EventCode::Boot:
      t.addf("boot (reset %s, count %ld", resetReasonName(a1), static_cast<long>(a2));
      if (hasTxt) {
        t.add(", fw ");
        t.add(txt);
      }
      t.add(")");
      return;
    case EventCode::ConfigImported:
      t.addf("legacy config imported (%ld keys, %ld rejected", static_cast<long>(a1),
             static_cast<long>(a2));
      if (hasTxt) {
        t.add(", first ");
        t.add(txt);
      }
      t.add(")");
      return;
    case EventCode::ConfigSaved:
      t.addf("config saved (revision %ld", static_cast<long>(a1));
      if (hasTxt) {
        t.add(", ");
        t.add(txt);
      }
      t.add(")");
      return;
    case EventCode::ConfigDefaults:
      t.addf("stored config unusable, using defaults (reason %ld)", static_cast<long>(a1));
      return;
    case EventCode::FsFormatted:
      t.add(a1 == -1 ? "file system format failed" : "file system formatted");
      return;
    case EventCode::EspOtaStarted:
      t.addf("ESP update started (%ld bytes)", static_cast<long>(a1));
      return;
    case EventCode::EspOtaDone:
      t.addf("ESP update done (%ld bytes", static_cast<long>(a1));
      if (hasTxt) {
        t.add(", ");
        t.add(txt);
      }
      t.add(")");
      return;
    case EventCode::EspOtaFailed:
      t.addf("ESP update failed (error %ld)", static_cast<long>(a1));
      return;
    case EventCode::AppMarkedValid:
      t.addf("firmware marked valid after %ld s", static_cast<long>(a1));
      return;
    case EventCode::RebootRequested:
      t.addf("restart requested (%s)", rebootReasonName(a1));
      return;
    case EventCode::LowHeap:
      t.addf("low heap (free %ld, min %ld)", static_cast<long>(a1), static_cast<long>(a2));
      return;
    case EventCode::TimeSynced:
      t.addf("time synced (step %ld s)", static_cast<long>(a1));
      return;
    case EventCode::CalibTimeMissing:
      t.addf("scheduled calibration skipped, no valid time (slot %ld)", static_cast<long>(a1));
      return;
    case EventCode::NetUp:
      t.addf("network up (%s", ifaceName(a1));
      if (hasTxt) {
        t.add(", ");
        t.add(txt);
      }
      t.add(")");
      return;
    case EventCode::NetDown:
      t.addf("network down (%s)", ifaceName(a1));
      return;
    case EventCode::MqttConnected:
      t.add("MQTT connected");
      return;
    case EventCode::MqttDisconnected:
      t.addf("MQTT disconnected (state %ld)", static_cast<long>(a1));
      return;
    case EventCode::MqttCommandRejected:
      t.add("MQTT command rejected (");
      if (a1 > 0) {
        t.addf("valve %ld", static_cast<long>(a1));
      } else {
        t.add("unknown valve");
      }
      if (hasTxt) {
        t.add(", ");
        t.add(txt);
      }
      t.add(")");
      return;
    case EventCode::HaDiscoverySent:
      t.addf("HA discovery sent (%ld configs, %ld deletes)", static_cast<long>(a1),
             static_cast<long>(a2));
      return;
    case EventCode::AuthFailed:
      t.addf("authentication failed (%ld in window", static_cast<long>(a1));
      if (hasTxt) {
        t.add(", ");
        t.add(txt);
      }
      t.add(")");
      return;
    case EventCode::LinkUp:
      t.add("STM link up");
      return;
    case EventCode::LinkDegraded:
      t.addf("STM link degraded (%ld timeouts)", static_cast<long>(a1));
      return;
    case EventCode::LinkDown:
      t.addf("STM link down (%ld timeouts)", static_cast<long>(a1));
      return;
    case EventCode::StmResetByPolicy:
      t.addf("STM reset by link policy (%ld timeouts in %ld s)", static_cast<long>(a1),
             static_cast<long>(a2));
      return;
    case EventCode::StmResetByUser:
      t.add("STM reset by user");
      return;
    case EventCode::StmRebootDetected:
      t.addf("STM reboot detected (%s)", rebootCauseName(a1));
      return;
    case EventCode::StmVersion:
      t.add("STM ");
      t.add(hasTxt ? txt : "version unknown");
      t.addf(" (protocol %ld, hw 0x%03lx)", static_cast<long>(a1),
             static_cast<unsigned long>(static_cast<uint32_t>(a2)));
      return;
    case EventCode::StmIncompatible:
      t.add("STM version ");
      t.add(hasTxt ? txt : "unknown");
      t.add(" is not supported");
      return;
    case EventCode::StmRxOverflow:
      t.addf("UART receive overflow (%ld total, %s side)", static_cast<long>(a1), sideName(a2));
      return;
    case EventCode::StmParseErrors:
      t.addf("UART parse errors (%ld total, %s side)", static_cast<long>(a1), sideName(a2));
      return;
    case EventCode::StmQueueFull:
      t.addf("STM request queue full (command %ld)", static_cast<long>(a1));
      return;
    case EventCode::StmFlashStarted:
      t.addf("STM flash started (%ld bytes", static_cast<long>(a1));
      if (hasTxt) {
        t.add(", ");
        t.add(txt);
      }
      t.add(")");
      return;
    case EventCode::StmFlashDone:
      t.addf("STM flash done in %ld ms", static_cast<long>(a1));
      if (hasTxt) {
        t.add(" (");
        t.add(txt);
        t.add(")");
      }
      return;
    case EventCode::StmFlashFailed:
      t.addf("STM flash failed (error %ld at 0x%08lx", static_cast<long>(a1),
             static_cast<unsigned long>(static_cast<uint32_t>(a2)));
      if (hasTxt) {
        t.add(", ");
        t.add(txt);
      }
      t.add(")");
      return;
    case EventCode::TargetSet:
      t.addf("target %ld %% (%s)", static_cast<long>(a1), sourceName(a2));
      return;
    case EventCode::ValveStateChanged:
      t.addf("state %s -> %s", statusKey(a1), statusKey(a2));
      return;
    case EventCode::ValveBlocked:
      t.addf("blocked (calibration retries %ld)", static_cast<long>(a1));
      return;
    case EventCode::ValveFailed:
      t.add("failed");
      return;
    case EventCode::ValveNoValve:
      t.add("no valve detected");
      return;
    case EventCode::ValveRecovered:
      t.add("recovered (");
      if (a1 != 0) {
        t.add("was ");
        t.add(statusKey(a1));
      } else {
        addRecoveredFlags(t, a2);
      }
      t.add(")");
      return;
    case EventCode::CalibStarted:
      t.add(a1 == 1 ? "calibration started (scheduled)" : "calibration started");
      return;
    case EventCode::CalibOk:
      t.addf("calibration ok (oc %ld, cc %ld)", static_cast<long>(a1), static_cast<long>(a2));
      return;
    case EventCode::CalibRetry:
      t.addf("calibration retry %ld", static_cast<long>(a1));
      return;
    case EventCode::CalibFailed:
      t.addf("calibration failed after %ld retries", static_cast<long>(a1));
      return;
    case EventCode::EarlyStop:
      t.addf("early stop (total %ld, %s)", static_cast<long>(a1), stopName(a2));
      return;
    case EventCode::CmdRejected:
      t.addf("command rejected by the STM (total %ld)", static_cast<long>(a1));
      return;
    case EventCode::TargetNotConfirmed:
      t.addf("target %ld %% not confirmed after %ld attempts", static_cast<long>(a1),
             static_cast<long>(a2));
      return;
    case EventCode::ValveStale:
      t.addf("no data for %ld s", static_cast<long>(a1));
      return;
    case EventCode::ServiceMoveDone:
      t.addf("service move done (%ld counts, %s)", static_cast<long>(a1), stopName(a2));
      return;
    case EventCode::TempSensorFailed:
      t.addf("temp sensor %ld failed (raw %ld", static_cast<long>(a1), static_cast<long>(a2));
      if (hasTxt) {
        t.add(", ");
        t.add(txt);
      }
      t.add(")");
      return;
    case EventCode::TempSensorRecovered:
      t.addf("temp sensor %ld recovered", static_cast<long>(a1));
      return;
    case EventCode::SensorCountChanged:
      t.addf("%s sensor count %ld", a2 == 1 ? "volt" : "temp", static_cast<long>(a1));
      return;
    case EventCode::VoltSensorFailed:
      t.addf("volt sensor %ld failed (raw %ld)", static_cast<long>(a1), static_cast<long>(a2));
      return;
    case EventCode::ScheduledCalibration:
      t.addf("scheduled calibration (slot %ld, %ld min late)", static_cast<long>(a1),
             static_cast<long>(a2));
      return;
  }
  t.addf("event %u", static_cast<unsigned>(e.code));
}

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
  if (out == nullptr || cap == 0) return 0;
  char txt[sizeof e.text];
  const size_t tl = boundedLength(e.text, kEventTextMax);
  memcpy(txt, e.text, tl);
  txt[tl] = '\0';
  Text t(out, cap);
  if (e.valve < kValveCount) {
    t.addf("valve %u: ", static_cast<unsigned>(e.valve) + 1);
  } else if (e.valve == kAllValves) {
    t.add("all valves: ");
  }
  addMessage(t, e, txt);
  return t.length();
}

size_t formatEventLine(const Event& e, char* out, size_t cap) {
  if (out == nullptr || cap == 0) return 0;
  Text t(out, cap);
  if (e.epoch != 0) {
    const uint32_t sod = e.epoch % 86400u;
    uint32_t y;
    unsigned m, d;
    civilFromDays(e.epoch / 86400u, y, m, d);
    t.addf("%04lu-%02u-%02uT%02u:%02u:%02uZ", static_cast<unsigned long>(y), m, d,
           static_cast<unsigned>(sod / 3600), static_cast<unsigned>(sod / 60 % 60),
           static_cast<unsigned>(sod % 60));
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
  char msg[128];
  formatEventMessage(e, msg, sizeof msg);
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
  jw.key("valve");
  if (e.valve < kValveCount) {
    jw.value(static_cast<uint32_t>(e.valve) + 1);
  } else {
    jw.nullValue();
  }
  jw.kv("a1", e.arg1);
  jw.kv("a2", e.arg2);
  jw.key("text");
  jw.value(e.text, boundedLength(e.text, kEventTextMax));
  jw.kv("msg", static_cast<const char*>(msg));
  jw.endObject();
  return jw.ok();
}

}  // namespace vdm
