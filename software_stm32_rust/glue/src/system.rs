//! The whole glue as the firmware runs it (docs/rust/GLUE-DESIGN-STM.md §1.1, §2): the
//! [`Controller`] owns the module states, implements every `<Module>Env` by delegation to the
//! sibling modules (the link of the C++ firmware) and runs `setup_system()` ([`Controller::setup`])
//! and `loop_system()` ([`Controller::step`]) on the hardware of a [`Platform`].
//!
//! The state the interrupts share stays with the firmware (design §2.3): the motor state behind
//! a [`MotorLock`] (TIM1, TIM2), the [`IsrFlags`] atomics (TIM2) and the serial [`Port`]s
//! (USART1, USART6); the controller holds references. A call into a module gets a context that
//! borrows the other modules. Two C++ calls made from inside a module are made by the
//! controller right after the module returns, because they need the module that is still
//! borrowed: `app_load_config()` of an EEPROM re-read (the last call of `eeprom_reread()`, so
//! after `eepromloop()` only the RAM mirror's status changed meanwhile, which `app_load_config()`
//! does not read) and `app_match_sensors()` / `app_temp_cycle_done()` of `temperature_loop()`
//! (the last calls of their branches). The order of every output stays the C++ order.

use vdm_stm_boot::capture::{write_guard, NOINIT_LEN};
use vdm_stm_core::calibration::EscalationConfig;
use vdm_stm_core::config_blocks::CalibRecord;
use vdm_stm_core::config_store::ConfigImage;
use vdm_stm_core::motor_params::MotorParams;
use vdm_stm_core::profile_recorder::ProfileRecorder;
use vdm_stm_core::reset_guard::ResetGuardCell;
use vdm_stm_core::system_stats::BootReason;
use vdm_stm_core::uart_errors::UartErrorCounters;

use crate::app::{App, AppEnv, ValveV3Info};
use crate::board::BoardRev;
#[cfg(feature = "terminal")]
use crate::communication::{
    comm_print_valve_sensor_ids, comm_set_learntime, comm_set_valve_sensor_index,
    comm_set_valve_sensors,
};
use crate::communication::{Communication, CommunicationEnv, FirmwareId, ValveGlobals};
use crate::eeprom::{Eeprom, EepromEnv, EepromLayout};
use crate::eeprom24::EepromDevice;
use crate::hal::{
    Clock, ControlTimer, In, NoinitStore, Out, Pins, RevIrq, Serial, System, Watchdog, NOINIT_SIZE,
};
use crate::main_loop::{MainLoop, MainLoopEnv};
use crate::motor::{IsrFlags, MotorEnv, MotorShared, ValveSnapshot};
use crate::ow_devices::{print_address, OwBus, OwDevices, OwDevicesEnv, OwSensors};
#[cfg(feature = "terminal")]
use crate::print::Print;
use crate::serial::Port;
use crate::sysstat::{Sysstat, SysstatEnv};
#[cfg(feature = "terminal")]
use crate::terminal::{Terminal, TerminalEnv};

use core::sync::atomic::Ordering;

// the no-init region of the glue is the one of the boot stage
const _: () = assert!(NOINIT_SIZE == NOINIT_LEN);

/// The motor state of TIM1 and TIM2 as the main loop reaches it (firmware `IsrCell`).
pub trait MotorLock {
    /// Runs `f` with the TIM1 and TIM2 interrupts masked (C++ `__disable_irq` around the copies
    /// of `myvalvemots[]`; `valve_idle()` sections). No time passes inside.
    fn lock<R>(&self, f: impl FnOnce(&mut MotorShared) -> R) -> R;
}

/// The EEPROM bus besides its transfers (`i2c_bus.cpp`, `Wire`).
pub trait I2cBus {
    /// `i2c_bus_recover()`: frees the bus before the driver starts
    fn recover(&mut self);
    /// `Wire.setSDA(PB7)`, `Wire.setSCL(PB6)`, `Wire.begin()`
    fn begin(&mut self);
    /// `i2c_bus_restart()`: driver off, recovery, driver on (before an EEPROM retry)
    fn restart(&mut self);
}

/// The hardware types of one build: the firmware's registers or the host test bench.
pub trait Platform {
    /// pins, the EXTI line of REVIN, the clock, the system reset and the device id: a handle
    /// every context copies
    type Board: Pins + RevIrq + Clock + System + Copy;
    /// USART1 (ESP) and USART6 (debug terminal)
    type Serial: Serial + Copy;
    /// the no-init RAM cells
    type Noinit: NoinitStore + Copy;
    type Watchdog: Watchdog;
    /// TIM1 (current, 1 ms) and TIM2 (valve state machine, 10 ms)
    type Timer: ControlTimer;
    /// the 24LC64 transfers
    type Eeprom: EepromDevice;
    type I2c: I2cBus;
    /// OneWire, DallasTemperature and DS2438 on PB10
    type OneWire: OwBus;
    type Motor: MotorLock;
}

/// The hardware a controller owns.
pub struct Hardware<P: Platform> {
    pub board: P::Board,
    pub esp: P::Serial,
    pub dbg: P::Serial,
    pub noinit: P::Noinit,
    pub watchdog: P::Watchdog,
    pub tim1: P::Timer,
    pub tim2: P::Timer,
    pub eeprom: P::Eeprom,
    pub i2c: P::I2c,
    pub one_wire: P::OneWire,
}

/// What the controller shares with the interrupts (firmware statics).
pub struct Shared<'a, M> {
    pub motor: &'a M,
    pub flags: &'a IsrFlags,
    /// USART1: the receive error counters of gstax
    pub esp_port: &'a Port,
}

/// The module states and the hardware: the C++ firmware's globals without `loop_system()`'s.
pub struct Modules<'a, P: Platform> {
    pub hw: Hardware<P>,
    pub motor: &'a P::Motor,
    pub flags: &'a IsrFlags,
    pub esp_port: &'a Port,
    pub app: App,
    pub comm: Communication,
    pub eeprom: Eeprom,
    pub ow: OwDevices,
    pub sysstat: Sysstat,
    #[cfg(feature = "terminal")]
    pub term: Terminal,
}

/// The controller: `setup_system()` once, then `loop_system()` for ever.
pub struct Controller<'a, P: Platform> {
    pub main: MainLoop,
    pub modules: Modules<'a, P>,
}

impl<'a, P: Platform> Controller<'a, P> {
    /// The modules with their C++ start values; `sysstat` is the reset capture of the boot
    /// stage, `id` the firmware identity of the ID block, `board` the MUX levels.
    pub fn new(
        hw: Hardware<P>,
        shared: Shared<'a, P::Motor>,
        sysstat: Sysstat,
        id: FirmwareId,
        board: BoardRev,
    ) -> Self {
        #[cfg(not(feature = "terminal"))]
        let _ = board;
        Controller {
            main: MainLoop::new(),
            modules: Modules {
                hw,
                motor: shared.motor,
                flags: shared.flags,
                esp_port: shared.esp_port,
                app: App::new(),
                comm: Communication::new(id),
                eeprom: Eeprom::default(),
                ow: OwDevices::default(),
                sysstat,
                #[cfg(feature = "terminal")]
                term: Terminal::new(id, board.mux_on_high()),
            },
        }
    }

    /// `setup_system()` (the firmware started the IWDG before).
    pub fn setup(&mut self) {
        self.main.setup_system(&mut self.modules);
    }

    /// One pass of `loop_system()`.
    pub fn step(&mut self) {
        let flags = self.modules.flags;
        self.main.step(flags, &mut self.modules);
    }

    /// `setup_system()`, then `loop_system()` for ever.
    pub fn run(&mut self) -> ! {
        let flags = self.modules.flags;
        self.main.run(flags, &mut self.modules)
    }
}

// ---------------------------------------------------------------- contexts

/// motor.cpp's calls out: `app_warm_moving()` (no-init cells) and `eep_content`.
struct MotorCtx<'c, N> {
    noinit: N,
    eep: &'c mut ConfigImage,
}

impl<N: NoinitStore> MotorEnv for MotorCtx<'_, N> {
    fn app_warm_moving(&mut self, valve: u32) {
        App::app_warm_moving(&mut self.noinit, valve);
    }

    fn eep_content(&mut self) -> &mut ConfigImage {
        self.eep
    }
}

/// app.cpp's calls out; the motor state comes with every call.
struct AppCtx<'c, P: Platform> {
    board: P::Board,
    noinit: P::Noinit,
    dbg: P::Serial,
    eeprom: &'c mut Eeprom,
    sysstat: &'c Sysstat,
    sensors: &'c OwSensors,
    /// `terminal_manual_active()` when the context was made
    manual: bool,
}

impl<P: Platform> AppCtx<'_, P> {
    fn motor_env(&mut self) -> MotorCtx<'_, P::Noinit> {
        MotorCtx {
            noinit: self.noinit,
            eep: &mut self.eeprom.eep_content.cfg,
        }
    }
}

impl<P: Platform> Clock for AppCtx<'_, P> {
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

impl<P: Platform> NoinitStore for AppCtx<'_, P> {
    fn read(&self) -> [u8; NOINIT_SIZE] {
        self.noinit.read()
    }

    fn write(&mut self, image: &[u8; NOINIT_SIZE]) {
        self.noinit.write(image);
    }
}

impl<P: Platform> System for AppCtx<'_, P> {
    fn reset(&self) -> ! {
        self.board.reset()
    }

    fn dev_id(&self) -> u16 {
        self.board.dev_id()
    }
}

impl<P: Platform> AppEnv for AppCtx<'_, P> {
    fn valve_idle(&mut self, m: &MotorShared) -> bool {
        m.valve_idle()
    }

    fn appsetaction(
        &mut self,
        m: &mut MotorShared,
        cmd: u8,
        valve: u32,
        pos: u8,
        force: bool,
        flags: u8,
    ) -> i16 {
        m.appsetaction(&mut self.motor_env(), cmd, valve, pos, force, flags)
    }

    fn appsetservice(
        &mut self,
        m: &mut MotorShared,
        valve: u32,
        dir: u8,
        counts: u16,
        maxma: u8,
    ) -> i16 {
        m.appsetservice(&mut self.motor_env(), valve, dir, counts, maxma)
    }

    fn appstop(&mut self, m: &mut MotorShared, valve: u32) -> i16 {
        m.appstop(valve)
    }

    fn valve_busy_index(&mut self, m: &MotorShared) -> i32 {
        m.valve_busy_index()
    }

    fn valve_get_snapshot(&mut self, m: &MotorShared, valve: u32, out: &mut ValveSnapshot) {
        m.valve_get_snapshot(valve, out);
    }

    fn motor_set_params(&mut self, m: &mut MotorShared, params: &MotorParams) {
        m.motor_set_params(&mut self.motor_env(), params);
    }

    fn motor_set_escalation(&mut self, m: &mut MotorShared, config: &EscalationConfig) {
        m.motor_set_escalation(&mut self.motor_env(), config);
    }

    fn eep_content(&mut self) -> &mut ConfigImage {
        &mut self.eeprom.eep_content.cfg
    }

    fn eeprom_lease_source(&mut self) -> u8 {
        self.eeprom.lease_source()
    }

    fn eeprom_cfg_flags(&mut self) -> u8 {
        self.eeprom.cfg_flags()
    }

    fn eeprom_store_calib(&mut self, valve: u8, rec: &CalibRecord) {
        self.eeprom.store_calib(valve, rec);
    }

    fn eeprom_free(&mut self) -> bool {
        self.eeprom.free()
    }

    fn sysstat_safe_mode(&mut self) -> bool {
        self.sysstat.safe_mode()
    }

    fn sysstat_uptime_s(&mut self) -> u32 {
        self.sysstat.uptime_s()
    }

    fn sysstat_boot_reason(&mut self) -> BootReason {
        self.sysstat.boot_reason()
    }

    fn ds18_count(&mut self) -> u8 {
        self.sensors.no_of_ds18_devices
    }

    fn ds18_address(&mut self, index: u8) -> [u8; 8] {
        self.sensors
            .tempsensors
            .get(usize::from(index))
            .map_or([0; 8], |t| t.address)
    }

    fn print_address(&mut self, address: &[u8; 8]) {
        print_address(&mut self.dbg, address);
    }

    fn terminal_manual_active(&mut self) -> bool {
        self.manual
    }

    fn debug(&mut self, text: &[u8]) {
        self.dbg.write(text);
    }
}

/// The calls of communication.cpp and terminal.cpp into the other modules.
struct Ctx<'c, P: Platform> {
    app: &'c mut App,
    eeprom: &'c mut Eeprom,
    ow: &'c mut OwDevices,
    sysstat: &'c mut Sysstat,
    motor: &'c P::Motor,
    flags: &'c IsrFlags,
    esp_port: &'c Port,
    board: P::Board,
    noinit: P::Noinit,
    dbg: P::Serial,
    manual: bool,
}

/// The app context made of the fields of a [`Ctx`] (a macro: the closures of `lock` borrow
/// the fields one by one).
macro_rules! app_ctx {
    ($c:expr) => {
        AppCtx::<P> {
            board: $c.board,
            noinit: $c.noinit,
            dbg: $c.dbg,
            eeprom: &mut *$c.eeprom,
            sysstat: &*$c.sysstat,
            sensors: &$c.ow.sensors,
            manual: $c.manual,
        }
    };
}

impl<P: Platform> CommunicationEnv for Ctx<'_, P> {
    fn app_lease_poll(&mut self) {
        self.app.app_lease_poll();
    }

    fn app_target_changed(&mut self, valve: u16) {
        self.motor.lock(|m| self.app.app_target_changed(m, valve));
    }

    fn app_set_learntime(&mut self, time: u32) -> i16 {
        self.motor.lock(|m| self.app.app_set_learntime(m, time))
    }

    fn app_set_learnmovements(&mut self, cycles: u16) -> i16 {
        self.motor
            .lock(|m| self.app.app_set_learnmovements(m, cycles))
    }

    fn app_set_valveopen(&mut self, valve: u16) -> i16 {
        self.motor.lock(|m| self.app.app_set_valveopen(m, valve))
    }

    fn app_set_valvelearning(&mut self, valve: u16) -> i16 {
        self.motor
            .lock(|m| self.app.app_set_valvelearning(m, valve))
    }

    fn app_scan_valves(&mut self) {
        self.motor.lock(|m| self.app.app_scan_valves(m));
    }

    fn app_match_sensors(&mut self) -> i16 {
        self.motor
            .lock(|m| self.app.app_match_sensors(m, &mut app_ctx!(self)))
    }

    fn reset_stm32(&mut self) {
        self.app.reset_stm32(&mut app_ctx!(self));
    }

    fn app_learn_pending(&mut self, valve: u16, status: u8, calibration: bool) -> bool {
        self.motor
            .lock(|m| self.app.app_learn_pending(m, valve, status, calibration))
    }

    fn app_service_move(&mut self, valve: u16, dir: u8, counts: u16, max_ma: u8) -> i16 {
        self.motor.lock(|m| {
            self.app
                .app_service_move(m, &mut app_ctx!(self), valve, dir, counts, max_ma)
        })
    }

    fn app_lease_heartbeat(&mut self, alive: bool) {
        self.app.app_lease_heartbeat(alive);
    }

    fn app_lease_state(&mut self) -> u8 {
        self.app.app_lease_state()
    }

    fn app_lease_remaining_s(&mut self) -> u32 {
        self.app.app_lease_remaining_s()
    }

    fn app_lease_client(&mut self) -> bool {
        self.app.app_lease_client()
    }

    fn app_lease_timeout(&mut self) -> u16 {
        self.app.app_lease_timeout()
    }

    fn app_lease_configure(&mut self, minutes: u16) {
        self.app.app_lease_configure(minutes);
    }

    fn app_lease_command(&mut self) {
        self.app.app_lease_command();
    }

    fn app_failsafe_mask(&mut self) -> u16 {
        self.motor.lock(|m| self.app.app_failsafe_mask(m))
    }

    fn app_set_failsafe(&mut self, valve: u16, pct: u8) {
        self.app.app_set_failsafe(valve, pct);
    }

    fn app_failsafe_pct(&mut self, valve: u16) -> u8 {
        self.app.app_failsafe_pct(valve)
    }

    fn app_stop(&mut self, valve: u16) -> i16 {
        self.motor
            .lock(|m| self.app.app_stop(m, &mut app_ctx!(self), valve))
    }

    fn app_get_learntime(&mut self) -> u32 {
        self.app.app_get_learntime()
    }

    fn app_temp_age_s(&mut self) -> u32 {
        self.app.app_temp_age_s(&mut app_ctx!(self))
    }

    fn app_protect_suspended(&mut self) -> bool {
        self.app.app_protect_suspended()
    }

    fn app_get_valve_v3(&mut self, valve: u16) -> ValveV3Info {
        let mut out = ValveV3Info::default();
        self.motor.lock(|m| {
            self.app
                .app_get_valve_v3(m, &mut app_ctx!(self), valve, &mut out)
        });
        out
    }

    fn motor_get_params(&mut self) -> MotorParams {
        self.motor.lock(|m| m.motor_get_params())
    }

    fn motor_set_params(&mut self, params: &MotorParams) {
        let mut env = MotorCtx {
            noinit: self.noinit,
            eep: &mut self.eeprom.eep_content.cfg,
        };
        self.motor.lock(|m| m.motor_set_params(&mut env, params));
    }

    fn motor_get_escalation(&mut self) -> EscalationConfig {
        self.motor.lock(|m| m.motor_get_escalation())
    }

    fn motor_set_escalation(&mut self, config: &EscalationConfig) {
        let mut env = MotorCtx {
            noinit: self.noinit,
            eep: &mut self.eeprom.eep_content.cfg,
        };
        self.motor
            .lock(|m| m.motor_set_escalation(&mut env, config));
    }

    fn valve_get_snapshot(&mut self, valve: u16) -> ValveSnapshot {
        let mut out = ValveSnapshot::default();
        self.motor
            .lock(|m| m.valve_get_snapshot(u32::from(valve), &mut out));
        out
    }

    fn valve_get_profile(&mut self, valve: u16, out: &mut ProfileRecorder) {
        self.motor
            .lock(|m| m.valve_get_profile(u32::from(valve), out));
    }

    fn eeprom_changed(&mut self, fields: u16) {
        self.eeprom.changed(fields);
    }

    fn eeprom_changed_slot(&mut self, slot: u8) {
        self.eeprom.changed_slot(slot);
    }

    fn eeprom_state(&mut self) -> u8 {
        self.eeprom.state()
    }

    fn eeprom_cfg_flags(&mut self) -> u8 {
        self.eeprom.cfg_flags()
    }

    fn eeprom_cfg_events(&mut self) -> u32 {
        self.eeprom.cfg_events()
    }

    fn eeprom_writes(&mut self) -> u32 {
        self.eeprom.writes()
    }

    fn temp_command(&mut self, command: i32) {
        let mut env = OwCtx::new(self.flags);
        self.ow.temp_command(command, &mut env);
    }

    fn ow_scan_age_s(&mut self) -> u32 {
        self.ow.ow_scan_age_s(&self.board)
    }

    fn sysstat_uptime_s(&mut self) -> u32 {
        self.sysstat.uptime_s()
    }

    fn sysstat_resets(&mut self) -> u32 {
        self.sysstat.resets()
    }

    fn sysstat_boot_reason(&mut self) -> BootReason {
        self.sysstat.boot_reason()
    }

    fn sysstat_safe_mode(&mut self) -> bool {
        self.sysstat.safe_mode()
    }

    fn sysstat_wdg_resets(&mut self) -> u8 {
        self.sysstat.wdg_resets()
    }

    fn sysstat_leave_safe_mode(&mut self) {
        self.sysstat.leave_safe_mode(&mut SysCtx {
            noinit: self.noinit,
        });
    }

    fn eep_content(&mut self) -> &mut EepromLayout {
        &mut self.eeprom.eep_content
    }

    fn ow_sensors(&self) -> &OwSensors {
        &self.ow.sensors
    }

    fn valve_globals(&self, valve: u16) -> ValveGlobals {
        let v = usize::from(valve);
        self.motor.lock(|m| match (m.mots.get(v), m.valves.get(v)) {
            (Some(mot), Some(valve)) => ValveGlobals {
                status: mot.status,
                calibration: mot.calibration,
                actual_position: mot.actual_position,
                target_position: mot.target_position,
                meancurrent: mot.meancurrent,
                opening_count: mot.opening_count,
                closing_count: mot.closing_count,
                deadzone_count: mot.deadzone_count,
                calib_retries: mot.calib_retries,
                sensorindex1: valve.sensorindex1,
                sensorindex2: valve.sensorindex2,
                movements: valve.movements,
                cmd_rejected: valve.cmd_rejected,
            },
            _ => ValveGlobals::default(),
        })
    }

    fn set_target_position(&mut self, valve: u16, pos: u8) {
        self.motor.lock(|m| {
            if let Some(mot) = m.mots.get_mut(usize::from(valve)) {
                mot.target_position = pos;
            }
        });
    }

    fn set_valve_sensor_index(&mut self, valve: u16, slot: u8, sensor: u16) {
        self.motor.lock(|m| {
            if let Some(v) = m.valves.get_mut(usize::from(valve)) {
                if slot == 1 {
                    v.sensorindex1 = u32::from(sensor);
                } else {
                    v.sensorindex2 = u32::from(sensor);
                }
            }
        });
    }

    fn learning_movements(&self) -> u32 {
        self.motor.lock(|m| m.learning_movements)
    }

    fn motor_globals(&self) -> MotorParams {
        self.motor.lock(|m| m.motor_get_params())
    }

    fn uart_errors(&mut self) -> UartErrorCounters {
        self.esp_port.errors()
    }
}

#[cfg(feature = "terminal")]
impl<P: Platform> TerminalEnv for Ctx<'_, P> {
    fn appsetaction(&mut self, cmd: u8, valve: u16, pos: u8) -> i16 {
        let mut env = MotorCtx {
            noinit: self.noinit,
            eep: &mut self.eeprom.eep_content.cfg,
        };
        self.motor
            .lock(|m| m.appsetaction(&mut env, cmd, u32::from(valve), pos, false, 0))
    }

    fn valve_idle(&mut self) -> bool {
        self.motor.lock(|m| m.valve_idle())
    }

    fn motor_get_params(&mut self) -> MotorParams {
        CommunicationEnv::motor_get_params(self)
    }

    fn motor_set_params(&mut self, params: &MotorParams) {
        CommunicationEnv::motor_set_params(self, params);
    }

    fn sysstat_safe_mode(&mut self) -> bool {
        self.sysstat.safe_mode()
    }

    fn print_sensordata(&mut self, out: &mut impl Print) {
        self.ow.print_sensordata(out);
    }

    fn temp_command(&mut self, command: i32) {
        CommunicationEnv::temp_command(self, command);
    }

    fn eeprom_state(&mut self) -> u8 {
        self.eeprom.state()
    }

    fn eeprom_changed(&mut self, fields: u16) {
        self.eeprom.changed(fields);
    }

    fn comm_set_valve_sensor_index(&mut self, valve: u16, slot: u8, sensor: u16) -> i16 {
        comm_set_valve_sensor_index(self, valve, slot, sensor)
    }

    fn comm_set_valve_sensors(&mut self, valve: u16, first: &[u8], second: &[u8]) -> i16 {
        comm_set_valve_sensors(self, valve, first, second)
    }

    fn comm_print_valve_sensor_ids(&mut self, out: &mut impl Print, valve: u16, delimiter: u8) {
        comm_print_valve_sensor_ids(self, out, valve, delimiter);
    }

    fn comm_set_learntime(&mut self, seconds: u32) -> i16 {
        comm_set_learntime(self, seconds)
    }

    fn app_match_sensors(&mut self) -> i16 {
        CommunicationEnv::app_match_sensors(self)
    }

    fn app_set_valveopen(&mut self, valve: u16) -> i16 {
        CommunicationEnv::app_set_valveopen(self, valve)
    }

    fn app_set_valvelearning(&mut self, valve: u16) -> i16 {
        CommunicationEnv::app_set_valvelearning(self, valve)
    }

    fn app_scan_valves(&mut self) {
        CommunicationEnv::app_scan_valves(self);
    }

    fn set_target_position(&mut self, valve: u16, pos: u8) {
        CommunicationEnv::set_target_position(self, valve, pos);
    }

    fn motor_globals(&self) -> MotorParams {
        CommunicationEnv::motor_globals(self)
    }

    fn eep_content(&mut self) -> &mut EepromLayout {
        &mut self.eeprom.eep_content
    }

    fn analog_current(&self) -> i32 {
        self.motor.lock(|m| m.analog_current)
    }
}

/// owDevices.cpp's calls out: the temperature lock of the valve state machine; the calls into
/// app.cpp are made by the controller when `temperature_loop()` returns.
struct OwCtx<'c> {
    flags: &'c IsrFlags,
    match_sensors: bool,
    cycle_done: bool,
}

impl<'c> OwCtx<'c> {
    fn new(flags: &'c IsrFlags) -> Self {
        OwCtx {
            flags,
            match_sensors: false,
            cycle_done: false,
        }
    }
}

impl OwDevicesEnv for OwCtx<'_> {
    fn app_temp_cycle_done(&mut self) {
        self.cycle_done = true;
    }

    fn app_match_sensors(&mut self) -> i16 {
        self.match_sensors = true;
        0
    }

    fn temp_locked(&self) -> bool {
        self.flags.temp_lock.load(Ordering::SeqCst)
    }

    fn set_temp_lock(&mut self, locked: bool) {
        self.flags.temp_lock.store(locked, Ordering::SeqCst);
    }
}

/// eeprom.cpp's calls out: the bus restart before a retry; `app_load_config()` of a re-read
/// is made by the controller when `eepromloop()` returns.
struct EepCtx<'c, I> {
    i2c: &'c mut I,
    reload: bool,
}

impl<I: I2cBus> EepromEnv for EepCtx<'_, I> {
    fn i2c_bus_restart(&mut self) {
        self.i2c.restart();
    }

    fn app_load_config(&mut self) {
        self.reload = true;
    }
}

/// sysstat.cpp's guard cell in the no-init RAM, with the layout of the boot stage's capture.
struct SysCtx<N> {
    noinit: N,
}

impl<N: NoinitStore> SysstatEnv for SysCtx<N> {
    fn store_guard_cell(&mut self, cell: &ResetGuardCell) {
        let mut image = self.noinit.read();
        write_guard(&mut image, cell);
        self.noinit.write(&image);
    }
}

// ---------------------------------------------------------------- main.cpp

impl<'a, P: Platform> Modules<'a, P> {
    fn manual_active(&self) -> bool {
        #[cfg(feature = "terminal")]
        {
            self.term.manual_active()
        }
        #[cfg(not(feature = "terminal"))]
        {
            false
        }
    }

    /// `app.cpp` with the motor state, under the lock.
    fn with_app<R>(
        &mut self,
        f: impl FnOnce(&mut App, &mut MotorShared, &mut AppCtx<'_, P>) -> R,
    ) -> R {
        let manual = self.manual_active();
        let mut env = AppCtx::<P> {
            board: self.hw.board,
            noinit: self.hw.noinit,
            dbg: self.hw.dbg,
            eeprom: &mut self.eeprom,
            sysstat: &self.sysstat,
            sensors: &self.ow.sensors,
            manual,
        };
        let app = &mut self.app;
        self.motor.lock(|m| f(app, m, &mut env))
    }
}

impl<P: Platform> Pins for Modules<'_, P> {
    fn set(&self, pin: Out, high: bool) {
        self.hw.board.set(pin, high);
    }

    fn latch(&self, pin: Out) -> bool {
        self.hw.board.latch(pin)
    }

    fn read(&self, pin: In) -> bool {
        self.hw.board.read(pin)
    }
}

impl<P: Platform> Clock for Modules<'_, P> {
    fn millis(&self) -> u32 {
        self.hw.board.millis()
    }

    fn micros(&self) -> u32 {
        self.hw.board.micros()
    }

    fn delay_ms(&self, ms: u32) {
        self.hw.board.delay_ms(ms);
    }

    fn delay_us(&self, us: u32) {
        self.hw.board.delay_us(us);
    }
}

impl<P: Platform> Watchdog for Modules<'_, P> {
    fn reload(&mut self) {
        self.hw.watchdog.reload();
    }
}

impl<P: Platform> ControlTimer for Modules<'_, P> {
    fn attach_interval(&mut self, interval_us: u32) -> bool {
        self.hw.tim2.attach_interval(interval_us)
    }
}

impl<P: Platform> MainLoopEnv for Modules<'_, P> {
    fn sysstat_boot_reason(&mut self) -> BootReason {
        self.sysstat.boot_reason()
    }

    fn sysstat_safe_mode(&mut self) -> bool {
        self.sysstat.safe_mode()
    }

    fn sysstat_loop(&mut self) {
        let board = self.hw.board;
        self.sysstat.loop_(
            &board,
            &mut SysCtx {
                noinit: self.hw.noinit,
            },
        );
    }

    fn sysstat_uptime_s(&mut self) -> u32 {
        self.sysstat.uptime_s()
    }

    fn i2c_bus_recover(&mut self) {
        self.hw.i2c.recover();
    }

    fn wire_begin(&mut self) {
        self.hw.i2c.begin();
    }

    fn terminal_init(&mut self) -> i16 {
        #[cfg(feature = "terminal")]
        {
            let mut dbg = self.hw.dbg;
            self.term.init(&mut dbg)
        }
        #[cfg(not(feature = "terminal"))]
        {
            0
        }
    }

    fn terminal_serve(&mut self) -> i16 {
        #[cfg(feature = "terminal")]
        {
            let mut dbg = self.hw.dbg;
            let board = self.hw.board;
            let manual = self.term.manual_active();
            let mut env = Ctx::<P> {
                app: &mut self.app,
                eeprom: &mut self.eeprom,
                ow: &mut self.ow,
                sysstat: &mut self.sysstat,
                motor: self.motor,
                flags: self.flags,
                esp_port: self.esp_port,
                board,
                noinit: self.hw.noinit,
                dbg,
                manual,
            };
            self.term.serve(&mut dbg, &board, &board, &mut env)
        }
        #[cfg(not(feature = "terminal"))]
        {
            -1
        }
    }

    fn terminal_supervise(&mut self) {
        #[cfg(feature = "terminal")]
        {
            let mut dbg = self.hw.dbg;
            let board = self.hw.board;
            let manual = self.term.manual_active();
            let env = Ctx::<P> {
                app: &mut self.app,
                eeprom: &mut self.eeprom,
                ow: &mut self.ow,
                sysstat: &mut self.sysstat,
                motor: self.motor,
                flags: self.flags,
                esp_port: self.esp_port,
                board,
                noinit: self.hw.noinit,
                dbg,
                manual,
            };
            self.term.supervise(&mut dbg, &board, &board, &env);
        }
    }

    fn communication_setup(&mut self) {
        let mut esp = self.hw.esp;
        let mut dbg = self.hw.dbg;
        self.comm.setup(&mut esp, &mut dbg);
    }

    fn communication_loop(&mut self) -> i16 {
        let mut esp = self.hw.esp;
        let mut dbg = self.hw.dbg;
        let board = self.hw.board;
        let manual = self.manual_active();
        let mut env = Ctx::<P> {
            app: &mut self.app,
            eeprom: &mut self.eeprom,
            ow: &mut self.ow,
            sysstat: &mut self.sysstat,
            motor: self.motor,
            flags: self.flags,
            esp_port: self.esp_port,
            board,
            noinit: self.hw.noinit,
            dbg,
            manual,
        };
        self.comm
            .loop_(&mut esp, &mut dbg, &board, &board, &mut env)
    }

    fn eepromsetup(&mut self) -> i16 {
        self.eeprom.setup()
    }

    fn eeprom_read_layout(&mut self) -> i16 {
        let mut dbg = self.hw.dbg;
        self.eeprom.read_layout(&mut self.hw.eeprom, &mut dbg)
    }

    fn eepromloop(&mut self) -> i16 {
        let mut dbg = self.hw.dbg;
        let mut env = EepCtx {
            i2c: &mut self.hw.i2c,
            reload: false,
        };
        let r = self.eeprom.loop_(&mut self.hw.eeprom, &mut dbg, &mut env);
        if env.reload {
            self.with_app(|app, m, env| app.app_load_config(m, env));
        }
        r
    }

    fn temperature_setup(&mut self) {
        let board = self.hw.board;
        self.ow.setup(&mut self.hw.one_wire, &board);
    }

    fn temperature_loop(&mut self) {
        let board = self.hw.board;
        let mut env = OwCtx::new(self.flags);
        self.ow.loop_(&mut self.hw.one_wire, &board, &mut env);
        if env.match_sensors {
            self.with_app(|app, m, env| app.app_match_sensors(m, env));
        }
        if env.cycle_done {
            self.with_app(|app, _, env| app.app_temp_cycle_done(env));
        }
    }

    fn app_setup(&mut self) -> i16 {
        self.with_app(|app, m, env| app.app_setup(m, env))
    }

    fn app_restore(&mut self) {
        self.with_app(|app, m, env| app.app_restore(m, env));
    }

    fn app_loop(&mut self) -> i16 {
        let flags = self.flags;
        self.with_app(|app, m, env| app.app_loop(m, flags, env))
    }

    fn app_warm_save(&mut self) {
        self.with_app(|app, m, env| app.app_warm_save(m, env));
    }

    fn app_1s_tick(&mut self, elapsed_s: u32) {
        self.with_app(|app, m, _| app.app_1s_tick(m, elapsed_s));
    }

    fn app_10s_loop(&mut self, elapsed_s: u32) -> u8 {
        self.with_app(|app, m, env| app.app_10s_loop(m, env, elapsed_s))
    }

    fn valve_setup(&mut self) -> u8 {
        let tim1 = &mut self.hw.tim1;
        self.motor.lock(|m| m.valve_setup(tim1))
    }

    fn debug(&mut self, text: &[u8]) {
        self.hw.dbg.write(text);
    }
}

// the system suites compare the debug output of the terminal build with the C++ goldens
#[cfg(all(test, feature = "terminal"))]
mod bench;
#[cfg(all(test, feature = "terminal"))]
mod golden;
#[cfg(all(test, feature = "terminal"))]
mod tests_env;
