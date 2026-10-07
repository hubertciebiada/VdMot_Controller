//! Test bench of the system suites (port of the C++ glue_system harness: `system_uart.h`, the
//! runner hooks, the fake board and the valve sim): the whole glue on the fake board, booted
//! like the firmware boots it, a case of several boots across resets and power cycles.
//!
//! The persistent stores of a case (the 24LC64 bytes, the no-init cells, the reset kind) live in
//! [`Case`]; every boot builds new "firmware statics" ([`Statics`], leaked for the boot, as the
//! firmware's are `'static`) and a new [`Controller`] from them (design §7.2). A boot:
//! 1. the reset capture of the boot stage on the no-init cells (`vdm_stm_boot::capture_reset`),
//!    the valve outputs safe (`valve_pins_safe`);
//! 2. the time of the C++ boot window (3011 ms: 10 ms drop and 3001 ticks of 1 ms), then the
//!    IWDG with 8 s (the firmware starts it before the glue);
//! 3. `setup_system()`, then the valve sim (C++ `rig.install()`).
//!
//! Every millisecond boundary runs, as the C++ fake: the valve sim (pulses on REVIN through
//! EXTI4), TIM1 (`timer_handler0`), TIM2 (`valve_loop`), the sim's tracking, the watchdog check.
//! The transcript ([`super::golden`]) records USART1 in and out and USART6 out with the time of
//! the main loop pass (or set-up) that wrote them.

use std::boxed::Box;
use std::cell::{Cell, Ref, RefCell, RefMut};
use std::rc::Rc;
use std::string::String;
use std::vec::Vec;

use vdm_stm_boot::capture::{
    capture_reset, read_guard, CSR_BORRSTF, CSR_IWDGRSTF, CSR_PINRSTF, CSR_PORRSTF, CSR_SFTRSTF,
};

use vdm_stm_boot::fault_record::FaultRecord;
use vdm_stm_core::motor_params::MotorParams;
use vdm_stm_core::valve_codes::{ST_IDLE, ST_PRESENT};

use super::golden::{self, BootRecord, Dir, Event, Reset, Transcript};
use super::{Controller, Hardware, I2cBus, MotorLock, Platform, Shared};
use crate::app::{App, AppEnv, ValveV3Info};
use crate::board::BoardRev;
use crate::communication::FirmwareId;
use crate::eeprom24::EepromDevice;
use crate::hal::{
    Clock, ControlTimer, In, NoinitStore, Out, Pins, RevIrq, System, Watchdog, NOINIT_SIZE,
};
use crate::i2c_bus::{self, Wire};
use crate::motor::{timer_handler0, valve_loop, valve_pins_safe, IsrFlags, MotorShared, Pulse};
use crate::serial::{Port, PortSerial, Tx, UsartTx, SR_TC, SR_TXE};
use crate::sysstat::Sysstat;
use crate::test_support::eeprom_fake::{FakeEeprom, EEPROM_SIZE};
use crate::test_support::fake_board::{
    quiet_expected_panics, Ev, FakeBoard, FakeTimer, FakeWatchdog, SystemReset, WatchdogReset,
};
use crate::test_support::ow_fakes::FakeOwBus;
use crate::test_support::stub_log::CallLog;
use crate::test_support::valve_sim::{FixedAdc, Rig, SimAdc};

/// The C++ glue_system builds with FIRMWARE_VERSION "2.1.7-revamped", FIRMWARE_BUILD "1", C2: the
/// goldens hold its gvers reply (the Rust images report the workspace version, read from their
/// ID block).
pub const CPP_ID: FirmwareId = FirmwareId {
    version: b"2.1.7-revamped",
    tag: b"C2",
    build: b"1",
};

/// The time the C++ fake's boot window takes: `BootSetup()` 10 ms, 3001 calls of `BootLoop()`
/// with `delay(1)`.
pub const BOOT_WINDOW_US: u64 = 3_011_000;
/// `IWatchdog.begin(8000000)`
const WATCHDOG_US: u32 = 8_000_000;

/// RCC_CSR of a reset (runner_hooks.cpp resetFlags(): the firmware cleared the flags before).
fn csr_of(reset: Reset) -> u32 {
    match reset {
        Reset::PowerOn => CSR_PORRSTF + CSR_PINRSTF + CSR_BORRSTF,
        Reset::Pin => CSR_PINRSTF,
        Reset::Software => CSR_SFTRSTF + CSR_PINRSTF,
        Reset::Watchdog => CSR_IWDGRSTF + CSR_PINRSTF,
    }
}

/// The motor state with the lock of the firmware's `IsrCell`: no time may pass inside, so the
/// mask (TIM1 and TIM2 on the device, every interrupt in the C++) is never observable.
pub struct BenchMotor {
    pub cell: RefCell<MotorShared>,
    board: FakeBoard,
    locked: Cell<bool>,
}

impl MotorLock for BenchMotor {
    fn lock<R>(&self, f: impl FnOnce(&mut MotorShared) -> R) -> R {
        struct Unlock<'a>(&'a Cell<bool>);
        impl Drop for Unlock<'_> {
            fn drop(&mut self) {
                self.0.set(false);
            }
        }
        assert!(!self.locked.get(), "nested motor lock");
        self.locked.set(true);
        let _unlock = Unlock(&self.locked);
        let t0 = self.board.now_us();
        let r = f(&mut self.cell.borrow_mut());
        assert_eq!(self.board.now_us(), t0, "time passed inside the motor lock");
        r
    }
}

/// The firmware statics and the fakes of one boot.
pub struct Statics {
    pub board: FakeBoard,
    pub motor: BenchMotor,
    pub pulse: Pulse,
    pub flags: IsrFlags,
    pub esp: Port,
    pub dbg: Port,
    /// what USART1 sent and the test did not take yet (C++ `Serial1.txLog`)
    pub esp_out: RefCell<Vec<u8>>,
    /// USART6 output not yet in the transcript
    pub dbg_out: RefCell<Vec<u8>>,
    pub tim1: RefCell<FakeTimer>,
    pub tim2: RefCell<FakeTimer>,
    pub dog: RefCell<FakeWatchdog>,
    pub noinit: RefCell<[u8; NOINIT_SIZE]>,
    pub eeprom: RefCell<FakeEeprom>,
    pub sim: RefCell<Rig>,
    pub sim_installed: Cell<bool>,
    pub log: Rc<RefCell<CallLog>>,
}

/// The board as a copyable handle.
#[derive(Clone, Copy)]
pub struct BenchBoard(pub &'static FakeBoard);

impl Pins for BenchBoard {
    fn set(&self, pin: Out, high: bool) {
        self.0.set(pin, high);
    }

    fn latch(&self, pin: Out) -> bool {
        self.0.latch(pin)
    }

    fn read(&self, pin: In) -> bool {
        Pins::read(self.0, pin)
    }
}

impl RevIrq for BenchBoard {
    fn attach(&self) {
        self.0.attach();
    }

    fn detach(&self) {
        self.0.detach();
    }
}

impl Clock for BenchBoard {
    fn millis(&self) -> u32 {
        self.0.millis()
    }

    fn micros(&self) -> u32 {
        self.0.micros()
    }

    fn delay_ms(&self, ms: u32) {
        self.0.delay_ms(ms);
    }

    fn delay_us(&self, us: u32) {
        self.0.delay_us(us);
    }
}

impl System for BenchBoard {
    fn reset(&self) -> ! {
        self.0.reset()
    }

    fn dev_id(&self) -> u16 {
        self.0.dev_id()
    }
}

/// A USART whose bytes leave at once (C++ fake: the TX log).
#[derive(Clone, Copy)]
pub struct BenchUsart {
    port: &'static Port,
    out: &'static RefCell<Vec<u8>>,
}

impl UsartTx for BenchUsart {
    fn start(&self) {
        while let Tx::Send(b) = self.port.on_irq(SR_TXE, 0) {
            self.out.borrow_mut().push(b);
        }
    }

    fn masked(&self) -> bool {
        false
    }

    fn sr(&self) -> u32 {
        SR_TXE + SR_TC
    }

    fn send(&self, byte: u8) {
        self.out.borrow_mut().push(byte);
    }
}

#[derive(Clone, Copy)]
pub struct BenchNoinit(&'static RefCell<[u8; NOINIT_SIZE]>);

impl NoinitStore for BenchNoinit {
    fn read(&self) -> [u8; NOINIT_SIZE] {
        *self.0.borrow()
    }

    fn write(&mut self, image: &[u8; NOINIT_SIZE]) {
        *self.0.borrow_mut() = *image;
    }
}

pub struct BenchWatchdog(&'static Statics);

impl Watchdog for BenchWatchdog {
    fn reload(&mut self) {
        let s = self.0;
        if s.dog.borrow_mut().reload_at(s.board.now_us()) {
            s.board.record(Ev::Reload);
        }
    }
}

pub struct BenchTimer {
    timer: &'static RefCell<FakeTimer>,
    board: &'static FakeBoard,
}

impl ControlTimer for BenchTimer {
    fn attach_interval(&mut self, interval_us: u32) -> bool {
        self.timer
            .borrow_mut()
            .attach_at(self.board.now_us(), interval_us)
    }
}

pub struct BenchEeprom(&'static RefCell<FakeEeprom>);

impl EepromDevice for BenchEeprom {
    fn write_block(&mut self, memory_address: u16, buffer: &[u8]) -> i32 {
        self.0.borrow_mut().write_block(memory_address, buffer)
    }

    fn read_block(&mut self, memory_address: u16, buffer: &mut [u8]) -> u16 {
        self.0.borrow_mut().read_block(memory_address, buffer)
    }
}

/// i2c_bus.cpp on the fake board's I2C lines (the C++ glue_system runs the real one).
pub struct BenchI2c(FakeBoard);

impl I2cBus for BenchI2c {
    fn recover(&mut self) {
        let clock = self.0.clone();
        i2c_bus::recover(&mut self.0, &clock);
    }

    fn begin(&mut self) {
        Wire::begin(&mut self.0);
    }

    fn restart(&mut self) {
        let clock = self.0.clone();
        i2c_bus::restart(&mut self.0, &clock);
    }
}

pub struct Bench;

impl Platform for Bench {
    type Board = BenchBoard;
    type Serial = PortSerial<'static, BenchUsart>;
    type Noinit = BenchNoinit;
    type Watchdog = BenchWatchdog;
    type Timer = BenchTimer;
    type Eeprom = BenchEeprom;
    type I2c = BenchI2c;
    type OneWire = FakeOwBus;
    type Motor = BenchMotor;
}

/// The interrupts of one millisecond boundary (C++ `millisecondTick()` and the valve sim).
fn on_ms(s: &'static Statics) {
    let board = BenchBoard(&s.board);
    let installed = s.sim_installed.get();
    if installed {
        s.sim.borrow_mut().on_ms(&s.board, &mut || {
            s.board.fire_exti(|| s.pulse.on_edge(&board))
        });
    }
    let now = s.board.now_us();
    let tim1 = s.tim1.borrow_mut().due(now);
    let tim2 = s.tim2.borrow_mut().due(now);
    assert!(
        !(s.motor.locked.get() && (tim1 || tim2)),
        "a timer interrupt inside the motor lock"
    );
    if tim1 {
        let counts = if installed {
            s.sim.borrow().adc_counts()
        } else {
            0
        };
        let mut adc = SimAdc(counts);
        let mut fixed = FixedAdc {
            current: 0,
            reference: 0,
        };
        let m = &mut s.motor.cell.borrow_mut();
        if installed {
            timer_handler0(m, &s.pulse, &mut adc, &board);
        } else {
            // C++ analogRead() of a pin without the sim: 0
            timer_handler0(m, &s.pulse, &mut fixed, &board);
        }
    }
    if tim2 {
        valve_loop(&mut s.motor.cell.borrow_mut(), &s.pulse, &s.flags, &board);
    }
    if installed {
        let ms = s.sim.borrow().ms;
        if ms.is_multiple_of(10) && !s.tim2.borrow().running() {
            valve_loop(&mut s.motor.cell.borrow_mut(), &s.pulse, &s.flags, &board);
        }
        let m = s.motor.cell.borrow();
        s.sim
            .borrow_mut()
            .track(m.valve_getstate() as u8, |v| m.mots[v].status);
    }
    s.dog.borrow_mut().check(now);
}

/// How a boot sets the valve sim up (the two `bootController()` of the C++ suites).
#[derive(Clone, Copy)]
pub struct BootOpts {
    /// valves without a motor (bit v)
    pub absent: u16,
    /// motor speed 2 pulses per ms (system_uart.h, test_system_motion.cpp), else the sim's 0.2
    pub fast: bool,
}

/// test_system_boot.cpp, test_system_io.cpp: the sim as it comes
pub const PLAIN: BootOpts = BootOpts {
    absent: 0,
    fast: false,
};

/// system_uart.h `bootController(rig, absent)`
pub fn sim(absent: u16) -> BootOpts {
    BootOpts { absent, fast: true }
}

/// One test case: the persistent stores and the transcript over its boots.
pub struct Case {
    slug: &'static str,
    rev: BoardRev,
    id: FirmwareId,
    fault: Option<FaultRecord>,
    eeprom: Vec<u8>,
    noinit: [u8; NOINIT_SIZE],
    next: Reset,
    boots: u32,
    transcript: Transcript,
}

impl Case {
    /// A new controller (erased EEPROM, power-on); `slug` names the golden, `name` is the C++
    /// test case.
    pub fn new(slug: &'static str, name: &str) -> Self {
        quiet_expected_panics();
        Case {
            slug,
            rev: BoardRev::C2,
            id: CPP_ID,
            fault: None,
            eeprom: std::vec![0xFF; EEPROM_SIZE],
            noinit: [0xA5; NOINIT_SIZE],
            next: Reset::PowerOn,
            boots: 0,
            transcript: Transcript {
                case: String::from(name),
                boots: Vec::new(),
            },
        }
    }

    /// Index of the next boot (testkit::boot()).
    pub fn boot_index(&self) -> u32 {
        self.boots
    }

    /// From reset to the main loop; `before` runs on the fakes of this boot first (C++
    /// `bootController(rig, absent, before)`).
    pub fn boot(&mut self, opts: BootOpts, before: impl FnOnce(&Statics)) -> Boot<'_> {
        let rev = self.rev;
        // the runner hook of a power-on: the no-init RAM comes up with 0xA5
        if self.next == Reset::PowerOn {
            self.noinit = [0xA5; NOINIT_SIZE];
        }
        let board = FakeBoard::new();
        let mut eeprom = FakeEeprom::default();
        eeprom.bytes.copy_from_slice(&self.eeprom);
        let s: &'static Statics = Box::leak(Box::new(Statics {
            board: board.clone(),
            motor: BenchMotor {
                cell: RefCell::new(MotorShared::new(rev)),
                board: board.clone(),
                locked: Cell::new(false),
            },
            pulse: Pulse::new(),
            flags: IsrFlags::new(),
            esp: Port::new(),
            dbg: Port::new(),
            esp_out: RefCell::new(Vec::new()),
            dbg_out: RefCell::new(Vec::new()),
            tim1: RefCell::new(FakeTimer::default()),
            tim2: RefCell::new(FakeTimer::default()),
            dog: RefCell::new(FakeWatchdog::default()),
            noinit: RefCell::new(self.noinit),
            eeprom: RefCell::new(eeprom),
            sim: RefCell::new(Rig::new(rev)),
            sim_installed: Cell::new(false),
            log: Rc::new(RefCell::new(CallLog::default())),
        }));
        before(s);
        {
            let mut sim = s.sim.borrow_mut();
            for (v, valve) in sim.valve.iter_mut().enumerate() {
                if opts.fast {
                    valve.connected = opts.absent & (1 << v) == 0;
                    valve.pulses_per_ms = 2.0;
                }
            }
        }
        s.board.set_on_ms(Some(Box::new(move || on_ms(s))));

        // the boot stage: the capture, the valve outputs, the window
        let reset = self.next;
        let mut cells = *s.noinit.borrow();
        let info = capture_reset(csr_of(reset), &mut cells);
        *s.noinit.borrow_mut() = cells;
        valve_pins_safe(&BenchBoard(&s.board));
        s.board.set_now_us(BOOT_WINDOW_US);
        s.dog.borrow_mut().begin(s.board.now_us(), WATCHDOG_US);
        let sysstat = Sysstat::new(info.reason, info.resets, info.safe_mode, read_guard(&cells));

        let hw = Hardware::<Bench> {
            board: BenchBoard(&s.board),
            esp: PortSerial {
                port: &s.esp,
                usart: BenchUsart {
                    port: &s.esp,
                    out: &s.esp_out,
                },
            },
            dbg: PortSerial {
                port: &s.dbg,
                usart: BenchUsart {
                    port: &s.dbg,
                    out: &s.dbg_out,
                },
            },
            noinit: BenchNoinit(&s.noinit),
            watchdog: BenchWatchdog(s),
            tim1: BenchTimer {
                timer: &s.tim1,
                board: &s.board,
            },
            tim2: BenchTimer {
                timer: &s.tim2,
                board: &s.board,
            },
            eeprom: BenchEeprom(&s.eeprom),
            i2c: BenchI2c(s.board.clone()),
            one_wire: FakeOwBus::new(s.log.clone()),
        };
        let mut ctl = Controller::new(
            hw,
            Shared {
                motor: &s.motor,
                flags: &s.flags,
                esp_port: &s.esp,
            },
            sysstat,
            self.id,
            rev,
        );
        ctl.set_fault_record(self.fault);
        let index = self.boots;
        let mut boot = Boot {
            s,
            ctl,
            case: self,
            index,
            reset,
            events: Vec::new(),
            esp_seen: 0,
            dbg_mark: 0,
        };
        boot.observe(|b| b.ctl.setup());
        let statuses = core::array::from_fn(|v| s.motor.cell.borrow().mots[v].status);
        let state = s.motor.cell.borrow().valve_getstate() as u8;
        s.sim.borrow_mut().install(state, statuses);
        s.sim_installed.set(true);
        boot
    }

    /// Compares the transcript of every boot with the golden of the C++ case.
    pub fn check_golden(&self) {
        let golden = golden::load(self.slug);
        assert!(
            golden.boots.iter().any(|b| !b.events.is_empty()),
            "{}: an empty golden",
            self.slug
        );
        if let Some(d) = golden::diff(&golden, &self.transcript) {
            panic!("{} differs from the C++ golden: {d}", self.slug);
        }
    }

    /// The firmware identity the next boots report (the C++ glue_system's by default).
    pub fn set_id(&mut self, id: FirmwareId) {
        self.id = id;
    }

    /// The fault record the firmware hands to the next boots (none by default).
    pub fn set_fault_record(&mut self, record: Option<FaultRecord>) {
        self.fault = record;
    }
}

/// One boot of a case: the controller, its statics, the transcript of the boot.
pub struct Boot<'c> {
    pub s: &'static Statics,
    pub ctl: Controller<'static, Bench>,
    case: &'c mut Case,
    index: u32,
    reset: Reset,
    events: Vec<Event>,
    /// bytes of esp_out already in the transcript
    esp_seen: usize,
    /// events already read by take_dbg()
    dbg_mark: usize,
}

impl Boot<'_> {
    fn now_ms(&self) -> u32 {
        (self.s.board.now_us() / 1000) as u32
    }

    fn event(&mut self, ms: u32, dir: Dir, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        self.events.push(Event {
            ms,
            dir,
            bytes: bytes.to_vec(),
        });
    }

    /// What the firmware wrote since the last call (recorder.cpp flushPorts()).
    fn flush_ports(&mut self, ms: u32) {
        let s = self.s;
        let tx = s.esp_out.borrow().clone();
        if tx.len() < self.esp_seen {
            self.esp_seen = 0;
        }
        if tx.len() > self.esp_seen {
            self.event(ms, Dir::Tx, &tx[self.esp_seen..]);
            self.esp_seen = tx.len();
        }
        let dbg: Vec<u8> = s.dbg_out.borrow_mut().drain(..).collect();
        self.event(ms, Dir::Dbg, &dbg);
    }

    /// A call of the firmware: what it wrote, at the time it started (also when it ends in a
    /// reset).
    fn observe<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        let ms = self.now_ms();
        self.flush_ports(ms);
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(self)));
        self.flush_ports(ms);
        match r {
            Ok(v) => v,
            Err(payload) => std::panic::resume_unwind(payload),
        }
    }

    /// One `loop_system()` per millisecond (system_uart.h `runMain`).
    pub fn run_main(&mut self, ms: u32) {
        for _ in 0..ms {
            self.observe(|b| b.ctl.step());
            self.s.board.advance_ms(1);
        }
    }

    /// `runMainUntil`: false after `max_ms`.
    pub fn run_main_until(&mut self, done: impl Fn(&Self) -> bool, max_ms: u32) -> bool {
        for _ in 0..max_ms {
            if done(self) {
                return true;
            }
            self.run_main(1);
        }
        done(self)
    }

    /// `s` seconds pass without a valve_loop tick, then 1.1 s of main loop (`jumpS`).
    pub fn jump_s(&mut self, s: u32) {
        let now = self.s.board.now_us() + u64::from(s) * 1_000_000;
        self.s.board.set_now_us(now);
        self.s.dog.borrow_mut().last_reload_us = now;
        self.run_main(1100);
    }

    /// Bytes from the ESP (`fake::inject(Serial1, ...)`).
    pub fn inject(&mut self, line: &str) {
        let ms = self.now_ms();
        self.flush_ports(ms);
        self.event(ms, Dir::Rx, line.as_bytes());
        for &b in line.as_bytes() {
            // the receive interrupt; TX never waits on the bench
            self.s.esp.on_irq(crate::serial::SR_RXNE, b);
        }
    }

    /// `fake::takeTx(Serial1)`
    pub fn take_tx(&mut self) -> String {
        let out = String::from_utf8_lossy(&self.s.esp_out.borrow()).into_owned();
        self.s.esp_out.borrow_mut().clear();
        out
    }

    /// `fake::tx(Serial1)`
    pub fn tx(&self) -> String {
        String::from_utf8_lossy(&self.s.esp_out.borrow()).into_owned()
    }

    /// A request and the reply within 50 ms (`exchange`).
    pub fn exchange(&mut self, line: &str) -> String {
        self.take_tx();
        self.inject(line);
        self.run_main(50);
        self.take_tx()
    }

    /// Value n (1-based, after the command word) of a reply, -1 if missing (`field`).
    pub fn field(reply: &str, n: usize) -> i64 {
        reply
            .split_whitespace()
            .skip(1)
            .nth(n - 1)
            .and_then(|w| w.parse().ok())
            .unwrap_or(-1)
    }

    pub fn gstax(&mut self, n: usize) -> i64 {
        let r = self.exchange("gstax\n");
        Self::field(&r, n)
    }

    pub fn gvlvx(&mut self, valve: u32, n: usize) -> i64 {
        let r = self.exchange(&std::format!("gvlvx {valve}\n"));
        Self::field(&r, n)
    }

    pub fn gvlvy(&mut self, valve: u32, n: usize) -> i64 {
        let r = self.exchange(&std::format!("gvlvy {valve}\n"));
        Self::field(&r, n)
    }

    /// The motor state (C++ `myvalvemots[]`, `myvalves[]`, ...).
    pub fn m(&self) -> Ref<'_, MotorShared> {
        self.s.motor.cell.borrow()
    }

    pub fn m_mut(&self) -> RefMut<'_, MotorShared> {
        self.s.motor.cell.borrow_mut()
    }

    pub fn sim(&self) -> RefMut<'_, Rig> {
        self.s.sim.borrow_mut()
    }

    /// `valve_idle()`
    pub fn idle(&self) -> bool {
        self.m().valve_idle()
    }

    /// `testkit::lastReset()`
    pub fn last_reset(&self) -> Reset {
        self.reset
    }

    /// The fake 24LC64 (C++ `fake::eeprom`).
    pub fn eeprom(&self) -> RefMut<'_, FakeEeprom> {
        self.s.eeprom.borrow_mut()
    }

    /// app.cpp's state (C++ calls of app functions without an environment).
    pub fn app(&mut self) -> &mut App {
        &mut self.ctl.modules.app
    }

    /// `app_get_valve_v3(valve, info)`
    pub fn app_get_valve_v3(&mut self, valve: u16) -> ValveV3Info {
        self.ctl.modules.with_app(|app, m, env| {
            let mut out = ValveV3Info::default();
            app.app_get_valve_v3(m, env, valve, &mut out);
            out
        })
    }

    /// `app_learn_pending(valve, status, calibration)`
    pub fn app_learn_pending(&mut self, valve: u16) -> bool {
        self.ctl.modules.with_app(|app, m, _| {
            let mot = m.mots[usize::from(valve)];
            app.app_learn_pending(m, valve, mot.status, mot.calibration)
        })
    }

    /// `appsetaction(cmd, valve, pos)` (force false, flags 0)
    pub fn appsetaction(&mut self, cmd: u8, valve: u32, pos: u8) -> i16 {
        self.ctl
            .modules
            .with_app(|_, m, env| env.appsetaction(m, cmd, valve, pos, false, 0))
    }

    /// `motor_get_params()`
    pub fn motor_get_params(&self) -> MotorParams {
        self.m().motor_get_params()
    }

    /// `resetController()`: the reset command, which answers, waits for the EEPROM and
    /// restarts the controller (software reset)
    pub fn reset_controller(mut self) {
        self.inject("reset\n");
        let reset = self.run(|b| {
            b.run_main(5000);
            panic!("the controller did not reset");
        });
        assert_eq!(reset, Some(Reset::Software));
    }

    /// `calibratedIdle(count)`: valves 0..count-1 idle and calibrated where the presence test
    /// left them (startOnPower 30 %)
    pub fn calibrated_idle(&mut self, count: usize) {
        let mut m = self.m_mut();
        for v in 0..count {
            assert_eq!(m.mots[v].status, ST_PRESENT, "valve {v}");
            m.mots[v].status = ST_IDLE;
            m.mots[v].calibrated = true;
            m.mots[v].scaler = 36;
        }
    }

    /// `idleAt(valve, pct)`
    pub fn idle_at(&self, valve: usize, pct: u8) -> bool {
        let m = self.m();
        m.mots[valve].actual_position == pct && m.mots[valve].status == ST_IDLE && m.valve_idle()
    }
}

/// `glcfg(timeout, fs)`: the reply of glcfg
pub fn glcfg(timeout: u16, fs: &[u8]) -> String {
    let mut s = std::format!("glcfg {timeout}");
    for p in fs {
        s.push_str(&std::format!(" {p}"));
    }
    s + "\r\n"
}

// gvlvx/gvlvy fields (system_uart.h)
pub const K_STATUS: usize = 2;
pub const K_POS: usize = 3;
pub const K_TARGET: usize = 4;
pub const K_MEAN_CUR: usize = 5;
pub const K_OC: usize = 6;
pub const K_CC: usize = 7;
pub const K_CAL_STATE: usize = 11;
pub const K_CMD_REJECTED: usize = 13;
pub const K_LAST_REQ: usize = 15;
pub const K_LAST_CNT: usize = 16;
pub const K_LAST_STOP: usize = 17;
pub const K_LAST_MS: usize = 19;
pub const K_FLAGS: usize = 20;
pub const K_FAULT: usize = 21;
pub const K_DRIVE: usize = 23;
pub const K_RETRY_S: usize = 24;
// gstax fields
pub const K_LEASE: usize = 7;
pub const K_LEASE_TIMEOUT: usize = 10;
pub const K_FAILSAFE_MASK: usize = 11;
pub const K_SAFE_MODE: usize = 12;
pub const K_CFG_FLAGS: usize = 18;

impl Boot<'_> {
    /// The USART6 output since the last call (C++ `fake::takeTx(Serial6)`).
    pub fn take_dbg(&mut self) -> String {
        let mut out = Vec::new();
        for e in &self.events[self.dbg_mark..] {
            if e.dir == Dir::Dbg {
                out.extend_from_slice(&e.bytes);
            }
        }
        self.dbg_mark = self.events.len();
        String::from_utf8_lossy(&out).into_owned()
    }

    /// A line on the debug terminal (USART6) and what the controller printed within 150 ms.
    pub fn terminal(&mut self, line: &str) -> String {
        self.take_dbg();
        for &b in line.as_bytes() {
            self.s.dbg.on_irq(crate::serial::SR_RXNE, b);
        }
        self.run_main(150);
        self.take_dbg()
    }

    /// The first status change of the valve at or after `from` (C++ `firstStatus`), 0 if none.
    pub fn first_status(&self, valve: u8, status: u8, from: u32) -> u32 {
        self.s
            .sim
            .borrow()
            .transitions
            .iter()
            .find(|s| s.ms >= from && s.valve == valve && s.status == status)
            .map_or(0, |s| s.ms)
    }

    /// The end of this boot (recorder.cpp endBoot()): what the firmware wrote since the last
    /// observation, the stores for the next boot, the boot's record.
    fn end(mut self, how: &str, next: Reset) {
        let s = self.s;
        let ms = self.now_ms();
        self.flush_ports(ms);
        s.board.set_on_ms(None);
        let Boot {
            case,
            index,
            reset,
            events,
            ..
        } = self;
        case.eeprom.copy_from_slice(&s.eeprom.borrow().bytes);
        case.noinit = *s.noinit.borrow();
        let record: BootRecord =
            golden::boot_end(index, reset, events, how, &case.eeprom, &case.noinit);
        case.transcript.boots.push(record);
        case.boots += 1;
        case.next = next;
    }

    /// The case ends with this boot.
    pub fn finish(self) {
        self.end("case", Reset::PowerOn);
    }

    /// `testkit::reboot(kind)`: ends the boot; the next boot starts with this reset.
    pub fn reboot(self, kind: Reset) {
        self.end(&std::format!("reboot {}", kind.name()), kind);
    }

    /// `glue::run(fn)`: `f` runs like the device runs code that may reset; a system reset ends
    /// the boot as a software reset, an expired watchdog as a watchdog reset. Returns how it
    /// ended (None: `f` returned).
    pub fn run(mut self, f: impl FnOnce(&mut Self)) -> Option<Reset> {
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&mut self)));
        match r {
            Ok(()) => {
                self.finish();
                None
            }
            Err(payload) if payload.is::<SystemReset>() => {
                self.end("reboot software", Reset::Software);
                Some(Reset::Software)
            }
            Err(payload) if payload.is::<WatchdogReset>() => {
                self.end("reboot watchdog", Reset::Watchdog);
                Some(Reset::Watchdog)
            }
            Err(payload) => std::panic::resume_unwind(payload),
        }
    }
}
