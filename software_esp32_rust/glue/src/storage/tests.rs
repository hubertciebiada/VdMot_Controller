//! `test_storage.cpp`: the smoke cases of file system mount, config load and apply, NVS values,
//! STM images; then the new Rust cases (constants, the boot guard records kept by a factory
//! reset across boots, the names of every result).
#![allow(clippy::large_stack_frames, clippy::large_stack_arrays)]

use super::rig::*;
use super::*;
use crate::boot_guard::{BootGuard, BootVerdict, KEY_OK, KEY_TRIAL};
use crate::port::Ota;
use crate::testkit::Reset;
use vdm_esp_core::config::{crc32, decode_config, DecodeResult};
use vdm_esp_core::stm_flasher::FlashImage;

#[test]
fn begin_fs_an_unformatted_partition_is_formatted_stm_and_log_created() {
    let rig = Rig::new();
    rig.dev.fs.set_formatted(false);
    let st = rig.storage();
    let mut formatted = false;
    assert!(st.begin_fs(&mut formatted));
    assert!(formatted);
    assert!(st.fs_ready());
    assert_eq!(rig.dev.fs.knobs().formats, 1);
    assert!(is_dir(&rig.dev, "/stm"));
    assert!(is_dir(&rig.dev, "/log"));
}

#[test]
fn begin_fs_a_format_that_fails_leaves_the_firmware_without_files() {
    let rig = Rig::new();
    rig.dev.fs.set_formatted(false);
    rig.dev.fs.knobs().format_ok = false;
    let st = rig.storage();
    let mut formatted = true;
    assert!(!st.begin_fs(&mut formatted));
    assert!(!formatted);
    assert!(!st.fs_ready());
    assert_eq!(st.fs_total(), 0);
}

#[test]
fn begin_fs_upload_leftovers_are_removed_images_are_indexed() {
    let rig = Rig::new();
    rig.dev.fs.put("/stm/a.bin", &[b'a'; 100]);
    rig.dev.fs.put("/stm/b.bin.part", b"x");
    rig.dev.fs.put("/stm/notes.txt", b"n");
    let st = rig.storage();
    mount(&st);
    assert!(!has_file(&rig.dev, "/stm/b.bin.part"));
    let e = st.find_image(b"a").unwrap();
    assert_eq!(e.size, 100);
    assert!(!e.scanned);
    let mut list: [ImageEntry; IMAGE_SLOTS] = Default::default();
    assert_eq!(st.list_images(&mut list), 1);
}

#[test]
fn load_config_a_stored_blob_is_loaded() {
    let rig = Rig::new();
    rig.dev
        .nvs
        .set_blob(NAMESPACE, KEY_CONFIG, &blob_of(&named(b"Stored")));
    let st = rig.storage();
    let l = load(&st);
    assert_eq!(l.src, LoadSource::Stored);
    assert_eq!(l.cfg.station.as_slice(), b"Stored");
    assert_eq!(rig.shared.boot_load_source(), LoadSource::Stored);
    assert!(rig.host.events().is_empty());
}

#[test]
fn load_config_an_unreadable_blob_gives_defaults_and_a_config_defaults_event() {
    let rig = Rig::new();
    let bad = corrupt(blob_of(&named(b"Bad")));
    rig.dev.nvs.set_blob(NAMESPACE, KEY_CONFIG, &bad);
    let st = rig.storage();
    let l = load(&st);
    assert_eq!(l.src, LoadSource::DefaultsAfterError);
    assert_eq!(l.cfg.station, Config::default().station);
    let e = &rig.host.with_code(EventCode::ConfigDefaults)[0];
    assert_eq!(e.arg1, i32::from(l.details.error_code));
    assert_ne!(l.details.error_code, 0);
    // never overwritten automatically
    assert_eq!(rig.dev.nvs.get_blob(NAMESPACE, KEY_CONFIG), bad);
}

#[test]
fn load_config_nothing_stored_and_no_legacy_data_gives_defaults_import_done() {
    let mut rig = Rig::new();
    {
        let st = rig.storage();
        let l = load(&st);
        assert_eq!(l.src, LoadSource::Defaults);
        assert_eq!(nvs_int(&rig.dev, NAMESPACE, KEY_IMPORTED), 1);
        assert!(!rig.dev.nvs.get_blob(NAMESPACE, KEY_CONFIG).is_empty());
    }
    // the next boot loads what the first one saved
    rig.reboot(Reset::Software);
    let st = rig.storage();
    assert_eq!(load(&st).src, LoadSource::Stored);
}

#[test]
fn apply_config_persisted_published_revision_plus_1_a_failed_write_names_nvs() {
    let rig = Rig::new();
    let st = rig.storage();
    rig.shared.set_active_config(&named(b"A"));
    let rev = rig.shared.config_revision();
    let mut path = [0u8; 32];
    assert_eq!(st.apply_config(&named(b"B"), &mut path), Ok(()));
    assert_eq!(rig.shared.config_revision(), rev + 1);
    assert!(rig.shared.config_saved_since_boot());
    let mut out = named(b"");
    rig.shared.get_config(&mut out);
    assert_eq!(out.station.as_slice(), b"B");
    let mut decoded = named(b"");
    let blob = rig.dev.nvs.get_blob(NAMESPACE, KEY_CONFIG);
    assert_eq!(decode_config(&blob, &mut decoded, None), DecodeResult::Ok);
    assert_eq!(decoded.station.as_slice(), b"B");
    rig.dev.nvs.knobs().fail_set.insert(KEY_CONFIG.to_string());
    assert_eq!(st.apply_config(&named(b"C"), &mut path), Err(3));
    assert_eq!(&path[..3], b"nvs");
    assert_eq!(rig.shared.config_revision(), rev + 1);
}

#[test]
fn calib_config_the_schedule_of_the_active_config_a_saved_change_at_once() {
    let rig = Rig::new();
    let st = rig.storage();
    let mut c = named(b"A");
    c.calib.day_mask = 0x41;
    c.calib.hour = 23;
    c.calib.minute = 59;
    rig.shared.set_active_config(&c);
    let k = rig.shared.calib_config();
    assert_eq!((k.day_mask, k.hour, k.minute), (0x41, 23, 59));
    c.calib.day_mask = 0;
    c.calib.hour = 4;
    c.calib.minute = 30;
    let mut path = [0u8; 32];
    assert_eq!(st.apply_config(&c, &mut path), Ok(()));
    let k = rig.shared.calib_config();
    assert_eq!((k.day_mask, k.hour, k.minute), (0, 4, 30));
}

#[test]
fn factory_reset_vdmrev_is_erased_the_latch_is_kept_imported_is_set() {
    let rig = Rig::new();
    let nvs = &rig.dev.nvs;
    nvs.set_blob(NAMESPACE, KEY_CONFIG, &blob_of(&named(b"X")));
    nvs.set_u8(NAMESPACE, KEY_FACTORY_LATCH, 1);
    nvs.set_u32(NAMESPACE, KEY_BOOT_COUNT, 9);
    let st = rig.storage();
    assert!(st.factory_reset());
    assert!(!nvs.has(NAMESPACE, KEY_CONFIG));
    assert!(!nvs.has(NAMESPACE, KEY_BOOT_COUNT));
    assert_eq!(nvs_int(&rig.dev, NAMESPACE, KEY_FACTORY_LATCH), 1);
    assert_eq!(nvs_int(&rig.dev, NAMESPACE, KEY_IMPORTED), 1);
}

#[test]
fn values_boot_count_calibration_slot_and_time_flags() {
    let rig = Rig::new();
    rig.dev.nvs.set_u32(NAMESPACE, KEY_BOOT_COUNT, 4);
    let st = rig.storage();
    assert_eq!(st.increment_boot_count(), 5);
    assert_eq!(rig.shared.boot_count(), 5);
    assert_eq!(nvs_int(&rig.dev, NAMESPACE, KEY_BOOT_COUNT), 5);
    st.save_calib_slot(20_260_923);
    assert_eq!(st.load_calib_slot(), 20_260_923);
    st.save_last_calib(0); // ignored
    assert_eq!(st.load_last_calib(), 0);
    st.save_last_calib(1_790_136_000);
    assert_eq!(st.load_last_calib(), 1_790_136_000);
    assert!(!st.ha_cleanup_done());
    st.set_ha_cleanup_done();
    assert!(st.ha_cleanup_done());
    st.set_ota_stm_required(true);
    assert!(st.ota_stm_required());
    st.clear_ota_stm_required();
    assert!(!rig.dev.nvs.has(NAMESPACE, KEY_OTA_STM));
    st.set_ha_layout(2);
    assert_eq!(st.ha_layout(), 2);
    assert!(st.save_targets(&[1, 2, 3]));
    let mut out = [0u8; 3];
    assert_eq!(st.load_targets(&mut out[..2]), 0);
    assert_eq!(st.load_targets(&mut out), 3);
    assert_eq!(out, [1, 2, 3]);
}

#[test]
fn image_upload_the_part_file_becomes_the_image_and_is_indexed() {
    let rig = Rig::new();
    let st = rig.storage();
    mount(&st);
    assert_eq!(st.image_upload_begin(b"fw.bin", 1000), ImageResult::Ok);
    assert!(st.image_upload_active());
    assert!(has_file(&rig.dev, "/stm/fw.bin.part"));
    let data = [b'd'; 300];
    assert_eq!(st.image_upload_write(&data), ImageResult::Ok);
    let info = st.image_upload_end().unwrap();
    assert_eq!(info.name.as_slice(), b"fw");
    assert_eq!(info.size, 300);
    assert_eq!(info.crc, crc32(&data, 0));
    assert_eq!(file(&rig.dev, "/stm/fw.bin").unwrap(), data);
    assert!(!has_file(&rig.dev, "/stm/fw.bin.part"));
    assert!(!st.image_upload_active());
    // C++: the part file got the 512 B stdio buffer (bufferSizes): no Rust form, the Fs port
    // has no stdio layer.
    assert_eq!(rig.dev.fs.open_handles(), 0);
}

#[test]
fn image_upload_bad_names_last_good_busy_too_many_images() {
    let rig = Rig::new();
    let st = rig.storage();
    mount(&st);
    assert_eq!(st.image_upload_begin(b".hidden", 10), ImageResult::BadName);
    assert_eq!(
        st.image_upload_begin(b"last_good.bin", 10),
        ImageResult::BadName
    );
    for n in ["a", "b", "c"] {
        rig.dev.fs.put(&format!("/stm/{n}.bin"), b"x");
    }
    rig.dev.fs.put("/stm/last_good.bin", b"x");
    let mut formatted = false;
    st.begin_fs(&mut formatted); // indexes the four images
    assert_eq!(st.image_upload_begin(b"d", 10), ImageResult::TooMany);
    assert_eq!(st.image_upload_begin(b"a", 10), ImageResult::Ok); // replaces a
    assert_eq!(st.image_upload_begin(b"b", 10), ImageResult::Busy);
    st.image_upload_abort();
    assert!(!has_file(&rig.dev, "/stm/a.bin.part"));
}

#[test]
fn image_upload_not_enough_free_space_for_the_image_and_the_reserve() {
    let rig = Rig::new();
    let st = rig.storage();
    mount(&st);
    let used = rig.dev.fs.usage().1;
    rig.dev.fs.knobs().total_bytes = used + 1000 + FS_RESERVE as u32;
    assert_eq!(st.image_upload_begin(b"x", 1001), ImageResult::NoSpace);
    assert_eq!(st.image_upload_begin(b"x", 1000), ImageResult::Ok);
}

#[test]
fn images_delete_and_the_last_good_copy_after_a_flash() {
    let rig = Rig::new();
    let st = rig.storage();
    mount(&st);
    let img = [b'i'; 3000];
    rig.dev.fs.put("/stm/fw.bin", &img);
    let mut formatted = false;
    st.begin_fs(&mut formatted);
    st.request_last_good_copy(b"fw");
    st.service();
    assert_eq!(file(&rig.dev, "/stm/last_good.bin").unwrap(), img);
    // C++: both files got the 512 B stdio buffer: no Rust form (no stdio layer).
    assert_eq!(rig.dev.fs.open_handles(), 0);
    let e = st.find_image(b"last_good").unwrap();
    assert_eq!(e.size, 3000);
    assert_eq!(st.delete_image(b"fw"), ImageResult::Ok);
    assert!(!has_file(&rig.dev, "/stm/fw.bin"));
    assert_eq!(st.delete_image(b"fw"), ImageResult::NotFound);
}

#[test]
fn file_image_reads_at_offsets_refuses_reads_past_the_end() {
    let rig = Rig::new();
    let st = rig.storage();
    mount(&st);
    rig.dev.fs.put("/stm/fw.bin", b"0123456789");
    let mut img = FileImage::new(rig.dev.fs.clone());
    assert!(img.open(b"fw"));
    assert_eq!(img.size(), 10);
    let mut out = [0u8; 4];
    assert!(img.read(6, &mut out));
    assert_eq!(&out, b"6789");
    assert!(img.read(0, &mut out[..2]));
    assert_eq!(out[0], b'0');
    assert!(!img.read(8, &mut out[..3]));
    img.close();
    assert_eq!(img.size(), 0);
    assert!(!img.open(b"missing"));
    // C++: the image got the 512 B stdio buffer: no Rust form (no stdio layer).
    assert_eq!(rig.dev.fs.open_handles(), 0);
}

#[test]
fn names_image_name_rules_and_paths() {
    let mut out = [0u8; 40];
    assert_eq!(normalize_image_name(b"fw-1_2.bin", &mut out), Some(6));
    assert_eq!(&out[..6], b"fw-1_2");
    assert_eq!(normalize_image_name(b"a b", &mut out), None);
    assert_eq!(normalize_image_name(b".bin", &mut out), None);
    let n = image_path(b"fw", true, &mut out).unwrap();
    assert_eq!(&out[..n], b"/stm/fw.bin.part");
    assert_eq!(image_path(b"fw", true, &mut out[..16]), None);
    assert_eq!(image_result_name(ImageResult::TooMany), "too_many_images");
}

// ---------------------------------------------------------------- Rust (new)

#[test]
fn constants_are_the_cpp_ones() {
    let keys = [
        NAMESPACE,
        KEY_CONFIG,
        KEY_CONFIG_EXT,
        KEY_IMPORTED,
        KEY_BOOT_COUNT,
        KEY_CALIB_SLOT,
        KEY_LAST_CALIB,
        KEY_HA_CLEANUP,
        KEY_TARGETS,
        KEY_NET_TRIAL,
        KEY_FACTORY_LATCH,
        KEY_OTA_STM,
        KEY_HA_LAYOUT,
    ];
    assert_eq!(
        keys,
        [
            "vdmrev", "cfg", "cfgx", "imported", "boots", "calSlot", "lastCal", "haDrop",
            "targets", "netTrial", "frLatch", "otaStm", "haLayout"
        ]
    );
    assert_eq!(
        [BACKUP_BASE, BACKUP_EXT, IMPORT_REPORT_FILE],
        ["/sys/cfg.bak", "/sys/cfgx.bak", "/sys/import.json"]
    );
    assert_eq!(BLOBS_LEN, 4096 + 1536);
    assert_eq!(MAX_IMAGE_SIZE, 524_288);
    assert_eq!((MAX_UPLOADED_IMAGES, IMAGE_SLOTS), (3, 4));
    assert_eq!(FS_RESERVE, 147_456);
    assert_eq!(LAST_GOOD_NAME, "last_good");
    assert_eq!(IMAGE_NAME_MAX, 31);
    // the boot guard records a factory reset keeps fit the buffer
    assert_eq!(GUARD_RECORD_MAX, 32);
    assert_eq!(FACTORY_RESET_KEEPS, [KEY_OK, KEY_TRIAL]);
}

#[test]
fn result_names_of_every_image_result() {
    let names = [
        (ImageResult::Ok, "ok"),
        (ImageResult::BadName, "bad_name"),
        (ImageResult::TooLarge, "too_large"),
        (ImageResult::NoSpace, "no_space"),
        (ImageResult::TooMany, "too_many_images"),
        (ImageResult::Busy, "busy"),
        (ImageResult::Io, "io_error"),
        (ImageResult::NotFound, "not_found"),
        (ImageResult::Empty, "empty"),
    ];
    for (r, name) in names {
        assert_eq!(image_result_name(r), name);
    }
}

#[test]
fn factory_reset_keeps_the_boot_guard_records_the_next_boot_stays_confirmed() {
    // design 6.5: a Rust factory reset keeps frLatch, otaOk and otaTrial
    let mut rig = Rig::new();
    let app = rig.dev.ota.running().app.unwrap();
    {
        // the first boot confirms itself: no valid fallback in the other slot
        let (guard, _) = BootGuard::boot(&rig.dev.nvs, &rig.dev.ota, &rig.dev.rtc, &rig.dev.system);
        assert_eq!(guard.verdict(), BootVerdict::Confirmed);
    }
    let ok = rig.dev.nvs.get_blob(NAMESPACE, KEY_OK);
    assert_eq!(ok.len(), 16);
    let trial = [7u8; 32];
    rig.dev.nvs.set_blob(NAMESPACE, KEY_TRIAL, &trial);
    rig.dev.nvs.set_u8(NAMESPACE, KEY_FACTORY_LATCH, 1);
    rig.dev.nvs.set_u8(NAMESPACE, KEY_HA_LAYOUT, 2);
    {
        let st = rig.storage();
        assert!(st.factory_reset());
    }
    assert_eq!(rig.dev.nvs.get_blob(NAMESPACE, KEY_OK), ok);
    assert_eq!(rig.dev.nvs.get_blob(NAMESPACE, KEY_TRIAL), trial);
    assert_eq!(nvs_int(&rig.dev, NAMESPACE, KEY_FACTORY_LATCH), 1);
    assert!(!rig.dev.nvs.has(NAMESPACE, KEY_HA_LAYOUT));
    rig.reboot(Reset::Software);
    let (guard, report) =
        BootGuard::boot(&rig.dev.nvs, &rig.dev.ota, &rig.dev.rtc, &rig.dev.system);
    assert_eq!(guard.verdict(), BootVerdict::Confirmed);
    assert_eq!(report.events, [None, None]);
    assert_eq!(rig.dev.ota.running().app, Some(app));
    let st = rig.storage();
    assert_eq!(load(&st).src, LoadSource::Defaults); // imported is set: no legacy import
}

#[test]
fn factory_reset_without_guard_records_or_latch_leaves_only_imported() {
    let rig = Rig::new();
    rig.dev.nvs.set_u32(NAMESPACE, KEY_BOOT_COUNT, 3);
    let st = rig.storage();
    assert!(st.factory_reset());
    assert_eq!(rig.dev.nvs.keys(NAMESPACE), vec![KEY_IMPORTED.to_string()]);
}

#[test]
fn factory_reset_drops_a_guard_record_longer_than_any_record() {
    let rig = Rig::new();
    rig.dev.nvs.set_blob(NAMESPACE, KEY_TRIAL, &[1u8; 33]);
    rig.dev.nvs.set_blob(NAMESPACE, KEY_OK, &[2u8; 32]);
    let st = rig.storage();
    assert!(st.factory_reset());
    assert!(!rig.dev.nvs.has(NAMESPACE, KEY_TRIAL));
    assert_eq!(rig.dev.nvs.get_blob(NAMESPACE, KEY_OK), vec![2u8; 32]);
}
