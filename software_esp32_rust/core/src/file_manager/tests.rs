//! Port of test/native/test_file_manager.cpp: path syntax, kinds and deletability (every row of
//! the table), kind names and protect reasons, legacy image names, the /api/files document.
//! The C++ null-pointer cases and the names of an out-of-range kind have no Rust form (named
//! where they stood).

use super::*;
use crate::common::copy_string;
use crate::test_support::assert_text;
use std::format;

fn valid(p: &str) -> bool {
    fs_path_valid(p.as_bytes())
}
fn kind(p: &str) -> FileKind {
    classify_fs_path(p.as_bytes())
}
fn legacy(p: &str) -> bool {
    is_legacy_image_file(p.as_bytes())
}

#[test]
fn path_syntax() {
    assert!(valid("/a"));
    assert!(valid("/stm/x.bin"));
    assert!(valid("/Bad Name.bin"));
    assert!(valid("/\u{141}azienka.bin")); // UTF-8
    assert!(valid("/.x"));
    assert!(valid("/..x"));
    assert!(valid("/a/.b/c"));
    assert!(valid(&format!("/{}", "a".repeat(94)))); // 95 bytes
    assert!(!valid(&format!("/{}", "a".repeat(95)))); // 96 bytes
    for bad in [
        "", "/", "a", "ab", "//a", "/a//b", "/a/", "/.", "/..", "/a/../b", "/a/./b", "/a/..",
        "/a\x01", "/a\0b",
    ] {
        assert!(!valid(bad), "{bad:?}");
    }
    // C++ fsPathValid(nullptr, 3): no Rust form.
}

#[test]
fn kinds_deletability_and_reasons() {
    let rows: [(&str, FileKind, &str, bool, &str); 18] = [
        (
            "/stm/x.bin",
            FileKind::StmImage,
            "stm_image",
            false,
            "use DELETE /api/stm/images/<name>",
        ),
        (
            "/stm/x.bin.part",
            FileKind::UploadPart,
            "upload_part",
            true,
            "",
        ),
        ("/stm/y.part", FileKind::UploadPart, "upload_part", true, ""),
        ("/stm/notes.txt", FileKind::Other, "other", true, ""),
        ("/stm/X.BIN", FileKind::Other, "other", true, ""),
        ("/stm/a/b.bin", FileKind::Other, "other", true, ""),
        ("/log/events.log", FileKind::Log, "log", false, "event log"),
        ("/log/a/b", FileKind::Log, "log", false, "event log"),
        (
            "/sys/cfg.bak",
            FileKind::Internal,
            "internal",
            false,
            "internal file",
        ),
        (
            "/sys/import.json",
            FileKind::Internal,
            "internal",
            false,
            "internal file",
        ),
        (
            "/HADiscovery.cfg",
            FileKind::LegacyHaList,
            "legacy_ha_list",
            false,
            "kept for a rollback to the legacy firmware",
        ),
        ("/HADiscovery.cfg.done", FileKind::Other, "other", true, ""),
        ("/HADiscovery.cf", FileKind::Other, "other", true, ""),
        ("/STM.BIN", FileKind::LegacyImage, "legacy_image", true, ""),
        ("/fw.bin", FileKind::LegacyImage, "legacy_image", true, ""),
        ("/log", FileKind::Other, "other", true, ""),
        ("/sysx/a", FileKind::Other, "other", true, ""),
        ("/index.html", FileKind::Other, "other", true, ""),
    ];
    for (path, k, name, deletable, reason) in rows {
        let got = kind(path);
        assert_eq!(got, k, "{path}");
        assert_eq!(file_kind_name(got), name, "{path}");
        assert_eq!(file_deletable(got), deletable, "{path}");
        assert_eq!(file_protect_reason(got), reason, "{path}");
    }
    // C++ the name and reason of FileKind 99 ("other", ""): a Rust enum cannot hold 99.
    assert_eq!(FileKind::from_raw(99), None);
}

#[test]
fn legacy_image_names() {
    for good in ["/a.bin", "/A.BIN", "/a.Bin", "/a.biN", "/a.bIn"] {
        assert!(legacy(good), "{good}");
    }
    for bad in [
        "/.bin",
        "/stm/a.bin",
        "/a.bin.part",
        "/a.bi",
        "/a.bix",
        "/a.xin",
        "/a_bin",
        "/aabin",
        "a.bin",
        "xa.bin",
    ] {
        assert!(!legacy(bad), "{bad}");
    }
    // C++ isLegacyImageFile(nullptr, 6): no Rust form.
}

#[test]
fn the_api_files_document() {
    let mut files: [FileEntry; 3] = Default::default();
    copy_string(&mut files[0].path, b"/stm/x.bin");
    files[0].size = 98304;
    copy_string(&mut files[1].path, b"/Bad \"1\".bin");
    files[1].size = 7;
    copy_string(&mut files[2].path, b"/sys/cfg.bak");
    files[2].size = 0;
    let mut buf = [0u8; 512];
    let mut jw = JsonWriter::new(&mut buf);
    assert!(write_files_json(&mut jw, &files, 1_507_328, 204_800, true));
    assert_text(
        jw.as_bytes(),
        "{\"total\":1507328,\"used\":204800,\"truncated\":true,\"files\":[\
{\"path\":\"/stm/x.bin\",\"size\":98304,\"kind\":\"stm_image\",\"deletable\":false},\
{\"path\":\"/Bad \\\"1\\\".bin\",\"size\":7,\"kind\":\"legacy_image\",\"deletable\":true},\
{\"path\":\"/sys/cfg.bak\",\"size\":0,\"kind\":\"internal\",\"deletable\":false}]}",
    );
    let mut empty = JsonWriter::new(&mut buf);
    assert!(write_files_json(&mut empty, &files[..0], 0, 0, false));
    assert_text(
        empty.as_bytes(),
        "{\"total\":0,\"used\":0,\"truncated\":false,\"files\":[]}",
    );
    let mut small = JsonWriter::new(&mut buf[..30]);
    assert!(!write_files_json(&mut small, &files, 1, 1, false));
}

#[test]
fn kind_values() {
    // Rust: the raw values of the kinds.
    for v in 0..=6 {
        assert_eq!(FileKind::from_raw(v).map(|k| k as u8), Some(v));
    }
    assert_eq!(FileKind::from_raw(7), None);
    assert_eq!(FS_PATH_MAX, 95);
}
