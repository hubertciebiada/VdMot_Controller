//! The Arduino `Update` contract (new: the C++ tests saw a fake of the library): codes, texts,
//! the sector buffer, the magic byte, MD5, `end(true)` of an image of unknown size.

use super::*;
use crate::testkit::board::{APP_A, APP_B};
use crate::testkit::{Device, FakeBoard, FakeHeap, FakeMd5, FakeOta, SlotImage};

type Up<'a> = Update<&'a FakeOta, FakeMd5, &'a FakeHeap>;

fn device() -> (FakeBoard, Device) {
    let board = FakeBoard::new();
    let dev = board.boot();
    (board, dev)
}

fn update(dev: &Device) -> Up<'_> {
    Update::new(&dev.ota, FakeMd5::default(), &dev.heap)
}

fn image(n: usize) -> Vec<u8> {
    let mut v: Vec<u8> = (0..n).map(|i| (i * 7 % 251) as u8).collect();
    if let Some(b) = v.first_mut() {
        *b = 0xE9;
    }
    v
}

/// Writes `data` in pieces of `piece` bytes; the bytes each write took.
fn write_in(u: &mut Up<'_>, data: &[u8], piece: usize) -> Vec<usize> {
    data.chunks(piece).map(|c| u.write(c)).collect()
}

#[test]
fn error_texts_are_the_ones_of_updater_cpp() {
    let texts: Vec<&str> = (0..=13).map(error_text).collect();
    assert_eq!(
        texts,
        vec![
            "No Error",
            "Flash Write Failed",
            "Flash Erase Failed",
            "Flash Read Failed",
            "Not Enough Space",
            "Bad Size Given",
            "Stream Read Timeout",
            "MD5 Check Failed",
            "Wrong Magic Byte",
            "Could Not Activate The Firmware",
            "Partition Could Not be Found",
            "Bad Argument",
            "Aborted",
            "UNKNOWN"
        ]
    );
    assert_eq!(UPDATE_SIZE_UNKNOWN, u32::MAX);
}

#[test]
fn begin_refusals_and_their_codes() {
    let (_b, dev) = device();
    let mut u = update(&dev);
    assert!(!u.is_running());
    assert!(!u.begin(0));
    assert_eq!((u.error(), u.error_string()), (5, "Bad Size Given"));
    assert!(!u.begin(0x14_0001));
    assert_eq!(u.error(), UPDATE_ERROR_SIZE);
    dev.ota.knobs().no_other = true;
    assert!(!u.begin(UPDATE_SIZE_UNKNOWN));
    assert_eq!(u.error_string(), "Partition Could Not be Found");
    dev.ota.knobs().no_other = false;
    // no memory for the sector buffer: false without an error code (Arduino's malloc failure)
    dev.heap.state().next.push_back(false);
    assert!(!u.begin(UPDATE_SIZE_UNKNOWN));
    assert_eq!((u.error(), u.error_string()), (0, "No Error"));
    assert_eq!(dev.heap.state().refused, vec![4096]);
    // the port refuses the update
    dev.ota.knobs().begin_err = Some(EspErr::NO_MEM);
    assert!(!u.begin(UPDATE_SIZE_UNKNOWN));
    assert_eq!(u.error(), UPDATE_ERROR_OK);
    dev.ota.knobs().begin_err = Some(EspErr::OTA_PARTITION_CONFLICT);
    assert!(!u.begin(UPDATE_SIZE_UNKNOWN));
    assert_eq!(u.error(), UPDATE_ERROR_NO_PARTITION);
    dev.ota.knobs().begin_err = None;
    assert!(u.begin(0x14_0000));
    assert_eq!(
        (u.error(), u.size(), u.remaining()),
        (0, 0x14_0000, 0x14_0000)
    );
    assert!(u.is_running() && !u.is_finished() && !u.has_error());
    // already running: refused, the error stays
    assert!(!u.begin(10));
    assert_eq!((u.error(), u.size()), (0, 0x14_0000));
    // the buffer comes first: the two refused port begins had it, and gave it back
    assert_eq!(dev.heap.state().granted, vec![4096, 4096, 4096]);
}

#[test]
fn an_image_of_unknown_size_goes_out_in_sectors_and_ends_with_end_true() {
    let (_b, dev) = device();
    let mut u = update(&dev);
    assert!(u.begin(UPDATE_SIZE_UNKNOWN));
    assert_eq!(u.size(), 0x14_0000); // the partition
    let img = image(10_000);
    assert_eq!(
        write_in(&mut u, &img, 1460),
        vec![1460; 6].into_iter().chain([1240]).collect::<Vec<_>>()
    );
    assert_eq!(u.progress(), 8192); // two full sectors; 1808 bytes wait in the buffer
    assert_eq!(dev.ota.knobs().writes, 2);
    assert_eq!(u.write(&[]), 0);
    dev.ota.knobs().next_app = Some(APP_B);
    assert!(u.end(true));
    assert_eq!(dev.ota.knobs().written, img);
    assert_eq!(dev.ota.knobs().writes, 3);
    assert_eq!(dev.ota.knobs().finishes, 1);
    assert_eq!(dev.ota.store().slots[1], SlotImage::glue(APP_B));
    assert_eq!(dev.ota.store().otadata, 1);
    assert!(!u.is_running());
    assert_eq!((u.size(), u.progress(), u.error()), (0, 0, 0));
    assert!(!u.end(true)); // nothing running
}

#[test]
fn a_known_size_flushes_its_last_sector_when_it_is_complete() {
    let (_b, dev) = device();
    let mut u = update(&dev);
    let img = image(5000);
    assert!(u.begin(5000));
    assert_eq!(u.write(&img[..4096]), 4096);
    assert_eq!(dev.ota.knobs().writes, 0); // a full buffer waits for the next byte
    assert_eq!(u.write(&img[4096..]), 904);
    assert_eq!(dev.ota.knobs().writes, 2);
    assert!(u.is_finished());
    assert_eq!(u.remaining(), 0);
    assert!(u.end(false));
}

#[test]
fn more_than_the_size_aborts_with_not_enough_space() {
    let (_b, dev) = device();
    let mut u = update(&dev);
    assert!(u.begin(100));
    assert_eq!(u.write(&image(101)), 0);
    assert_eq!((u.error(), u.error_string()), (4, "Not Enough Space"));
    assert!(!u.is_running());
    assert_eq!(dev.ota.knobs().aborts, 1);
    assert_eq!(u.write(&image(10)), 0); // after an error
    assert!(!u.end(true));
}

#[test]
fn the_space_check_ignores_the_buffered_bytes_like_arduino() {
    // remaining() counts the buffered bytes as not written: an update of known size takes more
    let (_b, dev) = device();
    let mut u = update(&dev);
    let img = image(6100);
    assert!(u.begin(5000));
    assert_eq!(u.write(&img[..4000]), 4000);
    assert_eq!(u.write(&img[4000..5500]), 1500);
    assert_eq!(u.remaining(), 904);
    assert_eq!(u.write(&img[5500..]), 600);
    assert!(!u.is_finished());
    assert!(u.end(true));
    assert_eq!(dev.ota.knobs().written, img);
}

#[test]
fn a_wrong_magic_byte_fails_at_the_first_sector() {
    let (_b, dev) = device();
    let mut u = update(&dev);
    assert!(u.begin(UPDATE_SIZE_UNKNOWN));
    let mut img = image(5000);
    img[0] = 0x00;
    // the first two pieces only fill the buffer; the third completes the sector
    assert_eq!(write_in(&mut u, &img, 1460), vec![1460, 1460, 0, 0]);
    assert_eq!((u.error(), u.error_string()), (8, "Wrong Magic Byte"));
    assert!(!u.is_running());
    assert_eq!(dev.ota.knobs().writes, 0);
    assert_eq!(dev.ota.knobs().aborts, 1);
    // one write over the sector boundary takes nothing
    assert!(u.begin(UPDATE_SIZE_UNKNOWN));
    assert_eq!(u.write(&img), 0);
    assert_eq!(u.error(), UPDATE_ERROR_MAGIC_BYTE);
}

#[test]
fn a_failed_flash_write_returns_the_bytes_before_its_sector() {
    let (_b, dev) = device();
    let mut u = update(&dev);
    dev.ota.knobs().fail_write_at = Some(4096);
    assert!(u.begin(UPDATE_SIZE_UNKNOWN));
    let img = image(9000);
    assert_eq!(u.write(&img[..4000]), 4000);
    assert_eq!(u.write(&img[4000..]), 96); // the first sector went out, the second failed
    assert_eq!((u.error(), u.error_string()), (1, "Flash Write Failed"));
    assert!(!u.is_running());
    assert_eq!(dev.ota.knobs().aborts, 1);
}

#[test]
fn end_false_before_the_end_aborts() {
    let (_b, dev) = device();
    let mut u = update(&dev);
    assert!(u.begin(UPDATE_SIZE_UNKNOWN));
    assert_eq!(u.write(&image(100)), 100);
    assert!(!u.end(false));
    assert_eq!((u.error(), u.error_string()), (12, "Aborted"));
    assert!(u.has_error());
    assert_eq!(dev.ota.knobs().aborts, 1);
    assert!(!u.end(true)); // an error is set
    u.clear_error();
    assert!(!u.has_error());
    assert!(!u.end(true)); // not running
}

#[test]
fn a_write_without_an_update_takes_nothing_and_sets_no_error() {
    let (_b, dev) = device();
    let mut u = update(&dev);
    assert_eq!(u.write(b"x"), 0);
    assert_eq!(u.error(), UPDATE_ERROR_OK);
    assert_eq!(dev.ota.knobs().writes, 0);
}

#[test]
fn a_write_of_exactly_the_remaining_bytes_fits() {
    let (_b, dev) = device();
    let mut u = update(&dev);
    assert!(u.begin(100));
    assert_eq!(u.write(&image(100)), 100);
    assert!(u.is_finished());
    assert!(u.end(false));
}

#[test]
fn end_true_with_an_empty_buffer_writes_nothing_more() {
    let (_b, dev) = device();
    let mut u = update(&dev);
    let img = image(5000);
    assert!(u.begin(5000));
    assert_eq!(u.write(&img), 5000);
    assert_eq!(dev.ota.knobs().writes, 2);
    dev.ota.knobs().fail_write_at = Some(0); // a further write would fail
    assert!(u.end(true));
    assert_eq!(dev.ota.knobs().writes, 2);
    assert_eq!(u.error(), UPDATE_ERROR_OK);
}

#[test]
fn abort_ends_the_update() {
    let (_b, dev) = device();
    let mut u = update(&dev);
    assert!(u.begin(UPDATE_SIZE_UNKNOWN));
    u.abort();
    assert_eq!(u.error(), UPDATE_ERROR_ABORT);
    assert!(!u.is_running());
    assert_eq!(dev.ota.knobs().aborts, 1);
    u.abort(); // nothing to abandon any more
    assert_eq!(dev.ota.knobs().aborts, 1);
    assert!(u.begin(UPDATE_SIZE_UNKNOWN)); // begin clears the error
    assert_eq!(u.error(), 0);
}

#[test]
fn md5_over_the_written_bytes_is_compared_as_given() {
    let (_b, dev) = device();
    let mut u = update(&dev);
    let img = image(10_000);
    let md5 = FakeMd5::hex(&img);
    assert!(!u.set_md5(b"0123"));
    assert!(!u.set_md5(&[b'a'; 33]));
    let mut with_nul = md5.as_bytes().to_vec();
    with_nul[31] = 0;
    assert!(!u.set_md5(&with_nul)); // strlen stops at the NUL
    with_nul.extend_from_slice(b"\0junk");
    with_nul[31] = md5.as_bytes()[31];
    assert!(u.begin(UPDATE_SIZE_UNKNOWN));
    assert!(u.set_md5(&with_nul[..32 + 1])); // a NUL right after 32 characters
    assert_eq!(u.write(&img), 10_000);
    assert!(u.end(true));
    // uppercase never matches the lowercase digest
    assert!(u.begin(UPDATE_SIZE_UNKNOWN));
    assert!(u.set_md5(md5.to_uppercase().as_bytes()));
    assert_eq!(u.write(&img), 10_000);
    assert!(!u.end(true));
    assert_eq!((u.error(), u.error_string()), (7, "MD5 Check Failed"));
    assert_eq!(dev.ota.knobs().aborts, 1);
    assert_eq!(dev.ota.knobs().finishes, 1);
    // begin forgets the expected MD5
    assert!(u.begin(UPDATE_SIZE_UNKNOWN));
    assert_eq!(u.write(&image(10)), 10);
    assert!(u.end(true));
}

#[test]
fn an_image_that_does_not_verify_cannot_be_activated() {
    let (_b, dev) = device();
    let mut u = update(&dev);
    dev.ota.knobs().finish_err = Some(EspErr::OTA_VALIDATE_FAILED);
    assert!(u.begin(UPDATE_SIZE_UNKNOWN));
    assert_eq!(u.write(&image(5000)), 5000);
    assert!(!u.end(true));
    assert_eq!(
        (u.error(), u.error_string()),
        (9, "Could Not Activate The Firmware")
    );
    assert!(!u.is_running());
    assert_eq!(dev.ota.store().otadata, 0);
}

#[test]
fn a_small_image_with_a_wrong_magic_byte_fails_at_end_true() {
    // the only sector goes out at end(true): no image passed the magic check since boot
    let (_b, dev) = device();
    let mut u = update(&dev);
    assert!(u.begin(UPDATE_SIZE_UNKNOWN));
    assert_eq!(u.write(b"<html>not firmware</html>"), 25);
    assert!(!u.end(true));
    assert_eq!((u.error(), u.error_string()), (3, "Flash Read Failed"));
    assert_eq!(dev.ota.knobs().set_boots, Vec::<u32>::new());
    // with an MD5 the comparison fails first
    let mut u = update(&dev);
    assert!(u.begin(UPDATE_SIZE_UNKNOWN));
    assert!(u.set_md5(FakeMd5::hex(b"x").as_bytes()));
    assert_eq!(u.write(b"x"), 1);
    assert!(!u.end(true));
    assert_eq!(u.error(), UPDATE_ERROR_MD5);
}

#[test]
fn a_failed_last_sector_boots_what_an_earlier_update_left_in_the_slot() {
    // Arduino wrote the stashed first bytes of the last image back and selected the slot: an
    // image left complete by an update that failed its MD5 becomes the boot image (C++ bug,
    // kept; docs/rust/PORT-NOTES.md)
    let (_b, dev) = device();
    let mut u = update(&dev);
    let img = image(10_000);
    assert!(u.begin(UPDATE_SIZE_UNKNOWN));
    assert!(u.set_md5(FakeMd5::hex(b"other").as_bytes()));
    assert_eq!(u.write(&img), 10_000);
    assert!(!u.end(true));
    assert_eq!(u.error(), UPDATE_ERROR_MD5);
    // the slot holds that complete image (the fake's abort leaves it broken: set it)
    dev.ota.store().slots[1] = SlotImage::glue(APP_B);
    assert!(u.begin(UPDATE_SIZE_UNKNOWN));
    assert_eq!(u.write(b"tiny"), 4);
    assert!(u.end(true));
    assert_eq!(u.error(), UPDATE_ERROR_MAGIC_BYTE); // the error of the failed sector stays
    assert_eq!(dev.ota.knobs().set_boots, vec![0x15_0000]);
    assert_eq!(dev.ota.store().otadata, 1);
    // a slot that does not verify: the firmware cannot be activated
    dev.ota.store().slots[1] = SlotImage::broken(Some(APP_B));
    dev.ota.store().otadata = 0;
    assert!(u.begin(UPDATE_SIZE_UNKNOWN));
    assert_eq!(u.write(b"tiny"), 4);
    assert!(!u.end(true));
    assert_eq!(u.error(), UPDATE_ERROR_ACTIVATE);
    assert_eq!(dev.ota.store().otadata, 0);
    assert_eq!(dev.ota.store().slots[0], SlotImage::glue(APP_A));
}

#[test]
fn a_failed_first_flash_write_still_stashes_the_image_start() {
    let (_b, dev) = device();
    let mut u = update(&dev);
    dev.ota.knobs().fail_write_at = Some(0);
    assert!(u.begin(UPDATE_SIZE_UNKNOWN));
    assert_eq!(u.write(&image(5000)), 0);
    assert_eq!(u.error(), UPDATE_ERROR_WRITE);
    dev.ota.knobs().fail_write_at = None;
    dev.ota.store().slots[1] = SlotImage::glue(APP_B);
    assert!(u.begin(UPDATE_SIZE_UNKNOWN));
    assert_eq!(u.write(b"x"), 1);
    assert!(u.end(true)); // the stash of the failed update selects the slot
    assert_eq!(dev.ota.knobs().set_boots, vec![0x15_0000]);
}

#[test]
fn an_update_that_wrote_nothing_ends_like_arduino() {
    let (_b, dev) = device();
    let mut u = update(&dev);
    assert!(u.begin(UPDATE_SIZE_UNKNOWN));
    assert!(!u.end(true));
    assert_eq!(u.error(), UPDATE_ERROR_READ);
    assert_eq!(dev.ota.knobs().aborts, 1);
    assert_eq!(dev.ota.knobs().finishes, 0);
}
