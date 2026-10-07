//! `test_web_server__mut_b1.cpp`: the STM action handlers (service move, valve sensors, profile
//! refresh, motor settings, reset), image removal, flash start and abort, factory reset and MQTT
//! discovery, each with its boundaries and its error answers. The images are real files of
//! storage, scanned by its service pass.
#![allow(clippy::large_stack_frames, clippy::large_stack_arrays)]

use vdm_esp_core::common::{OneWireId, NO_VALVE};
use vdm_esp_core::event_log::EventCode;
use vdm_esp_core::stm_codec::MoveDir;
use vdm_esp_core::stm_types::StmCommandType;

use super::rig::*;
use crate::port::Clock;
use crate::storage::{KEY_FACTORY_LATCH, KEY_IMPORTED, NAMESPACE};

const MOVE_URL: &str = "/api/valves/2/service-move";
const SENSORS_URL: &str = "/api/valves/3/sensors";
const MOTOR_URL: &str = "/api/stm/motor";

fn mv(dir: &str, counts: &str, max_ma: &str) -> String {
    format!("{{\"dir\":{dir},\"counts\":{counts},\"maxmA\":{max_ma}}}")
}

fn motor_body(low: i32, high: i32, sop: i32, min_cnt: i32, reps: i32) -> String {
    format!(
        "{{\"motor\":{{\"lowC\":{low},\"highC\":{high},\"startOnPower\":{sop},\
         \"noOfMinCount\":{min_cnt},\"maxCalReps\":{reps}}}}}"
    )
}

fn breakaway_body(enable: &str, step: i32, max_ma: i32) -> String {
    format!("{{\"breakaway\":{{\"enable\":{enable},\"stepPct\":{step},\"maxmA\":{max_ma}}}}}")
}

/// A scanned image without a board tag (check none).
fn good() -> Vec<u8> {
    stm_image(true, "1.4.9_Dev", "")
}

// ---------------------------------------------------------------- service move

#[test]
fn service_move_protocol_2_is_required_the_command_carries_dir_counts_and_max_ma() {
    let rig = Rig::started();
    rig.state().proto = 1;
    let st = rig.storage();
    let mut web = rig.web(&st);
    let r = perform(&mut web, api_post(MOVE_URL, &mv("\"open\"", "10", "20")));
    assert_eq!(r.status, 409);
    assert_eq!(
        text(&r),
        error_body("unsupported", "STM protocol v2 required")
    );
    assert!(rig.submitted().is_empty());
    rig.state().proto = 2;
    let r = perform(&mut web, api_post(MOVE_URL, &mv("\"open\"", "1", "5")));
    assert_eq!(r.status, 202);
    assert_eq!(text(&r), "{\"result\":\"queued\"}");
    let r = perform(
        &mut web,
        api_post(MOVE_URL, &mv("\"close\"", "10000", "60")),
    );
    assert_eq!(r.status, 202);
    let s = rig.submitted();
    assert_eq!(s.len(), 2);
    assert_eq!(s[0].kind, StmCommandType::ServiceMove);
    assert_eq!(s[0].valve, 1);
    assert_eq!(s[0].dir, MoveDir::Open);
    assert_eq!(s[0].counts, 1);
    assert_eq!(s[0].max_ma, 5);
    assert_eq!(s[1].dir, MoveDir::Close);
    assert_eq!(s[1].counts, 10000);
    assert_eq!(s[1].max_ma, 60);
    rig.state().proto = 3;
    assert_eq!(
        perform(&mut web, api_post(MOVE_URL, &mv("\"open\"", "10", "20"))).status,
        202
    );
}

#[test]
fn service_move_every_member_is_required_and_checked() {
    let rig = Rig::started();
    rig.state().proto = 2;
    let st = rig.storage();
    let mut web = rig.web(&st);
    let detail = "dir open|close, counts 1..10000, maxmA 5..60";
    for bad in [
        mv("\"up\"", "10", "20"),
        mv("5", "10", "20"),
        mv("\"open\"", "0", "20"),
        mv("\"open\"", "10001", "20"),
        mv("\"open\"", "10", "4"),
        mv("\"open\"", "10", "61"),
        mv("\"open\"", "true", "20"),
        "{\"counts\":10,\"maxmA\":20}".to_string(),
        "{\"dir\":\"open\",\"maxmA\":20}".to_string(),
        "{\"dir\":\"open\",\"counts\":10}".to_string(),
        "{\"dir\":\"open\",\"counts\":10,\"maxmA\":20,\"x\":1}".to_string(),
    ] {
        let r = perform(&mut web, api_post(MOVE_URL, &bad));
        assert_eq!(r.status, 400, "{bad}");
        assert_eq!(text(&r), error_body("out_of_range", detail));
    }
    assert!(rig.submitted().is_empty());
    rig.state().submit_result = false;
    let r = perform(&mut web, api_post(MOVE_URL, &mv("\"open\"", "10", "20")));
    assert_eq!(r.status, 503);
    assert_eq!(text(&r), error_body("queue_full", "STM command queue full"));
    rig.state().submit_result = true;
    let r = perform(&mut web, api_post(MOVE_URL, ""));
    assert_eq!(text(&r), error_body("bad_request", "JSON body required"));
}

// ---------------------------------------------------------------- valve sensors

#[test]
fn valve_sensors_the_slots_name_the_configured_ids() {
    let rig = Rig::started();
    rig.config(|c| {
        c.temps[0].id = valid_id(0x28, 10);
        c.temps[1].id = valid_id(0x28, 20);
        c.temps[33].id = valid_id(0x28, 30);
    });
    let c = rig.active();
    let st = rig.storage();
    let mut web = rig.web(&st);
    let r = perform(&mut web, api_post(SENSORS_URL, "{\"slot1\":1,\"slot2\":2}"));
    assert_eq!(r.status, 202);
    assert_eq!(text(&r), "{\"result\":\"queued\"}");
    for body in [
        "{\"slot1\":0,\"slot2\":34}",
        "{\"slot1\":34,\"slot2\":0}",
        "{\"slot1\":0,\"slot2\":0}",
    ] {
        assert_eq!(perform(&mut web, api_post(SENSORS_URL, body)).status, 202);
    }
    let s = rig.submitted();
    assert_eq!(s.len(), 4);
    let zero = OneWireId::default();
    assert_eq!(s[0].kind, StmCommandType::SetValveSensors);
    assert_eq!(s[0].valve, 2);
    assert_eq!(s[0].ids, [c.temps[0].id, c.temps[1].id]);
    assert_eq!(s[1].ids, [zero, c.temps[33].id]);
    assert_eq!(s[2].ids, [c.temps[33].id, zero]);
    assert_eq!(s[3].ids, [zero, zero]);
}

#[test]
fn valve_sensors_ranges_distinct_slots_and_valid_ids() {
    let rig = Rig::started();
    rig.config(|c| {
        c.temps[0].id = valid_id(0x28, 10);
        c.temps[2].id = valid_id(0x28, 20);
        c.temps[3].id = valid_id(0x28, 40);
        c.temps[4].id = valid_id(0x28, 50);
        c.temps[4].id.b[7] ^= 0x01; // CRC broken
    });
    let c = rig.active();
    let st = rig.storage();
    let mut web = rig.web(&st);
    let detail = "slot1/slot2 0..34, distinct";
    for bad in [
        "{\"slot1\":35,\"slot2\":0}",
        "{\"slot1\":0,\"slot2\":35}",
        "{\"slot1\":-1,\"slot2\":0}",
        "{\"slot1\":0,\"slot2\":-1}",
        "{\"slot2\":0}",
        "{\"slot1\":0}",
        "{\"slot1\":1,\"slot2\":1}",
        "{\"slot1\":3,\"slot2\":3}",
        "{\"slot1\":0,\"slot2\":0,\"x\":1}",
        "{\"slot1\":true,\"slot2\":0}",
    ] {
        let r = perform(&mut web, api_post(SENSORS_URL, bad));
        assert_eq!(r.status, 400, "{bad}");
        assert_eq!(text(&r), error_body("out_of_range", detail));
    }
    let r = perform(&mut web, api_post(SENSORS_URL, "{\"slot1\":2,\"slot2\":0}")); // no id
    assert_eq!(r.status, 400);
    assert_eq!(
        text(&r),
        error_body("invalid", "slot1 has no valid sensor id")
    );
    let r = perform(&mut web, api_post(SENSORS_URL, "{\"slot1\":1,\"slot2\":5}")); // bad CRC
    assert_eq!(r.status, 400);
    assert_eq!(
        text(&r),
        error_body("invalid", "slot2 has no valid sensor id")
    );
    let r = perform(&mut web, api_post(SENSORS_URL, "{\"slot1\":5,\"slot2\":0}"));
    assert_eq!(
        text(&r),
        error_body("invalid", "slot1 has no valid sensor id")
    );
    assert!(rig.submitted().is_empty());
    let r = perform(&mut web, api_post(SENSORS_URL, "{\"slot1\":3,\"slot2\":4}"));
    assert_eq!(r.status, 202);
    let s = rig.submitted();
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].ids, [c.temps[2].id, c.temps[3].id]);
    rig.state().submit_result = false;
    let r = perform(&mut web, api_post(SENSORS_URL, "{\"slot1\":0,\"slot2\":0}"));
    assert_eq!(r.status, 503);
}

// ---------------------------------------------------------------- profile refresh

#[test]
fn profile_refresh_protocol_2_is_required() {
    let rig = Rig::started();
    rig.state().proto = 1;
    let st = rig.storage();
    let mut web = rig.web(&st);
    let r = perform(&mut web, api_post("/api/valves/4/profile", ""));
    assert_eq!(r.status, 409);
    assert_eq!(
        text(&r),
        error_body("unsupported", "STM protocol v2 required")
    );
    assert!(rig.submitted().is_empty());
    rig.state().proto = 2;
    let r = perform(&mut web, api_post("/api/valves/4/profile", ""));
    assert_eq!(r.status, 202);
    let s = rig.submitted();
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].kind, StmCommandType::RequestProfile);
    assert_eq!(s[0].valve, 3);
}

// ---------------------------------------------------------------- motor settings

#[test]
fn motor_all_five_motor_values_are_sent_as_one_command() {
    let rig = Rig::started();
    let st = rig.storage();
    let mut web = rig.web(&st);
    let r = perform(&mut web, api_post(MOTOR_URL, &motor_body(10, 40, 0, 0, 0)));
    assert_eq!(r.status, 202);
    assert_eq!(text(&r), "{\"result\":\"queued\"}");
    let s = rig.submitted();
    assert_eq!(s.len(), 1);
    let c = &s[0];
    assert_eq!(c.kind, StmCommandType::SetMotorSettings);
    assert!(c.has_motor);
    assert!(!c.has_learn_movements);
    assert!(!c.has_breakaway);
    let m = c.motor;
    assert_eq!(
        (
            m.low_factor,
            m.high_factor,
            m.start_on_power,
            m.min_counts,
            m.max_calib_retries
        ),
        (10, 40, 0, 0, 0)
    );
    assert_eq!(m.field_count, 5);
    let r = perform(
        &mut web,
        api_post(MOTOR_URL, &motor_body(40, 10, 100, 60000, 2)),
    );
    assert_eq!(r.status, 202);
    let m = rig.submitted()[1].motor;
    assert_eq!(
        (
            m.low_factor,
            m.high_factor,
            m.start_on_power,
            m.min_counts,
            m.max_calib_retries
        ),
        (40, 10, 100, 60000, 2)
    );
}

#[test]
fn motor_each_motor_value_has_its_range() {
    let rig = Rig::started();
    let st = rig.storage();
    let mut web = rig.web(&st);
    for bad in [
        motor_body(9, 17, 30, 3000, 2),
        motor_body(41, 17, 30, 3000, 2),
        motor_body(17, 9, 30, 3000, 2),
        motor_body(17, 41, 30, 3000, 2),
        motor_body(17, 17, -1, 3000, 2),
        motor_body(17, 17, 101, 3000, 2),
        motor_body(17, 17, 30, -1, 2),
        motor_body(17, 17, 30, 60001, 2),
        motor_body(17, 17, 30, 3000, -1),
        motor_body(17, 17, 30, 3000, 3),
    ] {
        let r = perform(&mut web, api_post(MOTOR_URL, &bad));
        assert_eq!(r.status, 400, "{bad}");
        assert_eq!(text(&r), error_body("out_of_range", "motor"));
    }
    assert!(rig.submitted().is_empty());
}

#[test]
fn motor_a_partial_motor_update_keeps_the_values_read_from_the_stm() {
    let rig = Rig::started();
    {
        let mut st = rig.state();
        let m = &mut st.snapshot.motor;
        m.low_factor = 21;
        m.high_factor = 22;
        m.start_on_power = 23;
        m.min_counts = 2400;
        m.max_calib_retries = 1;
        m.field_count = 3;
    }
    let st = rig.storage();
    let mut web = rig.web(&st);
    // not read yet: only a complete set is accepted
    let r = perform(&mut web, api_post(MOTOR_URL, "{\"motor\":{\"lowC\":12}}"));
    assert_eq!(r.status, 409);
    assert_eq!(
        text(&r),
        error_body("unknown", "motor parameters not read yet: send all five")
    );
    let r = perform(
        &mut web,
        api_post(
            MOTOR_URL,
            "{\"motor\":{\"lowC\":12,\"highC\":13,\"startOnPower\":14,\"noOfMinCount\":15}}",
        ),
    );
    assert_eq!(r.status, 409);
    assert!(rig.submitted().is_empty());
    rig.state().snapshot.have_motor = true;
    for one in [
        "{\"motor\":{\"lowC\":12}}",
        "{\"motor\":{\"highC\":13}}",
        "{\"motor\":{\"startOnPower\":14}}",
        "{\"motor\":{\"noOfMinCount\":15}}",
        "{\"motor\":{\"maxCalReps\":0}}",
        "{\"motor\":{}}",
    ] {
        assert_eq!(
            perform(&mut web, api_post(MOTOR_URL, one)).status,
            202,
            "{one}"
        );
    }
    let c = rig.submitted();
    assert_eq!(c.len(), 6);
    assert_eq!(c[0].motor.low_factor, 12);
    assert_eq!(c[0].motor.high_factor, 22);
    assert_eq!(c[0].motor.start_on_power, 23);
    assert_eq!(c[0].motor.min_counts, 2400);
    assert_eq!(c[0].motor.max_calib_retries, 1);
    assert_eq!(c[0].motor.field_count, 5);
    assert_eq!(c[1].motor.low_factor, 21);
    assert_eq!(c[1].motor.high_factor, 13);
    assert_eq!(c[2].motor.start_on_power, 14);
    assert_eq!(c[2].motor.high_factor, 22);
    assert_eq!(c[3].motor.min_counts, 15);
    assert_eq!(c[3].motor.start_on_power, 23);
    assert_eq!(c[4].motor.max_calib_retries, 0);
    assert_eq!(c[4].motor.min_counts, 2400);
    assert!(c[5].has_motor);
    assert_eq!(c[5].motor.max_calib_retries, 1);
    // values read from the STM that a v1 STM would not keep are refused
    rig.state().snapshot.motor.low_factor = 5;
    let r = perform(&mut web, api_post(MOTOR_URL, "{\"motor\":{\"highC\":13}}"));
    assert_eq!(r.status, 400);
    assert_eq!(text(&r), error_body("out_of_range", "motor"));
    assert_eq!(rig.submitted().len(), 6);
}

#[test]
fn motor_the_members_and_their_shapes() {
    let rig = Rig::started();
    {
        let mut s = rig.state();
        s.proto = 2;
        s.snapshot.have_motor = true;
        s.snapshot.have_breakaway = true;
    }
    let st = rig.storage();
    let mut web = rig.web(&st);
    let r = perform(&mut web, api_post(MOTOR_URL, "{}"));
    assert_eq!(r.status, 400);
    assert_eq!(text(&r), error_body("bad_request", "nothing to set"));
    let r = perform(
        &mut web,
        api_post(MOTOR_URL, "{\"motor\":null,\"breakaway\":null}"),
    );
    assert_eq!(r.status, 400);
    assert_eq!(text(&r), error_body("bad_request", "nothing to set"));
    let r = perform(
        &mut web,
        api_post(MOTOR_URL, "{\"learnMovements\":0,\"x\":1}"),
    );
    assert_eq!(r.status, 400);
    assert_eq!(
        text(&r),
        error_body("unknown_key", "motor/learnMovements/breakaway")
    );
    for bad in [
        "{\"motor\":5}",
        "{\"motor\":[1]}",
        "{\"motor\":{\"lowC\":12,\"x\":1}}",
    ] {
        let r = perform(&mut web, api_post(MOTOR_URL, bad));
        assert_eq!(r.status, 400, "{bad}");
        assert_eq!(text(&r), error_body("invalid", "motor"));
    }
    for bad in [
        "{\"breakaway\":5}",
        "{\"breakaway\":{\"enable\":true,\"x\":1}}",
    ] {
        let r = perform(&mut web, api_post(MOTOR_URL, bad));
        assert_eq!(r.status, 400, "{bad}");
        assert_eq!(text(&r), error_body("invalid", "breakaway"));
    }
    assert!(rig.submitted().is_empty());
    // every member alone, and all three together
    for ok in [
        "{\"motor\":{\"lowC\":12,\"maxCalReps\":1}}",
        "{\"learnMovements\":100}",
        "{\"breakaway\":{\"enable\":true}}",
        "{\"motor\":{\"lowC\":12},\"learnMovements\":0,\"breakaway\":{\"stepPct\":5,\"maxmA\":30}}",
    ] {
        assert_eq!(
            perform(&mut web, api_post(MOTOR_URL, ok)).status,
            202,
            "{ok}"
        );
    }
    let c = rig.submitted();
    assert_eq!(c.len(), 4);
    assert!(c[0].has_motor && !c[0].has_learn_movements && !c[0].has_breakaway);
    assert!(!c[1].has_motor && c[1].has_learn_movements && !c[1].has_breakaway);
    assert_eq!(c[1].learn_movements, 100);
    assert!(!c[2].has_motor && !c[2].has_learn_movements && c[2].has_breakaway);
    assert!(c[3].has_motor && c[3].has_learn_movements && c[3].has_breakaway);
    assert_eq!(c[3].learn_movements, 0);
    assert_eq!(c[3].breakaway.step_pct, 5);
    assert_eq!(c[3].breakaway.max_ma, 30);
    rig.state().submit_result = false;
    let r = perform(&mut web, api_post(MOTOR_URL, "{\"learnMovements\":100}"));
    assert_eq!(r.status, 503);
    assert_eq!(text(&r), error_body("queue_full", "STM command queue full"));
}

#[test]
fn motor_learn_movements_is_0_or_50_to_65534() {
    let rig = Rig::started();
    let st = rig.storage();
    let mut web = rig.web(&st);
    for ok in [
        "{\"learnMovements\":0}",
        "{\"learnMovements\":50}",
        "{\"learnMovements\":65534}",
    ] {
        assert_eq!(
            perform(&mut web, api_post(MOTOR_URL, ok)).status,
            202,
            "{ok}"
        );
    }
    let s = rig.submitted();
    assert_eq!(
        s.iter().map(|c| c.learn_movements).collect::<Vec<_>>(),
        [0, 50, 65534]
    );
    for bad in [
        "{\"learnMovements\":-1}",
        "{\"learnMovements\":1}",
        "{\"learnMovements\":49}",
        "{\"learnMovements\":65535}",
        "{\"learnMovements\":true}",
        "{\"learnMovements\":\"50\"}",
    ] {
        let r = perform(&mut web, api_post(MOTOR_URL, bad));
        assert_eq!(r.status, 400, "{bad}");
        assert_eq!(
            text(&r),
            error_body("out_of_range", "learnMovements 0 or 50..65534")
        );
    }
    assert_eq!(rig.submitted().len(), 3);
}

#[test]
fn motor_breakaway_needs_protocol_2_and_a_complete_set_until_it_was_read() {
    let rig = Rig::started();
    {
        let mut s = rig.state();
        s.proto = 1;
        s.snapshot.breakaway.enable = true;
        s.snapshot.breakaway.step_pct = 7;
        s.snapshot.breakaway.max_ma = 44;
    }
    let st = rig.storage();
    let mut web = rig.web(&st);
    let r = perform(
        &mut web,
        api_post(MOTOR_URL, &breakaway_body("false", 0, 20)),
    );
    assert_eq!(r.status, 409);
    assert_eq!(
        text(&r),
        error_body("unsupported", "STM protocol v2 required")
    );
    rig.state().proto = 2;
    let r = perform(
        &mut web,
        api_post(
            MOTOR_URL,
            "{\"breakaway\":{\"enable\":false,\"stepPct\":3}}",
        ),
    );
    assert_eq!(r.status, 409);
    assert_eq!(
        text(&r),
        error_body("unknown", "breakaway not read yet: send all three")
    );
    assert!(rig.submitted().is_empty());
    assert_eq!(
        perform(
            &mut web,
            api_post(MOTOR_URL, &breakaway_body("false", 0, 20))
        )
        .status,
        202
    );
    assert_eq!(
        perform(
            &mut web,
            api_post(MOTOR_URL, &breakaway_body("true", 100, 60))
        )
        .status,
        202
    );
    let s = rig.submitted();
    assert_eq!(s.len(), 2);
    assert!(!s[0].breakaway.enable);
    assert_eq!((s[0].breakaway.step_pct, s[0].breakaway.max_ma), (0, 20));
    assert!(s[1].breakaway.enable);
    assert_eq!((s[1].breakaway.step_pct, s[1].breakaway.max_ma), (100, 60));
    for bad in [
        breakaway_body("1", 5, 30),
        breakaway_body("true", -1, 30),
        breakaway_body("true", 101, 30),
        breakaway_body("true", 5, 19),
        breakaway_body("true", 5, 61),
    ] {
        let r = perform(&mut web, api_post(MOTOR_URL, &bad));
        assert_eq!(r.status, 400, "{bad}");
        assert_eq!(text(&r), error_body("out_of_range", "breakaway"));
    }
    assert_eq!(rig.submitted().len(), 2);
    // read once: single members keep the other values
    rig.state().snapshot.have_breakaway = true;
    for one in [
        "{\"breakaway\":{\"stepPct\":9}}",
        "{\"breakaway\":{\"maxmA\":33}}",
        "{\"breakaway\":{\"enable\":false}}",
    ] {
        assert_eq!(
            perform(&mut web, api_post(MOTOR_URL, one)).status,
            202,
            "{one}"
        );
    }
    let c = rig.submitted();
    assert_eq!(c.len(), 5);
    assert!(c[2].breakaway.enable);
    assert_eq!((c[2].breakaway.step_pct, c[2].breakaway.max_ma), (9, 44));
    assert_eq!((c[3].breakaway.step_pct, c[3].breakaway.max_ma), (7, 33));
    assert!(!c[4].breakaway.enable);
    assert_eq!(c[4].breakaway.max_ma, 44);
    // a value read from the STM out of range is refused
    rig.state().snapshot.breakaway.max_ma = 10;
    let r = perform(
        &mut web,
        api_post(MOTOR_URL, "{\"breakaway\":{\"enable\":true}}"),
    );
    assert_eq!(r.status, 400);
    assert_eq!(text(&r), error_body("out_of_range", "breakaway"));
    assert_eq!(rig.submitted().len(), 5);
}

// ---------------------------------------------------------------- STM reset

#[test]
fn stm_reset_only_confirm_true_resets() {
    let rig = Rig::started();
    let st = rig.storage();
    let mut web = rig.web(&st);
    for bad in ["{}", "{\"confirm\":false}", "{\"confirm\":1}"] {
        let r = perform(&mut web, api_post("/api/stm/reset", bad));
        assert_eq!(r.status, 400, "{bad}");
        assert_eq!(
            text(&r),
            error_body("confirm_required", "{\\\"confirm\\\":true}")
        );
    }
    assert!(rig.submitted().is_empty());
    assert_eq!(
        perform(&mut web, api_post("/api/stm/reset", "{\"confirm\":true}")).status,
        202
    );
    let s = rig.submitted();
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].kind, StmCommandType::ResetStm);
    assert_eq!(s[0].valve, NO_VALVE);
}

// ---------------------------------------------------------------- images

#[test]
fn images_the_list_answers_every_image_once() {
    let rig = Rig::started();
    let fw = stm_image(true, "", "C2");
    put_images(&rig, &[("fw", fw.clone()), ("old", b"12345".to_vec())]);
    let st = rig.storage();
    st.service(); // fw only
    let mut web = rig.web(&st);
    let r = perform(&mut web, get("/api/stm/images"));
    assert_eq!(r.status, 200);
    assert_eq!(
        text(&r),
        format!(
            "[{{\"name\":\"fw\",\"size\":4096,\"crc32\":\"0x{:08x}\",\"version\":null,\
             \"check\":\"none\",\"hw\":\"C2\"}},{{\"name\":\"old\",\"size\":5,\"crc32\":null,\
             \"version\":null,\"check\":null,\"hw\":null}}]",
            vdm_esp_core::config::crc32(&fw, 0)
        )
    );
    for name in [b"fw".as_slice(), b"old"] {
        assert_eq!(st.delete_image(name), crate::storage::ImageResult::Ok);
    }
    assert_eq!(text(&perform(&mut web, get("/api/stm/images"))), "[]");
}

#[test]
fn image_delete_each_storage_result_has_its_answer() {
    let rig = Rig::started();
    let longest = "a".repeat(31);
    put_images(
        &rig,
        &[
            ("fw", good()),
            (&longest, good()),
            ("big", vec![0x5Au8; 20 * 1024]),
            ("held", good()),
        ],
    );
    let st = rig.storage();
    let mut web = rig.web(&st);
    let r = perform(&mut web, api_del("/api/stm/images/fw.bin"));
    assert_eq!(r.status, 204);
    assert!(rig.dev.fs.read("/stm/fw.bin").is_none());
    assert!(st.find_image(b"fw").is_none());
    let r = perform(&mut web, api_del(&format!("/api/stm/images/{longest}")));
    assert_eq!(r.status, 204);
    assert!(st.find_image(longest.as_bytes()).is_none());
    let r = perform(&mut web, api_del("/api/stm/images/fw"));
    assert_eq!(r.status, 404);
    assert_eq!(text(&r), error_body("not_found", "fw"));
    // the last_good copy reads the image
    st.request_last_good_copy(b"big");
    st.service();
    let r = perform(&mut web, api_del("/api/stm/images/big"));
    assert_eq!(r.status, 409);
    assert_eq!(text(&r), error_body("busy", "big"));
    rig.dev.fs.fail("remove", "/stm/held.bin", 1);
    let r = perform(&mut web, api_del("/api/stm/images/held"));
    assert_eq!(r.status, 500);
    assert_eq!(text(&r), error_body("io_error", "held"));
    rig.state().flash_active = true;
    let r = perform(&mut web, api_del("/api/stm/images/held"));
    assert_eq!(r.status, 409);
    assert_eq!(text(&r), error_body("flashing", "STM flash in progress"));
    assert!(st.find_image(b"held").is_some());
    let r = perform(&mut web, api_del("/api/stm/images/a%20b"));
    assert_eq!(r.status, 404); // not a {name}: no route
}

// ---------------------------------------------------------------- flash

#[test]
fn flash_the_request_members() {
    let rig = Rig::started();
    let longest = "b".repeat(31);
    put_images(&rig, &[("fw", good()), (&longest, good())]);
    let st = rig.storage();
    scan_all(&st);
    let mut web = rig.web(&st);
    let detail = "image, mode normal|blank, force, board C1|C2";
    for bad in [
        "{}",
        "{\"image\":5}",
        "{\"image\":\"fw\",\"board\":5}",
        "{\"image\":\"fw\",\"board\":\"C3\"}",
        "{\"image\":\"a b\"}",
        "{\"image\":\"fw\",\"mode\":\"fast\"}",
        "{\"image\":\"fw\",\"mode\":1}",
        "{\"image\":\"fw\",\"force\":1}",
        "{\"image\":\"fw\",\"x\":1}",
    ] {
        let r = perform(&mut web, api_post("/api/stm/flash", bad));
        assert_eq!(r.status, 400, "{bad}");
        assert_eq!(text(&r), error_body("bad_request", detail));
    }
    assert!(rig.submitted().is_empty());
    let r = perform(
        &mut web,
        api_post(
            "/api/stm/flash",
            "{\"image\":\"fw\",\"board\":\"C2\",\"mode\":\"normal\"}",
        ),
    );
    assert_eq!(r.status, 202);
    let r = perform(
        &mut web,
        api_post(
            "/api/stm/flash",
            &format!("{{\"image\":\"{longest}.bin\",\"force\":false}}"),
        ),
    );
    assert_eq!(r.status, 202);
    let s = rig.submitted();
    assert_eq!(s.len(), 2);
    assert_eq!(s[0].kind, StmCommandType::StartFlash);
    assert_eq!(s[0].image.as_slice(), b"fw");
    assert_eq!(s[0].board.as_slice(), b"C2");
    assert!(!s[0].blank);
    assert!(!s[0].force);
    assert_eq!(s[1].image.as_slice(), longest.as_bytes());
    rig.state().submit_result = false;
    let r = perform(&mut web, api_post("/api/stm/flash", "{\"image\":\"fw\"}"));
    assert_eq!(r.status, 503);
}

#[test]
fn flash_busy_restarting_unknown_and_unchecked_images_are_refused() {
    let rig = Rig::started();
    put_images(
        &rig,
        &[
            ("bad", bad_image()),
            ("fw", good()),
            ("nohs", stm_image(false, "1.4.9_Dev", "")),
            ("pending", good()),
        ],
    );
    let st = rig.storage();
    for _ in 0..3 {
        st.service(); // bad, fw, nohs; pending stays unscanned
    }
    let mut web = rig.web(&st);
    let fw = || api_post("/api/stm/flash", "{\"image\":\"fw\"}");
    let mut other = rig.ota_upload();
    assert!(other.upload_begin(10, b""));
    let r = perform(&mut web, fw());
    assert_eq!(r.status, 409);
    assert_eq!(text(&r), error_body("busy", "upload or flash running"));
    assert!(!other.upload_end(false));
    let r = perform(&mut web, api_post("/api/stm/flash", "{\"image\":\"none\"}"));
    assert_eq!(r.status, 404);
    assert_eq!(text(&r), error_body("not_found", "none"));
    let r = perform(
        &mut web,
        api_post("/api/stm/flash", "{\"image\":\"pending\"}"),
    );
    assert_eq!(r.status, 409);
    assert_eq!(
        text(&r),
        error_body("validating", "image check pending, retry")
    );
    let r = perform(&mut web, api_post("/api/stm/flash", "{\"image\":\"nohs\"}"));
    assert_eq!(r.status, 400);
    assert_eq!(text(&r), error_body("invalid_image", "image_no_handshake"));
    let r = perform(
        &mut web,
        api_post("/api/stm/flash", "{\"image\":\"bad\",\"force\":true}"),
    );
    assert_eq!(r.status, 400);
    assert_eq!(text(&r), error_body("invalid_image", "image_bad_vectors"));
    assert!(rig.submitted().is_empty());
    // force flashes an image without the handshake marker
    let r = perform(
        &mut web,
        api_post("/api/stm/flash", "{\"image\":\"nohs\",\"force\":true}"),
    );
    assert_eq!(r.status, 202);
    let s = rig.submitted();
    assert_eq!(s.len(), 1);
    assert!(s[0].force);
    assert_eq!(s[0].image.as_slice(), b"nohs");
    // a pending restart (stays pending: last)
    let now = rig.dev.clock.now_ms();
    let _ = rig.ota_shared.request_restart(now, 0, 1000, 0);
    let r = perform(&mut web, fw());
    assert_eq!(r.status, 409);
    assert_eq!(text(&r), error_body("restarting", "ESP restart pending"));
    assert_eq!(rig.submitted().len(), 1);
}

#[test]
fn flash_the_running_stms_board_tag_decides_even_a_one_letter_one() {
    let rig = Rig::started();
    put_images(&rig, &[("fw", stm_image(true, "1.4.9_Dev", "C1"))]);
    set_text(&mut rig.state().snapshot.version.hw, b"C");
    let st = rig.storage();
    scan_all(&st);
    let mut web = rig.web(&st);
    let r = perform(
        &mut web,
        api_post("/api/stm/flash", "{\"image\":\"fw\",\"board\":\"C1\"}"),
    );
    assert_eq!(r.status, 409);
    assert_eq!(text(&r), error_body("board_mismatch", "image C1, board C"));
    assert!(rig.submitted().is_empty());
}

#[test]
fn flash_abort_only_a_running_flash_is_aborted() {
    let rig = Rig::started();
    let st = rig.storage();
    let mut web = rig.web(&st);
    let r = perform(&mut web, api_post("/api/stm/flash/abort", ""));
    assert_eq!(r.status, 409);
    assert_eq!(text(&r), error_body("idle", "no flash running"));
    assert!(rig.submitted().is_empty());
    rig.state().flash_active = true;
    let r = perform(&mut web, api_post("/api/stm/flash/abort", ""));
    assert_eq!(r.status, 202);
    assert_eq!(text(&r), "{\"result\":\"queued\"}");
    let s = rig.submitted();
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].kind, StmCommandType::AbortFlash);
    rig.state().submit_result = false;
    assert_eq!(
        perform(&mut web, api_post("/api/stm/flash/abort", "")).status,
        503
    );
}

// ---------------------------------------------------------------- factory reset

#[test]
fn factory_reset_confirmed_erased_restart_requested() {
    let rig = Rig::started();
    rig.dev.nvs.set_u32(NAMESPACE, "boots", 7);
    rig.dev.nvs.set_u8(NAMESPACE, KEY_FACTORY_LATCH, 1);
    let st = rig.storage();
    let mut web = rig.web(&st);
    for bad in ["{}", "{\"confirm\":\"yes\"}", "{\"confirm\":true}"] {
        let r = perform(&mut web, api_post("/api/system/factory-reset", bad));
        assert_eq!(r.status, 400, "{bad}");
        assert_eq!(
            text(&r),
            error_body(
                "confirm_required",
                "{\\\"confirm\\\":\\\"factory-reset\\\"}"
            )
        );
    }
    assert!(rig.dev.nvs.has(NAMESPACE, "boots"));
    let confirmed = || {
        api_post(
            "/api/system/factory-reset",
            "{\"confirm\":\"factory-reset\"}",
        )
    };
    rig.state().flash_active = true;
    let r = perform(&mut web, confirmed());
    assert_eq!(r.status, 409);
    assert_eq!(text(&r), error_body("flashing", "STM flash in progress"));
    assert!(rig.dev.nvs.has(NAMESPACE, "boots"));
    rig.state().flash_active = false;
    rig.dev.nvs.knobs().fail_erase = true;
    let r = perform(&mut web, confirmed());
    assert_eq!(r.status, 500);
    assert_eq!(text(&r), error_body("nvs", "erase failed"));
    assert!(!rig.ota_shared.restart_pending());
    rig.dev.nvs.knobs().fail_erase = false;
    rig.dev.clock.set_ms(3000);
    let r = perform(&mut web, confirmed());
    assert_eq!(r.status, 202);
    assert_eq!(text(&r), "{\"result\":\"restarting\"}");
    // erased, the latch and the import flag kept
    assert!(!rig.dev.nvs.has(NAMESPACE, "boots"));
    assert_eq!(rig.dev.nvs.get_i(NAMESPACE, KEY_IMPORTED), 1);
    assert_eq!(rig.dev.nvs.get_i(NAMESPACE, KEY_FACTORY_LATCH), 1);
    let ev = rig.host.with_code(EventCode::RebootRequested);
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].arg1, 3);
    assert_restart_due(&rig, 3000, 1000);
}

// ---------------------------------------------------------------- discovery

#[test]
fn mqtt_discovery_an_unknown_action_is_refused() {
    let rig = Rig::started();
    rig.config(|c| c.mqtt.mode = vdm_esp_core::config::MqttMode::Mqtt);
    let st = rig.storage();
    let mut web = rig.web(&st);
    for bad in ["{}", "{\"action\":\"clear\"}", "{\"action\":1}"] {
        let r = perform(&mut web, api_post("/api/mqtt/discovery", bad));
        assert_eq!(r.status, 400, "{bad}");
        assert_eq!(
            text(&r),
            error_body("bad_request", "action publish|delete|republish")
        );
    }
    assert!(rig.state().discovery.is_empty());
}
