//! Port of test/native/test_stm_flasher.cpp: AN3155 over a simulated STM (v1 boot window, ROM
//! bootloader with flash model, application answering gvers), image checks, failure paths,
//! cleanup guarantees and fixed-seed fuzz.
//!
//! A C++ SUBCASE is a test of its own here (`<case>_<subcase>`). Three cases flash images above
//! 16 KiB, where the Rust flasher writes sector 0 last (D9): their erase frames, write order and
//! read order are the D9 ones, with the C++ expectation in a comment; tests_d9.rs pins the order
//! itself. C++ checks of out-of-range enum values ("unknown") are `from_raw(v) == None` here;
//! `checkBoard(nullptr, ..)` and `boardTagValid(nullptr)` are the empty slice.

use super::rig::*;
use super::*;
use crate::test_support::sim_stm::{SimLcg, BASE, KIB};
use std::boxed::Box;
use std::cell::Cell;
use std::format;
use std::rc::Rc;
use std::string::String;
use std::vec;
use std::vec::Vec;

const K: usize = KIB as usize;

fn b(s: &str) -> Vec<u8> {
    s.as_bytes().to_vec()
}

fn frames(rig: &Rig) -> Vec<Vec<u8>> {
    rig.sim.writes.iter().map(|w| w.1.clone()).collect()
}

// ================================================================ names

#[test]
fn phase_names_legacy_status_codes_and_error_names() {
    let phases = [
        (FlashPhase::Idle, "idle", 0),
        (FlashPhase::Validating, "validating", 1),
        (FlashPhase::Resetting, "resetting", 1),
        (FlashPhase::Handshake, "handshake", 1),
        (FlashPhase::Sync, "sync", 1),
        (FlashPhase::GetId, "getid", 1),
        (FlashPhase::Erasing, "erasing", 3),
        (FlashPhase::Writing, "writing", 4),
        (FlashPhase::Verifying, "verifying", 5),
        (FlashPhase::Starting, "starting", 5),
        (FlashPhase::WaitingApp, "waiting_app", 5),
        (FlashPhase::Done, "done", 6),
        (FlashPhase::Failed, "failed", 8),
    ];
    for (i, &(p, name, legacy)) in phases.iter().enumerate() {
        assert_eq!(flash_phase_name(p), name);
        assert_eq!(legacy_flash_status(p), legacy);
        assert_eq!(p as usize, i);
        assert_eq!(FlashPhase::from_raw(i as u8), Some(p));
    }
    // C++ flashPhaseName(200) "unknown", legacyFlashStatus(200) 8: no Rust value
    assert_eq!(FlashPhase::from_raw(13), None);
    assert_eq!(FlashPhase::from_raw(200), None);

    let errors = [
        (FlashError::None, "none"),
        (FlashError::ImageEmpty, "image_empty"),
        (FlashError::ImageTooLarge, "image_too_large"),
        (FlashError::ImageBadVectors, "image_bad_vectors"),
        (FlashError::ImageNoHandshake, "image_no_handshake"),
        (FlashError::ImageChipMismatch, "image_chip_mismatch"),
        (FlashError::ImageRead, "image_read"),
        (FlashError::HandshakeTimeout, "handshake_timeout"),
        (FlashError::SyncFailed, "sync_failed"),
        (FlashError::UnknownChip, "unknown_chip"),
        (FlashError::Nack, "nack"),
        (FlashError::Timeout, "timeout"),
        (FlashError::VerifyMismatch, "verify_mismatch"),
        (FlashError::TransportWrite, "transport_write"),
        (FlashError::AppNotResponding, "app_not_responding"),
        (FlashError::AppVersionMismatch, "app_version_mismatch"),
        (FlashError::Aborted, "aborted"),
        (FlashError::BoardMismatch, "board_mismatch"),
        (FlashError::BoardRequired, "board_required"),
    ];
    assert_eq!(errors.len(), FlashError::BoardRequired as usize + 1);
    for (i, &(e, name)) in errors.iter().enumerate() {
        assert_eq!(flash_error_name(e), name);
        assert_eq!(e as usize, i);
        assert_eq!(FlashError::from_raw(i as u8), Some(e));
    }
    // C++ flashErrorName(200) "unknown": no Rust value
    assert_eq!(FlashError::from_raw(19), None);
    assert_eq!(FlashError::from_raw(200), None);
}

// ================================================================ board check

#[test]
fn board_check_table_and_tag_rule() {
    assert_eq!(check_board(b"", b"C2"), BoardCheck::Untagged);
    assert_eq!(check_board(b"", b""), BoardCheck::Untagged);
    assert_eq!(check_board(&[], b"C2"), BoardCheck::Untagged); // C++ nullptr
    assert_eq!(check_board(b"C2", b""), BoardCheck::BoardRequired);
    assert_eq!(check_board(b"C2", &[]), BoardCheck::BoardRequired); // C++ nullptr
    assert_eq!(check_board(b"C2", b"C2"), BoardCheck::Ok);
    assert_eq!(check_board(b"C12", b"C12"), BoardCheck::Ok);
    assert_eq!(check_board(b"C1", b"C2"), BoardCheck::Mismatch);
    assert_eq!(check_board(b"C1", b"C12"), BoardCheck::Mismatch);
    assert_eq!(check_board(b"C12", b"C1"), BoardCheck::Mismatch);
    // Tag arrays without a NUL: at most their 4 bytes are read.
    let full = *b"C123";
    let full_too = *b"C123";
    assert_eq!(check_board(&full, &full_too), BoardCheck::Ok);
    assert_eq!(check_board(&full, b"C12"), BoardCheck::Mismatch);
    // Rust addition: a C string ends at its NUL; a longer array is read up to 4 bytes.
    assert_eq!(check_board(b"C2\0x", b"C2"), BoardCheck::Ok);
    assert_eq!(check_board(b"C123x", b"C123y"), BoardCheck::Ok);
    assert_eq!(check_board(b"\0C2", b"C2"), BoardCheck::Untagged);
    assert_eq!(check_board(b"C2", b"\0C2"), BoardCheck::BoardRequired);

    assert!(board_tag_valid(b"C1"));
    assert!(board_tag_valid(b"C0"));
    assert!(board_tag_valid(b"C9"));
    assert!(board_tag_valid(b"C12"));
    assert!(board_tag_valid(b"C99"));
    assert!(!board_tag_valid(b"C"));
    assert!(!board_tag_valid(b"C123"));
    assert!(!board_tag_valid(b"c1"));
    assert!(!board_tag_valid(b"X1"));
    assert!(!board_tag_valid(b"C1x"));
    assert!(!board_tag_valid(b"Cx"));
    assert!(!board_tag_valid(b"C/"));
    assert!(!board_tag_valid(b"C:"));
    assert!(!board_tag_valid(b""));
    assert!(!board_tag_valid(&[])); // C++ nullptr
    assert!(!board_tag_valid(&full)); // 3 digits, no NUL inside the 4 bytes
    assert!(board_tag_valid(b"C1\0x")); // Rust addition: a C string

    assert_eq!(board_check_name(BoardCheck::Ok), "ok");
    assert_eq!(board_check_name(BoardCheck::Untagged), "untagged");
    assert_eq!(board_check_name(BoardCheck::Mismatch), "mismatch");
    assert_eq!(
        board_check_name(BoardCheck::BoardRequired),
        "board_required"
    );
    // C++ boardCheckName(4) "unknown": no Rust value
    for (i, c) in [
        BoardCheck::Ok,
        BoardCheck::Untagged,
        BoardCheck::Mismatch,
        BoardCheck::BoardRequired,
    ]
    .into_iter()
    .enumerate()
    {
        assert_eq!(c as usize, i);
        assert_eq!(BoardCheck::from_raw(i as u8), Some(c));
    }
    assert_eq!(BoardCheck::from_raw(4), None);
}

#[test]
fn option_and_status_defaults_of_the_board_check() {
    let opt = FlashOptions::default();
    assert_eq!(opt.app_timeout_ms, 60000);
    assert_eq!(opt.fallback_baud, 57600);
    assert!(opt.board_hw.is_empty());
    let st = FlashStatus::default();
    assert_eq!(st.board, BoardCheck::Ok);
    assert!(st.board_hw.is_empty());
    assert!(!st.manual_reset);
    assert_eq!(st.baud, 0);
    let info = ImageInfo::default();
    assert!(info.hw_tag.is_empty());
    assert!(!info.hw_conflict);
}

// ================================================================ sectors

#[test]
fn sectors_for_image_boundaries() {
    assert_eq!(sectors_for_image(0), 0);
    assert_eq!(sectors_for_image(1), 1);
    let ends: [u32; 8] = [16, 32, 48, 64, 128, 256, 384, 512];
    for (i, &end) in ends.iter().enumerate() {
        let n = i as u8 + 1;
        assert_eq!(sectors_for_image(end * KIB), n, "{i}");
        assert_eq!(sectors_for_image(end * KIB - 1), n, "{i}");
        if i < 7 {
            assert_eq!(sectors_for_image(end * KIB + 1), n + 1, "{i}");
        }
    }
    assert_eq!(sectors_for_image(512 * KIB + 1), 0);
    assert_eq!(sectors_for_image(0xFFFF_FFFF), 0);
}

// ================================================================ validate_image

#[test]
fn validate_crc_reference_and_a_real_release_like_image() {
    assert_eq!(ref_crc32(b"123456789"), 0xCBF4_3926);

    // Shape of releases/STM32/1.4.9_Dev/STM32F411_C1_firmware.bin: 53 760 B, SP 0x20020000,
    // reset vector odd inside the image, handshake strings.
    let d = make_image_with(53760, 0x2002_0000, 0x0800_B4D1, true, "1.4.9_Dev");
    let mut info = ImageInfo::default();
    assert_eq!(validate(&d, 0, true, &mut info), FlashError::None);
    assert_eq!(info.size, 53760);
    assert_eq!(info.padded_size, 53760);
    assert_eq!(info.initial_sp, 0x2002_0000);
    assert_eq!(info.reset_vector, 0x0800_B4D1);
    assert!(info.has_handshake);
    assert_eq!(info.crc, ref_crc32(&d));
    assert_eq!(&info.version[..], b"1.4.9_Dev");
    assert_eq!(validate(&d, 0x431, true, &mut info), FlashError::None);
    assert_eq!(
        validate(&d, 0x433, true, &mut info),
        FlashError::ImageChipMismatch
    ); // 96 KiB RAM
    assert_eq!(
        validate(&d, 0x423, true, &mut info),
        FlashError::ImageChipMismatch
    );

    let f401 = make_image_with(53472, 0x2001_0000, 0x0800_B4B1, true, "1.4.9_Dev");
    assert_eq!(validate(&f401, 0x423, true, &mut info), FlashError::None);
    assert_eq!(validate(&f401, 0x433, true, &mut info), FlashError::None);
    assert_eq!(validate(&f401, 0x431, true, &mut info), FlashError::None);
}

#[test]
fn validate_size_limits_and_padding() {
    let mut info = ImageInfo::default();
    assert_eq!(validate(&[], 0, false, &mut info), FlashError::ImageEmpty);
    assert_eq!(info.size, 0);
    assert_eq!(info.padded_size, 0);

    let mut big = make_image_with(512 * K + 1, 0x2002_0000, BASE + 1, false, "");
    assert_eq!(
        validate(&big, 0, false, &mut info),
        FlashError::ImageTooLarge
    );
    assert_eq!(info.size as usize, 512 * K + 1);
    big.pop();
    assert_eq!(validate(&big, 0, false, &mut info), FlashError::None);
    assert_eq!(info.padded_size as usize, 512 * K);

    for n in 1..8usize {
        assert_eq!(
            validate(&vec![0; n], 0, false, &mut info),
            FlashError::ImageBadVectors,
            "{n}"
        );
        assert_eq!(info.size as usize, n);
        assert_eq!(info.padded_size as usize, (n + 3) & !3);
    }
    let pads: [(usize, u32); 7] = [
        (8, 8),
        (9, 12),
        (10, 12),
        (11, 12),
        (12, 12),
        (13, 16),
        (1001, 1004),
    ];
    for (size, padded) in pads {
        let d = make_image_with(size, 0x2000_1000, BASE + 1, false, "");
        assert_eq!(
            validate(&d, 0, false, &mut info),
            FlashError::None,
            "{size}"
        );
        assert_eq!(info.padded_size, padded, "{size}");
    }
}

#[test]
fn validate_initial_sp_and_reset_vector_boundaries() {
    let mut info = ImageInfo::default();
    let sps = [
        (0x2000_0000, FlashError::ImageBadVectors),
        (0x2000_0004, FlashError::None),
        (0x2001_FFFC, FlashError::None),
        (0x2002_0000, FlashError::None),
        (0x2002_0004, FlashError::ImageBadVectors),
        (0x2001_FFFE, FlashError::ImageBadVectors),
        (0x2001_0001, FlashError::ImageBadVectors),
        (0x1FFF_FFFC, FlashError::ImageBadVectors),
        (0x0000_0000, FlashError::ImageBadVectors),
        (0xFFFF_FFFC, FlashError::ImageBadVectors),
    ];
    for (sp, e) in sps {
        let d = make_image_with(1024, sp, BASE + 0x101, true, "1.4.9_Dev");
        assert_eq!(validate(&d, 0, true, &mut info), e, "{sp:#x}");
        assert_eq!(info.initial_sp, sp);
    }
    let size = 1023u32; // odd: BASE + size is odd too
    let pcs = [
        (BASE + 1, FlashError::None),
        (BASE + size - 2, FlashError::None),
        (BASE + size, FlashError::ImageBadVectors),
        (BASE + size + 2, FlashError::ImageBadVectors),
        (BASE + 0x100, FlashError::ImageBadVectors), // even: not Thumb
        (BASE - 1, FlashError::ImageBadVectors),
        (0x1FFF_0001, FlashError::ImageBadVectors),
        (0xFFFF_FFFF, FlashError::ImageBadVectors),
    ];
    for (pc, e) in pcs {
        let d = make_image_with(size as usize, 0x2002_0000, pc, true, "1.4.9_Dev");
        assert_eq!(validate(&d, 0, true, &mut info), e, "{pc:#x}");
        assert_eq!(info.reset_vector, pc);
    }
    // Even size: last odd address inside is BASE + size - 1.
    let ok = make_image_with(1024, 0x2002_0000, BASE + 1023, true, "1.4.9_Dev");
    assert_eq!(validate(&ok, 0, true, &mut info), FlashError::None);
    let out = make_image_with(1024, 0x2002_0000, BASE + 1025, true, "1.4.9_Dev");
    assert_eq!(
        validate(&out, 0, true, &mut info),
        FlashError::ImageBadVectors
    );
}

#[test]
fn validate_chip_specific_limits() {
    let mut info = ImageInfo::default();
    let mut at = |size: usize, sp: u32, pid: u16| {
        validate(
            &make_image_with(size, sp, BASE + 1, false, ""),
            pid,
            false,
            &mut info,
        )
    };
    assert_eq!(at(256 * K, 0x2001_0000, 0x423), FlashError::None);
    assert_eq!(
        at(256 * K + 1, 0x2001_0000, 0x423),
        FlashError::ImageTooLarge
    );
    assert_eq!(
        at(256 * K, 0x2001_0004, 0x423),
        FlashError::ImageChipMismatch
    );
    assert_eq!(at(512 * K, 0x2001_8000, 0x433), FlashError::None);
    assert_eq!(
        at(512 * K, 0x2001_800C, 0x433),
        FlashError::ImageChipMismatch
    );
    assert_eq!(at(512 * K, 0x2002_0000, 0x431), FlashError::None);
    // Size is checked before SP.
    assert_eq!(at(300 * K, 0x2002_0000, 0x423), FlashError::ImageTooLarge);
    // Unknown / non-F401/F411 PIDs.
    assert_eq!(at(1024, 0x2001_0000, 0x413), FlashError::UnknownChip);
    assert_eq!(at(1024, 0x2001_0000, 0x001), FlashError::UnknownChip);
    assert_eq!(at(1024, 0x2001_0000, 0xFFFF), FlashError::UnknownChip);
    // Generic errors win over chip errors.
    assert_eq!(at(1024, 0x2000_0000, 0x413), FlashError::ImageBadVectors);
    assert_eq!(
        validate(&[], 0x413, false, &mut info),
        FlashError::ImageEmpty
    );
}

#[test]
fn validate_handshake_strings_also_across_chunk_boundaries() {
    let mut info = ImageInfo::default();
    let d = make_image_with(4096, 0x2002_0000, BASE + 1, false, "");
    assert_eq!(
        validate(&d, 0, true, &mut info),
        FlashError::ImageNoHandshake
    );
    assert!(!info.has_handshake);
    assert_eq!(info.crc, ref_crc32(&d)); // filled before the handshake verdict
    assert_eq!(validate(&d, 0, false, &mut info), FlashError::None);
    assert!(!info.has_handshake);

    let mut only_dead = d.clone();
    put_str(&mut only_dead, 1000, b"DEADBEEF");
    assert_eq!(
        validate(&only_dead, 0, true, &mut info),
        FlashError::ImageNoHandshake
    );
    let mut only_beef = d.clone();
    put_str(&mut only_beef, 1000, b"BEEFIT");
    assert_eq!(
        validate(&only_beef, 0, true, &mut info),
        FlashError::ImageNoHandshake
    );
    let mut near_miss = d.clone();
    put_str(&mut near_miss, 1000, b"DEADBEEX");
    put_str(&mut near_miss, 2000, b"BEEFIX");
    assert_eq!(
        validate(&near_miss, 0, true, &mut info),
        FlashError::ImageNoHandshake
    );

    // Every split position of both strings across the 256-byte read chunks.
    for k in 1..8usize {
        let mut s = d.clone();
        put_str(&mut s, 512 - k, b"DEADBEEF");
        put_str(&mut s, 1024 - k.min(5), b"BEEFIT");
        assert_eq!(validate(&s, 0, true, &mut info), FlashError::None, "{k}");
        assert!(info.has_handshake);
    }
    // At the very end of the image.
    let mut end = d.clone();
    put_str(&mut end, 4096 - 14, b"DEADBEEFBEEFIT");
    assert_eq!(validate(&end, 0, true, &mut info), FlashError::None);
    // Overlapping "DEADBEEFIT" contains both.
    let mut both = d.clone();
    put_str(&mut both, 3000, b"DEADBEEFIT");
    assert_eq!(validate(&both, 0, true, &mut info), FlashError::None);
}

#[test]
fn validate_version_string_extraction() {
    let with_blob_at = |blob: &[u8], at: usize| {
        let mut d = make_image_with(4096, 0x2002_0000, BASE + 1, false, "");
        put_str(&mut d, at, blob);
        d
    };
    let with_blob = |blob: &[u8]| with_blob_at(blob, 2000);
    assert_eq!(version_of(&with_blob(b"\x011.4.9_Dev\0")), "1.4.9_Dev");
    assert_eq!(
        version_of(&with_blob(b"\x002.0.0-revamped\0")),
        "2.0.0-revamped"
    );
    assert_eq!(version_of(&with_blob(b"\x011.4.9_C1\0")), "1.4.9_C1");
    assert_eq!(version_of(&with_blob(b"\x011.4\0")), "");
    assert_eq!(version_of(&with_blob(b"\x01abc 1.4.9\0")), ""); // run has a space
    assert_eq!(version_of(&with_blob(b"\x011.4.9 \0")), "");
    assert_eq!(version_of(&with_blob(b"\x01x1.4.9\0")), "");
    assert_eq!(version_of(&with_blob(b"\x011.4.9\x01")), ""); // not NUL-terminated
    assert_eq!(version_of(&with_blob(b"xyz\x021.4.9\0")), "1.4.9"); // run restarts
                                                                    // Printable is 0x20..0x7E: '~' and ' ' belong to the run, 0x1F and 0x7F end it.
    assert_eq!(version_of(&with_blob(b"\x01~1.4.9\0")), "");
    assert_eq!(version_of(&with_blob(b"\x01 1.4.9\0")), "");
    assert_eq!(version_of(&with_blob(b"\x1F1.4.9\0")), "1.4.9");
    assert_eq!(version_of(&with_blob(b"~~\x7F1.4.9\0")), "1.4.9");
    // First match wins; invalid candidates before it are skipped.
    assert_eq!(
        version_of(&with_blob(b"\x019.9\x001.2.3\x004.5.6\0")),
        "1.2.3"
    );
    // 31 characters is the longest version; a 32-char run is not a version, not even its
    // 31-char tail.
    let v31 = String::from("1.4.9_") + &"a".repeat(25);
    assert_eq!(v31.len(), 31);
    let blob = |pre: &str, s: &str| {
        let mut v = b(pre);
        v.extend_from_slice(s.as_bytes());
        v.push(0);
        v
    };
    assert_eq!(version_of(&with_blob(&blob("\x01", &v31))), v31);
    assert_eq!(version_of(&with_blob(&blob("\x01x", &v31))), "");
    let long = "q".repeat(100) + "1.2.3";
    assert_eq!(version_of(&with_blob(&blob("\x01", &long))), "");
    // Image ending in a run without NUL.
    let mut tail = make_image_with(4096, 0x2002_0000, BASE + 1, false, "");
    put_str(&mut tail, 4096 - 6, b"\x011.2.3");
    assert_eq!(version_of(&tail), "");
    // Version as the first bytes after the vectors: vector bytes are not printable here.
    let mut early = make_image_with(4096, 0x2002_0000, BASE + 0x101, false, "");
    put_str(&mut early, 8, b"3.2.1\0");
    assert_eq!(version_of(&early), "3.2.1");
}

#[test]
fn validate_read_errors() {
    let mut m = MemImage::new(make_image(4096));
    let mut info = ImageInfo::default();
    m.fail_at = 3;
    assert_eq!(
        validate_image(&mut m, 0, true, &mut info),
        FlashError::ImageRead
    );
    m.fail_at = 3000;
    assert_eq!(
        validate_image(&mut m, 0, true, &mut info),
        FlashError::ImageRead
    );
    assert_eq!(info.initial_sp, 0x2002_0000); // header facts kept
    m.fail_at = -1;
    m.size_override = 5000; // size() larger than the readable data
    assert_eq!(
        validate_image(&mut m, 0, true, &mut info),
        FlashError::ImageRead
    );
}

#[test]
fn validate_fixed_seed_fuzz_against_the_reference_rules() {
    let mut r = SimLcg::new(0xC0FFEE);
    let pids: [u16; 5] = [0, 0x423, 0x431, 0x433, 0x413];
    for iter in 0..3000 {
        let size = if r.below(4) == 0 {
            r.below(16)
        } else {
            8 + r.below(2048)
        } as usize;
        let mut d: Vec<u8> = (0..size).map(|_| r.next() as u8).collect();
        if size >= 8 {
            let sps = [
                0x2002_0000,
                0x2001_0000,
                0x2001_8000,
                0x2000_0000,
                0x2000_0004,
                0x2002_0004,
                0x2001_8002,
                r.next(),
            ];
            put32(&mut d, 0, sps[r.below(8) as usize]);
            let pcs = [
                BASE + 1,
                BASE + size as u32 - 1,
                BASE + size as u32,
                BASE + 2 * r.below(1200) + 1,
                BASE + 2 * r.below(1200),
                r.next(),
            ];
            put32(&mut d, 4, pcs[r.below(6) as usize]);
        }
        if size > 40 && r.below(2) != 0 {
            let at = 10 + r.below(size as u32 - 30) as usize;
            put_str(&mut d, at, b"DEADBEEF");
        }
        if size > 40 && r.below(2) != 0 {
            let at = 10 + r.below(size as u32 - 30) as usize;
            put_str(&mut d, at, b"BEEFIT");
        }
        let pid = pids[r.below(5) as usize];
        let req = r.below(2) != 0;
        let mut info = ImageInfo::default();
        assert_eq!(
            validate(&d, pid, req, &mut info),
            ref_validate(&d, pid, req),
            "{iter}"
        );
        assert_eq!(info.size as usize, size, "{iter}");
    }
}

// ================================================================ begin/idle

#[test]
fn idle_object_begin_preconditions() {
    let mut rig = Rig::new(make_image(53760));
    assert!(!rig.f.active());
    assert_eq!(rig.f.status().phase, FlashPhase::Idle);
    assert_eq!(rig.step(), FlashPhase::Idle);
    rig.f.abort();
    assert_eq!(rig.step(), FlashPhase::Idle);
    assert!(rig.untouched());

    rig.opt.baud = 1199;
    assert!(!rig.begin());
    rig.opt.baud = 115_201;
    assert!(!rig.begin());
    rig.opt.baud = 0;
    assert!(!rig.begin());
    assert_eq!(rig.f.status().phase, FlashPhase::Idle);

    rig.opt.baud = 1200;
    assert!(rig.begin());
    assert!(rig.f.active());
    assert_eq!(rig.f.status().phase, FlashPhase::Validating);
    assert_eq!(rig.f.status().started_ms, 1000);
    assert_eq!(rig.f.status().percent, 0);
    assert_eq!(rig.f.status().error, FlashError::None);
    // Busy: a second begin is refused and changes nothing.
    rig.now += 50;
    assert!(!rig.f.begin(&rig.opt, rig.now));
    assert_eq!(rig.f.status().started_ms, 1000);
    assert!(rig.untouched());
}

#[test]
fn begin_accepts_the_upper_baud_limit() {
    let mut rig = Rig::new(make_image(53760));
    rig.opt.baud = 115_200;
    assert!(rig.begin());
}

// ================================================================ happy path

#[test]
fn normal_mode_end_to_end_on_a_1_4_9_sized_f411_image() {
    let mut rig = Rig::new(make_image(53760));
    assert!(rig.begin());
    assert_eq!(rig.run(), FlashPhase::Done);
    let st = rig.f.status().clone();
    assert_eq!(st.error, FlashError::None);
    assert_eq!(st.percent, 100);
    assert_eq!(st.chip_pid, 0x431);
    assert_eq!(st.bootloader_version, 0x31);
    assert_eq!(st.attempt, 0);
    assert_eq!(st.bytes_total, 53760);
    assert_eq!(st.bytes_done, 53760);
    assert_eq!(st.image.size, 53760);
    assert_eq!(&st.image.version[..], b"1.4.9_Dev");
    assert_eq!(st.image.crc, ref_crc32(&rig.img.data));
    assert!(st.app_version.valid);
    assert_eq!(st.app_version.major, 1);
    assert_eq!(st.app_version.patch, 9);
    assert_eq!(&st.app_version.hw[..], b"C1");
    assert_eq!(st.finished_ms, rig.now);
    assert_eq!(st.started_ms, 1000);
    assert!(!rig.f.active());
    assert!(rig.percent_monotonic());
    assert!(rig.flash_matches_image());
    // Sectors 0..3 (64 KiB) erased; the rest of the chip untouched.
    assert_eq!(rig.sim.flash[64 * K..], rig.sim.original[64 * K..]);
    assert!(rig.sim.flash[53760..64 * K].iter().all(|&b| b == 0xFF));

    // Phase order. C++: one Erasing, Writing, Verifying; D9: sectors 1..3, then sector 0.
    let order = [
        FlashPhase::Validating,
        FlashPhase::Resetting,
        FlashPhase::Handshake,
        FlashPhase::Sync,
        FlashPhase::GetId,
        FlashPhase::Erasing,
        FlashPhase::Writing,
        FlashPhase::Verifying,
        FlashPhase::Erasing,
        FlashPhase::Writing,
        FlashPhase::Verifying,
        FlashPhase::Starting,
        FlashPhase::WaitingApp,
        FlashPhase::Done,
    ];
    let mut seen = vec![FlashPhase::Validating];
    for &p in &rig.phases {
        if Some(&p) != seen.last() {
            seen.push(p);
        }
    }
    assert_eq!(seen, order);

    // UART: 8E1 at the bootloader baud, then 8N1 for the application.
    assert_eq!(rig.sim.configs, [(115_200, true), (115_200, false)]);
    // Two NRST pulses of 100 ms.
    let resets = &rig.sim.resets;
    assert_eq!(resets.len(), 4);
    assert!(resets[0].1);
    assert!(!resets[1].1);
    assert!((100..=102).contains(&(resets[1].0 - resets[0].0)));
    assert!(resets[2].1);
    assert!(!resets[3].1);
    assert!((100..=102).contains(&(resets[3].0 - resets[2].0)));

    // Handshake: first "DEADBEEF\n" 20 ms after release.
    let release = resets[1].0;
    let w0 = &rig.sim.writes[0];
    assert_eq!(w0.1, b"DEADBEEF\n");
    assert!((20..=22).contains(&(w0.0 - release)));

    // Sync 0x7F >= 250 ms after BEEFIT, then GET, GET ID, erase list.
    assert_eq!(rig.sim.sync_times.len(), 1);
    assert!((250..=254).contains(&(rig.sim.sync_times[0] - rig.sim.beefit_at)));
    assert!(rig.sim.commands.len() >= 3);
    assert_eq!(rig.sim.commands[..3], [0x00, 0x02, 0x44]);
    // C++: one frame {0x00, 0x03, 0x00, 0x00, 0x00, 0x01, 0x00, 0x02, 0x00, 0x03, 0x03}.
    assert_eq!(
        rig.sim.erase_frames,
        [
            vec![0x00, 0x02, 0x00, 0x01, 0x00, 0x02, 0x00, 0x03, 0x02],
            vec![0x00, 0x00, 0x00, 0x00, 0x00]
        ]
    );

    // 210 blocks written once each. C++: 1..209, then 0; D9: 64..209, 1..63, then 0.
    let programmed = &rig.sim.programmed;
    assert_eq!(programmed.len(), 210);
    let order: Vec<u32> = (64..210).chain(1..64).chain([0]).collect();
    for (p, &blk) in programmed.iter().zip(&order) {
        assert_eq!(p.addr, BASE + blk * 256);
        assert_eq!(p.len, 256);
    }
    // Verify reads, C++: 0..209 in order; D9: 64..209, then 0..63.
    let reads: Vec<u32> = (64..210).chain(0..64).map(|blk| BASE + blk * 256).collect();
    assert_eq!(rig.sim.read_addrs, reads);

    // App: gvers 4 s after the final release, exact request text.
    assert_eq!(rig.sim.gvers_times.len(), 1);
    assert!((4000..=4002).contains(&(rig.sim.gvers_times[0] - resets[3].0)));
    assert_eq!(
        rig.sim.writes.last().map(|w| &w.1[..]),
        Some(&b"gvers \r\n"[..])
    );
    assert!(rig.left_clean());
    assert!(rig.sim.in_app());
}

#[test]
fn exact_an3155_frames_for_write_and_read_of_one_block() {
    let mut rig = Rig::new(make_image(600)); // blocks: 0 (256), 1 (256), 2 (88)
    assert_eq!(rig.begin_and_run(), FlashPhase::Done);
    let w = frames(&rig);
    // Locate the first write command.
    let mut i = w.iter().position(|f| f == &[0x31, 0xCE]).expect("write");
    assert!(i + 2 < w.len());
    assert_eq!(w[i + 1], [0x08, 0x00, 0x01, 0x00, 0x09]);
    assert_eq!(w[i + 2].len(), 258);
    assert_eq!(w[i + 2][0], 0xFF);
    let mut cs = 0xFFu8;
    for k in 0..256 {
        assert_eq!(w[i + 2][1 + k], rig.img.data[256 + k]);
        cs ^= rig.img.data[256 + k];
    }
    assert_eq!(w[i + 2][257], cs);
    // Second block: the 88-byte tail at 0x08000200.
    assert_eq!(w[i + 3], [0x31, 0xCE]);
    assert_eq!(w[i + 4], [0x08, 0x00, 0x02, 0x00, 0x0A]);
    assert_eq!(w[i + 5].len(), 90);
    assert_eq!(w[i + 5][0], 87);
    // Then block 0 last.
    assert_eq!(w[i + 7], [0x08, 0x00, 0x00, 0x00, 0x08]);
    // First read: 11 EE, address, N ~N.
    i += w[i..]
        .iter()
        .position(|f| f == &[0x11, 0xEE])
        .expect("read");
    assert!(i + 2 < w.len());
    assert_eq!(w[i + 1], [0x08, 0x00, 0x00, 0x00, 0x08]);
    assert_eq!(w[i + 2], [0xFF, 0x00]);
    // Tail read: N = 87.
    assert_eq!(w[i + 8], [87, 87 ^ 0xFF]);
    // Command framing of GET / GET ID / erase.
    let count = |f: &[u8]| w.iter().filter(|x| x[..] == *f).count();
    assert_eq!(count(&[0x00, 0xFF]), 1);
    assert_eq!(count(&[0x02, 0xFD]), 1);
    assert_eq!(count(&[0x44, 0xBB]), 1);
    assert_eq!(count(&[0x7F]), 1);
    assert_eq!(rig.sim.erase_frames[0], [0x00, 0x00, 0x00, 0x00, 0x00]);
}

#[test]
fn image_sizes_around_block_and_word_boundaries() {
    let sizes = [8usize, 9, 255, 256, 257, 511, 512, 1001, 16 * 1024 + 4];
    for size in sizes {
        let hs = size >= 64;
        let mut rig = Rig::new(make_image_with(
            size,
            0x2002_0000,
            BASE + 1,
            hs,
            if hs { "1.4.9_Dev" } else { "" },
        ));
        rig.opt.force = !hs;
        assert_eq!(rig.begin_and_run(), FlashPhase::Done, "{size}");
        assert_eq!(rig.f.status().error, FlashError::None);
        assert!(rig.flash_matches_image());
        let padded = ((size + 3) & !3) as u32;
        assert_eq!(rig.f.status().bytes_total, padded);
        let mut total = 0;
        for p in &rig.sim.programmed {
            assert_eq!(p.len % 4, 0);
            total += p.len;
        }
        assert_eq!(total, padded);
        assert_eq!(rig.sim.programmed.last().map(|p| p.addr), Some(BASE));
        assert_eq!(rig.sim.programmed.len() as u32, padded.div_ceil(256));
        let n = sectors_for_image(padded);
        if padded > 16 * KIB {
            // D9: sectors 1..n first, then sector 0 (C++: one frame with n - 1)
            assert_eq!(rig.sim.erase_frames[0][1], n - 2, "{size}");
            assert_eq!(rig.sim.erase_frames[1], [0, 0, 0, 0, 0], "{size}");
        } else {
            assert_eq!(rig.sim.erase_frames[0][1], n - 1, "{size}");
        }
    }
}

#[test]
fn full_512_kib_image_erases_all_8_sectors() {
    let mut rig = Rig::new(make_image(512 * K));
    assert_eq!(rig.begin_and_run(), FlashPhase::Done);
    assert!(rig.flash_matches_image());
    // C++: one frame with sectors 0..7; D9: sectors 1..7, then sector 0.
    let mut exp = vec![0x00, 0x06];
    let mut cs = 0x06;
    for s in 1..8u8 {
        exp.push(0);
        exp.push(s);
        cs ^= s;
    }
    exp.push(cs);
    assert_eq!(rig.sim.erase_frames, [exp, vec![0, 0, 0, 0, 0]]);
}

#[test]
fn one_data_frame_per_step_chained_transitions() {
    let mut rig = Rig::new(make_image(16 * K));
    assert!(rig.begin());
    let mut seen = 0;
    let mut max_frames = 0;
    let (mut write_start, mut verify_start, mut starting_at) = (0u32, 0u32, 0u32);
    rig.run_hook(|r| {
        let frames = r.sim.writes[seen..]
            .iter()
            .filter(|w| w.1.len() > 16)
            .count();
        seen = r.sim.writes.len();
        max_frames = max_frames.max(frames);
        let p = r.f.status().phase;
        if p == FlashPhase::Writing && write_start == 0 {
            write_start = r.now;
        }
        if p == FlashPhase::Verifying && verify_start == 0 {
            verify_start = r.now;
        }
        if p == FlashPhase::Starting && starting_at == 0 {
            starting_at = r.now;
        }
    });
    assert_eq!(rig.f.status().phase, FlashPhase::Done);
    assert_eq!(max_frames, 1);
    // 64 blocks; the simulator answers after 1-2 ms, so a chained write is 2-3 steps (4-6 ms)
    // per block, a read-back about the same.
    assert!(verify_start - write_start <= 64 * 6 + 4);
    assert!(starting_at - verify_start <= 64 * 6 + 4);
}

#[test]
fn validation_reads_at_most_1_kib_per_step() {
    let mut rig = Rig::new(make_image(512 * K));
    assert!(rig.begin());
    let mut prev = 0;
    let mut max_per_step = 0;
    let mut validating_steps = 0;
    rig.run_hook(|r| {
        if r.f.status().phase == FlashPhase::Validating {
            validating_steps += 1;
        }
        if validating_steps > 0 && r.sim.resets.is_empty() {
            max_per_step = max_per_step.max(r.img.bytes_read - prev);
        }
        prev = r.img.bytes_read;
    });
    assert!(max_per_step <= 1024 + 8);
    // Step 1 reads the vectors and the first 1 KiB, steps 2..512 the rest; the last one
    // already moves on to Resetting.
    assert_eq!(validating_steps, 511);
    assert_eq!(rig.f.status().phase, FlashPhase::Done);
}

#[test]
fn validation_progress_is_reported_0_to_2_percent() {
    let mut rig = Rig::new(make_image(64 * K));
    assert!(rig.begin());
    let mut max_validating = 0;
    let mut saw_one = false;
    rig.run_hook(|r| {
        if r.f.status().phase == FlashPhase::Validating {
            max_validating = max_validating.max(r.f.status().percent);
            saw_one |= r.f.status().percent == 1;
        }
    });
    assert!(saw_one);
    assert!(max_validating <= 2);
}

#[test]
fn percent_milestones_per_phase() {
    let mut rig = Rig::new(make_image(8 * K));
    assert!(rig.begin());
    let mut lo = [255u8; 13];
    let mut hi = [0u8; 13];
    rig.run_hook(|g| {
        let p = g.f.status().phase as usize;
        let v = g.f.status().percent;
        lo[p] = lo[p].min(v);
        hi[p] = hi[p].max(v);
    });
    let lo = |p: FlashPhase| lo[p as usize];
    let hi = |p: FlashPhase| hi[p as usize];
    assert_eq!(lo(FlashPhase::Resetting), 2);
    assert_eq!(lo(FlashPhase::Handshake), 3);
    assert_eq!(hi(FlashPhase::Handshake), 3);
    assert_eq!(lo(FlashPhase::Sync), 4);
    assert_eq!(lo(FlashPhase::GetId), 5);
    assert_eq!(hi(FlashPhase::Erasing), 5);
    assert!(lo(FlashPhase::Writing) >= 15);
    assert!(hi(FlashPhase::Writing) <= 75);
    assert!(lo(FlashPhase::Verifying) >= 75);
    assert!(hi(FlashPhase::Verifying) <= 95);
    assert_eq!(lo(FlashPhase::Starting), 95);
    assert_eq!(lo(FlashPhase::WaitingApp), 97);
    assert_eq!(hi(FlashPhase::WaitingApp), 98);
    assert_eq!(lo(FlashPhase::Done), 100);
}

#[test]
fn exact_percent_and_byte_counters_while_writing_and_verifying() {
    // 8 KiB = 32 blocks. The last block's 75 % is set together with the switch to Verifying,
    // so Writing itself tops out at 15 + 60 * 31/32 = 73.
    let mut rig = Rig::new(make_image(8 * K));
    assert!(rig.begin());
    let (mut write_lo, mut write_hi, mut verify_lo, mut verify_hi) = (255u8, 0u8, 255u8, 0u8);
    let (mut write_bytes_max, mut verify_bytes_max) = (0u32, 0u32);
    rig.run_hook(|g| {
        let st = g.f.status();
        if st.phase == FlashPhase::Writing {
            write_lo = write_lo.min(st.percent);
            write_hi = write_hi.max(st.percent);
            write_bytes_max = write_bytes_max.max(st.bytes_done);
        }
        if st.phase == FlashPhase::Verifying {
            verify_lo = verify_lo.min(st.percent);
            verify_hi = verify_hi.max(st.percent);
            verify_bytes_max = verify_bytes_max.max(st.bytes_done);
        }
    });
    assert_eq!(rig.f.status().phase, FlashPhase::Done);
    assert_eq!(write_lo, 15);
    assert_eq!(write_hi, 73);
    assert_eq!(write_bytes_max, 31 * 256);
    assert_eq!(verify_lo, 75);
    assert_eq!(verify_hi, 94); // 75 + 20 * 31/32
    assert_eq!(verify_bytes_max, 31 * 256);
}

#[test]
fn blank_mode_reports_4_percent_in_sync() {
    let mut rig = Rig::new(make_image(2048));
    rig.opt.blank = true;
    rig.sim.has_boot_loop = false;
    rig.sim.boot_pin_resets = 1;
    assert!(rig.begin());
    let (mut sync_lo, mut sync_hi) = (255u8, 0u8);
    rig.run_hook(|g| {
        if g.f.status().phase == FlashPhase::Sync {
            sync_lo = sync_lo.min(g.f.status().percent);
            sync_hi = sync_hi.max(g.f.status().percent);
        }
    });
    assert_eq!(rig.f.status().phase, FlashPhase::Done);
    assert_eq!(sync_lo, 4);
    assert_eq!(sync_hi, 4);
}

#[test]
fn runs_across_the_millis_wrap() {
    let mut rig = Rig::new(make_image(4096));
    rig.now = 0xFFFF_F000;
    assert_eq!(rig.begin_and_run(), FlashPhase::Done);
    assert!(rig.flash_matches_image());
    assert_eq!(rig.f.status().started_ms, 0xFFFF_F000);
}

#[test]
fn other_bootloader_baud() {
    let mut rig = Rig::new(make_image(2048));
    rig.opt.baud = 57600;
    rig.sim.has_boot_loop = false;
    rig.sim.boot_pin_resets = 1; // v1 window needs 115200: use blank mode
    rig.opt.blank = true;
    assert_eq!(rig.begin_and_run(), FlashPhase::Done);
    assert_eq!(rig.sim.configs[0], (57600, true));
    assert_eq!(rig.sim.configs[1], (115_200, false));
}

#[test]
fn normal_mode_handshakes_at_115200_and_then_switches_to_the_bootloader_baud() {
    let mut rig = Rig::new(make_image(2048));
    rig.opt.baud = 57600;
    assert!(rig.begin());
    let mut switched_at = 0;
    rig.run_hook(|r| {
        if switched_at == 0 && r.sim.configs.len() == 2 {
            switched_at = r.now;
        }
    });
    assert_eq!(rig.f.status().phase, FlashPhase::Done);
    assert!(rig.flash_matches_image());
    assert_eq!(
        rig.sim.configs,
        [(115_200, true), (57600, true), (115_200, false)]
    );
    // Switched once BEEFIT was seen, before the first 0x7F.
    assert!(!rig.sim.sync_times.is_empty());
    assert!(switched_at >= rig.sim.beefit_at);
    assert!(switched_at <= rig.sim.sync_times[0]);
}

#[test]
fn normal_mode_at_115200_configures_the_uart_once_for_the_bootloader() {
    let mut rig = Rig::new(make_image(2048));
    assert_eq!(rig.begin_and_run(), FlashPhase::Done);
    assert_eq!(rig.sim.configs, [(115_200, true), (115_200, false)]);
}

#[test]
fn a_second_run_after_done_and_after_failed() {
    let mut rig = Rig::new(make_image(2048));
    assert_eq!(rig.begin_and_run(), FlashPhase::Done);
    rig.sim.programmed.clear();
    assert_eq!(rig.begin_and_run(), FlashPhase::Done);
    assert!(rig.f.status().started_ms > 1000);
    assert_eq!(rig.sim.programmed.len(), 8);
    rig.img.data = Vec::new();
    assert_eq!(rig.begin_and_run(), FlashPhase::Failed);
    assert_eq!(rig.f.status().error, FlashError::ImageEmpty);
    rig.img.data = make_image(2048);
    assert_eq!(rig.begin_and_run(), FlashPhase::Done);
    assert_eq!(rig.f.status().error, FlashError::None);
    assert_eq!(rig.f.status().chip_pid, 0x431);
}

// ================================================================ validation failures

#[test]
fn invalid_images_fail_before_the_stm_is_touched() {
    let cases = [
        (Vec::new(), FlashError::ImageEmpty),
        (vec![0; 7], FlashError::ImageBadVectors),
        (
            make_image_with(2048, 0x2000_0000, 0, true, "1.4.9_Dev"),
            FlashError::ImageBadVectors,
        ),
        (
            make_image_with(2048, 0x2002_0000, BASE + 2048 + 1, true, "1.4.9_Dev"),
            FlashError::ImageBadVectors,
        ),
        (
            make_image_with(2048, 0x2002_0000, BASE + 1, false, "1.4.9_Dev"),
            FlashError::ImageNoHandshake,
        ),
        (
            make_image_with(512 * K + 4, 0x2002_0000, BASE + 1, false, ""),
            FlashError::ImageTooLarge,
        ),
    ];
    for (image, e) in cases {
        let mut rig = Rig::new(image);
        assert!(rig.begin());
        assert_eq!(rig.run(), FlashPhase::Failed);
        assert_eq!(rig.f.status().error, e);
        assert_eq!(rig.f.status().error_phase, FlashPhase::Validating);
        assert_eq!(rig.f.status().finished_ms, rig.now);
        assert!(rig.untouched());
        assert!(rig.steps <= 3);
    }
}

#[test]
fn image_read_error_during_validation_reports_the_offset() {
    let mut rig = Rig::new(make_image(4096));
    rig.img.fail_at = 1500;
    assert!(rig.begin());
    assert_eq!(rig.run(), FlashPhase::Failed);
    assert_eq!(rig.f.status().error, FlashError::ImageRead);
    assert_eq!(rig.f.status().error_address, 1280);
    assert!(rig.untouched());
}

#[test]
fn force_flashes_an_image_without_handshake_strings() {
    let mut rig = Rig::new(make_image_with(2048, 0x2002_0000, BASE + 1, false, ""));
    rig.opt.force = true;
    assert_eq!(rig.begin_and_run(), FlashPhase::Done);
    assert!(!rig.f.status().image.has_handshake);
    assert!(rig.flash_matches_image());
}

// ================================================================ handshake

#[test]
fn handshake_resyncs_the_v1_stms_fixed_8_byte_matcher() {
    for stray in 0..8 {
        let mut rig = Rig::new(make_image(1024));
        rig.sim.stray = stray;
        assert_eq!(rig.begin_and_run(), FlashPhase::Done, "{stray}");
        // At most 8 sends are needed to realign.
        let sends = rig.sim.writes_equal(b"DEADBEEF\n");
        assert!(sends >= 1);
        assert!(sends <= 8);
    }
}

#[test]
fn bytes_received_before_the_first_deadbeef_are_discarded() {
    let mut rig = Rig::new(make_image(1024));
    rig.sim.boot_noise = b("BEEFIT\r\n"); // stale bytes / reset glitch
    assert_eq!(rig.begin_and_run(), FlashPhase::Done);
    assert_eq!(rig.f.status().error, FlashError::None);
    assert!(!rig.sim.sync_times.is_empty());
    assert!(rig.sim.sync_times[0] - rig.sim.beefit_at >= 250);
}

#[test]
fn beefit_behind_noise_and_split_over_many_reads() {
    let mut rig = Rig::new(make_image(1024));
    rig.sim.beefit_prefix = b"\xFF\x00zzBEEF".to_vec();
    rig.sim.reply_spacing_ms = 3;
    assert_eq!(rig.begin_and_run(), FlashPhase::Done);
}

#[test]
fn handshake_timeout_resets_the_stm_back_to_its_application() {
    let mut rig = Rig::new(make_image(1024));
    rig.sim.has_boot_loop = false; // running app has no BootLoop
    assert!(rig.begin());
    assert_eq!(rig.run(), FlashPhase::Failed);
    let st = rig.f.status().clone();
    assert_eq!(st.error, FlashError::HandshakeTimeout);
    assert_eq!(st.error_phase, FlashPhase::Handshake);
    let release = rig.sim.resets[1].0;
    // Sends at +20, +120, ... +2420: 25 of them, none at/after 2500.
    let sends: Vec<u32> = rig
        .sim
        .writes
        .iter()
        .filter(|w| w.1 == b"DEADBEEF\n")
        .map(|w| w.0 - release)
        .collect();
    assert_eq!(sends.len(), 25);
    assert!((20..=22).contains(&sends[0]));
    for w in sends.windows(2) {
        assert!((100..=102).contains(&(w[1] - w[0])));
    }
    assert!(*sends.last().unwrap() < 2500);
    // Cleanup: another 100 ms pulse, UART 8N1.
    assert_eq!(rig.sim.resets.len(), 4);
    assert!((2500..=2502).contains(&(rig.sim.resets[2].0 - release)));
    assert!(rig.sim.resets[3].0 - rig.sim.resets[2].0 >= 100);
    assert_eq!(st.finished_ms, rig.sim.resets[3].0);
    assert!(rig.left_clean());
    assert!(rig.sim.programmed.is_empty());
    assert!(rig.sim.erase_frames.is_empty());
}

#[test]
fn custom_handshake_timing_options_are_honoured() {
    let mut rig = Rig::new(make_image(1024));
    rig.sim.has_boot_loop = false;
    rig.opt.handshake_first_ms = 50;
    rig.opt.handshake_repeat_ms = 300;
    rig.opt.handshake_window_ms = 1000;
    rig.opt.reset_pulse_ms = 60;
    assert!(rig.begin());
    assert_eq!(rig.run(), FlashPhase::Failed);
    let release = rig.sim.resets[1].0;
    assert!((60..=62).contains(&(release - rig.sim.resets[0].0)));
    let sends: Vec<u32> = rig.sim.writes.iter().map(|w| w.0 - release).collect();
    assert_eq!(sends.len(), 4); // 50, 350, 650, 950
    assert!((50..=52).contains(&sends[0]));
    assert!(sends[3] < 1000);
    assert!((1000..=1002).contains(&(rig.sim.resets[2].0 - release)));
}

// ================================================================ blank mode

#[test]
fn a_first_handshake_due_at_the_window_end_is_never_sent() {
    let mut rig = Rig::new(make_image(1024));
    rig.opt.handshake_first_ms = 2500; // == handshake_window_ms
    assert!(rig.begin());
    assert_eq!(rig.run_with(|_| {}, 60000, 1), FlashPhase::Failed);
    assert_eq!(rig.f.status().error, FlashError::HandshakeTimeout);
    assert_eq!(rig.f.status().error_address, 0);
    assert!(rig.sim.writes.is_empty());
    assert_eq!(rig.sim.resets.len(), 4);
    assert_eq!(rig.sim.resets[2].0 - rig.sim.resets[1].0, 2500);
    assert!(rig.left_clean());
}

#[test]
fn blank_mode_skips_the_handshake() {
    let mut rig = Rig::new(make_image(4096));
    rig.opt.blank = true;
    rig.sim.has_boot_loop = false;
    rig.sim.boot_pin_resets = 1; // user holds BOOT0 for the first reset only
    assert_eq!(rig.begin_and_run(), FlashPhase::Done);
    assert_eq!(rig.sim.writes_equal(b"DEADBEEF\n"), 0);
    assert_eq!(rig.sim.sync_times.len(), 1);
    assert!((250..=252).contains(&(rig.sim.sync_times[0] - rig.sim.resets[1].0)));
    assert!(rig.flash_matches_image());
    assert_eq!(rig.f.status().percent, 100);
}

// ================================================================ sync

#[test]
fn sync_second_0x7f_answered() {
    let mut rig = Rig::new(make_image(1024));
    rig.sim.sync_silent = 1;
    assert_eq!(rig.begin_and_run(), FlashPhase::Done);
    assert_eq!(rig.sim.sync_times.len(), 2);
    assert!((1000..=1002).contains(&(rig.sim.sync_times[1] - rig.sim.sync_times[0])));
}

#[test]
fn sync_third_0x7f_answered() {
    let mut rig = Rig::new(make_image(1024));
    rig.sim.sync_silent = 2;
    assert_eq!(rig.begin_and_run(), FlashPhase::Done);
}

#[test]
fn sync_never_answered() {
    let mut rig = Rig::new(make_image(1024));
    rig.opt.fallback_baud = 0;
    rig.sim.sync_silent = 100;
    assert_eq!(rig.begin_and_run(), FlashPhase::Failed);
    assert_eq!(rig.f.status().error, FlashError::SyncFailed);
    assert_eq!(rig.f.status().error_phase, FlashPhase::Sync);
    assert_eq!(rig.sim.sync_times.len(), 3);
    assert!(rig.left_clean());
}

#[test]
fn sync_attempts_0_behaves_like_1() {
    let mut rig = Rig::new(make_image(1024));
    rig.opt.fallback_baud = 0;
    rig.opt.sync_attempts = 0;
    rig.sim.sync_silent = 100;
    assert_eq!(rig.begin_and_run(), FlashPhase::Failed);
    assert_eq!(rig.f.status().error, FlashError::SyncFailed);
    assert_eq!(rig.sim.sync_times.len(), 1);
}

#[test]
fn sync_attempts_5() {
    let mut rig = Rig::new(make_image(1024));
    rig.opt.sync_attempts = 5;
    rig.sim.sync_silent = 4;
    assert_eq!(rig.begin_and_run(), FlashPhase::Done);
    assert_eq!(rig.sim.sync_times.len(), 5);
}

#[test]
fn sync_already_synced_bootloader_nacks_0x7f() {
    let mut rig = Rig::new(make_image(1024));
    rig.sim.sync_nack = true;
    assert_eq!(rig.begin_and_run(), FlashPhase::Done);
}

#[test]
fn sync_noise_before_every_ack() {
    let mut rig = Rig::new(make_image(1024));
    rig.sim.noise_replies = 1000;
    assert_eq!(rig.begin_and_run(), FlashPhase::Done);
    assert!(rig.flash_matches_image());
}

// ================================================================ GET / GET ID

#[test]
fn get_silent_get() {
    let mut rig = Rig::new(make_image(1024));
    rig.sim.get_silent = true;
    assert_eq!(rig.begin_and_run(), FlashPhase::Done);
    assert_eq!(rig.f.status().bootloader_version, 0);
}

#[test]
fn get_get_id_retried() {
    let mut rig = Rig::new(make_image(1024));
    rig.sim.get_id_silent = 3;
    assert_eq!(rig.begin_and_run(), FlashPhase::Done);
    assert_eq!(rig.f.status().chip_pid, 0x431);
}

#[test]
fn get_get_id_never_answers() {
    let mut rig = Rig::new(make_image(1024));
    rig.opt.fallback_baud = 0;
    rig.sim.get_id_silent = 4;
    assert_eq!(rig.begin_and_run(), FlashPhase::Failed);
    assert_eq!(rig.f.status().error, FlashError::Timeout);
    assert_eq!(rig.f.status().error_phase, FlashPhase::GetId);
    assert!(rig.sim.erase_frames.is_empty());
    assert!(rig.left_clean());
}

#[test]
fn get_get_id_reply_without_closing_ack() {
    let mut rig = Rig::new(make_image(1024));
    rig.sim.get_id_bad_end = 4;
    assert_eq!(rig.begin_and_run(), FlashPhase::Failed);
    assert_eq!(rig.f.status().error, FlashError::Nack);
    assert_eq!(rig.f.status().error_phase, FlashPhase::GetId);
}

#[test]
fn get_get_id_bad_end_once() {
    let mut rig = Rig::new(make_image(1024));
    rig.sim.get_id_bad_end = 1;
    assert_eq!(rig.begin_and_run(), FlashPhase::Done);
}

#[test]
fn get_block_retries_0_no_get_id_retry() {
    let mut rig = Rig::new(make_image(1024));
    rig.opt.fallback_baud = 0;
    rig.opt.block_retries = 0;
    rig.sim.get_id_silent = 1;
    assert_eq!(rig.begin_and_run(), FlashPhase::Failed);
    assert_eq!(rig.f.status().error, FlashError::Timeout);
}

#[test]
fn get_unknown_chip() {
    let mut rig = Rig::new(make_image(1024));
    rig.sim.pid = 0x413;
    assert_eq!(rig.begin_and_run(), FlashPhase::Failed);
    assert_eq!(rig.f.status().error, FlashError::UnknownChip);
    assert_eq!(rig.f.status().chip_pid, 0x413);
    assert!(rig.sim.erase_frames.is_empty());
    assert!(rig.left_clean());
    assert!(rig.sim.flash == rig.sim.original);
}

#[test]
fn get_one_byte_pid_reply() {
    let mut rig = Rig::new(make_image(1024));
    rig.sim.pid_n = 0;
    rig.sim.pid = 0x31;
    assert_eq!(rig.begin_and_run(), FlashPhase::Failed);
    assert_eq!(rig.f.status().error, FlashError::UnknownChip);
    assert_eq!(rig.f.status().chip_pid, 0x31);
}

#[test]
fn get_longer_pid_reply_uses_the_first_two_bytes() {
    let mut rig = Rig::new(make_image(1024));
    rig.sim.pid_n = 2;
    assert_eq!(rig.begin_and_run(), FlashPhase::Done);
    assert_eq!(rig.f.status().chip_pid, 0x431);
}

#[test]
fn get_f411_image_on_an_f401cc() {
    let mut rig = Rig::new(make_image_with(1024, 0x2002_0000, 0, true, "1.4.9_Dev"));
    rig.sim.pid = 0x423;
    assert_eq!(rig.begin_and_run(), FlashPhase::Failed);
    assert_eq!(rig.f.status().error, FlashError::ImageChipMismatch);
    assert_eq!(rig.f.status().error_phase, FlashPhase::GetId);
    assert!(rig.sim.flash == rig.sim.original);
}

#[test]
fn get_image_larger_than_the_f401cc_flash() {
    let mut rig = Rig::new(make_image_with(300 * K, 0x2001_0000, 0, true, "1.4.9_Dev"));
    rig.sim.pid = 0x423;
    assert_eq!(rig.begin_and_run(), FlashPhase::Failed);
    assert_eq!(rig.f.status().error, FlashError::ImageTooLarge);
}

#[test]
fn get_f401_image_on_f401xe_and_f411() {
    for pid in [0x423, 0x433, 0x431] {
        let mut rig = Rig::new(make_image_with(1024, 0x2001_0000, 0, true, "1.4.9_Dev"));
        rig.sim.pid = pid;
        assert_eq!(rig.begin_and_run(), FlashPhase::Done, "{pid:#x}");
        assert_eq!(rig.f.status().chip_pid, pid);
    }
}

// ================================================================ erase

#[test]
fn erase_nack_once() {
    let mut rig = Rig::new(make_image(1024));
    rig.sim.nack_erase = 1;
    assert_eq!(rig.begin_and_run(), FlashPhase::Done);
    assert_eq!(rig.f.status().attempt, 1);
}

#[test]
fn erase_nack_always_write_protection() {
    let mut rig = Rig::new(make_image(1024));
    rig.sim.nack_erase = 100;
    assert_eq!(rig.begin_and_run(), FlashPhase::Failed);
    let st = rig.f.status();
    assert_eq!(st.error, FlashError::Nack);
    assert_eq!(st.error_phase, FlashPhase::Erasing);
    assert_eq!(st.error_address, BASE);
    assert_eq!(st.attempt, 2);
    assert_eq!(rig.sim.writes_equal(&[0x44, 0xBB]), 3);
    assert!(rig.left_clean());
}

#[test]
fn erase_session_retries_0() {
    let mut rig = Rig::new(make_image(1024));
    rig.opt.session_retries = 0;
    rig.sim.nack_erase = 1;
    assert_eq!(rig.begin_and_run(), FlashPhase::Failed);
    assert_eq!(rig.sim.writes_equal(&[0x44, 0xBB]), 1);
}

#[test]
fn erase_done_ack_lost() {
    let mut rig = Rig::new(make_image(1024));
    rig.opt.erase_timeout_ms = 5000;
    rig.sim.drop_erase_ack = 1;
    let (mut first_erase, mut second_erase) = (0u32, 0u32);
    assert!(rig.begin());
    rig.run_hook(|r| {
        if r.sim.erase_frames.len() == 1 && first_erase == 0 {
            first_erase = r.now;
        }
        if r.sim.erase_frames.len() == 2 && second_erase == 0 {
            second_erase = r.now;
        }
    });
    assert_eq!(rig.f.status().phase, FlashPhase::Done);
    assert!((5000..=5010).contains(&(second_erase - first_erase)));
}

#[test]
fn erase_waits_past_the_ack_timeout() {
    let mut rig = Rig::new(make_image(1024));
    rig.sim.erase_delay_ms = 20000; // > ack_timeout_ms, < erase_timeout_ms
    assert_eq!(rig.begin_and_run(), FlashPhase::Done);
    assert_eq!(rig.f.status().attempt, 0);
}

// ================================================================ write

#[test]
fn write_nack_once() {
    let mut rig = Rig::new(make_image(2048));
    rig.sim.nack_write_data = 1;
    assert_eq!(rig.begin_and_run(), FlashPhase::Done);
    assert_eq!(rig.f.status().attempt, 0);
    assert_eq!(rig.sim.writes_equal(&[0x31, 0xCE]), 9);
}

#[test]
fn write_nack_three_times_still_the_same_session() {
    let mut rig = Rig::new(make_image(2048));
    rig.sim.nack_write_data = 3;
    assert_eq!(rig.begin_and_run(), FlashPhase::Done);
    assert_eq!(rig.f.status().attempt, 0);
    assert_eq!(rig.sim.erase_frames.len(), 1);
}

#[test]
fn write_nack_four_times_re_erase_in_the_same_rom_session() {
    let mut rig = Rig::new(make_image(2048));
    rig.sim.nack_write_data = 4;
    assert_eq!(rig.begin_and_run(), FlashPhase::Done);
    assert_eq!(rig.f.status().attempt, 1);
    assert_eq!(rig.sim.erase_frames.len(), 2);
    assert_eq!(rig.sim.resets.len(), 4); // no extra reset between sessions
    assert!(rig.flash_matches_image());
    assert!(rig.percent_monotonic());
}

#[test]
fn write_lost_data_ack() {
    let mut rig = Rig::new(make_image(2048));
    rig.sim.drop_write_ack = 2;
    assert_eq!(rig.begin_and_run(), FlashPhase::Done);
    assert!(rig.flash_matches_image());
}

#[test]
fn write_a_block_that_never_programs() {
    let mut rig = Rig::new(make_image(2048));
    rig.sim.nack_write_addr.insert(BASE + 0x300);
    assert_eq!(rig.begin_and_run(), FlashPhase::Failed);
    let st = rig.f.status();
    assert_eq!(st.error, FlashError::Nack);
    assert_eq!(st.error_phase, FlashPhase::Writing);
    assert_eq!(st.error_address, BASE + 0x300);
    assert_eq!(st.attempt, 2);
    assert_eq!(rig.sim.erase_frames.len(), 3);
    // 3 sessions x (1 + 3 retries) attempts at that block.
    assert_eq!(rig.sim.writes_equal(&[0x08, 0x00, 0x03, 0x00, 0x0B]), 12);
    // Block 0 (vector table) was never written: the chip stays "blank".
    assert!(rig.sim.programmed.iter().all(|p| p.addr != BASE));
    assert!(rig.left_clean());
}

#[test]
fn write_readout_protection() {
    let mut rig = Rig::new(make_image(2048));
    rig.sim.rdp = true;
    assert_eq!(rig.begin_and_run(), FlashPhase::Failed);
    assert_eq!(rig.f.status().error, FlashError::Nack);
    assert_eq!(rig.f.status().error_phase, FlashPhase::Erasing);
}

/// Times of the write commands (0x31 0xCE) of a run.
fn write_command_times(rig: &mut Rig) -> Vec<u32> {
    let mut seen = 0;
    let mut times = Vec::new();
    rig.run_hook(|r| {
        for w in &r.sim.writes[seen..] {
            if w.1 == [0x31, 0xCE] {
                times.push(w.0);
            }
        }
        seen = r.sim.writes.len();
    });
    times
}

#[test]
fn write_data_ack_timeout_is_ack_timeout_plus_the_frames_wire_time() {
    let mut rig = Rig::new(make_image(1024));
    rig.opt.ack_timeout_ms = 300;
    rig.sim.drop_write_ack = 1;
    assert!(rig.begin());
    let cmd_times = write_command_times(&mut rig);
    assert_eq!(rig.f.status().phase, FlashPhase::Done);
    assert!(cmd_times.len() >= 2);
    // 258 bytes x 11 bits at 115200 = 24.6 -> 25 ms.
    assert!((325..=345).contains(&(cmd_times[1] - cmd_times[0])));
}

#[test]
fn write_slow_baud_a_data_frame_still_on_the_wire_is_not_timed_out() {
    // 258 bytes x 11 bits at 1200 baud = 2365 ms, more than ack_timeout_ms.
    let mut rig = Rig::new(make_image(1024));
    rig.opt.baud = 1200;
    rig.opt.blank = true;
    rig.opt.ack_timeout_ms = 300;
    rig.sim.has_boot_loop = false;
    rig.sim.boot_pin_resets = 1;
    rig.sim.drop_write_ack = 1;
    assert!(rig.begin());
    let cmd_times = write_command_times(&mut rig);
    assert_eq!(rig.f.status().phase, FlashPhase::Done);
    assert!(rig.flash_matches_image());
    assert!(cmd_times.len() >= 2);
    // 300 ms + 2365 ms wire time of the data frame.
    assert!((2665..=2700).contains(&(cmd_times[1] - cmd_times[0])));
}

// ================================================================ verify

#[test]
fn verify_transient_read_corruption_is_retried() {
    let mut rig = Rig::new(make_image(2048));
    rig.sim.corrupt_reads = 1;
    assert_eq!(rig.begin_and_run(), FlashPhase::Done);
    assert_eq!(rig.sim.read_addrs.len(), 9);
    assert_eq!(rig.sim.read_addrs[0], BASE);
    assert_eq!(rig.sim.read_addrs[1], BASE);
}

#[test]
fn verify_stuck_flash_bit() {
    let mut rig = Rig::new(make_image(2048));
    rig.sim.stuck.insert(BASE + 0x345);
    assert_eq!(rig.begin_and_run(), FlashPhase::Failed);
    let st = rig.f.status();
    assert_eq!(st.error, FlashError::VerifyMismatch);
    assert_eq!(st.error_phase, FlashPhase::Verifying);
    assert_eq!(st.error_address, BASE + 0x345);
    assert_eq!(st.attempt, 2);
    assert!(rig.left_clean());
}

#[test]
fn verify_stuck_bit_in_the_first_and_last_byte_of_a_block() {
    for a in [BASE + 0x100, BASE + 0x1FF, BASE + 0x7FF] {
        let mut rig = Rig::new(make_image(2048));
        rig.opt.session_retries = 0;
        rig.opt.block_retries = 0;
        rig.sim.stuck.insert(a);
        assert_eq!(rig.begin_and_run(), FlashPhase::Failed);
        assert_eq!(rig.f.status().error_address, a);
    }
}

#[test]
fn verify_image_changed_on_disk_after_writing() {
    let mut rig = Rig::new(make_image(2048));
    assert!(rig.begin());
    let mut changed = false;
    rig.run_hook(|r| {
        if !changed && r.f.status().phase == FlashPhase::Verifying {
            // Same change on disk and in flash: bytes compare equal, CRC does not.
            r.img.data[1500] ^= 0x10;
            r.sim.flash[1500] = r.img.data[1500];
            changed = true;
        }
    });
    assert_eq!(rig.f.status().phase, FlashPhase::Failed);
    assert_eq!(rig.f.status().error, FlashError::ImageRead);
    assert_eq!(rig.f.status().error_phase, FlashPhase::Verifying);
    assert!(rig.left_clean());
}

#[test]
fn verify_read_data_slower_than_the_ack_timeout_still_fits_the_data_budget() {
    // 257 reply bytes 1 ms apart: 50 ms ACK + 25 ms wire + 200 ms margin.
    let mut rig = Rig::new(make_image(1024));
    rig.opt.ack_timeout_ms = 50;
    rig.sim.reply_spacing_ms = 1;
    assert_eq!(rig.begin_and_run(), FlashPhase::Done);
    assert_eq!(rig.sim.read_addrs.len(), 4);
}

#[test]
fn verify_read_data_beyond_the_data_budget_times_out() {
    let mut rig = Rig::new(make_image(1024));
    rig.opt.ack_timeout_ms = 50;
    rig.opt.block_retries = 0;
    rig.opt.session_retries = 0;
    rig.sim.reply_spacing_ms = 2;
    assert_eq!(rig.begin_and_run(), FlashPhase::Failed);
    assert_eq!(rig.f.status().error, FlashError::Timeout);
    assert_eq!(rig.f.status().error_phase, FlashPhase::Verifying);
    assert_eq!(rig.f.status().error_address, BASE);
}

#[test]
fn image_read_failure_during_writing() {
    let mut rig = Rig::new(make_image(2048));
    assert!(rig.begin());
    rig.run_hook(|r| {
        if r.f.status().phase == FlashPhase::Erasing {
            r.img.fail_at = 1030;
        }
    });
    assert_eq!(rig.f.status().phase, FlashPhase::Failed);
    assert_eq!(rig.f.status().error, FlashError::ImageRead);
    assert_eq!(rig.f.status().error_phase, FlashPhase::Writing);
    assert_eq!(rig.f.status().error_address, BASE + 0x400);
    assert!(rig.left_clean());
}

// ================================================================ transport

#[test]
fn short_write_during_the_handshake() {
    let mut rig = Rig::new(make_image(1024));
    rig.sim.write_limit = 4;
    assert_eq!(rig.begin_and_run(), FlashPhase::Failed);
    assert_eq!(rig.f.status().error, FlashError::TransportWrite);
    assert_eq!(rig.f.status().error_phase, FlashPhase::Handshake);
    assert_eq!(rig.sim.writes.len(), 1);
    assert!(rig.left_clean());
}

#[test]
fn short_write_on_the_first_data_frame() {
    let mut rig = Rig::new(make_image(1024));
    rig.sim.write_limit = 200;
    assert_eq!(rig.begin_and_run(), FlashPhase::Failed);
    assert_eq!(rig.f.status().error, FlashError::TransportWrite);
    assert_eq!(rig.f.status().error_phase, FlashPhase::Writing);
    assert!(rig.left_clean());
}

#[test]
fn short_write_while_polling_the_application() {
    let mut rig = Rig::new(make_image(1024));
    assert!(rig.begin());
    rig.run_hook(|r| {
        if r.f.status().phase == FlashPhase::WaitingApp {
            r.sim.write_limit = 3;
        }
    });
    assert_eq!(rig.f.status().phase, FlashPhase::Failed);
    assert_eq!(rig.f.status().error, FlashError::TransportWrite);
    assert_eq!(rig.f.status().error_phase, FlashPhase::WaitingApp);
    assert_eq!(rig.sim.resets.len(), 4); // no extra pulse: the app already runs
}

// ================================================================ abort

#[test]
fn abort_while_validating_nothing_touched() {
    let mut rig = Rig::new(make_image(64 * K));
    assert!(rig.begin());
    rig.now += 2;
    rig.step();
    rig.f.abort();
    assert!(rig.f.active());
    rig.now += 2;
    assert_eq!(rig.step(), FlashPhase::Failed);
    assert_eq!(rig.f.status().error, FlashError::Aborted);
    assert_eq!(rig.f.status().error_phase, FlashPhase::Validating);
    assert!(rig.untouched());
}

#[test]
fn abort_while_writing_pulse_8n1_then_failed() {
    let mut rig = Rig::new(make_image(8 * K));
    assert!(rig.begin());
    let mut aborted = false;
    rig.run_hook(|r| {
        if !aborted && r.f.status().phase == FlashPhase::Writing && r.sim.programmed.len() == 3 {
            r.f.abort();
            aborted = true;
        }
    });
    let st = rig.f.status();
    assert_eq!(st.phase, FlashPhase::Failed);
    assert_eq!(st.error, FlashError::Aborted);
    assert_eq!(st.error_phase, FlashPhase::Writing);
    assert!(rig.sim.programmed.len() <= 4);
    assert!(rig.left_clean());
    assert_eq!(rig.sim.resets.len(), 4);
    assert!(rig.sim.resets[3].0 - rig.sim.resets[2].0 >= 100);
}

#[test]
fn abort_during_the_cleanup_pulse_it_is_ignored() {
    let mut rig = Rig::new(make_image(1024));
    rig.sim.has_boot_loop = false;
    assert!(rig.begin());
    let mut once = false;
    rig.run_hook(|r| {
        if !once && r.f.status().error == FlashError::HandshakeTimeout {
            r.f.abort();
            once = true;
        }
    });
    assert_eq!(rig.f.status().error, FlashError::HandshakeTimeout);
    assert_eq!(rig.sim.resets.len(), 4);
}

#[test]
fn abort_while_waiting_for_the_application_no_extra_pulse() {
    let mut rig = Rig::new(make_image(1024));
    assert!(rig.begin());
    rig.run_hook(|r| {
        if r.f.status().phase == FlashPhase::WaitingApp {
            r.f.abort();
        }
    });
    assert_eq!(rig.f.status().phase, FlashPhase::Failed);
    assert_eq!(rig.f.status().error, FlashError::Aborted);
    assert_eq!(rig.f.status().error_phase, FlashPhase::WaitingApp);
    assert_eq!(rig.sim.resets.len(), 4);
    assert!(rig.left_clean());
}

#[test]
fn abort_before_begin_does_not_leak_into_the_next_run() {
    let mut rig = Rig::new(make_image(1024));
    rig.f.abort();
    assert_eq!(rig.begin_and_run(), FlashPhase::Done);
}

#[test]
fn abort_after_done_does_nothing() {
    let mut rig = Rig::new(make_image(1024));
    assert_eq!(rig.begin_and_run(), FlashPhase::Done);
    rig.f.abort();
    rig.now += 2;
    assert_eq!(rig.step(), FlashPhase::Done);
    assert_eq!(rig.f.status().error, FlashError::None);
}

// ================================================================ application

#[test]
fn app_no_answer_gvers_at_4_s_then_every_1_s_until_15_s() {
    let mut rig = Rig::new(make_image(1024));
    rig.sim.app_answers = false;
    rig.opt.app_timeout_ms = 15000;
    assert_eq!(rig.begin_and_run(), FlashPhase::Failed);
    assert_eq!(rig.f.status().error, FlashError::AppNotResponding);
    assert_eq!(rig.f.status().error_phase, FlashPhase::WaitingApp);
    let release = rig.sim.resets.last().unwrap().0;
    assert_eq!(rig.sim.gvers_times.len(), 11);
    for (i, &t) in rig.sim.gvers_times.iter().enumerate() {
        let i = i as u32;
        assert!(t - release >= 4000 + 1000 * i);
        assert!(t - release <= 4000 + 1000 * i + 2 * (i + 1));
    }
    assert!((15000..=15002).contains(&(rig.f.status().finished_ms - release)));
    assert_eq!(rig.sim.resets.len(), 4);
    assert!(rig.left_clean());
    assert!(rig.flash_matches_image());
}

#[test]
fn app_late_answer_within_the_budget() {
    let mut rig = Rig::new(make_image(1024));
    rig.sim.app_answers = false;
    assert!(rig.begin());
    rig.run_hook(|r| {
        if r.sim.gvers_times.len() == 5 {
            r.sim.app_answers = true;
        }
    });
    assert_eq!(rig.f.status().phase, FlashPhase::Done);
    assert_eq!(rig.sim.gvers_times.len(), 6);
}

#[test]
fn app_version_mismatch() {
    let mut rig = Rig::new(make_image(1024));
    rig.sim.app_reply = b("gvers 1.4.8_Dev_C1 1 ");
    assert_eq!(rig.begin_and_run(), FlashPhase::Failed);
    assert_eq!(rig.f.status().error, FlashError::AppVersionMismatch);
    assert_eq!(rig.f.status().app_version.patch, 8);
    assert_eq!(rig.sim.resets.len(), 4);
}

#[test]
fn app_suffix_mismatch() {
    let mut rig = Rig::new(make_image(1024));
    rig.sim.app_reply = b("gvers 1.4.9_C1 1 ");
    assert_eq!(rig.begin_and_run(), FlashPhase::Failed);
    assert_eq!(rig.f.status().error, FlashError::AppVersionMismatch);
}

#[test]
fn app_force_skips_the_version_check() {
    let mut rig = Rig::new(make_image(1024));
    rig.opt.force = true;
    rig.sim.app_reply = b("gvers 1.4.8_Dev_C1 1 ");
    assert_eq!(rig.begin_and_run(), FlashPhase::Done);
    assert_eq!(rig.f.status().app_version.patch, 8);
}

#[test]
fn app_image_without_a_version_string_accepts_any_version() {
    let mut rig = Rig::new(make_image_with(1024, 0x2002_0000, 0, true, ""));
    rig.sim.app_reply = b("gvers 7.0.1-revamped_C2 ");
    assert_eq!(rig.begin_and_run(), FlashPhase::Done);
    assert_eq!(rig.f.status().app_version.major, 7);
}

#[test]
fn app_image_version_with_a_hw_tag_must_match_it() {
    let mut a = Rig::new(make_image_with(1024, 0x2002_0000, 0, true, "1.4.9_C1"));
    a.sim.app_reply = b("gvers 1.4.9_C2 1 ");
    assert_eq!(a.begin_and_run(), FlashPhase::Failed);
    assert_eq!(a.f.status().error, FlashError::AppVersionMismatch);
    let mut b2 = Rig::new(make_image_with(1024, 0x2002_0000, 0, true, "1.4.9_C1"));
    b2.sim.app_reply = b("gvers 1.4.9_C1 1 ");
    assert_eq!(b2.begin_and_run(), FlashPhase::Done);
}

#[test]
fn app_revamped_version_without_build() {
    let mut rig = Rig::new(make_image_with(
        1024,
        0x2002_0000,
        0,
        true,
        "2.0.0-revamped",
    ));
    rig.sim.app_reply = b("gvers 2.0.0-revamped_C2 ");
    assert_eq!(rig.begin_and_run(), FlashPhase::Done);
}

#[test]
fn app_noise_and_other_lines_before_the_reply_are_ignored() {
    let mut rig = Rig::new(make_image(1024));
    let mut noise = b"\xFF\xFE garbage\r\n".to_vec();
    noise.extend_from_slice(b"gvlst 12 1,1,\r\n");
    noise.extend_from_slice(b"gversX 9.9.9\r\n");
    noise.extend_from_slice(b"gvers\r\n");
    noise.extend_from_slice(b"gvers   \r\n");
    noise.extend_from_slice(&[b'z'; 400]);
    noise.extend_from_slice(b"\r\n");
    noise.extend_from_slice(b"gvers 1.4\r\n");
    noise.extend_from_slice(b"gvers 1.4.9_Dev_C1\x01\r\n");
    rig.sim.app_noise = noise;
    assert_eq!(rig.begin_and_run(), FlashPhase::Done);
    assert_eq!(&rig.f.status().app_version.suffix[..], b"_Dev");
}

#[test]
fn app_custom_app_timing() {
    let mut rig = Rig::new(make_image(1024));
    rig.sim.app_answers = false;
    rig.opt.app_boot_ms = 500;
    rig.opt.app_poll_ms = 250;
    rig.opt.app_timeout_ms = 1500;
    assert_eq!(rig.begin_and_run(), FlashPhase::Failed);
    assert_eq!(rig.sim.writes_equal(b"gvers \r\n"), 4); // 500, 750, 1000, 1250
    assert_eq!(
        rig.f.status().finished_ms - rig.sim.resets.last().unwrap().0,
        1500
    );
}

// ================================================================ exact timing

#[test]
fn exact_timing_successful_run() {
    let mut rig = Rig::new(make_image(1024));
    assert!(rig.begin());
    assert_eq!(rig.run_with(|_| {}, 60000, 1), FlashPhase::Done);
    let resets = &rig.sim.resets;
    assert_eq!(resets.len(), 4);
    let release = resets[1].0;
    assert_eq!(release - resets[0].0, 100);
    let first_handshake = rig
        .sim
        .writes
        .iter()
        .find(|w| w.1 == b"DEADBEEF\n")
        .map(|w| w.0)
        .unwrap();
    assert_eq!(first_handshake - release, 20);
    assert!(!rig.sim.sync_times.is_empty());
    assert_eq!(rig.sim.sync_times[0] - rig.sim.beefit_at, 250);
    assert_eq!(resets[3].0 - resets[2].0, 100);
    assert_eq!(rig.sim.gvers_times.len(), 1);
    assert_eq!(rig.sim.gvers_times[0] - resets[3].0, 4000);
}

#[test]
fn exact_timing_handshake_window() {
    let mut rig = Rig::new(make_image(1024));
    rig.sim.has_boot_loop = false;
    assert!(rig.begin());
    assert_eq!(rig.run_with(|_| {}, 60000, 1), FlashPhase::Failed);
    assert_eq!(rig.f.status().error, FlashError::HandshakeTimeout);
    assert_eq!(rig.sim.resets.len(), 4);
    let release = rig.sim.resets[1].0;
    let sends: Vec<u32> = rig
        .sim
        .writes
        .iter()
        .filter(|w| w.1 == b"DEADBEEF\n")
        .map(|w| w.0 - release)
        .collect();
    assert_eq!(sends.len(), 25);
    for (i, &s) in sends.iter().enumerate() {
        assert_eq!(s, 20 + 100 * i as u32);
    }
    assert_eq!(rig.sim.resets[2].0 - release, 2500);
    assert_eq!(rig.sim.resets[3].0 - rig.sim.resets[2].0, 100);
    assert_eq!(rig.f.status().finished_ms, rig.sim.resets[3].0);
}

#[test]
fn exact_timing_sync_retry_after_ack_timeout_plus_1_ms_wire_time() {
    let mut rig = Rig::new(make_image(1024));
    rig.sim.sync_silent = 1;
    assert!(rig.begin());
    assert_eq!(rig.run_with(|_| {}, 60000, 1), FlashPhase::Done);
    assert_eq!(rig.sim.sync_times.len(), 2);
    assert_eq!(rig.sim.sync_times[1] - rig.sim.sync_times[0], 1001);
}

#[test]
fn exact_timing_application_polling() {
    let mut rig = Rig::new(make_image(1024));
    rig.sim.app_answers = false;
    rig.opt.app_timeout_ms = 15000;
    assert!(rig.begin());
    assert_eq!(rig.run_with(|_| {}, 60000, 1), FlashPhase::Failed);
    let release = rig.sim.resets.last().unwrap().0;
    assert_eq!(rig.sim.gvers_times.len(), 11);
    for (i, &t) in rig.sim.gvers_times.iter().enumerate() {
        assert_eq!(t - release, 4000 + 1000 * i as u32);
    }
    assert_eq!(rig.f.status().finished_ms - release, 15000);
}

#[test]
fn exact_timing_read_back_data_budget_at_1200_baud_both_sides_of_the_limit() {
    // Per read: ACK at +2 ms, then 256 more bytes 11 ms apart: last at +2818.
    // Budget: ack_timeout_ms + wire(2 B) 19 + wire(257 B) 2356 + 200.
    for ack in [243u16, 242] {
        let mut rig = Rig::new(make_image(1024));
        rig.opt.baud = 1200;
        rig.opt.ack_timeout_ms = ack;
        rig.opt.block_retries = 0;
        rig.opt.session_retries = 0;
        rig.sim.reply_spacing_ms = 11;
        assert!(rig.begin());
        rig.run_with(|_| {}, 120_000, 1);
        if ack == 243 {
            assert_eq!(rig.f.status().phase, FlashPhase::Done);
            assert_eq!(rig.sim.read_addrs.len(), 4);
        } else {
            assert_eq!(rig.f.status().phase, FlashPhase::Failed);
            assert_eq!(rig.f.status().error, FlashError::Timeout);
            assert_eq!(rig.f.status().error_phase, FlashPhase::Verifying);
            assert_eq!(rig.f.status().error_address, BASE);
        }
    }
}

#[test]
fn instant_replies_chain_exactly_one_block_per_step() {
    let mut rig = Rig::new(make_image(16 * K)); // 64 blocks
    rig.sim.instant_replies = true;
    assert!(rig.begin());
    let mut seen = 0;
    let mut max_frames = 0;
    let mut data_times = Vec::new();
    let mut read_times = Vec::new();
    rig.run_with(
        |r| {
            let mut frames = 0;
            for w in &r.sim.writes[seen..] {
                if w.1.len() > 16 {
                    frames += 1;
                    data_times.push(w.0);
                }
                if w.1 == [0xFF, 0x00] {
                    read_times.push(w.0);
                }
            }
            seen = r.sim.writes.len();
            max_frames = max_frames.max(frames);
        },
        60000,
        1,
    );
    assert_eq!(rig.f.status().phase, FlashPhase::Done);
    assert_eq!(max_frames, 1);
    assert_eq!(data_times.len(), 64);
    assert_eq!(read_times.len(), 64);
    assert_eq!(data_times.last().unwrap() - data_times[0], 63);
    assert_eq!(read_times.last().unwrap() - read_times[0], 63);
}

// ================================================================ more failure detail

type Setup = fn(&mut Rig);
type Hook = Box<dyn FnMut(&mut Rig)>;

#[test]
fn failures_without_a_flash_address_report_address_0() {
    let changed = Rc::new(Cell::new(false));
    let image_changed = {
        let changed = changed.clone();
        move |r: &mut Rig| {
            // Same change on disk and in flash: bytes compare equal, CRC does not.
            if !changed.get() && r.f.status().phase == FlashPhase::Verifying {
                r.img.data[700] ^= 0x10;
                r.sim.flash[700] = r.img.data[700];
                changed.set(true);
            }
        }
    };
    let none: Setup = |_| {};
    let cases: Vec<(&str, FlashError, Setup, Hook)> = vec![
        (
            "empty image",
            FlashError::ImageEmpty,
            |r| r.img.data.clear(),
            Box::new(|_| {}),
        ),
        (
            "no handshake strings",
            FlashError::ImageNoHandshake,
            |r| r.img.data = make_image_with(1024, 0x2002_0000, 0, false, "1.4.9_Dev"),
            Box::new(|_| {}),
        ),
        (
            "handshake timeout",
            FlashError::HandshakeTimeout,
            |r| r.sim.has_boot_loop = false,
            Box::new(|_| {}),
        ),
        (
            "sync failed",
            FlashError::SyncFailed,
            |r| r.sim.sync_silent = 100,
            Box::new(|_| {}),
        ),
        (
            "GET ID timeout",
            FlashError::Timeout,
            |r| r.sim.get_id_silent = 100,
            Box::new(|_| {}),
        ),
        (
            "GET ID NACK",
            FlashError::Nack,
            |r| r.sim.get_id_bad_end = 100,
            Box::new(|_| {}),
        ),
        (
            "unknown chip",
            FlashError::UnknownChip,
            |r| r.sim.pid = 0x413,
            Box::new(|_| {}),
        ),
        (
            "short write",
            FlashError::TransportWrite,
            |r| r.sim.write_limit = 4,
            Box::new(|_| {}),
        ),
        (
            "abort while writing",
            FlashError::Aborted,
            none,
            Box::new(|r| {
                if r.f.status().phase == FlashPhase::Writing {
                    r.f.abort();
                }
            }),
        ),
        (
            "image changed during the run",
            FlashError::ImageRead,
            none,
            Box::new(image_changed),
        ),
        (
            "application silent",
            FlashError::AppNotResponding,
            |r| r.sim.app_answers = false,
            Box::new(|_| {}),
        ),
        (
            "application version",
            FlashError::AppVersionMismatch,
            |r| r.sim.app_reply = b("gvers 1.4.8_Dev_C1 1 "),
            Box::new(|_| {}),
        ),
    ];
    for (name, error, setup, mut hook) in cases {
        let mut rig = Rig::new(make_image(1024));
        setup(&mut rig);
        assert!(rig.begin());
        assert_eq!(rig.run_hook(&mut hook), FlashPhase::Failed, "{name}");
        assert_eq!(rig.f.status().error, error, "{name}");
        assert_eq!(rig.f.status().error_address, 0, "{name}");
        assert_eq!(rig.left_clean(), !rig.sim.resets.is_empty(), "{name}");
    }
    assert!(changed.get()); // Rust addition: the image-changed hook ran
}

#[test]
fn verify_retries_are_counted_per_block() {
    // One corrupted read-back on two different blocks with block_retries 1: each block has its
    // own retry, so no session retry is needed.
    let mut rig = Rig::new(make_image(2048));
    rig.opt.block_retries = 1;
    rig.sim.corrupt_read_at = vec![BASE + 0x100, BASE + 0x300];
    assert_eq!(rig.begin_and_run(), FlashPhase::Done);
    assert_eq!(rig.f.status().attempt, 0);
    assert_eq!(rig.sim.erase_frames.len(), 1);
    assert_eq!(rig.sim.read_addrs.len(), 10);
}

#[test]
fn beefit_detection_behind_noise_in_the_same_read() {
    let mut rig = Rig::new(make_image(1024));
    rig.sim.beefit_prefix = b("zzBEEF");
    assert_eq!(rig.begin_and_run(), FlashPhase::Done);
}

#[test]
fn beefit_detection_split_across_two_answers() {
    let mut rig = Rig::new(make_image(1024));
    rig.sim.has_boot_loop = false;
    rig.sim.boot_pin_resets = 1; // already in the ROM bootloader; the echo plays BEEFIT
    rig.sim.echoes = [b("xxBEE"), b("FIT\r\n")].into_iter().collect();
    assert_eq!(rig.begin_and_run(), FlashPhase::Done);
    assert_eq!(rig.sim.writes_equal(b"DEADBEEF\n"), 2);
}

#[test]
fn beefit_detection_near_misses_never_match() {
    let mut rig = Rig::new(make_image(1024));
    rig.sim.has_boot_loop = false;
    rig.sim.echo_repeat = b("BEEFIxBEEFI\r\nBEEFT beefit");
    assert_eq!(rig.begin_and_run(), FlashPhase::Failed);
    assert_eq!(rig.f.status().error, FlashError::HandshakeTimeout);
    assert!(rig.sim.sync_times.is_empty());
}

#[test]
fn app_reply_a_wrong_version_on_a_line_with_a_control_character_is_dropped() {
    let mut rig = Rig::new(make_image(1024));
    rig.sim.app_noise = b("\x01gvers 1.4.8_Dev_C1 1 \r\ngvers 1.4.8\x02_Dev_C1 1 \r\n");
    assert_eq!(rig.begin_and_run(), FlashPhase::Done);
    assert_eq!(rig.f.status().app_version.patch, 9);
}

#[test]
fn app_reply_tilde_is_printable() {
    let mut rig = Rig::new(make_image(1024));
    rig.sim.app_reply = b("gvers 1.4.9_Dev_C1 1~ ");
    assert_eq!(rig.begin_and_run(), FlashPhase::Done);
}

#[test]
fn app_reply_extra_spaces_before_the_version() {
    let mut rig = Rig::new(make_image(1024));
    rig.sim.app_reply = b("gvers   1.4.9_Dev_C1 1 ");
    assert_eq!(rig.begin_and_run(), FlashPhase::Done);
}

#[test]
fn app_reply_boot_noise_without_a_line_end_is_discarded_before_the_first_gvers() {
    let mut rig = Rig::new(make_image(1024));
    rig.sim.boot_noise = b("zz");
    assert_eq!(rig.begin_and_run(), FlashPhase::Done);
    assert_eq!(rig.sim.gvers_times.len(), 1);
}

// ================================================================ fuzz

#[test]
fn fixed_seed_fault_fuzz_always_ends_clean() {
    let mut r = SimLcg::new(0xF1A5);
    let (mut done, mut failed) = (0, 0);
    for iter in 0..120u32 {
        let size = 128 + 4 * r.below(700) as usize;
        let mut rig = Rig::new(make_image_seeded(
            size,
            0x2002_0000,
            BASE + 1,
            true,
            "1.4.9_Dev",
            iter + 1,
        ));
        rig.opt.ack_timeout_ms = 200;
        rig.opt.erase_timeout_ms = 2000;
        rig.sim.stray = r.below(8) as i32;
        rig.sim.sync_silent = r.below(4) as i32;
        rig.sim.get_id_silent = r.below(3) as i32;
        rig.sim.nack_erase = r.below(3) as i32;
        rig.sim.drop_erase_ack = r.below(2) as i32;
        rig.sim.nack_write_data = r.below(6) as i32;
        rig.sim.drop_write_ack = r.below(3) as i32;
        rig.sim.corrupt_reads = r.below(3) as i32;
        rig.sim.corrupt_offset = 0;
        rig.sim.noise_replies = r.below(20) as i32;
        rig.sim.reply_spacing_ms = r.below(2);
        rig.sim.erase_delay_ms = 50 + r.below(500);
        if r.below(10) == 0 {
            rig.sim.stuck.insert(BASE + r.below(size as u32));
        }
        if r.below(10) == 0 {
            rig.sim.app_answers = false;
        }
        let end = rig.begin_and_run();
        assert!(
            end == FlashPhase::Done || end == FlashPhase::Failed,
            "{iter}"
        );
        let st = rig.f.status();
        let why = format!(
            "{iter}: {} in {}",
            flash_error_name(st.error),
            flash_phase_name(st.error_phase)
        );
        assert!(rig.left_clean(), "{why}");
        assert!(rig.percent_monotonic(), "{why}");
        assert_eq!(st.finished_ms, rig.now, "{why}");
        if end == FlashPhase::Done {
            done += 1;
            assert!(rig.flash_matches_image(), "{why}");
            assert_eq!(st.error, FlashError::None, "{why}");
        } else {
            failed += 1;
            assert_ne!(st.error, FlashError::None, "{why}");
        }
        // Block 0 is written only after every other block of that session.
        for w in rig.sim.programmed.windows(2) {
            if w[0].addr == BASE && size > 256 {
                assert_ne!(w[1].addr, BASE, "{why}");
            }
        }
    }
    assert!(done > 0);
    assert!(failed > 0);
}

// ================================================================ board revision (W8)

#[test]
fn validate_the_board_marker_must_end_a_short_printable_run() {
    let hw = |strings: &[&[u8]]| scanned(&tagged(strings)).hw_tag;
    assert_eq!(&hw(&[b"VDM-HW:C2"])[..], b"C2");
    assert_eq!(&hw(&[b"xyVDM-HW:C1"])[..], b"C1");
    assert_eq!(&hw(&[b"VDM-HW:C12"])[..], b"C12");
    assert!(hw(&[b"VDM-HW:C"]).is_empty());
    assert!(hw(&[b"VDM-HW:C123"]).is_empty());
    assert!(hw(&[b"VDM-HW:C2x"]).is_empty());
    assert!(hw(&[b"VDM-HW:X2"]).is_empty());
    assert!(hw(&[b"DM-HW:C2"]).is_empty());
    assert!(hw(&[b"VDM-HW:Cx2"]).is_empty());
    assert!(hw(&[b"VDM-HW:"]).is_empty());
    let run = |n: usize, marker: &str| {
        let mut s = vec![b'a'; n];
        s.extend_from_slice(marker.as_bytes());
        s
    };
    assert_eq!(&hw(&[&run(22, "VDM-HW:C2")])[..], b"C2"); // 31
    assert!(hw(&[&run(23, "VDM-HW:C2")]).is_empty()); // 32
    assert_eq!(&hw(&[&run(21, "VDM-HW:C12")])[..], b"C12");
    assert!(hw(&[]).is_empty());
    // The version is still found next to the marker.
    assert_eq!(
        &scanned(&tagged(&[b"VDM-HW:C2"])).version[..],
        b"2.1.0-revamped"
    );
}

#[test]
fn validate_two_different_markers_are_a_conflict_the_same_twice_is_not() {
    let a = scanned(&tagged(&[b"VDM-HW:C1", b"VDM-HW:C2"]));
    assert_eq!(&a.hw_tag[..], b"C1");
    assert!(a.hw_conflict);
    let b2 = scanned(&tagged(&[b"VDM-HW:C2", b"VDM-HW:C2"]));
    assert_eq!(&b2.hw_tag[..], b"C2");
    assert!(!b2.hw_conflict);
    let c = scanned(&tagged(&[b"VDM-HW:C2", b"VDM-HW:C22"]));
    assert!(c.hw_conflict);
    assert!(!scanned(&tagged(&[b"VDM-HW:C2"])).hw_conflict);
}

#[test]
fn validate_the_validating_phase_finds_the_same_marker_as_validate_image() {
    let sets: [&[&[u8]]; 4] = [
        &[b"VDM-HW:C2"],
        &[b"VDM-HW:C1", b"VDM-HW:C2"],
        &[b"VDM-HW:C123"],
        &[],
    ];
    for strs in sets {
        let img = tagged_at(strs, 53760, 20000, "2.1.0-revamped");
        let reference = scanned(&img);
        let mut rig = Rig::new(img);
        rig.opt.force = true; // no board check: only compare the scan
        assert!(rig.begin());
        rig.run_hook(|r| {
            if r.f.status().phase != FlashPhase::Validating {
                r.f.abort();
            }
        });
        assert_eq!(rig.f.status().image.hw_tag, reference.hw_tag);
        assert_eq!(rig.f.status().image.hw_conflict, reference.hw_conflict);
    }
}

#[test]
fn a_c2_image_on_a_c1_board_fails_in_validating_before_any_reset() {
    let mut rig = Rig::new(tagged(&[b"VDM-HW:C2"]));
    rig.opt.board_hw = tag("C1");
    assert_eq!(rig.begin_and_run(), FlashPhase::Failed);
    assert_eq!(rig.f.status().error, FlashError::BoardMismatch);
    assert_eq!(rig.f.status().error_phase, FlashPhase::Validating);
    assert_eq!(rig.f.status().board, BoardCheck::Mismatch);
    assert_eq!(&rig.f.status().board_hw[..], b"C1");
    assert!(rig.untouched());
}

#[test]
fn force_flashes_a_mismatching_image() {
    let mut rig = Rig::new(tagged(&[b"VDM-HW:C2"]));
    rig.opt.board_hw = tag("C1");
    rig.opt.force = true;
    rig.sim.app_reply = b("gvers 2.1.0-revamped_C1 1 ");
    assert_eq!(rig.begin_and_run(), FlashPhase::Done);
    assert_eq!(rig.f.status().board, BoardCheck::Mismatch);
}

#[test]
fn a_conflicting_image_fails_like_a_mismatch() {
    let mut rig = Rig::new(tagged(&[b"VDM-HW:C1", b"VDM-HW:C2"]));
    rig.opt.board_hw = tag("C1");
    assert_eq!(rig.begin_and_run(), FlashPhase::Failed);
    assert_eq!(rig.f.status().error, FlashError::BoardMismatch);
    assert_eq!(rig.f.status().board, BoardCheck::Ok);
    assert!(rig.untouched());
}

#[test]
fn a_tagged_image_needs_a_known_board() {
    let mut rig = Rig::new(tagged(&[b"VDM-HW:C2"]));
    assert_eq!(rig.begin_and_run(), FlashPhase::Failed);
    assert_eq!(rig.f.status().error, FlashError::BoardRequired);
    assert_eq!(rig.f.status().board, BoardCheck::BoardRequired);
    assert!(rig.untouched());
    let mut forced = Rig::new(tagged(&[b"VDM-HW:C2"]));
    forced.opt.force = true;
    forced.sim.app_reply = b("gvers 2.1.0-revamped_C2 1 ");
    assert_eq!(forced.begin_and_run(), FlashPhase::Done);
}

#[test]
fn an_untagged_image_flashes_with_the_untagged_warning() {
    let mut rig = Rig::new(tagged_at(&[], 4096, 1024, "1.4.9_Dev"));
    rig.opt.board_hw = tag("C2");
    assert_eq!(rig.begin_and_run(), FlashPhase::Done);
    assert_eq!(rig.f.status().board, BoardCheck::Untagged);
}

#[test]
fn a_matching_board_flashes_the_new_application_must_report_the_same_tag() {
    let mut ok = Rig::new(tagged(&[b"VDM-HW:C2"]));
    ok.opt.board_hw = tag("C2");
    ok.sim.app_reply = b("gvers 2.1.0-revamped_C2 1 ");
    assert_eq!(ok.begin_and_run(), FlashPhase::Done);
    assert_eq!(ok.f.status().board, BoardCheck::Ok);
    let mut other = Rig::new(tagged(&[b"VDM-HW:C2"]));
    other.opt.board_hw = tag("C2");
    other.sim.app_reply = b("gvers 2.1.0-revamped_C1 1 ");
    assert_eq!(other.begin_and_run(), FlashPhase::Failed);
    assert_eq!(other.f.status().error, FlashError::AppVersionMismatch);
    let mut untagged_app = Rig::new(tagged(&[b"VDM-HW:C2"]));
    untagged_app.opt.board_hw = tag("C2");
    untagged_app.sim.app_reply = b("gvers 2.1.0-revamped 1 ");
    assert_eq!(untagged_app.begin_and_run(), FlashPhase::Done);
    let mut forced = Rig::new(tagged(&[b"VDM-HW:C2"]));
    forced.opt.board_hw = tag("C2");
    forced.opt.force = true;
    forced.sim.app_reply = b("gvers 2.1.0-revamped_C1 1 ");
    assert_eq!(forced.begin_and_run(), FlashPhase::Done);
}

// ================================================================ blank mode and baud (E6)

#[test]
fn blank_mode_ends_after_the_verify_without_a_reset_or_gvers() {
    let mut rig = Rig::new(make_image(4096));
    rig.opt.blank = true;
    rig.sim.has_boot_loop = false;
    rig.sim.boot_pin_resets = 100; // BOOT0 held: every release boots the ROM bootloader
    assert_eq!(rig.begin_and_run(), FlashPhase::Done);
    assert!(rig.f.status().manual_reset);
    assert_eq!(rig.f.status().percent, 100);
    assert_eq!(rig.sim.resets.len(), 2); // the one pulse of Resetting
    assert!(rig.sim.gvers_times.is_empty());
    assert_eq!(rig.sim.writes_equal(b"gvers \r\n"), 0);
    assert!(rig.flash_matches_image());
    assert_eq!(rig.sim.configs.last(), Some(&(115_200, false)));
    assert_eq!(rig.f.status().baud, 115_200);
}

#[test]
fn normal_mode_is_not_a_manual_reset() {
    let mut rig = Rig::new(make_image(1024));
    assert_eq!(rig.begin_and_run(), FlashPhase::Done);
    assert!(!rig.f.status().manual_reset);
    assert_eq!(rig.f.status().baud, 115_200);
}

#[test]
fn a_bootloader_that_only_syncs_at_57600_gets_a_second_session() {
    let mut rig = Rig::new(make_image(4096));
    rig.opt.blank = true;
    rig.sim.has_boot_loop = false;
    rig.sim.boot_pin_resets = 100;
    rig.sim.boot_max_baud = 57600;
    assert_eq!(rig.begin_and_run(), FlashPhase::Done);
    assert_eq!(rig.f.status().baud, 57600);
    assert_eq!(rig.sim.resets.len(), 4); // a new pulse for the second session
    assert_eq!(rig.sim.configs[0], (115_200, true));
    assert_eq!(
        rig.sim
            .configs
            .iter()
            .filter(|&&c| c == (57600, true))
            .count(),
        1
    );
    assert!(rig.flash_matches_image());
    assert!(rig.percent_monotonic());
}

#[test]
fn the_fallback_in_normal_mode_handshakes_at_115200_again() {
    let mut rig = Rig::new(make_image(4096));
    rig.sim.boot_max_baud = 57600;
    assert_eq!(rig.begin_and_run(), FlashPhase::Done);
    assert_eq!(rig.f.status().baud, 57600);
    assert_eq!(rig.sim.sync_times.len(), 1); // the 115200 ones never reach the bootloader
    assert_eq!(rig.sim.resets.len(), 6); // Resetting twice, Starting
    assert!(rig.flash_matches_image());
    assert!(rig.percent_monotonic());
}

#[test]
fn a_bootloader_that_never_answers_fails_after_the_second_session() {
    let mut rig = Rig::new(make_image(1024));
    rig.opt.blank = true;
    rig.sim.has_boot_loop = false;
    rig.sim.boot_pin_resets = 100;
    rig.sim.sync_silent = 1000;
    assert_eq!(rig.begin_and_run(), FlashPhase::Failed);
    assert_eq!(rig.f.status().error, FlashError::SyncFailed);
    assert_eq!(rig.sim.sync_times.len(), 6);
    assert_eq!(rig.f.status().baud, 57600);
    assert!(rig.left_clean());
    assert!(rig.percent_monotonic());
}

#[test]
fn no_fallback_without_a_fallback_baud_or_when_already_at_it() {
    let mut off = Rig::new(make_image(1024));
    off.opt.fallback_baud = 0;
    off.sim.sync_silent = 1000;
    assert_eq!(off.begin_and_run(), FlashPhase::Failed);
    assert_eq!(off.sim.sync_times.len(), 3);
    let mut at = Rig::new(make_image(1024));
    at.opt.baud = 57600;
    at.sim.sync_silent = 1000;
    assert_eq!(at.begin_and_run(), FlashPhase::Failed);
    assert_eq!(at.sim.sync_times.len(), 3);
    assert_eq!(at.f.status().baud, 57600);
}

#[test]
fn a_silent_get_id_falls_back_a_nack_does_not() {
    let mut silent = Rig::new(make_image(1024));
    silent.sim.get_id_silent = 1000;
    assert_eq!(silent.begin_and_run(), FlashPhase::Failed);
    assert_eq!(silent.f.status().error, FlashError::Timeout);
    assert_eq!(silent.f.status().baud, 57600);
    assert_eq!(silent.sim.sync_times.len(), 2);
    let mut nack = Rig::new(make_image(1024));
    nack.sim.get_id_bad_end = 1000;
    assert_eq!(nack.begin_and_run(), FlashPhase::Failed);
    assert_eq!(nack.f.status().error, FlashError::Nack);
    assert_eq!(nack.f.status().baud, 115_200);
    assert_eq!(nack.sim.sync_times.len(), 1);
}

// ================================================================ application wait (W13)

#[test]
fn the_new_application_may_take_up_to_60_s() {
    assert_eq!(FlashOptions::default().app_timeout_ms, 60000);
    let mut late = Rig::new(make_image(1024));
    late.sim.app_answers = false;
    assert!(late.begin());
    late.run_hook(|r| {
        if r.f.status().phase == FlashPhase::WaitingApp
            && r.now - r.sim.resets.last().unwrap().0 >= 58990
        {
            r.sim.app_answers = true;
        }
    });
    assert_eq!(late.f.status().phase, FlashPhase::Done);
    let mut none = Rig::new(make_image(1024));
    none.sim.app_answers = false;
    assert!(none.begin());
    assert_eq!(none.run_with(|_| {}, 120_000, 1), FlashPhase::Failed);
    assert_eq!(none.f.status().error, FlashError::AppNotResponding);
    assert_eq!(
        none.f.status().finished_ms - none.sim.resets.last().unwrap().0,
        60000
    );
}

#[test]
fn validate_board_markers_ending_in_0_and_9() {
    let c10 = scanned(&tagged(&[b"VDM-HW:C10"]));
    assert_eq!(&c10.hw_tag[..], b"C10");
    assert!(!c10.hw_conflict);
    assert_eq!(&scanned(&tagged(&[b"VDM-HW:C9"])).hw_tag[..], b"C9");
}

#[test]
fn a_board_tag_of_three_chars_is_kept_whole() {
    let mut rig = Rig::new(tagged(&[b"VDM-HW:C12"]));
    rig.opt.board_hw = tag("C12");
    assert!(rig.begin());
    assert_eq!(&rig.f.status().board_hw[..], b"C12");
}

#[test]
fn board_and_application_failures_report_address_0() {
    let mut mismatch = Rig::new(tagged(&[b"VDM-HW:C2"]));
    mismatch.opt.board_hw = tag("C1");
    assert_eq!(mismatch.begin_and_run(), FlashPhase::Failed);
    assert_eq!(mismatch.f.status().error, FlashError::BoardMismatch);
    assert_eq!(mismatch.f.status().error_address, 0);
    let mut required = Rig::new(tagged(&[b"VDM-HW:C2"]));
    assert_eq!(required.begin_and_run(), FlashPhase::Failed);
    assert_eq!(required.f.status().error, FlashError::BoardRequired);
    assert_eq!(required.f.status().error_address, 0);
    let mut app = Rig::new(tagged(&[b"VDM-HW:C2"]));
    app.opt.board_hw = tag("C2");
    app.sim.app_reply = b("gvers 2.1.0-revamped_C1 1 ");
    assert_eq!(app.begin_and_run(), FlashPhase::Failed);
    assert_eq!(app.f.status().error, FlashError::AppVersionMismatch);
    assert_eq!(app.f.status().error_address, 0);
}

#[test]
fn two_sync_attempts_send_exactly_two_0x7f() {
    let mut rig = Rig::new(make_image(1024));
    rig.opt.fallback_baud = 0;
    rig.opt.sync_attempts = 2;
    rig.sim.sync_silent = 100;
    assert_eq!(rig.begin_and_run(), FlashPhase::Failed);
    assert_eq!(rig.f.status().error, FlashError::SyncFailed);
    assert_eq!(rig.sim.sync_times.len(), 2);
}

#[test]
fn an_image_of_whole_blocks_is_written_in_exactly_its_blocks() {
    let mut rig = Rig::new(make_image(1024));
    assert_eq!(rig.begin_and_run(), FlashPhase::Done);
    assert_eq!(rig.sim.commands.iter().filter(|&&c| c == 0x31).count(), 4);
    assert!(rig.flash_matches_image());
}

// ---------------------------------------------------------------- Rust additions

#[test]
fn address_frame_checksum_is_the_xor_of_the_address_bytes() {
    // block addresses end in 0x00; the AN3155 rule holds for any address
    assert_eq!(address_frame(0x0800_020A), [0x08, 0x00, 0x02, 0x0A, 0x00]);
    assert_eq!(address_frame(0x1234_5678), [0x12, 0x34, 0x56, 0x78, 0x08]);
    assert_eq!(address_frame(BASE + 0x300), [0x08, 0x00, 0x03, 0x00, 0x0B]);
}

#[test]
fn app_major_or_minor_mismatch() {
    // the C++ cases differ in the patch or the suffix only
    for reply in ["gvers 2.4.9_Dev_C1 1 ", "gvers 1.5.9_Dev_C1 1 "] {
        let mut rig = Rig::new(make_image(1024));
        rig.sim.app_reply = b(reply);
        assert_eq!(rig.begin_and_run(), FlashPhase::Failed, "{reply}");
        assert_eq!(rig.f.status().error, FlashError::AppVersionMismatch);
    }
}

#[test]
fn app_reads_at_most_256_bytes_per_step() {
    // 702 noise bytes and the reply arrive 3 ms after gvers: 256 + 256 + 213 bytes in three
    // steps of 1 ms
    let mut rig = Rig::new(make_image(1024));
    let mut noise = vec![b'z'; 700];
    noise.extend_from_slice(b"\r\n");
    rig.sim.app_noise = noise;
    assert!(rig.begin());
    assert_eq!(rig.run_with(|_| {}, 60000, 1), FlashPhase::Done);
    assert_eq!(rig.sim.gvers_times.len(), 1);
    assert_eq!(rig.f.status().finished_ms - rig.sim.gvers_times[0], 5);
}

#[test]
fn sync_noise_before_every_ack_of_byte_by_byte_replies() {
    // the noise byte is dropped once; the rest of a reply arrives in later reads
    let mut rig = Rig::new(make_image(1024));
    rig.sim.noise_replies = 1000;
    rig.sim.reply_spacing_ms = 1;
    assert_eq!(rig.begin_and_run(), FlashPhase::Done);
    let st = rig.f.status();
    assert_eq!(st.bootloader_version, 0x31);
    assert_eq!(st.chip_pid, 0x431);
    assert_eq!(st.attempt, 0);
    assert_eq!(rig.sim.writes_equal(&[0x02, 0xFD]), 1);
    assert_eq!(rig.sim.read_addrs.len(), 4);
    assert!(rig.flash_matches_image());
}
