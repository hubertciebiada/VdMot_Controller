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
};

// DESIGN.md §13, the binding table.
const CodeRow kTable[] = {
    {EventCode::Boot, 100, "boot", Severity::Info},
    {EventCode::ConfigImported, 101, "config_imported", Severity::Info},
    {EventCode::ConfigSaved, 102, "config_saved", Severity::Info},
    {EventCode::ConfigDefaults, 103, "config_defaults", Severity::Error},
    {EventCode::FsFormatted, 104, "fs_formatted", Severity::Warning},
    {EventCode::EspOtaStarted, 105, "esp_ota_started", Severity::Info},
    {EventCode::EspOtaDone, 106, "esp_ota_done", Severity::Info},
    {EventCode::EspOtaFailed, 107, "esp_ota_failed", Severity::Error},
    {EventCode::AppMarkedValid, 108, "app_marked_valid", Severity::Info},
    {EventCode::RebootRequested, 109, "reboot_requested", Severity::Info},
    {EventCode::LowHeap, 110, "low_heap", Severity::Warning},
    {EventCode::TimeSynced, 111, "time_synced", Severity::Info},
    {EventCode::CalibTimeMissing, 112, "calib_time_missing", Severity::Warning},
    {EventCode::NetUp, 200, "net_up", Severity::Info},
    {EventCode::NetDown, 201, "net_down", Severity::Warning},
    {EventCode::MqttConnected, 202, "mqtt_connected", Severity::Info},
    {EventCode::MqttDisconnected, 203, "mqtt_disconnected", Severity::Warning},
    {EventCode::MqttCommandRejected, 204, "mqtt_command_rejected", Severity::Warning},
    {EventCode::HaDiscoverySent, 205, "ha_discovery_sent", Severity::Info},
    {EventCode::AuthFailed, 206, "auth_failed", Severity::Warning},
    {EventCode::LinkUp, 300, "link_up", Severity::Info},
    {EventCode::LinkDegraded, 301, "link_degraded", Severity::Info},
    {EventCode::LinkDown, 302, "link_down", Severity::Error},
    {EventCode::StmResetByPolicy, 303, "stm_reset_by_policy", Severity::Error},
    {EventCode::StmResetByUser, 304, "stm_reset_by_user", Severity::Info},
    {EventCode::StmRebootDetected, 305, "stm_reboot_detected", Severity::Warning},
    {EventCode::StmVersion, 306, "stm_version", Severity::Info},
    {EventCode::StmIncompatible, 307, "stm_incompatible", Severity::Error},
    {EventCode::StmRxOverflow, 308, "stm_rx_overflow", Severity::Warning},
    {EventCode::StmParseErrors, 309, "stm_parse_errors", Severity::Warning},
    {EventCode::StmQueueFull, 310, "stm_queue_full", Severity::Warning},
    {EventCode::StmFlashStarted, 311, "stm_flash_started", Severity::Info},
    {EventCode::StmFlashDone, 312, "stm_flash_done", Severity::Info},
    {EventCode::StmFlashFailed, 313, "stm_flash_failed", Severity::Critical},
    {EventCode::TargetSet, 400, "target_set", Severity::Info},
    {EventCode::ValveStateChanged, 401, "valve_state_changed", Severity::Debug},
    {EventCode::ValveBlocked, 402, "valve_blocked", Severity::Error},
    {EventCode::ValveFailed, 403, "valve_failed", Severity::Error},
    {EventCode::ValveNoValve, 404, "valve_no_valve", Severity::Warning},
    {EventCode::ValveRecovered, 405, "valve_recovered", Severity::Info},
    {EventCode::CalibStarted, 406, "calib_started", Severity::Info},
    {EventCode::CalibOk, 407, "calib_ok", Severity::Info},
    {EventCode::CalibRetry, 408, "calib_retry", Severity::Warning},
    {EventCode::CalibFailed, 409, "calib_failed", Severity::Error},
    {EventCode::EarlyStop, 410, "early_stop", Severity::Warning},
    {EventCode::CmdRejected, 411, "cmd_rejected", Severity::Warning},
    {EventCode::TargetNotConfirmed, 412, "target_not_confirmed", Severity::Warning},
    {EventCode::ValveStale, 413, "valve_stale", Severity::Warning},
    {EventCode::ServiceMoveDone, 414, "service_move_done", Severity::Info},
    {EventCode::TempSensorFailed, 500, "temp_sensor_failed", Severity::Warning},
    {EventCode::TempSensorRecovered, 501, "temp_sensor_recovered", Severity::Info},
    {EventCode::SensorCountChanged, 502, "sensor_count_changed", Severity::Info},
    {EventCode::VoltSensorFailed, 503, "volt_sensor_failed", Severity::Warning},
    {EventCode::ScheduledCalibration, 600, "scheduled_calibration", Severity::Info},
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
    const bool outcome = r.number == 407 || r.number == 408 || r.number == 409;
    CHECK(eventIsCalibrationOutcome(r.code) == outcome);
  }
  CHECK(strcmp(eventCodeName(static_cast<EventCode>(0)), "unknown") == 0);
  CHECK(strcmp(eventCodeName(static_cast<EventCode>(999)), "unknown") == 0);
  CHECK(eventDefaultSeverity(static_cast<EventCode>(999)) == Severity::Info);
  CHECK_FALSE(eventIsCalibrationOutcome(static_cast<EventCode>(999)));
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
      {ev(EventCode::RebootRequested, kNoValve, 5), "restart requested (unknown)"},
      {ev(EventCode::RebootRequested, kNoValve, -1), "restart requested (unknown)"},
      {ev(EventCode::LowHeap, kNoValve, 29000, 20000), "low heap (free 29000, min 20000)"},
      {ev(EventCode::TimeSynced, kNoValve, -3), "time synced (step -3 s)"},
      {ev(EventCode::CalibTimeMissing, kNoValve, 20260923), "scheduled calibration skipped, no valid time (slot 20260923)"},
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
      {ev(EventCode::ValveBlocked, 2, 2), "valve 3: blocked (calibration retries 2)"},
      {ev(EventCode::ValveFailed, 2), "valve 3: failed"},
      {ev(EventCode::ValveNoValve, 2), "valve 3: no valve detected"},
      {ev(EventCode::ValveRecovered, 2, 9), "valve 3: recovered (was blocked)"},
      {ev(EventCode::ValveRecovered, 2, 0, kHealthStale), "valve 3: recovered (data again)"},
      {ev(EventCode::ValveRecovered, 2, 0, kHealthTargetUnconfirmed), "valve 3: recovered (target confirmed)"},
      {ev(EventCode::ValveRecovered, 2, 0, kHealthStale | kHealthTargetUnconfirmed), "valve 3: recovered (data again, target confirmed)"},
      {ev(EventCode::ValveRecovered, 2, 0, 0), "valve 3: recovered ()"},
      {ev(EventCode::CalibStarted, 2, 0), "valve 3: calibration started"},
      {ev(EventCode::CalibStarted, kAllValves, 1), "all valves: calibration started (scheduled)"},
      {ev(EventCode::CalibStarted, kAllValves, 2), "all valves: calibration started"},
      {ev(EventCode::CalibOk, 2, 3120, 3350), "valve 3: calibration ok (oc 3120, cc 3350)"},
      {ev(EventCode::CalibRetry, 2, 1), "valve 3: calibration retry 1"},
      {ev(EventCode::CalibFailed, 2, 2), "valve 3: calibration failed after 2 retries"},
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
      {ev(EventCode::TempSensorFailed, kNoValve, 4, -1270, "28-84-37-94-97-ff-03-23"), "temp sensor 4 failed (raw -1270, 28-84-37-94-97-ff-03-23)"},
      {ev(EventCode::TempSensorFailed, kNoValve, 4, 850), "temp sensor 4 failed (raw 850)"},
      {ev(EventCode::TempSensorRecovered, kNoValve, 4), "temp sensor 4 recovered"},
      {ev(EventCode::SensorCountChanged, kNoValve, 3, 0), "temp sensor count 3"},
      {ev(EventCode::SensorCountChanged, kNoValve, 2, 1), "volt sensor count 2"},
      {ev(EventCode::VoltSensorFailed, kNoValve, 1, -1000), "volt sensor 1 failed (raw -1000)"},
      {ev(EventCode::ScheduledCalibration, kNoValve, 20260923, 3), "scheduled calibration (slot 20260923, 3 min late)"},
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
  char line[160];
  const size_t n = formatEventLine(e, line, sizeof line);
  CHECK(n == strlen(line));
  CHECK(std::string(line) ==
        "2026-09-23T14:03:05Z WARNING early_stop v3 valve 3: early stop (total 2, endstop)");
  Event b = ev(EventCode::MqttConnected);
  b.uptimeS = 123;
  formatEventLine(b, line, sizeof line);
  CHECK(std::string(line) == "+123s INFO mqtt_connected MQTT connected");

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
    CHECK(std::string(line) == std::string(t.prefix) + " INFO mqtt_connected MQTT connected");
  }
  const Severity sevs[] = {Severity::Debug, Severity::Error, Severity::Critical,
                           static_cast<Severity>(7), static_cast<Severity>(5)};
  const char* upper[] = {"DEBUG", "ERROR", "CRITICAL", "UNKNOWN", "UNKNOWN"};
  b.epoch = 0;
  b.uptimeS = 0;
  for (int i = 0; i < 5; ++i) {
    b.severity = sevs[i];
    formatEventLine(b, line, sizeof line);
    CHECK(std::string(line) == std::string("+0s ") + upper[i] + " mqtt_connected MQTT connected");
  }
  Event all = ev(EventCode::CalibStarted, kAllValves, 1);
  formatEventLine(all, line, sizeof line);
  CHECK(std::string(line) == "+0s INFO calib_started all valves: calibration started (scheduled)");

  // Truncation keeps the prefix and stays terminated.
  char single[1] = {'X'};
  CHECK(formatEventLine(e, single, 1) == 0);
  CHECK(single[0] == '\0');
  char small[12];
  CHECK(formatEventLine(e, small, sizeof small) == 11);
  CHECK(std::string(small) == "2026-09-23T");
  char mid[30];
  CHECK(formatEventLine(e, mid, sizeof mid) == 29);
  CHECK(std::string(mid) == "2026-09-23T14:03:05Z WARNING ");
  char exact[43];  // prefix fills it up to the message
  CHECK(formatEventLine(e, exact, sizeof exact) == 42);
  CHECK(std::string(exact) == "2026-09-23T14:03:05Z WARNING early_stop v3");
  char exact2[44];
  CHECK(formatEventLine(e, exact2, sizeof exact2) == 43);
  CHECK(std::string(exact2) == "2026-09-23T14:03:05Z WARNING early_stop v3 ");
  CHECK(formatEventLine(e, nullptr, 10) == 0);
  CHECK(formatEventLine(e, small, 0) == 0);
}

TEST_CASE("event line dates match an independent calendar for every day to 2106") {
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
    snprintf(expect, sizeof expect, "%04u-%02u-%02uT%02u:%02u:%02uZ", y, m, d,
             static_cast<unsigned>(e.epoch % 86400u / 3600), static_cast<unsigned>(e.epoch % 3600 / 60),
             static_cast<unsigned>(e.epoch % 60));
    if (strncmp(line, expect, 20) != 0) {
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
    snprintf(expect, sizeof expect, "2026-09-23T%02u:%02u:%02uZ", static_cast<unsigned>(sod / 3600),
             static_cast<unsigned>(sod / 60 % 60), static_cast<unsigned>(sod % 60));
    if (strncmp(line, expect, 20) != 0) {
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

TEST_CASE("parseSeverity fuzz: random bytes only ever match a case-folded name") {
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
