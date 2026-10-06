//! AN3155 STM simulator for the flasher: v1 boot window (DEADBEEF/BEEFIT), ROM bootloader with a
//! flash model and an application answering gvers (port of test/native/support/sim_stm.h,
//! shared by the flasher tests and the session tests).
//!
//! The C++ simulator reads the rig's clock through a reference; here the rig sets
//! [`SimStm::now`] before every flasher step. Times are `u32` milliseconds and wrap like the
//! C++ ones; the queue compares raw times (`t <= now`) as the C++ does.

use crate::stm_flasher::FlashTransport;
use crate::test_support::Lcg;
use std::collections::{BTreeSet, VecDeque};
use std::vec;
use std::vec::Vec;

pub const ACK: u8 = 0x79;
pub const NACK: u8 = 0x1F;
/// Flash base address (C++ `kBase`).
pub const BASE: u32 = 0x0800_0000;
pub const KIB: u32 = 1024;

/// `vdm_test::Lcg` of sim_stm.h: Numerical Recipes steps, `next()` = state >> 8.
pub struct SimLcg(Lcg);

impl SimLcg {
    pub fn new(seed: u32) -> Self {
        Self(Lcg::numerical_recipes(seed))
    }

    pub fn next(&mut self) -> u32 {
        self.0.next_state() >> 8
    }

    pub fn below(&mut self, n: u32) -> u32 {
        if n == 0 {
            0
        } else {
            self.next() % n
        }
    }
}

/// One programmed block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WriteRec {
    pub addr: u32,
    pub len: u32,
}

/// What the bootloader did to the flash, in order ([`SimStm::events`], an addition of the Rust
/// port: the C++ simulator keeps erase frames, programmed blocks and read-backs in separate
/// lists).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SimEvent {
    /// sectors erased by one 0x44 frame
    Erase(Vec<u16>),
    /// block programmed at this address
    Program(u32),
    /// block read back from this address
    Read(u32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    App,
    Reset,
    Window,
    Jumping,
    Boot,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Bs {
    Unsynced,
    Idle,
    Cmd2,
    Addr,
    WrN,
    WrData,
    EraseHdr,
    EraseList,
    ReadN,
}

/// The simulated STM (C++ `vdm_test::SimStm`).
pub struct SimStm {
    /// The rig's clock (C++ `now_`, a reference there).
    pub now: u32,

    // ---- behaviour knobs
    /// running app answers DEADBEEF (v1 fixed chunks)
    pub has_boot_loop: bool,
    pub window_ms: u32,
    /// bytes before this after release are lost
    pub window_ready_ms: u32,
    /// garbage bytes in the window chunk at boot
    pub stray: i32,
    /// noise before "BEEFIT"
    pub beefit_prefix: Vec<u8>,
    /// sent 1 ms after every reset release
    pub boot_noise: Vec<u8>,
    /// per-byte delay of replies (split reads)
    pub reply_spacing_ms: u32,
    /// next N releases boot into the ROM bootloader
    pub boot_pin_resets: i32,
    pub app_reply: Vec<u8>,
    pub app_answers: bool,
    /// sent before each gvers reply
    pub app_noise: Vec<u8>,
    pub pid: u16,
    pub pid_n: u8,
    pub bl_version: u8,
    pub get_silent: bool,
    pub get_id_silent: i32,
    pub get_id_bad_end: i32,
    pub sync_silent: i32,
    /// bootloader already synced
    pub sync_nack: bool,
    pub rdp: bool,
    pub nack_erase: i32,
    pub drop_erase_ack: i32,
    pub erase_delay_ms: u32,
    pub nack_write_data: i32,
    pub drop_write_ack: i32,
    /// NACK every write to this block address
    pub nack_write_addr: BTreeSet<u32>,
    pub corrupt_reads: i32,
    pub corrupt_offset: u32,
    /// flash addresses that program wrong
    pub stuck: BTreeSet<u32>,
    /// next N replies get a 0x55 byte in front
    pub noise_replies: i32,
    /// bootloader answers within the same millisecond
    pub instant_replies: bool,
    /// the ROM bootloader ignores bytes sent faster than this
    pub boot_max_baud: u32,
    /// one corrupted read-back per entry (block address; a multiset in C++)
    pub corrupt_read_at: Vec<u32>,
    /// sent once each, on every 8E1 '\n' received
    pub echoes: VecDeque<Vec<u8>>,
    /// sent on every 8E1 '\n' once `echoes` is empty
    pub echo_repeat: Vec<u8>,
    pub write_limit: usize,

    // ---- observation
    pub flash: Vec<u8>,
    pub original: Vec<u8>,
    pub configs: Vec<(u32, bool)>,
    /// (time, asserted)
    pub resets: Vec<(u32, bool)>,
    pub writes: Vec<(u32, Vec<u8>)>,
    pub programmed: Vec<WriteRec>,
    pub read_addrs: Vec<u32>,
    pub erase_frames: Vec<Vec<u8>>,
    pub events: Vec<SimEvent>,
    pub commands: Vec<u8>,
    pub handshake_times: Vec<u32>,
    pub gvers_times: Vec<u32>,
    pub sync_times: Vec<u32>,
    pub beefit_at: u32,
    pub in_reset: bool,
    pub even: bool,
    pub baud: u32,

    mode: Mode,
    boot_at: u32,
    jump_at: u32,
    app_ready_at: u32,
    chunk: Vec<u8>,
    line: Vec<u8>,
    out: VecDeque<(u32, u8)>,
    bs: Bs,
    cmd: u8,
    next: u8,
    buf: Vec<u8>,
    need: usize,
    addr: u32,
    n: u8,
}

impl SimStm {
    /// A chip with 512 KiB of random "old" flash, running its application, at time `now`.
    pub fn new(now: u32) -> Self {
        let mut r = SimLcg::new(99);
        let flash: Vec<u8> = (0..512 * KIB).map(|_| r.next() as u8).collect();
        Self {
            now,
            has_boot_loop: true,
            window_ms: 3000,
            window_ready_ms: 12,
            stray: 0,
            beefit_prefix: Vec::new(),
            boot_noise: Vec::new(),
            reply_spacing_ms: 0,
            boot_pin_resets: 0,
            app_reply: b"gvers 1.4.9_Dev_C1 1 ".to_vec(),
            app_answers: true,
            app_noise: Vec::new(),
            pid: 0x431,
            pid_n: 1,
            bl_version: 0x31,
            get_silent: false,
            get_id_silent: 0,
            get_id_bad_end: 0,
            sync_silent: 0,
            sync_nack: false,
            rdp: false,
            nack_erase: 0,
            drop_erase_ack: 0,
            erase_delay_ms: 300,
            nack_write_data: 0,
            drop_write_ack: 0,
            nack_write_addr: BTreeSet::new(),
            corrupt_reads: 0,
            corrupt_offset: 5,
            stuck: BTreeSet::new(),
            noise_replies: 0,
            instant_replies: false,
            boot_max_baud: 115_200,
            corrupt_read_at: Vec::new(),
            echoes: VecDeque::new(),
            echo_repeat: Vec::new(),
            write_limit: usize::MAX,
            original: flash.clone(),
            flash,
            configs: Vec::new(),
            resets: Vec::new(),
            writes: Vec::new(),
            programmed: Vec::new(),
            read_addrs: Vec::new(),
            erase_frames: Vec::new(),
            events: Vec::new(),
            commands: Vec::new(),
            handshake_times: Vec::new(),
            gvers_times: Vec::new(),
            sync_times: Vec::new(),
            beefit_at: 0,
            in_reset: false,
            even: false,
            baud: 0,
            mode: Mode::App,
            boot_at: 0,
            jump_at: 0,
            app_ready_at: 0,
            chunk: Vec::new(),
            line: Vec::new(),
            out: VecDeque::new(),
            bs: Bs::Unsynced,
            cmd: 0,
            next: 0,
            buf: Vec::new(),
            need: 0,
            addr: 0,
            n: 0,
        }
    }

    /// The STM runs (or is about to run) its application. (The C++ `inBootloader()` is used by
    /// no test and not ported.)
    pub fn in_app(&mut self) -> bool {
        self.advance();
        self.mode == Mode::App
    }

    /// Writes equal to `frame`.
    pub fn writes_equal(&self, frame: &[u8]) -> usize {
        self.writes.iter().filter(|w| w.1 == frame).count()
    }

    fn enter_bootloader(&mut self) {
        self.mode = Mode::Boot;
        self.bs = if self.sync_nack {
            Bs::Idle
        } else {
            Bs::Unsynced
        };
    }

    fn advance(&mut self) {
        if self.mode == Mode::Window && self.now.wrapping_sub(self.boot_at) >= self.window_ms {
            self.mode = Mode::App;
            self.app_ready_at = self.now.wrapping_add(600);
        }
        if self.mode == Mode::Jumping && self.now.wrapping_sub(self.jump_at) < 0x8000_0000 {
            self.enter_bootloader();
        }
    }

    fn reply(&mut self, bytes: &[u8], delay: u32) {
        let delay = if self.instant_replies && self.mode == Mode::Boot {
            0
        } else {
            delay
        };
        let mut t = self.now.wrapping_add(delay);
        if self.noise_replies > 0 && self.mode == Mode::Boot {
            self.noise_replies -= 1;
            self.out.push_back((t, 0x55));
        }
        for &b in bytes {
            self.out.push_back((t, b));
            t = t.wrapping_add(self.reply_spacing_ms);
        }
    }

    fn receive(&mut self, b: u8) {
        if b == b'\n' && self.even {
            if let Some(e) = self.echoes.pop_front() {
                self.reply(&e, 1);
            } else if !self.echo_repeat.is_empty() {
                let e = self.echo_repeat.clone();
                self.reply(&e, 1);
            }
        }
        match self.mode {
            Mode::Reset | Mode::Jumping => {}
            Mode::Window => self.window_byte(b),
            Mode::App => self.app_byte(b),
            Mode::Boot => self.boot_byte(b),
        }
    }

    fn window_byte(&mut self, b: u8) {
        if !self.even
            || self.baud != 115_200
            || self.now.wrapping_sub(self.boot_at) < self.window_ready_ms
        {
            return;
        }
        self.handshake_times.push(self.now);
        self.chunk.push(b);
        if self.chunk.len() < 8 {
            return;
        }
        let matched = self.chunk == b"DEADBEEF";
        self.chunk.clear();
        if !matched {
            return;
        }
        self.beefit_at = self.now.wrapping_add(10);
        let mut text = self.beefit_prefix.clone();
        text.extend_from_slice(b"BEEFIT\r\n");
        self.reply(&text, 10);
        self.mode = Mode::Jumping;
        self.jump_at = self.now.wrapping_add(210);
    }

    fn app_byte(&mut self, b: u8) {
        if self.even {
            return; // 8E1 bytes are garbage for the 8N1 application
        }
        if b != b'\r' && b != b'\n' {
            self.line.push(b);
            return;
        }
        if self.line == b"gvers " {
            self.gvers_times.push(self.now);
            if self.app_answers && self.now >= self.app_ready_at {
                let mut text = self.app_noise.clone();
                text.extend_from_slice(&self.app_reply);
                text.extend_from_slice(b"\r\n");
                self.reply(&text, 3);
            }
        }
        self.line.clear();
    }

    fn boot_byte(&mut self, b: u8) {
        if !self.even || self.baud > self.boot_max_baud {
            return;
        }
        match self.bs {
            Bs::Unsynced => {
                if b != 0x7F {
                    return;
                }
                self.sync_times.push(self.now);
                if self.sync_silent > 0 {
                    self.sync_silent -= 1;
                    return;
                }
                self.reply(&[ACK], 1);
                self.bs = Bs::Idle;
            }
            Bs::Idle => {
                if b == 0x7F {
                    self.sync_times.push(self.now);
                }
                self.cmd = b;
                self.bs = Bs::Cmd2;
            }
            Bs::Cmd2 => self.command(b),
            Bs::Addr => self.addr_byte(b),
            Bs::WrN => {
                self.n = b;
                self.buf.clear();
                self.need = usize::from(self.n) + 2;
                self.bs = Bs::WrData;
            }
            Bs::WrData => {
                self.buf.push(b);
                if self.buf.len() == self.need {
                    self.write_done();
                }
            }
            Bs::EraseHdr => {
                self.buf.push(b);
                if self.buf.len() == 2 {
                    let nm1 = u16::from_be_bytes([self.buf[0], self.buf[1]]);
                    self.need = if nm1 >= 0xFFF0 {
                        3
                    } else {
                        2 + 2 * (usize::from(nm1) + 1) + 1
                    };
                    self.bs = Bs::EraseList;
                }
            }
            Bs::EraseList => {
                self.buf.push(b);
                if self.buf.len() == self.need {
                    self.erase_done();
                }
            }
            Bs::ReadN => {
                self.buf.push(b);
                if self.buf.len() == 2 {
                    self.read_done();
                }
            }
        }
    }

    fn command(&mut self, b: u8) {
        self.bs = Bs::Idle;
        if b != self.cmd ^ 0xFF {
            self.reply(&[NACK], 1);
            return;
        }
        self.commands.push(self.cmd);
        if self.rdp && matches!(self.cmd, 0x31 | 0x11 | 0x44) {
            self.reply(&[NACK], 1);
            return;
        }
        match self.cmd {
            0x00 => {
                if !self.get_silent {
                    let r = [
                        ACK,
                        11,
                        self.bl_version,
                        0x00,
                        0x01,
                        0x02,
                        0x11,
                        0x21,
                        0x31,
                        0x44,
                        0x63,
                        0x73,
                        0x82,
                        0x92,
                        ACK,
                    ];
                    self.reply(&r, 1);
                }
            }
            0x02 => {
                if self.get_id_silent > 0 {
                    self.get_id_silent -= 1;
                    return;
                }
                let mut r = vec![ACK, self.pid_n];
                if self.pid_n == 0 {
                    r.push(self.pid as u8);
                } else {
                    r.push((self.pid >> 8) as u8);
                    r.push(self.pid as u8);
                    r.extend((1..self.pid_n).map(|_| 0xAA));
                }
                if self.get_id_bad_end > 0 {
                    self.get_id_bad_end -= 1;
                    r.push(0x00);
                } else {
                    r.push(ACK);
                }
                self.reply(&r, 1);
            }
            0x44 => {
                if self.nack_erase > 0 {
                    self.nack_erase -= 1;
                    self.reply(&[NACK], 1);
                    return;
                }
                self.reply(&[ACK], 1);
                self.buf.clear();
                self.bs = Bs::EraseHdr;
            }
            0x31 | 0x11 => {
                self.reply(&[ACK], 1);
                self.next = self.cmd;
                self.buf.clear();
                self.bs = Bs::Addr;
            }
            _ => self.reply(&[NACK], 1),
        }
    }

    fn addr_byte(&mut self, b: u8) {
        self.buf.push(b);
        if self.buf.len() < 5 {
            return;
        }
        self.bs = Bs::Idle;
        let [a, b1, c, d, x] = [
            self.buf[0],
            self.buf[1],
            self.buf[2],
            self.buf[3],
            self.buf[4],
        ];
        if a ^ b1 ^ c ^ d != x {
            self.reply(&[NACK], 1);
            return;
        }
        self.addr = u32::from_be_bytes([a, b1, c, d]);
        if self.addr < BASE || self.addr >= BASE + self.flash.len() as u32 {
            self.reply(&[NACK], 1);
            return;
        }
        self.reply(&[ACK], 1);
        self.buf.clear();
        self.bs = if self.next == 0x31 {
            Bs::WrN
        } else {
            Bs::ReadN
        };
    }

    fn write_done(&mut self) {
        self.bs = Bs::Idle;
        let len = usize::from(self.n) + 1;
        let cs = self.buf[..len].iter().fold(self.n, |cs, &b| cs ^ b);
        let off = (self.addr - BASE) as usize;
        if cs != self.buf[len]
            || !len.is_multiple_of(4)
            || !self.addr.is_multiple_of(4)
            || off + len > self.flash.len()
            || self.nack_write_addr.contains(&self.addr)
        {
            self.reply(&[NACK], 1);
            return;
        }
        if self.nack_write_data > 0 {
            self.nack_write_data -= 1;
            self.reply(&[NACK], 1);
            return;
        }
        let addr = self.addr;
        for (i, &b) in self.buf[..len].iter().enumerate() {
            let v = if self.stuck.contains(&(addr + i as u32)) {
                b ^ 0x01
            } else {
                b
            };
            self.flash[off + i] &= v; // programming only clears bits
        }
        self.programmed.push(WriteRec {
            addr,
            len: len as u32,
        });
        self.events.push(SimEvent::Program(addr));
        if self.drop_write_ack > 0 {
            self.drop_write_ack -= 1;
            return;
        }
        self.reply(&[ACK], 2);
    }

    fn erase_done(&mut self) {
        self.bs = Bs::Idle;
        let (body, last) = self.buf.split_at(self.buf.len() - 1);
        let cs = body.iter().fold(0, |cs, &b| cs ^ b);
        self.erase_frames.push(self.buf.clone());
        if cs != last[0] || self.buf.len() == 3 {
            self.reply(&[NACK], 1);
            return;
        }
        const SEC: [u32; 8] = [16, 16, 16, 16, 64, 128, 128, 128];
        let list: Vec<u16> = self.buf[2..self.buf.len() - 1]
            .chunks(2)
            .map(|p| u16::from_be_bytes([p[0], p[1]]))
            .collect();
        for &s in &list {
            if s >= 8 {
                self.reply(&[NACK], 1);
                return;
            }
            let start = SEC[..usize::from(s)].iter().sum::<u32>() * KIB;
            let end = start + SEC[usize::from(s)] * KIB;
            self.flash[start as usize..end as usize].fill(0xFF);
        }
        self.events.push(SimEvent::Erase(list));
        if self.drop_erase_ack > 0 {
            self.drop_erase_ack -= 1;
            return;
        }
        let delay = self.erase_delay_ms;
        self.reply(&[ACK], delay);
    }

    fn read_done(&mut self) {
        self.bs = Bs::Idle;
        if self.buf[1] != self.buf[0] ^ 0xFF {
            self.reply(&[NACK], 1);
            return;
        }
        self.read_addrs.push(self.addr);
        self.events.push(SimEvent::Read(self.addr));
        let len = usize::from(self.buf[0]) + 1;
        let off = (self.addr - BASE) as usize;
        let mut r = vec![ACK];
        r.extend((0..len).map(|i| self.flash.get(off + i).copied().unwrap_or(0)));
        let at = 1 + self.corrupt_offset as usize;
        if self.corrupt_reads > 0 {
            self.corrupt_reads -= 1;
            r[at] ^= 0x40;
        }
        if let Some(i) = self.corrupt_read_at.iter().position(|&a| a == self.addr) {
            self.corrupt_read_at.remove(i);
            r[at] ^= 0x40;
        }
        self.reply(&r, 2);
    }
}

impl FlashTransport for SimStm {
    fn configure(&mut self, baud: u32, even_parity: bool) {
        self.baud = baud;
        self.even = even_parity;
        self.configs.push((baud, even_parity));
    }

    fn write(&mut self, data: &[u8]) -> usize {
        self.advance();
        let n = data.len().min(self.write_limit);
        self.writes.push((self.now, data[..n].to_vec()));
        for &b in &data[..n] {
            self.receive(b);
        }
        n
    }

    fn read(&mut self, out: &mut [u8]) -> usize {
        self.advance();
        let mut n = 0;
        while n < out.len() {
            match self.out.front() {
                Some(&(t, b)) if t <= self.now => {
                    out[n] = b;
                    n += 1;
                    self.out.pop_front();
                }
                _ => break,
            }
        }
        n
    }

    fn discard_input(&mut self) {
        self.advance();
        while matches!(self.out.front(), Some(&(t, _)) if t <= self.now) {
            self.out.pop_front();
        }
    }

    fn set_reset(&mut self, asserted: bool) {
        self.resets.push((self.now, asserted));
        if asserted {
            self.in_reset = true;
            self.mode = Mode::Reset;
            self.out.clear();
            return;
        }
        if !self.in_reset {
            return;
        }
        self.in_reset = false;
        self.boot_at = self.now;
        self.chunk.clear();
        self.line.clear();
        if !self.boot_noise.is_empty() {
            let noise = self.boot_noise.clone();
            self.reply(&noise, 1);
        }
        if self.boot_pin_resets > 0 {
            self.boot_pin_resets -= 1;
            self.enter_bootloader();
        } else if self.has_boot_loop {
            self.mode = Mode::Window;
            for _ in 0..self.stray {
                self.chunk.push(b'x');
            }
        } else {
            self.mode = Mode::App;
            self.app_ready_at = self.now.wrapping_add(600);
        }
    }
}
