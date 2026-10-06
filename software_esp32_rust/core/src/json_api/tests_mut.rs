//! Port of test/native/test_json_api__mut.cpp: the longest STM version that is still written,
//! the sensor list of a valve stops at its two slots.

use super::*;
use std::string::String;
use std::vec;

fn status_with_version(v: &Version) -> String {
    let s = StatusSnapshot {
        stm_version: v.clone(),
        ..StatusSnapshot::default()
    };
    let mut buf = vec![0u8; 16384];
    let mut jw = JsonWriter::new(&mut buf);
    assert!(write_status_json(&mut jw, &s));
    String::from_utf8(jw.as_bytes().to_vec()).unwrap()
}

#[test]
fn status_an_stm_version_of_39_chars_is_written_one_of_40_chars_is_null() {
    let mut v = Version {
        valid: true,
        major: 65535,
        minor: 65535,
        patch: 65535, // "65535.65535.65535": 17 chars
        suffix: Text::from_slice(b"-abcdefghijklmnopqrstu").unwrap(), // 22 chars: 39 in total
        hw: Text::new(),
    };
    let mut j = status_with_version(&v);
    assert!(j.contains("\"version\":\"65535.65535.65535-abcdefghijklmnopqrstu\""));
    v.suffix = Text::from_slice(b"-abcdefghijklmnopqrstuv").unwrap(); // 40 chars
    j = status_with_version(&v);
    assert!(!j.contains("abcdefghijklmnopqrstuv"));
    assert!(j.contains("\"proto\":null,\"version\":null"));
}

#[test]
fn valves_only_the_two_sensor_slots_are_listed_whatever_follows_them_in_memory() {
    // C++ fills the memory after sensorSlot[] with 0x5A, so a third slot would look assigned:
    // no Rust form (the view has exactly two slots); the document of the two remains.
    let st = ValveState::default();
    let v = ValveView {
        state: Some(&st),
        sensor_slot: [1, 2],
        ..ValveView::default()
    };
    let mut buf = vec![0u8; 16384];
    let mut jw = JsonWriter::new(&mut buf);
    assert!(write_valves_json(&mut jw, &[v], 0));
    let j = String::from_utf8(jw.as_bytes().to_vec()).unwrap();
    assert!(j.contains(concat!(
        r#""sensors":[{"sensor":1,"slot":1,"name":"","temp":null},"#,
        r#"{"sensor":2,"slot":2,"name":"","temp":null}]"#
    )));
}
