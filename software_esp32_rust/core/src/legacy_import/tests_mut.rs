//! Port of test/native/test_legacy_import__mut.cpp: a station name starting with a control
//! character, the report of the first item and an empty root topic.

use super::tests::{first, import, FakeNvs};
use super::*;
use crate::test_support::assert_text;

#[test]
fn a_station_name_starting_with_a_control_character_is_rejected_not_taken_as_empty() {
    let mut n = FakeNvs::default();
    n.put_str("sysCfg", "stName", b"\x01VdMot");
    let mut c = Config::default();
    let r = import(&mut n, &mut c);
    assert_eq!(r.rejected, 1);
    assert_eq!(r.imported, 0);
    assert_eq!(first(&r), "sysCfg/stName");
    let mut d = Config::default();
    set_defaults(&mut d);
    assert_eq!(c.mqtt.root_topic, d.mqtt.root_topic);
    assert_eq!(c.station, d.station);
}

#[test]
fn write_import_report_json_the_first_valve_in_renamed_an_empty_root_topic_is_null() {
    let r = ImportReport {
        renamed_valves: 1 << 0,
        ..ImportReport::default()
    };
    let mut c = Config::default();
    set_defaults(&mut c);
    c.mqtt.root_topic.clear();
    copy_string(&mut c.valves[0].name, b"a_b");
    copy_string(&mut c.valves[0].topic, b"a/b");
    let mut buf = [0u8; 1024];
    let mut jw = JsonWriter::new(&mut buf);
    assert!(write_import_report_json(&mut jw, &r, &c));
    let j = jw.as_bytes();
    let has = |needle: &[u8]| j.windows(needle.len()).any(|w| w == needle);
    assert!(has(b"\"rootTopic\":null"));
    assert!(has(
        b"\"renamed\":[{\"kind\":\"valve\",\"n\":1,\"name\":\"a_b\",\"topic\":\"a/b\"}]"
    ));
    assert_text(&c.valves[0].topic, "a/b");
}
