//! Port of test/native/test_legacy_http__mut.cpp: edge cases of the /valves document (a name
//! that fills its whole config buffer).

use super::*;
use crate::common::ITEM_NAME_MAX;
use crate::config::ValveConfig;
use crate::valve_model::ValveState;
use std::string::String;

#[test]
fn legacy_valves_a_name_without_terminator_is_cut_at_item_name_max_characters() {
    // C++ fills all 11 bytes of the name with 'N' (no NUL): Rust a full Text<10>
    let st = ValveState {
        status: 1,
        ..ValveState::default()
    };
    let mut cfg = ValveConfig::default();
    cfg.name.resize(ITEM_NAME_MAX, b'N').unwrap();
    let v = ValveView {
        state: Some(&st),
        config: Some(&cfg),
        ..ValveView::default()
    };
    let mut buf = [0u8; 1024];
    let mut jw = JsonWriter::new(&mut buf);
    assert!(write_legacy_valves_json(&mut jw, &[v]));
    let j = String::from_utf8(jw.as_bytes().to_vec()).unwrap();
    assert!(j.contains(&(String::from("\"name\":\"") + &"N".repeat(ITEM_NAME_MAX) + "\",")));
}
