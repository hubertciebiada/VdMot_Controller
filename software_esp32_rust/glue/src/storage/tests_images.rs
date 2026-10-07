//! `test_storage_images.cpp`: the STM image store, the index and its slots, upload leftovers,
//! the upload limits and failures, the last_good copy (steps, space, failures), the image scan,
//! the protected part of a running upload, delete_image and FileImage; then the new Rust cases.
#![allow(clippy::large_stack_frames, clippy::large_stack_arrays)]

use std::collections::VecDeque;

use super::rig::*;
use super::*;
use crate::port::Fs;
use crate::testkit::Device;
use vdm_esp_core::config::crc32;
use vdm_esp_core::stm_flasher::{FlashError, FlashImage};

fn pattern(n: usize) -> Vec<u8> {
    (0..n).map(|i| b'a' + (i % 26) as u8).collect()
}

fn put32(s: &mut [u8], at: usize, v: u32) {
    s[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

/// A 4 KiB STM image that passes the chip-independent checks: SP and reset vector, optional
/// handshake strings, the version string.
fn stm_image(handshake: bool) -> Vec<u8> {
    let mut s = vec![0x80u8; 4096];
    put32(&mut s, 0, 0x2002_0000);
    put32(&mut s, 4, 0x0800_0101);
    let mut at = 3000;
    if handshake {
        let h = b"\x01DEADBEEF\0\x01BEEFIT\0";
        s[at..at + h.len()].copy_from_slice(h);
        at += h.len();
    }
    let v = b"\x011.4.9_Dev\0";
    s[at..at + v.len()].copy_from_slice(v);
    s
}

fn names(st: &TestStorage<'_>) -> Vec<Vec<u8>> {
    let mut list: [ImageEntry; IMAGE_SLOTS] = Default::default();
    let n = st.list_images(&mut list);
    list[..n].iter().map(|e| e.name.to_vec()).collect()
}

fn scanned(st: &TestStorage<'_>, name: &[u8]) -> bool {
    st.find_image(name).unwrap().scanned
}

fn part_size(dev: &Device, path: &str) -> Option<usize> {
    file(dev, path).map(|d| d.len())
}

const PART: &str = "/stm/last_good.bin.part";
const LAST_GOOD: &str = "/stm/last_good.bin";

// ---------------------------------------------------------------- index

#[test]
fn images_the_index_takes_the_free_slots_in_order_a_fifth_image_stays_out() {
    let rig = Rig::new();
    rig.dev.fs.put("/stm/a.bin", b"1");
    rig.dev.fs.put("/stm/b.bin", b"22");
    rig.dev.fs.put("/stm/c.bin", b"333");
    rig.dev.fs.put(LAST_GOOD, b"4444");
    rig.dev.fs.put("/stm/z.bin", b"55555");
    let st = rig.storage();
    mount(&st);
    assert_eq!(
        names(&st),
        vec![
            b"a".to_vec(),
            b"b".to_vec(),
            b"c".to_vec(),
            b"last_good".to_vec()
        ]
    );
    let e = st.find_image(b"c").unwrap();
    assert_eq!(e.name.as_slice(), b"c");
    assert_eq!(e.size, 3);
    assert!(st.find_image(b"z").is_none());
    let mut two: [ImageEntry; 2] = Default::default();
    assert_eq!(st.list_images(&mut two), 2);
    assert_eq!(two[1].name.as_slice(), b"b");
}

#[test]
fn images_a_31_character_name_is_indexed_uploaded_and_copied_to_last_good() {
    let rig = Rig::new();
    let disk = "d".repeat(31);
    rig.dev.fs.put(&format!("/stm/{disk}.bin"), b"on disk");
    let st = rig.storage();
    mount(&st);
    assert_eq!(st.find_image(disk.as_bytes()).unwrap().size, 7);
    let up = "u".repeat(31);
    assert_eq!(
        st.image_upload_begin(format!("{up}.bin").as_bytes(), 10),
        ImageResult::Ok
    );
    let data = pattern(10);
    assert_eq!(st.image_upload_write(&data), ImageResult::Ok);
    let info = st.image_upload_end().unwrap();
    assert_eq!(info.name.as_slice(), up.as_bytes());
    assert_eq!(file(&rig.dev, &format!("/stm/{up}.bin")).unwrap(), data);
    st.request_last_good_copy(up.as_bytes());
    st.service();
    assert_eq!(file(&rig.dev, LAST_GOOD).unwrap(), data);
}

#[test]
fn images_at_most_eight_upload_leftovers_are_removed_per_boot() {
    let rig = Rig::new();
    for i in 0..9 {
        rig.dev.fs.put(&format!("/stm/p{i}.part"), b"x");
    }
    let st = rig.storage();
    mount(&st);
    for i in 0..8 {
        assert!(!has_file(&rig.dev, &format!("/stm/p{i}.part")));
    }
    assert!(has_file(&rig.dev, "/stm/p8.part"));
    assert_eq!(rig.dev.journal.of("fs remove").len(), 8);
}

#[test]
fn images_a_bare_part_and_a_42_character_leftover_are_removed() {
    let rig = Rig::new();
    let long_part = format!("{}.part", "q".repeat(37));
    rig.dev.fs.put("/stm/.part", b"x");
    rig.dev.fs.put(&format!("/stm/{long_part}"), b"y");
    let st = rig.storage();
    mount(&st);
    assert!(!has_file(&rig.dev, "/stm/.part"));
    assert!(!has_file(&rig.dev, &format!("/stm/{long_part}")));
}

// ---------------------------------------------------------------- last_good copy

#[test]
fn last_good_copy_8_kib_per_call_the_copied_image_busy_until_it_ends() {
    let rig = Rig::new();
    let img = pattern(20000);
    rig.dev.fs.put("/stm/fw.bin", &img);
    rig.dev.fs.put("/stm/a.bin", b"a");
    let st = rig.storage();
    mount(&st);
    st.request_last_good_copy(b"fw");
    st.service();
    assert_eq!(part_size(&rig.dev, PART), Some(8192));
    assert!(!has_file(&rig.dev, LAST_GOOD));
    assert_eq!(st.delete_image(b"fw"), ImageResult::Busy);
    assert_eq!(st.delete_image(b"a"), ImageResult::Ok);
    st.service();
    assert_eq!(part_size(&rig.dev, PART), Some(16384));
    st.service();
    assert_eq!(file(&rig.dev, LAST_GOOD).unwrap(), img);
    assert!(!has_file(&rig.dev, PART));
    assert_eq!(rig.dev.fs.open_handles(), 0);
    assert_eq!(st.find_image(b"last_good").unwrap().size, 20000);
    // Done: the next call scans instead of copying again.
    let writes = rig.dev.fs.knobs().write_opens;
    st.service();
    assert_eq!(rig.dev.fs.knobs().write_opens, writes);
    assert!(scanned(&st, b"last_good")); // in the slot "a" left
    assert_eq!(st.delete_image(b"fw"), ImageResult::Ok);
}

#[test]
fn last_good_copy_a_1_kib_buffer_per_pass_a_pass_without_memory_copies_nothing() {
    let rig = Rig::new();
    let img = pattern(9000);
    rig.dev.fs.put("/stm/fw.bin", &img);
    let st = rig.storage();
    mount(&st);
    st.request_last_good_copy(b"fw");
    rig.dev.heap.state().next = VecDeque::from([false]);
    st.service(); // the copy starts, its buffer does not
    assert_eq!(part_size(&rig.dev, PART), Some(0));
    assert!(rig.dev.heap.state().granted.is_empty());
    assert_eq!(st.delete_image(b"fw"), ImageResult::Busy);
    st.service();
    assert_eq!(part_size(&rig.dev, PART), Some(8192));
    assert_eq!(rig.dev.heap.state().granted, vec![1024]);
    st.service();
    assert_eq!(file(&rig.dev, LAST_GOOD).unwrap(), img);
    assert_eq!(rig.dev.heap.state().granted, vec![1024, 1024]);
    assert_eq!(rig.dev.fs.open_handles(), 0);
}

#[test]
fn last_good_copy_the_image_plus_16_kib_must_be_free() {
    let rig = Rig::new();
    rig.dev.fs.put("/stm/fw.bin", &pattern(3000));
    let st = rig.storage();
    mount(&st);
    let used = rig.dev.fs.usage().1;
    rig.dev.fs.knobs().total_bytes = used + 3000 + 16 * 1024;
    st.request_last_good_copy(b"fw");
    st.service();
    assert_eq!(file(&rig.dev, LAST_GOOD).unwrap(), pattern(3000));
    assert!(!rig.host.has(EventCode::StmFlashFailed));
}

#[test]
fn last_good_copy_one_byte_short_of_the_space_logs_and_copies_nothing() {
    let rig = Rig::new();
    rig.dev.fs.put("/stm/fw.bin", &pattern(3000));
    let st = rig.storage();
    mount(&st);
    let used = rig.dev.fs.usage().1;
    rig.dev.fs.knobs().total_bytes = used + 3000 + 16 * 1024 - 1;
    st.request_last_good_copy(b"fw");
    st.service();
    assert!(!has_file(&rig.dev, PART));
    assert!(!has_file(&rig.dev, LAST_GOOD));
    assert_eq!(rig.dev.fs.open_handles(), 0);
    let ev = rig.host.with_code(EventCode::StmFlashFailed);
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].valve, NO_VALVE);
    assert_eq!(ev[0].arg1, 0);
    assert_eq!(ev[0].arg2, 0);
    assert_eq!(ev[0].text.as_slice(), b"last_good: no space");
}

#[test]
fn last_good_copy_an_empty_image_is_not_copied_a_one_byte_image_is() {
    let rig = Rig::new();
    rig.dev.fs.put("/stm/e.bin", b"");
    rig.dev.fs.put("/stm/o.bin", b"x");
    let st = rig.storage();
    mount(&st);
    st.request_last_good_copy(b"e");
    st.service();
    assert!(!has_file(&rig.dev, LAST_GOOD));
    assert!(!has_file(&rig.dev, PART));
    assert!(st.find_image(b"last_good").is_none());
    st.request_last_good_copy(b"o");
    st.service();
    assert_eq!(file(&rig.dev, LAST_GOOD).unwrap(), b"x");
    assert_eq!(st.find_image(b"last_good").unwrap().size, 1);
}

#[test]
fn last_good_copy_a_failed_write_keeps_the_old_copy_a_failed_rename_drops_it() {
    let rig = Rig::new();
    rig.dev.fs.put(LAST_GOOD, b"old");
    rig.dev.fs.put("/stm/z.bin", &pattern(3000));
    let st = rig.storage();
    mount(&st);
    rig.dev.fs.fail("write", PART, 1);
    st.request_last_good_copy(b"z");
    st.service();
    assert_eq!(file(&rig.dev, LAST_GOOD).unwrap(), b"old");
    assert!(!has_file(&rig.dev, PART));
    assert_eq!(rig.dev.fs.open_handles(), 0);
    assert_eq!(st.find_image(b"last_good").unwrap().size, 3);
    // The rename runs after the old copy was removed: nothing is left to index.
    rig.dev.fs.fail("rename", PART, 1);
    st.request_last_good_copy(b"z");
    st.service();
    assert!(!has_file(&rig.dev, LAST_GOOD));
    assert!(!has_file(&rig.dev, PART));
    assert!(st.find_image(b"last_good").is_none());
}

#[test]
fn last_good_copy_last_good_itself_is_never_copied() {
    let rig = Rig::new();
    rig.dev.fs.put(LAST_GOOD, b"lg");
    let st = rig.storage();
    mount(&st);
    st.request_last_good_copy(b"last_good");
    st.service();
    // C++: no "fs.open /stm/last_good.bin.part w" in the journal
    assert_eq!(rig.dev.fs.knobs().write_opens, 0);
    assert_eq!(file(&rig.dev, LAST_GOOD).unwrap(), b"lg");
}

// ---------------------------------------------------------------- scan

#[test]
fn scan_one_image_per_call_in_index_order_the_results_recorded() {
    let rig = Rig::new();
    let good = stm_image(true);
    rig.dev.fs.put("/stm/a.bin", &good);
    rig.dev.fs.put("/stm/b.bin", &stm_image(false));
    let st = rig.storage();
    mount(&st);
    st.service();
    let e = st.find_image(b"a").unwrap();
    assert!(e.scanned);
    assert_eq!(e.check, FlashError::None);
    assert_eq!(e.crc, crc32(&good, 0));
    assert_eq!(e.version.as_slice(), b"1.4.9_Dev");
    assert!(!scanned(&st, b"b"));
    st.service();
    let e = st.find_image(b"b").unwrap();
    assert!(e.scanned);
    assert_eq!(e.check, FlashError::ImageNoHandshake);
    st.service();
    assert_eq!(rig.dev.fs.open_handles(), 0);
}

#[test]
fn scan_an_image_whose_size_changed_after_indexing_is_not_scanned() {
    let rig = Rig::new();
    rig.dev.fs.put("/stm/a.bin", &[b'a'; 100]);
    let st = rig.storage();
    mount(&st);
    rig.dev.fs.put("/stm/a.bin", &[b'a'; 200]);
    st.service();
    st.service();
    assert!(!scanned(&st, b"a"));
}

#[test]
fn scan_an_upload_holds_back_only_the_scan_of_its_own_image() {
    let rig = Rig::new();
    rig.dev.fs.put("/stm/a.bin", &stm_image(true));
    let st = rig.storage();
    mount(&st);
    assert_eq!(st.image_upload_begin(b"a", 10), ImageResult::Ok);
    st.service();
    assert!(!scanned(&st, b"a"));
    st.image_upload_abort();
    assert_eq!(st.image_upload_begin(b"b", 10), ImageResult::Ok);
    st.service();
    assert!(scanned(&st, b"a"));
}

// ---------------------------------------------------------------- upload

#[test]
fn upload_exactly_512_kib_is_accepted_one_byte_more_removes_the_part() {
    let rig = Rig::new();
    let st = rig.storage();
    mount(&st);
    assert_eq!(st.image_upload_begin(b"big", 0), ImageResult::Ok);
    let chunk = pattern(64 * 1024);
    for _ in 0..8 {
        assert_eq!(st.image_upload_write(&chunk), ImageResult::Ok);
    }
    assert_eq!(
        part_size(&rig.dev, "/stm/big.bin.part"),
        Some(MAX_IMAGE_SIZE)
    );
    assert_eq!(st.image_upload_write(&chunk[..1]), ImageResult::TooLarge);
    assert!(!has_file(&rig.dev, "/stm/big.bin.part"));
    assert!(!st.image_upload_active());
    assert_eq!(rig.dev.fs.open_handles(), 0);
}

#[test]
fn upload_a_one_byte_image_is_written_an_empty_one_is_refused() {
    let rig = Rig::new();
    let st = rig.storage();
    mount(&st);
    assert_eq!(st.image_upload_begin(b"one", 1), ImageResult::Ok);
    assert_eq!(st.image_upload_write(b"z"), ImageResult::Ok);
    assert_eq!(st.image_upload_end().unwrap().size, 1);
    assert_eq!(file(&rig.dev, "/stm/one.bin").unwrap(), b"z");
    assert_eq!(st.image_upload_begin(b"none", 1), ImageResult::Ok);
    assert_eq!(st.image_upload_write(b""), ImageResult::Ok);
    assert_eq!(st.image_upload_end(), Err(ImageResult::Empty));
    assert!(!has_file(&rig.dev, "/stm/none.bin.part"));
    assert!(!has_file(&rig.dev, "/stm/none.bin"));
    assert!(!st.image_upload_active());
}

#[test]
fn upload_a_failed_write_removes_the_part() {
    let rig = Rig::new();
    let st = rig.storage();
    mount(&st);
    assert_eq!(st.image_upload_begin(b"w", 10), ImageResult::Ok);
    rig.dev.fs.fail("write", "/stm/w.bin.part", 1);
    assert_eq!(st.image_upload_write(b"abc"), ImageResult::Io);
    assert!(!has_file(&rig.dev, "/stm/w.bin.part"));
    assert!(!st.image_upload_active());
}

#[test]
fn upload_a_failed_rename_drops_the_replaced_image_and_the_part() {
    let rig = Rig::new();
    rig.dev.fs.put("/stm/a.bin", b"old");
    rig.dev.fs.put("/stm/b.bin", b"bb");
    let st = rig.storage();
    mount(&st);
    assert_eq!(st.image_upload_begin(b"a", 10), ImageResult::Ok);
    assert_eq!(st.image_upload_write(b"new"), ImageResult::Ok);
    rig.dev.fs.fail("rename", "/stm/a.bin.part", 1);
    assert_eq!(st.image_upload_end(), Err(ImageResult::Io));
    assert!(!has_file(&rig.dev, "/stm/a.bin"));
    assert!(!has_file(&rig.dev, "/stm/a.bin.part"));
    assert!(st.find_image(b"a").is_none());
    assert!(st.find_image(b"b").is_some());
    assert!(!st.image_upload_active());
}

#[test]
fn upload_a_replacement_counts_only_the_other_images() {
    let rig = Rig::new();
    for n in ["a", "m", "z"] {
        rig.dev.fs.put(&format!("/stm/{n}.bin"), b"x");
    }
    let st = rig.storage();
    mount(&st);
    assert_eq!(st.image_upload_begin(b"a", 10), ImageResult::Ok);
    st.image_upload_abort();
    assert_eq!(st.image_upload_begin(b"q", 10), ImageResult::TooMany);
}

#[test]
fn delete_file_the_part_of_the_running_upload_is_protected_others_are_not() {
    let rig = Rig::new();
    let st = rig.storage();
    mount(&st);
    rig.dev.fs.put("/stm/x.bin.part", b"x");
    assert_eq!(st.delete_file(b"/stm/x.bin.part"), FileResult::Ok);
    assert_eq!(st.image_upload_begin(b"up", 10), ImageResult::Ok);
    assert_eq!(st.delete_file(b"/stm/up.bin.part"), FileResult::Protected);
    assert!(has_file(&rig.dev, "/stm/up.bin.part"));
    rig.dev.fs.put("/stm/old.bin.part", b"o");
    assert_eq!(st.delete_file(b"/stm/old.bin.part"), FileResult::Ok);
    assert!(!has_file(&rig.dev, "/stm/old.bin.part"));
}

#[test]
fn delete_file_the_part_of_the_running_last_good_copy_is_protected() {
    let rig = Rig::new();
    let img = pattern(20000);
    rig.dev.fs.put("/stm/fw.bin", &img);
    let st = rig.storage();
    mount(&st);
    // the part of a copy that does not run is a leftover
    rig.dev.fs.put(PART, b"x");
    assert_eq!(st.delete_file(PART.as_bytes()), FileResult::Ok);
    st.request_last_good_copy(b"fw");
    st.service();
    assert_eq!(part_size(&rig.dev, PART), Some(8192));
    assert_eq!(st.delete_file(PART.as_bytes()), FileResult::Protected);
    rig.dev.fs.put("/stm/x.bin.part", b"x");
    assert_eq!(st.delete_file(b"/stm/x.bin.part"), FileResult::Ok);
    st.service();
    st.service();
    assert_eq!(file(&rig.dev, LAST_GOOD).unwrap(), img);
    assert_eq!(st.delete_file(PART.as_bytes()), FileResult::NotFound);
}

#[test]
fn last_good_copy_a_part_that_cannot_be_created_starts_no_copy() {
    let rig = Rig::new();
    rig.dev.fs.put("/stm/fw.bin", &pattern(20000));
    let st = rig.storage();
    mount(&st);
    rig.dev.fs.fail("open", PART, 1);
    st.request_last_good_copy(b"fw");
    st.service();
    assert!(!has_file(&rig.dev, PART));
    assert_eq!(rig.dev.fs.open_handles(), 0);
    assert_eq!(st.delete_image(b"fw"), ImageResult::Ok); // no copy holds it
}

// ---------------------------------------------------------------- delete_image

#[test]
fn delete_image_a_vanished_file_leaves_the_index_long_names() {
    let rig = Rig::new();
    rig.dev.fs.put("/stm/a.bin", b"a");
    let st = rig.storage();
    mount(&st);
    assert!(rig.dev.fs.remove("/stm/a.bin"));
    assert_eq!(st.delete_image(b"a"), ImageResult::Ok);
    assert!(st.find_image(b"a").is_none());
    // C++ deleteImage(nullptr) -> BadName: no Rust form; the empty name names no image
    assert_eq!(st.delete_image(b""), ImageResult::NotFound);
    // "/stm/<name>.bin" must fit 48 bytes.
    assert_eq!(
        st.delete_image("n".repeat(38).as_bytes()),
        ImageResult::NotFound
    );
    assert_eq!(
        st.delete_image("n".repeat(39).as_bytes()),
        ImageResult::BadName
    );
}

// ---------------------------------------------------------------- FileImage

#[test]
fn file_image_names_the_end_of_the_file_seeks_only_when_needed() {
    let rig = Rig::new();
    let st = rig.storage();
    mount(&st);
    let n38 = "f".repeat(38);
    let n39 = "f".repeat(39);
    rig.dev.fs.put(&format!("/stm/{n38}.bin"), b"0123456789");
    rig.dev.fs.put(&format!("/stm/{n39}.bin"), b"0123456789");
    let mut img = FileImage::new(rig.dev.fs.clone(), &rig.dev.heap);
    // C++ open(nullptr): no Rust form; the empty name opens nothing
    assert!(!img.open(b""));
    assert!(!img.open(n39.as_bytes()));
    assert!(img.open(n38.as_bytes()));
    let mut out = [0u8; 8];
    rig.dev.fs.fail("seek", "", 1); // armed until the first seek
    assert!(img.read(0, &mut out[..4])); // at the start after open: no seek
    assert_eq!(&out[..4], b"0123");
    assert!(!img.read(6, &mut out[..5])); // past the end: refused before any seek
    assert!(img.read(4, &mut out[..2])); // sequential: no seek
    assert_eq!(&out[..2], b"45");
    assert!(!img.read(2, &mut out[..2])); // the seek fails
    assert!(img.read(10, &mut out[..0]));
    assert!(!img.read(11, &mut out[..0]));
    rig.dev.fs.fail("read", "", 1);
    assert!(!img.read(2, &mut out[..2]));
    assert!(img.read(2, &mut out[..2]));
    assert_eq!(&out[..2], b"23");
    img.close();
    assert_eq!(rig.dev.fs.open_handles(), 0);
    assert!(!img.read(0, &mut out[..0]));
}

// ---------------------------------------------------------------- Rust (new)

#[test]
fn upload_without_a_file_system_nothing_runs() {
    let rig = Rig::new();
    let st = rig.storage();
    assert_eq!(st.image_upload_begin(b"x", 1), ImageResult::Io);
    assert_eq!(st.image_upload_write(b"x"), ImageResult::Io);
    assert_eq!(st.image_upload_end(), Err(ImageResult::Io));
    st.image_upload_abort();
    assert!(!st.image_upload_active());
}

#[test]
fn upload_a_part_that_cannot_be_created_is_an_io_error() {
    let rig = Rig::new();
    let st = rig.storage();
    mount(&st);
    rig.dev.fs.fail("open", "/stm/x.bin.part", 1);
    assert_eq!(st.image_upload_begin(b"x.bin", 1), ImageResult::Io);
    assert!(!st.image_upload_active());
    assert_eq!(st.image_upload_begin(b"x.bin", 1), ImageResult::Ok);
}

#[test]
fn upload_a_name_is_read_as_a_c_string() {
    let rig = Rig::new();
    let st = rig.storage();
    mount(&st);
    assert_eq!(st.image_upload_begin(b"fw\0junk", 1), ImageResult::Ok);
    assert_eq!(st.image_upload_write(b"1"), ImageResult::Ok);
    assert_eq!(st.image_upload_end().unwrap().name.as_slice(), b"fw");
    assert!(st.find_image(b"fw\0x").is_some());
    assert_eq!(st.delete_image(b"fw\0y"), ImageResult::Ok);
}

#[test]
fn upload_the_crc_runs_over_every_write() {
    let rig = Rig::new();
    let st = rig.storage();
    mount(&st);
    let data = pattern(3000);
    assert_eq!(st.image_upload_begin(b"c", 3000), ImageResult::Ok);
    for part in data.chunks(1460) {
        assert_eq!(st.image_upload_write(part), ImageResult::Ok);
    }
    let info = st.image_upload_end().unwrap();
    assert_eq!(info.crc, crc32(&data, 0));
    assert_eq!(info.size, 3000);
    assert_eq!(st.find_image(b"c").unwrap().size, 3000);
}

#[test]
fn upload_end_without_an_index_slot_removes_the_image() {
    // C++ "cannot happen": a remount during the upload fills the index
    let rig = Rig::new();
    for n in ["a", "b", "last_good"] {
        rig.dev.fs.put(&format!("/stm/{n}.bin"), b"x");
    }
    let st = rig.storage();
    mount(&st);
    assert_eq!(st.image_upload_begin(b"d", 1), ImageResult::Ok);
    rig.dev.fs.put("/stm/e.bin", b"x");
    mount(&st); // indexes e into the last slot
    assert_eq!(st.image_upload_write(b"1"), ImageResult::Ok);
    assert_eq!(st.image_upload_end(), Err(ImageResult::TooMany));
    assert!(!has_file(&rig.dev, "/stm/d.bin"));
    assert!(!st.image_upload_active());
}

#[test]
fn last_good_copy_continues_with_the_next_chunks_and_needs_no_new_request() {
    let rig = Rig::new();
    let img = pattern(8192);
    rig.dev.fs.put("/stm/fw.bin", &img);
    let st = rig.storage();
    mount(&st);
    st.request_last_good_copy(b"fw\0x");
    st.service(); // eight chunks: all bytes, the end is seen next time
    assert_eq!(part_size(&rig.dev, PART), Some(8192));
    st.service();
    assert_eq!(file(&rig.dev, LAST_GOOD).unwrap(), img);
}

#[test]
fn last_good_copy_of_a_missing_image_starts_nothing() {
    let rig = Rig::new();
    let st = rig.storage();
    mount(&st);
    st.request_last_good_copy(b"gone");
    st.service();
    assert_eq!(rig.dev.fs.knobs().write_opens, 0);
    st.service(); // the request was used up
    assert_eq!(rig.dev.fs.knobs().write_opens, 0);
}

#[test]
fn scan_an_image_that_cannot_be_opened_is_recorded_as_a_read_error() {
    let rig = Rig::new();
    rig.dev.fs.put("/stm/a.bin", b"");
    let st = rig.storage();
    mount(&st);
    assert!(rig.dev.fs.remove("/stm/a.bin"));
    st.service(); // the size of an image that is gone reads 0, as indexed
    let e = st.find_image(b"a").unwrap();
    assert!(e.scanned);
    assert_eq!(e.check, FlashError::ImageRead);
}

#[test]
fn scan_waits_for_a_running_copy_and_without_a_file_system_nothing_runs() {
    let rig = Rig::new();
    rig.dev.fs.put("/stm/a.bin", &stm_image(true));
    let st = rig.storage();
    st.service(); // not mounted
    mount(&st);
    st.request_last_good_copy(b"a");
    st.service(); // the copy (4 KiB, all of it), not the scan
    assert!(has_file(&rig.dev, LAST_GOOD));
    assert!(!scanned(&st, b"a"));
    st.service();
    assert!(scanned(&st, b"a"));
    assert!(!scanned(&st, b"last_good"));
}

#[test]
fn delete_image_a_removal_that_fails_on_an_existing_image_is_an_io_error() {
    let rig = Rig::new();
    rig.dev.fs.put("/stm/a.bin", b"a");
    let st = rig.storage();
    mount(&st);
    rig.dev.fs.fail("remove", "/stm/a.bin", 1);
    assert_eq!(st.delete_image(b"a"), ImageResult::Io);
    assert!(has_file(&rig.dev, "/stm/a.bin"));
    assert!(st.find_image(b"a").is_some());
    assert_eq!(st.delete_image(b"a"), ImageResult::Ok);
}

#[test]
fn file_image_a_second_open_closes_the_first_file() {
    let rig = Rig::new();
    let st = rig.storage();
    mount(&st);
    rig.dev.fs.put("/stm/a.bin", b"aaaa");
    rig.dev.fs.put("/stm/b.bin", b"bb");
    let mut img = FileImage::new(&rig.dev.fs, &rig.dev.heap);
    assert!(img.open(b"a"));
    assert!(img.open(b"b"));
    assert_eq!(rig.dev.fs.open_handles(), 1);
    assert_eq!(img.size(), 2);
    let mut out = [0u8; 2];
    assert!(img.read(0, &mut out));
    assert_eq!(&out, b"bb");
    assert!(!img.open(b"missing"));
    assert_eq!(img.size(), 0);
    assert_eq!(rig.dev.fs.open_handles(), 0);
}
