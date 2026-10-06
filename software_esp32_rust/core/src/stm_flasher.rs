//! STM32 flashing over the ST ROM bootloader (AN3155, USART, 8E1) as a non-blocking state
//! machine over an abstract byte transport, plus image validation (port of
//! `vdm/stm_flasher.h`). Hardware-free; the stm_link glue implements the transport on
//! Serial2/NRST and calls [`StmFlasher::step`] every 1-2 ms from the STM task while the
//! LinkPolicy is suspended.
//!
//! The C++ `StmFlasher` keeps references to its transport and its image; the Rust flasher keeps
//! neither: [`StmFlasher::step`] takes both, so the session that owns the transport keeps using
//! it between runs. The C++ `begin` only stores the image, which is read from the first step on;
//! every step of a run gets the same image.
//!
//! One intended deviation from the C++ flasher (decision D9, docs/rust/GLUE-DESIGN-STM.md
//! section 8): an image larger than sector 0 (16 KiB) is flashed in two passes of Erasing,
//! Writing and Verifying, first sectors 1..n (blocks from 0x08004000 upwards), then sector 0
//! (blocks 1..63, block 0 last). Sector 0 keeps the old vector table and boot stage until the
//! rest of the new image is written and verified, so an interrupted flash leaves the STM without
//! a bootable vector table for the sector-0 pass only (about 2 s) instead of the whole run. Each
//! pass compares the CRC32 of the image bytes it verified with the one the Validating phase
//! found for them; a session retry repeats the current pass, and the retries of both passes
//! count against `session_retries`. An erase failure reports the first address of the erased
//! range. An image of at most 16 KiB is flashed exactly as in C++. Percent and the byte counters
//! run over both passes: `bytes_done` counts the bytes written (Writing) or verified (Verifying)
//! of the whole image, and the percent stays where the first verify left it until the second
//! verify passes it.

use crate::common::{bounded_length, c_str, copy_string, elapsed_ms, Text};
use crate::stm_codec::build_get_version;
use crate::version::{parse_version, Version};

// ---------------------------------------------------------------- ports

/// UART + reset line owned by the flasher while it runs.
pub trait FlashTransport {
    /// Re-open the UART: 8E1 (bootloader) or 8N1 (application), RX buffer >= 1 KiB.
    fn configure(&mut self, baud: u32, even_parity: bool);
    /// Non-blocking write; returns bytes accepted (the glue's TX buffer is large enough for one
    /// 256-byte block + framing, so a short write is an error for the flasher).
    fn write(&mut self, data: &[u8]) -> usize;
    /// Non-blocking read of up to `out.len()` bytes; returns bytes read.
    fn read(&mut self, out: &mut [u8]) -> usize;
    fn discard_input(&mut self);
    /// true = hold the STM in reset (NRST asserted; IO15 HIGH on the board).
    fn set_reset(&mut self, asserted: bool);
}

/// Random-access image (LittleFS file in glue, array in tests).
pub trait FlashImage {
    fn size(&self) -> u32;
    /// Reads `out.len()` bytes at `offset`; false on I/O error or out of range.
    fn read(&mut self, offset: u32, out: &mut [u8]) -> bool;
}

// ---------------------------------------------------------------- phases, errors

/// Phase of a flash run (the numbers are the C++ ones).
#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FlashPhase {
    #[default]
    Idle = 0,
    /// image checks before touching the STM
    Validating = 1,
    /// NRST pulse, UART 8E1
    Resetting = 2,
    /// DEADBEEF every 100 ms, wait BEEFIT (normal mode)
    Handshake = 3,
    /// 0x7F -> ACK/NACK
    Sync = 4,
    /// 0x02
    GetId = 5,
    /// 0x44 sector list
    Erasing = 6,
    /// 0x31 blocks, sector 0 last (D9), block 0 last
    Writing = 7,
    /// 0x11 read-back compare
    Verifying = 8,
    /// NRST pulse, UART 8N1
    Starting = 9,
    /// gvers until the new application answers
    WaitingApp = 10,
    Done = 11,
    Failed = 12,
}

/// Indexed by the phase number.
const PHASES: [FlashPhase; 13] = [
    FlashPhase::Idle,
    FlashPhase::Validating,
    FlashPhase::Resetting,
    FlashPhase::Handshake,
    FlashPhase::Sync,
    FlashPhase::GetId,
    FlashPhase::Erasing,
    FlashPhase::Writing,
    FlashPhase::Verifying,
    FlashPhase::Starting,
    FlashPhase::WaitingApp,
    FlashPhase::Done,
    FlashPhase::Failed,
];

impl FlashPhase {
    pub fn from_raw(v: u8) -> Option<Self> {
        PHASES.get(usize::from(v)).copied()
    }
}

/// "idle", "validating", "resetting", "handshake", "sync", "getid", "erasing", "writing",
/// "verifying", "starting", "waiting_app", "done", "failed".
pub fn flash_phase_name(p: FlashPhase) -> &'static str {
    match p {
        FlashPhase::Idle => "idle",
        FlashPhase::Validating => "validating",
        FlashPhase::Resetting => "resetting",
        FlashPhase::Handshake => "handshake",
        FlashPhase::Sync => "sync",
        FlashPhase::GetId => "getid",
        FlashPhase::Erasing => "erasing",
        FlashPhase::Writing => "writing",
        FlashPhase::Verifying => "verifying",
        FlashPhase::Starting => "starting",
        FlashPhase::WaitingApp => "waiting_app",
        FlashPhase::Done => "done",
        FlashPhase::Failed => "failed",
    }
}

/// Legacy /stmupdstatus status code 0..8 for the old UI.
pub fn legacy_flash_status(p: FlashPhase) -> u8 {
    match p {
        FlashPhase::Idle => 0,
        FlashPhase::Validating
        | FlashPhase::Resetting
        | FlashPhase::Handshake
        | FlashPhase::Sync
        | FlashPhase::GetId => 1,
        FlashPhase::Erasing => 3,
        FlashPhase::Writing => 4,
        FlashPhase::Verifying | FlashPhase::Starting | FlashPhase::WaitingApp => 5,
        FlashPhase::Done => 6,
        FlashPhase::Failed => 8,
    }
}

/// Why a run failed (the numbers are the C++ ones).
#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FlashError {
    #[default]
    None = 0,
    ImageEmpty = 1,
    /// > flash size of the detected chip (or 512 KiB before GetId)
    ImageTooLarge = 2,
    /// initial SP / reset vector outside RAM / image
    ImageBadVectors = 3,
    /// "DEADBEEF"/"BEEFIT" missing (next update would need BOOT0)
    ImageNoHandshake = 4,
    /// SP above the RAM top of the detected chip
    ImageChipMismatch = 5,
    /// [`FlashImage::read`] failed
    ImageRead = 6,
    /// no BEEFIT within `handshake_window_ms`
    HandshakeTimeout = 7,
    /// no ACK/NACK to 0x7F after retries
    SyncFailed = 8,
    /// PID not 0x423/0x431/0x433
    UnknownChip = 9,
    /// bootloader NACK (see `error_phase`/`error_address`)
    Nack = 10,
    /// no ACK in time
    Timeout = 11,
    /// read-back differs (`error_address` = first bad byte)
    VerifyMismatch = 12,
    /// short write
    TransportWrite = 13,
    /// no gvers after flashing
    AppNotResponding = 14,
    /// gvers differs from the version found in the image
    AppVersionMismatch = 15,
    Aborted = 16,
    /// image built for another board revision
    BoardMismatch = 17,
    /// tagged image, board revision unknown (blank mode)
    BoardRequired = 18,
}

/// Indexed by the error number.
const ERRORS: [FlashError; 19] = [
    FlashError::None,
    FlashError::ImageEmpty,
    FlashError::ImageTooLarge,
    FlashError::ImageBadVectors,
    FlashError::ImageNoHandshake,
    FlashError::ImageChipMismatch,
    FlashError::ImageRead,
    FlashError::HandshakeTimeout,
    FlashError::SyncFailed,
    FlashError::UnknownChip,
    FlashError::Nack,
    FlashError::Timeout,
    FlashError::VerifyMismatch,
    FlashError::TransportWrite,
    FlashError::AppNotResponding,
    FlashError::AppVersionMismatch,
    FlashError::Aborted,
    FlashError::BoardMismatch,
    FlashError::BoardRequired,
];

impl FlashError {
    pub fn from_raw(v: u8) -> Option<Self> {
        ERRORS.get(usize::from(v)).copied()
    }
}

/// "none", "image_empty", ... (the variant names in snake case).
pub fn flash_error_name(e: FlashError) -> &'static str {
    match e {
        FlashError::None => "none",
        FlashError::ImageEmpty => "image_empty",
        FlashError::ImageTooLarge => "image_too_large",
        FlashError::ImageBadVectors => "image_bad_vectors",
        FlashError::ImageNoHandshake => "image_no_handshake",
        FlashError::ImageChipMismatch => "image_chip_mismatch",
        FlashError::ImageRead => "image_read",
        FlashError::HandshakeTimeout => "handshake_timeout",
        FlashError::SyncFailed => "sync_failed",
        FlashError::UnknownChip => "unknown_chip",
        FlashError::Nack => "nack",
        FlashError::Timeout => "timeout",
        FlashError::VerifyMismatch => "verify_mismatch",
        FlashError::TransportWrite => "transport_write",
        FlashError::AppNotResponding => "app_not_responding",
        FlashError::AppVersionMismatch => "app_version_mismatch",
        FlashError::Aborted => "aborted",
        FlashError::BoardMismatch => "board_mismatch",
        FlashError::BoardRequired => "board_required",
    }
}

// ---------------------------------------------------------------- image facts

/// Image facts found by [`validate_image`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImageInfo {
    pub size: u32,
    /// size rounded up to 4 (padding 0xFF)
    pub padded_size: u32,
    pub initial_sp: u32,
    pub reset_vector: u32,
    /// contains "DEADBEEF" and "BEEFIT"
    pub has_handshake: bool,
    /// CRC32 of the unpadded image
    pub crc: u32,
    /// first NUL-terminated string that parses as a Version and contains no spaces, "" if none
    /// (C++ `char[32]`)
    pub version: Text<31>,
    /// board revision of a "VDM-HW:C<n>" marker, "" untagged (C++ `char[4]`)
    pub hw_tag: Text<3>,
    /// two different markers found
    pub hw_conflict: bool,
}

/// Board revision check before flashing (the STM images of the C1 and C2 boards are not
/// interchangeable). The numbers are the C++ ones.
#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BoardCheck {
    #[default]
    Ok = 0,
    Untagged = 1,
    Mismatch = 2,
    BoardRequired = 3,
}

impl BoardCheck {
    pub fn from_raw(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Ok),
            1 => Some(Self::Untagged),
            2 => Some(Self::Mismatch),
            3 => Some(Self::BoardRequired),
            _ => None,
        }
    }
}

/// "ok", "untagged", "mismatch", "board_required".
pub fn board_check_name(c: BoardCheck) -> &'static str {
    match c {
        BoardCheck::Ok => "ok",
        BoardCheck::Untagged => "untagged",
        BoardCheck::Mismatch => "mismatch",
        BoardCheck::BoardRequired => "board_required",
    }
}

/// "C" followed by 1 or 2 digits ("C1", "C12"); `s` is a C string (ends at its first NUL).
pub fn board_tag_valid(s: &[u8]) -> bool {
    match c_str(s) {
        [b'C', digits @ ..] => {
            (1..=2).contains(&digits.len()) && digits.iter().all(u8::is_ascii_digit)
        }
        _ => false,
    }
}

/// `image_hw` "" -> Untagged; `board_hw` "" -> BoardRequired; equal -> Ok; else Mismatch. Both
/// are C strings of which at most 4 bytes are read (the C++ `char[4]` tags may lack the NUL).
pub fn check_board(image_hw: &[u8], board_hw: &[u8]) -> BoardCheck {
    // C++ sizeof(ImageInfo::hwTag)
    const TAG_CAP: usize = 4;
    let image = image_hw
        .get(..bounded_length(image_hw, TAG_CAP))
        .unwrap_or_default();
    let board = board_hw
        .get(..bounded_length(board_hw, TAG_CAP))
        .unwrap_or_default();
    if image.is_empty() {
        BoardCheck::Untagged
    } else if board.is_empty() {
        BoardCheck::BoardRequired
    } else if image == board {
        BoardCheck::Ok
    } else {
        BoardCheck::Mismatch
    }
}

// ---------------------------------------------------------------- constants

const ACK: u8 = 0x79;
const NACK: u8 = 0x1F;
const CMD_GET: u8 = 0x00;
const CMD_GET_ID: u8 = 0x02;
const CMD_READ: u8 = 0x11;
const CMD_WRITE: u8 = 0x31;
const CMD_EXT_ERASE: u8 = 0x44;
const SYNC: u8 = 0x7F;

const FLASH_BASE: u32 = 0x0800_0000;
const RAM_BASE: u32 = 0x2000_0000;
const RAM_TOP_MAX: u32 = 0x2002_0000;
const MAX_IMAGE: u32 = 512 * 1024;
const BLOCK_SIZE: u32 = 256;
const APP_BAUD: u32 = 115_200;
/// the v1 STM boot window listens at 115200 8E1 only
const HANDSHAKE_BAUD: u32 = 115_200;
const MIN_BAUD: u32 = 1200;
const MAX_BAUD: u32 = 115_200;
/// 1 KiB of image per step()
const VALIDATE_CHUNKS_PER_STEP: u32 = 4;
const MAX_TRANSITIONS_PER_STEP: u8 = 4;
const APP_READ_PER_STEP: usize = 256;

/// 9 bytes: the period re-aligns the v1 STM's fixed 8-byte matcher.
const HANDSHAKE: &[u8; 9] = b"DEADBEEF\n";
const BEEFIT: &[u8; 6] = b"BEEFIT";
/// "DEADBEEF"
const DEAD_WORD: u64 = 0x4445_4144_4245_4546;
/// "BEEFIT"
const BEEF_WORD: u64 = 0x4245_4546_4954;
const BEEF_MASK: u64 = 0xFFFF_FFFF_FFFF;

/// F401/F411 sector sizes (sectors 0..7).
const SECTOR_SIZE: [u32; 8] = [
    16 * 1024,
    16 * 1024,
    16 * 1024,
    16 * 1024,
    64 * 1024,
    128 * 1024,
    128 * 1024,
    128 * 1024,
];
/// Sector 0, which the D9 order erases and writes last.
const SECTOR0_BYTES: u32 = 16 * 1024;
const SECTOR0_BLOCKS: u32 = SECTOR0_BYTES / BLOCK_SIZE;

const ERASE_CMD: [u8; 2] = [CMD_EXT_ERASE, !CMD_EXT_ERASE];
const WRITE_CMD: [u8; 2] = [CMD_WRITE, !CMD_WRITE];
const READ_CMD: [u8; 2] = [CMD_READ, !CMD_READ];

// ---------------------------------------------------------------- helpers

fn chip_flash_size(pid: u16) -> u32 {
    match pid {
        0x423 => 256 * 1024,
        0x431 | 0x433 => 512 * 1024,
        _ => 0,
    }
}

fn chip_ram_top(pid: u16) -> u32 {
    match pid {
        0x423 => 0x2001_0000,
        0x431 => 0x2002_0000,
        0x433 => 0x2001_8000,
        _ => 0,
    }
}

/// CRC-32 (IEEE 802.3, reflected 0xEDB88320) on the pre-inverted state.
fn crc32_update(mut crc: u32, data: &[u8]) -> u32 {
    const NIBBLE: [u32; 16] = [
        0x0000_0000,
        0x1DB7_1064,
        0x3B6E_20C8,
        0x26D9_30AC,
        0x76DC_4190,
        0x6B6B_51F4,
        0x4DB2_6158,
        0x5005_713C,
        0xEDB8_8320,
        0xF00F_9344,
        0xD6D6_A3E8,
        0xCB61_B38C,
        0x9B64_C2B0,
        0x86D3_D2D4,
        0xA00A_E278,
        0xBDBD_F21C,
    ];
    for &b in data {
        crc ^= u32::from(b);
        crc = (crc >> 4) ^ NIBBLE[(crc & 0x0F) as usize];
        crc = (crc >> 4) ^ NIBBLE[(crc & 0x0F) as usize];
    }
    crc
}

/// A run that ends with "VDM-HW:C" + 1..2 digits (contracts: STM image revision marker): the
/// tag "C<digits>".
fn hw_marker(run: &[u8]) -> Option<Text<3>> {
    const MARKER: &[u8] = b"VDM-HW:C";
    let digits = run
        .iter()
        .rev()
        .take(2)
        .take_while(|c| c.is_ascii_digit())
        .count();
    let (head, tail) = run.split_at(run.len() - digits);
    if digits == 0 || !head.ends_with(MARKER) {
        return None;
    }
    let mut tag = Text::new();
    let _ = tag.push(b'C');
    let _ = tag.extend_from_slice(tail);
    Some(tag)
}

/// Incremental state of the image scan (CRC32, handshake strings, version string), shared by
/// [`validate_image`] and the Validating phase, which scans a bounded number of bytes per step
/// (C++ `detail::ImageScan`).
#[derive(Clone, Debug)]
struct ImageScan {
    offset: u32,
    crc: u32,
    /// last 8 bytes, newest in the low byte
    window: u64,
    /// "DEADBEEF" seen
    dead: bool,
    /// "BEEFIT" seen
    beef: bool,
    version_found: bool,
    /// printable run so far (C++ `run` and `runLen` < 32)
    run: Text<31>,
    /// the run is longer than any version, 31 chars (C++ `runLen` == 32)
    too_long: bool,
}

impl Default for ImageScan {
    fn default() -> Self {
        Self {
            offset: 0,
            crc: 0xFFFF_FFFF,
            window: 0,
            dead: false,
            beef: false,
            version_found: false,
            run: Text::new(),
            too_long: false,
        }
    }
}

fn scan_bytes(sc: &mut ImageScan, data: &[u8], info: &mut ImageInfo) {
    sc.crc = crc32_update(sc.crc, data);
    for &c in data {
        sc.window = (sc.window << 8) + u64::from(c);
        if sc.window == DEAD_WORD {
            sc.dead = true;
        }
        if (sc.window & BEEF_MASK) == BEEF_WORD {
            sc.beef = true;
        }
        match c {
            0 => {
                if !sc.too_long {
                    end_run(sc, info);
                }
                sc.run.clear();
                sc.too_long = false;
            }
            0x20..=0x7E => {
                if sc.run.push(c).is_err() {
                    sc.too_long = true;
                }
            }
            _ => {
                sc.run.clear();
                sc.too_long = false;
            }
        }
    }
    sc.offset += data.len() as u32;
}

/// A NUL ends a printable run of at most 31 chars: the first one that parses as a version is
/// the image version (an empty run never does); a board marker sets the tag or the conflict.
fn end_run(sc: &mut ImageScan, info: &mut ImageInfo) {
    if !sc.version_found && parse_version(&sc.run).valid {
        info.version = sc.run.clone();
        sc.version_found = true;
    }
    if let Some(tag) = hw_marker(&sc.run) {
        if info.hw_tag.is_empty() {
            info.hw_tag = tag;
        } else if info.hw_tag != tag {
            info.hw_conflict = true;
        }
    }
}

fn finish_scan(sc: &ImageScan, info: &mut ImageInfo) {
    info.crc = !sc.crc;
    info.has_handshake = sc.dead && sc.beef;
}

/// Family-independent checks; resets `out` and fills size and vectors.
fn check_header(img: &mut dyn FlashImage, out: &mut ImageInfo) -> FlashError {
    *out = ImageInfo::default();
    let size = img.size();
    out.size = size;
    if size == 0 {
        return FlashError::ImageEmpty;
    }
    if size > MAX_IMAGE {
        return FlashError::ImageTooLarge;
    }
    out.padded_size = (size + 3) & !3;
    if size < 8 {
        return FlashError::ImageBadVectors;
    }
    let mut vec = [0u8; 8];
    if !img.read(0, &mut vec) {
        return FlashError::ImageRead;
    }
    let [s0, s1, s2, s3, p0, p1, p2, p3] = vec;
    out.initial_sp = u32::from_le_bytes([s0, s1, s2, s3]);
    out.reset_vector = u32::from_le_bytes([p0, p1, p2, p3]);
    let sp = out.initial_sp;
    let pc = out.reset_vector;
    if sp <= RAM_BASE || sp > RAM_TOP_MAX || (sp & 3) != 0 {
        return FlashError::ImageBadVectors;
    }
    if (pc & 1) == 0 || pc < FLASH_BASE || pc >= FLASH_BASE + size {
        return FlashError::ImageBadVectors;
    }
    FlashError::None
}

fn check_chip(info: &ImageInfo, pid: u16) -> FlashError {
    let flash = chip_flash_size(pid);
    if flash == 0 {
        return FlashError::UnknownChip;
    }
    if info.size > flash {
        return FlashError::ImageTooLarge;
    }
    if info.initial_sp > chip_ram_top(pid) {
        return FlashError::ImageChipMismatch;
    }
    FlashError::None
}

/// Same numbers and suffix; the image's hw tag (usually a separate string in the binary, so
/// empty) must match only when present. The board marker of the image is checked separately.
fn same_version(image: &Version, app: &Version) -> bool {
    image.major == app.major
        && image.minor == app.minor
        && image.patch == app.patch
        && image.suffix == app.suffix
        && (image.hw.is_empty() || image.hw == app.hw)
}

/// Time `bytes` take on the wire at 8E1 (11 bits per byte), rounded up. `baud` is never 0 (the
/// session baud is checked by begin() and the fallback).
fn wire_ms(bytes: usize, baud: u32) -> u32 {
    (bytes as u64 * 11 * 1000).div_ceil(u64::from(baud)) as u32
}

fn address_frame(addr: u32) -> [u8; 5] {
    let [a, b, c, d] = addr.to_be_bytes();
    [a, b, c, d, a ^ b ^ c ^ d]
}

// ---------------------------------------------------------------- image

/// Pure image checks. `chip_pid` 0 = not known yet (then only the family-independent checks
/// run: size 1..512 KiB, SP in (0x20000000, 0x20020000] and 4-aligned (an SP equal to the RAM
/// base leaves no stack), reset vector odd and inside [0x08000000, 0x08000000+size)). With a
/// PID, size <= flash size (0x423 256 KiB, 0x431 512 KiB, 0x433 512 KiB) and SP <= RAM top
/// (0x423 0x20010000, 0x431 0x20020000, 0x433 0x20018000); any other non-zero PID gives
/// UnknownChip. `require_handshake` rejects images without the DEADBEEF/BEEFIT strings. Checks
/// run cheapest first; the first failing one is returned and `out` keeps what was learnt up to
/// that point.
pub fn validate_image(
    img: &mut dyn FlashImage,
    chip_pid: u16,
    require_handshake: bool,
    out: &mut ImageInfo,
) -> FlashError {
    let e = check_header(img, out);
    if e != FlashError::None {
        return e;
    }
    if chip_pid != 0 {
        let e = check_chip(out, chip_pid);
        if e != FlashError::None {
            return e;
        }
    }
    let mut sc = ImageScan::default();
    let mut buf = [0u8; BLOCK_SIZE as usize];
    let size = out.size;
    for offset in (0..size).step_by(BLOCK_SIZE as usize) {
        let n = (size - offset).min(BLOCK_SIZE) as usize;
        let chunk = buf.get_mut(..n).unwrap_or_default();
        if !img.read(offset, chunk) {
            return FlashError::ImageRead;
        }
        scan_bytes(&mut sc, chunk, out);
    }
    finish_scan(&sc, out);
    if require_handshake && !out.has_handshake {
        return FlashError::ImageNoHandshake;
    }
    FlashError::None
}

/// Sectors of an STM32F401/F411 covering [0, size): 16,16,16,16,64,128,... KiB. Returns the
/// number of sectors (1..8), 0 when size is 0 or > 512 KiB.
pub fn sectors_for_image(size: u32) -> u8 {
    let covering = SECTOR_SIZE
        .iter()
        .scan(0, |end, &s| {
            *end += s;
            Some(*end)
        })
        .position(|end| size <= end);
    match covering {
        Some(i) if size > 0 => i as u8 + 1,
        _ => 0,
    }
}

// ---------------------------------------------------------------- options, status

/// Options of one run; the defaults are the binding timing (DESIGN.md "STM flasher").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlashOptions {
    /// STM already in the ROM bootloader (BOOT0 held): no DEADBEEF
    pub blank: bool,
    /// skip handshake-string and version checks
    pub force: bool,
    /// ROM bootloader 8E1; the v1 boot window is always 115200
    pub baud: u32,
    /// NRST asserted
    pub reset_pulse_ms: u16,
    /// after reset release
    pub handshake_first_ms: u16,
    /// DEADBEEF resend period
    pub handshake_repeat_ms: u16,
    /// give up
    pub handshake_window_ms: u16,
    /// before 0x7F
    pub after_beefit_ms: u16,
    pub sync_attempts: u8,
    /// command/address/data ACK
    pub ack_timeout_ms: u16,
    pub erase_timeout_ms: u32,
    pub block_retries: u8,
    /// whole erase+write+verify again, same ROM session (D9: of the current pass)
    pub session_retries: u8,
    /// after the final reset, before the first gvers
    pub app_boot_ms: u16,
    /// gvers period
    pub app_poll_ms: u16,
    /// total wait for gvers, from NRST release
    pub app_timeout_ms: u16,
    /// Revision of this controller: the running STM's gvers tag, else the user's choice; ""
    /// unknown (C++ `char[4]`).
    pub board_hw: Text<3>,
    /// One more session at this baud after SyncFailed or a silent GetId (0 = off).
    pub fallback_baud: u32,
}

impl Default for FlashOptions {
    fn default() -> Self {
        Self {
            blank: false,
            force: false,
            baud: 115_200,
            reset_pulse_ms: 100,
            handshake_first_ms: 20,
            handshake_repeat_ms: 100,
            handshake_window_ms: 2500,
            after_beefit_ms: 250,
            sync_attempts: 3,
            ack_timeout_ms: 1000,
            erase_timeout_ms: 60_000,
            block_retries: 3,
            session_retries: 2,
            app_boot_ms: 4000,
            app_poll_ms: 1000,
            app_timeout_ms: 60_000,
            board_hw: Text::new(),
            fallback_baud: 57_600,
        }
    }
}

/// State of the current or last run.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FlashStatus {
    pub phase: FlashPhase,
    pub error: FlashError,
    pub error_phase: FlashPhase,
    pub error_address: u32,
    /// monotonic 0..100
    pub percent: u8,
    /// written (Writing) or verified (Verifying)
    pub bytes_done: u32,
    /// padded_size
    pub bytes_total: u32,
    /// from GetId
    pub chip_pid: u16,
    /// from GET (0x00) when read
    pub bootloader_version: u8,
    /// session retry number (0 = first)
    pub attempt: u8,
    pub started_ms: u32,
    pub finished_ms: u32,
    pub image: ImageInfo,
    /// gvers after flashing
    pub app_version: Version,
    /// result of check_board(image.hw_tag, board_hw)
    pub board: BoardCheck,
    /// FlashOptions::board_hw of this run (C++ `char[4]`)
    pub board_hw: Text<3>,
    /// blank mode: flashed and verified, BOOT0 still set
    pub manual_reset: bool,
    /// baud of the current/last session
    pub baud: u32,
}

// ---------------------------------------------------------------- flasher

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Resp {
    Pending,
    Ack,
    Nack,
    Timeout,
}

/// The error of a failed command: NACK or timeout.
fn op_error(r: Resp) -> FlashError {
    if r == Resp::Nack {
        FlashError::Nack
    } else {
        FlashError::Timeout
    }
}

/// The flasher (C++ `StmFlasher`).
#[derive(Clone, Debug)]
pub struct StmFlasher {
    opt: FlashOptions,
    /// opt.baud, or opt.fallback_baud in the fallback session
    session_baud: u32,
    st: FlashStatus,
    scan: ImageScan,
    phase_start_ms: u32,
    /// NRST released (Resetting/Starting)
    release_ms: u32,
    last_send_ms: u32,
    wait_start_ms: u32,
    wait_limit_ms: u32,
    /// index in the write/verify order of the pass
    block: u32,
    verify_crc: u32,
    retries: u8,
    sync_tries: u8,
    /// sub-step within a phase
    sub: u8,
    /// STM reset/UART re-opened: failure needs a pulse
    touched: bool,
    /// failure pulse in progress
    cleanup: bool,
    /// WaitingApp: discard up to the next CR/LF
    line_drop: bool,
    abort_requested: bool,
    /// D9: the pass of sectors 1..n runs (else the one of sector 0)
    upper: bool,
    /// D9: CRC32 state of the image bytes in sector 0, from the Validating phase
    crc_low: u32,
    /// D9: CRC32 state of the image bytes above sector 0, from the Validating phase
    crc_high: u32,
    /// data frame N-1, 256 data, checksum / expected block
    tx: [u8; 260],
    /// one read-back block + framing, or one app line
    rx: [u8; 272],
    rx_len: usize,
}

impl Default for StmFlasher {
    fn default() -> Self {
        Self {
            opt: FlashOptions::default(),
            session_baud: 0,
            st: FlashStatus::default(),
            scan: ImageScan::default(),
            phase_start_ms: 0,
            release_ms: 0,
            last_send_ms: 0,
            wait_start_ms: 0,
            wait_limit_ms: 0,
            block: 0,
            verify_crc: 0,
            retries: 0,
            sync_tries: 0,
            sub: 0,
            touched: false,
            cleanup: false,
            line_drop: false,
            abort_requested: false,
            upper: false,
            crc_low: 0,
            crc_high: 0,
            tx: [0; 260],
            rx: [0; 272],
            rx_len: 0,
        }
    }
}

impl StmFlasher {
    pub fn new() -> Self {
        Self::default()
    }

    /// Starts a run. False (and nothing touched) when a run is active or `opt.baud` is outside
    /// 1200..115200 (the AN3155 USART range). Every [`step`](Self::step) of the run gets the
    /// image of the run.
    ///
    /// Wire details beyond DESIGN.md §15:
    ///  - The handshake sends "DEADBEEF\n" (9 bytes). The STM compares fixed 8-byte chunks
    ///    without resync (1.x and 2.x alike), so a stray byte at reset would misalign a pure
    ///    8-byte stream forever; the 9-byte period re-aligns within 8 sends.
    ///  - Normal mode opens the UART at 115200 8E1 for the handshake (the v1 boot window listens
    ///    only there) and switches to `opt.baud` when BEEFIT arrives; blank mode opens it at
    ///    `opt.baud` directly.
    ///  - Every ACK timeout starts when the frame is queued and includes the frame's wire time
    ///    at the session baud (11 bits per byte), so slow bauds do not time out a 258-byte data
    ///    frame that is still being sent.
    ///  - Validating ends with the board check (`check_board(image hw_tag, opt.board_hw)`): a
    ///    mismatch or two different markers fail with BoardMismatch, an unknown board with a
    ///    tagged image with BoardRequired, both unless force; an untagged image flashes
    ///    (`status().board` Untagged). Nothing is touched before.
    ///  - SyncFailed or a GetId without any answer start one more session at
    ///    `opt.fallback_baud` (new NRST pulse) unless it is 0 or the session ran at it already;
    ///    `status().baud` is the session baud.
    ///  - Blank mode ends Done after the verify with `manual_reset` (BOOT0 still set: no reset,
    ///    no gvers); the UART is back at 115200 8N1.
    ///  - After the flash a gvers board tag other than the image's marker fails with
    ///    AppVersionMismatch unless force.
    ///  - GetId first sends GET (0x00) for the bootloader version; a failed GET is not fatal
    ///    (the byte stays 0).
    ///  - The verify passes recompute the CRC32 of the image bytes they cover; a difference from
    ///    the validated one (file changed during the run) fails with ImageRead.
    ///  - Any failure after the STM was touched pulses NRST and restores 8N1 before the phase
    ///    becomes Failed (phase keeps the failing phase during that pulse). Failures while
    ///    waiting for the app need no new pulse.
    ///  - D9: an image above 16 KiB is erased, written and verified in two passes, sector 0
    ///    last (module documentation).
    pub fn begin(&mut self, opt: &FlashOptions, now_ms: u32) -> bool {
        if self.active() || !(MIN_BAUD..=MAX_BAUD).contains(&opt.baud) {
            return false;
        }
        self.opt = opt.clone();
        self.st = FlashStatus::default();
        self.st.started_ms = now_ms;
        self.session_baud = opt.baud;
        self.st.baud = opt.baud;
        copy_string(&mut self.st.board_hw, &opt.board_hw);
        self.scan = ImageScan::default();
        self.release_ms = now_ms;
        self.last_send_ms = now_ms;
        self.wait_start_ms = now_ms;
        self.wait_limit_ms = 0;
        self.block = 0;
        self.verify_crc = 0;
        self.sync_tries = 0;
        self.touched = false;
        self.cleanup = false;
        self.line_drop = false;
        self.abort_requested = false;
        self.upper = false;
        self.crc_low = 0xFFFF_FFFF;
        self.crc_high = 0xFFFF_FFFF;
        self.rx_len = 0;
        self.enter(FlashPhase::Validating, now_ms);
        true
    }

    /// Advances the state machine; never blocks longer than the time to queue one block (~300
    /// bytes) on the transport. Returns the current phase.
    pub fn step(
        &mut self,
        transport: &mut dyn FlashTransport,
        image: &mut dyn FlashImage,
        now_ms: u32,
    ) -> FlashPhase {
        if !self.active() {
            return self.st.phase;
        }
        if self.abort_requested {
            self.abort_requested = false;
            if !self.cleanup {
                self.fail(FlashError::Aborted, 0, now_ms);
            }
        }
        // Chain transitions that need no waiting (ACK already there -> next frame). Four
        // transitions contain at most one data frame, so a step never queues more than one
        // block.
        for _ in 0..MAX_TRANSITIONS_PER_STEP {
            if !self.active() {
                break;
            }
            let before = self.progress();
            self.run_once(transport, image, now_ms);
            if self.progress() == before {
                break;
            }
        }
        self.st.phase
    }

    /// Requests an abort: the STM is reset into the application (which may be gone if erase
    /// already started; then the status says Failed/Aborted).
    pub fn abort(&mut self) {
        if self.active() {
            self.abort_requested = true;
        }
    }

    /// Phase not Idle/Done/Failed.
    pub fn active(&self) -> bool {
        !matches!(
            self.st.phase,
            FlashPhase::Idle | FlashPhase::Done | FlashPhase::Failed
        )
    }

    pub fn status(&self) -> &FlashStatus {
        &self.st
    }

    /// What a transition changes: a step chains run_once() while it changes.
    fn progress(&self) -> (FlashPhase, u8, u32, bool, u8, u8, u8) {
        (
            self.st.phase,
            self.sub,
            self.block,
            self.cleanup,
            self.st.attempt,
            self.retries,
            self.sync_tries,
        )
    }

    fn run_once(&mut self, t: &mut dyn FlashTransport, img: &mut dyn FlashImage, now: u32) {
        if self.cleanup {
            self.step_pulse(t, now);
            return;
        }
        match self.st.phase {
            FlashPhase::Validating => self.step_validating(img, now),
            FlashPhase::Resetting | FlashPhase::Starting => self.step_pulse(t, now),
            FlashPhase::Handshake => self.step_handshake(t, now),
            FlashPhase::Sync => self.step_sync(t, now),
            FlashPhase::GetId => self.step_get_id(t, now),
            FlashPhase::Erasing => self.step_erasing(t, now),
            FlashPhase::Writing => self.step_writing(t, img, now),
            FlashPhase::Verifying => self.step_verifying(t, img, now),
            FlashPhase::WaitingApp => self.step_waiting_app(t, now),
            FlashPhase::Idle | FlashPhase::Done | FlashPhase::Failed => {}
        }
    }

    fn enter(&mut self, p: FlashPhase, now: u32) {
        self.st.phase = p;
        self.phase_start_ms = now;
        self.sub = 0;
        self.retries = 0;
    }

    fn finish(&mut self, p: FlashPhase, now: u32) {
        self.enter(p, now);
        self.st.finished_ms = now;
        if p == FlashPhase::Done {
            self.st.percent = 100;
        }
        self.touched = false;
        self.cleanup = false;
    }

    fn fail(&mut self, e: FlashError, address: u32, now: u32) {
        self.st.error = e;
        self.st.error_phase = self.st.phase;
        self.st.error_address = address;
        if self.touched {
            self.cleanup = true;
            self.sub = 0;
        } else {
            self.finish(FlashPhase::Failed, now);
        }
    }

    /// Write/verify failures retry the block; erase failures and exhausted block retries repeat
    /// erase+write+verify (of the current pass) in the same ROM session.
    fn op_failed(&mut self, e: FlashError, address: u32, now: u32) {
        if self.st.phase != FlashPhase::Erasing && self.retries < self.opt.block_retries {
            self.retries += 1;
            self.sub = 0;
            return;
        }
        if self.st.attempt < self.opt.session_retries {
            self.st.attempt += 1;
            self.enter(FlashPhase::Erasing, now);
            return;
        }
        self.fail(e, address, now);
    }

    /// Every caller passes at most 98 (the byte counters never pass bytes_total).
    fn set_percent(&mut self, p: u32) {
        self.st.percent = self.st.percent.max(p as u8);
    }

    fn put(&mut self, t: &mut dyn FlashTransport, data: &[u8], now: u32) -> bool {
        if t.write(data) != data.len() {
            self.fail(FlashError::TransportWrite, 0, now);
            return false;
        }
        true
    }

    fn send(&mut self, t: &mut dyn FlashTransport, data: &[u8], now: u32, timeout_ms: u32) -> bool {
        t.discard_input();
        self.rx_len = 0;
        if !self.put(t, data, now) {
            return false;
        }
        // write() only queues: the reply cannot start before the frame has left.
        self.wait_start_ms = now;
        self.wait_limit_ms = timeout_ms.wrapping_add(wire_ms(data.len(), self.session_baud));
        true
    }

    fn ack_timeout(&self) -> u32 {
        u32::from(self.opt.ack_timeout_ms)
    }

    fn pump(&mut self, t: &mut dyn FlashTransport) {
        if let Some(free) = self.rx.get_mut(self.rx_len..).filter(|f| !f.is_empty()) {
            let cap = free.len();
            let n = t.read(free);
            self.rx_len += n.min(cap);
        }
    }

    /// Waits for an ACK followed by `need - 1` more bytes (need >= 1), or a NACK. Bytes before
    /// the first ACK/NACK are line noise and dropped. Nothing is consumed; the next send()
    /// clears the buffer.
    fn wait_reply(&mut self, t: &mut dyn FlashTransport, now: u32, need: usize) -> Resp {
        self.pump(t);
        let got = self.rx.get(..self.rx_len).unwrap_or_default();
        let skip = got
            .iter()
            .position(|&b| b == ACK || b == NACK)
            .unwrap_or(got.len());
        self.rx.copy_within(skip..self.rx_len, 0);
        self.rx_len -= skip;
        match self.rx.get(..self.rx_len) {
            Some([NACK, ..]) => Resp::Nack,
            // need >= 1: the reply starts with the ACK
            Some(reply) if reply.len() >= need => Resp::Ack,
            _ if elapsed_ms(now, self.wait_start_ms) >= self.wait_limit_ms => Resp::Timeout,
            _ => Resp::Pending,
        }
    }

    fn block_count(&self) -> u32 {
        self.st.image.padded_size.div_ceil(BLOCK_SIZE)
    }

    fn block_len(&self, block: u32) -> u32 {
        (self.st.image.padded_size - block * BLOCK_SIZE).min(BLOCK_SIZE)
    }

    /// D9: the blocks [first, end) of the current pass.
    fn pass_blocks(&self) -> (u32, u32) {
        let count = self.block_count();
        if self.upper {
            (SECTOR0_BLOCKS, count)
        } else {
            (0, count.min(SECTOR0_BLOCKS))
        }
    }

    /// D9: bytes of the image flashed by the passes before the current one.
    fn pass_base(&self) -> u32 {
        if self.upper {
            0
        } else {
            self.st.image.padded_size.saturating_sub(SECTOR0_BYTES)
        }
    }

    /// tx = N-1, data (image bytes, then 0xFF padding), checksum N-1^D0^..^DN.
    fn load_block(&mut self, img: &mut dyn FlashImage, block: u32) -> bool {
        let len = self.block_len(block) as usize;
        let off = block * BLOCK_SIZE;
        // at least one image byte: blocks start below padded_size, at most 3 bytes above size
        let avail = (self.st.image.size.saturating_sub(off) as usize).min(len);
        let Some((n, rest)) = self.tx.split_first_mut() else {
            return false;
        };
        let Some(data) = rest.get_mut(..len) else {
            return false;
        };
        let (bytes, pad) = data.split_at_mut(avail);
        if !img.read(off, bytes) {
            return false;
        }
        pad.fill(0xFF);
        *n = (len - 1) as u8;
        let cs = data.iter().fold(*n, |cs, &b| cs ^ b);
        if let Some(slot) = rest.get_mut(len) {
            *slot = cs;
        }
        true
    }

    fn step_validating(&mut self, img: &mut dyn FlashImage, now: u32) {
        if self.sub == 0 {
            let e = check_header(img, &mut self.st.image);
            if e != FlashError::None {
                self.fail(e, 0, now);
                return;
            }
            self.st.bytes_total = self.st.image.padded_size;
            self.sub = 1;
            return;
        }
        let size = self.st.image.size;
        for _ in 0..VALIDATE_CHUNKS_PER_STEP {
            let start = self.scan.offset;
            let n = size.saturating_sub(start).min(BLOCK_SIZE) as usize;
            if n == 0 {
                break;
            }
            if !img.read(start, self.tx.get_mut(..n).unwrap_or_default()) {
                self.fail(FlashError::ImageRead, start, now);
                return;
            }
            let chunk = self.tx.get(..n).unwrap_or_default();
            scan_bytes(&mut self.scan, chunk, &mut self.st.image);
            // D9: the CRC32 of each pass, for its verify (chunks never straddle 16 KiB)
            if start < SECTOR0_BYTES {
                self.crc_low = self.scan.crc;
            } else {
                self.crc_high = crc32_update(self.crc_high, chunk);
            }
        }
        self.set_percent((2 * self.scan.offset).checked_div(size).unwrap_or(0));
        if self.scan.offset < size {
            return;
        }
        finish_scan(&self.scan, &mut self.st.image);
        if !self.opt.force && !self.st.image.has_handshake {
            self.fail(FlashError::ImageNoHandshake, 0, now);
            return;
        }
        // The board check comes before anything touches the STM.
        self.st.board = check_board(&self.st.image.hw_tag, &self.st.board_hw);
        if !self.opt.force && (self.st.image.hw_conflict || self.st.board == BoardCheck::Mismatch) {
            self.fail(FlashError::BoardMismatch, 0, now);
            return;
        }
        if !self.opt.force && self.st.board == BoardCheck::BoardRequired {
            self.fail(FlashError::BoardRequired, 0, now);
            return;
        }
        self.enter(FlashPhase::Resetting, now);
    }

    /// NRST pulse for Resetting (UART 8E1: at the handshake baud in normal mode, at opt.baud in
    /// blank mode), Starting and the failure cleanup (both UART 8N1 at the application baud).
    fn step_pulse(&mut self, t: &mut dyn FlashTransport, now: u32) {
        let boot = !self.cleanup && self.st.phase == FlashPhase::Resetting;
        if self.sub == 0 {
            t.set_reset(true);
            if boot {
                let baud = if self.opt.blank {
                    self.session_baud
                } else {
                    HANDSHAKE_BAUD
                };
                t.configure(baud, true);
            } else {
                t.configure(APP_BAUD, false);
            }
            t.discard_input();
            self.rx_len = 0;
            self.touched = true;
            self.wait_start_ms = now;
            self.sub = 1;
            if boot {
                self.set_percent(2);
            }
            return;
        }
        if elapsed_ms(now, self.wait_start_ms) < u32::from(self.opt.reset_pulse_ms) {
            return;
        }
        t.set_reset(false);
        self.release_ms = now;
        if self.cleanup {
            self.finish(FlashPhase::Failed, now);
        } else if boot {
            if self.opt.blank {
                self.enter(FlashPhase::Sync, now);
                self.set_percent(4);
            } else {
                self.enter(FlashPhase::Handshake, now);
                self.set_percent(3);
            }
        } else {
            // The application runs at 8N1 now: a later failure needs no new pulse.
            self.touched = false;
            self.line_drop = false;
            self.enter(FlashPhase::WaitingApp, now);
            self.set_percent(97);
        }
    }

    fn step_handshake(&mut self, t: &mut dyn FlashTransport, now: u32) {
        let elapsed = elapsed_ms(now, self.release_ms);
        if self.sub == 0 {
            // Reset glitches and stale bytes until the first DEADBEEF are no answer.
            t.discard_input();
            self.rx_len = 0;
            if elapsed < u32::from(self.opt.handshake_first_ms) {
                return;
            }
            if elapsed >= u32::from(self.opt.handshake_window_ms) {
                self.fail(FlashError::HandshakeTimeout, 0, now);
                return;
            }
            if !self.put(t, HANDSHAKE, now) {
                return;
            }
            self.last_send_ms = now;
            self.sub = 1;
            return;
        }
        self.pump(t);
        let got = self.rx.get(..self.rx_len).unwrap_or_default();
        if got.windows(BEEFIT.len()).any(|w| w == BEEFIT) {
            self.rx_len = 0;
            // The ROM bootloader autobauds on 0x7F, so it may run slower than the boot window.
            if self.session_baud != HANDSHAKE_BAUD {
                t.configure(self.session_baud, true);
            }
            self.enter(FlashPhase::Sync, now);
            self.set_percent(4);
            return;
        }
        // Keep a possible "BEEFI" prefix for the next read.
        let stale = self.rx_len.saturating_sub(BEEFIT.len() - 1);
        self.rx.copy_within(stale..self.rx_len, 0);
        self.rx_len -= stale;
        if elapsed >= u32::from(self.opt.handshake_window_ms) {
            self.fail(FlashError::HandshakeTimeout, 0, now);
            return;
        }
        if elapsed_ms(now, self.last_send_ms) >= u32::from(self.opt.handshake_repeat_ms) {
            if !self.put(t, HANDSHAKE, now) {
                return;
            }
            self.last_send_ms = now;
        }
    }

    fn step_sync(&mut self, t: &mut dyn FlashTransport, now: u32) {
        let attempts = self.opt.sync_attempts.max(1);
        if self.sub == 0 {
            if elapsed_ms(now, self.phase_start_ms) < u32::from(self.opt.after_beefit_ms) {
                return;
            }
            self.sync_tries = 1;
            if !self.send(t, &[SYNC], now, self.ack_timeout()) {
                return;
            }
            self.sub = 1;
            return;
        }
        let r = self.wait_reply(t, now, 1);
        if r == Resp::Pending {
            return;
        }
        if r != Resp::Timeout {
            // ACK, or NACK = already synced
            self.enter(FlashPhase::GetId, now);
            self.set_percent(5);
            return;
        }
        if self.sync_tries >= attempts {
            if !self.fallback_session(now) {
                self.fail(FlashError::SyncFailed, 0, now);
            }
            return;
        }
        self.sync_tries += 1;
        self.send(t, &[SYNC], now, self.ack_timeout());
    }

    /// sub 0/1: GET (bootloader version, not fatal); sub 2/3: GET ID. Both replies are ACK, N,
    /// N+1 bytes, ACK.
    fn step_get_id(&mut self, t: &mut dyn FlashTransport, now: u32) {
        if self.sub == 0 || self.sub == 2 {
            let cmd = if self.sub == 0 { CMD_GET } else { CMD_GET_ID };
            if !self.send(t, &[cmd, !cmd], now, self.ack_timeout()) {
                return;
            }
            self.sub += 1;
            return;
        }
        let mut need = 2;
        let mut r = self.wait_reply(t, now, need);
        if r == Resp::Ack {
            need = usize::from(self.rx[1]) + 4;
            r = self.wait_reply(t, now, need);
        }
        if r == Resp::Pending {
            return;
        }
        if r == Resp::Ack && self.rx.get(need - 1) != Some(&ACK) {
            r = Resp::Nack;
        }
        if self.sub == 1 {
            if r == Resp::Ack {
                self.st.bootloader_version = self.rx[2];
            }
            self.sub = 2;
            return;
        }
        if r != Resp::Ack {
            if self.retries < self.opt.block_retries {
                self.retries += 1;
                self.sub = 2;
                return;
            }
            // A silent GetId (not a NACK) may be a bootloader at another baud.
            if r == Resp::Nack || !self.fallback_session(now) {
                self.fail(op_error(r), 0, now);
            }
            return;
        }
        let pid = if self.rx[1] == 0 {
            u16::from(self.rx[2])
        } else {
            u16::from_be_bytes([self.rx[2], self.rx[3]])
        };
        self.st.chip_pid = pid;
        let e = check_chip(&self.st.image, pid);
        if e != FlashError::None {
            self.fail(e, 0, now);
            return;
        }
        self.upper = self.st.image.padded_size > SECTOR0_BYTES;
        self.enter(FlashPhase::Erasing, now);
    }

    /// 0x44 with the sector list of the pass: N-1 (2 bytes), sector numbers (2 bytes each), XOR
    /// of all of them.
    fn step_erasing(&mut self, t: &mut dyn FlashTransport, now: u32) {
        if self.sub == 0 {
            if !self.send(t, &ERASE_CMD, now, self.ack_timeout()) {
                return;
            }
            self.sub = 1;
            return;
        }
        let r = self.wait_reply(t, now, 1);
        if r == Resp::Pending {
            return;
        }
        let (first, _) = self.pass_blocks();
        if r != Resp::Ack {
            self.op_failed(op_error(r), FLASH_BASE + first * BLOCK_SIZE, now);
            return;
        }
        if self.sub == 1 {
            let n = sectors_for_image(self.st.image.padded_size);
            let sectors = if self.upper { 1..n } else { 0..1 };
            let mut frame: heapless::Vec<u8, 19> = heapless::Vec::new();
            let _ = frame.push(0);
            let _ = frame.push(sectors.len() as u8 - 1);
            for s in sectors {
                let _ = frame.push(0);
                let _ = frame.push(s);
            }
            let cs = frame.iter().fold(0, |cs, &b| cs ^ b);
            let _ = frame.push(cs);
            if !self.send(t, &frame, now, self.opt.erase_timeout_ms) {
                return;
            }
            self.sub = 2;
            return;
        }
        self.set_percent(15);
        self.st.bytes_done = self.pass_base();
        self.block = 0;
        self.enter(FlashPhase::Writing, now);
    }

    /// The upper pass from its first block upwards; the sector-0 pass from block 1 upwards and
    /// block 0 (the vector table) last.
    fn step_writing(&mut self, t: &mut dyn FlashTransport, img: &mut dyn FlashImage, now: u32) {
        let (first, end) = self.pass_blocks();
        let count = end - first;
        let block = if self.upper {
            first + self.block
        } else if self.block + 1 < count {
            self.block + 1
        } else {
            0
        };
        let addr = FLASH_BASE + block * BLOCK_SIZE;
        if self.sub == 0 {
            if !self.load_block(img, block) {
                self.fail(FlashError::ImageRead, addr, now);
                return;
            }
            if !self.send(t, &WRITE_CMD, now, self.ack_timeout()) {
                return;
            }
            self.sub = 1;
            return;
        }
        let r = self.wait_reply(t, now, 1);
        if r == Resp::Pending {
            return;
        }
        if r != Resp::Ack {
            self.op_failed(op_error(r), addr, now);
            return;
        }
        if self.sub == 1 {
            if !self.send(t, &address_frame(addr), now, self.ack_timeout()) {
                return;
            }
            self.sub = 2;
            return;
        }
        let len = self.block_len(block);
        if self.sub == 2 {
            let frame = self.tx;
            let data = frame.get(..len as usize + 2).unwrap_or_default();
            if !self.send(t, data, now, self.ack_timeout()) {
                return;
            }
            self.sub = 3;
            return;
        }
        self.st.bytes_done += len;
        self.set_percent(
            15 + (60 * self.st.bytes_done)
                .checked_div(self.st.bytes_total)
                .unwrap_or(0),
        );
        self.block += 1;
        self.retries = 0;
        self.sub = 0;
        if self.block >= count {
            self.st.bytes_done = self.pass_base();
            self.block = 0;
            self.verify_crc = 0xFFFF_FFFF;
            self.enter(FlashPhase::Verifying, now);
            self.set_percent(75);
        }
    }

    fn step_verifying(&mut self, t: &mut dyn FlashTransport, img: &mut dyn FlashImage, now: u32) {
        let (first, end) = self.pass_blocks();
        let block = first + self.block;
        let addr = FLASH_BASE + block * BLOCK_SIZE;
        let len = self.block_len(block);
        if self.sub == 0 {
            if !self.load_block(img, block) {
                self.fail(FlashError::ImageRead, addr, now);
                return;
            }
            if !self.send(t, &READ_CMD, now, self.ack_timeout()) {
                return;
            }
            self.sub = 1;
            return;
        }
        let need = if self.sub == 3 { len as usize + 1 } else { 1 };
        let r = self.wait_reply(t, now, need);
        if r == Resp::Pending {
            return;
        }
        if r != Resp::Ack {
            self.op_failed(op_error(r), addr, now);
            return;
        }
        if self.sub == 1 {
            if !self.send(t, &address_frame(addr), now, self.ack_timeout()) {
                return;
            }
            self.sub = 2;
            return;
        }
        if self.sub == 2 {
            let n = (len - 1) as u8;
            // ACK + N+1 data bytes at 11 bits each, plus 200 ms.
            let data_ms = wire_ms(len as usize + 1, self.session_baud) + 200;
            if !self.send(t, &[n, !n], now, self.ack_timeout() + data_ms) {
                return;
            }
            self.sub = 3;
            return;
        }
        let range = 1..=len as usize;
        let got = self.rx.get(range.clone()).unwrap_or_default();
        let want = self.tx.get(range).unwrap_or_default();
        if let Some(i) = got.iter().zip(want).position(|(a, b)| a != b) {
            self.op_failed(FlashError::VerifyMismatch, addr + i as u32, now);
            return;
        }
        let off = block * BLOCK_SIZE;
        let image_bytes = self.st.image.size.saturating_sub(off).min(len) as usize;
        let data = self.tx.get(1..1 + image_bytes).unwrap_or_default();
        self.verify_crc = crc32_update(self.verify_crc, data);
        self.st.bytes_done += len;
        self.set_percent(
            75 + (20 * self.st.bytes_done)
                .checked_div(self.st.bytes_total)
                .unwrap_or(0),
        );
        self.block += 1;
        self.retries = 0;
        self.sub = 0;
        if self.block < end - first {
            return;
        }
        let expected = if self.upper {
            self.crc_high
        } else {
            self.crc_low
        };
        if self.verify_crc != expected {
            self.fail(FlashError::ImageRead, 0, now); // the file changed during the run
            return;
        }
        if self.upper {
            // D9: sectors 1..n are written and verified; sector 0 now.
            self.upper = false;
            self.enter(FlashPhase::Erasing, now);
            return;
        }
        if self.opt.blank {
            // BOOT0 is still set: a reset would start the ROM bootloader again. The user
            // removes the jumper and resets the STM.
            t.configure(APP_BAUD, false);
            self.st.manual_reset = true;
            self.finish(FlashPhase::Done, now);
            return;
        }
        self.enter(FlashPhase::Starting, now);
        self.set_percent(95);
    }

    /// One more session at fallback_baud: new NRST pulse, then the handshake at 115200 (normal
    /// mode) or 0x7F at the fallback baud (blank mode).
    fn fallback_session(&mut self, now: u32) -> bool {
        if self.opt.fallback_baud == 0 || self.session_baud == self.opt.fallback_baud {
            return false;
        }
        self.session_baud = self.opt.fallback_baud;
        self.st.baud = self.session_baud;
        self.sync_tries = 0;
        self.enter(FlashPhase::Resetting, now);
        true
    }

    fn step_waiting_app(&mut self, t: &mut dyn FlashTransport, now: u32) {
        let mut buf = [0u8; 64];
        let mut budget = APP_READ_PER_STEP;
        loop {
            let want = budget.min(buf.len());
            if want == 0 || !self.active() {
                break;
            }
            let got = t.read(&mut buf[..want]).min(want);
            if got == 0 {
                break;
            }
            budget -= got;
            for &c in &buf[..got] {
                if !self.active() {
                    break;
                }
                match c {
                    b'\r' | b'\n' => {
                        if !self.line_drop {
                            self.on_app_line(now);
                        }
                        self.rx_len = 0;
                        self.line_drop = false;
                    }
                    0x20..=0x7E if self.rx_len < self.rx.len() => {
                        self.rx[self.rx_len] = c;
                        self.rx_len += 1;
                    }
                    // noise or an over-long line: drop up to CR/LF
                    _ => self.line_drop = true,
                }
            }
        }
        if !self.active() {
            return;
        }
        let elapsed = elapsed_ms(now, self.release_ms);
        if elapsed >= u32::from(self.opt.app_timeout_ms) {
            self.fail(FlashError::AppNotResponding, 0, now);
            return;
        }
        if elapsed < u32::from(self.opt.app_boot_ms) {
            return;
        }
        if self.sub != 0 && elapsed_ms(now, self.last_send_ms) < u32::from(self.opt.app_poll_ms) {
            return;
        }
        let req = build_get_version();
        if self.sub == 0 {
            t.discard_input(); // boot-window noise
            self.rx_len = 0;
            self.line_drop = false;
        }
        if !self.put(t, &req.text, now) {
            return;
        }
        self.last_send_ms = now;
        self.sub = 1;
        self.set_percent(98);
    }

    /// "gvers <version> [<build>] ": the new application answers. Lines shorter than "gvers "
    /// (an empty one too) are not.
    fn on_app_line(&mut self, now: u32) {
        let line = self.rx.get(..self.rx_len).unwrap_or_default();
        let Some(rest) = line.strip_prefix(b"gvers ") else {
            return;
        };
        let start = rest.iter().position(|&c| c != b' ').unwrap_or(rest.len());
        let word = rest.get(start..).unwrap_or_default();
        let end = word.iter().position(|&c| c == b' ').unwrap_or(word.len());
        let app = parse_version(word.get(..end).unwrap_or_default());
        if !app.valid {
            return;
        }
        self.st.app_version = app;
        let app = &self.st.app_version;
        let image = &self.st.image;
        if !self.opt.force
            && !image.version.is_empty()
            && !same_version(&parse_version(&image.version), app)
        {
            self.fail(FlashError::AppVersionMismatch, 0, now);
            return;
        }
        if !self.opt.force
            && !image.hw_tag.is_empty()
            && !app.hw.is_empty()
            && image.hw_tag != app.hw
        {
            self.fail(FlashError::AppVersionMismatch, 0, now);
            return;
        }
        self.finish(FlashPhase::Done, now);
    }
}

#[cfg(test)]
mod rig;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_d9;
