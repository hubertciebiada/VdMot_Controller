//! The link-seam stubs of the glue modules (port of `test/native/glue/stubs/stub_<module>.cpp`):
//! every stubbed call appends "name(arg, arg)" to the call log (integers in decimal, masks in
//! hex) and returns a knob; the globals of the stubbed modules keep their real types. One
//! struct carries the stubs of every module, as the C++ executables link them; it implements
//! the Env trait of the module under test.

use crate::communication::{
    CommunicationEnv, ValveGlobals, ValveSnapshot, ValveV3Info, SERIAL_RX_BUFFER_SIZE,
};
use crate::eeprom::EepromLayout;
use crate::ow_devices::OwSensors;
#[cfg(feature = "terminal")]
use crate::print::Print;
#[cfg(feature = "terminal")]
use crate::terminal::TerminalEnv;
use crate::test_support::stub_log::CallLog;
use vdm_stm_core::calibration::{EscalationConfig, ESCALATION_DEFAULT};
use vdm_stm_core::motor_params::MotorParams;
use vdm_stm_core::profile_recorder::ProfileRecorder;
use vdm_stm_core::replies_v2::EEP_STATE_OK;
use vdm_stm_core::system_stats::BootReason;
use vdm_stm_core::uart_errors::UartErrorCounters;

/// NO_OF_MIN_COUNTS (app.h)
pub const NO_OF_MIN_COUNTS: u16 = 3000;
/// LEARN_AFTER_MOVEMENTS_DEFAULT (app.h)
pub const LEARN_AFTER_MOVEMENTS_DEFAULT: u32 = 2000;
/// VALVE_SENSOR_UNKNOWN (app.h)
pub const VALVE_SENSOR_UNKNOWN: u32 = 65535;

/// knobs of stub_app
pub struct App {
    pub set_learn_movements: i16,
    pub set_learn_time: i16,
    pub set_valve_learning: i16,
    pub set_valve_open: i16,
    pub learn_pending: bool,
    pub service_move: i16,
    pub match_sensors: i16,
    pub lease_state: u8,
    pub lease_remaining_s: u32,
    pub lease_client: bool,
    pub lease_timeout: u16,
    pub failsafe_mask: u16,
    pub failsafe_pct: u8,
    pub stop: i16,
    pub learn_time: u32,
    pub temp_age_s: u32,
    pub protect_suspended: bool,
    pub valve_v3: ValveV3Info,
}

impl Default for App {
    fn default() -> Self {
        App {
            set_learn_movements: 0,
            set_learn_time: 0,
            set_valve_learning: 0,
            set_valve_open: 0,
            learn_pending: false,
            service_move: 0,
            match_sensors: 0,
            lease_state: 0,
            lease_remaining_s: 0,
            lease_client: false,
            lease_timeout: 0,
            failsafe_mask: 0,
            failsafe_pct: 255,
            stop: 0,
            learn_time: 0,
            temp_age_s: 0,
            protect_suspended: false,
            valve_v3: ValveV3Info {
                flags: 0,
                fault: 0,
                fs_pct: 255,
                drive: 0,
                retry_s: 0,
                retries: 0,
            },
        }
    }
}

/// knobs of stub_motor (the setters store into them)
pub struct Motor {
    pub idle: bool,
    pub action: i16,
    pub snapshot: [ValveSnapshot; 12],
    pub profile: ProfileRecorder,
    pub params: MotorParams,
    pub escalation: EscalationConfig,
}

impl Default for Motor {
    fn default() -> Self {
        Motor {
            idle: true,
            action: 0,
            snapshot: [ValveSnapshot::default(); 12],
            profile: ProfileRecorder::default(),
            params: MotorParams {
                low_fac: 17,
                high_fac: 17,
                start_on_power: 50,
                min_counts: NO_OF_MIN_COUNTS,
                max_retries: 0,
            },
            escalation: ESCALATION_DEFAULT,
        }
    }
}

/// knobs of stub_eeprom
#[derive(Default)]
pub struct Eeprom {
    pub cfg_flags: u8,
    pub cfg_events: u32,
    pub writes: u32,
    pub state: u8,
}

/// knobs of stub_owDevices
pub struct OwDevices {
    pub scan_age_s: u32,
    /// what print_sensordata() prints
    pub sensor_data: String,
}

impl Default for OwDevices {
    fn default() -> Self {
        OwDevices {
            scan_age_s: 0,
            sensor_data: "{\"cnt\":0}\r\n".to_string(),
        }
    }
}

/// knobs of stub_communication
pub struct Communication {
    pub set_sensors: i16,
    pub set_sensor_index: i16,
    pub set_learn_time: i16,
    /// what comm_print_valve_sensor_ids() prints (the delimiter goes between the two)
    pub first_sensor: String,
    pub second_sensor: String,
}

impl Default for Communication {
    fn default() -> Self {
        Communication {
            set_sensors: 0,
            set_sensor_index: 0,
            set_learn_time: 0,
            first_sensor: "00-00-00-00-00-00-00-00".to_string(),
            second_sensor: "00-00-00-00-00-00-00-00".to_string(),
        }
    }
}

/// knobs of stub_sysstat
#[derive(Default)]
pub struct Sysstat {
    pub uptime: u32,
    pub resets: u32,
    pub reason: BootReason,
    pub safe_mode: bool,
    pub wdg_resets: u8,
}

pub struct Stubs {
    pub calls: CallLog,
    pub app: App,
    pub motor: Motor,
    pub eeprom: Eeprom,
    pub ow: OwDevices,
    pub sysstat: Sysstat,
    pub comm: Communication,
    /// motor.cpp `analog_current`: the filtered motor current in 0.1 mA
    pub analog_current: i32,
    // ---- the globals of the stubbed modules
    /// eeprom.cpp `eep_content`
    pub eep_content: EepromLayout,
    /// owDevices.cpp `tempsensors`, `voltsensors`, `noOf*Devices`
    pub sensors: OwSensors,
    /// motor.cpp `myvalvemots[]` and app.cpp `myvalves[]`
    pub valves: [ValveGlobals; 12],
    /// app.cpp `learning_movements`
    pub learning_movements: u32,
    /// motor.cpp `currentbound_low_fac`, ... `maxCalibRetries`
    pub motor_globals: MotorParams,
    /// communication.cpp `commUartErrors` (the rig copies the counters of the ESP UART)
    pub uart: UartErrorCounters,
}

impl Default for Stubs {
    fn default() -> Self {
        Stubs {
            calls: CallLog::default(),
            app: App::default(),
            motor: Motor::default(),
            eeprom: Eeprom {
                state: EEP_STATE_OK,
                ..Eeprom::default()
            },
            ow: OwDevices::default(),
            sysstat: Sysstat::default(),
            comm: Communication::default(),
            analog_current: 0,
            eep_content: EepromLayout::default(),
            sensors: OwSensors::default(),
            valves: [ValveGlobals::default(); 12],
            learning_movements: LEARN_AFTER_MOVEMENTS_DEFAULT,
            motor_globals: MotorParams {
                low_fac: 17,
                high_fac: 17,
                start_on_power: 50,
                min_counts: NO_OF_MIN_COUNTS,
                max_retries: 0,
            },
            uart: UartErrorCounters::default(),
        }
    }
}

impl Stubs {
    fn log(&mut self, entry: String) {
        self.calls.log(entry);
    }
}

// the serial ring sizes the rig assumes
const _: () = assert!(SERIAL_RX_BUFFER_SIZE == 1024);

impl CommunicationEnv for Stubs {
    fn app_lease_poll(&mut self) {
        self.log("app_lease_poll()".into());
    }

    fn app_target_changed(&mut self, valve: u16) {
        self.log(format!("app_target_changed({valve})"));
    }

    fn app_set_learntime(&mut self, time: u32) -> i16 {
        self.log(format!("app_set_learntime({time})"));
        self.app.set_learn_time
    }

    fn app_set_learnmovements(&mut self, cycles: u16) -> i16 {
        self.log(format!("app_set_learnmovements({cycles})"));
        self.app.set_learn_movements
    }

    fn app_set_valveopen(&mut self, valve: u16) -> i16 {
        self.log(format!("app_set_valveopen({valve})"));
        self.app.set_valve_open
    }

    fn app_set_valvelearning(&mut self, valve: u16) -> i16 {
        self.log(format!("app_set_valvelearning({valve})"));
        self.app.set_valve_learning
    }

    fn app_scan_valves(&mut self) {
        self.log("app_scan_valves()".into());
    }

    fn app_match_sensors(&mut self) -> i16 {
        self.log("app_match_sensors()".into());
        self.app.match_sensors
    }

    fn reset_stm32(&mut self) {
        self.log("reset_STM32()".into());
    }

    fn app_learn_pending(&mut self, valve: u16, status: u8, calibration: bool) -> bool {
        self.log(format!(
            "app_learn_pending({valve}, {status}, {})",
            u8::from(calibration)
        ));
        self.app.learn_pending
    }

    fn app_service_move(&mut self, valve: u16, dir: u8, counts: u16, max_ma: u8) -> i16 {
        self.log(format!(
            "app_service_move({valve}, {dir}, {counts}, {max_ma})"
        ));
        self.app.service_move
    }

    fn app_lease_heartbeat(&mut self, alive: bool) {
        self.log(format!("app_lease_heartbeat({})", u8::from(alive)));
    }

    fn app_lease_state(&mut self) -> u8 {
        self.log("app_lease_state()".into());
        self.app.lease_state
    }

    fn app_lease_remaining_s(&mut self) -> u32 {
        self.log("app_lease_remaining_s()".into());
        self.app.lease_remaining_s
    }

    fn app_lease_client(&mut self) -> bool {
        self.log("app_lease_client()".into());
        self.app.lease_client
    }

    fn app_lease_timeout(&mut self) -> u16 {
        self.log("app_lease_timeout()".into());
        self.app.lease_timeout
    }

    fn app_lease_configure(&mut self, minutes: u16) {
        self.log(format!("app_lease_configure({minutes})"));
    }

    fn app_lease_command(&mut self) {
        self.log("app_lease_command()".into());
    }

    fn app_failsafe_mask(&mut self) -> u16 {
        self.log("app_failsafe_mask()".into());
        self.app.failsafe_mask
    }

    fn app_set_failsafe(&mut self, valve: u16, pct: u8) {
        self.log(format!("app_set_failsafe({valve}, {pct})"));
    }

    fn app_failsafe_pct(&mut self, valve: u16) -> u8 {
        self.log(format!("app_failsafe_pct({valve})"));
        self.app.failsafe_pct
    }

    fn app_stop(&mut self, valve: u16) -> i16 {
        self.log(format!("app_stop({valve})"));
        self.app.stop
    }

    fn app_get_learntime(&mut self) -> u32 {
        self.log("app_get_learntime()".into());
        self.app.learn_time
    }

    fn app_temp_age_s(&mut self) -> u32 {
        self.log("app_temp_age_s()".into());
        self.app.temp_age_s
    }

    fn app_protect_suspended(&mut self) -> bool {
        self.log("app_protect_suspended()".into());
        self.app.protect_suspended
    }

    fn app_get_valve_v3(&mut self, valve: u16) -> ValveV3Info {
        self.log(format!("app_get_valve_v3({valve})"));
        self.app.valve_v3
    }

    fn motor_get_params(&mut self) -> MotorParams {
        self.log("motor_get_params()".into());
        self.motor.params
    }

    fn motor_set_params(&mut self, p: &MotorParams) {
        self.log(format!(
            "motor_set_params({}, {}, {}, {}, {})",
            p.low_fac, p.high_fac, p.start_on_power, p.min_counts, p.max_retries
        ));
        self.motor.params = *p;
    }

    fn motor_get_escalation(&mut self) -> EscalationConfig {
        self.log("motor_get_escalation()".into());
        self.motor.escalation
    }

    fn motor_set_escalation(&mut self, c: &EscalationConfig) {
        self.log(format!(
            "motor_set_escalation({}, {}, {})",
            c.enable, c.step_pct, c.max_ma
        ));
        self.motor.escalation = *c;
    }

    fn valve_get_snapshot(&mut self, valve: u16) -> ValveSnapshot {
        self.log(format!("valve_get_snapshot({valve})"));
        self.motor
            .snapshot
            .get(usize::from(valve))
            .copied()
            .unwrap_or_default()
    }

    fn valve_get_profile(&mut self, valve: u16, out: &mut ProfileRecorder) {
        self.log(format!("valve_get_profile({valve})"));
        *out = self.motor.profile;
    }

    fn eeprom_changed(&mut self, fields: u16) {
        self.log(format!("eeprom_changed(0x{fields:04x})"));
    }

    fn eeprom_changed_slot(&mut self, slot: u8) {
        self.log(format!("eeprom_changed_slot({slot})"));
    }

    fn eeprom_state(&mut self) -> u8 {
        self.log("eeprom_state()".into());
        self.eeprom.state
    }

    fn eeprom_cfg_flags(&mut self) -> u8 {
        self.log("eeprom_cfg_flags()".into());
        self.eeprom.cfg_flags
    }

    fn eeprom_cfg_events(&mut self) -> u32 {
        self.log("eeprom_cfg_events()".into());
        self.eeprom.cfg_events
    }

    fn eeprom_writes(&mut self) -> u32 {
        self.log("eeprom_writes()".into());
        self.eeprom.writes
    }

    fn temp_command(&mut self, command: i32) {
        self.log(format!("temp_command({command})"));
    }

    fn ow_scan_age_s(&mut self) -> u32 {
        self.log("ow_scan_age_s()".into());
        self.ow.scan_age_s
    }

    fn sysstat_uptime_s(&mut self) -> u32 {
        self.log("sysstat_uptime_s()".into());
        self.sysstat.uptime
    }

    fn sysstat_resets(&mut self) -> u32 {
        self.log("sysstat_resets()".into());
        self.sysstat.resets
    }

    fn sysstat_boot_reason(&mut self) -> BootReason {
        self.log("sysstat_boot_reason()".into());
        self.sysstat.reason
    }

    fn sysstat_safe_mode(&mut self) -> bool {
        self.log("sysstat_safe_mode()".into());
        self.sysstat.safe_mode
    }

    fn sysstat_wdg_resets(&mut self) -> u8 {
        self.log("sysstat_wdg_resets()".into());
        self.sysstat.wdg_resets
    }

    fn sysstat_leave_safe_mode(&mut self) {
        self.log("sysstat_leave_safe_mode()".into());
    }

    fn eep_content(&mut self) -> &mut EepromLayout {
        &mut self.eep_content
    }

    fn ow_sensors(&self) -> &OwSensors {
        &self.sensors
    }

    fn valve_globals(&self, valve: u16) -> ValveGlobals {
        self.valves[usize::from(valve)]
    }

    fn set_target_position(&mut self, valve: u16, pos: u8) {
        self.valves[usize::from(valve)].target_position = pos;
    }

    fn set_valve_sensor_index(&mut self, valve: u16, slot: u8, sensor: u16) {
        let v = &mut self.valves[usize::from(valve)];
        if slot == 1 {
            v.sensorindex1 = u32::from(sensor);
        } else {
            v.sensorindex2 = u32::from(sensor);
        }
    }

    fn learning_movements(&self) -> u32 {
        self.learning_movements
    }

    fn motor_globals(&self) -> MotorParams {
        self.motor_globals
    }

    fn uart_errors(&mut self) -> UartErrorCounters {
        self.uart
    }
}

#[cfg(feature = "terminal")]
impl TerminalEnv for Stubs {
    fn appsetaction(&mut self, cmd: u8, valve: u16, pos: u8) -> i16 {
        self.log(format!("appsetaction({}, {valve}, {pos}, 0)", cmd as char));
        self.motor.action
    }

    fn valve_idle(&mut self) -> bool {
        self.log("valve_idle()".into());
        self.motor.idle
    }

    fn motor_get_params(&mut self) -> MotorParams {
        CommunicationEnv::motor_get_params(self)
    }

    fn motor_set_params(&mut self, params: &MotorParams) {
        CommunicationEnv::motor_set_params(self, params);
    }

    fn sysstat_safe_mode(&mut self) -> bool {
        CommunicationEnv::sysstat_safe_mode(self)
    }

    fn print_sensordata(&mut self, out: &mut impl Print) {
        self.log("print_sensordata()".into());
        out.print(self.ow.sensor_data.as_bytes());
    }

    fn temp_command(&mut self, command: i32) {
        CommunicationEnv::temp_command(self, command);
    }

    fn eeprom_state(&mut self) -> u8 {
        CommunicationEnv::eeprom_state(self)
    }

    fn eeprom_changed(&mut self, fields: u16) {
        CommunicationEnv::eeprom_changed(self, fields);
    }

    fn comm_set_valve_sensor_index(&mut self, valve: u16, slot: u8, sensor: u16) -> i16 {
        self.log(format!(
            "comm_set_valve_sensor_index({valve}, {slot}, {sensor})"
        ));
        self.comm.set_sensor_index
    }

    fn comm_set_valve_sensors(&mut self, valve: u16, first: &[u8], second: &[u8]) -> i16 {
        self.log(format!(
            "comm_set_valve_sensors({valve}, {}, {})",
            String::from_utf8_lossy(first),
            String::from_utf8_lossy(second)
        ));
        self.comm.set_sensors
    }

    fn comm_print_valve_sensor_ids(&mut self, out: &mut impl Print, valve: u16, delimiter: u8) {
        self.log(format!(
            "comm_print_valve_sensor_ids({valve}, '{}')",
            delimiter as char
        ));
        out.print(self.comm.first_sensor.as_bytes());
        out.print_char(delimiter);
        out.print(self.comm.second_sensor.as_bytes());
    }

    fn comm_set_learntime(&mut self, seconds: u32) -> i16 {
        self.log(format!("comm_set_learntime({seconds})"));
        self.comm.set_learn_time
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
        self.motor_globals
    }

    fn eep_content(&mut self) -> &mut EepromLayout {
        &mut self.eep_content
    }

    fn analog_current(&self) -> i32 {
        self.analog_current
    }
}
