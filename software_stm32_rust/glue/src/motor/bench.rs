//! Test harness of the motor suites (the C++ glue_motor executable): the fake board, the shared
//! motor state, the interrupt atomics, the stubs of the modules motor.cpp calls (app_warm_moving,
//! eep_content) and the valve sim, run millisecond by millisecond in the order of the C++ fake:
//! sim, TIM1, TIM2, then valve_loop() every 10th ms while no timer runs it.

use std::format;
use std::string::String;
use std::vec::Vec;

use vdm_stm_core::config_store::ConfigImage;
use vdm_stm_core::move_classifier::MoveResult;

use crate::board::BoardRev;
use crate::test_support::fake_board::{BoardTimer, FakeBoard, FakeTimer};
use crate::test_support::stub_log::CallLog;
use crate::test_support::valve_sim::{Rig, SimAdc, Step};

use super::{
    timer_handler0, valve_loop, valve_pins_safe, AState, Io, IsrFlags, MotorEnv, MotorShared,
    Pulse, ValveDiag, ValveSnapshot,
};

/// Stubs of app.cpp (`app_warm_moving`) and eeprom.cpp (`eep_content`).
#[derive(Default)]
pub struct StubMotorEnv {
    pub log: CallLog,
    pub eep: ConfigImage,
}

impl MotorEnv for StubMotorEnv {
    fn app_warm_moving(&mut self, valve: u32) {
        self.log.log(format!("app_warm_moving({valve})"));
    }

    fn eep_content(&mut self) -> &mut ConfigImage {
        &mut self.eep
    }
}

pub struct Bench {
    pub board: FakeBoard,
    pub m: MotorShared,
    pub pulse: Pulse,
    pub flags: IsrFlags,
    pub env: StubMotorEnv,
    pub tim1: FakeTimer,
    pub tim2: FakeTimer,
    pub rig: Rig,
    installed: bool,
}

impl Bench {
    pub fn new(rev: BoardRev) -> Self {
        Bench {
            board: FakeBoard::new(),
            m: MotorShared::new(rev),
            pulse: Pulse::new(),
            flags: IsrFlags::new(),
            env: StubMotorEnv::default(),
            tim1: FakeTimer::default(),
            tim2: FakeTimer::default(),
            rig: Rig::new(rev),
            installed: false,
        }
    }

    /// valve_setup() with TIM1 at the time of the board
    pub fn valve_setup(&mut self) -> u8 {
        let mut tim1 = BoardTimer {
            board: &self.board,
            timer: &mut self.tim1,
        };
        self.m.valve_setup(&mut tim1)
    }

    /// Hooks the valve sim into the millisecond steps.
    pub fn install(&mut self) {
        self.installed = true;
        let statuses = core::array::from_fn(|v| self.m.mots[v].status);
        self.rig.install(self.m.valve_getstate() as u8, statuses);
    }

    pub fn run_ms(&mut self, ms: u32) {
        for _ in 0..ms {
            self.step_ms();
        }
    }

    fn step_ms(&mut self) {
        let board = &self.board;
        let m = &mut self.m;
        let pulse = &self.pulse;
        let flags = &self.flags;
        let rig = &mut self.rig;
        let tim1 = &mut self.tim1;
        let tim2 = &mut self.tim2;
        let installed = self.installed;
        board.advance_us_with(1000, &mut || {
            if installed {
                rig.on_ms(board, &mut || board.fire_exti(|| pulse.on_edge(board)));
            }
            let now = board.now_us();
            if tim1.due(now) {
                timer_handler0(m, pulse, &mut SimAdc(rig.adc_counts()), board);
            }
            if tim2.due(now) {
                valve_loop(m, pulse, flags, board);
            }
            if installed {
                if rig.ms.is_multiple_of(10) && !tim2.running() {
                    valve_loop(m, pulse, flags, board);
                }
                rig.track(m.valve_getstate() as u8, |v| m.mots[v].status);
            }
        });
    }

    /// Runs until done() is true (checked before every millisecond); false after max_ms.
    pub fn run_until(&mut self, done: impl Fn(&Bench) -> bool, max_ms: u32) -> bool {
        for _ in 0..max_ms {
            if done(self) {
                return true;
            }
            self.run_ms(1);
        }
        done(self)
    }

    /// One valve_loop() outside the sim (C++ calls of valve_loop() in the tests).
    pub fn valve_loop(&mut self) {
        valve_loop(&mut self.m, &self.pulse, &self.flags, &self.board);
    }

    /// motorcycle() called by a test.
    pub fn motorcycle(&mut self, valve: i32, cmd: u8) -> u8 {
        let io = Io {
            pulse: &self.pulse,
            flags: &self.flags,
            hw: &self.board,
        };
        self.m.motorcycle(&io, valve, cmd)
    }

    pub fn appsetaction(&mut self, cmd: u8, valve: u32, pos: u8) -> i16 {
        self.m
            .appsetaction(&mut self.env, cmd, valve, pos, false, 0)
    }

    pub fn appsetaction_with(
        &mut self,
        cmd: u8,
        valve: u32,
        pos: u8,
        force: bool,
        flags: u8,
    ) -> i16 {
        self.m
            .appsetaction(&mut self.env, cmd, valve, pos, force, flags)
    }

    pub fn appsetservice(&mut self, valve: u32, dir: u8, counts: u16, maxma: u8) -> i16 {
        self.m
            .appsetservice(&mut self.env, valve, dir, counts, maxma)
    }

    pub fn idle(&self) -> bool {
        self.m.valve_idle()
    }

    pub fn state(&self) -> AState {
        self.m.valve_getstate()
    }

    pub fn snapshot(&self, v: u32) -> ValveSnapshot {
        let mut s = ValveSnapshot::default();
        self.m.valve_get_snapshot(v, &mut s);
        s
    }

    pub fn diag(&self, v: u32) -> ValveDiag {
        self.snapshot(v).diag
    }

    pub fn last(&self, v: u32) -> MoveResult {
        self.diag(v).last
    }

    /// A valve that knows its stroke: scaler 36, standing at pct % (sim position pct x 36).
    pub fn place(&mut self, v: usize, pct: u8) {
        let mot = &mut self.m.mots[v];
        mot.scaler = 36;
        mot.opening_count = 3600;
        mot.closing_count = 3600;
        mot.status = vdm_stm_core::valve_codes::ST_IDLE;
        mot.actual_position = pct;
        mot.target_position = pct;
        self.rig.valve[v].position = i32::from(pct) * 36;
    }

    /// A move handed over, until the valve state machine is idle again.
    pub fn run(&mut self, cmd: u8, v: u32, pos: u8, flags: u8) {
        assert_eq!(self.appsetaction_with(cmd, v, pos, false, flags), 0);
        self.run_ms(10);
        assert!(self.run_until(Bench::idle, 200_000));
    }

    pub fn service(&mut self, v: u32, dir: u8, counts: u16, maxma: u8) {
        assert_eq!(self.appsetservice(v, dir, counts, maxma), 0);
        self.run_ms(10);
        assert!(self.run_until(Bench::idle, 200_000));
    }

    pub fn learn(&mut self, v: u32) {
        assert_eq!(self.appsetaction(super::CMD_A_LEARN, v, 0), 0);
        self.run_ms(10);
        let vi = v as usize;
        assert!(self.run_until(
            |b| !b.m.mots[vi].calib_active && b.state() == AState::Idle,
            2_000_000
        ));
    }

    /// ms from now until the valve PSU goes off
    pub fn ms_to_psu_off(&mut self) -> u32 {
        let start = self.rig.ms;
        assert!(self.run_until(|b| !b.rig.powered(&b.board), 40_000));
        self.rig.ms - start
    }
}

/// What setup_system() does for the valve machine: safe pins, valve_setup(); then the first two
/// valve_loop() runs (motor Init, Idle) leave Init. (The 12-bit ADC resolution of the C++ has no
/// Rust form: CurrentAdc always delivers 12 bit.)
pub fn start_valves(rev: BoardRev) -> Bench {
    let mut b = Bench::new(rev);
    valve_pins_safe(&b.board);
    b.valve_setup();
    b.install();
    b.run_ms(20);
    assert_eq!(b.state(), AState::Idle);
    b.rig.transitions.clear();
    b.env.log.clear();
    b
}

/// valvestate changes recorded by the sim, in order ("2 5 6").
pub fn states(rig: &Rig) -> String {
    let parts: Vec<String> = rig
        .transitions
        .iter()
        .filter(|s| s.valve == 255)
        .map(|s| format!("{}", s.valvestate))
        .collect();
    parts.join(" ")
}

pub fn status_change(rig: &Rig, valve: u8, status: u8) -> Option<Step> {
    rig.transitions
        .iter()
        .find(|s| s.valve == valve && s.status == status)
        .copied()
}

pub fn state_step(rig: &Rig, state: AState) -> Option<Step> {
    rig.transitions
        .iter()
        .find(|s| s.valve == 255 && s.valvestate == state as u8)
        .copied()
}

/// Both board revisions: the C++ suite runs as glue_motor (C2) and glue_motor_c1.
pub const REVS: [BoardRev; 2] = [BoardRev::C2, BoardRev::C1];
