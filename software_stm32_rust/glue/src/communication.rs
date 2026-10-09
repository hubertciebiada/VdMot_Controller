//! The UART protocol to the ESP (`software_stm32/src/communication.cpp`,
//! `include/communication.h`): protocol v1 (byte-exact against firmware 2.0.0), v2 and v3
//! (`docs/revamped/PROTOCOL_V2.md`). Requests are lines of the core line assembler, split by the
//! core tokenizer; v1 replies are written with Arduino `Print`, v2/v3 replies are formatted by the
//! core into one reply buffer. Unknown commands get no reply.

use crate::dallas::DallasTemperature;
use crate::eeprom::EepromLayout;
use crate::hal::{Clock, Serial, System};
use crate::onewire::DeviceAddress;
use crate::ow_devices::{OwSensors, MAXDS2438CNT, MAXONEWIRECNT, TEMP_CMD_NEWSEARCH};
use crate::print::{Print, DEC};
use vdm_stm_core::arg_parser::{
    is_zero_address, parse_one_wire_address, ONE_WIRE_ADDRESS_TEXT_LEN,
};
use vdm_stm_core::buf_writer::{BufWriter, StaticBufWriter};
use vdm_stm_core::calibration::{
    EscalationConfig, ESCALATION_MAX_MA_MAX, ESCALATION_MAX_MA_MIN, ESCALATION_STEP_MAX,
    SAFETY_LIMIT_MA,
};
use vdm_stm_core::config_store::{
    CHANGED_ESCALATION, CHANGED_FAILSAFE, CHANGED_LEARN_TIME, CHANGED_LEASE, CHANGED_MOTOR,
    CHANGED_MOVEMENTS,
};
use vdm_stm_core::failsafe::failsafe_pct_valid;
use vdm_stm_core::lease::lease_timeout_valid;
use vdm_stm_core::legacy_layout::{SensorSlot, VALVE_COUNT};
use vdm_stm_core::line_assembler::StaticLineAssembler;
use vdm_stm_core::motor_params::{
    apply_motor_params_request, same_motor_params, MotorParams, ParamsRequest,
};
use vdm_stm_core::move_classifier::{DIR_CLOSE, DIR_OPEN};
use vdm_stm_core::profile_recorder::ProfileRecorder;
use vdm_stm_core::replies::{
    format_status_list, format_valve_data, ValveDataReply, VALVE_DATA_REPLY_MAX_LEN,
};
use vdm_stm_core::replies_v2::{
    compose_cal_state, eepst_saved, encode_valve_status, format_escalation, format_indexed_result,
    format_motor_limits, format_profile, format_protocol_version, format_result, format_stat,
    format_valve_ext, StatReply, ValveExtReply, EEP_STATE_READ_FAILED, PROFILE_REPLY_MAX_LEN,
};
use vdm_stm_core::replies_v3::{
    format_heartbeat, format_learn_time, format_lease_config, format_stat_v3, format_valve_ext_v3,
    StatV3Reply, ValveExtV3Reply, STAT_V3_REPLY_MAX_LEN, VALVE_EXT_V3_REPLY_MAX_LEN,
};
use vdm_stm_core::settings::learn_movements_from_request;
use vdm_stm_core::system_stats::BootReason;
use vdm_stm_core::tokenizer::Tokenizer;
use vdm_stm_core::uart_errors::{count_uart_errors, UartErrorCounters};
use vdm_stm_core::valve_codes::SYS_FLAG_PROTECT_SUSPENDED;

// command prefixes (communication.h APP_PRE_*)
pub const APP_PRE_SETTARGETPOS: &[u8] = b"stgtp";
pub const APP_PRE_GETONEWIRECNT: &[u8] = b"gonec";
pub const APP_PRE_GETONEWIREDATA: &[u8] = b"goned";
pub const APP_PRE_GETOWVOLTCNT: &[u8] = b"gowvc";
pub const APP_PRE_GETOWVOLTDATA: &[u8] = b"gowvd";
pub const APP_PRE_SET1STSENSORINDEX: &[u8] = b"stsnx";
pub const APP_PRE_SET2NDSENSORINDEX: &[u8] = b"stsny";
pub const APP_PRE_SETALLVLVOPEN: &[u8] = b"staop";
pub const APP_PRE_GETONEWIRESETT: &[u8] = b"gvlon";
pub const APP_PRE_GETVLVDATA: &[u8] = b"gvlvd";
pub const APP_PRE_GETVLSTATUS: &[u8] = b"gvlst";
pub const APP_PRE_SETONEWIRESEARCH: &[u8] = b"stons";
pub const APP_PRE_GETVERSION: &[u8] = b"gvers";
pub const APP_PRE_GETHWINFO: &[u8] = b"ghwin";
pub const APP_PRE_GETTARGETPOS: &[u8] = b"gtgtp";
pub const APP_PRE_SETLEARNTIME: &[u8] = b"stlnt";
pub const APP_PRE_SETLEARNMOVEM: &[u8] = b"stlnm";
pub const APP_PRE_GETLEARNMOVEM: &[u8] = b"gtlnm";
pub const APP_PRE_SETVLLEARN: &[u8] = b"staln";
pub const APP_PRE_SETMOTCHARS: &[u8] = b"smotc";
pub const APP_PRE_GETMOTCHARS: &[u8] = b"gmotc";
pub const APP_PRE_SETVLVSENSOR: &[u8] = b"stvls";
pub const APP_PRE_SETDETECTVLV: &[u8] = b"stdet";
pub const APP_PRE_MATCHSENS: &[u8] = b"masns";
pub const APP_PRE_SOFTRESET: &[u8] = b"reset";
pub const APP_PRE_EEPSTATE: &[u8] = b"eepst";
// protocol v2
pub const APP_PRE_GETPROTOCOL: &[u8] = b"gproto";
pub const APP_PRE_GETVLVEXT: &[u8] = b"gvlvx";
pub const APP_PRE_GETPROFILE: &[u8] = b"gprof";
pub const APP_PRE_SERVICEMOVE: &[u8] = b"svmov";
pub const APP_PRE_SETCALESC: &[u8] = b"scalx";
pub const APP_PRE_GETCALESC: &[u8] = b"gcalx";
pub const APP_PRE_GETSTATUS: &[u8] = b"gstat";
pub const APP_PRE_GETMOTLIMITS: &[u8] = b"gmotx";
// protocol 3
pub const APP_PRE_LEASEHEARTBEAT: &[u8] = b"slhbt";
pub const APP_PRE_SETLEASE: &[u8] = b"slcfg";
pub const APP_PRE_SETFAILSAFE: &[u8] = b"sfspo";
pub const APP_PRE_GETLEASE: &[u8] = b"glcfg";
pub const APP_PRE_GETVLVEXT3: &[u8] = b"gvlvy";
pub const APP_PRE_GETSTATUS3: &[u8] = b"gstax";
pub const APP_PRE_STOP: &[u8] = b"sstop";
pub const APP_PRE_GETLEARNTIME: &[u8] = b"gtlnt";
pub const APP_PRE_SAFEMODE: &[u8] = b"ssafe";

/// arguments of a request at most
pub const NO_OF_ARGS: u8 = 5;
/// the longest v1 request (stvls) has less than 64 characters
const COMM_LINE_SIZE: usize = 128;
/// requests handled per loop call
const COMM_MAX_LINES: u8 = 4;
/// bytes taken from the UART per loop call
const COMM_MAX_READ: u16 = 512;
/// an unterminated line is dropped after this idle time (a request takes ~10 ms)
const COMM_LINE_TIMEOUT_MS: u32 = 100;
const NO_SENSOR_ADDRESS: &[u8] = b"00-00-00-00-00-00-00-00";
/// sfspo, sstop: every valve
const ALL_VALVES: u16 = 255;
const ACTUATOR_COUNT: u16 = VALVE_COUNT as u16;
/// gonec, gowvc, gvlon: the whole list
const ALL_SENSORS: u16 = 255;
/// the temperature of a valve without a sensor (0.1 degC)
const NO_TEMPERATURE: i32 = -500;
/// the rings of the firmware's serial ports (the commDebug start line prints them)
pub const SERIAL_TX_BUFFER_SIZE: u32 = 1024;
pub const SERIAL_RX_BUFFER_SIZE: u32 = 1024;
/// svmov ranges (motor.h)
pub const SVMOV_COUNTS_MIN: u16 = 1;
pub const SVMOV_COUNTS_MAX: u16 = 10000;
pub const SVMOV_MAXMA_MIN: u8 = 5;
pub const SVMOV_MAXMA_MAX: u8 = SAFETY_LIMIT_MA;
/// app_service_move() result: a calibration of the valve is pending
const SVMOV_CALIB_PENDING: i16 = -3;

/// The firmware identity in the ID block (docs/rust/GLUE-DESIGN-STM.md §6.2): gvers reports
/// `<version>_<tag> <build>`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FirmwareId {
    /// FIRMWARE_VERSION
    pub version: &'static [u8],
    /// the board revision behind `VDM-HW:` (HARDWARE_REVISION_TAG)
    pub tag: &'static [u8],
    /// FIRMWARE_BUILD: "1" in release images
    pub build: &'static [u8],
}

/// motor.h `valve_diag` and `valve_snapshot` (the fields gvlvx reports, copied together), app.h
/// `valve_v3_info` (gvlvy values 20..25): the types of the motor and app ports.
pub use crate::app::ValveV3Info;
pub use crate::motor::{ValveDiag, ValveSnapshot};

/// What communication.cpp reads of the globals `myvalvemots[v]` (motor.cpp) and `myvalves[v]`
/// (app.cpp): plain data in C++, no call.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ValveGlobals {
    // myvalvemots[v]
    pub status: u8,
    pub calibration: bool,
    pub actual_position: u8,
    pub target_position: u8,
    pub meancurrent: u32,
    pub opening_count: u32,
    pub closing_count: u32,
    pub deadzone_count: i32,
    pub calib_retries: u8,
    // myvalves[v]
    /// index into tempsensors, VALVE_SENSOR_UNKNOWN (65535) without one
    pub sensorindex1: u32,
    pub sensorindex2: u32,
    pub movements: u32,
    pub cmd_rejected: u16,
}

/// Calls of communication.cpp into other glue modules, and the globals of those modules it
/// reads or writes (plain data in C++: the stubs do not log them).
pub trait CommunicationEnv {
    // ---- app.cpp
    fn app_lease_poll(&mut self);
    fn app_target_changed(&mut self, valve: u16);
    fn app_set_learntime(&mut self, time: u32) -> i16;
    fn app_set_learnmovements(&mut self, cycles: u16) -> i16;
    fn app_set_valveopen(&mut self, valve: u16) -> i16;
    fn app_set_valvelearning(&mut self, valve: u16) -> i16;
    fn app_scan_valves(&mut self);
    fn app_match_sensors(&mut self) -> i16;
    /// `reset_STM32()`: a soft reset once no EEPROM write waits
    fn reset_stm32(&mut self);
    /// calState "requested"
    fn app_learn_pending(&mut self, valve: u16, status: u8, calibration: bool) -> bool;
    /// 0, -3 calibration pending, another value: busy or safe mode
    fn app_service_move(&mut self, valve: u16, dir: u8, counts: u16, max_ma: u8) -> i16;
    fn app_lease_heartbeat(&mut self, alive: bool);
    fn app_lease_state(&mut self) -> u8;
    fn app_lease_remaining_s(&mut self) -> u32;
    fn app_lease_client(&mut self) -> bool;
    fn app_lease_timeout(&mut self) -> u16;
    fn app_lease_configure(&mut self, minutes: u16);
    fn app_lease_command(&mut self);
    fn app_failsafe_mask(&mut self) -> u16;
    fn app_set_failsafe(&mut self, valve: u16, pct: u8);
    fn app_failsafe_pct(&mut self, valve: u16) -> u8;
    fn app_stop(&mut self, valve: u16) -> i16;
    fn app_get_learntime(&mut self) -> u32;
    fn app_temp_age_s(&mut self) -> u32;
    fn app_protect_suspended(&mut self) -> bool;
    fn app_get_valve_v3(&mut self, valve: u16) -> ValveV3Info;
    // ---- motor.cpp
    fn motor_get_params(&mut self) -> MotorParams;
    fn motor_set_params(&mut self, params: &MotorParams);
    fn motor_get_escalation(&mut self) -> EscalationConfig;
    fn motor_set_escalation(&mut self, config: &EscalationConfig);
    /// a consistent copy of the gvlvx fields
    fn valve_get_snapshot(&mut self, valve: u16) -> ValveSnapshot;
    fn valve_get_profile(&mut self, valve: u16, out: &mut ProfileRecorder);
    // ---- eeprom.cpp
    fn eeprom_changed(&mut self, fields: u16);
    fn eeprom_changed_slot(&mut self, slot: u8);
    fn eeprom_state(&mut self) -> u8;
    fn eeprom_cfg_flags(&mut self) -> u8;
    fn eeprom_cfg_events(&mut self) -> u32;
    fn eeprom_writes(&mut self) -> u32;
    // ---- owDevices.cpp
    fn temp_command(&mut self, command: i32);
    fn ow_scan_age_s(&mut self) -> u32;
    // ---- sysstat.cpp
    fn sysstat_uptime_s(&mut self) -> u32;
    fn sysstat_resets(&mut self) -> u32;
    fn sysstat_boot_reason(&mut self) -> BootReason;
    fn sysstat_safe_mode(&mut self) -> bool;
    fn sysstat_wdg_resets(&mut self) -> u8;
    fn sysstat_leave_safe_mode(&mut self);
    // ---- globals of other modules
    /// eeprom.cpp `eep_content`
    fn eep_content(&mut self) -> &mut EepromLayout;
    /// owDevices.cpp `tempsensors[]`, `voltsensors[]`, `noOf*Devices`
    fn ow_sensors(&self) -> &OwSensors;
    /// `myvalvemots[valve]`, `myvalves[valve]` (valve 0..11)
    fn valve_globals(&self, valve: u16) -> ValveGlobals;
    /// `myvalvemots[valve].target_position = pos`
    fn set_target_position(&mut self, valve: u16, pos: u8);
    /// `myvalves[valve].sensorindex1` (slot 1) or `sensorindex2` (slot 2) = sensor
    fn set_valve_sensor_index(&mut self, valve: u16, slot: u8, sensor: u16);
    /// app.cpp `learning_movements`
    fn learning_movements(&self) -> u32;
    /// motor.cpp `currentbound_low_fac`, `currentbound_high_fac`, `startOnPower`,
    /// `noOfMinCounts`, `maxCalibRetries`
    fn motor_globals(&self) -> MotorParams;
    /// the counters of the USART1 RX interrupt; the firmware reads the four counters of
    /// `serial::Port` one after the other (C++ copied them with interrupts off,
    /// docs/rust/PORT-NOTES-STM.md)
    fn uart_errors(&mut self) -> UartErrorCounters;
}

/// RX interrupt of the ESP UART (C++ `comm_rx_irq`): the HAL error code of a byte and whether
/// it finds the receive ring full are counted; the serial port stores the byte. The firmware's
/// USART1 interrupt counts in `serial::Port::on_irq` with the same core function; this form
/// serves the suites.
pub fn comm_rx_irq(counters: &mut UartErrorCounters, hal_error_code: u32, ring_full: bool) {
    count_uart_errors(counters, hal_error_code, ring_full);
}

/// Stores a sensor address in the EEPROM sensor slot `stored` (slot number s for
/// eeprom_changed_slot); the EEPROM is marked only when the slot changes.
fn store_sensor_address(env: &mut impl CommunicationEnv, s: u8, address: &DeviceAddress) {
    let slot = SensorSlot {
        familycode: address[0],
        romcode: [
            address[1], address[2], address[3], address[4], address[5], address[6],
        ],
        crc: address[7],
    };
    let valve = usize::from(s % VALVE_COUNT);
    let layout = &mut env.eep_content().cfg.layout;
    let stored = if s < VALVE_COUNT {
        &mut layout.owsensors1[valve]
    } else {
        &mut layout.owsensors2[valve]
    };
    if *stored == slot {
        return;
    }
    *stored = slot;
    env.eeprom_changed_slot(s);
}

/// Parses one "xx-xx-xx-xx-xx-xx-xx-xx" address into the EEPROM sensor slot s: valid addresses
/// are stored, the all-zero address clears the slot, anything else is ignored.
fn set_valve_id_sensor(env: &mut impl CommunicationEnv, text: &[u8], s: u8) {
    let Some(address) = parse_one_wire_address(text) else {
        return;
    };
    if is_zero_address(&address) || DallasTemperature::valid_address(&address) {
        store_sensor_address(env, s, &address);
    }
}

/// `comm_set_valve_sensors`: both sensor slots of a valve (0..11) from their addresses (stvls);
/// 0, or -1 for an invalid valve.
pub fn comm_set_valve_sensors(
    env: &mut impl CommunicationEnv,
    valve: u16,
    first: &[u8],
    second: &[u8],
) -> i16 {
    if valve >= ACTUATOR_COUNT {
        return -1;
    }
    let v = valve as u8;
    set_valve_id_sensor(env, first, v);
    set_valve_id_sensor(env, second, VALVE_COUNT + v);
    0
}

/// `comm_set_valve_sensor_index`: sensor slot 1 or 2 of a valve takes the sensor at this index of
/// the gonec list (stsnx, stsny); 0, or -1 for an invalid valve, slot or sensor.
pub fn comm_set_valve_sensor_index(
    env: &mut impl CommunicationEnv,
    valve: u16,
    slot: u8,
    sensor: u16,
) -> i16 {
    let sensors = env.ow_sensors();
    if valve >= ACTUATOR_COUNT
        || sensor >= u16::from(sensors.no_of_ds18_devices)
        || usize::from(sensor) >= MAXONEWIRECNT
    {
        return -1;
    }
    let address = sensors.tempsensors[usize::from(sensor)].address;
    let v = valve as u8;
    match slot {
        1 => {
            store_sensor_address(env, v, &address);
            env.set_valve_sensor_index(valve, 1, sensor);
        }
        2 => {
            store_sensor_address(env, VALVE_COUNT + v, &address);
            env.set_valve_sensor_index(valve, 2, sensor);
        }
        _ => return -1,
    }
    0
}

/// `comm_set_learntime`: stlnt: the learn time to the app and into the EEPROM (marked only when
/// it changes); 0, or -1 if the app refuses it.
pub fn comm_set_learntime(env: &mut impl CommunicationEnv, seconds: u32) -> i16 {
    if env.app_set_learntime(seconds) != 0 {
        return -1;
    }
    let content = env.eep_content();
    if content.cfg.learn_time_s != seconds {
        content.cfg.learn_time_s = seconds;
        env.eeprom_changed(CHANGED_LEARN_TIME);
    }
    0
}

fn print_sensor_address(out: &mut impl Print, address: &DeviceAddress) {
    let mut text: StaticBufWriter<{ ONE_WIRE_ADDRESS_TEXT_LEN + 1 }> = BufWriter::default();
    if text.append_one_wire_address(address) {
        out.print(text.as_bytes());
    } else {
        out.print(NO_SENSOR_ADDRESS);
    }
}

/// the sensor of a valve slot: an index below MAXONEWIRECNT (the table size), else none
fn print_valve_sensor_address(out: &mut impl Print, sensors: &OwSensors, sensorindex: u32) {
    match sensors.tempsensors.get(sensorindex as usize) {
        Some(t) => print_sensor_address(out, &t.address),
        None => out.print(NO_SENSOR_ADDRESS),
    }
}

/// `comm_print_valve_sensor_ids`: the addresses of the two sensors of a valve (0..11) with the
/// delimiter between them ("00-..." without a sensor); nothing for another valve.
pub fn comm_print_valve_sensor_ids(
    env: &impl CommunicationEnv,
    out: &mut impl Print,
    valve: u16,
    delimiter: u8,
) {
    if valve >= ACTUATOR_COUNT {
        return;
    }
    let g = env.valve_globals(valve);
    print_valve_sensor_address(out, env.ow_sensors(), g.sensorindex1);
    out.print_char(delimiter);
    print_valve_sensor_address(out, env.ow_sensors(), g.sensorindex2);
}

/// the temperature of a valve slot: an index below MAXONEWIRECNT (the table size), else none
fn valve_temperature(sensors: &OwSensors, sensorindex: u32) -> i32 {
    sensors
        .tempsensors
        .get(sensorindex as usize)
        .map_or(NO_TEMPERATURE, |t| t.temperature)
}

/// argument i is a valve index or 255 (every valve)
fn arg_valve_or_all(req: &Tokenizer, i: u8) -> Option<u16> {
    req.arg_u16(i, 0, ACTUATOR_COUNT - 1)
        .or_else(|| req.arg_u16(i, ALL_VALVES, ALL_VALVES))
}

/// The hardware the dispatch writes to.
struct Io<'a, E, D, C, Y> {
    /// USART1 to the ESP (COMM_SER)
    esp: &'a mut E,
    /// the debug lines (COMM_DBG, USART6)
    dbg: &'a mut D,
    clock: &'a C,
    system: &'a Y,
}

/// The module state (the C++ statics of communication.cpp).
pub struct Communication {
    id: FirmwareId,
    comm_line: StaticLineAssembler<COMM_LINE_SIZE>,
    /// when comm_line consumed its last byte
    comm_last_byte_ms: u32,
    /// requests dropped for too many arguments (gstat parseErr)
    comm_too_many_args: u32,
    /// the longest reply (gprof)
    reply_line: StaticBufWriter<{ PROFILE_REPLY_MAX_LEN + 1 }>,
    /// gprof copy of the last profile (static in C++: off the stack)
    profile: ProfileRecorder,
}

// gvlvd, gvlvy and gstax fit the reply buffer
const _: () = assert!(VALVE_DATA_REPLY_MAX_LEN <= PROFILE_REPLY_MAX_LEN);
const _: () = assert!(VALVE_EXT_V3_REPLY_MAX_LEN <= PROFILE_REPLY_MAX_LEN);
const _: () = assert!(STAT_V3_REPLY_MAX_LEN <= PROFILE_REPLY_MAX_LEN);

impl Communication {
    pub fn new(id: FirmwareId) -> Self {
        Communication {
            id,
            comm_line: StaticLineAssembler::default(),
            comm_last_byte_ms: 0,
            comm_too_many_args: 0,
            reply_line: StaticBufWriter::default(),
            profile: ProfileRecorder::default(),
        }
    }

    /// `communication_setup`: the bytes received with the wrong framing during the 8E1 boot
    /// window are dropped. (The firmware sets USART1 to 115200 8N1 and counts the receive
    /// errors in its RX interrupt, `serial::Port::on_irq`.)
    pub fn setup(&mut self, esp: &mut impl Serial, dbg: &mut impl Print) {
        while esp.available() > 0 {
            esp.read();
        }
        self.comm_line.reset();
        dbg.print(b"SERIAL_BUFFER_SIZE TX=");
        dbg.print_signed(SERIAL_TX_BUFFER_SIZE as i32, DEC);
        dbg.print(b" RX=");
        dbg.println_signed(SERIAL_RX_BUFFER_SIZE as i32, DEC);
    }

    /// `communication_loop`: reads bytes from the ESP and executes complete requests, at most
    /// COMM_MAX_LINES requests and COMM_MAX_READ bytes per call; an unterminated line is dropped
    /// after COMM_LINE_TIMEOUT_MS without bytes. Returns 0 if at least one request was executed,
    /// otherwise -1.
    pub fn loop_(
        &mut self,
        esp: &mut impl Serial,
        dbg: &mut impl Print,
        clock: &impl Clock,
        system: &impl System,
        env: &mut impl CommunicationEnv,
    ) -> i16 {
        let mut io = Io {
            esp,
            dbg,
            clock,
            system,
        };
        let mut result = -1;
        let mut budget = COMM_MAX_READ;

        for _ in 0..COMM_MAX_LINES {
            // bytes following a complete line stay in the UART buffer for the next request
            while !self.comm_line.has_line() && budget > 0 && io.esp.available() > 0 {
                let c = io.esp.read().unwrap_or(0xFF);
                self.comm_line.push(c);
                self.comm_last_byte_ms = io.clock.millis();
                budget -= 1;
            }

            let mut line = [0u8; COMM_LINE_SIZE];
            let len = match self.comm_line.take_line() {
                Some(l) => {
                    line[..l.len()].copy_from_slice(l);
                    l.len()
                }
                None => break,
            };

            let mut req = Tokenizer::default();
            if req.parse(&line[..len], NO_OF_ARGS) {
                self.dispatch(&req, &mut io, env);
                result = 0;
            } else if req.too_many_args() {
                self.comm_too_many_args = self.comm_too_many_args.wrapping_add(1);
                io.dbg.println_text(b"comm: too many arguments");
            }
            self.comm_line.release();
        }

        // the rest of a partial line is read first: the main loop may have been blocked while it
        // arrived. Bytes of a line that stay unterminated (ESP restarted in the middle of a
        // request, noise) must not be glued to the next request.
        if io.esp.available() == 0
            && self.comm_line.expire(
                io.clock.millis(),
                self.comm_last_byte_ms,
                COMM_LINE_TIMEOUT_MS,
            )
        {
            io.dbg.println_text(b"comm: incomplete line dropped");
        }

        result
    }

    /// sends the reply formatted into reply_line (nothing if it did not fit)
    fn send_reply(&mut self, esp: &mut impl Serial, formatted: bool) {
        if formatted {
            esp.println_text(self.reply_line.as_bytes());
        }
        self.reply_line.clear();
    }

    /// the gstat values (also the first ones of gstax)
    fn fill_stat(&self, env: &mut impl CommunicationEnv) -> StatReply {
        StatReply {
            uptime_seconds: env.sysstat_uptime_s(),
            resets: env.sysstat_resets(),
            boot_reason: env.sysstat_boot_reason() as u8,
            rx_overflow: self.comm_line.overflow_count(),
            parse_errors: self
                .comm_line
                .malformed_count()
                .wrapping_add(self.comm_line.expired_count())
                .wrapping_add(self.comm_too_many_args),
            eeprom_state: env.eeprom_state(),
        }
    }

    fn dispatch<E: Serial, D: Print, C: Clock, Y: System>(
        &mut self,
        req: &Tokenizer,
        io: &mut Io<'_, E, D, C, Y>,
        env: &mut impl CommunicationEnv,
    ) {
        // the valve polls of an ESP that sends no lease commands (2.0.0, legacy) keep the lease
        // alive
        if req.is(APP_PRE_GETVLVDATA) || req.is(APP_PRE_GETVLVEXT) {
            env.app_lease_poll();
        }
        let valve = |i: u8| req.arg_u16(i, 0, ACTUATOR_COUNT - 1);

        if req.is(APP_PRE_SETTARGETPOS) {
            io.dbg.println_text(b"set target pos");
            match (req.argc() == 2, valve(0), req.arg_u8(1, 0, 100)) {
                (true, Some(x), Some(pos)) => {
                    // also taken while a calibration is requested or running: its final
                    // positioning reads the target when the calibration ends, and app_loop
                    // starts a requested calibration before it moves the valve to a new target
                    env.set_target_position(x, pos);
                    env.app_target_changed(x);
                    io.esp.println_text(APP_PRE_SETTARGETPOS);
                }
                _ => io.dbg.println_text(b"invalid arguments"),
            }
        } else if req.is(APP_PRE_GETTARGETPOS) {
            io.dbg.println_text(b"get target pos");
            match (req.argc() == 1, valve(0)) {
                (true, Some(x)) => {
                    io.esp.print(APP_PRE_GETTARGETPOS);
                    io.esp.print(b" ");
                    io.esp.print_signed(i32::from(x), DEC);
                    io.esp.print(b" ");
                    io.esp
                        .print_unsigned(u32::from(env.valve_globals(x).target_position), DEC);
                    io.esp.println_text(b" ");
                }
                _ => io.dbg.println_text(b"invalid arguments"),
            }
        } else if req.is(APP_PRE_GETVLVDATA) {
            match (req.argc() == 1, valve(0)) {
                (true, Some(x)) => {
                    let g = env.valve_globals(x);
                    let sensors = env.ow_sensors();
                    let data = ValveDataReply {
                        index: u32::from(x),
                        actual_position: i32::from(g.actual_position),
                        mean_current: g.meancurrent as i32,
                        // status | 0x80 while the calibration flag is set
                        status: i32::from(encode_valve_status(g.status, g.calibration)),
                        temperature1: valve_temperature(sensors, g.sensorindex1),
                        temperature2: valve_temperature(sensors, g.sensorindex2),
                        movements: g.movements as i32,
                        opening_count: g.opening_count as i32,
                        closing_count: g.closing_count as i32,
                        deadzone_count: g.deadzone_count,
                        calib_retries: i32::from(g.calib_retries),
                    };
                    // C++ formats into a buffer of VALVE_DATA_REPLY_MAX_LEN + 1: the reply buffer
                    // is larger, the reply always fits both
                    let ok = format_valve_data(&mut self.reply_line, APP_PRE_GETVLVDATA, &data);
                    self.send_reply(io.esp, ok);
                }
                _ => io.dbg.println_text(b"gvlvd: invalid arguments"),
            }
        } else if req.is(APP_PRE_GETVLSTATUS) {
            let mut status = [0u8; VALVE_COUNT as usize];
            for (v, s) in (0u16..).zip(status.iter_mut()) {
                *s = env.valve_globals(v).status;
            }
            let mut sendbuffer: StaticBufWriter<64> = BufWriter::default();
            io.dbg.println_text(b"cmd: get valve status");
            if format_status_list(&mut sendbuffer, APP_PRE_GETVLSTATUS, &status) {
                io.esp.println_text(sendbuffer.as_bytes());
            }
        } else if req.is(APP_PRE_GETONEWIRECNT) {
            let sensors = env.ow_sensors();
            let count = sensors.no_of_ds18_devices.min(MAXONEWIRECNT as u8);
            let addresses = sensors.tempsensors.iter().map(|t| &t.address);
            sensor_count_reply(io.esp, req, APP_PRE_GETONEWIRECNT, count, addresses);
        } else if req.is(APP_PRE_GETONEWIREDATA) {
            io.esp.print(APP_PRE_GETONEWIREDATA);
            io.esp.print(b" ");
            match (req.argc() == 1, req.arg_u16(0, 0, MAXONEWIRECNT as u16 - 1)) {
                (true, Some(x)) => {
                    let t = env.ow_sensors().tempsensors[usize::from(x)];
                    print_sensor_address(io.esp, &t.address);
                    io.esp.print(b" ");
                    io.esp.print_signed(t.temperature, DEC);
                }
                _ => io.esp.print(b"0"),
            }
            io.esp.println_text(b" ");
        } else if req.is(APP_PRE_GETONEWIRESETT) {
            io.dbg
                .print(b"cmd: get 1st and 2nd onewire sensor addresses");
            if let (true, Some(x)) = (req.argc() == 1, valve(0)) {
                io.esp.print(APP_PRE_GETONEWIRESETT);
                io.esp.print(b" ");
                // valve index
                io.esp.print_signed(i32::from(x), DEC);
                io.esp.print(b" ");
                comm_print_valve_sensor_ids(env, io.esp, x, b' ');
                io.esp.println_text(b" ");
            } else if let (true, Some(_)) = (req.argc() == 1, req.arg_u16(0, 255, 255)) {
                io.esp.print(APP_PRE_GETONEWIRESETT);
                io.esp.print(b" ");
                io.esp.print_signed(i32::from(ACTUATOR_COUNT), DEC);
                io.esp.print(b" ");
                for i in 0..ACTUATOR_COUNT {
                    comm_print_valve_sensor_ids(env, io.esp, i, b',');
                    if i < ACTUATOR_COUNT - 1 {
                        io.esp.print(b",");
                    }
                }
                io.esp.println_text(b" ");
            } else {
                io.dbg.println_text(b" - error");
                // v1 reports this error with the goned prefix, the ESP relies on it
                io.esp.print(APP_PRE_GETONEWIREDATA);
                io.esp.println_text(b" error ");
            }
        } else if req.is(APP_PRE_GETOWVOLTCNT) {
            let sensors = env.ow_sensors();
            let count = sensors.no_of_ds2438_devices.min(MAXDS2438CNT as u8);
            let addresses = sensors.voltsensors.iter().map(|v| &v.address);
            sensor_count_reply(io.esp, req, APP_PRE_GETOWVOLTCNT, count, addresses);
        } else if req.is(APP_PRE_GETOWVOLTDATA) {
            io.esp.print(APP_PRE_GETOWVOLTDATA);
            io.esp.print(b" ");
            match (req.argc() == 1, req.arg_u16(0, 0, MAXDS2438CNT as u16 - 1)) {
                (true, Some(x)) => {
                    let v = env.ow_sensors().voltsensors[usize::from(x)];
                    print_sensor_address(io.esp, &v.address);
                    io.esp.print(b" ");
                    io.esp.print_signed(v.vad, DEC);
                }
                _ => io.esp.print(b"0"),
            }
            io.esp.println_text(b" ");
        } else if req.is(APP_PRE_SETONEWIRESEARCH) {
            io.dbg.println_text(b"start new 1-wire search");
            env.temp_command(TEMP_CMD_NEWSEARCH);
            io.esp.println_text(APP_PRE_SETONEWIRESEARCH);
        } else if req.is(APP_PRE_SETLEARNTIME) {
            io.dbg.print(b"set valve learning time to ");
            match (req.argc() == 1, req.arg_u32(0, 0, u32::MAX)) {
                (true, Some(xu32)) if comm_set_learntime(env, xu32) == 0 => {
                    io.esp.println_text(APP_PRE_SETLEARNTIME);
                    io.dbg.println_unsigned(xu32, DEC);
                }
                _ => io.dbg.println_text(b"- error"),
            }
        } else if req.is(APP_PRE_SETLEARNMOVEM) {
            io.dbg.print(b"set valve learning movements to ");
            // the ESP sends a uint32; v1 always replied, so a value outside 0, 50..65534 is moved
            // to the nearest bound instead of being rejected (the same range is loaded at
            // start-up)
            let valid = match (req.argc() == 1, req.arg_u32(0, 0, u32::MAX)) {
                (true, Some(xu32)) => Some(learn_movements_from_request(xu32)),
                _ => None,
            };
            // every request restarts the movement counters of all valves (v1), the EEPROM is
            // written only for a new value
            match valid {
                Some(x) if env.app_set_learnmovements(x) == 0 => {
                    io.dbg.println_signed(i32::from(x), DEC);
                    let content = env.eep_content();
                    if content.cfg.layout.number_of_movements != x {
                        content.cfg.layout.number_of_movements = x;
                        env.eeprom_changed(CHANGED_MOVEMENTS);
                    }
                    io.esp.println_text(APP_PRE_SETLEARNMOVEM);
                }
                _ => io.dbg.println_text(b"- error"),
            }
        } else if req.is(APP_PRE_GETLEARNMOVEM) {
            io.dbg.print(b"get valve learning movements");
            io.esp.print(APP_PRE_GETLEARNMOVEM);
            io.esp.print(b" ");
            io.esp.print_unsigned(env.learning_movements(), DEC);
            io.esp.println_text(b" ");
        } else if req.is(b"ESPalive") {
            io.dbg.println_text(b"received ESPalive");
        } else if req.is(APP_PRE_SET1STSENSORINDEX) || req.is(APP_PRE_SET2NDSENSORINDEX) {
            let first = req.is(APP_PRE_SET1STSENSORINDEX);
            io.dbg.println_text(if first {
                b"comm: set 1st sensor index"
            } else {
                b"comm: set 2nd sensor index"
            });
            let slot = if first { 1 } else { 2 };
            // C++ parses the sensor as 0..MAXONEWIRECNT - 1 here; comm_set_valve_sensor_index
            // refuses the same indexes before it touches anything, so any 16-bit number is
            // parsed and the answer is the same
            match (req.argc() == 2, valve(0), req.arg_u16(1, 0, u16::MAX)) {
                (true, Some(x), Some(y)) if comm_set_valve_sensor_index(env, x, slot, y) == 0 => {
                    io.esp.println_text(if first {
                        APP_PRE_SET1STSENSORINDEX
                    } else {
                        APP_PRE_SET2NDSENSORINDEX
                    });
                }
                _ => io.dbg.println_text(b"invalid arguments"),
            }
        } else if req.is(APP_PRE_SETVLVSENSOR) {
            match (req.argc() == 3, valve(0)) {
                (true, Some(x)) => {
                    io.dbg.println_text(b"comm: set valve sensors");
                    comm_set_valve_sensors(env, x, req.arg(1), req.arg(2));
                    io.esp.print(APP_PRE_SETVLVSENSOR);
                    io.esp.print(b" ");
                    io.esp.println_signed(i32::from(x), DEC);
                }
                _ => io.dbg.println_text(b"invalid arguments"),
            }
        } else if req.is(APP_PRE_SETALLVLVOPEN) {
            io.dbg.print(b"got open valve request for ");
            match (req.argc() == 1, req.arg_u16(0, 0, u16::MAX)) {
                (true, Some(x)) if env.app_set_valveopen(x) == 0 => {
                    io.esp.print(APP_PRE_SETALLVLVOPEN);
                    io.esp.println_text(b" ");
                    io.dbg.println_signed(i32::from(x), DEC);
                }
                _ => io.dbg.println_text(b"- error"),
            }
        } else if req.is(APP_PRE_SETVLLEARN) {
            io.dbg.print(b"start learning for valve ");
            match (req.argc() == 1, req.arg_u16(0, 0, u16::MAX)) {
                (true, Some(x)) if env.app_set_valvelearning(x) == 0 => {
                    io.esp.println_text(APP_PRE_SETVLLEARN);
                    io.dbg.println_signed(i32::from(x), DEC);
                }
                _ => io.dbg.println_text(b"- error"),
            }
        } else if req.is(APP_PRE_SETMOTCHARS) {
            self.set_motor_characteristics(req, io, env);
        } else if req.is(APP_PRE_GETMOTCHARS) {
            io.dbg
                .println_text(b"got get motor characteristics request ");
            let m = env.motor_globals();
            io.esp.print(APP_PRE_GETMOTCHARS);
            for v in [
                u32::from(m.low_fac),
                u32::from(m.high_fac),
                u32::from(m.start_on_power),
                u32::from(m.min_counts),
                u32::from(m.max_retries),
            ] {
                io.esp.print(b" ");
                io.esp.print_unsigned(v, DEC);
            }
            io.esp.println_text(b" ");
        } else if req.is(APP_PRE_SETDETECTVLV) {
            // stdet 255 tests every valve again; another number is answered with "stdet err"
            // (v1 confirmed it without doing anything)
            io.dbg.print(b"got detect valve status request");
            match (req.argc() == 1, req.arg_u16(0, 0, u16::MAX)) {
                (true, Some(255)) => {
                    env.app_scan_valves();
                    io.dbg.println_text(b" - reset all valves");
                    io.esp.print(APP_PRE_SETDETECTVLV);
                    io.esp.println_text(b" ");
                }
                (true, Some(_)) => {
                    io.dbg.println_text(b" - error");
                    let ok = format_result(&mut self.reply_line, APP_PRE_SETDETECTVLV, false);
                    self.send_reply(io.esp, ok);
                }
                _ => io.dbg.println_text(b" - error"),
            }
        } else if req.is(APP_PRE_GETVERSION) {
            // "gvers <version>_<board revision> <build> "
            io.dbg.println_text(b"got version request");
            io.esp.print(APP_PRE_GETVERSION);
            io.esp.print(b" ");
            io.esp.print(self.id.version);
            io.esp.print(b"_");
            io.esp.print(self.id.tag);
            io.esp.print(b" ");
            io.esp.print(self.id.build);
            io.esp.println_text(b" ");
        } else if req.is(APP_PRE_GETHWINFO) {
            io.dbg.println_text(b"got hw info request");
            io.esp.print(APP_PRE_GETHWINFO);
            io.esp.print(b" ");
            io.esp.print_unsigned(u32::from(io.system.dev_id()), DEC);
            io.esp.println_text(b" ");
        } else if req.is(APP_PRE_MATCHSENS) {
            io.dbg.println_text(b"got match sensors request");
            env.app_match_sensors();
            io.esp.print(APP_PRE_MATCHSENS);
            io.esp.println_text(b" ");
        } else if req.is(APP_PRE_SOFTRESET) {
            io.dbg.println_text(b"got software reset request");
            io.esp.print(APP_PRE_SOFTRESET);
            io.esp.println_text(b" ");
            io.clock.delay_ms(200);
            env.reset_stm32();
        } else if req.is(APP_PRE_EEPSTATE) {
            io.dbg.println_text(b"got get eeprom status request ");
            io.esp.print(APP_PRE_EEPSTATE);
            io.esp.print(b" ");
            io.esp
                .print_unsigned(u32::from(eepst_saved(env.eeprom_state())), DEC);
            io.esp.println_text(b" ");
        } else {
            self.dispatch_v2(req, io, env);
        }
    }

    /// smotc low high startOnPower [noOfMinCounts [maxCalibRetries]]: every value is checked
    /// against the range table (gmotx); the values in range are applied, a value out of range
    /// leaves its field unchanged and the reply is "smotc err" (a factor of 41..50 is applied as
    /// 40); fewer than 3 values or a value that is not a number changes nothing ("smotc err")
    fn set_motor_characteristics<E: Serial, D: Print, C: Clock, Y: System>(
        &mut self,
        req: &Tokenizer,
        io: &mut Io<'_, E, D, C, Y>,
        env: &mut impl CommunicationEnv,
    ) {
        let mut values = [0u32; 5];
        let current = env.motor_get_params();
        let mut params = current;
        let mut result = ParamsRequest::Rejected;
        let argc = req.argc();
        let mut numbers = (3..=5).contains(&argc);

        io.dbg.print(b"got set motor characteristics request ");

        for (i, v) in (0..argc).zip(values.iter_mut()) {
            if !numbers {
                break;
            }
            match req.arg_u32(i, 0, u32::MAX) {
                Some(x) => *v = x,
                None => numbers = false,
            }
        }
        if numbers {
            result = apply_motor_params_request(&mut params, argc, &values);
        }

        if result != ParamsRequest::Rejected && !same_motor_params(&params, &current) {
            env.motor_set_params(&params);
            env.eeprom_changed(CHANGED_MOTOR);
        }

        if result == ParamsRequest::Applied {
            io.esp.println_text(APP_PRE_SETMOTCHARS);
            io.dbg.println_text(b"- valid");
        } else {
            let ok = format_result(&mut self.reply_line, APP_PRE_SETMOTCHARS, false);
            self.send_reply(io.esp, ok);
            io.dbg.println_text(b"- invalid arguments");
        }
    }

    /// the gvlvx values of valve x (also the first ones of gvlvy)
    fn fill_valve_ext(x: u16, env: &mut impl CommunicationEnv) -> ValveExtReply {
        // one copy: a calibration pass or the end of a move changes several fields at once
        let snap = env.valve_get_snapshot(x);
        let requested = env.app_learn_pending(x, snap.status, snap.calibration);
        ValveExtReply {
            index: x as u8,
            status: encode_valve_status(snap.status, snap.calibration),
            position: snap.actual_position,
            target: snap.target_position,
            mean_current: snap.meancurrent.min(0xFFFF) as u16,
            opening_count: snap.opening_count,
            closing_count: snap.closing_count,
            deadzone_count: snap.deadzone_count,
            calib_retries: snap.calib_retries,
            movements: snap.movements,
            cal_state: compose_cal_state(
                snap.calib_active,
                requested,
                snap.diag.early_warn,
                snap.diag.last_cal_failed,
            ),
            early_stops: snap.diag.early_stops,
            cmd_rejected: env.valve_globals(x).cmd_rejected,
            last: snap.diag.last,
        }
    }

    /// Stores the failsafe position of one valve or of all (255) and applies it; while the
    /// EEPROM read has failed the mirror holds the default 50, not the stored positions, so
    /// every position is marked: the re-read takes over the marked fields only.
    fn set_failsafe(valve: u16, pct: u8, env: &mut impl CommunicationEnv) {
        let mut changed = false;
        for (v, stored) in (0u16..).zip(env.eep_content().cfg.failsafe_pct.iter_mut()) {
            if (valve == v || valve == ALL_VALVES) && *stored != pct {
                *stored = pct;
                changed = true;
            }
        }
        if changed || env.eeprom_state() == EEP_STATE_READ_FAILED {
            env.eeprom_changed(CHANGED_FAILSAFE);
        }
        env.app_set_failsafe(valve, pct);
    }

    /// the commands of protocol 2 and 3
    fn dispatch_v2<E: Serial, D: Print, C: Clock, Y: System>(
        &mut self,
        req: &Tokenizer,
        io: &mut Io<'_, E, D, C, Y>,
        env: &mut impl CommunicationEnv,
    ) {
        let valve = |i: u8| req.arg_u16(i, 0, ACTUATOR_COUNT - 1);

        let formatted = if req.is(APP_PRE_GETPROTOCOL) {
            format_protocol_version(&mut self.reply_line)
        } else if req.is(APP_PRE_GETVLVEXT) {
            // gvlvx idx status pos target meanCur oc cc dc cr moves calState earlyStops
            // cmdRejected lastDir lastReq lastCnt lastStop lastPeak lastMs
            match (req.argc() == 1, valve(0)) {
                (true, Some(x)) => {
                    let data = Self::fill_valve_ext(x, env);
                    format_valve_ext(&mut self.reply_line, &data)
                }
                _ => {
                    io.dbg.println_text(b"gvlvx: invalid arguments");
                    return;
                }
            }
        } else if req.is(APP_PRE_GETPROFILE) {
            // current profile of the last move: gprof idx n c1:m1 ... cn:mn
            match (req.argc() == 1, valve(0)) {
                (true, Some(x)) => {
                    env.valve_get_profile(x, &mut self.profile);
                    format_profile(&mut self.reply_line, x as u8, &self.profile)
                }
                _ => {
                    io.dbg.println_text(b"gprof: invalid arguments");
                    return;
                }
            }
        } else if req.is(APP_PRE_SERVICEMOVE) {
            // svmov idx dir counts maxmA -> "svmov idx ok" / "svmov idx err code"; code 1:
            // invalid arguments, 2: valve state machine busy, 3: calibration of the valve pending
            let index = match (req.argc() >= 1, valve(0)) {
                (true, Some(x)) => Some(x),
                _ => None,
            };
            let mut error = 1;
            if let (Some(x), true, Some(dir), Some(counts), Some(max_ma)) = (
                index,
                req.argc() == 4,
                req.arg_u8(1, DIR_OPEN, DIR_CLOSE),
                req.arg_u16(2, SVMOV_COUNTS_MIN, SVMOV_COUNTS_MAX),
                req.arg_u8(3, SVMOV_MAXMA_MIN, SVMOV_MAXMA_MAX),
            ) {
                error = match env.app_service_move(x, dir, counts, max_ma) {
                    0 => 0,
                    SVMOV_CALIB_PENDING => 3,
                    _ => 2,
                };
            }
            let index = index.map_or(-1, i32::from);
            format_indexed_result(&mut self.reply_line, APP_PRE_SERVICEMOVE, index, error)
        } else if req.is(APP_PRE_SETCALESC) {
            // breakaway escalation: scalx enable stepPct maxmA
            let config = match (
                req.argc() == 3,
                req.arg_u8(0, 0, 1),
                req.arg_u8(1, 0, ESCALATION_STEP_MAX),
                req.arg_u8(2, ESCALATION_MAX_MA_MIN, ESCALATION_MAX_MA_MAX),
            ) {
                (true, Some(enable), Some(step_pct), Some(max_ma)) => Some(EscalationConfig {
                    enable,
                    step_pct,
                    max_ma,
                }),
                _ => None,
            };
            if let Some(config) = config {
                let changed = env.eep_content().cfg.escalation != config;
                env.motor_set_escalation(&config);
                if changed {
                    env.eeprom_changed(CHANGED_ESCALATION);
                }
            }
            format_result(&mut self.reply_line, APP_PRE_SETCALESC, config.is_some())
        } else if req.is(APP_PRE_GETCALESC) {
            let config = env.motor_get_escalation();
            format_escalation(&mut self.reply_line, &config)
        } else if req.is(APP_PRE_GETSTATUS) {
            // health: gstat uptime_s resets bootReason rxOverflow parseErr eepState
            let stat = self.fill_stat(env);
            format_stat(&mut self.reply_line, &stat)
        } else if req.is(APP_PRE_GETMOTLIMITS) {
            format_motor_limits(&mut self.reply_line)
        } else if req.is(APP_PRE_LEASEHEARTBEAT) {
            // lease heartbeat: slhbt alive (0/1) -> "slhbt lease remainS" / "slhbt err"
            match (req.argc() == 1, req.arg_u8(0, 0, 1)) {
                (true, Some(alive)) => {
                    env.app_lease_heartbeat(alive != 0);
                    let lease = env.app_lease_state();
                    let remain = env.app_lease_remaining_s();
                    format_heartbeat(&mut self.reply_line, lease, remain)
                }
                _ => format_result(&mut self.reply_line, APP_PRE_LEASEHEARTBEAT, false),
            }
        } else if req.is(APP_PRE_SETLEASE) {
            // lease timeout: slcfg minutes (0 = off, 5..1440), stored in the EEPROM; no renewal
            let minutes = match (req.argc() == 1, req.arg_u32(0, 0, u32::MAX)) {
                (true, Some(m)) if lease_timeout_valid(m) => Some(m as u16),
                _ => None,
            };
            if let Some(m) = minutes {
                // while the EEPROM read has failed the mirror holds the default 60, not the
                // stored timeout: marked also when equal, the re-read takes over the marked
                // fields only
                if env.eep_content().cfg.lease_timeout_min != m
                    || env.eeprom_state() == EEP_STATE_READ_FAILED
                {
                    env.eep_content().cfg.lease_timeout_min = m;
                    env.eeprom_changed(CHANGED_LEASE);
                }
                env.app_lease_configure(m);
                env.app_lease_command();
            }
            format_result(&mut self.reply_line, APP_PRE_SETLEASE, minutes.is_some())
        } else if req.is(APP_PRE_SETFAILSAFE) {
            // failsafe position: sfspo idx|255 pct (0..100, 255 = hold), stored in the EEPROM
            // -> "sfspo idx ok" / "sfspo idx err 1" / "sfspo -1 err 1" (no valid index)
            let index = if req.argc() >= 1 {
                arg_valve_or_all(req, 0)
            } else {
                None
            };
            let mut error = 1;
            if let (Some(x), true, Some(pct)) = (index, req.argc() == 2, req.arg_u8(1, 0, 255)) {
                if failsafe_pct_valid(u32::from(pct)) {
                    Self::set_failsafe(x, pct, env);
                    env.app_lease_command();
                    error = 0;
                }
            }
            let index = index.map_or(-1, i32::from);
            format_indexed_result(&mut self.reply_line, APP_PRE_SETFAILSAFE, index, error)
        } else if req.is(APP_PRE_GETLEASE) {
            // lease timeout and failsafe positions: "glcfg timeoutMin fs0 ... fs11"
            let mut fs = [0u8; VALVE_COUNT as usize];
            env.app_lease_command();
            for (v, pct) in (0u16..).zip(fs.iter_mut()) {
                *pct = env.app_failsafe_pct(v);
            }
            let timeout = env.app_lease_timeout();
            format_lease_config(&mut self.reply_line, timeout, &fs)
        } else if req.is(APP_PRE_GETVLVEXT3) {
            // gvlvx plus flags fault fsPct drive retryS retries
            match (req.argc() == 1, valve(0)) {
                (true, Some(x)) => {
                    let base = Self::fill_valve_ext(x, env);
                    let info = env.app_get_valve_v3(x);
                    let data = ValveExtV3Reply {
                        base,
                        flags: info.flags,
                        fault: info.fault,
                        failsafe_pct: info.fs_pct,
                        drive: info.drive,
                        retry_s: info.retry_s,
                        retries: info.retries,
                    };
                    format_valve_ext_v3(&mut self.reply_line, &data)
                }
                _ => {
                    io.dbg.println_text(b"gvlvy: invalid arguments");
                    return;
                }
            }
        } else if req.is(APP_PRE_GETSTATUS3) {
            // gstat plus lease, safe mode, UART errors, EEPROM load and writes, temperature
            // ages, sysFlags
            let base = self.fill_stat(env);
            let lease = env.app_lease_state();
            let lease_remain_s = env.app_lease_remaining_s();
            let lease_client = u8::from(env.app_lease_client());
            let lease_timeout_min = env.app_lease_timeout();
            let failsafe_mask = env.app_failsafe_mask();
            let safe_mode = u8::from(env.sysstat_safe_mode());
            let wdg_resets = env.sysstat_wdg_resets();
            let uart = env.uart_errors();
            let cfg_flags = env.eeprom_cfg_flags();
            let cfg_events = env.eeprom_cfg_events();
            let eep_writes = env.eeprom_writes();
            let temp_age_s = env.app_temp_age_s();
            let ow_scan_age_s = env.ow_scan_age_s();
            let sys_flags = if env.app_protect_suspended() {
                SYS_FLAG_PROTECT_SUSPENDED
            } else {
                0
            };
            let stat = StatV3Reply {
                base,
                lease,
                lease_remain_s,
                lease_client,
                lease_timeout_min,
                failsafe_mask,
                safe_mode,
                wdg_resets,
                uart_ore: uart.overrun,
                uart_fe: uart.framing,
                uart_ne: uart.noise,
                rx_dropped: uart.dropped,
                cfg_flags,
                cfg_events,
                eep_writes,
                temp_age_s,
                ow_scan_age_s,
                sys_flags,
            };
            format_stat_v3(&mut self.reply_line, &stat)
        } else if req.is(APP_PRE_STOP) {
            // stop: sstop idx|255 -> "sstop idx ok" (also when nothing ran) / "sstop -1 err 1"
            let stopped = match (req.argc() == 1, arg_valve_or_all(req, 0)) {
                (true, Some(x)) if env.app_stop(x) == 0 => Some(x),
                _ => None,
            };
            let (index, error) = stopped.map_or((-1, 1), |x| (i32::from(x), 0));
            format_indexed_result(&mut self.reply_line, APP_PRE_STOP, index, error)
        } else if req.is(APP_PRE_GETLEARNTIME) {
            // stored learn time: "gtlnt seconds" (0 = time trigger off)
            let seconds = env.app_get_learntime();
            format_learn_time(&mut self.reply_line, seconds)
        } else if req.is(APP_PRE_SAFEMODE) {
            // leave safe mode: ssafe 0 -> "ssafe ok" / "ssafe err"
            let valid = req.argc() == 1 && req.arg_u8(0, 0, 0).is_some();
            if valid {
                env.sysstat_leave_safe_mode();
            }
            format_result(&mut self.reply_line, APP_PRE_SAFEMODE, valid)
        } else {
            // unknown commands are ignored without reply (protocol v1)
            return;
        };
        self.send_reply(io.esp, formatted);
    }
}

/// gonec / gowvc: "<cmd> n " without arguments; with 255 the list of addresses
/// "<cmd> n a1,a2,... " (no space before an empty list); anything else gets no reply.
fn sensor_count_reply<'a>(
    esp: &mut impl Serial,
    req: &Tokenizer,
    cmd: &[u8],
    count: u8,
    addresses: impl Iterator<Item = &'a DeviceAddress>,
) {
    if req.argc() == 0 {
        esp.print(cmd);
        esp.print(b" ");
        esp.print_unsigned(u32::from(count), DEC);
        esp.println_text(b" ");
    } else if req.argc() == 1 && req.arg_u16(0, ALL_SENSORS, ALL_SENSORS).is_some() {
        // the whole list of the detected sensors
        esp.print(cmd);
        esp.print(b" ");
        esp.print_unsigned(u32::from(count), DEC);
        if count > 0 {
            esp.print(b" ");
        }
        for (i, address) in addresses.take(usize::from(count)).enumerate() {
            print_sensor_address(esp, address);
            if i + 1 < usize::from(count) {
                esp.print(b",");
            }
        }
        esp.println_text(b" ");
    }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_mut;
#[cfg(test)]
mod tests_v1;
#[cfg(test)]
mod tests_v3;
