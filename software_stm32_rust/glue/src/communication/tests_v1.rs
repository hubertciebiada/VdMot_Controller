// Port of test/native/glue/test_communication_v1.cpp: the v1 and v2 replies keep the bytes of
// firmware 2.0.0 (58632d6) for every command, valid and invalid arguments, against stub values.

use super::tests::Rig;
use super::*;
use crate::test_support::stubs::VALVE_SENSOR_UNKNOWN;
use vdm_stm_core::replies_v2::EEP_STATE_PENDING;

const ROM_A: DeviceAddress = [0x28, 0x84, 0x37, 0x94, 0x97, 0xFF, 0x03, 0x23];
const ROM_B: DeviceAddress = [0x28, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07];
const TEXT_A: &str = "28-84-37-94-97-ff-03-23";
const TEXT_B: &str = "28-01-02-03-04-05-06-07";

fn two_sensors(r: &mut Rig) {
    let s = &mut r.stubs.sensors;
    s.tempsensors[0].address = ROM_A;
    s.tempsensors[1].address = ROM_B;
    s.tempsensors[0].temperature = 215;
    s.tempsensors[1].temperature = -53;
    s.no_of_ds18_devices = 2;
}

#[test]
fn v1_gonec_and_goned() {
    let mut r = Rig::begin();
    assert_eq!(r.request("gonec\n"), "gonec 0 \r\n");
    assert_eq!(r.request("gonec 255\n"), "gonec 0 \r\n");
    two_sensors(&mut r);
    assert_eq!(r.request("gonec\n"), "gonec 2 \r\n");
    assert_eq!(
        r.request("gonec 255\n"),
        format!("gonec 2 {TEXT_A},{TEXT_B} \r\n")
    );
    assert!(r.request("gonec 3\n").is_empty());
    assert_eq!(r.request("goned 1\n"), format!("goned {TEXT_B} -53 \r\n"));
    assert_eq!(r.request("goned 34\n"), "goned 0 \r\n");
    assert_eq!(r.request("goned\n"), "goned 0 \r\n");
}

#[test]
fn v1_gowvc_and_gowvd() {
    let mut r = Rig::begin();
    r.stubs.sensors.voltsensors[0].address = ROM_A;
    r.stubs.sensors.voltsensors[0].vad = 450;
    r.stubs.sensors.no_of_ds2438_devices = 1;
    assert_eq!(r.request("gowvc\n"), "gowvc 1 \r\n");
    assert_eq!(r.request("gowvc 255\n"), format!("gowvc 1 {TEXT_A} \r\n"));
    assert_eq!(r.request("gowvd 0\n"), format!("gowvd {TEXT_A} 450 \r\n"));
    assert_eq!(r.request("gowvd 8\n"), "gowvd 0 \r\n");
}

#[test]
fn v1_gvlon_for_one_valve_all_valves_and_an_invalid_index() {
    let mut r = Rig::begin();
    two_sensors(&mut r);
    r.stubs.valves[1].sensorindex1 = 0;
    r.stubs.valves[1].sensorindex2 = VALVE_SENSOR_UNKNOWN;
    assert_eq!(
        r.request("gvlon 1\n"),
        format!("gvlon 1 {TEXT_A} 00-00-00-00-00-00-00-00 \r\n")
    );
    let all = r.request("gvlon 255\n");
    assert!(all.starts_with("gvlon 12 "));
    assert_eq!(all.len(), 9 + 12 * 47 + 11 + 3);
    assert_eq!(r.request("gvlon 12\n"), "goned error \r\n");
}

#[test]
fn v1_stons_stlnt_stlnm_gtlnm_staop_staln() {
    let mut r = Rig::begin();
    assert_eq!(r.request("stons\n"), "stons\r\n");
    assert_eq!(r.request("stlnt 100\n"), "stlnt\r\n");
    assert_eq!(r.request("stlnm 100\n"), "stlnm\r\n");
    assert_eq!(r.request("gtlnm\n"), "gtlnm 2000 \r\n");
    assert_eq!(r.request("staop 255\n"), "staop \r\n");
    assert_eq!(r.request("staln 3\n"), "staln\r\n");
    r.stubs.app.set_valve_open = -1;
    r.stubs.app.set_valve_learning = -1;
    r.stubs.app.set_learn_movements = -1;
    assert!(r.request("staop 20\n").is_empty());
    assert!(r.request("staln 20\n").is_empty());
    assert!(r.request("stlnm 100\n").is_empty());
    assert!(r.request("stlnt\n").is_empty());
    assert_eq!(
        r.calls(),
        vec![
            "temp_command(1)",
            "app_set_learntime(100)",
            "eeprom_changed(0x0010)",
            "app_set_learnmovements(100)",
            "eeprom_changed(0x0002)",
            "app_set_valveopen(255)",
            "app_set_valvelearning(3)",
            "app_set_valveopen(20)",
            "app_set_valvelearning(20)",
            "app_set_learnmovements(100)",
        ]
    );
}

#[test]
fn v1_smotc_and_gmotc() {
    let mut r = Rig::begin();
    assert_eq!(r.request("gmotc\n"), "gmotc 17 17 50 3000 0 \r\n");
    assert_eq!(r.request("smotc 18 19 40\n"), "smotc\r\n");
    assert_eq!(r.request("smotc 18 19 40\n"), "smotc\r\n");
    assert_eq!(r.calls_of("eeprom_changed"), vec!["eeprom_changed(0x0004)"]);
    assert_eq!(r.request("smotc 18 19\n"), "smotc err\r\n");
    assert_eq!(r.request("smotc 18 99 40\n"), "smotc err\r\n");
}

#[test]
fn v1_ghwin_masns_eepst_gvlst_reset() {
    let mut r = Rig::begin();
    assert_eq!(r.request("ghwin\n"), "ghwin 1059 \r\n");
    assert_eq!(r.request("masns\n"), "masns \r\n");
    assert_eq!(r.request("eepst\n"), "eepst 1 \r\n");
    r.stubs.eeprom.state = EEP_STATE_PENDING;
    assert_eq!(r.request("eepst\n"), "eepst 0 \r\n");
    r.stubs.valves[0].status = 8;
    r.stubs.valves[11].status = 6;
    assert_eq!(
        r.request("gvlst\n"),
        "gvlst 12 8,0,0,0,0,0,0,0,0,0,0,6 \r\n"
    );
    assert_eq!(r.request("reset\n"), "reset \r\n");
    assert_eq!(r.calls_of("reset_STM32"), vec!["reset_STM32()"]);
}

#[test]
fn v2_gprof_svmov_scalx_gcalx_gmotx() {
    let mut r = Rig::begin();
    assert_eq!(r.request("gprof 0\n"), "gprof 0 0\r\n");
    assert!(r.request("gprof 12\n").is_empty());
    assert_eq!(r.request("svmov 2 1 100 20\n"), "svmov 2 ok\r\n");
    r.stubs.app.service_move = -3;
    assert_eq!(r.request("svmov 2 1 100 20\n"), "svmov 2 err 3\r\n");
    r.stubs.app.service_move = -2;
    assert_eq!(r.request("svmov 2 1 100 20\n"), "svmov 2 err 2\r\n");
    assert_eq!(r.request("svmov 2 1 0 20\n"), "svmov 2 err 1\r\n");
    assert_eq!(r.request("svmov 12 1 100 20\n"), "svmov -1 err 1\r\n");
    assert_eq!(r.request("scalx 1 30 40\n"), "scalx ok\r\n");
    assert_eq!(r.request("scalx 2 30 40\n"), "scalx err\r\n");
    assert_eq!(r.request("gcalx\n"), "gcalx 1 30 40\r\n");
    assert!(r.request("gmotx\n").starts_with("gmotx "));
    assert!(r.request("ESPalive\n").is_empty());
}
