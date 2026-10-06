//! Self-test of the harness (the subjects of the C++ `selftest.cpp`, rewritten for the Rust
//! fakes): isolation, multi-boot hand-over of the persistent stores, the invariants, and the
//! behaviour of the fakes the glue relies on. The C++ "xfail" cases are `should_panic` here.
//!
//! Subjects of `selftest.cpp` without a Rust form: the FreeRTOS queue and mutex fakes and the
//! critical-section and `xTaskGetHandle(nullptr)` xfails (the glue shares data through std types,
//! design 2.2); the sibling fakes (none, design 5.1); `File::setBufferSize` (no stdio layer);
//! Preferences return values (the NVS port's own contract is tested instead); the
//! AsyncWebServer driver cases (header filtering, 1460-byte pieces, pipelined bytes, multipart
//! framing, `failNextResponse`, recycled request addresses: library quirks retired by design
//! 5.2, the parsing lives in `http_parse`); the use-after-publish of a PubSubClient callback
//! (the borrow checker rules it out); PENDING_VERIFY and the bootloader rollback (the devices'
//! bootloader has none: the fake bootloader keeps the selection, see the `ota_` cases). The
//! fake STM (`support/fake_stm.cpp`) comes with the stm_link port: it needs the golden replies
//! and the AN3155 simulator of the core's test support.

use super::board::{Booted, APP_A, APP_B, APP_CPP};
use super::broker::publish_packet;
use super::net::TcpPeer;
use super::*;
use crate::port::*;

fn read_all<F: FsFile>(f: &mut F) -> Vec<u8> {
    let mut out = Vec::new();
    let mut buf = [0u8; 7];
    loop {
        let n = f.read(&mut buf);
        if n == 0 {
            return out;
        }
        out.extend_from_slice(&buf[..n]);
    }
}

// ---------------------------------------------------------------- isolation and boots

#[test]
fn isolation_two_boards_share_nothing() {
    let a = FakeBoard::new();
    let b = FakeBoard::new();
    a.nvs().set_u8("vdmrev", "k", 1);
    a.fs().put("/x", b"1");
    a.rtc().store(0, &[1]);
    assert!(!b.nvs().has("vdmrev", "k"));
    assert_eq!(b.fs().read("/x"), None);
    assert_eq!(b.rtc().snapshot()[0], 0xA5);
}

#[test]
fn boots_keep_nvs_littlefs_and_rtc_across_a_software_reset() {
    let board = FakeBoard::new();
    let dev = board.boot();
    assert_eq!(dev.system.reset_reason(), 1); // ESP_RST_POWERON
    let mut word = [0u8; 4];
    dev.rtc.load(16, &mut word);
    assert_eq!(word, [0xA5; 4]); // power-on content
    dev.rtc.store(16, &0x1234_5678u32.to_le_bytes());
    dev.nvs.set_u32("vdmrev", "boots", 7);
    assert!(dev.fs.mount());
    dev.fs.put("/log/events.log", b"line\n");
    assert_eq!(run(|| dev.system.restart()), Ended::Reset(Reset::Software));
    drop(dev);
    let dev = board.boot();
    assert_eq!(dev.system.reset_reason(), 3); // ESP_RST_SW
    dev.rtc.load(16, &mut word);
    assert_eq!(u32::from_le_bytes(word), 0x1234_5678);
    assert_eq!(dev.nvs.get_i("vdmrev", "boots"), 7);
    assert_eq!(dev.fs.read("/log/events.log").unwrap(), b"line\n");
    assert!(!dev.fs.exists("/log/events.log")); // not mounted: the firmware mounts at every boot
    assert_eq!(dev.journal.entries(), Vec::<String>::new()); // a new journal per boot
    assert_eq!(board.history().len(), 2);
}

#[test]
fn boots_a_power_on_clears_the_rtc_and_keeps_nvs_and_flash() {
    let board = FakeBoard::new();
    let dev = board.boot();
    dev.rtc.store(0, &[1, 2, 3]);
    dev.nvs.set_u8("vdmrev", "imported", 1);
    board.reset(Reset::PowerOn);
    drop(dev);
    let dev = board.boot();
    assert_eq!(dev.system.reset_reason(), 1);
    assert_eq!(dev.rtc.snapshot(), vec![0xA5; system::RTC_SIZE]);
    assert_eq!(dev.nvs.get_i("vdmrev", "imported"), 1);
}

#[test]
fn boots_pin_panic_and_watchdog_resets_keep_the_rtc() {
    let board = FakeBoard::new();
    let expect = [(Reset::Pin, 2u8), (Reset::Panic, 4), (Reset::TaskWdt, 6)];
    let mut dev = board.boot();
    dev.rtc.store(0, &[9]);
    for (kind, reason) in expect {
        assert_eq!(run(|| board.crash(kind)), Ended::<()>::Reset(kind));
        drop(dev);
        dev = board.boot();
        assert_eq!(dev.system.reset_reason(), reason);
        assert_eq!(dev.system.reset_kind(), kind);
        assert_eq!(dev.rtc.snapshot()[0], 9);
    }
    assert_eq!(run(|| 5), Ended::Returned(5));
    assert_eq!(run(|| 5).returned(), 5);
}

#[test]
fn boots_a_reset_from_outside_replaces_the_pending_one() {
    let board = FakeBoard::new();
    board.rtc().store(0, &[7]);
    board.reset(Reset::Pin); // a warm first boot
    let dev = board.boot();
    assert_eq!((dev.system.reset_reason(), dev.rtc.snapshot()[0]), (2, 7));
    assert_eq!(run(|| dev.system.restart()), Ended::Reset(Reset::Software));
    board.reset(Reset::PowerOn); // the power went while it restarted
    drop(dev);
    let dev = board.boot();
    assert_eq!((dev.reset, dev.rtc.snapshot()[0]), (Reset::PowerOn, 0xA5));
}

#[test]
#[should_panic(expected = "while the last boot runs")]
fn xfail_a_boot_before_the_last_one_ended() {
    let board = FakeBoard::new();
    let _dev = board.boot();
    let _again = board.boot();
}

#[test]
#[should_panic(expected = "more than 8 boots")]
fn xfail_more_than_eight_boots() {
    let board = FakeBoard::new();
    for _ in 0..9 {
        let _dev = board.boot();
        board.reset(Reset::Software);
    }
}

#[test]
#[should_panic(expected = "instead of returning")]
fn xfail_returned_of_a_reset() {
    let board = FakeBoard::new();
    let dev = board.boot();
    run(|| dev.system.restart()).returned();
}

#[test]
fn run_passes_other_panics_on() {
    let r = std::panic::catch_unwind(|| run(|| panic!("boom")));
    assert!(r.is_err());
}

// ---------------------------------------------------------------- OTA and the bootloader

#[test]
fn ota_an_update_boots_at_the_next_reset_and_stays() {
    let board = FakeBoard::new();
    let dev = board.boot();
    assert_eq!(dev.ota.running().address, 0x1_0000);
    assert_eq!(dev.ota.running().app, Some(APP_A));
    let other = dev.ota.other().unwrap();
    assert_eq!(
        (other.address, other.size, other.app),
        (0x15_0000, 0x14_0000, None)
    );
    dev.ota.knobs().next_app = Some(APP_B);
    let mut u = dev.ota.begin().unwrap();
    assert_eq!(u.write(&[0xE9, 1, 2]), Ok(()));
    assert_eq!(u.write(&[3]), Ok(()));
    assert_eq!(u.finish(), Ok(()));
    assert_eq!(dev.ota.knobs().written, vec![0xE9, 1, 2, 3]);
    assert_eq!(dev.ota.store().otadata, 1);
    board.reset(Reset::Software);
    drop(dev);
    let dev = board.boot();
    assert_eq!(dev.ota.running().app, Some(APP_B));
    assert_eq!(dev.ota.other().unwrap().app, Some(APP_A));
    board.reset(Reset::Panic); // no rollback support: the new image stays
    drop(dev);
    assert_eq!(board.boot().ota.running().app, Some(APP_B));
    assert_eq!(
        board.history().iter().map(|b| b.slot).collect::<Vec<_>>(),
        vec![0, 1, 1]
    );
}

#[test]
fn ota_update_failures_and_the_derived_app_id() {
    let board = FakeBoard::with_slots([SlotImage::glue(APP_A), SlotImage::glue(APP_B)], 0);
    let dev = board.boot();
    let mut u = dev.ota.begin().unwrap();
    assert_eq!(u.write(&[0x00, 1]), Err(EspErr::OTA_VALIDATE_FAILED)); // magic byte
    assert_eq!(dev.ota.other().unwrap().app, Some(APP_B)); // nothing written yet
    assert_eq!(u.write(&[0xE9, 1]), Ok(()));
    assert_eq!(dev.ota.other().unwrap().app, None); // erased as written
    u.abort();
    assert_eq!(dev.ota.knobs().aborts, 1);
    dev.ota.knobs().fail_write_at = Some(3);
    let mut u = dev.ota.begin().unwrap();
    assert_eq!(u.write(&[0xE9, 1, 2]), Ok(()));
    assert_eq!(u.write(&[3]), Err(EspErr::FLASH_OP_FAIL));
    dev.ota.knobs().fail_write_at = None;
    assert_eq!(u.finish(), Ok(()));
    let app = ota::derived_app(&[0xE9, 1, 2]);
    assert_eq!(dev.ota.other().unwrap().app, Some(app));
    dev.ota.knobs().finish_err = Some(EspErr::OTA_VALIDATE_FAILED);
    let mut u = dev.ota.begin().unwrap();
    u.write(&[0xE9]).unwrap();
    assert_eq!(u.finish(), Err(EspErr::OTA_VALIDATE_FAILED));
    dev.ota.knobs().finish_err = None;
    assert_eq!(
        dev.ota.begin().unwrap().finish(),
        Err(EspErr::OTA_VALIDATE_FAILED)
    ); // empty
    dev.ota.knobs().begin_err = Some(EspErr::NO_MEM);
    assert!(dev.ota.begin().is_err());
    dev.ota.knobs().no_other = true;
    assert_eq!(dev.ota.other(), None);
    assert_eq!(dev.ota.running_image_size(), 1_091_984);
    assert_eq!(
        dev.journal.of("ota "),
        vec![
            "ota begin",
            "ota abort",
            "ota begin",
            "ota finish",
            "ota begin",
            "ota finish",
            "ota begin",
            "ota finish",
            "ota begin"
        ]
    );
}

#[test]
fn ota_set_boot_verifies_and_the_bootloader_falls_back() {
    let board = FakeBoard::with_slots([SlotImage::glue(APP_A), SlotImage::broken(Some(APP_B))], 0);
    let dev = board.boot();
    assert_eq!(
        dev.ota.set_boot(0x15_0000),
        Err(EspErr::OTA_VALIDATE_FAILED)
    );
    assert_eq!(dev.ota.set_boot(0x12_3456), Err(EspErr::INVALID_ARG));
    dev.ota.knobs().set_boot_err = Some(EspErr::FAIL);
    assert_eq!(dev.ota.set_boot(0x1_0000), Err(EspErr::FAIL));
    dev.ota.knobs().set_boot_err = None;
    assert_eq!(dev.ota.set_boot(0x1_0000), Ok(()));
    assert_eq!(
        dev.ota.knobs().set_boots,
        vec![0x15_0000, 0x12_3456, 0x1_0000, 0x1_0000]
    );
    // otadata on a slot that does not load: the bootloader takes the other valid one
    dev.ota.store().otadata = 1;
    board.reset(Reset::Software);
    drop(dev);
    assert_eq!(board.boot().ota.running().app, Some(APP_A));
    board.reset(Reset::Software);
    // a foreign image ends the case; no image at all is a reflash
    board.ota().store().slots[1] = SlotImage::foreign(APP_CPP);
    board.ota().store().otadata = 1;
    assert!(matches!(board.boot_any(), Booted::Foreign(a) if a == APP_CPP));
    board.reset(Reset::Software);
    board.ota().store().slots = [SlotImage::EMPTY; 2];
    assert!(matches!(board.boot_any(), Booted::NoImage));
    assert_eq!(board.history().last().unwrap().app, None);
}

#[test]
#[should_panic(expected = "foreign image")]
fn xfail_boot_of_a_foreign_image() {
    let board = FakeBoard::with_slots([SlotImage::foreign(APP_CPP), SlotImage::EMPTY], 0);
    board.boot();
}

#[test]
#[should_panic(expected = "no slot holds a bootable image")]
fn xfail_boot_without_an_image() {
    let board = FakeBoard::with_slots([SlotImage::EMPTY, SlotImage::EMPTY], 0);
    board.boot();
}

// ---------------------------------------------------------------- time and heap

#[test]
fn clock_sleep_advances_and_a_loop_ends_after_stop_after_sleeps() {
    let clock = FakeClock::default();
    clock.stop_after_sleeps(3);
    let loops = std::cell::Cell::new(0);
    let ended = run(|| loop {
        loops.set(loops.get() + 1);
        clock.sleep_ms(100);
    });
    assert_eq!(ended, Ended::Stopped);
    assert_eq!(loops.get(), 3);
    assert_eq!(clock.now_ms(), 300);
    assert_eq!(clock.sleeps(), vec![100, 100, 100]);
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let s = seen.clone();
    clock.on_sleep(move |ms| s.lock().unwrap().push(ms));
    clock.sleep_ms(0);
    clock.sleep_ms(7);
    assert_eq!(*seen.lock().unwrap(), vec![0, 7]);
    assert_eq!(clock.ms(), 307);
}

#[test]
fn clock_millis_wraps_at_32_bits() {
    let clock = FakeClock::default();
    clock.set_ms(0xFFFF_F000);
    clock.advance_ms(0x1000 + 5);
    assert_eq!(clock.now_ms(), 5);
    assert_eq!(clock.uptime_s(), ((0xFFFF_F000u64 + 0x1005) / 1000) as u32);
    clock.advance_us(1500);
    assert_eq!(clock.us() % 1_000_000, 302_500);
}

#[test]
fn heap_follows_the_script_then_fail_all() {
    let heap = FakeHeap::default();
    heap.state().next.extend([true, false]);
    assert!(heap.grant(4));
    assert!(!heap.grant(10));
    heap.state().fail_all = true;
    assert!(!heap.grant(8));
    heap.state().fail_all = false;
    assert!(heap.grant(20));
    assert_eq!(heap.state().granted, vec![4, 20]);
    assert_eq!(heap.state().refused, vec![10, 8]);
}

// ---------------------------------------------------------------- NVS and LittleFS

#[test]
fn nvs_typed_lookup_lengths_small_buffers_and_read_only_opens() {
    let board = FakeBoard::new();
    let dev = board.boot();
    dev.nvs.set_u32("legacy", "n", 5);
    dev.nvs.set_i8("legacy", "neg", -3);
    dev.nvs.set_i16("legacy", "i16", -300);
    dev.nvs.set_u16("legacy", "u16", 65535);
    dev.nvs.set_i32("legacy", "i32", -70_000);
    dev.nvs.set_i64("legacy", "i64", -(1 << 40));
    dev.nvs.set_u64("legacy", "u64", 1);
    dev.nvs.set_blob("legacy", "b", &[1, 2, 3]);
    dev.nvs.set_str("legacy", "s", b"abc");
    assert!(dev.nvs.open("nothere", false).is_none());
    let ns = dev.nvs.open("legacy", false).unwrap();
    assert_eq!(ns.get_int("n", NvsInt::U8), None); // no conversion
    assert_eq!(ns.get_int("n", NvsInt::U32), Some(5));
    assert_eq!(ns.get_int("neg", NvsInt::I8), Some(-3));
    assert_eq!(ns.get_int("i16", NvsInt::I16), Some(-300));
    assert_eq!(ns.get_int("u16", NvsInt::U16), Some(65535));
    assert_eq!(ns.get_int("i32", NvsInt::I32), Some(-70_000));
    assert_eq!(ns.get_int("i64", NvsInt::I64), Some(-(1 << 40)));
    assert_eq!(ns.get_int("u64", NvsInt::I64), None);
    assert_eq!(ns.get_int("b", NvsInt::U8), None);
    assert_eq!(ns.blob_len("b"), Some(3));
    assert_eq!(ns.blob_len("s"), None);
    let mut small = [0u8; 2];
    assert_eq!(ns.get_blob("b", &mut small), None);
    let mut buf = [0u8; 8];
    assert_eq!(ns.get_blob("b", &mut buf), Some(3));
    assert_eq!(&buf[..3], &[1, 2, 3]);
    assert_eq!(ns.str_len("s"), Some(4)); // with the NUL
    assert_eq!(ns.get_str("s", &mut small), None);
    assert_eq!(ns.get_str("s", &mut buf), Some(3));
    assert_eq!(&buf[..3], b"abc");
    assert_eq!(ns.get_str("b", &mut buf), None);
    assert_eq!(ns.str_len("x"), None);
    let mut ro = ns;
    assert!(!ro.set_int("x", NvsInt::U8, 1)); // read-only
    assert!(!ro.remove("n"));
    assert!(!ro.erase_all());
    drop(ro);
    assert_eq!(dev.nvs.knobs().open_handles, 0);
    assert_eq!(dev.nvs.knobs().opens, 2);
}

#[test]
fn nvs_writes_commit_and_scripted_failures_store_nothing() {
    let board = FakeBoard::new();
    let dev = board.boot();
    let mut ns = dev.nvs.open("vdmrev", true).unwrap(); // created by a write open
    assert!(ns.set_int("a", NvsInt::U8, 0x1FF)); // truncated to the type
    assert_eq!(ns.get_int("a", NvsInt::U8), Some(0xFF));
    assert!(ns.set_int("c", NvsInt::I64, -9));
    assert!(ns.set_blob("d", &[1, 2, 3, 4, 5]));
    assert!(!ns.set_int("toolongkeyname16", NvsInt::U8, 1));
    assert!(!ns.set_int("", NvsInt::U8, 1));
    dev.nvs.knobs().fail_set.insert("e".to_string());
    assert!(!ns.set_int("e", NvsInt::U8, 1));
    assert!(!dev.nvs.has("vdmrev", "e"));
    dev.nvs.knobs().fail_commit = true;
    assert!(!ns.set_blob("d", &[9]));
    assert!(!ns.remove("a"));
    assert!(!ns.erase_all());
    dev.nvs.knobs().fail_commit = false;
    assert_eq!(dev.nvs.get_blob("vdmrev", "d"), vec![1, 2, 3, 4, 5]);
    assert!(ns.remove("a"));
    assert!(!ns.remove("a"));
    dev.nvs.knobs().fail_erase = true;
    assert!(!ns.erase_all());
    dev.nvs.knobs().fail_erase = false;
    assert_eq!(dev.nvs.keys("vdmrev"), vec!["c", "d"]);
    assert!(ns.erase_all());
    assert!(dev.nvs.keys("vdmrev").is_empty());
    drop(ns);
    assert_eq!(dev.nvs.knobs().sets.get("vdmrev/a"), Some(&1));
    assert_eq!(
        dev.journal.entries(),
        vec![
            "nvs set vdmrev/a",
            "nvs set vdmrev/c",
            "nvs set vdmrev/d",
            "nvs remove vdmrev/a",
            "nvs erase vdmrev"
        ]
    );
    dev.nvs.knobs().fail_open.insert("vdmrev".to_string());
    assert!(dev.nvs.open("vdmrev", true).is_none());
    dev.nvs.knobs().fail_open.clear();
    dev.nvs.knobs().init_failed = true;
    assert!(dev.nvs.open("vdmrev", true).is_none());
}

#[test]
fn littlefs_open_modes_parents_rename_remove_capacity() {
    let board = FakeBoard::new();
    let dev = board.boot();
    let fs = &dev.fs;
    assert!(fs.open("/a.txt", OpenMode::Write).is_none()); // not mounted
    assert_eq!(fs.usage(), (0, 0));
    assert!(fs.mount());
    assert!(fs.open("/nodir/x.txt", OpenMode::Write).is_none());
    assert!(fs.open("/missing.txt", OpenMode::Read).is_none());
    let mut f = fs.open("/a.txt", OpenMode::Write).unwrap();
    assert_eq!(f.write(b"abc"), 3);
    assert_eq!(f.write(b""), 0);
    drop(f);
    let mut f = fs.open("/a.txt", OpenMode::Append).unwrap();
    assert_eq!(f.write(b"d"), 1);
    assert_eq!(f.size(), 4);
    drop(f);
    assert_eq!(fs.read("/a.txt").unwrap(), b"abcd");
    let mut f = fs.open("/a.txt", OpenMode::Read).unwrap();
    assert_eq!(f.write(b"x"), 0);
    assert!(f.seek(2));
    assert!(!f.seek(5));
    assert_eq!(read_all(&mut f), b"cd");
    drop(f);
    drop(fs.open("/a.txt", OpenMode::Write).unwrap());
    assert_eq!(fs.read("/a.txt").unwrap(), b"");
    fs.put("/b.txt", b"B");
    assert!(fs.rename("/b.txt", "/a.txt"));
    assert_eq!(fs.read("/a.txt").unwrap(), b"B");
    assert!(!fs.exists("/b.txt"));
    assert!(!fs.remove("/b.txt"));
    assert!(fs.mkdir("/d"));
    assert!(!fs.mkdir("/d"));
    assert!(!fs.mkdir("/x/y"));
    assert!(fs.open("/d", OpenMode::Read).is_none());
    fs.fail("rename", "/a.txt", 1);
    assert!(!fs.rename("/a.txt", "/c.txt"));
    assert!(fs.rename("/a.txt", "/c.txt"));
    assert!(!fs.rename("/missing", "/c.txt"));
    let (total, used) = fs.usage();
    assert_eq!((total, used), (0x17_0000, 4 * 4096)); // superblocks, /c.txt, /d
    fs.knobs().total_bytes = used + 4096;
    let mut f = fs.open("/big.bin", OpenMode::Write).unwrap(); // takes the last free block
    assert_eq!(f.write(&vec![b'x'; 5000]), 4096);
    assert_eq!(f.write(b"y"), 0);
    drop(f);
    let mut names = Vec::new();
    fs.list("/", &mut |e| {
        names.push((String::from_utf8(e.name.to_vec()).unwrap(), e.size, e.dir));
        true
    });
    assert_eq!(
        names,
        vec![
            ("big.bin".to_string(), 4096, false),
            ("c.txt".to_string(), 1, false),
            ("d".to_string(), 0, true)
        ]
    );
    let mut first = Vec::new();
    fs.list("/", &mut |e| {
        first.push(e.name.to_vec());
        false
    });
    assert_eq!(first, vec![b"big.bin".to_vec()]);
    fs.put("/d/e/f.txt", b"deep");
    let mut sub = Vec::new();
    fs.list("/d", &mut |e| {
        sub.push((e.name.to_vec(), e.dir));
        true
    });
    assert_eq!(sub, vec![(b"e".to_vec(), true)]);
    assert!(!fs.remove("/d")); // not empty
    assert_eq!(fs.knobs().renames, 2);
}

#[test]
fn littlefs_an_open_file_is_busy_and_failures_are_scripted() {
    let board = FakeBoard::new();
    let dev = board.boot();
    let fs = &dev.fs;
    assert!(fs.mount());
    fs.put("/log/events.log", b"1\n");
    let f = fs.open("/log/events.log", OpenMode::Read).unwrap();
    assert_eq!(fs.open_handles(), 1);
    assert!(!fs.rename("/log/events.log", "/log/events.1.log")); // EBUSY
    assert!(!fs.rename("/x", "/log/events.log"));
    assert!(!fs.remove("/log/events.log"));
    drop(f);
    assert!(fs.rename("/log/events.log", "/log/events.1.log"));
    fs.fail("open", "", 2);
    assert!(fs.open("/log/events.1.log", OpenMode::Read).is_none());
    assert!(fs.open("/log/events.1.log", OpenMode::Read).is_none());
    let mut f = fs.open("/log/events.1.log", OpenMode::Read).unwrap();
    fs.fail("read", "/log/events.1.log", 1);
    let mut b = [0u8; 4];
    assert_eq!(f.read(&mut b), 0);
    fs.fail("seek", "", 1);
    assert!(!f.seek(0));
    assert_eq!(f.read(&mut b), 2);
    drop(f);
    let mut f = fs.open("/w", OpenMode::Write).unwrap();
    fs.fail("write", "/w", 1);
    assert_eq!(f.write(b"a"), 0);
    assert_eq!(f.write(b"a"), 1);
    drop(f);
    fs.fail("mkdir", "/m", 1);
    assert!(!fs.mkdir("/m"));
    fs.fail("remove", "/w", 1);
    assert!(!fs.remove("/w"));
    assert!(fs.remove("/w"));
    assert_eq!(fs.knobs().removes, 1);
    fs.knobs().format_ok = false;
    assert!(!fs.format());
    fs.knobs().format_ok = true;
    assert!(fs.format());
    assert!(fs.paths().is_empty());
    fs.set_formatted(false);
    assert!(!fs.mount());
    fs.set_formatted(true);
    fs.knobs().mount_ok = false;
    assert!(!fs.mount());
    assert_eq!(fs.knobs().mounts, 3);
    assert_eq!(
        dev.journal.of("fs "),
        vec![
            "fs mount true",
            "fs rename /log/events.log /log/events.1.log",
            "fs remove /w",
            "fs format",
            "fs format",
            "fs mount false",
            "fs mount false"
        ]
    );
}

#[test]
#[should_panic(expected = "a LittleFS file is still open")]
fn xfail_a_file_left_open_at_the_end_of_a_boot() {
    let board = FakeBoard::new();
    let dev = board.boot();
    assert!(dev.fs.mount());
    let f = dev.fs.open("/x", OpenMode::Write).unwrap();
    std::mem::forget(f);
    drop(dev);
}

// ---------------------------------------------------------------- UART, GPIO, chip

#[test]
fn uart_bytes_arrive_at_their_time_and_a_full_ring_drops_the_rest() {
    let clock = FakeClock::default();
    let mut uart = FakeUart::new(clock.clone());
    let mut buf = vec![0u8; 4096];
    assert_eq!(uart.write(b"x"), 0); // not open
    uart.inject(b"old", None);
    uart.configure(115_200, false); // drops what arrived before
    assert_eq!(uart.read(&mut buf), 0);
    uart.inject(b"ab", Some(10));
    assert_eq!(uart.available(), 0);
    clock.advance_ms(10);
    assert_eq!(uart.read(&mut buf), 2);
    assert_eq!(&buf[..2], b"ab");
    uart.inject(&vec![b'x'; 3000], None);
    assert_eq!(uart.read(&mut buf), 2048);
    assert_eq!(uart.state().rx_dropped, 952);
    uart.state().tx_accept = 3;
    assert_eq!(uart.write(b"hello"), 3);
    assert_eq!(uart.take_tx(), b"hel");
    assert_eq!(uart.take_tx(), b"");
    uart.configure(115_200, true);
    let s = uart.state();
    assert_eq!((s.baud, s.even_parity, s.configures), (115_200, true, 2));
    drop(s);
}

struct Echo;
impl uart::SerialPeer for Echo {
    fn on_tx(&mut self, data: &[u8], now_ms: u64, rx: &mut uart::UartRx) {
        rx.inject_at(now_ms + 3, data);
    }
}

#[test]
fn uart_a_peer_answers_after_its_delay() {
    let clock = FakeClock::default();
    let mut uart = FakeUart::new(clock.clone());
    uart.attach(Box::new(Echo));
    uart.configure(115_200, false);
    assert_eq!(uart.write(b"gproto\r\n"), 8);
    let mut buf = [0u8; 16];
    assert_eq!(uart.read(&mut buf), 0);
    clock.advance_ms(3);
    assert_eq!(uart.read(&mut buf), 8);
}

#[test]
fn gpio_writes_are_journaled_and_inputs_follow_the_time() {
    let clock = FakeClock::default();
    let journal = Journal::default();
    let gpio = FakeGpio::new(clock.clone(), journal.clone());
    let mut nrst = gpio.output(15);
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let s = seen.clone();
    gpio.on_write(move |p, l| s.lock().unwrap().push((p, l)));
    nrst.set(false);
    nrst.set(true);
    assert_eq!(gpio.level(15), Some(true));
    assert_eq!(gpio.level(14), None);
    assert_eq!(gpio.writes(), vec![(15, false), (15, true)]);
    assert_eq!(*seen.lock().unwrap(), vec![(15, false), (15, true)]);
    assert_eq!(journal.entries(), vec!["gpio 15=0", "gpio 15=1"]);
    let mut pin = gpio.input(2);
    assert!(!pin.is_low()); // pull-up
    gpio.set_input(2, |now| now < 5000);
    assert!(pin.is_low());
    clock.advance_ms(5000);
    assert!(!pin.is_low());
}

#[test]
fn chip_heap_stacks_mac_console_watchdog() {
    let board = FakeBoard::new();
    let dev = board.boot();
    assert_eq!(
        dev.system.heap(),
        HeapStats {
            free: 150_000,
            min_free: 120_000,
            largest: 110_000
        }
    );
    assert_eq!(dev.system.stack_min_free("stm"), None);
    dev.system.state().stacks.insert("stm".to_string(), 4000);
    assert_eq!(dev.system.stack_min_free("stm"), Some(4000));
    assert_eq!(dev.system.base_mac(), [0x24, 0x0A, 0xC4, 0x12, 0x34, 0x56]);
    dev.console.line(b"boot");
    assert_eq!(dev.console.lines(), vec![b"boot".to_vec()]);
    dev.watchdog.feed();
    dev.watchdog.feed();
    assert_eq!(dev.watchdog.feeds(), 2);
    assert_eq!(run(|| dev.system.restart()), Ended::Reset(Reset::Software));
    assert_eq!(dev.system.state().restarts, 1);
    assert_eq!(dev.journal.entries(), vec!["esp_restart"]);
}

#[test]
#[should_panic(expected = "RTC store out of range")]
fn xfail_rtc_access_out_of_range() {
    let board = FakeBoard::new();
    board.rtc().store(system::RTC_SIZE - 1, &[1, 2]);
}

#[test]
#[should_panic(expected = "RTC load out of range")]
fn xfail_rtc_load_out_of_range() {
    let board = FakeBoard::new();
    board.rtc().load(system::RTC_SIZE, &mut [0]);
}

// ---------------------------------------------------------------- network

#[test]
fn network_interfaces_sntp_udp_and_ping() {
    let mut eth = FakeEthernet::default();
    let setup = IpSetup {
        hostname: "vdmot",
        fixed: None,
    };
    assert!(eth.begin(&setup));
    eth.state().begin_ok = false;
    assert!(!eth.begin(&setup));
    assert_eq!(eth.state().begins.len(), 2);
    assert_eq!(eth.state().begins[0].hostname, "vdmot");
    let info = IpInfo {
        ip: 0x0701_A8C0,
        mask: 0x00FF_FFFF,
        gateway: 0x0101_A8C0,
        dns: 0x0101_A8C0,
    };
    eth.got_ip(info);
    assert!(eth.link() && eth.has_ip());
    assert_eq!((eth.got_ip_count(), eth.info()), (1, info));
    eth.link_down();
    assert!(!eth.link() && !eth.has_ip());
    assert!(eth.restart());
    eth.state().restart_ok = false;
    assert!(!eth.restart());
    assert_eq!(eth.state().restarts, 2);
    assert_eq!(eth.mac(), [0xA8, 0x03, 0x2A, 0xA1, 0xB2, 0xC3]);

    let mut wifi = FakeWifi::default();
    assert!(wifi.begin(&setup));
    wifi.connect(b"net", b"pw");
    wifi.reconnect();
    wifi.got_ip(info);
    assert!(wifi.up());
    assert_eq!(wifi.got_ip_count(), 1);
    assert!(!wifi.take_unrequested_disconnect());
    wifi.drop_station();
    assert!(!wifi.up());
    assert!(wifi.take_unrequested_disconnect());
    assert!(!wifi.take_unrequested_disconnect());
    wifi.stop();
    let s = wifi.state();
    assert_eq!((s.reconnects, s.stops), (1, 1));
    drop(s);
    assert_eq!(
        wifi.state().connects,
        vec![(b"net".to_vec(), b"pw".to_vec())]
    );
    assert_eq!((wifi.rssi(), wifi.info()), (-60, info));
    assert_eq!(wifi.mac(), [0x24, 0x0A, 0xC4, 0x12, 0x34, 0x56]);

    let mut sntp = FakeSntp::default();
    sntp.configure(Some("pool.ntp.org"));
    sntp.configure(None);
    sntp.sync(1_767_225_600);
    assert_eq!(
        (sntp.sync_count(), sntp.last_sync_epoch()),
        (1, 1_767_225_600)
    );
    assert_eq!(
        sntp.state().configured,
        vec![Some("pool.ntp.org".to_string()), None]
    );

    let mut udp = FakeUdp::default();
    assert!(udp.send_to(1, 514, b"x"));
    udp.set_fail(true);
    assert!(!udp.send_to(1, 514, b"y"));
    assert_eq!(udp.sent(), vec![(1, 514, b"x".to_vec())]);
}

#[test]
fn network_a_ping_session_answers_only_while_started() {
    let mut p = FakePinger::default();
    p.step(); // not started: no answer
    assert!(!p.done());
    p.state().start_ok = false;
    assert!(!p.start(0x0101_A8C0));
    p.step();
    assert!(!p.done());
    p.state().start_ok = true;
    p.state().answers.push_back(false);
    assert!(p.start(0x0101_A8C0));
    p.step();
    assert!(p.done());
    assert_eq!(p.replies(), 0);
    p.delete();
    assert!(!p.done());
    assert!(p.start(0x0101_A8C0));
    p.step();
    p.step(); // one echo per session
    assert_eq!(p.replies(), 1);
    p.delete();
    assert_eq!(p.state().started.len(), 3);
    assert_eq!(p.state().deletes, 2);
}

struct Script(Vec<u8>);
impl TcpPeer for Script {
    fn on_connect(&mut self, wire: &mut net::Wire, now_ms: u64) {
        wire.send_at(now_ms + 5, b"hello");
        wire.close_at(now_ms + 5);
    }
    fn on_data(&mut self, _wire: &mut net::Wire, data: &[u8], _now_ms: u64) {
        self.0.extend_from_slice(data);
    }
}

#[test]
fn tcp_connections_reach_registered_peers_only() {
    let clock = FakeClock::default();
    let tcp = FakeTcp::new(clock.clone(), Journal::default());
    assert!(tcp.connect("nowhere", 80, 3000).is_none());
    assert_eq!(clock.ms(), 3000); // a refused connect takes the timeout
    tcp.listen("server", 80, || Box::new(Script(Vec::new())));
    tcp.set_connect_ms(2);
    tcp.refuse_next(1);
    assert!(tcp.connect("server", 80, 1000).is_none());
    let mut s = tcp.connect("server", 80, 1000).unwrap();
    assert_eq!(clock.ms(), 4002);
    let mut buf = [0u8; 3];
    assert_eq!(s.read(&mut buf), TcpRead::Empty);
    assert!(s.connected());
    assert!(s.write_all(b"GET"));
    clock.advance_ms(5);
    assert!(s.connected()); // bytes before the close: still connected
    assert_eq!(s.read(&mut buf), TcpRead::Data(3));
    assert_eq!(s.read(&mut buf), TcpRead::Data(2));
    assert!(!s.connected());
    assert_eq!(s.read(&mut buf), TcpRead::Closed);
    assert_eq!(s.read(&mut buf), TcpRead::Closed);
    assert!(!s.write_all(b"x"));
    assert_eq!(tcp.wire(0).lock().unwrap().written, b"GET");
    s.close();
    assert_eq!(tcp.connections(), 1);
    assert_eq!(tcp.connects().len(), 3);
    assert_eq!(tcp.connects()[2].timeout_ms, 1000);
    let mut s = tcp.connect("server", 80, 1000).unwrap();
    s.close();
    assert_eq!(s.read(&mut buf), TcpRead::Closed);
    assert!(!s.connected());
    assert!(!s.write_all(b"x"));
    tcp.wire(1).lock().unwrap().fail_writes = true;
    assert_eq!(tcp.wire(1).lock().unwrap().pending(), 5);
}

#[test]
fn broker_decodes_the_device_packets_and_answers_as_scripted() {
    let clock = FakeClock::default();
    let tcp = FakeTcp::new(clock.clone(), Journal::default());
    let broker = FakeBroker::default();
    broker.attach(&tcp, "broker", 1883);
    broker.state().connack.push_back(Some(5));
    let mut s = tcp.connect("broker", 1883, 3000).unwrap();
    // CONNECT: MQTT 4, will + clean + user + password, keepalive 15, id "id"
    let connect = [
        0x10, 30, 0, 4, b'M', b'Q', b'T', b'T', 4, 0xE6, 0, 15, 0, 2, b'i', b'd', 0, 1, b'w', 0, 1,
        b'm', 0, 1, b'u', 0, 1, b'p', 0, 0, 0, 0,
    ];
    assert!(s.write_all(&connect[..32]));
    clock.advance_ms(1);
    let mut buf = [0u8; 8];
    assert_eq!(s.read(&mut buf), TcpRead::Data(4));
    assert_eq!(&buf[..4], &[0x20, 2, 0, 5]);
    let c = broker.state().connects[0].clone();
    assert_eq!(c.client_id, b"id");
    assert_eq!(
        (c.will_topic.clone(), c.will_message.clone()),
        (Some(b"w".to_vec()), Some(b"m".to_vec()))
    );
    assert_eq!(
        (c.user.clone(), c.password.clone()),
        (Some(b"u".to_vec()), Some(b"p".to_vec()))
    );
    assert!(c.clean_session() && c.will_retain());
    assert_eq!((c.will_qos(), c.keep_alive, c.level), (0, 15, 4));
    // PUBLISH QoS 0 retained, SUBSCRIBE, PINGREQ, PUBACK, DISCONNECT, garbage
    let mut p = publish_packet(b"t", b"1", 0, true, 0);
    p.extend_from_slice(&[0x82, 6, 0, 1, 0, 1, b'x', 1]);
    p.extend_from_slice(&[0xC0, 0, 0x40, 2, 0, 9, 0xE0, 0, 0xF0, 0]);
    assert!(s.write_all(&p));
    clock.advance_ms(1);
    assert_eq!(s.read(&mut buf), TcpRead::Data(7)); // SUBACK + PINGRESP
    assert_eq!(&buf[..7], &[0x90, 3, 0, 1, 1, 0xD0, 0]);
    {
        let st = broker.state();
        assert_eq!(st.published_to(b"t")[0].payload, b"1");
        assert!(st.published[0].retain);
        assert_eq!(st.subscribed, vec![(1, b"x".to_vec(), 1)]);
        assert_eq!(
            (st.pingreqs, st.pubacks.clone(), st.disconnects, st.garbage),
            (1, vec![9], 1, 1)
        );
    }
    // a QoS 1 publish of the device gets its PUBACK; inbound messages and a drop
    assert!(s.write_all(&publish_packet(b"q", b"", 1, false, 7)));
    broker.send_publish(b"cmd", b"50", 1, false, 3);
    clock.advance_ms(1);
    let mut big = [0u8; 32];
    assert_eq!(s.read(&mut big), TcpRead::Data(4 + 11));
    assert_eq!(&big[..4], &[0x40, 2, 0, 7]);
    assert_eq!(&big[4..15], &publish_packet(b"cmd", b"50", 1, false, 3)[..]);
    broker.drop_connection();
    assert!(!s.connected());
    drop(s);
    assert_eq!(broker.state().closes, 1);
}

// ---------------------------------------------------------------- HTTP

#[test]
fn http_request_headers_body_in_segments_and_one_answer() {
    let mut r = FakeRequest::post("/api/x?a=1", &vec![b'j'; 3000], "application/json")
        .with_header("X-VdMot", "1");
    r.segment = 1460;
    r.stalls.push_back(1460);
    assert_eq!(r.method(), HttpMethod::Post);
    assert_eq!(r.target(), b"/api/x?a=1");
    let mut v = [0u8; 4];
    assert_eq!(r.header("x-vdmot", &mut v), Some(1));
    assert_eq!(r.header("content-type", &mut v), Some(16));
    assert_eq!(&v, b"appl");
    assert_eq!(r.header("Origin", &mut v), None);
    assert_eq!(
        (r.content_length(), r.remote_ip(), r.local_ip()),
        (3000, 0x3201_A8C0, 0x0701_A8C0)
    );
    let mut buf = vec![0u8; 2048];
    assert_eq!(r.read_body(&mut buf), BodyRead::Data(1460));
    assert_eq!(r.read_body(&mut buf), BodyRead::Timeout);
    assert_eq!(r.read_body(&mut buf), BodyRead::Data(1460));
    assert_eq!(r.read_body(&mut buf), BodyRead::Data(80));
    assert_eq!(r.read_body(&mut buf), BodyRead::End);
    assert_eq!(r.body_read(), 3000);
    assert!(r.respond(200, "text/plain", &[("ETag", "\"1\"")], b"ok"));
    assert_eq!(r.response.header("etag"), "\"1\"");
    assert_eq!(r.response.header("x"), "");
}

#[test]
fn http_chunked_answer_a_client_that_goes_away_and_a_short_body() {
    let mut r = FakeRequest::get("/api/log");
    assert!(r.begin_chunked(200, "text/plain", &[]));
    assert!(r.chunk(b"a"));
    assert!(r.chunk(b"b"));
    assert!(r.chunk(b""));
    assert_eq!(
        (r.response.chunks, r.response.ended, r.response.body.clone()),
        (2, true, b"ab".to_vec())
    );
    let mut gone = FakeRequest::post("/api/ota/esp", &[1; 100], "multipart/form-data; boundary=b");
    gone.gone_at = Some(50);
    let mut buf = [0u8; 64];
    assert_eq!(gone.read_body(&mut buf), BodyRead::Data(50));
    assert_eq!(gone.read_body(&mut buf), BodyRead::Closed);
    assert!(!gone.respond(400, "application/json", &[], b"{}"));
    let mut short = FakeRequest::post("/x", b"ab", "application/json");
    short.content_length = Some(4);
    assert_eq!(short.read_body(&mut buf), BodyRead::Data(2));
    assert_eq!(short.read_body(&mut buf), BodyRead::Timeout);
    short.unchecked = true;
    let mut silent = FakeRequest::get("/");
    silent.client_gone = true; // gone without an answer: fine
    assert!(!silent.chunk(b"x"));
    assert!(!silent.begin_chunked(200, "text/plain", &[]));
}

#[test]
#[should_panic(expected = "not answered exactly once: 0 answers")]
fn xfail_an_http_request_without_an_answer() {
    let _r = FakeRequest::get("/api/status");
}

#[test]
#[should_panic(expected = "not answered exactly once: 2 answers")]
fn xfail_an_http_request_answered_twice() {
    let mut r = FakeRequest::get("/");
    r.respond(200, "text/plain", &[], b"1");
    r.respond(200, "text/plain", &[], b"2");
}

#[test]
fn http_server_start_can_be_refused() {
    let mut s = FakeHttpServer::default();
    s.set_refuse(true);
    assert!(!s.start());
    s.set_refuse(false);
    assert!(s.start());
    assert_eq!(s.starts(), 2);
}

// ---------------------------------------------------------------- ports through references

fn clock_via<C: Clock>(c: C) -> (u32, u32) {
    c.sleep_ms(1500);
    (c.now_ms(), c.uptime_s())
}

#[test]
fn ports_reach_the_fakes_through_shared_references() {
    let board = FakeBoard::with_slots([SlotImage::glue(APP_A), SlotImage::glue(APP_B)], 0);
    let dev = board.boot();
    assert_eq!(clock_via(&dev.clock), (1500, 1));
    let wall: &dyn WallClock = &dev.wall;
    let w = &wall;
    w.set_time_zone("CET-1");
    assert_eq!(w.epoch(), 1);
    assert_eq!(w.local_time(0).map(|t| t.hour), Some(1));
    let console = &&dev.console;
    console.line(b"x");
    assert_eq!(dev.console.lines().len(), 1);
    let nvs = &dev.nvs;
    assert!(Nvs::open(&nvs, "n", true).is_some());
    let fs = &dev.fs;
    assert!(Fs::mount(&fs));
    assert!(Fs::mkdir(&fs, "/d"));
    assert!(Fs::exists(&fs, "/d"));
    let mut f = Fs::open(&fs, "/d/f", OpenMode::Write).unwrap();
    assert_eq!(f.write(b"1"), 1);
    drop(f);
    assert!(Fs::rename(&fs, "/d/f", "/d/g"));
    let mut n = 0;
    Fs::list(&fs, "/d", &mut |_| {
        n += 1;
        true
    });
    assert_eq!(n, 1);
    assert_eq!(Fs::usage(&fs).1, 4 * 4096);
    assert!(Fs::remove(&fs, "/d/g"));
    // the false answers pass through as well
    assert!(!Fs::remove(&fs, "/d/g"));
    assert!(!Fs::rename(&fs, "/d/g", "/d/h"));
    assert!(!Fs::mkdir(&fs, "/d"));
    assert!(!Fs::exists(&fs, "/d/g"));
    assert!(Fs::open(&fs, "/d/g", OpenMode::Read).is_none());
    assert!(Fs::format(&fs));
    fs.knobs().format_ok = false;
    assert!(!Fs::format(&fs));
    fs.knobs().mount_ok = false;
    assert!(!Fs::mount(&fs));
    let tcp = &dev.tcp;
    assert!(TcpConnector::connect(&tcp, "x", 1, 0).is_none());
    tcp.listen("x", 1, || Box::new(Script(Vec::new())));
    assert!(TcpConnector::connect(&tcp, "x", 1, 0).is_some());
    let ota = &dev.ota;
    assert_eq!(Ota::running(&ota).app, Some(APP_A));
    assert_eq!(Ota::other(&ota).unwrap().app, Some(APP_B));
    assert_eq!(Ota::set_boot(&ota, 0x15_0000), Ok(()));
    assert_eq!(Ota::set_boot(&ota, 0x1234), Err(EspErr::INVALID_ARG));
    assert!(Ota::verify(&ota, 0x15_0000));
    assert!(!Ota::verify(&ota, 0x1234));
    Ota::mark_valid(&ota);
    assert_eq!(ota.knobs().mark_valids, 1);
    assert!(Ota::begin(&ota).is_ok());
    assert_eq!(Ota::running_image_size(&ota), 1_091_984);
    let sys = &dev.system;
    assert_eq!(System::reset_reason(&sys), 1);
    assert_eq!(System::heap(&sys).free, 150_000);
    assert_eq!(System::stack_min_free(&sys, "x"), None);
    sys.state().stacks.insert("x".to_string(), 77);
    assert_eq!(System::stack_min_free(&sys, "x"), Some(77));
    assert_eq!(System::base_mac(&sys), [0x24, 0x0A, 0xC4, 0x12, 0x34, 0x56]);
    assert_eq!(run(|| System::restart(&sys)), Ended::Reset(Reset::Software));
    let heap = &dev.heap;
    assert!(HeapGate::grant(&heap, 3));
    heap.state().fail_all = true;
    assert!(!HeapGate::grant(&heap, 3));
    let rtc = &dev.rtc;
    Rtc::store(&rtc, 0, &[7]);
    let mut b = [0u8; 1];
    Rtc::load(&rtc, 0, &mut b);
    assert_eq!(b, [7]);
    let mut md5 = FakeMd5::default();
    let mut m = &mut md5;
    Md5::update(&mut m, b"abc");
    let d = Md5::digest(&mut m);
    assert_eq!(d[0], 0x90);
    Md5::reset(&mut m);
    assert_eq!(md5.added, 0);
}

#[test]
fn the_reset_reason_reaches_through_a_reference() {
    let board = FakeBoard::new();
    board.reset(Reset::TaskWdt);
    let dev = board.boot();
    let sys = &dev.system;
    assert_eq!(System::reset_reason(&sys), 6);
}
