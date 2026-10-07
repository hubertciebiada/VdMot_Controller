//! Port of test/native/test_event_log.cpp: event registry, ring buffer and text/JSON
//! formatting.
//!
//! A Rust `Severity`/`EventCode` cannot hold an out-of-range number, so the C++ checks of the
//! "unknown" names of `static_cast<Severity>(5)` or `static_cast<EventCode>(999)` become
//! `from_raw(v) == None`. A `Text<23>` cannot hold the C++ unterminated 24-byte text: the
//! "unterminated" cases use a full text of 23 bytes. Rust writes no NUL after a text: the C++
//! checks of that NUL check the length and the untouched byte after the text.

use std::vec::Vec;

use super::*;
use crate::common::contains_bytes;
use crate::test_support::{assert_text, Lcg};

use EventMqtt::{Always, No, WarnPlus as Warn};
use Severity::{Critical, Debug, Error, Info, Warning};

type C = EventCode;

/// DESIGN.md section 13, the binding table (registry order): code, number, name, severity,
/// MQTT class.
#[rustfmt::skip]
const TABLE: [(EventCode, u16, &str, Severity, EventMqtt); 87] = [
    (C::Boot, 100, "boot", Info, Warn),
    (C::ConfigImported, 101, "config_imported", Info, Warn),
    (C::ConfigSaved, 102, "config_saved", Info, Warn),
    (C::ConfigDefaults, 103, "config_defaults", Error, Warn),
    (C::FsFormatted, 104, "fs_formatted", Warning, Warn),
    (C::EspOtaStarted, 105, "esp_ota_started", Info, Warn),
    (C::EspOtaDone, 106, "esp_ota_done", Info, Warn),
    (C::EspOtaFailed, 107, "esp_ota_failed", Error, Warn),
    (C::AppMarkedValid, 108, "app_marked_valid", Info, Warn),
    (C::RebootRequested, 109, "reboot_requested", Info, Warn),
    (C::LowHeap, 110, "low_heap", Warning, Warn),
    (C::TimeSynced, 111, "time_synced", Info, Warn),
    (C::CalibTimeMissing, 112, "calib_time_missing", Warning, Warn),
    (C::FactoryResetSkipped, 113, "factory_reset_skipped", Warning, Warn),
    (C::StackLow, 114, "stack_low", Warning, Warn),
    (C::HeapFragmented, 115, "heap_fragmented", Warning, Warn),
    (C::LogWriteFailed, 116, "log_write_failed", Warning, Warn),
    (C::ImportDropped, 117, "import_dropped", Warning, Warn),
    (C::ConfigRestored, 118, "config_restored", Warning, Warn),
    (C::ConfigRepaired, 119, "config_repaired", Warning, Warn),
    (C::ConfigNewerSchema, 120, "config_newer_schema", Warning, Warn),
    (C::FilesRemoved, 121, "files_removed", Info, No),
    (C::HeapCritical, 122, "heap_critical", Error, Warn),
    (C::NetUp, 200, "net_up", Info, Warn),
    (C::NetDown, 201, "net_down", Warning, Warn),
    (C::MqttConnected, 202, "mqtt_connected", Info, Warn),
    (C::MqttDisconnected, 203, "mqtt_disconnected", Warning, Warn),
    (C::MqttCommandRejected, 204, "mqtt_command_rejected", Warning, Warn),
    (C::HaDiscoverySent, 205, "ha_discovery_sent", Info, Warn),
    (C::AuthFailed, 206, "auth_failed", Warning, No),
    (C::NetTrialStarted, 207, "net_trial_started", Info, No),
    (C::NetTrialConfirmed, 208, "net_trial_confirmed", Info, No),
    (C::NetTrialReverted, 209, "net_trial_reverted", Warning, Warn),
    (C::NetUnreachable, 210, "net_unreachable", Warning, Warn),
    (C::NetReachable, 211, "net_reachable", Info, No),
    (C::NetInterfaceRestart, 212, "net_interface_restart", Warning, Warn),
    (C::RequestRefused, 213, "request_refused", Warning, Warn),
    (C::AuthLocked, 214, "auth_locked", Warning, No),
    (C::LinkUp, 300, "link_up", Info, Warn),
    (C::LinkDegraded, 301, "link_degraded", Info, Warn),
    (C::LinkDown, 302, "link_down", Error, Warn),
    (C::StmResetByPolicy, 303, "stm_reset_by_policy", Error, Warn),
    (C::StmResetByUser, 304, "stm_reset_by_user", Info, Warn),
    (C::StmRebootDetected, 305, "stm_reboot_detected", Warning, Warn),
    (C::StmVersion, 306, "stm_version", Info, Warn),
    (C::StmIncompatible, 307, "stm_incompatible", Error, Warn),
    (C::StmRxOverflow, 308, "stm_rx_overflow", Warning, Warn),
    (C::StmParseErrors, 309, "stm_parse_errors", Warning, Warn),
    (C::StmQueueFull, 310, "stm_queue_full", Warning, Warn),
    (C::StmFlashStarted, 311, "stm_flash_started", Info, Warn),
    (C::StmFlashDone, 312, "stm_flash_done", Info, Warn),
    (C::StmFlashFailed, 313, "stm_flash_failed", Critical, Warn),
    (C::FailsafeActive, 314, "failsafe_active", Warning, Always),
    (C::FailsafeEnded, 315, "failsafe_ended", Info, Always),
    (C::RegulatorLost, 316, "regulator_lost", Warning, Warn),
    (C::RegulatorBack, 317, "regulator_back", Info, Always),
    (C::LeaseConfigFailed, 318, "lease_config_failed", Warning, Warn),
    (C::StmSafeMode, 319, "stm_safe_mode", Critical, Always),
    (C::StmSafeModeEnded, 320, "stm_safe_mode_ended", Info, Always),
    (C::StmConfigRepaired, 321, "stm_config_repaired", Warning, Warn),
    (C::StmUartErrors, 322, "stm_uart_errors", Warning, Warn),
    (C::StmEepromWaitTimeout, 323, "stm_eeprom_wait_timeout", Warning, Warn),
    (C::TargetsRestored, 324, "targets_restored", Info, No),
    (C::StmProtectionSuspended, 325, "stm_protection_suspended", Error, Warn),
    (C::TargetSet, 400, "target_set", Info, Warn),
    (C::ValveStateChanged, 401, "valve_state_changed", Debug, Warn),
    (C::ValveBlocked, 402, "valve_blocked", Error, Warn),
    (C::ValveFailed, 403, "valve_failed", Error, Warn),
    (C::ValveNoValve, 404, "valve_no_valve", Warning, Warn),
    (C::ValveRecovered, 405, "valve_recovered", Info, Warn),
    (C::CalibStarted, 406, "calib_started", Info, Warn),
    (C::CalibOk, 407, "calib_ok", Info, Always),
    (C::CalibRetry, 408, "calib_retry", Warning, Always),
    (C::CalibFailed, 409, "calib_failed", Error, Always),
    (C::EarlyStop, 410, "early_stop", Warning, Warn),
    (C::CmdRejected, 411, "cmd_rejected", Warning, Warn),
    (C::TargetNotConfirmed, 412, "target_not_confirmed", Warning, Warn),
    (C::ValveStale, 413, "valve_stale", Warning, Warn),
    (C::ServiceMoveDone, 414, "service_move_done", Info, Warn),
    (C::CalibStrokeShort, 415, "calib_stroke_short", Warning, Warn),
    (C::TempSensorFailed, 500, "temp_sensor_failed", Warning, Warn),
    (C::TempSensorRecovered, 501, "temp_sensor_recovered", Info, Warn),
    (C::SensorCountChanged, 502, "sensor_count_changed", Info, Warn),
    (C::VoltSensorFailed, 503, "volt_sensor_failed", Warning, Warn),
    (C::ScheduledCalibration, 600, "scheduled_calibration", Info, Warn),
    (C::ScheduledCalibrationFailed, 601, "scheduled_calibration_failed", Warning, Warn),
    (C::ScheduledCalibrationMissed, 602, "scheduled_calibration_missed", Error, Warn),
];

/// C++ `ev(c, valve, a1, a2, text)` (severity Info).
fn ev(c: EventCode, valve: u8, a1: i32, a2: i32, text: &str) -> Event {
    make_event(c, Info, valve, a1, a2, text.as_bytes())
}

/// C++ `ev(c, valve, a1, a2, text, sev)`.
fn ev_sev(c: EventCode, valve: u8, a1: i32, a2: i32, text: &str, sev: Severity) -> Event {
    make_event(c, sev, valve, a1, a2, text.as_bytes())
}

/// C++ `ev(c)`.
fn ev0(c: EventCode) -> Event {
    ev(c, NO_VALVE, 0, 0, "")
}

/// C++ `message(e)`: the message in a 160-byte buffer.
fn message(e: &Event) -> Vec<u8> {
    let mut buf = [0u8; 160];
    let n = format_event_message(e, &mut buf);
    assert!(n < buf.len());
    buf[..n].to_vec()
}

fn line_of(e: &Event, cap: usize) -> Vec<u8> {
    let mut buf = [0u8; 200];
    let n = format_event_line(e, &mut buf[..cap]);
    buf[..n].to_vec()
}

fn text_of(s: &[u8]) -> Text<EVENT_TEXT_MAX> {
    Text::from_slice(s).unwrap()
}

#[test]
fn severity_names_parsing_and_syslog_mapping() {
    let names = ["debug", "info", "warning", "error", "critical"];
    let syslog = [7u8, 6, 4, 3, 2];
    for i in 0..5u8 {
        let s = Severity::from_raw(i).unwrap();
        assert_eq!(s as u8, i);
        assert_eq!(severity_name(s), names[usize::from(i)]);
        assert_eq!(syslog_severity(s), syslog[usize::from(i)]);
        assert_eq!(parse_severity(names[usize::from(i)].as_bytes()), Some(s));
    }
    // C++ severityName(Severity(5)) == "unknown", syslogSeverity(Severity(9)) == 7
    assert_eq!(Severity::from_raw(5), None);
    assert_eq!(Severity::from_raw(9), None);

    assert_eq!(parse_severity(b"WARNING"), Some(Warning));
    assert_eq!(parse_severity(b"Critical"), Some(Critical));
    assert_eq!(parse_severity(b"DeBuG"), Some(Debug));
    assert_eq!(parse_severity(b"INFO"), Some(Info));
    assert_eq!(parse_severity(&b"errorXYZ"[..5]), Some(Error)); // exactly len bytes
    let bad: [&[u8]; 15] = [
        b"warn",
        b"",
        b"warnings",
        b"debug ",
        b" info",
        b"err0r",
        b"crit",
        b"inf\x0f",
        b"xnfo",
        b"Xebug",
        b"ebug",
        b"nfo",
        b"iNfo\x80",
        b"inf\xef",
        b"inF\x4f\x01",
    ];
    for b in bad {
        assert_eq!(parse_severity(b), None, "{}", b.escape_ascii());
    }
    assert_eq!(parse_severity(b"info\0"), None);
    let exact: Vec<u8> = std::vec![b'e', b'R', b'r', b'o', b'r']; // no terminator after it
    assert_eq!(parse_severity(&exact), Some(Error));
    // C++ parseSeverity(nullptr, 4, out): no Rust form
    assert_eq!(parse_severity(&b"info"[..3]), None);
    // C++ "out unchanged by the failures": the None results
}

#[test]
fn event_code_registry_matches_the_design_table() {
    for (code, number, name, sev, mqtt) in TABLE {
        assert_eq!(code as u16, number);
        assert_eq!(event_code_name(code), name, "{number}");
        assert_eq!(event_default_severity(code), sev, "{number}");
        assert_eq!(event_mqtt(code), mqtt, "{number}");
        let outcome = number == 407 || number == 408 || number == 409;
        assert_eq!(event_is_calibration_outcome(code), outcome, "{number}");
    }
    // C++ eventCodeName(EventCode(0)) and (999) == "unknown", eventDefaultSeverity(999) == Info,
    // eventMqtt(999) == No, eventIsCalibrationOutcome(999) false: an EventCode cannot hold them
    assert_eq!(EventCode::from_raw(0), None);
    assert_eq!(EventCode::from_raw(999), None);
}

#[test]
fn event_code_from_raw_knows_exactly_the_registry_numbers() {
    let mut found = 0;
    for v in 0..=u16::MAX {
        let row = TABLE.iter().find(|r| r.1 == v);
        assert_eq!(EventCode::from_raw(v), row.map(|r| r.0), "{v}");
        found += usize::from(row.is_some());
    }
    assert_eq!(found, 87);
    // the registry holds every code once, in the order of the design table
    assert_eq!(CODES.len(), TABLE.len());
    for (ci, row) in CODES.iter().zip(TABLE) {
        assert_eq!(ci.code, row.0);
    }
}

#[test]
fn event_mqtt_names_lists_every_published_code_in_registry_order() {
    let expected: Vec<&str> = TABLE.iter().filter(|r| r.4 != No).map(|r| r.2).collect();
    assert_eq!(expected.len(), 80);
    let mut names = [""; 100];
    assert_eq!(event_mqtt_names(&mut names), expected.len());
    for (i, want) in expected.iter().enumerate() {
        assert_eq!(names[i], *want, "{i}");
    }
    assert_eq!(names[expected.len()], ""); // untouched
                                           // A short array gets the first names, the total is still returned.
    let mut few = [""; 3];
    assert_eq!(event_mqtt_names(&mut few[..2]), expected.len());
    assert_eq!(few[0], "boot");
    assert_eq!(few[1], "config_imported");
    assert_eq!(few[2], "");
    // C++ eventMqttNames(nullptr, 5): no Rust form
    assert_eq!(event_mqtt_names(&mut few[..0]), expected.len());
}

#[test]
fn event_mqtt_name_one_name_of_that_list_at_a_time() {
    let expected: Vec<&str> = TABLE.iter().filter(|r| r.4 != No).map(|r| r.2).collect();
    for (i, want) in expected.iter().enumerate() {
        assert_eq!(event_mqtt_name(i), Some(*want), "{i}");
    }
    assert_eq!(event_mqtt_name(expected.len()), None);
    assert_eq!(event_mqtt_name(usize::MAX), None);
}

#[test]
fn event_reaches_mqtt_always_or_warn_plus_at_warning_and_above() {
    let reaches = |c: EventCode, sev: Severity, valve: u8| {
        event_reaches_mqtt(&make_event(c, sev, valve, 0, 0, b""))
    };
    assert!(reaches(C::CalibOk, Info, 1));
    assert!(reaches(C::CalibOk, Debug, 1));
    assert!(reaches(C::FailsafeEnded, Info, NO_VALVE));
    assert!(!reaches(C::Boot, Info, NO_VALVE));
    assert!(reaches(C::Boot, Warning, NO_VALVE));
    assert!(reaches(C::EarlyStop, Warning, 1));
    assert!(reaches(C::LinkDown, Error, NO_VALVE));
    assert!(reaches(C::StmFlashFailed, Critical, NO_VALVE));
    assert!(!reaches(C::ServiceMoveDone, Info, 1));
    // Codes without MQTT never reach it, whatever the severity.
    assert!(!reaches(C::FilesRemoved, Critical, NO_VALVE));
    // C++ eventReachesMqtt of EventCode(999), Critical: no Rust form
}

#[test]
fn make_event_fills_and_truncates() {
    let e = make_event(C::NetUp, Warning, 3, -7, 9, b"0123456789abcdefghijklmnopq");
    assert_eq!(e.code, C::NetUp);
    assert_eq!(e.severity, Warning);
    assert_eq!(e.valve, 3);
    assert_eq!(e.arg1, -7);
    assert_eq!(e.arg2, 9);
    assert_eq!(e.text.len(), EVENT_TEXT_MAX);
    assert_text(&e.text, "0123456789abcdefghijklm");
    assert_eq!(e.seq, 0);
    // C++ makeEvent(.., nullptr): the empty text
    let n = make_event(C::Boot, Info, NO_VALVE, 0, 0, b"");
    assert!(n.text.is_empty());
    // a C string ends at its NUL
    let z = make_event(C::Boot, Info, NO_VALVE, 0, 0, b"ab\0cd");
    assert_text(&z.text, "ab");
}

#[test]
fn event_defaults_are_the_cpp_member_initialisers() {
    let e = Event::default();
    assert_eq!(
        (e.seq, e.uptime_s, e.epoch, e.code, e.severity, e.valve, e.arg1, e.arg2),
        (0, 0, 0, C::Boot, Info, NO_VALVE, 0, 0)
    );
    assert!(e.text.is_empty());
    let f = EventFilter::default();
    assert_eq!((f.since_seq, f.min_severity, f.valve), (0, Debug, NO_VALVE));
    assert_eq!(EVENT_TEXT_MAX, 23);
    assert_eq!(
        (0..=8).map(RebootReason::from_raw).collect::<Vec<_>>(),
        [
            Some(RebootReason::User),
            Some(RebootReason::Ota),
            Some(RebootReason::NetWatchdog),
            Some(RebootReason::FactoryReset),
            Some(RebootReason::Rollback),
            Some(RebootReason::NetRevert),
            Some(RebootReason::HeapGuard),
            Some(RebootReason::SwitchBack),
            None
        ]
    );
    assert_eq!(RebootReason::HeapGuard as u8, 6);
    assert_eq!(RebootReason::SwitchBack as u8, 7);
    assert_eq!(
        [
            EventMqtt::No as u8,
            EventMqtt::WarnPlus as u8,
            EventMqtt::Always as u8
        ],
        [0, 1, 2]
    );
}

#[test]
fn event_log_without_storage_drops_everything() {
    // C++ EventLog(nullptr, 10) and EventLog(buf, 0): both are capacity 0
    let mut a = EventLog::<0>::new();
    assert_eq!(a.capacity(), 0);
    assert_eq!(a.append(&ev0(C::Boot)), 0);
    assert_eq!(a.dropped(), 1);
    assert_eq!(a.size(), 0);
    assert_eq!(a.first_seq(), 0);
    assert_eq!(a.last_seq(), 0);
    let mut b = EventLog::<0>::default();
    assert_eq!(b.append(&ev0(C::Boot)), 0);
    assert_eq!(b.dropped(), 1);
    let mut buf = [Event::default()];
    let mut next = 42;
    let f = EventFilter {
        since_seq: 7,
        ..EventFilter::default()
    };
    assert_eq!(b.read(&f, &mut buf, &mut next), 0);
    assert_eq!(next, 7);
    assert!(b.get(1).is_none());
}

#[test]
fn event_log_ring_sequence_numbers_overwrite_get_clear() {
    let mut log = EventLog::<3>::new();
    assert_eq!(log.capacity(), 3);
    assert_eq!(log.append(&ev(C::Boot, NO_VALVE, 1, 0, "")), 1);
    assert_eq!(log.size(), 1);
    assert_eq!(log.first_seq(), 1);
    assert_eq!(log.last_seq(), 1);
    assert_eq!(log.append(&ev(C::Boot, NO_VALVE, 2, 0, "")), 2);
    assert_eq!(log.append(&ev(C::Boot, NO_VALVE, 3, 0, "")), 3);
    assert_eq!(log.dropped(), 0);
    assert_eq!(log.append(&ev(C::Boot, NO_VALVE, 4, 0, "")), 4);
    assert_eq!(log.append(&ev(C::Boot, NO_VALVE, 5, 0, "")), 5);
    assert_eq!(log.size(), 3);
    assert_eq!(log.dropped(), 2);
    assert_eq!(log.first_seq(), 3);
    assert_eq!(log.last_seq(), 5);
    assert!(log.get(0).is_none());
    assert!(log.get(2).is_none());
    assert!(log.get(6).is_none());
    let out = log.get(4).unwrap();
    assert_eq!(out.seq, 4);
    assert_eq!(out.arg1, 4);
    assert_eq!(log.get(3).unwrap().arg1, 3);

    // C++ "unterminated text is terminated in storage": a full text is stored whole
    let mut raw = ev0(C::NetUp);
    raw.text = text_of(&[b'x'; EVENT_TEXT_MAX]);
    assert_eq!(log.append(&raw), 6);
    assert_eq!(log.get(6).unwrap().text.len(), EVENT_TEXT_MAX);

    log.clear();
    assert_eq!(log.size(), 0);
    assert_eq!(log.first_seq(), 0);
    assert_eq!(log.last_seq(), 0);
    assert!(log.get(6).is_none());
    assert_eq!(log.append(&ev0(C::Boot)), 7); // numbering continues
    assert_eq!(log.first_seq(), 7);
    assert_eq!(log.last_seq(), 7);
}

#[test]
fn event_log_stores_a_copy_with_its_own_seq() {
    let mut log = EventLog::<2>::new();
    let mut e = ev(C::LinkDown, 4, -1, 2, "abc");
    e.seq = 99;
    e.uptime_s = 5;
    e.epoch = 6;
    assert_eq!(log.append(&e), 1);
    let stored = log.get(1).unwrap();
    assert_eq!(stored.seq, 1);
    assert_eq!(
        Event {
            seq: 99,
            ..stored.clone()
        },
        e
    );
}

#[test]
fn event_log_seq_skips_0_after_the_last_u32() {
    // C++ NOMUTATE: "wraps after 2^32 events, not reachable in tests"; reached here directly
    let mut log = EventLog::<2>::new();
    log.next_seq = u32::MAX;
    assert_eq!(log.append(&ev0(C::Boot)), u32::MAX);
    assert_eq!(log.append(&ev0(C::Boot)), 1);
    assert_eq!(log.append(&ev0(C::Boot)), 2);
    assert_eq!((log.first_seq(), log.last_seq()), (1, 2));
}

#[test]
fn event_log_read_filters_and_cursors() {
    let mut log = EventLog::<16>::new();
    log.append(&ev_sev(C::Boot, NO_VALVE, 0, 0, "", Info)); // 1
    log.append(&ev_sev(C::ValveStateChanged, 2, 0, 0, "", Debug)); // 2
    log.append(&ev_sev(C::ValveBlocked, 2, 0, 0, "", Error)); // 3
    log.append(&ev_sev(C::CalibStarted, ALL_VALVES, 0, 0, "", Info)); // 4
    log.append(&ev_sev(C::ValveNoValve, 5, 0, 0, "", Warning)); // 5
    log.append(&ev_sev(C::LinkDown, NO_VALVE, 0, 0, "", Critical)); // 6

    let mut out: [Event; 16] = core::array::from_fn(|_| Event::default());
    let mut next = 0;
    let all = EventFilter::default();
    assert_eq!(log.read(&all, &mut out, &mut next), 6);
    assert_eq!(next, 6);
    for (i, e) in out.iter().take(6).enumerate() {
        assert_eq!(e.seq as usize, i + 1);
    }

    let mut since = EventFilter {
        since_seq: 4,
        ..EventFilter::default()
    };
    assert_eq!(log.read(&since, &mut out, &mut next), 2);
    assert_eq!(out[0].seq, 5);
    assert_eq!(next, 6);
    since.since_seq = 6;
    assert_eq!(log.read(&since, &mut out, &mut next), 0);
    assert_eq!(next, 6);
    since.since_seq = 100;
    assert_eq!(log.read(&since, &mut out, &mut next), 0);
    assert_eq!(next, 100);

    let warn = EventFilter {
        min_severity: Warning,
        ..EventFilter::default()
    };
    assert_eq!(log.read(&warn, &mut out, &mut next), 3);
    assert_eq!(out[0].seq, 3);
    assert_eq!(out[1].seq, 5);
    assert_eq!(out[2].seq, 6);
    assert_eq!(next, 6);

    let valve = EventFilter {
        valve: 2,
        ..EventFilter::default()
    };
    assert_eq!(log.read(&valve, &mut out, &mut next), 3);
    assert_eq!(out[0].seq, 2);
    assert_eq!(out[1].seq, 3);
    assert_eq!(out[2].seq, 4); // "all valves" matches every valve filter
    assert_eq!(next, 6); // the last examined event, although filtered out

    // Limited output: the cursor stops at the last copied event.
    assert_eq!(log.read(&all, &mut out[..2], &mut next), 2);
    assert_eq!(out[1].seq, 2);
    assert_eq!(next, 2);
    // Filtered-out events before the limit are skipped by the cursor.
    assert_eq!(log.read(&warn, &mut out[..1], &mut next), 1);
    assert_eq!(out[0].seq, 3);
    assert_eq!(next, 3);
    // C++ read(all, nullptr, 5, next) and read(all, out, 0, next): the empty output
    assert_eq!(log.read(&all, &mut out[..0], &mut next), 0);
    assert_eq!(next, 0);

    // get() on a partly filled ring.
    for seq in 1..=6 {
        assert_eq!(log.get(seq).unwrap().seq, seq);
    }
    assert_eq!(log.get(6).unwrap().code, C::LinkDown);
    assert!(log.get(7).is_none());
    assert!(log.get(0).is_none());

    // Wrapped ring: oldest first.
    let mut w = EventLog::<2>::new();
    for i in 0..5 {
        w.append(&ev(C::Boot, NO_VALVE, i, 0, ""));
    }
    assert_eq!(w.read(&all, &mut out[..5], &mut next), 2);
    assert_eq!(out[0].seq, 4);
    assert_eq!(out[1].seq, 5);
    assert_eq!(next, 5);
}

#[test]
fn event_log_read_and_get_after_clear_start_at_head() {
    // clear() keeps head: the next events start where the ring was
    let mut log = EventLog::<3>::new();
    for i in 0..4 {
        log.append(&ev(C::Boot, NO_VALVE, i, 0, ""));
    }
    log.clear();
    log.append(&ev(C::Boot, NO_VALVE, 10, 0, ""));
    log.append(&ev(C::Boot, NO_VALVE, 11, 0, ""));
    let mut out: [Event; 3] = core::array::from_fn(|_| Event::default());
    let mut next = 0;
    assert_eq!(log.read(&EventFilter::default(), &mut out, &mut next), 2);
    assert_eq!(
        (out[0].seq, out[0].arg1, out[1].seq, out[1].arg1),
        (5, 10, 6, 11)
    );
    assert_eq!((log.first_seq(), log.last_seq(), next), (5, 6, 6));
    assert_eq!(log.get(5).unwrap().arg1, 10);
}

#[test]
fn event_messages_for_every_code() {
    #[rustfmt::skip]
    let rows: Vec<(Event, &str)> = std::vec![
        (ev(C::Boot, NO_VALVE, 6, 17, "2.0.0-revamped"), "boot (reset task_wdt, count 17, fw 2.0.0-revamped)"),
        (ev(C::Boot, NO_VALVE, 1, 1, ""), "boot (reset poweron, count 1)"),
        (ev(C::Boot, NO_VALVE, 11, 1, ""), "boot (reset unknown, count 1)"),
        (ev(C::Boot, NO_VALVE, -1, 1, ""), "boot (reset unknown, count 1)"),
        (ev(C::Boot, NO_VALVE, 10, 2, ""), "boot (reset sdio, count 2)"),
        (ev(C::Boot, NO_VALVE, 0, 2, ""), "boot (reset unknown, count 2)"),
        (ev(C::ConfigImported, NO_VALVE, 40, 2, "mqtt.port"), "legacy config imported (40 keys, 2 rejected, first mqtt.port)"),
        (ev(C::ConfigImported, NO_VALVE, 40, 0, ""), "legacy config imported (40 keys, 0 rejected)"),
        (ev(C::ConfigSaved, NO_VALVE, 3, 0, "web"), "config saved (revision 3, web)"),
        (ev(C::ConfigSaved, NO_VALVE, 3, 0, ""), "config saved (revision 3)"),
        (ev(C::ConfigDefaults, NO_VALVE, 2, 0, ""), "stored config unusable, using defaults (reason 2)"),
        (ev(C::FsFormatted, NO_VALVE, 0, 0, ""), "file system formatted"),
        (ev(C::FsFormatted, NO_VALVE, -1, 0, ""), "file system format failed"),
        (ev(C::EspOtaStarted, NO_VALVE, 1000, 0, ""), "ESP update started (1000 bytes)"),
        (ev(C::EspOtaDone, NO_VALVE, 1000, 0, "2.0.1"), "ESP update done (1000 bytes, 2.0.1)"),
        (ev(C::EspOtaDone, NO_VALVE, 1000, 0, ""), "ESP update done (1000 bytes)"),
        (ev(C::EspOtaFailed, NO_VALVE, 8, 0, ""), "ESP update failed (error 8)"),
        (ev(C::AppMarkedValid, NO_VALVE, 120, 0, ""), "firmware marked valid after 120 s"),
        (ev(C::RebootRequested, NO_VALVE, 0, 0, ""), "restart requested (user)"),
        (ev(C::RebootRequested, NO_VALVE, 1, 0, ""), "restart requested (ota)"),
        (ev(C::RebootRequested, NO_VALVE, 2, 0, ""), "restart requested (net watchdog)"),
        (ev(C::RebootRequested, NO_VALVE, 3, 0, ""), "restart requested (factory reset)"),
        (ev(C::RebootRequested, NO_VALVE, 4, 0, ""), "restart requested (rollback)"),
        (ev(C::RebootRequested, NO_VALVE, 5, 0, ""), "restart requested (network revert)"),
        (ev(C::RebootRequested, NO_VALVE, 6, 0, ""), "restart requested (heap guard)"),
        // intended deviation (PORT-NOTES): C++ 2.1.7 reads reason 7 as "unknown"
        (ev(C::RebootRequested, NO_VALVE, 7, 0, ""), "restart requested (switch back)"),
        (ev(C::RebootRequested, NO_VALVE, 8, 0, ""), "restart requested (unknown)"),
        (ev(C::RebootRequested, NO_VALVE, -1, 0, ""), "restart requested (unknown)"),
        (ev(C::RebootRequested, NO_VALVE, 2, 10, ""), "restart requested (net watchdog, after 10 min)"),
        (ev(C::RebootRequested, NO_VALVE, 2, 1, ""), "restart requested (net watchdog, after 1 min)"),
        (ev(C::RebootRequested, NO_VALVE, 2, -5, ""), "restart requested (net watchdog)"),
        (ev(C::RebootRequested, NO_VALVE, 0, 10, ""), "restart requested (user)"),
        (ev(C::RebootRequested, NO_VALVE, 4, 7, ""), "restart requested (rollback, missing net, http, stm)"),
        (ev(C::RebootRequested, NO_VALVE, 4, 1, ""), "restart requested (rollback, missing net)"),
        (ev(C::RebootRequested, NO_VALVE, 4, 2, ""), "restart requested (rollback, missing http)"),
        (ev(C::RebootRequested, NO_VALVE, 4, 4, ""), "restart requested (rollback, missing stm)"),
        (ev(C::RebootRequested, NO_VALVE, 4, 5, ""), "restart requested (rollback, missing net, stm)"),
        (ev(C::RebootRequested, NO_VALVE, 4, 8, ""), "restart requested (rollback)"),
        (ev(C::RebootRequested, NO_VALVE, 5, 7, ""), "restart requested (network revert)"),
        (ev(C::RebootRequested, NO_VALVE, 3, 7, ""), "restart requested (factory reset)"),
        (ev(C::LowHeap, NO_VALVE, 29000, 20000, ""), "low heap (free 29000, min 20000)"),
        (ev(C::TimeSynced, NO_VALVE, -3, 0, ""), "time synced (step -3 s)"),
        (ev(C::CalibTimeMissing, NO_VALVE, 20260923, 0, ""), "scheduled calibration skipped, no valid time (slot 20260923)"),
        (ev(C::FactoryResetSkipped, NO_VALVE, 0, 0, ""), "factory reset pin still set: settings kept, remove the jumper"),
        (ev(C::StackLow, NO_VALVE, 480, 6144, "stm"), "task stm: stack low (480 of 6144 bytes free)"),
        (ev(C::StackLow, NO_VALVE, 1, 2, ""), "task : stack low (1 of 2 bytes free)"),
        (ev(C::HeapFragmented, NO_VALVE, 7000, 90000, ""), "heap fragmented (largest block 7000, free 90000)"),
        (ev(C::LogWriteFailed, NO_VALVE, 1, 3, ""), "log file write failed (open, 3 events lost)"),
        (ev(C::LogWriteFailed, NO_VALVE, 2, 0, ""), "log file write failed (write, 0 events lost)"),
        (ev(C::LogWriteFailed, NO_VALVE, 3, 1, ""), "log file write failed (rotate, 1 events lost)"),
        (ev(C::LogWriteFailed, NO_VALVE, 4, 9, ""), "log file write failed (size limit, 9 events lost)"),
        (ev(C::LogWriteFailed, NO_VALVE, 0, 9, ""), "log file write failed (unknown, 9 events lost)"),
        (ev(C::LogWriteFailed, NO_VALVE, 5, 9, ""), "log file write failed (unknown, 9 events lost)"),
        (ev(C::ImportDropped, NO_VALVE, 3, 31, "ignored 14 keys"), "legacy import dropped pi, window, messenger, ds18Timeout, legacyFailsafe (3 PI valves, ignored 14 keys)"),
        (ev(C::ImportDropped, NO_VALVE, 0, 2, ""), "legacy import dropped window (0 PI valves)"),
        (ev(C::ImportDropped, NO_VALVE, 1, 1, "x"), "legacy import dropped pi (1 PI valves, x)"),
        (ev(C::ImportDropped, NO_VALVE, 0, 16, ""), "legacy import dropped legacyFailsafe (0 PI valves)"),
        (ev(C::ImportDropped, NO_VALVE, 0, 12, ""), "legacy import dropped messenger, ds18Timeout (0 PI valves)"),
        (ev(C::ImportDropped, NO_VALVE, 2, 0, ""), "legacy import dropped nothing (2 PI valves)"),
        (ev(C::ImportDropped, NO_VALVE, 2, 32, ""), "legacy import dropped nothing (2 PI valves)"),
        (ev(C::ConfigRestored, NO_VALVE, 0, 0, ""), "configuration restored from the backup (NVS empty)"),
        (ev(C::ConfigRestored, NO_VALVE, 3, 0, ""), "configuration restored from the backup (decode error 3)"),
        (ev(C::ConfigRestored, NO_VALVE, -1, 0, ""), "configuration restored from the backup (decode error -1)"),
        (ev(C::ConfigRepaired, NO_VALVE, 5, 2, "calib.hour"), "configuration repaired (2 fields, first calib.hour)"),
        (ev(C::ConfigRepaired, NO_VALVE, 5, 1, ""), "configuration repaired (1 fields)"),
        (ev(C::ConfigNewerSchema, NO_VALVE, 2, 3, ""), "configuration written by a newer firmware (schema 2, 3 unknown settings kept)"),
        (ev(C::FilesRemoved, NO_VALVE, 2, 96, "legacy images"), "removed 2 files (96 KiB): legacy images"),
        (ev(C::FilesRemoved, NO_VALVE, 1, 0, ""), "removed 1 files (0 KiB)"),
        (ev(C::HeapCritical, NO_VALVE, 11000, 4096, ""), "heap critical, restarting (free 11000, largest block 4096)"),
        (ev(C::NetUp, NO_VALVE, 1, 0, "192.168.1.5"), "network up (eth, 192.168.1.5)"),
        (ev(C::NetUp, NO_VALVE, 2, 0, ""), "network up (wifi)"),
        (ev(C::NetUp, NO_VALVE, 3, 0, ""), "network up (unknown)"),
        (ev(C::NetDown, NO_VALVE, 1, 0, ""), "network down (eth)"),
        (ev(C::NetDown, NO_VALVE, 0, 0, ""), "network down (unknown)"),
        (ev(C::MqttConnected, NO_VALVE, 0, 0, ""), "MQTT connected"),
        (ev(C::MqttDisconnected, NO_VALVE, -3, 0, ""), "MQTT disconnected (state -3)"),
        (ev(C::MqttCommandRejected, NO_VALVE, 3, 2, "payload"), "MQTT command rejected (valve 3, payload)"),
        (ev(C::MqttCommandRejected, NO_VALVE, 0, 0, ""), "MQTT command rejected (unknown valve)"),
        (ev(C::MqttCommandRejected, NO_VALVE, 1, 0, ""), "MQTT command rejected (valve 1)"),
        (ev(C::HaDiscoverySent, NO_VALVE, 80, 170, ""), "HA discovery sent (80 configs, 170 deletes)"),
        (ev(C::AuthFailed, NO_VALVE, 3, 0, "10.0.0.9"), "authentication failed (3 in window, 10.0.0.9)"),
        (ev(C::AuthFailed, NO_VALVE, 3, 0, ""), "authentication failed (3 in window)"),
        (ev(C::NetTrialStarted, NO_VALVE, 120, 0, "192.168.1.50"), "network settings on trial for 120 s (192.168.1.50)"),
        (ev(C::NetTrialStarted, NO_VALVE, 120, 0, ""), "network settings on trial for 120 s"),
        (ev(C::NetTrialConfirmed, NO_VALVE, 40, 0, ""), "network settings confirmed after 40 s"),
        (ev(C::NetTrialConfirmed, NO_VALVE, 40, 1, ""), "network settings confirmed after 40 s (by a newer change)"),
        (ev(C::NetTrialConfirmed, NO_VALVE, 40, 2, ""), "network settings confirmed after 40 s"),
        (ev(C::NetTrialReverted, NO_VALVE, 1, 0, "dhcp"), "network settings reverted (not confirmed, back to dhcp)"),
        (ev(C::NetTrialReverted, NO_VALVE, 2, 0, ""), "network settings reverted (no network)"),
        (ev(C::NetTrialReverted, NO_VALVE, 3, 0, "10.0.0.2"), "network settings reverted (interrupted, back to 10.0.0.2)"),
        (ev(C::NetTrialReverted, NO_VALVE, 4, 0, ""), "network settings reverted (user)"),
        (ev(C::NetTrialReverted, NO_VALVE, 5, 0, "dhcp"), "network settings reverted (trial not stored, back to dhcp)"),
        (ev(C::NetTrialReverted, NO_VALVE, 6, 0, ""), "network settings reverted (unknown)"),
        (ev(C::NetTrialReverted, NO_VALVE, 1, -1, "dhcp"), "network settings could not be reverted (not confirmed)"),
        (ev(C::NetTrialReverted, NO_VALVE, 1, -2, ""), "network settings reverted (not confirmed)"),
        (ev(C::NetTrialReverted, NO_VALVE, 1, 1, ""), "network settings reverted (not confirmed)"),
        (ev(C::NetTrialReverted, NO_VALVE, 4, 0, "x"), "network settings reverted (user, back to x)"),
        (ev(C::NetUnreachable, NO_VALVE, 150, 1, ""), "network unreachable (nothing for 150 s, last ping)"),
        (ev(C::NetUnreachable, NO_VALVE, 150, 0, ""), "network unreachable (nothing for 150 s, last none)"),
        (ev(C::NetUnreachable, NO_VALVE, 150, 5, ""), "network unreachable (nothing for 150 s, last dhcp)"),
        (ev(C::NetUnreachable, NO_VALVE, 150, 6, ""), "network unreachable (nothing for 150 s, last unknown)"),
        (ev(C::NetUnreachable, NO_VALVE, 150, 257, ""), "network unreachable (nothing for 150 s, last unknown)"),
        (ev(C::NetUnreachable, NO_VALVE, 150, -1, ""), "network unreachable (nothing for 150 s, last unknown)"),
        (ev(C::NetReachable, NO_VALVE, 300, 0, ""), "network reachable again after 300 s"),
        (ev(C::NetInterfaceRestart, NO_VALVE, 300, 1, ""), "network interface restarted (eth, after 300 s)"),
        (ev(C::NetInterfaceRestart, NO_VALVE, 300, 2, ""), "network interface restarted (wifi, after 300 s)"),
        (ev(C::NetInterfaceRestart, NO_VALVE, 300, 3, ""), "network interface restarted (eth+wifi, after 300 s)"),
        (ev(C::NetInterfaceRestart, NO_VALVE, 300, 4, ""), "network interface restarted (unknown, after 300 s)"),
        (ev(C::RequestRefused, NO_VALVE, 1, 0, "10.0.0.9"), "request refused (host) from 10.0.0.9"),
        (ev(C::RequestRefused, NO_VALVE, 2, 0, ""), "request refused (origin)"),
        (ev(C::RequestRefused, NO_VALVE, 3, 0, ""), "request refused (header)"),
        (ev(C::RequestRefused, NO_VALVE, 4, 0, ""), "request refused (content type)"),
        (ev(C::RequestRefused, NO_VALVE, 0, 0, ""), "request refused (unknown)"),
        (ev(C::AuthLocked, NO_VALVE, 60, 1, "10.0.0.2"), "login locked for 60 s (lockout 1) for 10.0.0.2"),
        (ev(C::AuthLocked, NO_VALVE, 900, 3, ""), "login locked for 900 s (lockout 3)"),
        (ev(C::LinkUp, NO_VALVE, 0, 0, ""), "STM link up"),
        (ev(C::LinkDegraded, NO_VALVE, 1, 0, ""), "STM link degraded (1 timeouts)"),
        (ev(C::LinkDown, NO_VALVE, 5, 0, ""), "STM link down (5 timeouts)"),
        (ev(C::StmResetByPolicy, NO_VALVE, 7, 65, ""), "STM reset by link policy (7 timeouts in 65 s)"),
        (ev(C::StmResetByUser, NO_VALVE, 0, 0, ""), "STM reset by user"),
        (ev(C::StmRebootDetected, NO_VALVE, 1, 0, ""), "STM reboot detected (uptime)"),
        (ev(C::StmRebootDetected, NO_VALVE, 2, 0, ""), "STM reboot detected (resets)"),
        (ev(C::StmRebootDetected, NO_VALVE, 3, 0, ""), "STM reboot detected (v1 heuristic)"),
        (ev(C::StmRebootDetected, NO_VALVE, 4, 0, ""), "STM reboot detected (link recovered)"),
        (ev(C::StmRebootDetected, NO_VALVE, 0, 0, ""), "STM reboot detected (unknown)"),
        (ev(C::StmRebootDetected, NO_VALVE, 5, 0, ""), "STM reboot detected (unknown)"),
        (ev(C::StmVersion, NO_VALVE, 2, 0x431, "2.0.0-revamped_C2"), "STM 2.0.0-revamped_C2 (protocol 2, hw 0x431)"),
        (ev(C::StmVersion, NO_VALVE, 1, 0x23, ""), "STM version unknown (protocol 1, hw 0x023)"),
        (ev(C::StmIncompatible, NO_VALVE, 0, 0, "1.3.0"), "STM version 1.3.0 is not supported"),
        (ev(C::StmIncompatible, NO_VALVE, 0, 0, ""), "STM version unknown is not supported"),
        (ev(C::StmRxOverflow, NO_VALVE, 12, 0, ""), "UART receive overflow (12 total, esp side)"),
        (ev(C::StmRxOverflow, NO_VALVE, 12, 1, ""), "UART receive overflow (12 total, stm side)"),
        (ev(C::StmParseErrors, NO_VALVE, 3, 1, ""), "UART parse errors (3 total, stm side)"),
        (ev(C::StmParseErrors, NO_VALVE, 3, 2, ""), "UART parse errors (3 total, unknown side)"),
        (ev(C::StmQueueFull, NO_VALVE, 1, 0, ""), "STM request queue full (command 1)"),
        (ev(C::StmFlashStarted, NO_VALVE, 65536, 0, "fw.bin"), "STM flash started (65536 bytes, fw.bin)"),
        (ev(C::StmFlashStarted, NO_VALVE, 65536, 0, ""), "STM flash started (65536 bytes)"),
        (ev(C::StmFlashDone, NO_VALVE, 21000, 0, "2.0.0"), "STM flash done in 21000 ms (2.0.0)"),
        (ev(C::StmFlashDone, NO_VALVE, 21000, 0, ""), "STM flash done in 21000 ms"),
        (ev(C::StmFlashFailed, NO_VALVE, 4, 0x08004000, "writing"), "STM flash failed (error 4 at 0x08004000, writing)"),
        (ev(C::StmFlashFailed, NO_VALVE, 4, -1, ""), "STM flash failed (error 4 at 0xffffffff)"),
        (ev(C::FailsafeActive, NO_VALVE, 0x0FFF, 1, ""), "failsafe active on 12 valves (STM lease)"),
        (ev(C::FailsafeActive, NO_VALVE, 0x0005, 2, ""), "failsafe active on 2 valves (ESP)"),
        (ev(C::FailsafeActive, NO_VALVE, 0, 3, ""), "failsafe active on 0 valves (unknown)"),
        (ev(C::FailsafeActive, NO_VALVE, -1, 0, ""), "failsafe active on 32 valves (unknown)"),
        (ev(C::FailsafeEnded, NO_VALVE, 3600, 1, ""), "failsafe ended after 3600 s (STM lease)"),
        (ev(C::FailsafeEnded, NO_VALVE, 60, 2, ""), "failsafe ended after 60 s (ESP)"),
        (ev(C::RegulatorLost, NO_VALVE, 1, 0, ""), "regulator lost (MQTT broker disconnected)"),
        (ev(C::RegulatorLost, NO_VALVE, 2, 0, ""), "regulator lost (Home Assistant offline)"),
        (ev(C::RegulatorLost, NO_VALVE, 0, 0, ""), "regulator lost (unknown)"),
        (ev(C::RegulatorLost, NO_VALVE, 3, 0, ""), "regulator lost (unknown)"),
        (ev(C::RegulatorBack, NO_VALVE, 125, 0, ""), "regulator back after 125 s"),
        (ev(C::LeaseConfigFailed, NO_VALVE, 1, 3, ""), "failsafe settings not accepted by the STM (no reply, 3 attempts)"),
        (ev(C::LeaseConfigFailed, NO_VALVE, 2, 1, ""), "failsafe settings not accepted by the STM (rejected, 1 attempts)"),
        (ev(C::LeaseConfigFailed, NO_VALVE, 3, 3, ""), "failsafe settings not accepted by the STM (read-back differs, 3 attempts)"),
        (ev(C::LeaseConfigFailed, NO_VALVE, 4, 3, ""), "failsafe settings not accepted by the STM (unknown, 3 attempts)"),
        (ev(C::StmSafeMode, NO_VALVE, 3, 0, ""), "STM in safe mode (3 watchdog resets)"),
        (ev(C::StmSafeModeEnded, NO_VALVE, 0, 0, ""), "STM left safe mode"),
        (ev(C::StmConfigRepaired, NO_VALVE, 5, 1, ""), "STM configuration repaired (flags 0x05, 1 repairs)"),
        (ev(C::StmConfigRepaired, NO_VALVE, 0xC0, 2, ""), "STM configuration repaired (flags 0xc0, 2 repairs)"),
        (ev(C::StmUartErrors, NO_VALVE, 7, 2, ""), "STM UART errors (7 total, 2 bytes dropped)"),
        (ev(C::StmEepromWaitTimeout, NO_VALVE, 10000, 1, ""), "STM EEPROM write still pending after 10000 ms (STM reset)"),
        (ev(C::StmEepromWaitTimeout, NO_VALVE, 10000, 2, ""), "STM EEPROM write still pending after 10000 ms (flash)"),
        (ev(C::StmEepromWaitTimeout, NO_VALVE, 12000, 3, "stm task silent"), "STM EEPROM write still pending after 12000 ms (ESP restart): stm task silent"),
        (ev(C::StmEepromWaitTimeout, NO_VALVE, 1, 0, ""), "STM EEPROM write still pending after 1 ms (unknown)"),
        (ev(C::TargetsRestored, NO_VALVE, 12, 1, ""), "desired targets restored for 12 valves (RTC)"),
        (ev(C::TargetsRestored, NO_VALVE, 3, 2, ""), "desired targets restored for 3 valves (NVS)"),
        (ev(C::TargetsRestored, NO_VALVE, 3, 3, ""), "desired targets restored for 3 valves (unknown)"),
        (ev(C::StmProtectionSuspended, NO_VALVE, 0, 0, ""), "STM short-circuit and inrush limits suspended until the next STM start"),
        (ev(C::TargetSet, 0, 55, 3, ""), "valve 1: target 55 % (mqtt)"),
        (ev(C::TargetSet, 11, 0, 2, ""), "valve 12: target 0 % (web)"),
        (ev(C::TargetSet, 1, 1, 9, ""), "valve 2: target 1 % (unknown)"),
        (ev(C::TargetSet, 1, 1, -1, ""), "valve 2: target 1 % (unknown)"),
        (ev(C::TargetSet, 1, 1, 0, ""), "valve 2: target 1 % (none)"),
        (ev(C::TargetSet, 1, 1, 257, ""), "valve 2: target 1 % (unknown)"),
        (ev(C::ValveStateChanged, 2, 1, 2, ""), "valve 3: state idle -> opening"),
        (ev(C::ValveStateChanged, 2, 300, -1, ""), "valve 3: state invalid -> invalid"),
        (ev(C::ValveStateChanged, 2, 256, 0, ""), "valve 3: state invalid -> nodata"),
        (ev(C::ValveStateChanged, 2, 9, 265, ""), "valve 3: state blocked -> invalid"),
        (ev(C::ValveBlocked, 2, 2, -1, ""), "valve 3: blocked (calibration retries 2)"),
        (ev(C::ValveBlocked, 2, 2, 50, ""), "valve 3: blocked (calibration retries 2, failsafe 50 %)"),
        (ev(C::ValveBlocked, 2, 2, 0, ""), "valve 3: blocked (calibration retries 2, failsafe 0 %)"),
        (ev(C::ValveFailed, 2, 0, -1, ""), "valve 3: failed"),
        (ev(C::ValveFailed, 2, 0, 0, ""), "valve 3: failed (none)"),
        (ev(C::ValveFailed, 2, 0, 3, ""), "valve 3: failed (short)"),
        (ev(C::ValveFailed, 2, 0, 5, ""), "valve 3: failed (inrush_trip)"),
        (ev(C::ValveFailed, 2, 0, 6, ""), "valve 3: failed (unknown)"),
        (ev(C::ValveFailed, 2, 0, 256, ""), "valve 3: failed (unknown)"),
        (ev(C::ValveNoValve, 2, 0, 0, ""), "valve 3: no valve detected"),
        (ev(C::ValveRecovered, 2, 9, 0, ""), "valve 3: recovered (was blocked)"),
        (ev(C::ValveRecovered, 2, 0, i32::from(HEALTH_STALE), ""), "valve 3: recovered (data again)"),
        (ev(C::ValveRecovered, 2, 0, i32::from(HEALTH_TARGET_UNCONFIRMED), ""), "valve 3: recovered (target confirmed)"),
        (ev(C::ValveRecovered, 2, 0, i32::from(HEALTH_STALE | HEALTH_TARGET_UNCONFIRMED), ""), "valve 3: recovered (data again, target confirmed)"),
        (ev(C::ValveRecovered, 2, 0, 0, ""), "valve 3: recovered ()"),
        (ev(C::CalibStarted, 2, 0, 0, ""), "valve 3: calibration started"),
        (ev(C::CalibStarted, ALL_VALVES, 1, 0, ""), "all valves: calibration started (scheduled)"),
        (ev(C::CalibStarted, ALL_VALVES, 2, 0, ""), "all valves: calibration started (automatic retry)"),
        (ev(C::CalibStarted, 2, 3, 0, ""), "valve 3: calibration started"),
        (ev(C::CalibOk, 2, 3120, 3350, ""), "valve 3: calibration ok (oc 3120, cc 3350)"),
        (ev(C::CalibRetry, 2, 1, 0, ""), "valve 3: calibration retry 1"),
        (ev(C::CalibFailed, 2, 2, -1, ""), "valve 3: calibration failed after 2 retries"),
        (ev(C::CalibFailed, 2, 2, 50, ""), "valve 3: calibration failed after 2 retries, failsafe 50 %"),
        (ev(C::CalibFailed, 2, 2, 0, ""), "valve 3: calibration failed after 2 retries, failsafe 0 %"),
        (ev(C::EarlyStop, 2, 2, 2, ""), "valve 3: early stop (total 2, endstop)"),
        (ev(C::EarlyStop, 2, 2, 3, ""), "valve 3: early stop (total 2, early_endstop)"),
        (ev(C::EarlyStop, 2, 2, -1, ""), "valve 3: early stop (total 2, unknown)"),
        (ev(C::EarlyStop, 2, 2, 256, ""), "valve 3: early stop (total 2, unknown)"),
        (ev(C::EarlyStop, 2, 2, 0, ""), "valve 3: early stop (total 2, none)"),
        (ev(C::EarlyStop, 2, 2, 7, ""), "valve 3: early stop (total 2, aborted)"),
        (ev(C::EarlyStop, 2, 2, 258, ""), "valve 3: early stop (total 2, unknown)"),
        (ev(C::CmdRejected, 2, 4, 0, ""), "valve 3: command rejected by the STM (total 4)"),
        (ev(C::TargetNotConfirmed, 2, 40, 5, ""), "valve 3: target 40 % not confirmed after 5 attempts"),
        (ev(C::ValveStale, 2, 60, 0, ""), "valve 3: no data for 60 s"),
        (ev(C::ServiceMoveDone, 2, 500, 1, ""), "valve 3: service move done (500 counts, target)"),
        (ev(C::CalibStrokeShort, 2, 3599, 3000, ""), "valve 3: calibration stroke 3599 close to the minimum 3000"),
        (ev(C::TempSensorFailed, NO_VALVE, 4, -1270, "28-84-37-94-97-ff-03-23"), "temp sensor 4 failed (raw -1270, 28-84-37-94-97-ff-03-23)"),
        (ev(C::TempSensorFailed, NO_VALVE, 4, 850, ""), "temp sensor 4 failed (raw 850)"),
        (ev(C::TempSensorRecovered, NO_VALVE, 4, 0, ""), "temp sensor 4 recovered"),
        (ev(C::SensorCountChanged, NO_VALVE, 3, 0, ""), "temp sensor count 3"),
        (ev(C::SensorCountChanged, NO_VALVE, 2, 1, ""), "volt sensor count 2"),
        (ev(C::VoltSensorFailed, NO_VALVE, 1, -1000, ""), "volt sensor 1 failed (raw -1000)"),
        (ev(C::ScheduledCalibration, NO_VALVE, 20260923, 3, ""), "scheduled calibration (slot 20260923, 3 min late)"),
        (ev(C::ScheduledCalibrationFailed, NO_VALVE, 20260923, 1, ""), "scheduled calibration not confirmed (slot 20260923, no reply)"),
        (ev(C::ScheduledCalibrationFailed, NO_VALVE, 20260923, 2, ""), "scheduled calibration not confirmed (slot 20260923, not sent)"),
        (ev(C::ScheduledCalibrationFailed, NO_VALVE, 20260923, 3, ""), "scheduled calibration not confirmed (slot 20260923, no result)"),
        (ev(C::ScheduledCalibrationFailed, NO_VALVE, 20260923, 4, ""), "scheduled calibration not confirmed (slot 20260923, STM unsupported)"),
        (ev(C::ScheduledCalibrationFailed, NO_VALVE, 20260923, 5, ""), "scheduled calibration not confirmed (slot 20260923, unknown)"),
        (ev(C::ScheduledCalibrationMissed, NO_VALVE, 20260923, 3, ""), "scheduled calibration missed (slot 20260923, 3 attempts)"),
        // C++ {ev(static_cast<EventCode>(999)), "event 999"}, no Rust form (EventCode cannot hold 999)
        (ev(C::NetUp, 12, 1, 0, ""), "network up (eth)"),  // valve 12 is not a valve
    ];
    for (e, msg) in &rows {
        assert_text(&message(e), msg);
    }
}

#[test]
fn event_message_truncation_and_bad_buffers() {
    let e = ev(C::CalibOk, 2, 3120, 3350, "");
    let mut buf = [0u8; 8];
    assert_eq!(format_event_message(&e, &mut buf), 7);
    assert_text(&buf[..7], "valve 3");
    let mut one = [b'X'; 1];
    assert_eq!(format_event_message(&e, &mut one), 0);
    let mut two = [b'X'; 2];
    assert_eq!(format_event_message(&e, &mut two), 1);
    assert_eq!(two, [b'v', b'X']); // the C++ NUL is not written
                                   // Numbers are cut at the buffer end as well.
    let mut nine = [b'X'; 10];
    let low = ev(C::LowHeap, NO_VALVE, 123_456, 7, "");
    assert_eq!(format_event_message(&low, &mut nine), 9);
    assert_text(&nine[..9], "low heap ");
    let mut cut = [0u8; 14];
    assert_eq!(format_event_message(&low, &mut cut), 13);
    assert_text(&cut[..13], "low heap (fre");
    // Extreme arguments are never truncated in a normal buffer.
    assert_text(
        &message(&ev(C::CalibTimeMissing, NO_VALVE, i32::MIN, 0, "")),
        "scheduled calibration skipped, no valid time (slot -2147483648)",
    );
    assert_text(
        &message(&ev(C::StmResetByPolicy, NO_VALVE, i32::MIN, i32::MAX, "")),
        "STM reset by link policy (-2147483648 timeouts in 2147483647 s)",
    );
    assert_text(
        &message(&ev(C::ScheduledCalibration, 11, i32::MIN, i32::MIN, "")),
        "valve 12: scheduled calibration (slot -2147483648, -2147483648 min late)",
    );
    // C++ formatEventMessage(e, nullptr, 10): no Rust form
    assert_eq!(format_event_message(&e, &mut buf[..0]), 0);
    // C++ "unterminated text is read bounded": a full text
    let mut t = ev(C::ConfigSaved, NO_VALVE, 1, 0, "");
    t.text = text_of(&[b'y'; EVENT_TEXT_MAX]);
    assert_text(
        &message(&t),
        "config saved (revision 1, yyyyyyyyyyyyyyyyyyyyyyy)",
    );
}

#[test]
fn event_text_ends_at_an_embedded_nul_like_the_cpp_char_array() {
    let mut t = ev(C::ConfigSaved, NO_VALVE, 1, 0, "");
    t.text = text_of(b"ab\0cd");
    assert_text(&message(&t), "config saved (revision 1, ab)");
    t.text = text_of(b"\0cd");
    assert_text(&message(&t), "config saved (revision 1)");
    let mut buf = [0u8; 320];
    let mut jw = JsonWriter::new(&mut buf);
    assert!(write_event_json(&mut jw, &t));
    assert!(contains_bytes(jw.as_bytes(), br#""text":"","msg""#));
}

#[test]
fn event_lines_with_utc_time_or_uptime() {
    let mut e = ev_sev(C::EarlyStop, 2, 2, 2, "", Warning);
    e.epoch = 1_790_172_185;
    e.seq = 7;
    assert_text(
        &line_of(&e, 160),
        "#7 2026-09-23T14:03:05Z WARNING early_stop v3 valve 3: early stop (total 2, endstop)",
    );
    let mut big = ev0(C::MqttConnected);
    big.seq = 4_294_967_295;
    assert_text(
        &line_of(&big, 160),
        "#4294967295 +0s INFO mqtt_connected MQTT connected",
    );
    e.seq = 0;
    let mut b = ev0(C::MqttConnected);
    b.uptime_s = 123;
    assert_text(
        &line_of(&b, 160),
        "#0 +123s INFO mqtt_connected MQTT connected",
    );

    let times: [(u32, &str); 6] = [
        (1, "1970-01-01T00:00:01Z"),
        (1_709_164_800, "2024-02-29T00:00:00Z"),
        (951_868_799, "2000-02-29T23:59:59Z"),
        (4_107_542_400, "2100-03-01T00:00:00Z"),
        (0x8000_0000, "2038-01-19T03:14:08Z"),
        (0xFFFF_FFFF, "2106-02-07T06:28:15Z"),
    ];
    for (epoch, prefix) in times {
        b.epoch = epoch;
        let want = std::format!("#0 {prefix} INFO mqtt_connected MQTT connected");
        assert_text(&line_of(&b, 160), want);
    }
    // C++ severities 7 and 5 print "UNKNOWN": a Severity cannot hold them
    let sevs = [(Debug, "DEBUG"), (Error, "ERROR"), (Critical, "CRITICAL")];
    b.epoch = 0;
    b.uptime_s = 0;
    for (sev, upper) in sevs {
        b.severity = sev;
        let want = std::format!("#0 +0s {upper} mqtt_connected MQTT connected");
        assert_text(&line_of(&b, 160), want);
    }
    let mut all = ev(C::CalibStarted, ALL_VALVES, 1, 0, "");
    all.seq = 12;
    assert_text(
        &line_of(&all, 160),
        "#12 +0s INFO calib_started all valves: calibration started (scheduled)",
    );

    // Truncation keeps the prefix.
    assert_eq!(format_event_line(&e, &mut [b'X'; 1]), 0);
    let mut three = [b'X'; 3];
    assert_eq!(format_event_line(&e, &mut three), 2);
    assert_eq!(three, *b"#0X");
    let mut small = [0u8; 15];
    assert_eq!(format_event_line(&e, &mut small), 14);
    assert_text(&small[..14], "#0 2026-09-23T");
    let mut mid = [0u8; 33];
    assert_eq!(format_event_line(&e, &mut mid), 32);
    assert_text(&mid[..32], "#0 2026-09-23T14:03:05Z WARNING ");
    let mut exact = [0u8; 46]; // prefix fills it up to the message
    assert_eq!(format_event_line(&e, &mut exact), 45);
    assert_text(
        &exact[..45],
        "#0 2026-09-23T14:03:05Z WARNING early_stop v3",
    );
    let mut exact2 = [0u8; 47];
    assert_eq!(format_event_line(&e, &mut exact2), 46);
    assert_text(
        &exact2[..46],
        "#0 2026-09-23T14:03:05Z WARNING early_stop v3 ",
    );
    let mut exact3 = [b'X'; 48];
    assert_eq!(format_event_line(&e, &mut exact3), 47);
    assert_text(
        &exact3[..48],
        "#0 2026-09-23T14:03:05Z WARNING early_stop v3 vX",
    );
    // C++ formatEventLine(e, nullptr, 10): no Rust form
    assert_eq!(format_event_line(&e, &mut small[..0]), 0);
}

#[test]
fn event_line_dates_match_an_independent_calendar_for_every_day_to_2106() {
    // C++ test suite "slow". Reference: walk the calendar day by day from 1970-01-01.
    const MONTH_DAYS: [u32; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let (mut y, mut m, mut d) = (1970u32, 1u32, 1u32);
    let mut e = ev0(C::MqttConnected);
    let mut line = [0u8; 96];
    for day in 0..=(u32::MAX / 86_400) {
        // A different second of the day each time, so all fields vary.
        let sod = day.wrapping_mul(7919) % 86_400;
        e.epoch = day.wrapping_mul(86_400).wrapping_add(sod);
        if e.epoch == 0 {
            e.epoch = 1;
        }
        let n = format_event_line(&e, &mut line);
        let expect = std::format!(
            "#0 {y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
            e.epoch % 86_400 / 3600,
            e.epoch % 3600 / 60,
            e.epoch % 60
        );
        assert_eq!(
            &line[..23.min(n)],
            expect.as_bytes(),
            "day {day}: {} != {expect}",
            line[..n].escape_ascii()
        );
        let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
        let len = if m == 2 && leap {
            29
        } else {
            MONTH_DAYS[m as usize - 1]
        };
        d += 1;
        if d > len {
            d = 1;
            m += 1;
            if m > 12 {
                m = 1;
                y += 1;
            }
        }
    }
    assert_eq!(y, 2106);
    // Every second of one day.
    for sod in 0..86_400u32 {
        e.epoch = 1_790_121_600 + sod; // 2026-09-23
        let n = format_event_line(&e, &mut line);
        let expect = std::format!(
            "#0 2026-09-23T{:02}:{:02}:{:02}Z",
            sod / 3600,
            sod / 60 % 60,
            sod % 60
        );
        assert_eq!(&line[..23.min(n)], expect.as_bytes(), "{sod}");
    }
}

#[test]
fn event_json() {
    let mut e = ev_sev(C::EarlyStop, 2, 2, 2, "a\"b", Warning);
    e.seq = 5;
    e.epoch = 1_790_172_185;
    e.uptime_s = 42;
    let mut buf = [0u8; 320];
    let mut jw = JsonWriter::new(&mut buf);
    assert!(write_event_json(&mut jw, &e));
    assert_text(
        jw.as_bytes(),
        concat!(
            r#"{"seq":5,"t":1790172185,"up":42,"sev":"warning","code":410,"#,
            r#""name":"early_stop","valve":3,"a1":2,"a2":2,"text":"a\"b","#,
            r#""msg":"valve 3: early stop (total 2, endstop)"}"#
        ),
    );

    let mut s = ev_sev(C::LinkDown, NO_VALVE, -5, 0, "", Error);
    s.seq = 1;
    jw.reset();
    assert!(write_event_json(&mut jw, &s));
    assert_text(
        jw.as_bytes(),
        concat!(
            r#"{"seq":1,"t":null,"up":0,"sev":"error","code":302,"name":"link_down","#,
            r#""valve":null,"a1":-5,"a2":0,"text":"","msg":"STM link down (-5 timeouts)"}"#
        ),
    );

    let mut twelve = s.clone();
    twelve.valve = VALVE_COUNT; // not a valve index
    jw.reset();
    assert!(write_event_json(&mut jw, &twelve));
    assert!(contains_bytes(jw.as_bytes(), br#""valve":null"#));
    let mut last = s.clone();
    last.valve = VALVE_COUNT - 1;
    jw.reset();
    assert!(write_event_json(&mut jw, &last));
    assert!(contains_bytes(jw.as_bytes(), br#""valve":12,"#));

    let mut u = s.clone();
    u.text = text_of(&[b'z'; EVENT_TEXT_MAX]); // C++: unterminated
    u.valve = ALL_VALVES;
    jw.reset();
    assert!(write_event_json(&mut jw, &u));
    assert!(contains_bytes(jw.as_bytes(), br#""valve":null"#));
    assert!(contains_bytes(
        jw.as_bytes(),
        br#""text":"zzzzzzzzzzzzzzzzzzzzzzz""#
    ));

    let mut tiny = [0u8; 40];
    let mut small = JsonWriter::new(&mut tiny);
    assert!(!write_event_json(&mut small, &e));
}

#[test]
fn mqtt_event_json_single_and_aggregate() {
    let mut e = ev_sev(C::ValveStale, 2, 60, 0, "", Warning);
    e.seq = 7;
    e.uptime_s = 12;
    let mut buf = [0u8; 512];
    let mut jw = JsonWriter::new(&mut buf);
    assert!(write_mqtt_event_json(&mut jw, &e, 0));
    assert_text(
        jw.as_bytes(),
        concat!(
            r#"{"seq":7,"t":null,"up":12,"sev":"warning","code":413,"name":"valve_stale","#,
            r#""event_type":"valve_stale","valve":3,"a1":60,"a2":0,"text":"","#,
            r#""msg":"valve 3: no data for 60 s"}"#
        ),
    );
    // One valve bit is not an aggregate: the event's own valve counts.
    jw.reset();
    assert!(write_mqtt_event_json(&mut jw, &e, 1 << 7));
    assert!(contains_bytes(jw.as_bytes(), br#""valve":3,"a1""#));
    assert!(!contains_bytes(jw.as_bytes(), b"valves"));

    jw.reset();
    assert!(write_mqtt_event_json(
        &mut jw,
        &e,
        (1 << 0) | (1 << 1) | (1 << 11)
    ));
    assert_text(
        jw.as_bytes(),
        concat!(
            r#"{"seq":7,"t":null,"up":12,"sev":"warning","code":413,"name":"valve_stale","#,
            r#""event_type":"valve_stale","valve":null,"valves":[1,2,12],"a1":60,"a2":0,"#,
            r#""text":"","msg":"valves 1, 2, 12: no data for 60 s"}"#
        ),
    );
    // Bits above valve 12 are ignored: bit 0 + bit 12 is a single valve.
    jw.reset();
    assert!(write_mqtt_event_json(&mut jw, &e, 0x1001));
    assert!(contains_bytes(jw.as_bytes(), br#""valve":3,"a1""#));
    jw.reset();
    assert!(write_mqtt_event_json(&mut jw, &e, 0xFFFF));
    assert!(contains_bytes(
        jw.as_bytes(),
        br#""valves":[1,2,3,4,5,6,7,8,9,10,11,12]"#
    ));
    assert!(contains_bytes(
        jw.as_bytes(),
        br#""msg":"valves 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12: no data for 60 s""#
    ));
    // The plain event JSON has no event_type.
    jw.reset();
    assert!(write_event_json(&mut jw, &e));
    assert!(!contains_bytes(jw.as_bytes(), b"event_type"));
    // A system event keeps "valve":null.
    let s = ev_sev(C::FailsafeActive, NO_VALVE, 3, 1, "", Warning);
    jw.reset();
    assert!(write_mqtt_event_json(&mut jw, &s, 0));
    assert!(contains_bytes(
        jw.as_bytes(),
        br#""event_type":"failsafe_active","valve":null,"a1":3"#
    ));
    let mut tiny = [0u8; 60];
    let mut small = JsonWriter::new(&mut tiny);
    assert!(!write_mqtt_event_json(&mut small, &e, 3));
}

#[test]
fn format_event_message_multi_cases() {
    let e = ev(C::ValveStale, 4, 60, 0, "");
    let mut buf = [0u8; 160];
    let multi = |mask: u16| {
        let mut buf = [0u8; 160];
        let n = format_event_message_multi(&e, mask, &mut buf);
        buf[..n].to_vec()
    };
    assert_eq!(
        format_event_message_multi(&e, 0, &mut buf),
        "valve 5: no data for 60 s".len()
    );
    assert_text(&buf[..25], "valve 5: no data for 60 s");
    assert_text(&multi(1 << 3), "valve 5: no data for 60 s");
    assert_text(
        &multi((1 << 3) | (1 << 9)),
        "valves 4, 10: no data for 60 s",
    );
    assert_text(&multi(0x0003), "valves 1, 2: no data for 60 s");
    assert_text(&multi(0xF000), "valve 5: no data for 60 s");
    let all = ev(C::CalibStarted, ALL_VALVES, 0, 0, "");
    let n = format_event_message_multi(&all, 0, &mut buf);
    assert_text(&buf[..n], "all valves: calibration started");
    let mut cut = [0u8; 10];
    assert_eq!(format_event_message_multi(&e, 0x0003, &mut cut), 9);
    assert_text(&cut[..9], "valves 1,");
    // C++ formatEventMessageMulti(e, 3, nullptr, 10): no Rust form
    assert_eq!(format_event_message_multi(&e, 0x0003, &mut cut[..0]), 0);
    assert_eq!(VALVE_MASK_ALL, 0x0FFF);
}

#[test]
fn format_utc_timestamp_cases() {
    let mut buf = [0u8; 40];
    let mut ts = |epoch: u32| {
        let n = format_utc_timestamp(epoch, &mut buf);
        assert_eq!(n, 25);
        buf[..n].to_vec()
    };
    assert_text(&ts(0), "1970-01-01T00:00:00+00:00");
    assert_text(&ts(1_790_072_393), "2026-09-22T10:19:53+00:00");
    assert_text(&ts(4_102_444_799), "2099-12-31T23:59:59+00:00");
    assert_text(&ts(1_709_164_800), "2024-02-29T00:00:00+00:00");
    let mut exact = [0u8; 26];
    assert_eq!(format_utc_timestamp(1_790_072_393, &mut exact), 25);
    assert_text(&exact[..25], "2026-09-22T10:19:53+00:00");
    let mut short = [b'x'; 25];
    assert_eq!(format_utc_timestamp(1_790_072_393, &mut short), 0);
    assert_eq!(short, [b'x'; 25]); // nothing written (C++: "")
    let mut one = [b'x'; 1];
    assert_eq!(format_utc_timestamp(0, &mut one[..0]), 0);
    assert_eq!(one[0], b'x');
    // C++ formatUtcTimestamp(0, nullptr, 40): no Rust form
}

#[test]
fn parse_severity_fuzz_random_bytes_only_ever_match_a_case_folded_name() {
    // C++ test suite "fuzz". Fixed seed: reproducible. Candidates are biased towards the real
    // names so that near misses (one flipped byte, wrong length) are exercised as well.
    let names = ["debug", "info", "warning", "error", "critical"];
    let mut lcg = Lcg::numerical_recipes(0x5EED_0001);
    let mut rnd = move || lcg.next_state() >> 8;
    let mut buf = [0u8; 12];
    let mut matches = 0;
    for _ in 0..50_000 {
        let mut len;
        if rnd() % 2 == 0 {
            let n = names[(rnd() % 5) as usize];
            len = n.len();
            buf[..len].copy_from_slice(n.as_bytes());
            for c in &mut buf[..len] {
                let r = rnd() % 8;
                if r == 0 {
                    *c = (rnd() & 0xFF) as u8;
                } else if r == 1 && c.is_ascii_lowercase() {
                    *c -= 32;
                }
            }
            if rnd() % 8 == 0 {
                len = (rnd() % (buf.len() as u32 + 1)) as usize;
            }
        } else {
            len = (rnd() % (buf.len() as u32 + 1)) as usize;
            for c in &mut buf[..len] {
                *c = (rnd() & 0xFF) as u8;
            }
        }
        // Reference: ASCII-only case-insensitive compare of exactly len bytes.
        let mut expect = None;
        for (i, name) in names.iter().enumerate() {
            if name.len() == len
                && buf[..len]
                    .iter()
                    .zip(name.bytes())
                    .all(|(&c, n)| c.to_ascii_lowercase() == n)
            {
                expect = Some(i);
            }
        }
        let got = parse_severity(&buf[..len]);
        assert_eq!(
            got.map(|s| s as usize),
            expect,
            "{}",
            buf[..len].escape_ascii()
        );
        if got.is_some() {
            matches += 1;
        }
    }
    assert!(matches > 250, "{matches}"); // the bias really produced hits
}

#[test]
fn event_message_a_one_character_text_counts_as_text() {
    let e = make_event(C::Boot, Info, NO_VALVE, 1, 3, b"x");
    assert_text(&message(&e), "boot (reset poweron, count 3, fw x)");
    let e = make_event(C::Boot, Info, NO_VALVE, 1, 3, b"");
    assert_text(&message(&e), "boot (reset poweron, count 3)");
    let e = make_event(C::ConfigSaved, Info, NO_VALVE, 7, 0, b"y");
    assert_text(&message(&e), "config saved (revision 7, y)");
}

#[test]
fn event_line_of_a_valve_index_past_the_valves_has_no_valve_number() {
    // valve 12 is not a valve: no " v13" (the message has no "valve 13: " either)
    let mut e = ev(C::NetUp, VALVE_COUNT, 1, 0, "");
    assert_text(&line_of(&e, 160), "#0 +0s INFO net_up network up (eth)");
    e.valve = VALVE_COUNT - 1;
    assert_text(
        &line_of(&e, 160),
        "#0 +0s INFO net_up v12 valve 12: network up (eth)",
    );
    e.valve = NO_VALVE;
    assert_text(&line_of(&e, 160), "#0 +0s INFO net_up network up (eth)");
}

#[test]
fn switch_back_restart_and_trial_failure_are_the_glue_design_contract() {
    // Intended deviation (PORT-NOTES, GLUE-DESIGN-ESP §7 item 6): reboot reason 7 is the switch
    // back, logged with the default severity of reboot_requested (Info); 8 and above stay
    // "unknown" like every value outside the names.
    let mut e = make_event(
        C::RebootRequested,
        event_default_severity(C::RebootRequested),
        NO_VALVE,
        RebootReason::SwitchBack as i32,
        0,
        b"",
    );
    assert_eq!(e.severity, Info);
    assert_text(
        &line_of(&e, 160),
        "#0 +0s INFO reboot_requested restart requested (switch back)",
    );
    // no detail: the arg2 suffixes belong to reasons 2 and 4
    e.arg2 = 7;
    assert_text(&message(&e), "restart requested (switch back)");
    e.arg2 = 0;
    for v in [8, 9, 255, 256, i32::MAX, -7, i32::MIN] {
        e.arg1 = v;
        assert_text(&message(&e), "restart requested (unknown)");
    }
    // Event 107 arg1 -4: the previous image failed its trial (arg2 1 boot limit, 2 health); the
    // generic message, like -3 (no other valid image)
    for (a1, a2) in [(-4, 1), (-4, 2), (-3, 0)] {
        let f = ev_sev(C::EspOtaFailed, NO_VALVE, a1, a2, "", Error);
        let want = std::format!("ESP update failed (error {a1})");
        assert_text(&message(&f), &want);
        let mut buf = [0u8; 320];
        let mut jw = JsonWriter::new(&mut buf);
        assert!(write_event_json(&mut jw, &f));
        let tail = std::format!(r#""a1":{a1},"a2":{a2},"text":"","msg":"{want}"}}"#);
        assert!(jw.as_bytes().ends_with(tail.as_bytes()));
    }
    assert_eq!(event_default_severity(C::EspOtaFailed), Error);
}
