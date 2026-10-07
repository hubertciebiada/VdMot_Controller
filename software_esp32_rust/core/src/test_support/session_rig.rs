//! StmSession test rig (port of test/native/support/session_rig.h): a fake port, the scripted
//! line-level STM ([`LineStm`]), the flash transport simulator ([`SimStm`]) and a loop that
//! drives the session like the stm_link task.
//!
//! The C++ rig owns the LineStm and hands the port a pointer to it (the port's NRST pulse resets
//! it); here the port owns it ([`Rig::stm`]). The C++ SimStm reads the rig's clock through a
//! reference; the rig sets [`SimStm::now`] before every flasher step.

use std::collections::VecDeque;
use std::string::String;
use std::vec::Vec;

use crate::calib_schedule::CalibFailure;
use crate::common::{Text, NO_VALVE};
use crate::config::{set_defaults, Config};
use crate::event_log::{Event, EventCode};
use crate::failsafe::RegulatorInput;
use crate::lease_client::LeaseClientSnapshot;
use crate::stm_codec::Profile;
use crate::stm_flasher::FlashImage;
use crate::stm_session::{StmSession, StmSessionPort};
use crate::stm_types::{StmCommand, StmCommandType, StmSaveState, StmSnapshot};
use crate::target_store::{PersistedTargets, RestoreSource};
use crate::test_support::line_stm::LineStm;
use crate::test_support::sim_stm::SimStm;
use crate::valve_model::TargetSource;

/// In-memory image (C++ `MemImage` of session_rig.h).
#[derive(Default)]
pub struct MemImage {
    pub data: Vec<u8>,
    /// D9: the bytes `hold_low` copied
    pub held: Vec<u8>,
}

impl FlashImage for MemImage {
    fn size(&self) -> u32 {
        self.data.len() as u32
    }

    fn read(&mut self, offset: u32, out: &mut [u8]) -> bool {
        let offset = offset as usize;
        match self.data.get(offset..offset.saturating_add(out.len())) {
            Some(src) => {
                out.copy_from_slice(src);
                true
            }
            None => false,
        }
    }

    fn hold_low(&mut self, len: u32) -> bool {
        match self.data.get(..len as usize) {
            Some(src) => {
                self.held = src.to_vec();
                true
            }
            None => false,
        }
    }

    fn low(&self) -> &[u8] {
        &self.held
    }
}

/// One `post_scheduled_calib_result` call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Calib {
    pub attempt: u16,
    pub ok: bool,
    pub reason: CalibFailure,
}

/// The fake port: records every call.
pub struct TestPort {
    pub stm: LineStm,
    pub events: Vec<Event>,
    pub last: StmSnapshot,
    pub publishes: usize,
    pub pulses: Vec<u32>,
    pub last_good: Vec<String>,
    pub restart: bool,
    pub flash_marks: usize,
    pub targets: Vec<PersistedTargets>,
    pub calibs: Vec<Calib>,
    pub saves: Vec<StmSaveState>,
    pub lease: LeaseClientSnapshot,
    pub lease_records: usize,
    pub image: MemImage,
    pub image_ok: bool,
    pub opened: Vec<String>,
    pub closed: usize,
    /// store_profile() calls
    pub profiles: Vec<Profile>,
    pub publish_times: Vec<u32>,
}

impl Default for TestPort {
    fn default() -> Self {
        Self {
            stm: LineStm::default(),
            events: Vec::new(),
            last: StmSnapshot::default(),
            publishes: 0,
            pulses: Vec::new(),
            last_good: Vec::new(),
            restart: false,
            flash_marks: 0,
            targets: Vec::new(),
            calibs: Vec::new(),
            saves: Vec::new(),
            lease: LeaseClientSnapshot::default(),
            lease_records: 0,
            image: MemImage::default(),
            image_ok: true,
            opened: Vec::new(),
            closed: 0,
            profiles: Vec::new(),
            publish_times: Vec::new(),
        }
    }
}

fn text(s: &[u8]) -> String {
    String::from_utf8(s.to_vec()).expect("ASCII")
}

impl TestPort {
    pub fn with_code(&self, c: EventCode) -> Vec<Event> {
        self.events
            .iter()
            .filter(|e| e.code == c)
            .cloned()
            .collect()
    }

    pub fn has(&self, c: EventCode) -> bool {
        self.events.iter().any(|e| e.code == c)
    }
}

impl StmSessionPort for TestPort {
    fn log_event(&mut self, e: &Event) {
        self.events.push(e.clone());
    }

    fn pulse_reset(&mut self, now_ms: u32) -> u32 {
        self.pulses.push(now_ms);
        self.stm.reset(u64::from(now_ms.wrapping_add(100)));
        now_ms.wrapping_add(100)
    }

    fn publish(&mut self, s: &StmSnapshot) {
        self.last.clone_from(s);
        self.publishes += 1;
        self.publish_times.push(s.taken_ms);
    }

    fn store_profile(&mut self, p: &Profile) {
        self.profiles.push(*p);
    }

    fn request_last_good_copy(&mut self, image: &[u8]) {
        self.last_good.push(text(image));
    }

    fn restart_pending(&mut self) -> bool {
        self.restart
    }

    fn mark_flash_active(&mut self) {
        self.flash_marks += 1;
    }

    fn store_desired_targets(&mut self, t: &PersistedTargets) {
        self.targets.push(*t);
    }

    fn post_scheduled_calib_result(&mut self, attempt: u16, ok: bool, reason: CalibFailure) {
        self.calibs.push(Calib {
            attempt,
            ok,
            reason,
        });
    }

    fn set_stm_save_state(&mut self, s: StmSaveState) {
        self.saves.push(s);
    }

    fn store_lease_record(&mut self, s: &LeaseClientSnapshot) {
        self.lease = *s;
        self.lease_records += 1;
    }

    fn open_image(&mut self, name: &[u8]) -> bool {
        self.opened.push(text(name));
        self.image_ok
    }

    fn image(&mut self) -> &mut dyn FlashImage {
        &mut self.image
    }

    fn close_image(&mut self) {
        self.closed += 1;
    }
}

pub type Session = StmSession<TestPort>;

/// The session, its port and STM, and the stm_link task loop.
pub struct Rig {
    pub now: u32,
    pub sim: SimStm,
    pub s: Session,
    pub cfg: Config,
    pub trusted: bool,
    pub reg: RegulatorInput,
    pub save: StmSaveState,
    pub replies: VecDeque<(u32, String)>,
    pub last_second: u32,
    pub reply_delay_ms: u32,
    /// replies as UART bytes (on_rx, CR LF) instead of on_line()
    pub via_rx: bool,
}

impl Rig {
    /// C++ `Rig(protocol = 3, active = 0x00F)`.
    pub fn new(protocol: u8, active: u16) -> Self {
        let now = 1000;
        let mut port = TestPort::default();
        port.stm.protocol = protocol;
        let mut cfg = Config::default();
        set_defaults(&mut cfg);
        for (v, c) in (0u16..).zip(cfg.valves.iter_mut()) {
            c.active = (active >> v) & 1 != 0;
        }
        Self {
            now,
            sim: SimStm::new(now),
            s: StmSession::new(port),
            cfg,
            trusted: true,
            reg: RegulatorInput::default(),
            save: StmSaveState::Idle,
            replies: VecDeque::new(),
            last_second: 0,
            reply_delay_ms: 3,
            via_rx: false,
        }
    }

    pub fn port(&self) -> &TestPort {
        self.s.port()
    }

    pub fn port_mut(&mut self) -> &mut TestPort {
        self.s.port_mut()
    }

    pub fn stm(&self) -> &LineStm {
        &self.s.port().stm
    }

    pub fn stm_mut(&mut self) -> &mut LineStm {
        &mut self.s.port_mut().stm
    }

    /// C++ `start()` with its defaults: no targets, no lease record.
    pub fn start(&mut self) {
        self.start_with(&PersistedTargets::default(), RestoreSource::None, None);
    }

    pub fn start_with(
        &mut self,
        t: &PersistedTargets,
        src: RestoreSource,
        lease: Option<&LeaseClientSnapshot>,
    ) {
        self.s.apply_config(&self.cfg, self.trusted);
        self.s.begin(self.now, t, src, lease);
        self.last_second = self.now;
    }

    pub fn command(&mut self, c: &StmCommand) {
        self.s.handle_command(c, self.now);
    }

    pub fn step(&mut self) {
        self.now = self.now.wrapping_add(2);
        while let Some((at, _)) = self.replies.front() {
            if *at > self.now {
                break;
            }
            let Some((_, r)) = self.replies.pop_front() else {
                break;
            };
            if self.via_rx {
                let bytes = r + "\r\n";
                self.s.on_rx(bytes.as_bytes(), self.now);
            } else {
                self.s.on_line(r.as_bytes(), self.now);
            }
        }
        if self.s.flashing() {
            self.sim.now = self.now;
            self.s.flash_step(&mut self.sim, self.now);
        } else {
            self.s.poll(self.now);
            if let Some(line) = self.s.next_to_send(self.now) {
                let mut text = text(&line.text);
                while text.ends_with(['\n', '\r', ' ']) {
                    text.pop();
                }
                self.s.on_sent(self.now);
                let now = self.now;
                let r = self.stm_mut().answer(&text, u64::from(now));
                if !r.is_empty() {
                    self.replies
                        .push_back((now.wrapping_add(self.reply_delay_ms), r));
                }
            }
        }
        if self.now.wrapping_sub(self.last_second) >= 1000 {
            self.last_second = self.now;
            self.s.every_second(self.now, &self.reg, self.save);
        }
        self.s.publish_if_due(self.now);
    }

    pub fn run(&mut self, ms: u32) {
        let end = self.now.wrapping_add(ms);
        while (end.wrapping_sub(self.now) as i32) > 0 {
            self.step();
        }
    }

    /// Runs until `cond` or `ms` passed; returns cond().
    pub fn run_until(&mut self, mut cond: impl FnMut(&Rig) -> bool, ms: u32) -> bool {
        let end = self.now.wrapping_add(ms);
        while !cond(self) && (end.wrapping_sub(self.now) as i32) > 0 {
            self.step();
        }
        cond(self)
    }

    pub fn count(&self, cmd: &str) -> usize {
        self.stm().lines_of(cmd).len()
    }

    pub fn target(&self, v: u8, pos: u8, src: TargetSource) -> StmCommand {
        StmCommand {
            kind: StmCommandType::SetTarget,
            valve: v,
            pos,
            source: src,
            ..StmCommand::default()
        }
    }
}

/// C++ `cmd(t, valve = kNoValve)`.
pub fn cmd(t: StmCommandType, valve: u8) -> StmCommand {
    StmCommand {
        kind: t,
        valve,
        ..StmCommand::default()
    }
}

/// C++ `cmd(t)`.
pub fn cmd0(t: StmCommandType) -> StmCommand {
    cmd(t, NO_VALVE)
}

/// An image name or board for a command (C++ `memcpy`/`copyString` into the char array).
pub fn name<const N: usize>(s: &str) -> Text<N> {
    Text::from_slice(s.as_bytes()).expect("fits")
}
