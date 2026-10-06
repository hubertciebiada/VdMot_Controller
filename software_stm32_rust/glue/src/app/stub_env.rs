//! The link seams of app.cpp in the C++ glue_app executable: the stubs of motor.cpp, eeprom.cpp,
//! owDevices.cpp, sysstat.cpp and terminal.cpp (test/native/glue/stubs) in one [`AppEnv`], with
//! their knobs and call log, the fake clock, the no-init RAM (0xA5 after a power-on) and the
//! debug UART (USART6).

use std::format;
use std::string::String;
use std::vec::Vec;

use vdm_stm_core::calibration::{EscalationConfig, ESCALATION_DEFAULT};
use vdm_stm_core::config_blocks::CalibRecord;
use vdm_stm_core::config_store::{ConfigImage, LEASE_SOURCE_DEFAULT};
use vdm_stm_core::motor_params::MotorParams;
use vdm_stm_core::system_stats::BootReason;

use crate::board::BoardRev;
use crate::hal::{Clock, NoinitStore, System, NOINIT_SIZE};
use crate::motor::{IsrFlags, MotorShared, ValveSnapshot};
use crate::test_support::fake_board::{FakeBoard, FakeNoinit, FakeSystem};
use crate::test_support::stub_log::CallLog;

use super::{App, AppEnv, ValveV3Info, NO_OF_MIN_COUNTS};

pub struct StubAppEnv {
    pub log: CallLog,
    // stub_motor
    pub idle: bool,
    pub action: i16,
    pub service: i16,
    /// appstop()
    pub stop: i16,
    /// valve_busy_index()
    pub busy: i32,
    /// valve_get_snapshot() of valve v
    pub snapshot: [ValveSnapshot; 12],
    pub params: MotorParams,
    pub escalation: EscalationConfig,
    // stub_eeprom
    pub eep: ConfigImage,
    pub cfg_flags: u8,
    pub lease_source: u8,
    pub free: bool,
    /// record of the last eeprom_store_calib()
    pub last_calib: CalibRecord,
    // stub_sysstat
    pub uptime: u32,
    pub reason: BootReason,
    pub safe_mode: bool,
    // stub_owDevices: noOfDS18Devices, tempsensors[].address
    pub ds18_count: u8,
    pub tempsensors: [[u8; 8]; 34],
    // stub_terminal
    pub manual_active: bool,
    // fakes
    pub board: FakeBoard,
    pub noinit: FakeNoinit,
    /// bytes written to the debug UART (Serial6)
    pub tx: Vec<u8>,
}

impl Default for StubAppEnv {
    fn default() -> Self {
        StubAppEnv {
            log: CallLog::default(),
            idle: true,
            action: 0,
            service: 0,
            stop: -1,
            busy: -1,
            snapshot: [ValveSnapshot::default(); 12],
            params: MotorParams {
                low_fac: 17,
                high_fac: 17,
                start_on_power: 50,
                min_counts: NO_OF_MIN_COUNTS,
                max_retries: 0,
            },
            escalation: ESCALATION_DEFAULT,
            eep: ConfigImage::default(),
            cfg_flags: 0,
            lease_source: LEASE_SOURCE_DEFAULT,
            free: true,
            last_calib: CalibRecord::default(),
            uptime: 0,
            reason: BootReason::Unknown,
            safe_mode: false,
            ds18_count: 0,
            tempsensors: [[0; 8]; 34],
            manual_active: false,
            board: FakeBoard::new(),
            noinit: FakeNoinit::default(),
            tx: Vec::new(),
        }
    }
}

impl StubAppEnv {
    /// The debug output since the last call (C++ `fake::takeTx(Serial6)`).
    pub fn take_tx(&mut self) -> String {
        let text = String::from_utf8(self.tx.clone()).expect("ASCII debug lines");
        self.tx.clear();
        text
    }
}

impl Clock for StubAppEnv {
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

impl NoinitStore for StubAppEnv {
    fn read(&self) -> [u8; NOINIT_SIZE] {
        self.noinit.read()
    }

    fn write(&mut self, image: &[u8; NOINIT_SIZE]) {
        self.noinit.write(image);
    }
}

impl System for StubAppEnv {
    fn reset(&self) -> ! {
        FakeSystem.reset()
    }

    fn dev_id(&self) -> u16 {
        FakeSystem.dev_id()
    }
}

impl AppEnv for StubAppEnv {
    fn valve_idle(&mut self, _m: &MotorShared) -> bool {
        self.log.log("valve_idle()");
        self.idle
    }

    fn appsetaction(
        &mut self,
        _m: &mut MotorShared,
        cmd: u8,
        valve: u32,
        pos: u8,
        force: bool,
        flags: u8,
    ) -> i16 {
        let cmd = char::from(cmd);
        let force = u8::from(force);
        if flags != 0 {
            self.log.log(format!(
                "appsetaction({cmd}, {valve}, {pos}, {force}, 0x{flags:02x})"
            ));
        } else {
            self.log
                .log(format!("appsetaction({cmd}, {valve}, {pos}, {force})"));
        }
        self.action
    }

    fn appsetservice(
        &mut self,
        _m: &mut MotorShared,
        valve: u32,
        dir: u8,
        counts: u16,
        maxma: u8,
    ) -> i16 {
        self.log
            .log(format!("appsetservice({valve}, {dir}, {counts}, {maxma})"));
        self.service
    }

    fn appstop(&mut self, _m: &mut MotorShared, valve: u32) -> i16 {
        self.log.log(format!("appstop({valve})"));
        self.stop
    }

    fn valve_busy_index(&mut self, _m: &MotorShared) -> i32 {
        self.log.log("valve_busy_index()");
        self.busy
    }

    fn valve_get_snapshot(&mut self, _m: &MotorShared, valve: u32, out: &mut ValveSnapshot) {
        self.log.log(format!("valve_get_snapshot({valve})"));
        *out = self
            .snapshot
            .get(valve as usize)
            .copied()
            .unwrap_or_default();
    }

    fn motor_set_params(&mut self, _m: &mut MotorShared, p: &MotorParams) {
        self.log.log(format!(
            "motor_set_params({}, {}, {}, {}, {})",
            p.low_fac, p.high_fac, p.start_on_power, p.min_counts, p.max_retries
        ));
        self.params = *p;
    }

    fn motor_set_escalation(&mut self, _m: &mut MotorShared, c: &EscalationConfig) {
        self.log.log(format!(
            "motor_set_escalation({}, {}, {})",
            c.enable, c.step_pct, c.max_ma
        ));
        self.escalation = *c;
    }

    fn eep_content(&mut self) -> &mut ConfigImage {
        &mut self.eep
    }

    fn eeprom_lease_source(&mut self) -> u8 {
        self.log.log("eeprom_lease_source()");
        self.lease_source
    }

    fn eeprom_cfg_flags(&mut self) -> u8 {
        self.log.log("eeprom_cfg_flags()");
        self.cfg_flags
    }

    fn eeprom_store_calib(&mut self, valve: u8, rec: &CalibRecord) {
        self.last_calib = *rec;
        self.log.log(format!("eeprom_store_calib({valve})"));
    }

    fn eeprom_free(&mut self) -> bool {
        self.log.log("eeprom_free()");
        self.free
    }

    fn sysstat_safe_mode(&mut self) -> bool {
        self.log.log("sysstat_safe_mode()");
        self.safe_mode
    }

    fn sysstat_uptime_s(&mut self) -> u32 {
        self.log.log("sysstat_uptime_s()");
        self.uptime
    }

    fn sysstat_boot_reason(&mut self) -> BootReason {
        self.log.log("sysstat_boot_reason()");
        self.reason
    }

    fn ds18_count(&mut self) -> u8 {
        self.ds18_count
    }

    fn ds18_address(&mut self, index: u8) -> [u8; 8] {
        self.tempsensors[usize::from(index)]
    }

    fn print_address(&mut self, a: &[u8; 8]) {
        self.log.log(format!(
            "printAddress({:02x}-{:02x}-{:02x}-{:02x}-{:02x}-{:02x}-{:02x}-{:02x})",
            a[0], a[1], a[2], a[3], a[4], a[5], a[6], a[7]
        ));
    }

    fn terminal_manual_active(&mut self) -> bool {
        self.log.log("terminal_manual_active()");
        self.manual_active
    }

    fn debug(&mut self, text: &[u8]) {
        self.tx.extend_from_slice(text);
    }
}

/// app.cpp with its stubs (C++ glue_app): the app state, the motor data of stub_motor, the
/// interrupt flags and the env.
pub struct AppBench {
    pub app: App,
    pub m: MotorShared,
    pub flags: IsrFlags,
    pub env: StubAppEnv,
}

impl AppBench {
    /// `glue::begin()`: fresh fakes and stubs (the board revision does not matter to app.cpp).
    pub fn new() -> Self {
        AppBench {
            app: App::default(),
            m: MotorShared::new(BoardRev::C2),
            flags: IsrFlags::default(),
            env: StubAppEnv::default(),
        }
    }

    pub fn app_setup(&mut self) -> i16 {
        self.app.app_setup(&mut self.m, &mut self.env)
    }

    pub fn app_load_config(&mut self) {
        self.app.app_load_config(&mut self.m, &mut self.env);
    }

    pub fn app_loop(&mut self) -> i16 {
        self.app.app_loop(&mut self.m, &self.flags, &mut self.env)
    }

    pub fn app_10s_loop(&mut self, elapsed_s: u32) -> u8 {
        self.app.app_10s_loop(&mut self.m, &mut self.env, elapsed_s)
    }

    pub fn app_1s_tick(&mut self, elapsed_s: u32) {
        self.app.app_1s_tick(&mut self.m, elapsed_s);
    }

    pub fn app_restore(&mut self) {
        self.app.app_restore(&mut self.m, &mut self.env);
    }

    pub fn app_warm_save(&mut self) {
        self.app.app_warm_save(&self.m, &mut self.env);
    }

    pub fn app_warm_moving(&mut self, valve: u32) {
        App::app_warm_moving(&mut self.env, valve);
    }

    pub fn app_set_learntime(&mut self, time: u32) -> i16 {
        self.app.app_set_learntime(&mut self.m, time)
    }

    pub fn app_set_learnmovements(&mut self, movements: u16) -> i16 {
        self.app.app_set_learnmovements(&mut self.m, movements)
    }

    pub fn app_set_valvelearning(&mut self, valve: u16) -> i16 {
        self.app.app_set_valvelearning(&mut self.m, valve)
    }

    pub fn app_set_valveopen(&mut self, valve: u16) -> i16 {
        self.app.app_set_valveopen(&mut self.m, valve)
    }

    pub fn app_service_move(&mut self, valve: u16, dir: u8, counts: u16, maxma: u8) -> i16 {
        self.app
            .app_service_move(&mut self.m, &mut self.env, valve, dir, counts, maxma)
    }

    pub fn app_learn_pending(&self, valve: u16, status: u8, calibration: bool) -> bool {
        self.app
            .app_learn_pending(&self.m, valve, status, calibration)
    }

    pub fn app_target_changed(&mut self, valve: u16) {
        self.app.app_target_changed(&mut self.m, valve);
    }

    pub fn app_stop(&mut self, valve: u16) -> i16 {
        self.app.app_stop(&mut self.m, &mut self.env, valve)
    }

    pub fn app_match_sensors(&mut self) -> i16 {
        self.app.app_match_sensors(&mut self.m, &mut self.env)
    }

    pub fn app_scan_valves(&mut self) {
        self.app.app_scan_valves(&mut self.m);
    }

    pub fn app_failsafe_mask(&self) -> u16 {
        self.app.app_failsafe_mask(&self.m)
    }

    pub fn app_temp_age_s(&mut self) -> u32 {
        self.app.app_temp_age_s(&mut self.env)
    }

    pub fn app_temp_cycle_done(&mut self) {
        self.app.app_temp_cycle_done(&mut self.env);
    }

    /// gvlvy values 20..25, from a record that starts with all bits set (C++ `v3()`).
    pub fn v3(&mut self, valve: u16) -> ValveV3Info {
        let mut info = ValveV3Info {
            flags: 0xFFFF,
            fault: 0xFF,
            fs_pct: 0xFF,
            drive: 0xFF,
            retry_s: 0xFFFF_FFFF,
            retries: 0xFF,
        };
        self.app
            .app_get_valve_v3(&self.m, &mut self.env, valve, &mut info);
        info
    }

    /// The appsetaction() calls of `passes` app_loop() passes (C++ `loopActions()`).
    pub fn loop_actions(&mut self, passes: u32) -> Vec<String> {
        self.env.log.clear();
        for _ in 0..passes {
            self.app_loop();
        }
        self.env.log.calls_of("appsetaction")
    }
}
