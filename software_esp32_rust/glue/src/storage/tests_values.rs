//! `test_storage_values.cpp`: the small NVS values at boot and without NVS, flags, targets and
//! the network trial record, the config revision and apply refusals, the load table edges (blob
//! sizes, backup files, the comparison), the legacy NVS reader (integer widths, strings, blobs)
//! and the import events; then the new Rust cases of the reader.
#![allow(clippy::large_stack_frames, clippy::large_stack_arrays)]

use super::rig::*;
use super::*;
use crate::port::{Fs, Nvs, NvsNamespace};
use crate::testkit::nvs::NvsType;
use crate::testkit::{Device, Reset};
use vdm_esp_core::config::DecodeResult;
use vdm_esp_core::file_manager::FS_PATH_MAX;
use vdm_esp_core::legacy_import::{DROPPED_MESSENGER, LEGACY_TEMPS_BLOB};

/// A config with an ext key away from its default.
fn with_ext(station: &[u8]) -> Box<Config> {
    let mut c = named(station);
    c.failsafe.timeout_min = 90;
    c
}

/// A damaged cfg in NVS and a usable cfg.bak: the backup is loaded with this cfgx.bak.
fn restore_with_ext_backup(rig: &Rig, st: &TestStorage<'_>, ext_bak: &[u8]) -> Load {
    mount(st);
    rig.dev.fs.put(BACKUP_BASE, &blob_of(&with_ext(b"Bak")));
    rig.dev.fs.put(BACKUP_EXT, ext_bak);
    rig.dev
        .nvs
        .set_blob(NAMESPACE, KEY_CONFIG, &corrupt(blob_of(&named(b"Bad"))));
    rig.dev
        .nvs
        .set_blob(NAMESPACE, KEY_CONFIG_EXT, &ext_of(&named(b"Bad"), &[]));
    load(st)
}

fn erase_namespace(dev: &Device, ns: &str) {
    let mut h = dev.nvs.open(ns, true).unwrap();
    assert!(h.erase_all());
}

// ---------------------------------------------------------------- NVS values

#[test]
fn values_a_fresh_device() {
    let rig = Rig::new();
    let st = rig.storage();
    assert_eq!(rig.shared.config_revision(), 0);
    assert_eq!(rig.shared.boot_count(), 0);
    assert!(!st.ota_stm_required());
    assert!(!st.factory_latched());
    assert_eq!(st.ha_layout(), 0);
    assert_eq!(st.load_calib_slot(), 0);
    assert_eq!(st.load_last_calib(), 0);
    assert_eq!(st.increment_boot_count(), 1);
    assert_eq!(nvs_int(&rig.dev, NAMESPACE, KEY_BOOT_COUNT), 1);
}

#[test]
fn values_keep_the_nvs_types_of_the_cpp_firmware_both_ways() {
    // C++ Preferences: putULong (u32) boots and calSlot, putLong64 (i64) lastCal, putUChar (u8)
    // haDrop, haLayout, frLatch, otaStm and imported. NVS reads are typed: a value of another
    // type reads as missing, so a switch between the firmwares would lose it.
    let rig = Rig::new();
    let nvs = &rig.dev.nvs;
    nvs.set_u32(NAMESPACE, KEY_BOOT_COUNT, 41);
    nvs.set_u32(NAMESPACE, KEY_CALIB_SLOT, 20_260_923);
    nvs.set_i64(NAMESPACE, KEY_LAST_CALIB, 1_790_136_000);
    nvs.set_u8(NAMESPACE, KEY_HA_CLEANUP, 1);
    nvs.set_u8(NAMESPACE, KEY_HA_LAYOUT, 2);
    nvs.set_u8(NAMESPACE, KEY_FACTORY_LATCH, 1);
    nvs.set_u8(NAMESPACE, KEY_OTA_STM, 1);
    let st = rig.storage();
    assert_eq!(st.increment_boot_count(), 42);
    assert_eq!(st.load_calib_slot(), 20_260_923);
    assert_eq!(st.load_last_calib(), 1_790_136_000);
    assert!(st.ha_cleanup_done());
    assert_eq!(st.ha_layout(), 2);
    assert!(st.factory_latched());
    assert!(st.ota_stm_required());
    // what the Rust writes, the C++ reads with the same types
    st.save_calib_slot(20_261_001);
    st.save_last_calib(1_790_200_000);
    st.set_ha_layout(3);
    let types = [
        (KEY_BOOT_COUNT, NvsType::U32),
        (KEY_CALIB_SLOT, NvsType::U32),
        (KEY_LAST_CALIB, NvsType::I64),
        (KEY_HA_CLEANUP, NvsType::U8),
        (KEY_HA_LAYOUT, NvsType::U8),
        (KEY_FACTORY_LATCH, NvsType::U8),
        (KEY_OTA_STM, NvsType::U8),
    ];
    let check = |types: &[(&str, NvsType)]| {
        for (key, ty) in types {
            let e = nvs
                .get(NAMESPACE, key)
                .unwrap_or_else(|| panic!("{key} missing"));
            assert_eq!(e.ty, *ty, "{key}");
        }
    };
    check(&types);
    assert_eq!(nvs_int(&rig.dev, NAMESPACE, KEY_BOOT_COUNT), 42);
    assert_eq!(nvs_int(&rig.dev, NAMESPACE, KEY_CALIB_SLOT), 20_261_001);
    assert_eq!(nvs_int(&rig.dev, NAMESPACE, KEY_LAST_CALIB), 1_790_200_000);
    assert_eq!(nvs_int(&rig.dev, NAMESPACE, KEY_HA_LAYOUT), 3);
    // written fresh on an erased namespace
    assert!(st.factory_reset());
    st.set_ha_cleanup_done();
    st.set_ota_stm_required(true);
    st.increment_boot_count();
    check(&[
        (KEY_IMPORTED, NvsType::U8),
        (KEY_FACTORY_LATCH, NvsType::U8),
        (KEY_HA_CLEANUP, NvsType::U8),
        (KEY_OTA_STM, NvsType::U8),
        (KEY_BOOT_COUNT, NvsType::U32),
    ]);
}

#[test]
fn values_nvs_that_cannot_be_opened_reads_as_zero_and_refuses_the_reset() {
    let rig = Rig::new();
    let nvs = &rig.dev.nvs;
    nvs.set_u32(NAMESPACE, KEY_BOOT_COUNT, 7);
    nvs.set_u32(NAMESPACE, KEY_CALIB_SLOT, 20_260_101);
    nvs.set_i64(NAMESPACE, KEY_LAST_CALIB, 1_790_136_000);
    nvs.set_u8(NAMESPACE, KEY_HA_LAYOUT, 2);
    nvs.set_blob(NAMESPACE, KEY_TARGETS, &[1, 2, 3]);
    nvs.knobs().fail_open.insert(NAMESPACE.to_string());
    let st = rig.storage();
    assert_eq!(st.increment_boot_count(), 0);
    assert_eq!(st.load_calib_slot(), 0);
    assert_eq!(st.load_last_calib(), 0);
    assert_eq!(st.ha_layout(), 0);
    let mut out = [0u8; 8];
    assert_eq!(st.load_targets(&mut out), 0);
    assert!(!st.factory_reset());
    assert_eq!(nvs_int(&rig.dev, NAMESPACE, KEY_BOOT_COUNT), 7);
}

#[test]
fn values_flags_targets_the_network_trial_record_the_calibration_time() {
    let rig = Rig::new();
    let st = rig.storage();
    st.set_factory_latched(true);
    assert!(st.factory_latched());
    st.set_factory_latched(false);
    assert!(!st.factory_latched());
    assert!(!rig.dev.nvs.has(NAMESPACE, KEY_FACTORY_LATCH));
    let t = [7u8, 8, 9];
    assert!(!st.save_targets(&t[..0]));
    assert!(!rig.dev.nvs.has(NAMESPACE, KEY_TARGETS));
    assert!(st.save_targets(&t[..1]));
    let mut out = [0u8; 8];
    assert_eq!(st.load_targets(&mut out), 1);
    assert_eq!(out[0], 7);
    // C++ loadTargets(nullptr, 8): no Rust form; the empty output takes nothing
    assert_eq!(st.load_targets(&mut []), 0);
    assert!(st.save_net_trial_blob(&t));
    assert_eq!(
        rig.dev.nvs.get_blob(NAMESPACE, KEY_NET_TRIAL),
        vec![7, 8, 9]
    );
    out = [0; 8];
    assert_eq!(st.load_net_trial_blob(&mut out), 3);
    assert_eq!(out[2], 9);
    st.clear_net_trial();
    assert!(!rig.dev.nvs.has(NAMESPACE, KEY_NET_TRIAL));
    assert_eq!(st.load_net_trial_blob(&mut out), 0);
    st.save_last_calib(0);
    assert!(!rig.dev.nvs.has(NAMESPACE, KEY_LAST_CALIB));
    st.save_last_calib(1);
    assert_eq!(st.load_last_calib(), 1);
}

#[test]
fn set_active_config_every_publish_adds_exactly_one_to_the_revision() {
    let rig = Rig::new();
    rig.shared.set_active_config(&named(b"A"));
    assert_eq!(rig.shared.config_revision(), 1);
    rig.shared.set_active_config(&named(b"B"));
    assert_eq!(rig.shared.config_revision(), 2);
}

#[test]
fn apply_config_an_invalid_config_is_refused_with_its_key_nothing_saved() {
    let rig = Rig::new();
    let st = rig.storage();
    let mut c = named(b"Inv");
    c.calib.hour = 24;
    let mut path = [0u8; 32];
    assert_eq!(st.apply_config(&c, &mut path), Err(10));
    assert_eq!(&path[..10], b"calib.hour");
    assert!(!rig.dev.nvs.has(NAMESPACE, KEY_CONFIG));
    assert_eq!(rig.shared.config_revision(), 0);
    assert!(!rig.shared.config_saved_since_boot());
}

#[test]
fn apply_config_a_failed_write_without_a_path_buffer() {
    let rig = Rig::new();
    rig.dev.nvs.knobs().fail_set.insert(KEY_CONFIG.to_string());
    let st = rig.storage();
    // C++ applyConfig(c, nullptr, 16): no Rust form; the empty path buffer gets nothing
    assert_eq!(st.apply_config(&named(b"N"), &mut []), Err(0));
    assert_eq!(rig.shared.config_revision(), 0);
}

#[test]
fn factory_reset_a_failed_erase_fails_the_reset() {
    let rig = Rig::new();
    rig.dev.nvs.set_u32(NAMESPACE, KEY_BOOT_COUNT, 3);
    rig.dev.nvs.knobs().fail_erase = true;
    let st = rig.storage();
    assert!(!st.factory_reset());
    assert_eq!(nvs_int(&rig.dev, NAMESPACE, KEY_IMPORTED), 1);
}

#[test]
fn factory_reset_the_kept_unknown_cfgx_records_are_forgotten() {
    let rig = Rig::new();
    let unknown = [200u8, 0, 2, 7, 9];
    rig.dev
        .nvs
        .set_blob(NAMESPACE, KEY_CONFIG, &blob_of(&named(b"K")));
    rig.dev
        .nvs
        .set_blob(NAMESPACE, KEY_CONFIG_EXT, &ext_of(&named(b"K"), &unknown));
    let st = rig.storage();
    assert_eq!(load(&st).src, LoadSource::Stored);
    assert!(st.factory_reset());
    let mut path = [0u8; 16];
    assert_eq!(st.apply_config(&named(b"After"), &mut path), Ok(()));
    assert_eq!(
        rig.dev.nvs.get_blob(NAMESPACE, KEY_CONFIG_EXT),
        ext_of(&named(b"After"), &[])
    );
}

// ---------------------------------------------------------------- load table edges

#[test]
fn load_defaults_keep_no_unknown_cfgx_records_for_the_next_save() {
    let rig = Rig::new();
    rig.dev.nvs.set_u8(NAMESPACE, KEY_IMPORTED, 1);
    let st = rig.storage();
    assert_eq!(load(&st).src, LoadSource::Defaults);
    let mut path = [0u8; 16];
    assert_eq!(st.apply_config(&named(b"K"), &mut path), Ok(()));
    assert_eq!(
        rig.dev.nvs.get_blob(NAMESPACE, KEY_CONFIG_EXT),
        ext_of(&named(b"K"), &[])
    );
}

#[test]
fn load_a_one_byte_cfg_is_a_damaged_blob() {
    let rig = Rig::new();
    rig.dev.nvs.set_blob(NAMESPACE, KEY_CONFIG, &[1]);
    let st = rig.storage();
    let l = load(&st);
    assert_eq!(l.src, LoadSource::DefaultsAfterError);
    assert_eq!(l.details.error_code, DecodeResult::TooShort as u8);
}

#[test]
fn load_a_damaged_cfg_of_the_maximum_size_is_decoded_not_unreadable() {
    let rig = Rig::new();
    rig.dev
        .nvs
        .set_blob(NAMESPACE, KEY_CONFIG, &vec![0x55u8; CONFIG_BLOB_MAX]);
    let st = rig.storage();
    let l = load(&st);
    assert_eq!(l.src, LoadSource::DefaultsAfterError);
    assert_eq!(l.details.error_code, DecodeResult::BadMagic as u8);
}

#[test]
fn load_without_cfgx_the_backup_follows_with_a_damaged_cfgx_it_waits() {
    let mut rig = Rig::new();
    {
        let st = rig.storage();
        mount(&st);
        rig.dev
            .nvs
            .set_blob(NAMESPACE, KEY_CONFIG, &blob_of(&named(b"NoExt")));
        let l = load(&st);
        assert_eq!(l.src, LoadSource::Stored);
        rig.shared.set_active_config(&l.cfg);
        st.service();
        assert_eq!(
            file(&rig.dev, BACKUP_BASE).unwrap(),
            blob_of(&named(b"NoExt"))
        );
        assert!(rig.dev.fs.remove(BACKUP_BASE));
        assert!(rig.dev.fs.remove(BACKUP_EXT));
        rig.dev.nvs.set_blob(
            NAMESPACE,
            KEY_CONFIG_EXT,
            &corrupt(ext_of(&with_ext(b"NoExt"), &[])),
        );
    }
    rig.reboot(Reset::Software);
    let st = rig.storage();
    mount(&st);
    let l = load(&st);
    assert_eq!(l.src, LoadSource::Stored);
    assert_eq!(l.details.info.ext, ExtResult::BadCrc);
    st.service();
    assert!(!has_file(&rig.dev, BACKUP_BASE));
}

#[test]
fn load_a_backup_that_differs_after_its_first_64_bytes_is_rewritten() {
    let rig = Rig::new();
    let st = rig.storage();
    mount(&st);
    let c = with_ext(b"Cmp");
    let base = blob_of(&c);
    assert!(base.len() > 128);
    let mut bak = base.clone();
    bak[100] ^= 0x01;
    rig.dev.nvs.set_blob(NAMESPACE, KEY_CONFIG, &base);
    rig.dev
        .nvs
        .set_blob(NAMESPACE, KEY_CONFIG_EXT, &ext_of(&c, &[]));
    rig.dev.fs.put(BACKUP_BASE, &bak);
    rig.dev.fs.put(BACKUP_EXT, &ext_of(&c, &[]));
    let l = load(&st);
    assert_eq!(l.src, LoadSource::Stored);
    rig.shared.set_active_config(&l.cfg);
    st.service();
    assert_eq!(file(&rig.dev, BACKUP_BASE).unwrap(), base);
}

#[test]
fn load_a_backup_file_that_cannot_be_opened_is_not_taken_as_equal() {
    let rig = Rig::new();
    let st = rig.storage();
    mount(&st);
    let c = with_ext(b"Open");
    rig.dev.nvs.set_blob(NAMESPACE, KEY_CONFIG, &blob_of(&c));
    rig.dev
        .nvs
        .set_blob(NAMESPACE, KEY_CONFIG_EXT, &ext_of(&c, &[]));
    rig.dev.fs.put(BACKUP_BASE, &blob_of(&c));
    rig.dev.fs.put(BACKUP_EXT, &ext_of(&c, &[]));
    rig.dev.fs.fail("open", BACKUP_BASE, 1);
    let l = load(&st);
    assert_eq!(l.src, LoadSource::Stored);
    rig.shared.set_active_config(&l.cfg);
    st.service();
    assert_eq!(rig.dev.journal.of("fs rename").len(), 2);
}

#[test]
fn load_a_cfgx_bak_that_cannot_be_opened_restores_without_cfgx() {
    let rig = Rig::new();
    rig.dev.fs.fail("open", BACKUP_EXT, 1);
    let st = rig.storage();
    let l = restore_with_ext_backup(&rig, &st, &ext_of(&with_ext(b"Bak"), &[]));
    assert_eq!(l.src, LoadSource::Backup);
    assert_eq!(l.cfg.failsafe.timeout_min, 60);
    assert!(!rig.dev.nvs.has(NAMESPACE, KEY_CONFIG_EXT));
}

#[test]
fn load_a_cfgx_bak_larger_than_a_cfgx_blob_restores_without_cfgx() {
    let rig = Rig::new();
    let st = rig.storage();
    let mut ext = ext_of(&with_ext(b"Bak"), &[]);
    ext.resize(CONFIG_EXT_BLOB_MAX + 1, 0);
    let l = restore_with_ext_backup(&rig, &st, &ext);
    assert_eq!(l.src, LoadSource::Backup);
    assert_eq!(l.cfg.failsafe.timeout_min, 60);
    assert!(!rig.dev.nvs.has(NAMESPACE, KEY_CONFIG_EXT));
}

#[test]
fn load_a_cfgx_bak_of_exactly_the_blob_maximum_is_used() {
    let rig = Rig::new();
    let st = rig.storage();
    let mut ext = ext_of(&with_ext(b"Bak"), &[]);
    ext.resize(CONFIG_EXT_BLOB_MAX, 0); // bytes after the CRC are ignored
    let l = restore_with_ext_backup(&rig, &st, &ext);
    assert_eq!(l.src, LoadSource::Backup);
    assert_eq!(l.cfg.failsafe.timeout_min, 90);
    assert_eq!(rig.dev.nvs.get_blob(NAMESPACE, KEY_CONFIG_EXT), ext);
}

#[test]
fn load_a_one_byte_cfgx_bak_goes_back_to_nvs_as_it_is() {
    let rig = Rig::new();
    let st = rig.storage();
    let l = restore_with_ext_backup(&rig, &st, b"V");
    assert_eq!(l.src, LoadSource::Backup);
    assert_eq!(l.cfg.failsafe.timeout_min, 60);
    assert_eq!(rig.dev.nvs.get_blob(NAMESPACE, KEY_CONFIG_EXT), b"V");
}

#[test]
fn load_a_single_unknown_cfgx_record_is_logged_as_a_newer_firmware() {
    let rig = Rig::new();
    let c = named(b"One");
    rig.dev.nvs.set_blob(NAMESPACE, KEY_CONFIG, &blob_of(&c));
    rig.dev
        .nvs
        .set_blob(NAMESPACE, KEY_CONFIG_EXT, &ext_of(&c, &[201, 0, 0]));
    let st = rig.storage();
    assert_eq!(load(&st).src, LoadSource::Stored);
    let ev = rig.host.with_code(EventCode::ConfigNewerSchema);
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].arg2, 1);
}

#[test]
fn begin_fs_a_partition_that_mounts_is_not_reported_as_formatted() {
    let rig = Rig::new();
    let st = rig.storage();
    let mut formatted = true;
    assert!(st.begin_fs(&mut formatted));
    assert!(!formatted);
    assert_eq!(rig.dev.fs.knobs().formats, 0);
    let total = rig.dev.fs.knobs().total_bytes;
    assert_eq!(st.fs_total(), total);
    assert_eq!(st.fs_used(), rig.dev.fs.usage().1);
}

#[test]
fn fs_used_zero_without_a_file_system() {
    let rig = Rig::new();
    rig.dev.fs.set_formatted(false);
    rig.dev.fs.knobs().format_ok = false;
    let st = rig.storage();
    let mut formatted = false;
    assert!(!st.begin_fs(&mut formatted));
    assert_eq!(st.fs_used(), 0);
}

// ---------------------------------------------------------------- legacy reader and import

#[test]
fn import_legacy_integers_of_every_width_are_read() {
    let widths: [(NvsType, i64); 7] = [
        (NvsType::U8, 200),
        (NvsType::I8, -5),
        (NvsType::U16, 40000),
        (NvsType::I16, -300),
        (NvsType::U32, 100_000),
        (NvsType::I32, -70000),
        (NvsType::I64, 123_456),
    ];
    for (ty, value) in widths {
        // a fresh device per width (the C++ erased vdmrev and protCfg)
        let rig = Rig::new();
        let nvs = &rig.dev.nvs;
        match ty {
            NvsType::U8 => nvs.set_u8("protCfg", "brokerMQTO", value as u8),
            NvsType::I8 => nvs.set_i8("protCfg", "brokerMQTO", value as i8),
            NvsType::U16 => nvs.set_u16("protCfg", "brokerMQTO", value as u16),
            NvsType::I16 => nvs.set_i16("protCfg", "brokerMQTO", value as i16),
            NvsType::U32 => nvs.set_u32("protCfg", "brokerMQTO", value as u32),
            NvsType::I32 => nvs.set_i32("protCfg", "brokerMQTO", value as i32),
            _ => nvs.set_i64("protCfg", "brokerMQTO", value),
        }
        let st = rig.storage();
        let l = load(&st);
        assert_eq!(l.src, LoadSource::Imported, "{ty:?}");
        assert!(l.report.legacy_failsafe_valid, "{ty:?}");
        assert_eq!(
            i64::from(l.report.legacy_failsafe_timeout_min),
            value,
            "{ty:?}"
        );
        assert_eq!(l.report.legacy_failsafe_pct, 0, "{ty:?}");
        assert_eq!(l.report.ignored, 1, "{ty:?}"); // brokerMQTO; brokerMQToPos is absent
    }
}

#[test]
fn import_legacy_strings_are_read_whole_an_over_long_one_is_rejected() {
    let rig = Rig::new();
    rig.dev.nvs.set_str("sysCfg", "stName", b"Legacy");
    rig.dev
        .nvs
        .set_str("netCfg", "timeServer", "h".repeat(65).as_bytes());
    let st = rig.storage();
    let l = load(&st);
    assert_eq!(l.src, LoadSource::Imported);
    assert_eq!(l.cfg.station.as_slice(), b"Legacy");
    assert_eq!(l.cfg.time.ntp_server.as_slice(), b"pool.ntp.org");
    assert_eq!(l.report.imported, 1);
    assert_eq!(l.report.first_rejected.as_slice(), b"netCfg/timeServer");
}

#[test]
fn import_a_legacy_blob_of_exactly_its_size_is_read() {
    let rig = Rig::new();
    let mut valves = vec![0u8; 12 * 12];
    valves[..3].copy_from_slice(b"Bad");
    valves[11] = 1;
    rig.dev.nvs.set_blob("valvesCfg", "valves", &valves);
    let st = rig.storage();
    let l = load(&st);
    assert_eq!(l.src, LoadSource::Imported);
    assert_eq!(l.cfg.valves[0].name.as_slice(), b"Bad");
}

#[test]
fn import_the_1_5_kb_temps_blob_is_read_into_the_config_blob_buffer() {
    let mut rig = Rig::new();
    let mut temps = vec![0u8; LEGACY_TEMPS_BLOB];
    temps[33 * 44..33 * 44 + 4].copy_from_slice(b"Flur"); // the last of 34 elements of 44 bytes
    temps[33 * 44 + 12] = 25; // offset 2.5 C
    rig.dev.nvs.set_blob("tempsCfg", "temps", &temps);
    {
        let st = rig.storage();
        let l = load(&st);
        assert_eq!(l.src, LoadSource::Imported);
        assert_eq!(l.report.imported, 1);
        assert_eq!(l.cfg.temps[33].name.as_slice(), b"Flur");
        assert_eq!(l.cfg.temps[33].offset, 25);
    }
    // The same buffer encoded the save afterwards: the next boot loads the imported config.
    rig.reboot(Reset::Software);
    let st = rig.storage();
    let again = load(&st);
    assert_eq!(again.src, LoadSource::Stored);
    assert_eq!(again.cfg.temps[33].name.as_slice(), b"Flur");
}

#[test]
fn import_the_last_calibration_time_is_stored_only_when_the_import_has_one() {
    let mut rig = Rig::new();
    rig.dev.nvs.set_str("sysCfg", "stName", b"Legacy");
    {
        let st = rig.storage();
        assert_eq!(load(&st).src, LoadSource::Imported);
        assert!(!rig.dev.nvs.has(NAMESPACE, KEY_LAST_CALIB));
    }
    erase_namespace(&rig.dev, NAMESPACE);
    rig.dev.nvs.set_i64("Misc", "MiscLC", 1_700_000_000);
    rig.reboot(Reset::Software);
    let st = rig.storage();
    let l = load(&st);
    assert_eq!(l.src, LoadSource::Imported);
    assert_eq!(l.report.last_calib_epoch, 1_700_000_000);
    assert_eq!(st.load_last_calib(), 1_700_000_000);
}

#[test]
fn import_dropped_messenger_settings_without_pi_valves_are_logged() {
    let rig = Rig::new();
    rig.dev.nvs.set_str("sysCfg", "stName", b"Legacy");
    rig.dev.nvs.set_u8("msgCfg", "msgFlags", 1);
    let st = rig.storage();
    assert_eq!(load(&st).src, LoadSource::Imported);
    let ev = rig.host.with_code(EventCode::ImportDropped);
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].arg1, 0);
    assert_eq!(ev[0].arg2, i32::from(DROPPED_MESSENGER));
}

#[test]
fn import_a_single_removed_legacy_image_is_logged() {
    let rig = Rig::new();
    let st = rig.storage();
    mount(&st);
    rig.dev.nvs.set_str("sysCfg", "stName", b"Legacy");
    rig.dev.fs.put("/x.bin", &[b'x'; 100]);
    assert_eq!(load(&st).src, LoadSource::Imported);
    let ev = rig.host.with_code(EventCode::FilesRemoved);
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].arg1, 1);
    assert_eq!(ev[0].arg2, 1);
}

#[test]
fn remove_legacy_images_a_legacy_image_with_the_longest_path_is_removed() {
    let rig = Rig::new();
    let st = rig.storage();
    mount(&st);
    let path = format!("/{}.bin", "L".repeat(FS_PATH_MAX - 5));
    assert_eq!(path.len(), FS_PATH_MAX);
    rig.dev.fs.put(&path, b"x");
    let mut kib = 0;
    assert_eq!(st.remove_legacy_images(&mut kib), 1);
    assert_eq!(kib, 1);
    assert!(!has_file(&rig.dev, &path));
}

#[test]
fn remove_legacy_images_a_file_that_cannot_be_removed_ends_the_run_after_its_batch() {
    let rig = Rig::new();
    let st = rig.storage();
    mount(&st);
    for i in 0..10 {
        rig.dev.fs.put(&format!("/i{i}.bin"), b"x");
    }
    rig.dev.fs.fail("remove", "/i0.bin", 100);
    let mut kib = 0;
    // the batch of eight, less the one that stays
    assert_eq!(st.remove_legacy_images(&mut kib), 7);
    assert!(has_file(&rig.dev, "/i0.bin"));
    assert!(has_file(&rig.dev, "/i8.bin"));
    assert!(has_file(&rig.dev, "/i9.bin"));
}

// ---------------------------------------------------------------- Rust (new): the reader

fn reader(dev: &Device) -> LegacyReader<'_, crate::testkit::FakeNvs> {
    LegacyReader { nvs: &dev.nvs }
}

#[test]
fn reader_integers_of_another_type_or_namespace_are_absent() {
    let rig = Rig::new();
    rig.dev.nvs.set_str("netCfg", "dhcp", b"1");
    rig.dev.nvs.set_u64("netCfg", "big", 5);
    let mut r = reader(&rig.dev);
    assert_eq!(r.read_int("netCfg", "dhcp"), None);
    assert_eq!(r.read_int("netCfg", "big"), None); // u64 is no legacy width
    assert_eq!(r.read_int("netCfg", "missing"), None);
    assert_eq!(r.read_int("noSuchNs", "dhcp"), None);
    rig.dev.nvs.set_u16("netCfg", "port", 65535);
    assert_eq!(r.read_int("netCfg", "port"), Some(65535));
    assert_eq!(rig.dev.nvs.knobs().open_handles, 0);
}

#[test]
fn reader_strings_up_to_the_buffer_whole_a_longer_one_refused_empty_ones_absent() {
    let rig = Rig::new();
    let nvs = &rig.dev.nvs;
    nvs.set_str("ns", "fits", b"abcd");
    nvs.set_str("ns", "long", b"abcde");
    nvs.set_str("ns", "empty", b"");
    nvs.put("ns", "broken", NvsType::Str, b""); // no NUL: no string at all
    nvs.set_blob("ns", "blob", b"abc");
    let mut r = reader(&rig.dev);
    let mut out: vdm_esp_core::common::Text<4> = vdm_esp_core::common::Text::new();
    assert_eq!(r.read_string("ns", "fits", &mut out), Some(false));
    assert_eq!(out.as_slice(), b"abcd");
    assert_eq!(r.read_string("ns", "long", &mut out), Some(true));
    assert!(out.is_empty());
    assert_eq!(r.read_string("ns", "empty", &mut out), Some(false));
    assert!(out.is_empty());
    assert_eq!(r.read_string("ns", "broken", &mut out), None);
    assert_eq!(r.read_string("ns", "blob", &mut out), None);
    assert_eq!(r.read_string("ns", "missing", &mut out), None);
    assert_eq!(r.read_string("nope", "fits", &mut out), None);
}

#[test]
fn reader_blobs_give_their_stored_length_a_larger_one_zeroes_the_output() {
    let rig = Rig::new();
    rig.dev.nvs.set_blob("ns", "b3", b"xyz");
    rig.dev.nvs.set_u8("ns", "u8", 1);
    let mut r = reader(&rig.dev);
    let mut out = [0xEEu8; 4];
    assert_eq!(r.read_blob("ns", "b3", &mut out), Some(3));
    assert_eq!(&out, b"xyz\xEE");
    let mut small = [0xEEu8; 2];
    assert_eq!(r.read_blob("ns", "b3", &mut small), Some(3));
    assert_eq!(small, [0, 0]);
    assert_eq!(r.read_blob("ns", "u8", &mut out), None);
    assert_eq!(r.read_blob("nope", "b3", &mut out), None);
}
