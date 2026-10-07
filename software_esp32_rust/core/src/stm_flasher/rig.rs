//! Test rig of the flasher tests (the helpers and the `Rig` of test_stm_flasher.cpp): images,
//! an in-memory [`FlashImage`], the simulated STM and a run loop that records percent and
//! phases.

use super::*;
use crate::test_support::sim_stm::{SimLcg, SimStm, BASE, KIB};
use std::string::String;
use std::vec::Vec;

/// Bitwise reference CRC-32.
pub fn ref_crc32(d: &[u8]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for &b in d {
        c ^= u32::from(b);
        for _ in 0..8 {
            c = if c & 1 != 0 {
                (c >> 1) ^ 0xEDB8_8320
            } else {
                c >> 1
            };
        }
    }
    !c
}

pub fn put32(v: &mut [u8], off: usize, x: u32) {
    v[off..off + 4].copy_from_slice(&x.to_le_bytes());
}

pub fn put_str(v: &mut [u8], off: usize, s: &[u8]) {
    v[off..off + s.len()].copy_from_slice(s);
}

/// Random body without printable runs (bytes 0x80..0xFF), vectors at 0, optional
/// "\x01DEADBEEF\0" "\x01BEEFIT\0" and "\x01<version>\0" near 3/4 (C++ `makeImage`).
pub fn make_image_seeded(
    size: usize,
    sp: u32,
    pc: u32,
    handshake: bool,
    version: &str,
    seed: u32,
) -> Vec<u8> {
    let mut r = SimLcg::new(seed);
    let mut v: Vec<u8> = (0..size).map(|_| (0x80 | r.next()) as u8).collect();
    if size >= 8 {
        put32(&mut v, 0, sp);
        put32(&mut v, 4, if pc != 0 { pc } else { BASE + 0x1C5 });
    }
    let mut at = size * 3 / 4;
    if handshake {
        assert!(at + 20 <= size);
        put_str(&mut v, at, b"\x01DEADBEEF\0\x01BEEFIT\0");
        at += 18;
    }
    if !version.is_empty() {
        assert!(at + version.len() + 2 <= size);
        v[at] = 0x01;
        put_str(&mut v, at + 1, version.as_bytes());
        v[at + 1 + version.len()] = 0;
    }
    v
}

/// C++ `makeImage(size, sp, pc, handshake, version)` (seed 7).
pub fn make_image_with(size: usize, sp: u32, pc: u32, handshake: bool, version: &str) -> Vec<u8> {
    make_image_seeded(size, sp, pc, handshake, version, 7)
}

/// C++ `makeImage(size)`: SP 0x20020000, reset vector 0x080001C5, handshake strings, version
/// "1.4.9_Dev".
pub fn make_image(size: usize) -> Vec<u8> {
    make_image_with(size, 0x2002_0000, 0, true, "1.4.9_Dev")
}

/// In-memory image (C++ `MemImage`).
pub struct MemImage {
    pub data: Vec<u8>,
    /// a read covering this offset fails
    pub fail_at: i64,
    pub size_override: u32,
    pub bytes_read: usize,
    pub max_read: usize,
    /// D9: `hold_low` fails (no memory)
    pub fail_hold: bool,
    /// D9: the copy `hold_low` made (later changes of `data` do not reach it, as the glue's RAM
    /// copy of the file)
    pub held: Vec<u8>,
    /// D9: `hold_low` calls
    pub holds: usize,
}

impl MemImage {
    pub fn new(data: Vec<u8>) -> Self {
        Self {
            data,
            fail_at: -1,
            size_override: 0,
            bytes_read: 0,
            max_read: 0,
            fail_hold: false,
            held: Vec::new(),
            holds: 0,
        }
    }
}

impl FlashImage for MemImage {
    fn size(&self) -> u32 {
        if self.size_override != 0 {
            self.size_override
        } else {
            self.data.len() as u32
        }
    }

    fn read(&mut self, offset: u32, out: &mut [u8]) -> bool {
        let len = out.len();
        self.bytes_read += len;
        self.max_read = self.max_read.max(len);
        let off = i64::from(offset);
        if self.fail_at >= 0 && self.fail_at >= off && self.fail_at < off + len as i64 {
            return false;
        }
        let offset = offset as usize;
        if offset > self.data.len() || len > self.data.len() - offset {
            return false;
        }
        out.copy_from_slice(&self.data[offset..offset + len]);
        true
    }

    fn hold_low(&mut self, len: u32) -> bool {
        self.holds += 1;
        self.held.clear();
        let len = len as usize;
        if self.fail_hold || len > self.data.len() {
            return false;
        }
        // a read like any other (the fail_at knob reaches it)
        let mut copy = std::vec![0u8; len];
        if !self.read(0, &mut copy) {
            return false;
        }
        self.held = copy;
        true
    }

    fn low(&self) -> &[u8] {
        &self.held
    }
}

/// Longest time a run may take without progress (percent, bytes done or session attempt): the
/// longest waits of the tests are 60 s, for the application or an erase. Frames on the wire do
/// not count, so a retry or resend loop fails here instead of running for the whole `max_ms`.
const STALL_MS: u32 = 70_000;

/// The C++ `Rig`: the simulated STM, an image, the flasher and its options.
pub struct Rig {
    pub now: u32,
    pub sim: SimStm,
    pub img: MemImage,
    pub f: StmFlasher,
    pub opt: FlashOptions,
    pub percents: Vec<u8>,
    pub phases: Vec<FlashPhase>,
    pub steps: usize,
    /// D9: a run stops in this phase too (it stays active in Sector0Pending)
    pub stop: Option<FlashPhase>,
}

impl Rig {
    pub fn new(image: Vec<u8>) -> Self {
        Self {
            now: 1000,
            sim: SimStm::new(1000),
            img: MemImage::new(image),
            f: StmFlasher::default(),
            opt: FlashOptions::default(),
            percents: Vec::new(),
            phases: Vec::new(),
            steps: 0,
            stop: None,
        }
    }

    pub fn begin(&mut self) -> bool {
        self.f.begin(&self.opt, self.now)
    }

    /// One flasher step at `now` (the simulator's clock follows the rig's).
    pub fn step(&mut self) -> FlashPhase {
        self.sim.now = self.now;
        self.f.step(&mut self.sim, &mut self.img, self.now)
    }

    fn progress_key(&self) -> (u8, u32, u8) {
        let st = self.f.status();
        (st.percent, st.bytes_done, st.attempt)
    }

    /// Steps every `step_ms` while the run is active, for at most `max_ms`, calling `hook`
    /// after every step. Panics after [`STALL_MS`] without progress (a stuck flasher fails
    /// instead of running for the whole `max_ms`).
    pub fn run_with(
        &mut self,
        mut hook: impl FnMut(&mut Rig),
        max_ms: u32,
        step_ms: u32,
    ) -> FlashPhase {
        let start = self.now;
        let mut key = self.progress_key();
        let mut moved_at = self.now;
        while self.f.active()
            && Some(self.f.status().phase) != self.stop
            && self.now.wrapping_sub(start) < max_ms
        {
            self.now = self.now.wrapping_add(step_ms);
            self.step();
            self.steps += 1;
            self.percents.push(self.f.status().percent);
            let phase = self.f.status().phase;
            if self.phases.last() != Some(&phase) {
                self.phases.push(phase);
            }
            hook(self);
            let k = self.progress_key();
            if k != key {
                key = k;
                moved_at = self.now;
            }
            assert!(
                self.now.wrapping_sub(moved_at) <= STALL_MS,
                "no progress for {STALL_MS} ms in {:?}",
                self.f.status().phase
            );
        }
        self.f.status().phase
    }

    /// C++ `run(hook)`: 2 ms steps, at most 30 minutes.
    pub fn run_hook(&mut self, hook: impl FnMut(&mut Rig)) -> FlashPhase {
        self.run_with(hook, 30 * 60 * 1000, 2)
    }

    pub fn run(&mut self) -> FlashPhase {
        self.run_hook(|_| {})
    }

    /// [`run_hook`](Self::run_hook) until the run ends or reaches `phase` (D9: Sector0Pending,
    /// where it stays active).
    pub fn run_to(&mut self, phase: FlashPhase, hook: impl FnMut(&mut Rig)) -> FlashPhase {
        self.stop = Some(phase);
        let end = self.run_hook(hook);
        self.stop = None;
        end
    }

    pub fn begin_and_run(&mut self) -> FlashPhase {
        assert!(self.begin());
        self.run()
    }

    pub fn flash_matches_image(&self) -> bool {
        let d = &self.img.data;
        let padded = (d.len() + 3) & !3;
        self.sim.flash[..d.len()] == d[..]
            && self.sim.flash[d.len()..padded].iter().all(|&b| b == 0xFF)
    }

    /// STM released from reset, running (or about to run) its application, UART 8N1.
    pub fn left_clean(&self) -> bool {
        matches!(self.sim.resets.last(), Some(&(_, false)))
            && self.sim.configs.last() == Some(&(115_200, false))
    }

    pub fn percent_monotonic(&self) -> bool {
        self.percents.windows(2).all(|w| w[1] >= w[0])
    }

    /// Only the handshake/app bytes: STM never touched.
    pub fn untouched(&self) -> bool {
        self.sim.resets.is_empty() && self.sim.configs.is_empty() && self.sim.writes.is_empty()
    }
}

/// Reference image rules (independent re-statement of the header contract).
pub fn ref_validate(d: &[u8], pid: u16, req: bool) -> FlashError {
    let size = d.len();
    if size == 0 {
        return FlashError::ImageEmpty;
    }
    if size > 512 * KIB as usize {
        return FlashError::ImageTooLarge;
    }
    if size < 8 {
        return FlashError::ImageBadVectors;
    }
    let sp = u32::from_le_bytes([d[0], d[1], d[2], d[3]]);
    let pc = u32::from_le_bytes([d[4], d[5], d[6], d[7]]);
    if !(sp > 0x2000_0000 && sp <= 0x2002_0000 && sp.is_multiple_of(4)) {
        return FlashError::ImageBadVectors;
    }
    if !(pc % 2 == 1 && pc >= BASE && pc < BASE + size as u32) {
        return FlashError::ImageBadVectors;
    }
    if pid != 0 {
        let (fl, top) = match pid {
            0x423 => (256 * KIB, 0x2001_0000),
            0x431 => (512 * KIB, 0x2002_0000),
            0x433 => (512 * KIB, 0x2001_8000),
            _ => (0, 0),
        };
        if fl == 0 {
            return FlashError::UnknownChip;
        }
        if size as u32 > fl {
            return FlashError::ImageTooLarge;
        }
        if sp > top {
            return FlashError::ImageChipMismatch;
        }
    }
    let has = |n: &[u8]| d.windows(n.len()).any(|w| w == n);
    if req && !(has(b"DEADBEEF") && has(b"BEEFIT")) {
        return FlashError::ImageNoHandshake;
    }
    FlashError::None
}

/// validate_image() on a copy of `d`.
pub fn validate(d: &[u8], pid: u16, req: bool, info: &mut ImageInfo) -> FlashError {
    let mut m = MemImage::new(d.to_vec());
    validate_image(&mut m, pid, req, info)
}

/// The version string validate_image() finds.
pub fn version_of(d: &[u8]) -> String {
    let mut info = ImageInfo::default();
    validate(d, 0, false, &mut info);
    String::from_utf8(info.version.to_vec()).expect("ASCII")
}

/// make_image() with printable strings (each "\x01" <s> "\0") written from `at` (C++ `tagged`).
pub fn tagged_at(strings: &[&[u8]], size: usize, at: usize, version: &str) -> Vec<u8> {
    let mut v = make_image_with(size, 0x2002_0000, 0, true, version);
    let mut at = at;
    for s in strings {
        let mut blob = std::vec![0x01];
        blob.extend_from_slice(s);
        blob.push(0);
        put_str(&mut v, at, &blob);
        at += s.len() + 2;
    }
    v
}

/// C++ `tagged(strings)`: 4096 bytes, strings from 1024, version "2.1.0-revamped".
pub fn tagged(strings: &[&[u8]]) -> Vec<u8> {
    tagged_at(strings, 4096, 1024, "2.1.0-revamped")
}

/// validate_image() must pass; the facts it found.
pub fn scanned(d: &[u8]) -> ImageInfo {
    let mut info = ImageInfo::default();
    assert_eq!(validate(d, 0, false, &mut info), FlashError::None);
    info
}

/// FlashOptions::board_hw / a Text<3> from a literal.
pub fn tag(s: &str) -> Text<3> {
    Text::from_slice(s.as_bytes()).expect("tag fits")
}
