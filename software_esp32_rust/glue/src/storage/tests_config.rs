//! `test_storage_config.cpp`: the config load order (every row of the load table), the cfgx
//! blob, the backup files, the legacy import report and image cleanup, factory reset, the file
//! list and delete of the file manager. A second load of a C++ case is the load of the next
//! boot here. Then the new Rust cases (multi-boot: a save loaded by the next boot, the import
//! that runs once, a failed import save that the next boot retries).
#![allow(clippy::large_stack_frames, clippy::large_stack_arrays)]

use std::collections::VecDeque;

use super::rig::*;
use super::*;
use crate::testkit::{Device, Reset};
use vdm_esp_core::common::copy_string;
use vdm_esp_core::config::{crc32, DecodeResult, REPAIR_FIELD, REPAIR_HA_IDS, REPAIR_TOPICS};
use vdm_esp_core::file_manager::FileEntry;
use vdm_esp_core::json_writer::JsonWriter;
use vdm_esp_core::legacy_import::{
    write_import_report_json, DROPPED_DS18_TIMEOUT, DROPPED_MESSENGER, DROPPED_PI,
};

/// A config with ext keys away from their defaults.
fn with_ext(station: &[u8]) -> Box<Config> {
    let mut c = named(station);
    c.failsafe.timeout_min = 90;
    copy_string(&mut c.valves[1].topic, b"Bad/WC");
    c
}

fn store_nvs(dev: &Device, c: &Config) {
    dev.nvs.set_blob(NAMESPACE, KEY_CONFIG, &blob_of(c));
    dev.nvs.set_blob(NAMESPACE, KEY_CONFIG_EXT, &ext_of(c, &[]));
}

fn store_backup(dev: &Device, c: &Config) {
    dev.fs.put(BACKUP_BASE, &blob_of(c));
    dev.fs.put(BACKUP_EXT, &ext_of(c, &[]));
}

fn renames(dev: &Device) -> Vec<String> {
    dev.journal.of("fs rename")
}

fn rename(from: &str, to: &str) -> String {
    format!("fs rename {from} {to}")
}

// ---------------------------------------------------------------- load table

#[test]
fn load_cfg_and_cfgx_are_used_no_event_the_backup_follows_once() {
    let rig = Rig::new();
    let st = rig.storage();
    mount(&st);
    let c = with_ext(b"Stored");
    store_nvs(&rig.dev, &c);
    let l = load(&st);
    assert_eq!(l.src, LoadSource::Stored);
    assert_eq!(l.cfg.station.as_slice(), b"Stored");
    assert_eq!(l.cfg.failsafe.timeout_min, 90);
    assert_eq!(l.cfg.valves[1].topic.as_slice(), b"Bad/WC");
    assert_eq!(l.details.error_code, 0);
    assert_eq!(l.details.info.ext, ExtResult::Ok);
    assert!(rig.host.events().is_empty());
    assert!(!has_file(&rig.dev, BACKUP_BASE));
    rig.shared.set_active_config(&l.cfg);
    st.service();
    assert_eq!(file(&rig.dev, BACKUP_BASE).unwrap(), blob_of(&c));
    assert_eq!(file(&rig.dev, BACKUP_EXT).unwrap(), ext_of(&c, &[]));
    assert!(!has_file(&rig.dev, "/sys/cfg.bak.tmp"));
    assert!(!has_file(&rig.dev, "/sys/cfgx.bak.tmp"));
    assert_eq!(
        renames(&rig.dev),
        vec![
            rename("/sys/cfg.bak.tmp", BACKUP_BASE),
            rename("/sys/cfgx.bak.tmp", BACKUP_EXT)
        ]
    );
    // Nothing pending any more.
    let writes = rig.dev.fs.knobs().write_opens;
    st.service();
    assert_eq!(rig.dev.fs.knobs().write_opens, writes);
}

#[test]
fn load_backup_files_equal_to_nvs_are_not_written_again() {
    let rig = Rig::new();
    let st = rig.storage();
    mount(&st);
    let c = with_ext(b"Same");
    store_nvs(&rig.dev, &c);
    store_backup(&rig.dev, &c);
    let l = load(&st);
    assert_eq!(l.src, LoadSource::Stored);
    // C++: both files were compared through 512 B stdio buffers; no Rust form (no stdio
    // layer): the two opens of the comparison are counted.
    assert_eq!(rig.dev.fs.knobs().opens, 2);
    rig.shared.set_active_config(&l.cfg);
    let writes = rig.dev.fs.knobs().write_opens;
    st.service();
    assert_eq!(rig.dev.fs.knobs().write_opens, writes);
}

#[test]
fn load_a_cfgx_backup_that_differs_is_rewritten_also_a_missing_one() {
    let rig = Rig::new();
    let st = rig.storage();
    mount(&st);
    let c = with_ext(b"Diff");
    store_nvs(&rig.dev, &c);
    rig.dev.fs.put(BACKUP_BASE, &blob_of(&c));
    rig.dev.fs.put(BACKUP_EXT, &ext_of(&named(b"Diff"), &[]));
    let l = load(&st);
    rig.shared.set_active_config(&l.cfg);
    st.service();
    assert_eq!(file(&rig.dev, BACKUP_EXT).unwrap(), ext_of(&c, &[]));
}

#[test]
fn load_a_cfg_without_cfgx_2_0_0_loads_with_the_new_keys_at_defaults() {
    let rig = Rig::new();
    let st = rig.storage();
    mount(&st);
    rig.dev
        .nvs
        .set_blob(NAMESPACE, KEY_CONFIG, &blob_of(&with_ext(b"Old")));
    let l = load(&st);
    assert_eq!(l.src, LoadSource::Stored);
    assert_eq!(l.details.info.ext, ExtResult::Absent);
    assert_eq!(l.cfg.failsafe.timeout_min, 60);
    assert!(l.cfg.valves[1].topic.is_empty());
    assert!(rig.host.events().is_empty());
}

#[test]
fn load_a_damaged_cfgx_with_a_good_cfg_loads_the_new_keys_at_defaults_the_backup_kept() {
    let rig = Rig::new();
    let st = rig.storage();
    mount(&st);
    let c = with_ext(b"Ext");
    store_backup(&rig.dev, &c);
    rig.dev.nvs.set_blob(NAMESPACE, KEY_CONFIG, &blob_of(&c));
    rig.dev
        .nvs
        .set_blob(NAMESPACE, KEY_CONFIG_EXT, &corrupt(ext_of(&c, &[])));
    let l = load(&st);
    assert_eq!(l.src, LoadSource::Stored);
    assert_eq!(l.details.error_code, 0);
    assert_eq!(l.details.info.ext, ExtResult::BadCrc);
    assert_eq!(l.cfg.station.as_slice(), b"Ext");
    assert_eq!(l.cfg.failsafe.timeout_min, 60);
    assert!(l.cfg.valves[1].topic.is_empty());
    assert!(rig.host.events().is_empty());
    // the backup still holds the new keys: only an explicit save replaces it
    rig.shared.set_active_config(&l.cfg);
    let writes = rig.dev.fs.knobs().write_opens;
    st.service();
    assert_eq!(rig.dev.fs.knobs().write_opens, writes);
    assert_eq!(file(&rig.dev, BACKUP_EXT).unwrap(), ext_of(&c, &[]));
}

#[test]
fn load_repairs_are_logged_once_with_the_first_key_path() {
    let rig = Rig::new();
    let st = rig.storage();
    mount(&st);
    let mut e = named(b"Rep");
    copy_string(&mut e.valves[0].name, b"Bad");
    let mut broken = e.clone();
    broken.calib.hour = 24; // base blob: a field repair
    let base = blob_of(&broken);
    let mut ext = e.clone();
    copy_string(&mut ext.valves[3].topic, b"Bad"); // V2 after cfgx
    rig.dev.nvs.set_blob(NAMESPACE, KEY_CONFIG, &base);
    rig.dev
        .nvs
        .set_blob(NAMESPACE, KEY_CONFIG_EXT, &ext_of(&ext, &[]));
    let l = load(&st);
    assert_eq!(l.src, LoadSource::Stored);
    assert_eq!(l.cfg.calib.hour, 0);
    assert!(l.cfg.valves[3].topic.is_empty());
    let ev = rig.host.with_code(EventCode::ConfigRepaired);
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].arg1, (REPAIR_FIELD | REPAIR_TOPICS) as i32);
    assert_eq!(ev[0].arg2, 2);
    assert_eq!(ev[0].text.as_slice(), b"calib.hour");
    assert_eq!(rig.host.events().len(), 1);
}

#[test]
fn load_a_repair_only_after_the_ext_records_names_that_key() {
    let rig = Rig::new();
    let st = rig.storage();
    mount(&st);
    let mut e = named(b"Rep");
    copy_string(&mut e.valves[0].name, b"Bad");
    let mut ext = e.clone();
    copy_string(&mut ext.valves[3].topic, b"Bad");
    rig.dev.nvs.set_blob(NAMESPACE, KEY_CONFIG, &blob_of(&e));
    rig.dev
        .nvs
        .set_blob(NAMESPACE, KEY_CONFIG_EXT, &ext_of(&ext, &[]));
    load(&st);
    let ev = rig.host.with_code(EventCode::ConfigRepaired);
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].arg1, REPAIR_TOPICS as i32);
    assert_eq!(ev[0].arg2, 1);
    assert_eq!(ev[0].text.as_slice(), b"valves.4.topic");
}

#[test]
fn load_a_newer_firmwares_config_is_logged_its_unknown_keys_survive_a_save() {
    let rig = Rig::new();
    let st = rig.storage();
    mount(&st);
    let c = named(b"Newer");
    let mut base = blob_of(&c);
    base[4] = 2; // base schema 2: read by its schema-1 prefix
    let n = base.len();
    let crc = crc32(&base[..n - 4], 0);
    base[n - 4..].copy_from_slice(&crc.to_le_bytes());
    let unknown = [200u8, 0, 2, 7, 9];
    rig.dev.nvs.set_blob(NAMESPACE, KEY_CONFIG, &base);
    rig.dev
        .nvs
        .set_blob(NAMESPACE, KEY_CONFIG_EXT, &ext_of(&c, &unknown));
    let l = load(&st);
    assert_eq!(l.src, LoadSource::Stored);
    let ev = rig.host.with_code(EventCode::ConfigNewerSchema);
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].arg1, 2);
    assert_eq!(ev[0].arg2, 1);
    assert!(!rig.host.has(EventCode::ConfigRepaired));
    // The next save writes the unknown record back.
    let mut path = [0u8; 16];
    assert_eq!(st.apply_config(&named(b"Saved"), &mut path), Ok(()));
    assert_eq!(
        rig.dev.nvs.get_blob(NAMESPACE, KEY_CONFIG_EXT),
        ext_of(&named(b"Saved"), &unknown)
    );
}

#[test]
fn load_only_unknown_cfgx_records_are_logged_as_a_newer_firmware() {
    let rig = Rig::new();
    let st = rig.storage();
    mount(&st);
    let c = named(b"Ext");
    rig.dev.nvs.set_blob(NAMESPACE, KEY_CONFIG, &blob_of(&c));
    rig.dev.nvs.set_blob(
        NAMESPACE,
        KEY_CONFIG_EXT,
        &ext_of(&c, &[201, 0, 0, 202, 1, 1, 5]),
    );
    load(&st);
    let ev = rig.host.with_code(EventCode::ConfigNewerSchema);
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].arg1, 1);
    assert_eq!(ev[0].arg2, 2);
}

#[test]
fn load_an_unusable_cfg_is_replaced_by_the_backup_nvs_rewritten() {
    let rig = Rig::new();
    let st = rig.storage();
    mount(&st);
    let good = with_ext(b"Backup");
    store_backup(&rig.dev, &good);
    let bad = corrupt(blob_of(&named(b"Bad")));
    rig.dev.nvs.set_blob(NAMESPACE, KEY_CONFIG, &bad);
    rig.dev
        .nvs
        .set_blob(NAMESPACE, KEY_CONFIG_EXT, &ext_of(&named(b"Bad"), &[]));
    let l = load(&st);
    assert_eq!(l.src, LoadSource::Backup);
    assert_eq!(rig.shared.boot_load_source(), LoadSource::Backup);
    assert_eq!(l.cfg.station.as_slice(), b"Backup");
    assert_eq!(l.cfg.failsafe.timeout_min, 90);
    assert_eq!(l.details.error_code, DecodeResult::BadCrc as u8);
    assert_eq!(rig.dev.nvs.get_blob(NAMESPACE, KEY_CONFIG), blob_of(&good));
    assert_eq!(
        rig.dev.nvs.get_blob(NAMESPACE, KEY_CONFIG_EXT),
        ext_of(&good, &[])
    );
    let ev = rig.host.with_code(EventCode::ConfigRestored);
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].arg1, DecodeResult::BadCrc as i32);
    assert_eq!(rig.host.events().len(), 1);
    // C++: both backup files read through 512 B stdio buffers; no Rust form (no stdio layer):
    // the two opens are counted.
    assert_eq!(rig.dev.fs.knobs().opens, 2);
}

#[test]
fn load_a_backup_without_cfgx_bak_restores_the_base_and_removes_cfgx() {
    let rig = Rig::new();
    let st = rig.storage();
    mount(&st);
    rig.dev.fs.put(BACKUP_BASE, &blob_of(&named(b"Base")));
    rig.dev
        .nvs
        .set_blob(NAMESPACE, KEY_CONFIG, &corrupt(blob_of(&named(b"Bad"))));
    rig.dev
        .nvs
        .set_blob(NAMESPACE, KEY_CONFIG_EXT, &ext_of(&with_ext(b"Bad"), &[]));
    let l = load(&st);
    assert_eq!(l.src, LoadSource::Backup);
    assert_eq!(l.cfg.station.as_slice(), b"Base");
    assert_eq!(l.cfg.failsafe.timeout_min, 60);
    assert!(!rig.dev.nvs.has(NAMESPACE, KEY_CONFIG_EXT));
}

#[test]
fn load_an_unreadable_cfg_too_large_is_replaced_by_the_backup_reason_101() {
    let rig = Rig::new();
    let st = rig.storage();
    mount(&st);
    store_backup(&rig.dev, &named(b"Backup"));
    rig.dev
        .nvs
        .set_blob(NAMESPACE, KEY_CONFIG, &vec![1u8; CONFIG_BLOB_MAX + 1]);
    let l = load(&st);
    assert_eq!(l.src, LoadSource::Backup);
    assert_eq!(l.details.error_code, 101);
    assert_eq!(rig.host.with_code(EventCode::ConfigRestored)[0].arg1, 101);
}

#[test]
fn load_an_unusable_cfg_and_an_unusable_backup_give_defaults() {
    let rig = Rig::new();
    let st = rig.storage();
    mount(&st);
    rig.dev.fs.put(BACKUP_BASE, &corrupt(blob_of(&named(b"B"))));
    let bad = corrupt(blob_of(&named(b"Bad")));
    rig.dev.nvs.set_blob(NAMESPACE, KEY_CONFIG, &bad);
    let l = load(&st);
    assert_eq!(l.src, LoadSource::DefaultsAfterError);
    assert_eq!(l.cfg.station.as_slice(), b"VdMot");
    assert_eq!(rig.dev.nvs.get_blob(NAMESPACE, KEY_CONFIG), bad);
    assert_eq!(
        rig.host.with_code(EventCode::ConfigDefaults)[0].arg1,
        DecodeResult::BadCrc as i32
    );
    assert!(!rig.host.has(EventCode::ConfigRestored));
}

#[test]
fn load_without_a_mounted_file_system_the_backup_is_not_tried() {
    let rig = Rig::new();
    store_backup(&rig.dev, &named(b"Backup"));
    rig.dev
        .nvs
        .set_blob(NAMESPACE, KEY_CONFIG, &corrupt(blob_of(&named(b"Bad"))));
    let st = rig.storage();
    let l = load(&st);
    assert_eq!(l.src, LoadSource::DefaultsAfterError);
}

#[test]
fn load_nvs_erased_after_a_factory_reset_gives_defaults_not_the_backup() {
    let rig = Rig::new();
    let st = rig.storage();
    mount(&st);
    store_backup(&rig.dev, &named(b"Backup"));
    rig.dev.nvs.set_u8(NAMESPACE, KEY_IMPORTED, 1);
    let l = load(&st);
    assert_eq!(l.src, LoadSource::Defaults);
    assert_eq!(l.cfg.station.as_slice(), b"VdMot");
    assert!(rig.host.events().is_empty());
}

#[test]
fn load_an_empty_nvs_without_the_import_flag_takes_the_backup_reason_0() {
    let rig = Rig::new();
    let st = rig.storage();
    mount(&st);
    store_backup(&rig.dev, &with_ext(b"Backup"));
    rig.dev.nvs.set_str("sysCfg", "stName", b"Legacy"); // not imported: the backup wins
    let l = load(&st);
    assert_eq!(l.src, LoadSource::Backup);
    assert_eq!(l.cfg.station.as_slice(), b"Backup");
    assert_eq!(
        rig.dev.nvs.get_blob(NAMESPACE, KEY_CONFIG),
        blob_of(&with_ext(b"Backup"))
    );
    assert_eq!(rig.host.with_code(EventCode::ConfigRestored)[0].arg1, 0);
    assert!(!rig.host.has(EventCode::ConfigImported));
}

#[test]
fn load_the_legacy_import_writes_the_report_logs_dropped_features_removes_images() {
    let rig = Rig::new();
    let st = rig.storage();
    mount(&st);
    let nvs = &rig.dev.nvs;
    nvs.set_str("sysCfg", "stName", b"");
    nvs.set_u8("protCfg", "brokerMQF", 0x02);
    nvs.set_u8("msgCfg", "msgFlags", 1);
    let mut ctrl = vec![0u8; 12 * 20];
    ctrl[0] = 0x01;
    ctrl[40] = 0x01;
    nvs.set_blob("valvesCtrlCfg", "valvesCtrl", &ctrl);
    rig.dev.fs.put("/x.bin", &[b'x'; 1500]);
    rig.dev.fs.put("/Y.BIN", &[b'y'; 700]);
    rig.dev.fs.put("/HADiscovery.cfg", b"list");
    rig.dev.fs.put("/stm/fw.bin", b"img");
    let l = load(&st);
    assert_eq!(l.src, LoadSource::Imported);
    assert_eq!(l.cfg.mqtt.root_topic.as_slice(), b"VdMotFBH");
    assert_eq!(nvs_int(&rig.dev, NAMESPACE, KEY_IMPORTED), 1);
    assert!(nvs.has(NAMESPACE, KEY_CONFIG_EXT));
    let ev = rig.host.events();
    assert_eq!(ev.len(), 3);
    assert_eq!(ev[0].code, EventCode::ConfigImported);
    assert_eq!(ev[1].code, EventCode::ImportDropped);
    assert_eq!(ev[1].arg1, 2);
    assert_eq!(
        ev[1].arg2,
        i32::from(DROPPED_PI | DROPPED_MESSENGER | DROPPED_DS18_TIMEOUT)
    );
    assert_eq!(
        ev[1].text.as_slice(),
        format!("ignored {} keys", l.report.ignored).as_bytes()
    );
    assert_eq!(ev[2].code, EventCode::FilesRemoved);
    assert_eq!(ev[2].arg1, 2);
    assert_eq!(ev[2].arg2, 3); // 2200 bytes
    assert_eq!(ev[2].text.as_slice(), b"legacy images");
    assert!(!has_file(&rig.dev, "/x.bin"));
    assert!(!has_file(&rig.dev, "/Y.BIN"));
    assert!(has_file(&rig.dev, "/HADiscovery.cfg"));
    assert!(has_file(&rig.dev, "/stm/fw.bin"));
    let mut expect = vec![0u8; 2048];
    let mut jw = JsonWriter::new(&mut expect);
    assert!(write_import_report_json(&mut jw, &l.report, &l.cfg));
    assert_eq!(file(&rig.dev, IMPORT_REPORT_FILE).unwrap(), jw.as_bytes());
    assert!(st.has_import_report());
    // The import save sets the backup flag.
    rig.shared.set_active_config(&l.cfg);
    st.service();
    assert_eq!(file(&rig.dev, BACKUP_BASE).unwrap(), blob_of(&l.cfg));
}

#[test]
fn load_an_import_without_dropped_features_or_images_logs_only_the_import() {
    let rig = Rig::new();
    let st = rig.storage();
    mount(&st);
    rig.dev.nvs.set_str("sysCfg", "stName", b"Legacy");
    let l = load(&st);
    assert_eq!(l.src, LoadSource::Imported);
    let ev = rig.host.events();
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].code, EventCode::ConfigImported);
    assert!(has_file(&rig.dev, IMPORT_REPORT_FILE));
}

#[test]
fn load_only_the_pi_count_also_logs_the_dropped_features() {
    let rig = Rig::new();
    let st = rig.storage();
    mount(&st);
    let mut ctrl = vec![0u8; 12 * 20];
    ctrl[20] = 0x01;
    rig.dev.nvs.set_blob("valvesCtrlCfg", "valvesCtrl", &ctrl);
    rig.dev.nvs.set_str("sysCfg", "stName", b"Legacy");
    load(&st);
    let ev = rig.host.with_code(EventCode::ImportDropped);
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].arg1, 1);
    assert_eq!(ev[0].arg2, i32::from(DROPPED_PI));
}

#[test]
fn load_nvs_not_usable_gives_defaults_with_reason_100() {
    let rig = Rig::new();
    rig.dev.nvs.knobs().fail_open.insert(NAMESPACE.to_string());
    let st = rig.storage();
    let l = load(&st);
    assert_eq!(l.src, LoadSource::DefaultsAfterError);
    assert_eq!(l.details.error_code, 100);
    assert_eq!(rig.shared.boot_load_details().error_code, 100);
}

// ---------------------------------------------------------------- blob buffers

fn blob_grants(dev: &Device) -> usize {
    dev.heap
        .state()
        .granted
        .iter()
        .filter(|&&a| a == CONFIG_BLOB_MAX + CONFIG_EXT_BLOB_MAX)
        .count()
}

#[test]
fn blobs_a_load_a_save_and_a_backup_each_take_the_buffers_for_the_moment() {
    let rig = Rig::new();
    let st = rig.storage();
    mount(&st);
    store_nvs(&rig.dev, &with_ext(b"L"));
    assert_eq!(blob_grants(&rig.dev), 0); // nothing at boot
    assert_eq!(load(&st).src, LoadSource::Stored);
    assert_eq!(blob_grants(&rig.dev), 1);
    st.service(); // the backup of the first boot
    assert_eq!(blob_grants(&rig.dev), 2);
    let mut path = [0u8; 16];
    assert_eq!(st.apply_config(&with_ext(b"S"), &mut path), Ok(()));
    assert_eq!(blob_grants(&rig.dev), 3);
    st.service();
    assert_eq!(blob_grants(&rig.dev), 4);
    assert_eq!(
        file(&rig.dev, BACKUP_BASE).unwrap(),
        blob_of(&with_ext(b"S"))
    );
}

#[test]
fn blobs_without_memory_the_boot_load_takes_the_defaults_reason_102() {
    let rig = Rig::new();
    let st = rig.storage();
    mount(&st);
    store_nvs(&rig.dev, &with_ext(b"L"));
    store_backup(&rig.dev, &with_ext(b"B"));
    rig.dev.heap.state().fail_all = true;
    let l = load(&st);
    rig.dev.heap.state().fail_all = false;
    assert_eq!(l.src, LoadSource::DefaultsAfterError);
    assert_eq!(l.details.error_code, 102);
    assert_eq!(l.cfg.station, Config::default().station);
    let ev = rig.host.with_code(EventCode::ConfigDefaults);
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].arg1, 102);
    // nothing read or written: NVS and the backup keep the stored config
    assert_eq!(
        rig.dev.nvs.get_blob(NAMESPACE, KEY_CONFIG),
        blob_of(&with_ext(b"L"))
    );
    assert_eq!(
        file(&rig.dev, BACKUP_BASE).unwrap(),
        blob_of(&with_ext(b"B"))
    );
    st.service();
    assert_eq!(
        file(&rig.dev, BACKUP_BASE).unwrap(),
        blob_of(&with_ext(b"B"))
    );
}

#[test]
fn blobs_without_memory_a_save_fails_like_nvs_nothing_changes() {
    let rig = Rig::new();
    let st = rig.storage();
    mount(&st);
    rig.shared.set_active_config(&named(b"A"));
    let rev = rig.shared.config_revision();
    rig.dev.heap.state().fail_all = true;
    let mut path = [0u8; 16];
    assert_eq!(st.apply_config(&with_ext(b"B"), &mut path), Err(3));
    rig.dev.heap.state().fail_all = false;
    assert_eq!(&path[..3], b"nvs");
    assert!(!rig.dev.nvs.has(NAMESPACE, KEY_CONFIG));
    assert!(!rig.dev.nvs.has(NAMESPACE, KEY_CONFIG_EXT));
    assert_eq!(rig.shared.config_revision(), rev);
    assert!(!rig.shared.config_saved_since_boot());
    let mut active = named(b"");
    rig.shared.get_config(&mut active);
    assert_eq!(active.station.as_slice(), b"A");
}

#[test]
fn blobs_without_memory_the_backup_stays_pending_for_the_next_pass() {
    let rig = Rig::new();
    let st = rig.storage();
    mount(&st);
    let mut path = [0u8; 16];
    assert_eq!(st.apply_config(&with_ext(b"S"), &mut path), Ok(()));
    rig.dev.heap.state().fail_all = true;
    st.service();
    st.service();
    rig.dev.heap.state().fail_all = false;
    assert!(!has_file(&rig.dev, BACKUP_BASE));
    assert!(!has_file(&rig.dev, "/sys/cfg.bak.tmp"));
    st.service();
    assert_eq!(
        file(&rig.dev, BACKUP_BASE).unwrap(),
        blob_of(&with_ext(b"S"))
    );
    assert_eq!(
        file(&rig.dev, BACKUP_EXT).unwrap(),
        ext_of(&with_ext(b"S"), &[])
    );
}

#[test]
fn blobs_without_memory_the_import_report_is_not_written() {
    let rig = Rig::new();
    let st = rig.storage();
    mount(&st);
    rig.dev.heap.state().fail_all = true;
    assert!(!st.write_import_report(&ImportReport::default()));
    rig.dev.heap.state().fail_all = false;
    assert!(!st.has_import_report());
    assert!(st.write_import_report(&ImportReport::default()));
    assert!(st.has_import_report());
}

// ---------------------------------------------------------------- save and backup

#[test]
fn save_cfgx_is_written_before_cfg_a_failing_cfgx_write_names_nvs() {
    let rig = Rig::new();
    let st = rig.storage();
    mount(&st);
    rig.shared.set_active_config(&named(b"A"));
    let rev = rig.shared.config_revision();
    rig.dev
        .nvs
        .knobs()
        .fail_set
        .insert(KEY_CONFIG_EXT.to_string());
    let mut path = [0u8; 16];
    assert_eq!(st.apply_config(&with_ext(b"B"), &mut path), Err(3));
    assert_eq!(&path[..3], b"nvs");
    assert!(!rig.dev.nvs.has(NAMESPACE, KEY_CONFIG));
    assert_eq!(rig.shared.config_revision(), rev);
    assert!(!rig.shared.config_saved_since_boot());
    st.service();
    assert!(!has_file(&rig.dev, BACKUP_BASE)); // nothing saved, nothing to back up
}

#[test]
fn save_a_failing_cfg_write_after_cfgx_fails_the_save() {
    let rig = Rig::new();
    let st = rig.storage();
    mount(&st);
    rig.dev.nvs.knobs().fail_set.insert(KEY_CONFIG.to_string());
    let mut path = [0u8; 16];
    assert_eq!(st.apply_config(&with_ext(b"B"), &mut path), Err(3));
    assert_eq!(&path[..3], b"nvs");
    assert_eq!(
        rig.dev.nvs.get_blob(NAMESPACE, KEY_CONFIG_EXT),
        ext_of(&with_ext(b"B"), &[])
    );
    assert!(!rig.dev.nvs.has(NAMESPACE, KEY_CONFIG));
}

#[test]
fn save_both_blobs_saved_the_backup_written_by_service() {
    let rig = Rig::new();
    let st = rig.storage();
    mount(&st);
    let mut path = [0u8; 16];
    assert_eq!(st.apply_config(&with_ext(b"S"), &mut path), Ok(()));
    assert_eq!(
        rig.dev.nvs.get_blob(NAMESPACE, KEY_CONFIG),
        blob_of(&with_ext(b"S"))
    );
    assert_eq!(
        rig.dev.nvs.get_blob(NAMESPACE, KEY_CONFIG_EXT),
        ext_of(&with_ext(b"S"), &[])
    );
    assert!(!has_file(&rig.dev, BACKUP_BASE));
    st.service();
    assert_eq!(
        file(&rig.dev, BACKUP_BASE).unwrap(),
        blob_of(&with_ext(b"S"))
    );
    assert_eq!(
        file(&rig.dev, BACKUP_EXT).unwrap(),
        ext_of(&with_ext(b"S"), &[])
    );
    // C++: the two .tmp files got 512 B stdio buffers; no Rust form (no stdio layer): the two
    // write opens are counted.
    assert_eq!(rig.dev.fs.knobs().write_opens, 2);
}

#[test]
fn save_no_backup_while_a_network_trial_runs_written_after_it() {
    let rig = Rig::new();
    let st = rig.storage();
    mount(&st);
    store_backup(&rig.dev, &named(b"Old"));
    let mut path = [0u8; 16];
    assert_eq!(st.apply_config(&named(b"Trial"), &mut path), Ok(()));
    rig.host
        .trial
        .store(true, std::sync::atomic::Ordering::Relaxed);
    st.service();
    assert_eq!(
        file(&rig.dev, BACKUP_BASE).unwrap(),
        blob_of(&named(b"Old"))
    );
    rig.host
        .trial
        .store(false, std::sync::atomic::Ordering::Relaxed);
    st.service();
    assert_eq!(
        file(&rig.dev, BACKUP_BASE).unwrap(),
        blob_of(&named(b"Trial"))
    );
}

#[test]
fn save_a_failed_backup_rename_keeps_the_old_backup_removes_the_tmp_files() {
    let rig = Rig::new();
    let st = rig.storage();
    mount(&st);
    store_backup(&rig.dev, &named(b"Old"));
    let mut path = [0u8; 16];
    assert_eq!(st.apply_config(&named(b"New"), &mut path), Ok(()));
    rig.dev.fs.fail("rename", "/sys/cfg.bak.tmp", 1);
    st.service();
    assert_eq!(
        file(&rig.dev, BACKUP_BASE).unwrap(),
        blob_of(&named(b"Old"))
    );
    assert!(!has_file(&rig.dev, "/sys/cfg.bak.tmp"));
    assert!(!has_file(&rig.dev, "/sys/cfgx.bak.tmp"));
}

#[test]
fn save_a_failed_tmp_write_renames_nothing() {
    let rig = Rig::new();
    let st = rig.storage();
    mount(&st);
    store_backup(&rig.dev, &named(b"Old"));
    let mut path = [0u8; 16];
    assert_eq!(st.apply_config(&named(b"New"), &mut path), Ok(()));
    rig.dev.fs.fail("write", "/sys/cfg.bak.tmp", 1);
    st.service();
    assert!(renames(&rig.dev).is_empty());
    assert_eq!(
        file(&rig.dev, BACKUP_EXT).unwrap(),
        ext_of(&named(b"Old"), &[])
    );
    assert!(!has_file(&rig.dev, "/sys/cfgx.bak.tmp"));
}

#[test]
fn save_a_failed_rename_of_the_ext_keeps_its_tmp_file_a_load_pairs_it_with_the_new_base() {
    let mut rig = Rig::new();
    let mut n = named(b"New");
    n.failsafe.timeout_min = 30;
    {
        let st = rig.storage();
        mount(&st);
        store_backup(&rig.dev, &with_ext(b"Old")); // failsafe timeout 90
        let mut path = [0u8; 16];
        assert_eq!(st.apply_config(&n, &mut path), Ok(()));
        rig.dev.fs.fail("rename", "/sys/cfgx.bak.tmp", 1);
        st.service();
        assert_eq!(file(&rig.dev, BACKUP_BASE).unwrap(), blob_of(&n));
        assert_eq!(
            file(&rig.dev, BACKUP_EXT).unwrap(),
            ext_of(&with_ext(b"Old"), &[])
        );
        assert_eq!(
            file(&rig.dev, "/sys/cfgx.bak.tmp").unwrap(),
            ext_of(&n, &[])
        );
        assert!(!has_file(&rig.dev, "/sys/cfg.bak.tmp"));
    }
    // NVS lost: the next boot takes the pair of the last save from the backup
    rig.reboot(Reset::PowerOn);
    rig.dev
        .nvs
        .set_blob(NAMESPACE, KEY_CONFIG, &corrupt(blob_of(&n)));
    let st = rig.storage();
    mount(&st);
    let l = load(&st);
    assert_eq!(l.src, LoadSource::Backup);
    assert_eq!(l.cfg.station.as_slice(), b"New");
    assert_eq!(l.cfg.failsafe.timeout_min, 30);
    assert_eq!(
        rig.dev.nvs.get_blob(NAMESPACE, KEY_CONFIG_EXT),
        ext_of(&n, &[])
    );
}

#[test]
fn load_a_backup_cut_between_its_renames_uses_cfgx_bak_tmp_one_cut_before_them_the_old_pair() {
    let mut rig = Rig::new();
    let mut n = named(b"New");
    n.failsafe.timeout_min = 30;
    {
        let st = rig.storage();
        mount(&st);
        // cut after the rename of the base
        rig.dev.fs.put(BACKUP_BASE, &blob_of(&n));
        rig.dev.fs.put(BACKUP_EXT, &ext_of(&with_ext(b"Old"), &[]));
        rig.dev.fs.put("/sys/cfgx.bak.tmp", &ext_of(&n, &[]));
        rig.dev
            .nvs
            .set_blob(NAMESPACE, KEY_CONFIG, &corrupt(blob_of(&n)));
        let l = load(&st);
        assert_eq!(l.src, LoadSource::Backup);
        assert_eq!(l.cfg.station.as_slice(), b"New");
        assert_eq!(l.cfg.failsafe.timeout_min, 30);
    }
    rig.reboot(Reset::PowerOn);
    // cut before it: both tmp files next to the old pair
    store_backup(&rig.dev, &with_ext(b"Old"));
    rig.dev.fs.put("/sys/cfg.bak.tmp", &blob_of(&n));
    rig.dev
        .nvs
        .set_blob(NAMESPACE, KEY_CONFIG, &corrupt(blob_of(&n)));
    let st = rig.storage();
    mount(&st);
    let l = load(&st);
    assert_eq!(l.src, LoadSource::Backup);
    assert_eq!(l.cfg.station.as_slice(), b"Old");
    assert_eq!(l.cfg.failsafe.timeout_min, 90);
}

#[test]
fn save_the_next_backup_first_finishes_one_cut_between_its_renames() {
    let rig = Rig::new();
    let st = rig.storage();
    mount(&st);
    let mut first = named(b"First");
    first.failsafe.timeout_min = 30;
    rig.dev.fs.put(BACKUP_BASE, &blob_of(&first));
    rig.dev.fs.put(BACKUP_EXT, &ext_of(&with_ext(b"Old"), &[]));
    rig.dev.fs.put("/sys/cfgx.bak.tmp", &ext_of(&first, &[]));
    let mut second = named(b"Second");
    second.failsafe.timeout_min = 40;
    let mut path = [0u8; 16];
    assert_eq!(st.apply_config(&second, &mut path), Ok(()));
    // the rename that finishes the cut backup fails: nothing else is written
    rig.dev.fs.fail("rename", "/sys/cfgx.bak.tmp", 1);
    st.service();
    assert_eq!(file(&rig.dev, BACKUP_BASE).unwrap(), blob_of(&first));
    assert_eq!(
        file(&rig.dev, "/sys/cfgx.bak.tmp").unwrap(),
        ext_of(&first, &[])
    );
    assert!(!has_file(&rig.dev, "/sys/cfg.bak.tmp"));
    // the next save finishes it, then writes its own pair
    assert_eq!(st.apply_config(&second, &mut path), Ok(()));
    rig.dev.journal.clear();
    st.service();
    assert_eq!(
        renames(&rig.dev),
        vec![
            rename("/sys/cfgx.bak.tmp", BACKUP_EXT),
            rename("/sys/cfg.bak.tmp", BACKUP_BASE),
            rename("/sys/cfgx.bak.tmp", BACKUP_EXT)
        ]
    );
    assert_eq!(file(&rig.dev, BACKUP_BASE).unwrap(), blob_of(&second));
    assert_eq!(file(&rig.dev, BACKUP_EXT).unwrap(), ext_of(&second, &[]));
    assert!(!has_file(&rig.dev, "/sys/cfgx.bak.tmp"));
}

#[test]
fn factory_reset_the_backup_and_report_files_go_the_latch_stays() {
    let rig = Rig::new();
    let st = rig.storage();
    mount(&st);
    store_backup(&rig.dev, &named(b"B"));
    rig.dev.fs.put(IMPORT_REPORT_FILE, b"{}");
    rig.dev.fs.put("/sys/other", b"x");
    rig.dev.nvs.set_u8(NAMESPACE, KEY_FACTORY_LATCH, 1);
    let mut path = [0u8; 16];
    assert_eq!(st.apply_config(&named(b"Pending"), &mut path), Ok(()));
    assert!(st.factory_reset());
    assert!(!has_file(&rig.dev, BACKUP_BASE));
    assert!(!has_file(&rig.dev, BACKUP_EXT));
    assert!(!has_file(&rig.dev, IMPORT_REPORT_FILE));
    assert!(has_file(&rig.dev, "/sys/other"));
    assert_eq!(nvs_int(&rig.dev, NAMESPACE, KEY_FACTORY_LATCH), 1);
    // The save before the reset is not backed up afterwards.
    st.service();
    assert!(!has_file(&rig.dev, BACKUP_BASE));
}

#[test]
fn begin_fs_sys_is_created() {
    let rig = Rig::new();
    let st = rig.storage();
    mount(&st);
    assert!(is_dir(&rig.dev, "/sys"));
}

// ---------------------------------------------------------------- import report

#[test]
fn import_report_written_from_the_active_config_dismissed_once() {
    let rig = Rig::new();
    let st = rig.storage();
    mount(&st);
    let mut c = named(b"R");
    copy_string(&mut c.mqtt.root_topic, b"Root");
    rig.shared.set_active_config(&c);
    assert!(!st.has_import_report());
    assert!(!st.dismiss_import_report());
    let r = ImportReport {
        imported: 4,
        ..ImportReport::default()
    };
    assert!(st.write_import_report(&r));
    let mut expect = vec![0u8; 512];
    let mut jw = JsonWriter::new(&mut expect);
    assert!(write_import_report_json(&mut jw, &r, &c));
    assert_eq!(file(&rig.dev, IMPORT_REPORT_FILE).unwrap(), jw.as_bytes());
    assert!(st.has_import_report());
    assert!(st.dismiss_import_report());
    assert!(!st.has_import_report());
    assert!(!st.dismiss_import_report());
}

#[test]
fn import_report_nothing_without_a_file_system() {
    let rig = Rig::new();
    rig.dev.fs.put(IMPORT_REPORT_FILE, b"{}");
    let st = rig.storage();
    assert!(!st.write_import_report(&ImportReport::default()));
    assert!(!st.has_import_report());
    assert!(!st.dismiss_import_report());
}

#[test]
fn import_report_a_failed_write_returns_false() {
    let rig = Rig::new();
    let st = rig.storage();
    mount(&st);
    rig.dev.fs.fail("open", IMPORT_REPORT_FILE, 1);
    assert!(!st.write_import_report(&ImportReport::default()));
}

// ---------------------------------------------------------------- files

fn entries(n: usize) -> Vec<FileEntry> {
    vec![FileEntry::default(); n]
}

#[test]
fn list_files_the_root_and_one_level_below_sizes_and_paths() {
    let rig = Rig::new();
    let st = rig.storage();
    mount(&st);
    rig.dev.fs.put("/a.bin", b"12345");
    rig.dev.fs.put("/stm/fw.bin", b"123");
    rig.dev.fs.put("/sys/deep/x", b"1"); // two levels down: not listed
    rig.dev.fs.put("/zz", b"");
    let mut out = entries(8);
    let mut truncated = true;
    let n = st.list_files(&mut out, &mut truncated);
    assert!(!truncated);
    let paths: Vec<&[u8]> = out[..n].iter().map(|e| e.path.as_slice()).collect();
    assert_eq!(paths, vec![b"/a.bin".as_slice(), b"/stm/fw.bin", b"/zz"]);
    assert_eq!(out[0].size, 5);
    assert_eq!(out[1].size, 3);
    assert_eq!(out[2].size, 0);
}

#[test]
fn list_files_70_files_give_64_and_truncated() {
    let rig = Rig::new();
    let st = rig.storage();
    mount(&st);
    for i in 0..35 {
        rig.dev.fs.put(&format!("/f{}", 100 + i), b"x");
        rig.dev.fs.put(&format!("/stm/g{}.part", 100 + i), b"y");
    }
    let mut out = entries(64);
    let mut truncated = false;
    assert_eq!(st.list_files(&mut out, &mut truncated), 64);
    assert!(truncated);
    // Exactly full: not truncated.
    let mut all = entries(70);
    assert_eq!(st.list_files(&mut all, &mut truncated), 70);
    assert!(!truncated);
}

#[test]
fn list_files_a_full_list_in_the_subdirectory_is_truncated_too() {
    let rig = Rig::new();
    let st = rig.storage();
    mount(&st);
    rig.dev.fs.put("/stm/a.part", b"1");
    rig.dev.fs.put("/stm/b.part", b"1");
    let mut out = entries(1);
    let mut truncated = false;
    assert_eq!(st.list_files(&mut out, &mut truncated), 1);
    assert!(truncated);
    assert_eq!(out[0].path.as_slice(), b"/stm/a.part");
}

#[test]
fn list_files_nothing_without_a_file_system() {
    let rig = Rig::new();
    let st = rig.storage();
    let mut out = entries(4);
    let mut truncated = true;
    assert_eq!(st.list_files(&mut out, &mut truncated), 0);
    assert!(!truncated);
}

#[test]
fn delete_file_a_deletable_file_is_removed_and_logged() {
    let rig = Rig::new();
    let st = rig.storage();
    mount(&st);
    rig.dev.fs.put("/x.bin", &[b'x'; 2049]);
    assert_eq!(st.delete_file(b"/x.bin"), FileResult::Ok);
    assert!(!has_file(&rig.dev, "/x.bin"));
    let ev = rig.host.with_code(EventCode::FilesRemoved);
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].arg1, 1);
    assert_eq!(ev[0].arg2, 3);
    assert_eq!(ev[0].text.as_slice(), b"/x.bin");
}

#[test]
fn delete_file_every_refusal() {
    let rig = Rig::new();
    let st = rig.storage();
    mount(&st);
    rig.dev.fs.put(BACKUP_BASE, b"b");
    rig.dev.fs.put("/stm/fw.bin", b"i");
    rig.dev.fs.put("/y.txt", b"y");
    // C++ deleteFile(nullptr): no Rust form; the empty path is refused the same way
    assert_eq!(st.delete_file(b""), FileResult::BadPath);
    assert_eq!(st.delete_file(b"//x"), FileResult::BadPath);
    assert_eq!(st.delete_file(b"/sys/cfg.bak"), FileResult::Protected);
    assert_eq!(st.delete_file(b"/stm/fw.bin"), FileResult::Protected);
    assert_eq!(st.delete_file(b"/missing.bin"), FileResult::NotFound);
    assert_eq!(st.delete_file(b"/stm"), FileResult::BadPath); // a directory
    rig.dev.fs.fail("remove", "/y.txt", 1);
    assert_eq!(st.delete_file(b"/y.txt"), FileResult::Io);
    rig.dev.fs.fail("open", "/y.txt", 1);
    assert_eq!(st.delete_file(b"/y.txt"), FileResult::Io);
    assert!(has_file(&rig.dev, BACKUP_BASE));
    assert!(has_file(&rig.dev, "/stm/fw.bin"));
    assert!(is_dir(&rig.dev, "/stm"));
    assert!(has_file(&rig.dev, "/y.txt"));
    assert!(!rig.host.has(EventCode::FilesRemoved));
}

#[test]
fn delete_file_without_a_file_system_io() {
    let rig = Rig::new();
    rig.dev.fs.put("/y.txt", b"y");
    let st = rig.storage();
    assert_eq!(st.delete_file(b"/y.txt"), FileResult::Io);
}

#[test]
fn remove_legacy_images_more_images_than_one_batch_directories_and_others_kept() {
    let rig = Rig::new();
    let st = rig.storage();
    mount(&st);
    for i in 0..10 {
        rig.dev.fs.put(&format!("/img{i}.Bin"), &[b'i'; 1024]);
    }
    rig.dev.fs.put("/keep.txt", b"k");
    assert!(rig.dev.fs.mkdir("/dir.bin"));
    let mut kib = 99;
    assert_eq!(st.remove_legacy_images(&mut kib), 10);
    assert_eq!(kib, 10);
    assert!(has_file(&rig.dev, "/keep.txt"));
    assert!(is_dir(&rig.dev, "/dir.bin"));
    for i in 0..10 {
        assert!(!has_file(&rig.dev, &format!("/img{i}.Bin")));
    }
}

#[test]
fn remove_legacy_images_a_file_that_cannot_be_removed_ends_the_run() {
    let rig = Rig::new();
    let st = rig.storage();
    mount(&st);
    for i in 0..8 {
        rig.dev.fs.put(&format!("/i{i}.bin"), b"x");
    }
    rig.dev.fs.fail("remove", "/i0.bin", 100);
    let mut kib = 0;
    assert_eq!(st.remove_legacy_images(&mut kib), 7);
    assert_eq!(kib, 1);
    assert!(has_file(&rig.dev, "/i0.bin"));
}

#[test]
fn remove_legacy_images_nothing_without_a_file_system() {
    let rig = Rig::new();
    rig.dev.fs.put("/x.bin", b"x");
    let st = rig.storage();
    let mut kib = 5;
    assert_eq!(st.remove_legacy_images(&mut kib), 0);
    assert_eq!(kib, 0);
}

// ---------------------------------------------------------------- Rust (new): boots

#[test]
fn boots_a_saved_config_is_loaded_by_the_next_boot_its_backup_kept() {
    let mut rig = Rig::new();
    {
        let st = rig.storage();
        mount(&st);
        let mut path = [0u8; 16];
        assert_eq!(st.apply_config(&with_ext(b"Kept"), &mut path), Ok(()));
        st.service();
    }
    rig.reboot(Reset::Software);
    let st = rig.storage();
    mount(&st);
    let l = load(&st);
    assert_eq!(l.src, LoadSource::Stored);
    assert_eq!(l.cfg.station.as_slice(), b"Kept");
    assert_eq!(l.cfg.valves[1].topic.as_slice(), b"Bad/WC");
    assert!(rig.host.events().is_empty());
    rig.shared.set_active_config(&l.cfg);
    let writes = rig.dev.fs.knobs().write_opens;
    st.service(); // the backup equals NVS: nothing to write
    assert_eq!(rig.dev.fs.knobs().write_opens, writes);
}

#[test]
fn boots_the_legacy_import_runs_once_and_leaves_the_legacy_keys() {
    let mut rig = Rig::new();
    rig.dev.nvs.set_str("sysCfg", "stName", b"Legacy");
    {
        let st = rig.storage();
        mount(&st);
        assert_eq!(load(&st).src, LoadSource::Imported);
    }
    rig.reboot(Reset::Software);
    let st = rig.storage();
    mount(&st);
    let l = load(&st);
    assert_eq!(l.src, LoadSource::Stored);
    assert_eq!(l.cfg.station.as_slice(), b"Legacy");
    assert!(rig.dev.nvs.has("sysCfg", "stName")); // a downgrade still finds it
    assert!(rig.host.events().is_empty());
}

#[test]
fn boots_a_failed_import_save_is_retried_by_the_next_boot() {
    let mut rig = Rig::new();
    rig.dev.nvs.set_str("sysCfg", "stName", b"Again");
    rig.dev.nvs.knobs().fail_set.insert(KEY_CONFIG.to_string());
    {
        let st = rig.storage();
        mount(&st);
        assert_eq!(load(&st).src, LoadSource::Imported);
        assert!(!rig.dev.nvs.has(NAMESPACE, KEY_IMPORTED));
    }
    rig.reboot(Reset::Software);
    let st = rig.storage();
    mount(&st);
    let l = load(&st);
    assert_eq!(l.src, LoadSource::Imported);
    assert_eq!(l.cfg.station.as_slice(), b"Again");
    assert_eq!(nvs_int(&rig.dev, NAMESPACE, KEY_IMPORTED), 1);
    assert!(rig.dev.nvs.has(NAMESPACE, KEY_CONFIG));
}

#[test]
fn load_the_same_repair_in_the_blob_and_after_the_ext_records_is_one_bit() {
    let rig = Rig::new();
    let st = rig.storage();
    mount(&st);
    let mut base = named(b"Ha");
    copy_string(&mut base.valves[0].name, b"A.b");
    copy_string(&mut base.valves[1].name, b"A,b"); // the HA id of valve 1: repaired by the decode
    let mut ext = named(b"Ha");
    copy_string(&mut ext.valves[2].topic, b"C.d");
    copy_string(&mut ext.valves[3].topic, b"C,d"); // the HA id of valve 3: repaired after cfgx
    rig.dev.nvs.set_blob(NAMESPACE, KEY_CONFIG, &blob_of(&base));
    rig.dev
        .nvs
        .set_blob(NAMESPACE, KEY_CONFIG_EXT, &ext_of(&ext, &[]));
    let l = load(&st);
    assert_eq!(l.src, LoadSource::Stored);
    assert!(l.cfg.valves[1].name.is_empty());
    assert!(l.cfg.valves[3].topic.is_empty());
    let ev = rig.host.with_code(EventCode::ConfigRepaired);
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].arg1, REPAIR_HA_IDS as i32);
    assert_eq!(ev[0].arg2, 2);
    assert_eq!(ev[0].text.as_slice(), b"valves.2.name");
}

#[test]
fn list_files_a_path_of_95_bytes_is_listed_a_longer_one_left_out() {
    let rig = Rig::new();
    let st = rig.storage();
    mount(&st);
    let root95 = format!("/{}", "r".repeat(94));
    let sub95 = format!("/stm/{}", "t".repeat(90));
    rig.dev.fs.put(&root95, b"1");
    rig.dev.fs.put(&format!("/{}", "s".repeat(95)), b"2");
    rig.dev.fs.put(&sub95, b"3");
    rig.dev.fs.put(&format!("/stm/{}", "u".repeat(91)), b"4");
    let mut out = entries(8);
    let mut truncated = true;
    let n = st.list_files(&mut out, &mut truncated);
    assert!(!truncated);
    let paths: Vec<&[u8]> = out[..n].iter().map(|e| e.path.as_slice()).collect();
    assert_eq!(paths, vec![root95.as_bytes(), sub95.as_bytes()]);
}

#[test]
fn delete_file_a_directory_below_the_root_is_a_bad_path() {
    let rig = Rig::new();
    let st = rig.storage();
    mount(&st);
    assert!(rig.dev.fs.mkdir("/stm/sub"));
    rig.dev.fs.put("/stm/subway", b"x");
    assert_eq!(st.delete_file(b"/stm/sub"), FileResult::BadPath);
    assert!(is_dir(&rig.dev, "/stm/sub"));
    assert_eq!(st.delete_file(b"/stm/subway"), FileResult::Ok);
}

#[test]
fn shared_data_is_reached_through_storage_and_read_in_place() {
    let rig = Rig::new();
    let st = rig.storage();
    assert!(core::ptr::eq(st.shared(), &rig.shared));
    rig.shared.set_active_config(&with_ext(b"W"));
    let (station, timeout) = st
        .shared()
        .with_config(|c| (c.station.to_vec(), c.failsafe.timeout_min));
    assert_eq!((station, timeout), (b"W".to_vec(), 90));
}

#[test]
fn remove_legacy_images_without_memory_for_the_batch_removes_nothing() {
    let rig = Rig::new();
    let st = rig.storage();
    mount(&st);
    rig.dev.fs.put("/x.bin", b"x");
    rig.dev.heap.state().fail_all = true;
    let mut kib = 7;
    assert_eq!(st.remove_legacy_images(&mut kib), 0);
    assert_eq!(kib, 0);
    assert!(has_file(&rig.dev, "/x.bin"));
    assert_eq!(rig.dev.heap.state().refused.len(), 1);
}

#[test]
fn heap_scripts_reach_the_blob_buffer_of_one_load() {
    // the first grant of a load is its blob buffer: refused, the load takes the defaults
    let rig = Rig::new();
    let st = rig.storage();
    store_nvs(&rig.dev, &with_ext(b"L"));
    rig.dev.heap.state().next = VecDeque::from([false]);
    assert_eq!(load(&st).src, LoadSource::DefaultsAfterError);
    assert_eq!(
        rig.dev.heap.state().refused,
        vec![CONFIG_BLOB_MAX + CONFIG_EXT_BLOB_MAX]
    );
}
