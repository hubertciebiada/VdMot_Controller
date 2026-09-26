// Event registry, ring buffer and text/JSON formatting.
#include <stdio.h>
#include <string.h>

#include <initializer_list>
#include <string>
#include <vector>

#include "doctest.h"
#include "vdm/event_log.h"
#include "vdm/json_writer.h"
#include "vdm/stm_codec.h"
#include "vdm/valve_model.h"

using namespace vdm;

namespace {

struct CodeRow {
  EventCode code;
  uint16_t number;
  const char* name;
  Severity sev;
  EventMqtt mqtt;
};

constexpr EventMqtt kNo = EventMqtt::No;
constexpr EventMqtt kWarn = EventMqtt::WarnPlus;
constexpr EventMqtt kAlways = EventMqtt::Always;

// DESIGN.md §13, the binding table (registry order).
const CodeRow kTable[] = {
    {EventCode::Boot, 100, "boot", Severity::Info, kWarn},
    {EventCode::ConfigImported, 101, "config_imported", Severity::Info, kWarn},
    {EventCode::ConfigSaved, 102, "config_saved", Severity::Info, kWarn},
    {EventCode::ConfigDefaults, 103, "config_defaults", Severity::Error, kWarn},
    {EventCode::FsFormatted, 104, "fs_formatted", Severity::Warning, kWarn},
    {EventCode::EspOtaStarted, 105, "esp_ota_started", Severity::Info, kWarn},
    {EventCode::EspOtaDone, 106, "esp_ota_done", Severity::Info, kWarn},
    {EventCode::EspOtaFailed, 107, "esp_ota_failed", Severity::Error, kWarn},
    {EventCode::AppMarkedValid, 108, "app_marked_valid", Severity::Info, kWarn},
    {EventCode::RebootRequested, 109, "reboot_requested", Severity::Info, kWarn},
    {EventCode::LowHeap, 110, "low_heap", Severity::Warning, kWarn},
    {EventCode::TimeSynced, 111, "time_synced", Severity::Info, kWarn},
    {EventCode::CalibTimeMissing, 112, "calib_time_missing", Severity::Warning, kWarn},
    {EventCode::FactoryResetSkipped, 113, "factory_reset_skipped", Severity::Warning, kWarn},
    {EventCode::StackLow, 114, "stack_low", Severity::Warning, kWarn},
    {EventCode::HeapFragmented, 115, "heap_fragmented", Severity::Warning, kWarn},
    {EventCode::LogWriteFailed, 116, "log_write_failed", Severity::Warning, kWarn},
    {EventCode::ImportDropped, 117, "import_dropped", Severity::Warning, kWarn},
    {EventCode::ConfigRestored, 118, "config_restored", Severity::Warning, kWarn},
    {EventCode::ConfigRepaired, 119, "config_repaired", Severity::Warning, kWarn},
    {EventCode::ConfigNewerSchema, 120, "config_newer_schema", Severity::Warning, kWarn},
    {EventCode::FilesRemoved, 121, "files_removed", Severity::Info, kNo},
    {EventCode::NetUp, 200, "net_up", Severity::Info, kWarn},
    {EventCode::NetDown, 201, "net_down", Severity::Warning, kWarn},
    {EventCode::MqttConnected, 202, "mqtt_connected", Severity::Info, kWarn},
    {EventCode::MqttDisconnected, 203, "mqtt_disconnected", Severity::Warning, kWarn},
    {EventCode::MqttCommandRejected, 204, "mqtt_command_rejected", Severity::Warning, kWarn},
    {EventCode::HaDiscoverySent, 205, "ha_discovery_sent", Severity::Info, kWarn},
    {EventCode::AuthFailed, 206, "auth_failed", Severity::Warning, kWarn},
    {EventCode::NetTrialStarted, 207, "net_trial_started", Severity::Info, kNo},
    {EventCode::NetTrialConfirmed, 208, "net_trial_confirmed", Severity::Info, kNo},
    {EventCode::NetTrialReverted, 209, "net_trial_reverted", Severity::Warning, kWarn},
    {EventCode::NetUnreachable, 210, "net_unreachable", Severity::Warning, kWarn},
    {EventCode::NetReachable, 211, "net_reachable", Severity::Info, kNo},
    {EventCode::NetInterfaceRestart, 212, "net_interface_restart", Severity::Warning, kWarn},
    {EventCode::RequestRefused, 213, "request_refused", Severity::Warning, kWarn},
    {EventCode::AuthLocked, 214, "auth_locked", Severity::Warning, kWarn},
    {EventCode::LinkUp, 300, "link_up", Severity::Info, kWarn},
    {EventCode::LinkDegraded, 301, "link_degraded", Severity::Info, kWarn},
    {EventCode::LinkDown, 302, "link_down", Severity::Error, kWarn},
    {EventCode::StmResetByPolicy, 303, "stm_reset_by_policy", Severity::Error, kWarn},
    {EventCode::StmResetByUser, 304, "stm_reset_by_user", Severity::Info, kWarn},
    {EventCode::StmRebootDetected, 305, "stm_reboot_detected", Severity::Warning, kWarn},
    {EventCode::StmVersion, 306, "stm_version", Severity::Info, kWarn},
    {EventCode::StmIncompatible, 307, "stm_incompatible", Severity::Error, kWarn},
    {EventCode::StmRxOverflow, 308, "stm_rx_overflow", Severity::Warning, kWarn},
    {EventCode::StmParseErrors, 309, "stm_parse_errors", Severity::Warning, kWarn},
    {EventCode::StmQueueFull, 310, "stm_queue_full", Severity::Warning, kWarn},
    {EventCode::StmFlashStarted, 311, "stm_flash_started", Severity::Info, kWarn},
    {EventCode::StmFlashDone, 312, "stm_flash_done", Severity::Info, kWarn},
    {EventCode::StmFlashFailed, 313, "stm_flash_failed", Severity::Critical, kWarn},
    {EventCode::FailsafeActive, 314, "failsafe_active", Severity::Warning, kAlways},
    {EventCode::FailsafeEnded, 315, "failsafe_ended", Severity::Info, kAlways},
    {EventCode::RegulatorLost, 316, "regulator_lost", Severity::Warning, kWarn},
    {EventCode::RegulatorBack, 317, "regulator_back", Severity::Info, kAlways},
    {EventCode::LeaseConfigFailed, 318, "lease_config_failed", Severity::Warning, kWarn},
    {EventCode::StmSafeMode, 319, "stm_safe_mode", Severity::Critical, kAlways},
    {EventCode::StmSafeModeEnded, 320, "stm_safe_mode_ended", Severity::Info, kAlways},
    {EventCode::StmConfigRepaired, 321, "stm_config_repaired", Severity::Warning, kWarn},
    {EventCode::StmUartErrors, 322, "stm_uart_errors", Severity::Warning, kWarn},
    {EventCode::StmEepromWaitTimeout, 323, "stm_eeprom_wait_timeout", Severity::Warning, kWarn},
    {EventCode::TargetsRestored, 324, "targets_restored", Severity::Info, kNo},
    {EventCode::StmProtectionSuspended, 325, "stm_protection_suspended", Severity::Error, kWarn},
    {EventCode::TargetSet, 400, "target_set", Severity::Info, kWarn},
    {EventCode::ValveStateChanged, 401, "valve_state_changed", Severity::Debug, kWarn},
    {EventCode::ValveBlocked, 402, "valve_blocked", Severity::Error, kWarn},
    {EventCode::ValveFailed, 403, "valve_failed", Severity::Error, kWarn},
    {EventCode::ValveNoValve, 404, "valve_no_valve", Severity::Warning, kWarn},
    {EventCode::ValveRecovered, 405, "valve_recovered", Severity::Info, kWarn},
    {EventCode::CalibStarted, 406, "calib_started", Severity::Info, kWarn},
    {EventCode::CalibOk, 407, "calib_ok", Severity::Info, kAlways},
    {EventCode::CalibRetry, 408, "calib_retry", Severity::Warning, kAlways},
    {EventCode::CalibFailed, 409, "calib_failed", Severity::Error, kAlways},
    {EventCode::EarlyStop, 410, "early_stop", Severity::Warning, kWarn},
    {EventCode::CmdRejected, 411, "cmd_rejected", Severity::Warning, kWarn},
    {EventCode::TargetNotConfirmed, 412, "target_not_confirmed", Severity::Warning, kWarn},
    {EventCode::ValveStale, 413, "valve_stale", Severity::Warning, kWarn},
    {EventCode::ServiceMoveDone, 414, "service_move_done", Severity::Info, kWarn},
    {EventCode::CalibStrokeShort, 415, "calib_stroke_short", Severity::Warning, kWarn},
    {EventCode::TempSensorFailed, 500, "temp_sensor_failed", Severity::Warning, kWarn},
    {EventCode::TempSensorRecovered, 501, "temp_sensor_recovered", Severity::Info, kWarn},
    {EventCode::SensorCountChanged, 502, "sensor_count_changed", Severity::Info, kWarn},
    {EventCode::VoltSensorFailed, 503, "volt_sensor_failed", Severity::Warning, kWarn},
    {EventCode::ScheduledCalibration, 600, "scheduled_calibration", Severity::Info, kWarn},
    {EventCode::ScheduledCalibrationFailed, 601, "scheduled_calibration_failed",
     Severity::Warning, kWarn},
    {EventCode::ScheduledCalibrationMissed, 602, "scheduled_calibration_missed", Severity::Error,
     kWarn},
};

Event ev(EventCode c, uint8_t valve = kNoValve, int32_t a1 = 0, int32_t a2 = 0,
         const char* text = "", Severity sev = Severity::Info) {
  return makeEvent(c, sev, valve, a1, a2, text);
}

std::string message(const Event& e) {
  char buf[160];
  const size_t n = formatEventMessage(e, buf, sizeof buf);
  CHECK(n == strlen(buf));
  return buf;
}

}  // namespace

TEST_CASE("severity names, parsing and syslog mapping") {
  const char* names[] = {"debug", "info", "warning", "error", "critical"};
  const uint8_t syslog[] = {7, 6, 4, 3, 2};
  for (uint8_t i = 0; i < 5; ++i) {
    const Severity s = static_cast<Severity>(i);
    CHECK(strcmp(severityName(s), names[i]) == 0);
    CHECK(syslogSeverity(s) == syslog[i]);
    Severity out = Severity::Critical;
    if (i == 4) out = Severity::Debug;
    REQUIRE(parseSeverity(names[i], strlen(names[i]), out));
    CHECK(out == s);
  }
  CHECK(strcmp(severityName(static_cast<Severity>(5)), "unknown") == 0);
  CHECK(syslogSeverity(static_cast<Severity>(9)) == 7);

  Severity out = Severity::Info;
  CHECK(parseSeverity("WARNING", 7, out));
  CHECK(out == Severity::Warning);
  CHECK(parseSeverity("Critical", 8, out));
  CHECK(out == Severity::Critical);
  CHECK(parseSeverity("DeBuG", 5, out));
  CHECK(out == Severity::Debug);
  CHECK(parseSeverity("INFO", 4, out));
  CHECK(out == Severity::Info);
  CHECK(parseSeverity("errorXYZ", 5, out));  // exactly len bytes
  CHECK(out == Severity::Error);
  out = Severity::Info;
  for (const char* bad : {"warn", "", "warnings", "debug ", " info", "err0r", "crit", "inf\x0f",
                          "xnfo", "Xebug", "ebug", "nfo",
                          "iNfo\x80", "inf\xef", "inF\x4f\x01"}) {
    CHECK_FALSE(parseSeverity(bad, strlen(bad), out));
  }
  CHECK_FALSE(parseSeverity("info\0", 5, out));
  const std::vector<char> exact = {'e', 'R', 'r', 'o', 'r'};  // no terminator after it
  CHECK(parseSeverity(exact.data(), exact.size(), out));
  CHECK(out == Severity::Error);
  CHECK_FALSE(parseSeverity(nullptr, 4, out));
  CHECK_FALSE(parseSeverity("info", 3, out));
  CHECK(out == Severity::Error);  // unchanged by the failures
}

TEST_CASE("event code registry matches the design table") {
  for (const CodeRow& r : kTable) {
    CAPTURE(r.number);
    CHECK(static_cast<uint16_t>(r.code) == r.number);
    CHECK(strcmp(eventCodeName(r.code), r.name) == 0);
    CHECK(eventDefaultSeverity(r.code) == r.sev);
    CHECK(eventMqtt(r.code) == r.mqtt);
    const bool outcome = r.number == 407 || r.number == 408 || r.number == 409;
    CHECK(eventIsCalibrationOutcome(r.code) == outcome);
  }
  CHECK(strcmp(eventCodeName(static_cast<EventCode>(0)), "unknown") == 0);
  CHECK(strcmp(eventCodeName(static_cast<EventCode>(999)), "unknown") == 0);
  CHECK(eventDefaultSeverity(static_cast<EventCode>(999)) == Severity::Info);
  CHECK(eventMqtt(static_cast<EventCode>(999)) == EventMqtt::No);
  CHECK_FALSE(eventIsCalibrationOutcome(static_cast<EventCode>(999)));
}

TEST_CASE("eventMqttNames lists every published code in registry order") {
  std::vector<std::string> expected;
  for (const CodeRow& r : kTable) {
    if (r.mqtt != EventMqtt::No) expected.push_back(r.name);
  }
  REQUIRE(expected.size() == 81);
  const char* names[100] = {};
  CHECK(eventMqttNames(names, 100) == expected.size());
  for (size_t i = 0; i < expected.size(); ++i) {
    CAPTURE(i);
    REQUIRE(names[i] != nullptr);
    CHECK(expected[i] == names[i]);
  }
  CHECK(names[expected.size()] == nullptr);
  // A short array gets the first names, the total is still returned.
  const char* few[3] = {};
  CHECK(eventMqttNames(few, 2) == expected.size());
  CHECK(std::string(few[0]) == "boot");
  CHECK(std::string(few[1]) == "config_imported");
  CHECK(few[2] == nullptr);
  CHECK(eventMqttNames(nullptr, 5) == expected.size());
  CHECK(eventMqttNames(few, 0) == expected.size());
}

TEST_CASE("eventReachesMqtt: Always, or WarnPlus at Warning and above") {
  CHECK(eventReachesMqtt(makeEvent(EventCode::CalibOk, Severity::Info, 1, 0, 0, "")));
  CHECK(eventReachesMqtt(makeEvent(EventCode::CalibOk, Severity::Debug, 1, 0, 0, "")));
  CHECK(eventReachesMqtt(makeEvent(EventCode::FailsafeEnded, Severity::Info, kNoValve, 0, 0, "")));
  CHECK_FALSE(eventReachesMqtt(makeEvent(EventCode::Boot, Severity::Info, kNoValve, 0, 0, "")));
  CHECK(eventReachesMqtt(makeEvent(EventCode::Boot, Severity::Warning, kNoValve, 0, 0, "")));
  CHECK(eventReachesMqtt(makeEvent(EventCode::EarlyStop, Severity::Warning, 1, 0, 0, "")));
  CHECK(eventReachesMqtt(makeEvent(EventCode::LinkDown, Severity::Error, kNoValve, 0, 0, "")));
  CHECK(eventReachesMqtt(makeEvent(EventCode::StmFlashFailed, Severity::Critical, kNoValve, 0, 0,
                                   "")));
  CHECK_FALSE(eventReachesMqtt(makeEvent(EventCode::ServiceMoveDone, Severity::Info, 1, 0, 0, "")));
  // Codes without MQTT never reach it, whatever the severity.
  CHECK_FALSE(eventReachesMqtt(makeEvent(EventCode::FilesRemoved, Severity::Critical, kNoValve,
                                         0, 0, "")));
  CHECK_FALSE(eventReachesMqtt(makeEvent(static_cast<EventCode>(999), Severity::Critical,
                                         kNoValve, 0, 0, "")));
}

TEST_CASE("makeEvent fills and truncates") {
  const Event e = makeEvent(EventCode::NetUp, Severity::Warning, 3, -7, 9, "0123456789abcdefghijklmnopq");
  CHECK(e.code == EventCode::NetUp);
  CHECK(e.severity == Severity::Warning);
  CHECK(e.valve == 3);
  CHECK(e.arg1 == -7);
  CHECK(e.arg2 == 9);
  CHECK(strlen(e.text) == kEventTextMax);
  CHECK(strcmp(e.text, "0123456789abcdefghijklm") == 0);
  CHECK(e.seq == 0);
  const Event n = makeEvent(EventCode::Boot, Severity::Info, kNoValve, 0, 0, nullptr);
  CHECK(n.text[0] == '\0');
}

TEST_CASE("EventLog without storage drops everything") {
  EventLog a(nullptr, 10);
  CHECK(a.capacity() == 0);
  CHECK(a.append(ev(EventCode::Boot)) == 0);
  CHECK(a.dropped() == 1);
  CHECK(a.size() == 0);
  CHECK(a.firstSeq() == 0);
  CHECK(a.lastSeq() == 0);
  Event buf[1];
  EventLog b(buf, 0);
  CHECK(b.append(ev(EventCode::Boot)) == 0);
  CHECK(b.dropped() == 1);
  uint32_t next = 42;
  EventFilter f;
  f.sinceSeq = 7;
  CHECK(b.read(f, buf, 1, next) == 0);
  CHECK(next == 7);
  Event out;
  CHECK_FALSE(b.get(1, out));
}

TEST_CASE("EventLog ring: sequence numbers, overwrite, get, clear") {
  Event storage[3];
  EventLog log(storage, 3);
  CHECK(log.capacity() == 3);
  CHECK(log.append(ev(EventCode::Boot, kNoValve, 1)) == 1);
  CHECK(log.size() == 1);
  CHECK(log.firstSeq() == 1);
  CHECK(log.lastSeq() == 1);
  CHECK(log.append(ev(EventCode::Boot, kNoValve, 2)) == 2);
  CHECK(log.append(ev(EventCode::Boot, kNoValve, 3)) == 3);
  CHECK(log.dropped() == 0);
  CHECK(log.append(ev(EventCode::Boot, kNoValve, 4)) == 4);
  CHECK(log.append(ev(EventCode::Boot, kNoValve, 5)) == 5);
  CHECK(log.size() == 3);
  CHECK(log.dropped() == 2);
  CHECK(log.firstSeq() == 3);
  CHECK(log.lastSeq() == 5);
  Event out;
  CHECK_FALSE(log.get(0, out));
  CHECK_FALSE(log.get(2, out));
  CHECK_FALSE(log.get(6, out));
  REQUIRE(log.get(4, out));
  CHECK(out.seq == 4);
  CHECK(out.arg1 == 4);
  REQUIRE(log.get(3, out));
  CHECK(out.arg1 == 3);

  // Unterminated text is terminated in storage.
  Event raw = ev(EventCode::NetUp);
  memset(raw.text, 'x', sizeof raw.text);
  CHECK(log.append(raw) == 6);
  REQUIRE(log.get(6, out));
  CHECK(strlen(out.text) == kEventTextMax);

  log.clear();
  CHECK(log.size() == 0);
  CHECK(log.firstSeq() == 0);
  CHECK(log.lastSeq() == 0);
  CHECK_FALSE(log.get(6, out));
  CHECK(log.append(ev(EventCode::Boot)) == 7);  // numbering continues
  CHECK(log.firstSeq() == 7);
  CHECK(log.lastSeq() == 7);
}

TEST_CASE("EventLog read filters and cursors") {
  Event storage[16];
  EventLog log(storage, 16);
  log.append(ev(EventCode::Boot, kNoValve, 0, 0, "", Severity::Info));             // 1
  log.append(ev(EventCode::ValveStateChanged, 2, 0, 0, "", Severity::Debug));      // 2
  log.append(ev(EventCode::ValveBlocked, 2, 0, 0, "", Severity::Error));           // 3
  log.append(ev(EventCode::CalibStarted, kAllValves, 0, 0, "", Severity::Info));   // 4
  log.append(ev(EventCode::ValveNoValve, 5, 0, 0, "", Severity::Warning));         // 5
  log.append(ev(EventCode::LinkDown, kNoValve, 0, 0, "", Severity::Critical));     // 6

  Event out[16];
  uint32_t next = 0;
  EventFilter all;
  CHECK(log.read(all, out, 16, next) == 6);
  CHECK(next == 6);
  for (uint32_t i = 0; i < 6; ++i) CHECK(out[i].seq == i + 1);

  EventFilter since;
  since.sinceSeq = 4;
  CHECK(log.read(since, out, 16, next) == 2);
  CHECK(out[0].seq == 5);
  CHECK(next == 6);
  since.sinceSeq = 6;
  CHECK(log.read(since, out, 16, next) == 0);
  CHECK(next == 6);
  since.sinceSeq = 100;
  CHECK(log.read(since, out, 16, next) == 0);
  CHECK(next == 100);

  EventFilter warn;
  warn.minSeverity = Severity::Warning;
  REQUIRE(log.read(warn, out, 16, next) == 3);
  CHECK(out[0].seq == 3);
  CHECK(out[1].seq == 5);
  CHECK(out[2].seq == 6);
  CHECK(next == 6);

  EventFilter valve;
  valve.valve = 2;
  REQUIRE(log.read(valve, out, 16, next) == 3);
  CHECK(out[0].seq == 2);
  CHECK(out[1].seq == 3);
  CHECK(out[2].seq == 4);  // "all valves" matches every valve filter
  CHECK(next == 6);         // the last examined event, although filtered out

  // Limited output: the cursor stops at the last copied event.
  REQUIRE(log.read(all, out, 2, next) == 2);
  CHECK(out[1].seq == 2);
  CHECK(next == 2);
  // Filtered-out events before the limit are skipped by the cursor.
  REQUIRE(log.read(warn, out, 1, next) == 1);
  CHECK(out[0].seq == 3);
  CHECK(next == 3);
  CHECK(log.read(all, nullptr, 5, next) == 0);
  CHECK(next == 0);
  CHECK(log.read(all, out, 0, next) == 0);
  CHECK(next == 0);

  // get() on a partly filled ring.
  Event g;
  for (uint32_t seq = 1; seq <= 6; ++seq) {
    REQUIRE(log.get(seq, g));
    CHECK(g.seq == seq);
  }
  CHECK(g.code == EventCode::LinkDown);
  CHECK_FALSE(log.get(7, g));
  CHECK_FALSE(log.get(0, g));

  // Wrapped ring: oldest first.
  Event small[2];
  EventLog w(small, 2);
  for (int i = 0; i < 5; ++i) w.append(ev(EventCode::Boot, kNoValve, i));
  REQUIRE(w.read(all, out, 5, next) == 2);
  CHECK(out[0].seq == 4);
  CHECK(out[1].seq == 5);
  CHECK(next == 5);
}

TEST_CASE("event messages for every code") {
  struct Row {
    Event e;
    const char* msg;
  };
  const Row rows[] = {
      {ev(EventCode::Boot, kNoValve, 6, 17, "2.0.0-revamped"), "boot (reset task_wdt, count 17, fw 2.0.0-revamped)"},
      {ev(EventCode::Boot, kNoValve, 1, 1), "boot (reset poweron, count 1)"},
      {ev(EventCode::Boot, kNoValve, 11, 1), "boot (reset unknown, count 1)"},
      {ev(EventCode::Boot, kNoValve, -1, 1), "boot (reset unknown, count 1)"},
      {ev(EventCode::Boot, kNoValve, 10, 2), "boot (reset sdio, count 2)"},
      {ev(EventCode::Boot, kNoValve, 0, 2), "boot (reset unknown, count 2)"},
      {ev(EventCode::ConfigImported, kNoValve, 40, 2, "mqtt.port"), "legacy config imported (40 keys, 2 rejected, first mqtt.port)"},
      {ev(EventCode::ConfigImported, kNoValve, 40, 0), "legacy config imported (40 keys, 0 rejected)"},
      {ev(EventCode::ConfigSaved, kNoValve, 3, 0, "web"), "config saved (revision 3, web)"},
      {ev(EventCode::ConfigSaved, kNoValve, 3), "config saved (revision 3)"},
      {ev(EventCode::ConfigDefaults, kNoValve, 2), "stored config unusable, using defaults (reason 2)"},
      {ev(EventCode::FsFormatted, kNoValve, 0), "file system formatted"},
      {ev(EventCode::FsFormatted, kNoValve, -1), "file system format failed"},
      {ev(EventCode::EspOtaStarted, kNoValve, 1000), "ESP update started (1000 bytes)"},
      {ev(EventCode::EspOtaDone, kNoValve, 1000, 0, "2.0.1"), "ESP update done (1000 bytes, 2.0.1)"},
      {ev(EventCode::EspOtaDone, kNoValve, 1000), "ESP update done (1000 bytes)"},
      {ev(EventCode::EspOtaFailed, kNoValve, 8), "ESP update failed (error 8)"},
      {ev(EventCode::AppMarkedValid, kNoValve, 120), "firmware marked valid after 120 s"},
      {ev(EventCode::RebootRequested, kNoValve, 0), "restart requested (user)"},
      {ev(EventCode::RebootRequested, kNoValve, 1), "restart requested (ota)"},
      {ev(EventCode::RebootRequested, kNoValve, 2), "restart requested (net watchdog)"},
      {ev(EventCode::RebootRequested, kNoValve, 3), "restart requested (factory reset)"},
      {ev(EventCode::RebootRequested, kNoValve, 4), "restart requested (rollback)"},
      {ev(EventCode::RebootRequested, kNoValve, 5), "restart requested (network revert)"},
      {ev(EventCode::RebootRequested, kNoValve, 6), "restart requested (unknown)"},
      {ev(EventCode::RebootRequested, kNoValve, -1), "restart requested (unknown)"},
      {ev(EventCode::RebootRequested, kNoValve, 2, 10), "restart requested (net watchdog, after 10 min)"},
      {ev(EventCode::RebootRequested, kNoValve, 2, 1), "restart requested (net watchdog, after 1 min)"},
      {ev(EventCode::RebootRequested, kNoValve, 2, -5), "restart requested (net watchdog)"},
      {ev(EventCode::RebootRequested, kNoValve, 0, 10), "restart requested (user)"},
      {ev(EventCode::RebootRequested, kNoValve, 4, 7), "restart requested (rollback, missing net, http, stm)"},
      {ev(EventCode::RebootRequested, kNoValve, 4, 1), "restart requested (rollback, missing net)"},
      {ev(EventCode::RebootRequested, kNoValve, 4, 2), "restart requested (rollback, missing http)"},
      {ev(EventCode::RebootRequested, kNoValve, 4, 4), "restart requested (rollback, missing stm)"},
      {ev(EventCode::RebootRequested, kNoValve, 4, 5), "restart requested (rollback, missing net, stm)"},
      {ev(EventCode::RebootRequested, kNoValve, 4, 8), "restart requested (rollback)"},
      {ev(EventCode::RebootRequested, kNoValve, 5, 7), "restart requested (network revert)"},
      {ev(EventCode::RebootRequested, kNoValve, 3, 7), "restart requested (factory reset)"},
      {ev(EventCode::LowHeap, kNoValve, 29000, 20000), "low heap (free 29000, min 20000)"},
      {ev(EventCode::TimeSynced, kNoValve, -3), "time synced (step -3 s)"},
      {ev(EventCode::CalibTimeMissing, kNoValve, 20260923), "scheduled calibration skipped, no valid time (slot 20260923)"},
      {ev(EventCode::FactoryResetSkipped), "factory reset pin still set: settings kept, remove the jumper"},
      {ev(EventCode::StackLow, kNoValve, 480, 6144, "stm"), "task stm: stack low (480 of 6144 bytes free)"},
      {ev(EventCode::StackLow, kNoValve, 1, 2), "task : stack low (1 of 2 bytes free)"},
      {ev(EventCode::HeapFragmented, kNoValve, 7000, 90000), "heap fragmented (largest block 7000, free 90000)"},
      {ev(EventCode::LogWriteFailed, kNoValve, 1, 3), "log file write failed (open, 3 events lost)"},
      {ev(EventCode::LogWriteFailed, kNoValve, 2, 0), "log file write failed (write, 0 events lost)"},
      {ev(EventCode::LogWriteFailed, kNoValve, 3, 1), "log file write failed (rotate, 1 events lost)"},
      {ev(EventCode::LogWriteFailed, kNoValve, 4, 9), "log file write failed (size limit, 9 events lost)"},
      {ev(EventCode::LogWriteFailed, kNoValve, 0, 9), "log file write failed (unknown, 9 events lost)"},
      {ev(EventCode::LogWriteFailed, kNoValve, 5, 9), "log file write failed (unknown, 9 events lost)"},
      {ev(EventCode::ImportDropped, kNoValve, 3, 31, "ignored 14 keys"), "legacy import dropped pi, window, messenger, ds18Timeout, legacyFailsafe (3 PI valves, ignored 14 keys)"},
      {ev(EventCode::ImportDropped, kNoValve, 0, 2), "legacy import dropped window (0 PI valves)"},
      {ev(EventCode::ImportDropped, kNoValve, 1, 1, "x"), "legacy import dropped pi (1 PI valves, x)"},
      {ev(EventCode::ImportDropped, kNoValve, 0, 16), "legacy import dropped legacyFailsafe (0 PI valves)"},
      {ev(EventCode::ImportDropped, kNoValve, 0, 12), "legacy import dropped messenger, ds18Timeout (0 PI valves)"},
      {ev(EventCode::ImportDropped, kNoValve, 2, 0), "legacy import dropped nothing (2 PI valves)"},
      {ev(EventCode::ImportDropped, kNoValve, 2, 32), "legacy import dropped nothing (2 PI valves)"},
      {ev(EventCode::ConfigRestored, kNoValve, 0), "configuration restored from the backup (NVS empty)"},
      {ev(EventCode::ConfigRestored, kNoValve, 3), "configuration restored from the backup (decode error 3)"},
      {ev(EventCode::ConfigRestored, kNoValve, -1), "configuration restored from the backup (decode error -1)"},
      {ev(EventCode::ConfigRepaired, kNoValve, 5, 2, "calib.hour"), "configuration repaired (2 fields, first calib.hour)"},
      {ev(EventCode::ConfigRepaired, kNoValve, 5, 1), "configuration repaired (1 fields)"},
      {ev(EventCode::ConfigNewerSchema, kNoValve, 2, 3), "configuration written by a newer firmware (schema 2, 3 unknown settings kept)"},
      {ev(EventCode::FilesRemoved, kNoValve, 2, 96, "legacy images"), "removed 2 files (96 KiB): legacy images"},
      {ev(EventCode::FilesRemoved, kNoValve, 1, 0), "removed 1 files (0 KiB)"},
      {ev(EventCode::NetUp, kNoValve, 1, 0, "192.168.1.5"), "network up (eth, 192.168.1.5)"},
      {ev(EventCode::NetUp, kNoValve, 2), "network up (wifi)"},
      {ev(EventCode::NetUp, kNoValve, 3), "network up (unknown)"},
      {ev(EventCode::NetDown, kNoValve, 1), "network down (eth)"},
      {ev(EventCode::NetDown, kNoValve, 0), "network down (unknown)"},
      {ev(EventCode::MqttConnected), "MQTT connected"},
      {ev(EventCode::MqttDisconnected, kNoValve, -3), "MQTT disconnected (state -3)"},
      {ev(EventCode::MqttCommandRejected, kNoValve, 3, 2, "payload"), "MQTT command rejected (valve 3, payload)"},
      {ev(EventCode::MqttCommandRejected, kNoValve, 0), "MQTT command rejected (unknown valve)"},
      {ev(EventCode::MqttCommandRejected, kNoValve, 1), "MQTT command rejected (valve 1)"},
      {ev(EventCode::HaDiscoverySent, kNoValve, 80, 170), "HA discovery sent (80 configs, 170 deletes)"},
      {ev(EventCode::AuthFailed, kNoValve, 3, 0, "10.0.0.9"), "authentication failed (3 in window, 10.0.0.9)"},
      {ev(EventCode::AuthFailed, kNoValve, 3), "authentication failed (3 in window)"},
      {ev(EventCode::NetTrialStarted, kNoValve, 120, 0, "192.168.1.50"), "network settings on trial for 120 s (192.168.1.50)"},
      {ev(EventCode::NetTrialStarted, kNoValve, 120), "network settings on trial for 120 s"},
      {ev(EventCode::NetTrialConfirmed, kNoValve, 40, 0), "network settings confirmed after 40 s"},
      {ev(EventCode::NetTrialConfirmed, kNoValve, 40, 1), "network settings confirmed after 40 s (by a newer change)"},
      {ev(EventCode::NetTrialConfirmed, kNoValve, 40, 2), "network settings confirmed after 40 s"},
      {ev(EventCode::NetTrialReverted, kNoValve, 1, 0, "dhcp"), "network settings reverted (not confirmed, back to dhcp)"},
      {ev(EventCode::NetTrialReverted, kNoValve, 2, 0), "network settings reverted (no network)"},
      {ev(EventCode::NetTrialReverted, kNoValve, 3, 0, "10.0.0.2"), "network settings reverted (interrupted, back to 10.0.0.2)"},
      {ev(EventCode::NetTrialReverted, kNoValve, 4, 0), "network settings reverted (user)"},
      {ev(EventCode::NetTrialReverted, kNoValve, 5, 0, "dhcp"), "network settings reverted (trial not stored, back to dhcp)"},
      {ev(EventCode::NetTrialReverted, kNoValve, 6, 0), "network settings reverted (unknown)"},
      {ev(EventCode::NetTrialReverted, kNoValve, 1, -1, "dhcp"), "network settings could not be reverted (not confirmed)"},
      {ev(EventCode::NetTrialReverted, kNoValve, 1, -2), "network settings reverted (not confirmed)"},
      {ev(EventCode::NetTrialReverted, kNoValve, 1, 1), "network settings reverted (not confirmed)"},
      {ev(EventCode::NetTrialReverted, kNoValve, 4, 0, "x"), "network settings reverted (user, back to x)"},
      {ev(EventCode::NetUnreachable, kNoValve, 150, 1), "network unreachable (nothing for 150 s, last ping)"},
      {ev(EventCode::NetUnreachable, kNoValve, 150, 0), "network unreachable (nothing for 150 s, last none)"},
      {ev(EventCode::NetUnreachable, kNoValve, 150, 5), "network unreachable (nothing for 150 s, last dhcp)"},
      {ev(EventCode::NetUnreachable, kNoValve, 150, 6), "network unreachable (nothing for 150 s, last unknown)"},
      {ev(EventCode::NetUnreachable, kNoValve, 150, 257), "network unreachable (nothing for 150 s, last unknown)"},
      {ev(EventCode::NetUnreachable, kNoValve, 150, -1), "network unreachable (nothing for 150 s, last unknown)"},
      {ev(EventCode::NetReachable, kNoValve, 300), "network reachable again after 300 s"},
      {ev(EventCode::NetInterfaceRestart, kNoValve, 300, 1), "network interface restarted (eth, after 300 s)"},
      {ev(EventCode::NetInterfaceRestart, kNoValve, 300, 2), "network interface restarted (wifi, after 300 s)"},
      {ev(EventCode::NetInterfaceRestart, kNoValve, 300, 3), "network interface restarted (eth+wifi, after 300 s)"},
      {ev(EventCode::NetInterfaceRestart, kNoValve, 300, 4), "network interface restarted (unknown, after 300 s)"},
      {ev(EventCode::RequestRefused, kNoValve, 1, 0, "10.0.0.9"), "request refused (host) from 10.0.0.9"},
      {ev(EventCode::RequestRefused, kNoValve, 2), "request refused (origin)"},
      {ev(EventCode::RequestRefused, kNoValve, 3), "request refused (header)"},
      {ev(EventCode::RequestRefused, kNoValve, 4), "request refused (content type)"},
      {ev(EventCode::RequestRefused, kNoValve, 0), "request refused (unknown)"},
      {ev(EventCode::AuthLocked, kNoValve, 60, 1, "10.0.0.2"), "login locked for 60 s (lockout 1) for 10.0.0.2"},
      {ev(EventCode::AuthLocked, kNoValve, 900, 3), "login locked for 900 s (lockout 3)"},
      {ev(EventCode::LinkUp), "STM link up"},
      {ev(EventCode::LinkDegraded, kNoValve, 1), "STM link degraded (1 timeouts)"},
      {ev(EventCode::LinkDown, kNoValve, 5), "STM link down (5 timeouts)"},
      {ev(EventCode::StmResetByPolicy, kNoValve, 7, 65), "STM reset by link policy (7 timeouts in 65 s)"},
      {ev(EventCode::StmResetByUser), "STM reset by user"},
      {ev(EventCode::StmRebootDetected, kNoValve, 1), "STM reboot detected (uptime)"},
      {ev(EventCode::StmRebootDetected, kNoValve, 2), "STM reboot detected (resets)"},
      {ev(EventCode::StmRebootDetected, kNoValve, 3), "STM reboot detected (v1 heuristic)"},
      {ev(EventCode::StmRebootDetected, kNoValve, 4), "STM reboot detected (link recovered)"},
      {ev(EventCode::StmRebootDetected, kNoValve, 0), "STM reboot detected (unknown)"},
      {ev(EventCode::StmRebootDetected, kNoValve, 5), "STM reboot detected (unknown)"},
      {ev(EventCode::StmVersion, kNoValve, 2, 0x431, "2.0.0-revamped_C2"), "STM 2.0.0-revamped_C2 (protocol 2, hw 0x431)"},
      {ev(EventCode::StmVersion, kNoValve, 1, 0x23), "STM version unknown (protocol 1, hw 0x023)"},
      {ev(EventCode::StmIncompatible, kNoValve, 0, 0, "1.3.0"), "STM version 1.3.0 is not supported"},
      {ev(EventCode::StmIncompatible), "STM version unknown is not supported"},
      {ev(EventCode::StmRxOverflow, kNoValve, 12, 0), "UART receive overflow (12 total, esp side)"},
      {ev(EventCode::StmRxOverflow, kNoValve, 12, 1), "UART receive overflow (12 total, stm side)"},
      {ev(EventCode::StmParseErrors, kNoValve, 3, 1), "UART parse errors (3 total, stm side)"},
      {ev(EventCode::StmParseErrors, kNoValve, 3, 2), "UART parse errors (3 total, unknown side)"},
      {ev(EventCode::StmQueueFull, kNoValve, 1), "STM request queue full (command 1)"},
      {ev(EventCode::StmFlashStarted, kNoValve, 65536, 0, "fw.bin"), "STM flash started (65536 bytes, fw.bin)"},
      {ev(EventCode::StmFlashStarted, kNoValve, 65536), "STM flash started (65536 bytes)"},
      {ev(EventCode::StmFlashDone, kNoValve, 21000, 0, "2.0.0"), "STM flash done in 21000 ms (2.0.0)"},
      {ev(EventCode::StmFlashDone, kNoValve, 21000), "STM flash done in 21000 ms"},
      {ev(EventCode::StmFlashFailed, kNoValve, 4, 0x08004000, "writing"), "STM flash failed (error 4 at 0x08004000, writing)"},
      {ev(EventCode::StmFlashFailed, kNoValve, 4, -1), "STM flash failed (error 4 at 0xffffffff)"},
      {ev(EventCode::FailsafeActive, kNoValve, 0x0FFF, 1), "failsafe active on 12 valves (STM lease)"},
      {ev(EventCode::FailsafeActive, kNoValve, 0x0005, 2), "failsafe active on 2 valves (ESP)"},
      {ev(EventCode::FailsafeActive, kNoValve, 0, 3), "failsafe active on 0 valves (unknown)"},
      {ev(EventCode::FailsafeActive, kNoValve, -1, 0), "failsafe active on 32 valves (unknown)"},
      {ev(EventCode::FailsafeEnded, kNoValve, 3600, 1), "failsafe ended after 3600 s (STM lease)"},
      {ev(EventCode::FailsafeEnded, kNoValve, 60, 2), "failsafe ended after 60 s (ESP)"},
      {ev(EventCode::RegulatorLost, kNoValve, 1), "regulator lost (MQTT broker disconnected)"},
      {ev(EventCode::RegulatorLost, kNoValve, 2), "regulator lost (Home Assistant offline)"},
      {ev(EventCode::RegulatorLost, kNoValve, 0), "regulator lost (unknown)"},
      {ev(EventCode::RegulatorLost, kNoValve, 3), "regulator lost (unknown)"},
      {ev(EventCode::RegulatorBack, kNoValve, 125), "regulator back after 125 s"},
      {ev(EventCode::LeaseConfigFailed, kNoValve, 1, 3), "failsafe settings not accepted by the STM (no reply, 3 attempts)"},
      {ev(EventCode::LeaseConfigFailed, kNoValve, 2, 1), "failsafe settings not accepted by the STM (rejected, 1 attempts)"},
      {ev(EventCode::LeaseConfigFailed, kNoValve, 3, 3), "failsafe settings not accepted by the STM (read-back differs, 3 attempts)"},
      {ev(EventCode::LeaseConfigFailed, kNoValve, 4, 3), "failsafe settings not accepted by the STM (unknown, 3 attempts)"},
      {ev(EventCode::StmSafeMode, kNoValve, 3), "STM in safe mode (3 watchdog resets)"},
      {ev(EventCode::StmSafeModeEnded), "STM left safe mode"},
      {ev(EventCode::StmConfigRepaired, kNoValve, 5, 1), "STM configuration repaired (flags 0x05, 1 repairs)"},
      {ev(EventCode::StmConfigRepaired, kNoValve, 0xC0, 2), "STM configuration repaired (flags 0xc0, 2 repairs)"},
      {ev(EventCode::StmUartErrors, kNoValve, 7, 2), "STM UART errors (7 total, 2 bytes dropped)"},
      {ev(EventCode::StmEepromWaitTimeout, kNoValve, 10000, 1), "STM EEPROM write still pending after 10000 ms (STM reset)"},
      {ev(EventCode::StmEepromWaitTimeout, kNoValve, 10000, 2), "STM EEPROM write still pending after 10000 ms (flash)"},
      {ev(EventCode::StmEepromWaitTimeout, kNoValve, 12000, 3, "stm task silent"), "STM EEPROM write still pending after 12000 ms (ESP restart): stm task silent"},
      {ev(EventCode::StmEepromWaitTimeout, kNoValve, 1, 0), "STM EEPROM write still pending after 1 ms (unknown)"},
      {ev(EventCode::TargetsRestored, kNoValve, 12, 1), "desired targets restored for 12 valves (RTC)"},
      {ev(EventCode::TargetsRestored, kNoValve, 3, 2), "desired targets restored for 3 valves (NVS)"},
      {ev(EventCode::TargetsRestored, kNoValve, 3, 3), "desired targets restored for 3 valves (unknown)"},
      {ev(EventCode::StmProtectionSuspended), "STM short-circuit and inrush limits suspended until the next STM start"},
      {ev(EventCode::TargetSet, 0, 55, 3), "valve 1: target 55 % (mqtt)"},
      {ev(EventCode::TargetSet, 11, 0, 2), "valve 12: target 0 % (web)"},
      {ev(EventCode::TargetSet, 1, 1, 9), "valve 2: target 1 % (unknown)"},
      {ev(EventCode::TargetSet, 1, 1, -1), "valve 2: target 1 % (unknown)"},
      {ev(EventCode::TargetSet, 1, 1, 0), "valve 2: target 1 % (none)"},
      {ev(EventCode::TargetSet, 1, 1, 257), "valve 2: target 1 % (unknown)"},
      {ev(EventCode::ValveStateChanged, 2, 1, 2), "valve 3: state idle -> opening"},
      {ev(EventCode::ValveStateChanged, 2, 300, -1), "valve 3: state invalid -> invalid"},
      {ev(EventCode::ValveStateChanged, 2, 256, 0), "valve 3: state invalid -> nodata"},
      {ev(EventCode::ValveStateChanged, 2, 9, 265), "valve 3: state blocked -> invalid"},
      {ev(EventCode::ValveBlocked, 2, 2, -1), "valve 3: blocked (calibration retries 2)"},
      {ev(EventCode::ValveBlocked, 2, 2, 50), "valve 3: blocked (calibration retries 2, failsafe 50 %)"},
      {ev(EventCode::ValveBlocked, 2, 2, 0), "valve 3: blocked (calibration retries 2, failsafe 0 %)"},
      {ev(EventCode::ValveFailed, 2, 0, -1), "valve 3: failed"},
      {ev(EventCode::ValveFailed, 2, 0, 0), "valve 3: failed (none)"},
      {ev(EventCode::ValveFailed, 2, 0, 3), "valve 3: failed (short)"},
      {ev(EventCode::ValveFailed, 2, 0, 5), "valve 3: failed (inrush_trip)"},
      {ev(EventCode::ValveFailed, 2, 0, 6), "valve 3: failed (unknown)"},
      {ev(EventCode::ValveFailed, 2, 0, 256), "valve 3: failed (unknown)"},
      {ev(EventCode::ValveNoValve, 2), "valve 3: no valve detected"},
      {ev(EventCode::ValveRecovered, 2, 9), "valve 3: recovered (was blocked)"},
      {ev(EventCode::ValveRecovered, 2, 0, kHealthStale), "valve 3: recovered (data again)"},
      {ev(EventCode::ValveRecovered, 2, 0, kHealthTargetUnconfirmed), "valve 3: recovered (target confirmed)"},
      {ev(EventCode::ValveRecovered, 2, 0, kHealthStale | kHealthTargetUnconfirmed), "valve 3: recovered (data again, target confirmed)"},
      {ev(EventCode::ValveRecovered, 2, 0, 0), "valve 3: recovered ()"},
      {ev(EventCode::CalibStarted, 2, 0), "valve 3: calibration started"},
      {ev(EventCode::CalibStarted, kAllValves, 1), "all valves: calibration started (scheduled)"},
      {ev(EventCode::CalibStarted, kAllValves, 2), "all valves: calibration started (automatic retry)"},
      {ev(EventCode::CalibStarted, 2, 3), "valve 3: calibration started"},
      {ev(EventCode::CalibOk, 2, 3120, 3350), "valve 3: calibration ok (oc 3120, cc 3350)"},
      {ev(EventCode::CalibRetry, 2, 1), "valve 3: calibration retry 1"},
      {ev(EventCode::CalibFailed, 2, 2, -1), "valve 3: calibration failed after 2 retries"},
      {ev(EventCode::CalibFailed, 2, 2, 50), "valve 3: calibration failed after 2 retries, failsafe 50 %"},
      {ev(EventCode::CalibFailed, 2, 2, 0), "valve 3: calibration failed after 2 retries, failsafe 0 %"},
      {ev(EventCode::EarlyStop, 2, 2, 2), "valve 3: early stop (total 2, endstop)"},
      {ev(EventCode::EarlyStop, 2, 2, 3), "valve 3: early stop (total 2, early_endstop)"},
      {ev(EventCode::EarlyStop, 2, 2, -1), "valve 3: early stop (total 2, unknown)"},
      {ev(EventCode::EarlyStop, 2, 2, 256), "valve 3: early stop (total 2, unknown)"},
      {ev(EventCode::EarlyStop, 2, 2, 0), "valve 3: early stop (total 2, none)"},
      {ev(EventCode::EarlyStop, 2, 2, 7), "valve 3: early stop (total 2, aborted)"},
      {ev(EventCode::EarlyStop, 2, 2, 258), "valve 3: early stop (total 2, unknown)"},
      {ev(EventCode::CmdRejected, 2, 4), "valve 3: command rejected by the STM (total 4)"},
      {ev(EventCode::TargetNotConfirmed, 2, 40, 5), "valve 3: target 40 % not confirmed after 5 attempts"},
      {ev(EventCode::ValveStale, 2, 60), "valve 3: no data for 60 s"},
      {ev(EventCode::ServiceMoveDone, 2, 500, 1), "valve 3: service move done (500 counts, target)"},
      {ev(EventCode::CalibStrokeShort, 2, 3599, 3000), "valve 3: calibration stroke 3599 close to the minimum 3000"},
      {ev(EventCode::TempSensorFailed, kNoValve, 4, -1270, "28-84-37-94-97-ff-03-23"), "temp sensor 4 failed (raw -1270, 28-84-37-94-97-ff-03-23)"},
      {ev(EventCode::TempSensorFailed, kNoValve, 4, 850), "temp sensor 4 failed (raw 850)"},
      {ev(EventCode::TempSensorRecovered, kNoValve, 4), "temp sensor 4 recovered"},
      {ev(EventCode::SensorCountChanged, kNoValve, 3, 0), "temp sensor count 3"},
      {ev(EventCode::SensorCountChanged, kNoValve, 2, 1), "volt sensor count 2"},
      {ev(EventCode::VoltSensorFailed, kNoValve, 1, -1000), "volt sensor 1 failed (raw -1000)"},
      {ev(EventCode::ScheduledCalibration, kNoValve, 20260923, 3), "scheduled calibration (slot 20260923, 3 min late)"},
      {ev(EventCode::ScheduledCalibrationFailed, kNoValve, 20260923, 1), "scheduled calibration not confirmed (slot 20260923, no reply)"},
      {ev(EventCode::ScheduledCalibrationFailed, kNoValve, 20260923, 2), "scheduled calibration not confirmed (slot 20260923, not sent)"},
      {ev(EventCode::ScheduledCalibrationFailed, kNoValve, 20260923, 3), "scheduled calibration not confirmed (slot 20260923, no result)"},
      {ev(EventCode::ScheduledCalibrationFailed, kNoValve, 20260923, 4), "scheduled calibration not confirmed (slot 20260923, STM unsupported)"},
      {ev(EventCode::ScheduledCalibrationFailed, kNoValve, 20260923, 5), "scheduled calibration not confirmed (slot 20260923, unknown)"},
      {ev(EventCode::ScheduledCalibrationMissed, kNoValve, 20260923, 3), "scheduled calibration missed (slot 20260923, 3 attempts)"},
      {ev(static_cast<EventCode>(999)), "event 999"},
      {ev(EventCode::NetUp, 12, 1), "network up (eth)"},  // valve 12 is not a valve
  };
  for (const Row& r : rows) {
    CAPTURE(r.msg);
    CHECK(message(r.e) == r.msg);
  }
}

TEST_CASE("event message truncation and bad buffers") {
  const Event e = ev(EventCode::CalibOk, 2, 3120, 3350);
  char buf[8];
  CHECK(formatEventMessage(e, buf, sizeof buf) == 7);
  CHECK(strcmp(buf, "valve 3") == 0);
  char one[1] = {'X'};
  CHECK(formatEventMessage(e, one, 1) == 0);
  CHECK(one[0] == '\0');
  char two[2] = {'X', 'X'};
  CHECK(formatEventMessage(e, two, 2) == 1);
  CHECK(two[0] == 'v');
  CHECK(two[1] == '\0');
  // Numbers are cut at the buffer end as well.
  char nine[10];
  memset(nine, 'X', sizeof nine);
  CHECK(formatEventMessage(ev(EventCode::LowHeap, kNoValve, 123456, 7), nine, sizeof nine) == 9);
  CHECK(std::string(nine) == "low heap ");
  char cut[14];
  CHECK(formatEventMessage(ev(EventCode::LowHeap, kNoValve, 123456, 7), cut, sizeof cut) == 13);
  CHECK(std::string(cut) == "low heap (fre");
  // Extreme arguments are never truncated in a normal buffer.
  CHECK(message(ev(EventCode::CalibTimeMissing, kNoValve, INT32_MIN)) ==
        "scheduled calibration skipped, no valid time (slot -2147483648)");
  CHECK(message(ev(EventCode::StmResetByPolicy, kNoValve, INT32_MIN, INT32_MAX)) ==
        "STM reset by link policy (-2147483648 timeouts in 2147483647 s)");
  CHECK(message(ev(EventCode::ScheduledCalibration, 11, INT32_MIN, INT32_MIN)) ==
        "valve 12: scheduled calibration (slot -2147483648, -2147483648 min late)");
  CHECK(formatEventMessage(e, nullptr, 10) == 0);
  CHECK(formatEventMessage(e, buf, 0) == 0);
  // Unterminated text is read bounded.
  Event t = ev(EventCode::ConfigSaved, kNoValve, 1);
  memset(t.text, 'y', sizeof t.text);
  const std::string m = message(t);
  CHECK(m == "config saved (revision 1, " + std::string(kEventTextMax, 'y') + ")");
}

TEST_CASE("event lines with UTC time or uptime") {
  Event e = ev(EventCode::EarlyStop, 2, 2, 2, "", Severity::Warning);
  e.epoch = 1790172185;
  e.seq = 7;
  char line[160];
  const size_t n = formatEventLine(e, line, sizeof line);
  CHECK(n == strlen(line));
  CHECK(std::string(line) ==
        "#7 2026-09-23T14:03:05Z WARNING early_stop v3 valve 3: early stop (total 2, endstop)");
  Event big = ev(EventCode::MqttConnected);
  big.seq = 4294967295u;
  formatEventLine(big, line, sizeof line);
  CHECK(std::string(line) == "#4294967295 +0s INFO mqtt_connected MQTT connected");
  e.seq = 0;
  Event b = ev(EventCode::MqttConnected);
  b.uptimeS = 123;
  formatEventLine(b, line, sizeof line);
  CHECK(std::string(line) == "#0 +123s INFO mqtt_connected MQTT connected");

  struct T {
    uint32_t epoch;
    const char* prefix;
  };
  const T times[] = {
      {1, "1970-01-01T00:00:01Z"},
      {1709164800, "2024-02-29T00:00:00Z"},
      {951868799, "2000-02-29T23:59:59Z"},
      {4107542400u, "2100-03-01T00:00:00Z"},
      {0x80000000u, "2038-01-19T03:14:08Z"},
      {0xFFFFFFFFu, "2106-02-07T06:28:15Z"},
  };
  for (const T& t : times) {
    b.epoch = t.epoch;
    formatEventLine(b, line, sizeof line);
    CHECK(std::string(line) == "#0 " + std::string(t.prefix) + " INFO mqtt_connected MQTT connected");
  }
  const Severity sevs[] = {Severity::Debug, Severity::Error, Severity::Critical,
                           static_cast<Severity>(7), static_cast<Severity>(5)};
  const char* upper[] = {"DEBUG", "ERROR", "CRITICAL", "UNKNOWN", "UNKNOWN"};
  b.epoch = 0;
  b.uptimeS = 0;
  for (int i = 0; i < 5; ++i) {
    b.severity = sevs[i];
    formatEventLine(b, line, sizeof line);
    CHECK(std::string(line) == std::string("#0 +0s ") + upper[i] + " mqtt_connected MQTT connected");
  }
  Event all = ev(EventCode::CalibStarted, kAllValves, 1);
  all.seq = 12;
  formatEventLine(all, line, sizeof line);
  CHECK(std::string(line) ==
        "#12 +0s INFO calib_started all valves: calibration started (scheduled)");

  // Truncation keeps the prefix and stays terminated.
  char single[1] = {'X'};
  CHECK(formatEventLine(e, single, 1) == 0);
  CHECK(single[0] == '\0');
  char three[3];
  CHECK(formatEventLine(e, three, sizeof three) == 2);
  CHECK(std::string(three) == "#0");
  char small[15];
  CHECK(formatEventLine(e, small, sizeof small) == 14);
  CHECK(std::string(small) == "#0 2026-09-23T");
  char mid[33];
  CHECK(formatEventLine(e, mid, sizeof mid) == 32);
  CHECK(std::string(mid) == "#0 2026-09-23T14:03:05Z WARNING ");
  char exact[46];  // prefix fills it up to the message
  CHECK(formatEventLine(e, exact, sizeof exact) == 45);
  CHECK(std::string(exact) == "#0 2026-09-23T14:03:05Z WARNING early_stop v3");
  char exact2[47];
  CHECK(formatEventLine(e, exact2, sizeof exact2) == 46);
  CHECK(std::string(exact2) == "#0 2026-09-23T14:03:05Z WARNING early_stop v3 ");
  CHECK(formatEventLine(e, nullptr, 10) == 0);
  CHECK(formatEventLine(e, small, 0) == 0);
}

TEST_CASE("event line dates match an independent calendar for every day to 2106" *
          doctest::test_suite("slow")) {
  // Reference: walk the calendar day by day from 1970-01-01.
  static const unsigned kMonthDays[] = {31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31};
  unsigned y = 1970, m = 1, d = 1;
  Event e = ev(EventCode::MqttConnected);
  char line[96];
  char expect[32];
  for (uint32_t day = 0; day <= 0xFFFFFFFFu / 86400u; ++day) {
    // A different second of the day each time, so all fields vary.
    const uint32_t sod = (day * 7919u) % 86400u;
    e.epoch = day * 86400u + sod;
    if (e.epoch == 0) e.epoch = 1;
    formatEventLine(e, line, sizeof line);
    snprintf(expect, sizeof expect, "#0 %04u-%02u-%02uT%02u:%02u:%02uZ", y, m, d,
             static_cast<unsigned>(e.epoch % 86400u / 3600), static_cast<unsigned>(e.epoch % 3600 / 60),
             static_cast<unsigned>(e.epoch % 60));
    if (strncmp(line, expect, 23) != 0) {
      FAIL_CHECK("day " << day << ": " << line << " != " << expect);
      break;
    }
    const bool leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    const unsigned len = (m == 2 && leap) ? 29 : kMonthDays[m - 1];
    if (++d > len) {
      d = 1;
      if (++m > 12) {
        m = 1;
        ++y;
      }
    }
  }
  CHECK(y == 2106);
  // Every second of one day.
  for (uint32_t sod = 0; sod < 86400; ++sod) {
    e.epoch = 1790121600u + sod;  // 2026-09-23
    formatEventLine(e, line, sizeof line);
    snprintf(expect, sizeof expect, "#0 2026-09-23T%02u:%02u:%02uZ",
             static_cast<unsigned>(sod / 3600), static_cast<unsigned>(sod / 60 % 60),
             static_cast<unsigned>(sod % 60));
    if (strncmp(line, expect, 23) != 0) {
      FAIL_CHECK(line << " != " << expect);
      break;
    }
  }
}

TEST_CASE("event JSON") {
  Event e = ev(EventCode::EarlyStop, 2, 2, 2, "a\"b", Severity::Warning);
  e.seq = 5;
  e.epoch = 1790172185;
  e.uptimeS = 42;
  char buf[320];
  JsonWriter jw(buf, sizeof buf);
  REQUIRE(writeEventJson(jw, e));
  CHECK(std::string(buf) ==
        "{\"seq\":5,\"t\":1790172185,\"up\":42,\"sev\":\"warning\",\"code\":410,"
        "\"name\":\"early_stop\",\"valve\":3,\"a1\":2,\"a2\":2,\"text\":\"a\\\"b\","
        "\"msg\":\"valve 3: early stop (total 2, endstop)\"}");

  Event s = ev(EventCode::LinkDown, kNoValve, -5, 0, "", Severity::Error);
  s.seq = 1;
  jw.reset();
  REQUIRE(writeEventJson(jw, s));
  CHECK(std::string(buf) ==
        "{\"seq\":1,\"t\":null,\"up\":0,\"sev\":\"error\",\"code\":302,\"name\":\"link_down\","
        "\"valve\":null,\"a1\":-5,\"a2\":0,\"text\":\"\",\"msg\":\"STM link down (-5 timeouts)\"}");

  Event twelve = s;
  twelve.valve = kValveCount;  // not a valve index
  jw.reset();
  REQUIRE(writeEventJson(jw, twelve));
  CHECK(std::string(buf).find("\"valve\":null") != std::string::npos);
  Event last = s;
  last.valve = kValveCount - 1;
  jw.reset();
  REQUIRE(writeEventJson(jw, last));
  CHECK(std::string(buf).find("\"valve\":12,") != std::string::npos);

  Event u = s;
  memset(u.text, 'z', sizeof u.text);  // unterminated
  u.valve = kAllValves;
  jw.reset();
  REQUIRE(writeEventJson(jw, u));
  CHECK(std::string(buf).find("\"valve\":null") != std::string::npos);
  CHECK(std::string(buf).find("\"text\":\"" + std::string(kEventTextMax, 'z') + "\"") !=
        std::string::npos);

  char tiny[40];
  JsonWriter small(tiny, sizeof tiny);
  CHECK_FALSE(writeEventJson(small, e));
}

TEST_CASE("MQTT event JSON: single and aggregate") {
  Event e = ev(EventCode::ValveStale, 2, 60, 0, "", Severity::Warning);
  e.seq = 7;
  e.uptimeS = 12;
  char buf[512];
  JsonWriter jw(buf, sizeof buf);
  REQUIRE(writeMqttEventJson(jw, e, 0));
  CHECK(std::string(buf) ==
        "{\"seq\":7,\"t\":null,\"up\":12,\"sev\":\"warning\",\"code\":413,\"name\":\"valve_stale\","
        "\"event_type\":\"valve_stale\",\"valve\":3,\"a1\":60,\"a2\":0,\"text\":\"\","
        "\"msg\":\"valve 3: no data for 60 s\"}");
  // One valve bit is not an aggregate: the event's own valve counts.
  jw.reset();
  REQUIRE(writeMqttEventJson(jw, e, 1u << 7));
  CHECK(std::string(buf).find("\"valve\":3,\"a1\"") != std::string::npos);
  CHECK(std::string(buf).find("valves") == std::string::npos);

  jw.reset();
  REQUIRE(writeMqttEventJson(jw, e, (1u << 0) | (1u << 1) | (1u << 11)));
  CHECK(std::string(buf) ==
        "{\"seq\":7,\"t\":null,\"up\":12,\"sev\":\"warning\",\"code\":413,\"name\":\"valve_stale\","
        "\"event_type\":\"valve_stale\",\"valve\":null,\"valves\":[1,2,12],\"a1\":60,\"a2\":0,"
        "\"text\":\"\",\"msg\":\"valves 1, 2, 12: no data for 60 s\"}");
  // Bits above valve 12 are ignored: bit 0 + bit 12 is a single valve.
  jw.reset();
  REQUIRE(writeMqttEventJson(jw, e, 0x1001));
  CHECK(std::string(buf).find("\"valve\":3,\"a1\"") != std::string::npos);
  jw.reset();
  REQUIRE(writeMqttEventJson(jw, e, 0xFFFF));
  CHECK(std::string(buf).find("\"valves\":[1,2,3,4,5,6,7,8,9,10,11,12]") != std::string::npos);
  CHECK(std::string(buf).find("\"msg\":\"valves 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12: no data for "
                              "60 s\"") != std::string::npos);
  // The plain event JSON has no event_type.
  jw.reset();
  REQUIRE(writeEventJson(jw, e));
  CHECK(std::string(buf).find("event_type") == std::string::npos);
  // A system event keeps "valve":null.
  Event s = ev(EventCode::FailsafeActive, kNoValve, 3, 1, "", Severity::Warning);
  jw.reset();
  REQUIRE(writeMqttEventJson(jw, s, 0));
  CHECK(std::string(buf).find("\"event_type\":\"failsafe_active\",\"valve\":null,\"a1\":3") !=
        std::string::npos);
  char tiny[60];
  JsonWriter small(tiny, sizeof tiny);
  CHECK_FALSE(writeMqttEventJson(small, e, 3));
}

TEST_CASE("formatEventMessageMulti") {
  const Event e = ev(EventCode::ValveStale, 4, 60);
  char buf[160];
  CHECK(formatEventMessageMulti(e, 0, buf, sizeof buf) == strlen("valve 5: no data for 60 s"));
  CHECK(std::string(buf) == "valve 5: no data for 60 s");
  formatEventMessageMulti(e, 1u << 3, buf, sizeof buf);
  CHECK(std::string(buf) == "valve 5: no data for 60 s");
  formatEventMessageMulti(e, (1u << 3) | (1u << 9), buf, sizeof buf);
  CHECK(std::string(buf) == "valves 4, 10: no data for 60 s");
  formatEventMessageMulti(e, 0x0003, buf, sizeof buf);
  CHECK(std::string(buf) == "valves 1, 2: no data for 60 s");
  formatEventMessageMulti(e, 0xF000, buf, sizeof buf);
  CHECK(std::string(buf) == "valve 5: no data for 60 s");
  const Event all = ev(EventCode::CalibStarted, kAllValves, 0);
  formatEventMessageMulti(all, 0, buf, sizeof buf);
  CHECK(std::string(buf) == "all valves: calibration started");
  char cut[10];
  CHECK(formatEventMessageMulti(e, 0x0003, cut, sizeof cut) == 9);
  CHECK(std::string(cut) == "valves 1,");
  CHECK(formatEventMessageMulti(e, 0x0003, nullptr, 10) == 0);
  CHECK(formatEventMessageMulti(e, 0x0003, cut, 0) == 0);
}

TEST_CASE("formatUtcTimestamp") {
  char buf[40];
  CHECK(formatUtcTimestamp(0, buf, sizeof buf) == 25);
  CHECK(std::string(buf) == "1970-01-01T00:00:00+00:00");
  CHECK(formatUtcTimestamp(1790072393, buf, sizeof buf) == 25);
  CHECK(std::string(buf) == "2026-09-22T10:19:53+00:00");
  CHECK(formatUtcTimestamp(4102444799u, buf, sizeof buf) == 25);
  CHECK(std::string(buf) == "2099-12-31T23:59:59+00:00");
  CHECK(formatUtcTimestamp(1709164800, buf, sizeof buf) == 25);
  CHECK(std::string(buf) == "2024-02-29T00:00:00+00:00");
  char exact[26];
  CHECK(formatUtcTimestamp(1790072393, exact, sizeof exact) == 25);
  CHECK(std::string(exact) == "2026-09-22T10:19:53+00:00");
  char shortBuf[25];
  memset(shortBuf, 'x', sizeof shortBuf);
  CHECK(formatUtcTimestamp(1790072393, shortBuf, sizeof shortBuf) == 0);
  CHECK(shortBuf[0] == '\0');
  char one[1] = {'x'};
  CHECK(formatUtcTimestamp(0, one, 0) == 0);
  CHECK(one[0] == 'x');
  CHECK(formatUtcTimestamp(0, nullptr, 40) == 0);
}

TEST_CASE("parseSeverity fuzz: random bytes only ever match a case-folded name" *
          doctest::test_suite("fuzz")) {
  // Fixed seed: reproducible. Candidates are biased towards the real names so
  // that near misses (one flipped byte, wrong length) are exercised as well.
  const char* names[] = {"debug", "info", "warning", "error", "critical"};
  uint32_t seed = 0x5EED0001u;
  auto rnd = [&seed]() {
    seed = seed * 1664525u + 1013904223u;
    return seed >> 8;
  };
  char buf[12];
  size_t matches = 0;
  for (int iter = 0; iter < 50000; ++iter) {
    size_t len;
    if (rnd() % 2 == 0) {
      const char* n = names[rnd() % 5];
      len = strlen(n);
      memcpy(buf, n, len);
      for (size_t k = 0; k < len; ++k) {
        const uint32_t r = rnd() % 8;
        if (r == 0) buf[k] = static_cast<char>(rnd() & 0xFF);
        else if (r == 1 && buf[k] >= 'a' && buf[k] <= 'z') buf[k] = static_cast<char>(buf[k] - 32);
      }
      if (rnd() % 8 == 0) len = rnd() % (sizeof buf + 1);
    } else {
      len = rnd() % (sizeof buf + 1);
      for (size_t k = 0; k < len; ++k) buf[k] = static_cast<char>(rnd() & 0xFF);
    }
    // Reference: ASCII-only case-insensitive compare of exactly len bytes.
    int expect = -1;
    for (int i = 0; i < 5; ++i) {
      if (strlen(names[i]) != len) continue;
      size_t k = 0;
      while (k < len) {
        const char c = buf[k];
        const char lower = (c >= 'A' && c <= 'Z') ? static_cast<char>(c + 32) : c;
        if (lower != names[i][k]) break;
        ++k;
      }
      if (k == len) expect = i;
    }
    Severity out = Severity::Critical;
    const bool ok = parseSeverity(buf, len, out);
    REQUIRE(ok == (expect >= 0));
    if (ok) {
      ++matches;
      REQUIRE(out == static_cast<Severity>(expect));
    } else {
      REQUIRE(out == Severity::Critical);
    }
  }
  CHECK(matches > 250);  // the bias really produced hits
}

TEST_CASE("event message: a one-character text counts as text") {
  char buf[160];
  Event e = makeEvent(EventCode::Boot, Severity::Info, kNoValve, 1, 3, "x");
  formatEventMessage(e, buf, sizeof buf);
  CHECK(std::string(buf) == "boot (reset poweron, count 3, fw x)");
  e = makeEvent(EventCode::Boot, Severity::Info, kNoValve, 1, 3, "");
  formatEventMessage(e, buf, sizeof buf);
  CHECK(std::string(buf) == "boot (reset poweron, count 3)");
  e = makeEvent(EventCode::ConfigSaved, Severity::Info, kNoValve, 7, 0, "y");
  formatEventMessage(e, buf, sizeof buf);
  CHECK(std::string(buf) == "config saved (revision 7, y)");
}
