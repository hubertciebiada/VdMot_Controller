//! The link seams of main.cpp in the C++ glue_main executable: the stubs of app, communication,
//! eeprom, i2c_bus, motor, owDevices, sysstat and terminal (test/native/glue/stubs) in one
//! [`MainLoopEnv`], with their knobs and call log, on the fake board (pins, clock, watchdog, TIM2)
//! and the debug UART.

use std::format;
use std::string::String;
use std::vec::Vec;

use vdm_stm_core::system_stats::BootReason;

use crate::hal::{Clock, ControlTimer, In, Out, Pins, Watchdog};
use crate::motor::IsrFlags;
use crate::test_support::fake_board::{Ev, FakeBoard, FakeTimer, FakeWatchdog, Stop};
use crate::test_support::stub_log::CallLog;

use super::{MainLoop, MainLoopEnv};

pub struct StubMainEnv {
    pub log: CallLog,
    pub board: FakeBoard,
    /// started by the firmware before the glue runs (design §5.4)
    pub dog: FakeWatchdog,
    pub tim2: FakeTimer,
    /// bytes written to the debug UART (Serial6)
    pub tx: Vec<u8>,
    // stub_sysstat
    pub reason: BootReason,
    pub safe_mode: bool,
    pub uptime: u32,
    /// app_loop() ends the main loop with a [`Stop`] panic at this call (0: never)
    pub app_loop_stop_after: u32,
    app_loop_calls: u32,
}

impl Default for StubMainEnv {
    fn default() -> Self {
        let mut dog = FakeWatchdog::default();
        dog.begin(0, 8_000_000);
        StubMainEnv {
            log: CallLog::default(),
            board: FakeBoard::new(),
            dog,
            tim2: FakeTimer::default(),
            tx: Vec::new(),
            reason: BootReason::Unknown,
            safe_mode: false,
            uptime: 0,
            app_loop_stop_after: 0,
            app_loop_calls: 0,
        }
    }
}

impl StubMainEnv {
    /// The debug output since the last call (C++ `fake::takeTx(Serial6)`).
    pub fn take_tx(&mut self) -> String {
        let text = String::from_utf8(self.tx.clone()).expect("ASCII debug lines");
        self.tx.clear();
        text
    }
}

impl Pins for StubMainEnv {
    fn set(&self, pin: Out, high: bool) {
        self.board.set(pin, high);
    }

    fn latch(&self, pin: Out) -> bool {
        self.board.latch(pin)
    }

    fn read(&self, pin: In) -> bool {
        self.board.read(pin)
    }
}

impl Clock for StubMainEnv {
    fn millis(&self) -> u32 {
        self.board.millis()
    }

    fn micros(&self) -> u32 {
        self.board.micros()
    }

    fn delay_ms(&self, ms: u32) {
        self.board.delay_ms(ms);
    }

    fn delay_us(&self, us: u32) {
        self.board.delay_us(us);
    }
}

impl Watchdog for StubMainEnv {
    fn reload(&mut self) {
        if self.dog.reload_at(self.board.now_us()) {
            self.board.record(Ev::Reload);
        }
    }
}

impl ControlTimer for StubMainEnv {
    fn attach_interval(&mut self, interval_us: u32) -> bool {
        self.tim2.attach_at(self.board.now_us(), interval_us)
    }
}

impl MainLoopEnv for StubMainEnv {
    fn sysstat_boot_reason(&mut self) -> BootReason {
        self.log.log("sysstat_boot_reason()");
        self.reason
    }

    fn sysstat_safe_mode(&mut self) -> bool {
        self.log.log("sysstat_safe_mode()");
        self.safe_mode
    }

    fn sysstat_loop(&mut self) {
        self.log.log("sysstat_loop()");
    }

    fn sysstat_uptime_s(&mut self) -> u32 {
        self.log.log("sysstat_uptime_s()");
        self.uptime
    }

    fn i2c_bus_recover(&mut self) {
        self.log.log("i2c_bus_recover()");
    }

    fn wire_begin(&mut self) {
        self.board.record(Ev::WireBegin);
    }

    fn terminal_init(&mut self) -> i16 {
        self.log.log("Terminal_Init()");
        0
    }

    fn terminal_serve(&mut self) -> i16 {
        self.log.log("Terminal_Serve()");
        -1
    }

    fn terminal_supervise(&mut self) {
        self.log.log("terminal_supervise()");
    }

    fn communication_setup(&mut self) {
        self.log.log("communication_setup()");
    }

    fn communication_loop(&mut self) -> i16 {
        self.log.log("communication_loop()");
        -1
    }

    fn eepromsetup(&mut self) -> i16 {
        self.log.log("eepromsetup()");
        0
    }

    fn eeprom_read_layout(&mut self) -> i16 {
        self.log.log("eeprom_read_layout(&eep_content)");
        0
    }

    fn eepromloop(&mut self) -> i16 {
        self.log.log("eepromloop()");
        0
    }

    fn temperature_setup(&mut self) {
        self.log.log("temperature_setup()");
    }

    fn temperature_loop(&mut self) {
        self.log.log("temperature_loop()");
    }

    fn app_setup(&mut self) -> i16 {
        self.log.log("app_setup()");
        0
    }

    fn app_restore(&mut self) {
        self.log.log("app_restore()");
    }

    fn app_loop(&mut self) -> i16 {
        self.log.log("app_loop()");
        self.app_loop_calls += 1;
        if self.app_loop_stop_after != 0 && self.app_loop_calls >= self.app_loop_stop_after {
            std::panic::panic_any(Stop);
        }
        0
    }

    fn app_warm_save(&mut self) {
        self.log.log("app_warm_save()");
    }

    fn app_1s_tick(&mut self, elapsed_s: u32) {
        self.log.log(format!("app_1s_tick({elapsed_s})"));
    }

    fn app_10s_loop(&mut self, elapsed_s: u32) -> u8 {
        self.log.log(format!("app_10s_loop({elapsed_s})"));
        0
    }

    fn valve_setup(&mut self) -> u8 {
        self.log.log("valve_setup()");
        0
    }

    fn debug(&mut self, text: &[u8]) {
        self.tx.extend_from_slice(text);
    }
}

/// main.cpp with its stubs (C++ glue_main).
pub struct MainBench {
    pub main: MainLoop,
    pub flags: IsrFlags,
    pub env: StubMainEnv,
}

impl MainBench {
    pub fn new() -> Self {
        MainBench {
            main: MainLoop::new(),
            flags: IsrFlags::new(),
            env: StubMainEnv::default(),
        }
    }

    pub fn setup_system(&mut self) {
        self.main.setup_system(&mut self.env);
    }

    /// loop_system() at the fake time ms
    pub fn loop_at(&mut self, ms: u32) {
        self.env.board.set_now_us(u64::from(ms) * 1000);
        self.main.loop_system(&self.flags, &mut self.env);
    }

    /// the stub calls of the branches, without the sysstat_loop() of every pass
    pub fn branch_calls(&self) -> Vec<String> {
        self.env
            .log
            .calls
            .iter()
            .filter(|c| c.as_str() != "sysstat_loop()")
            .cloned()
            .collect()
    }
}
