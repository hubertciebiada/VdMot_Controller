//! The debug terminal on USART6 (`software_stm32/src/terminal.cpp`, `include/terminal.h`), ported
//! 1:1 behind the cargo feature `terminal` (docs/rust/GLUE-DESIGN-STM.md §4.2, D7): a tokenizer
//! terminal (128-byte lines, 3 arguments, at most 256 bytes per call), the start banner and the
//! limits of its hardware access (S10): a motor output switched on by `sena` goes off after 2 s
//! or above 60 mA, `smux`/`sdir`/`sena` only while the valve machine is idle.

use crate::communication::FirmwareId;
use crate::eeprom::EepromLayout;
use crate::hal::{Clock, Out, Pins, Serial};
use crate::ow_devices::TEMP_CMD_NEWSEARCH;
use crate::print::{Print, DEC};
use vdm_stm_core::config_store::{CHANGED_ALL, CHANGED_MOTOR};
use vdm_stm_core::line_assembler::StaticLineAssembler;
use vdm_stm_core::manual_enable::manual_enable_expired;
use vdm_stm_core::motor_params::{
    apply_motor_params_request, same_motor_params, MotorParams, ParamsRequest,
};
use vdm_stm_core::replies_v2::EEP_STATE_READ_FAILED;
use vdm_stm_core::tokenizer::Tokenizer;

// results of Terminal_Serve (terminal.h)
pub const CMD_NONE: i16 = 0x00;
pub const CMD_HELP: i16 = 0x01;
pub const CMD_CLOSE: i16 = 0x02;
pub const CMD_OPEN: i16 = 0x03;
pub const CMD_LEARN: i16 = 0x04;

/// valve machine commands (motor.h CMD_A_*)
pub const CMD_A_OPEN: u8 = b'o';
pub const CMD_A_CLOSE: u8 = b'c';
pub const CMD_A_LEARN: u8 = b'l';

/// number of allowed command arguments
const TERM_ARG_CNT: u8 = 3;
/// max length of one terminal line
const TERM_LINE_SIZE: usize = 128;
/// bytes taken from the terminal per call
const TERM_MAX_READ: u16 = 256;
/// motor outputs ENA0..ENA5 (sena)
const TERM_ENA_CHANNELS: u16 = 6;
/// SYSTEM_NAME (hardware.h), the description seteep writes
pub const SYSTEM_NAME: &[u8] = b"VdMot Controller";
/// valves (ACTUATOR_COUNT)
const ACTUATOR_COUNT: u16 = 12;

/// Calls of terminal.cpp into other glue modules, and the globals of those modules it reads or
/// writes (plain data in C++: the stubs do not log them).
pub trait TerminalEnv {
    // ---- motor.cpp
    /// `appsetaction(cmd, valve, pos)` (force false, flags 0): 0 if the valve machine took it
    fn appsetaction(&mut self, cmd: u8, valve: u16, pos: u8) -> i16;
    /// idle and no command pending
    fn valve_idle(&mut self) -> bool;
    fn motor_get_params(&mut self) -> MotorParams;
    fn motor_set_params(&mut self, params: &MotorParams);
    // ---- sysstat.cpp
    fn sysstat_safe_mode(&mut self) -> bool;
    // ---- owDevices.cpp
    fn print_sensordata(&mut self, out: &mut impl Print);
    fn temp_command(&mut self, command: i32);
    // ---- eeprom.cpp
    fn eeprom_state(&mut self) -> u8;
    fn eeprom_changed(&mut self, fields: u16);
    // ---- communication.cpp
    fn comm_set_valve_sensor_index(&mut self, valve: u16, slot: u8, sensor: u16) -> i16;
    fn comm_set_valve_sensors(&mut self, valve: u16, first: &[u8], second: &[u8]) -> i16;
    fn comm_print_valve_sensor_ids(&mut self, out: &mut impl Print, valve: u16, delimiter: u8);
    fn comm_set_learntime(&mut self, seconds: u32) -> i16;
    // ---- app.cpp
    fn app_match_sensors(&mut self) -> i16;
    fn app_set_valveopen(&mut self, valve: u16) -> i16;
    fn app_set_valvelearning(&mut self, valve: u16) -> i16;
    fn app_scan_valves(&mut self);
    // ---- globals of other modules
    /// `myvalvemots[valve].target_position = pos` (motor.cpp)
    fn set_target_position(&mut self, valve: u16, pos: u8);
    /// `currentbound_low_fac`, `currentbound_high_fac`, ... (motor.cpp)
    fn motor_globals(&self) -> MotorParams;
    /// `eep_content` (eeprom.cpp)
    fn eep_content(&mut self) -> &mut EepromLayout;
    /// `analog_current` (motor.cpp): the filtered motor current in 0.1 mA
    fn analog_current(&self) -> i32;
}

/// The module state (the C++ statics of terminal.cpp).
pub struct Terminal {
    id: FirmwareId,
    /// the MUX level of "on" (C1: high, C2: low; board `BoardRev::mux_on_high`)
    mux_on_high: bool,
    term_line: StaticLineAssembler<TERM_LINE_SIZE>,
    /// a motor output switched on by sena: switched off by supervise() after the time or at
    /// the current limit, the valve machine gets no command meanwhile
    manual_active: bool,
    manual_channel: u8,
    manual_start_ms: u32,
}

/// The output of an ENA channel (0..4; anything above is ENA5).
fn ena(ch: u8) -> Out {
    match ch {
        0 => Out::Ena0,
        1 => Out::Ena1,
        2 => Out::Ena2,
        3 => Out::Ena3,
        4 => Out::Ena4,
        _ => Out::Ena5,
    }
}

/// `PSU_ON()` / `PSU_OFF()`: the valve supply (open drain, low = on)
fn psu(pins: &impl Pins, on: bool) {
    pins.set(Out::PsuEna, !on);
}

impl Terminal {
    pub fn new(id: FirmwareId, mux_on_high: bool) -> Self {
        Terminal {
            id,
            mux_on_high,
            term_line: StaticLineAssembler::default(),
            manual_active: false,
            manual_channel: 0,
            manual_start_ms: 0,
        }
    }

    /// `Terminal_Init`: the banner with version and board revision. (The firmware sets USART6
    /// up: PA12/PA11, 115200 8N1.)
    pub fn init(&mut self, dbg: &mut impl Serial) -> i16 {
        dbg.print(b"VdMot Controller ");
        dbg.print(self.id.version);
        dbg.print(b"_");
        dbg.println_text(self.id.tag);
        dbg.flush();
        0
    }

    /// ends a sena: output and valve PSU off
    fn manual_off(&mut self, pins: &impl Pins) {
        pins.set(ena(self.manual_channel), false);
        psu(pins, false);
        self.manual_active = false;
    }

    /// `Terminal_Serve`: reads at most TERM_MAX_READ bytes and executes one complete line;
    /// the command code, -1 if no complete line was received.
    pub fn serve(
        &mut self,
        dbg: &mut impl Serial,
        pins: &impl Pins,
        clock: &impl Clock,
        env: &mut impl TerminalEnv,
    ) -> i16 {
        let mut budget = TERM_MAX_READ;
        while !self.term_line.has_line() && budget > 0 && dbg.available() > 0 {
            let c = dbg.read().unwrap_or(0xFF);
            self.term_line.push(c);
            budget -= 1;
        }

        let mut line = [0u8; TERM_LINE_SIZE];
        let len = match self.term_line.take_line() {
            Some(l) => {
                line[..l.len()].copy_from_slice(l);
                l.len()
            }
            None => return -1,
        };

        let mut req = Tokenizer::default();
        let parsed = req.parse(&line[..len], TERM_ARG_CNT);
        let mut result = -1;
        if parsed {
            result = self.execute(&req, dbg, pins, clock, env);
        } else if req.too_many_args() {
            dbg.println_text(b"too many arguments");
        }

        self.term_line.release();
        result
    }

    fn execute(
        &mut self,
        req: &Tokenizer,
        dbg: &mut impl Serial,
        pins: &impl Pins,
        clock: &impl Clock,
        env: &mut impl TerminalEnv,
    ) -> i16 {
        // most commands take up to two numeric arguments
        let x = if req.argc() >= 1 {
            req.arg_u16(0, 0, u16::MAX)
        } else {
            None
        };
        let y = if req.argc() >= 2 {
            req.arg_u16(1, 0, u16::MAX)
        } else {
            None
        };
        let argc = req.argc();

        if req.is(b"help") {
            dbg.println_text(b"Help:");
            dbg.println_text(b"*********************");
            return CMD_HELP;
        } else if req.is(b"learn") {
            match (argc == 1, x) {
                (true, Some(x)) => {
                    if env.appsetaction(CMD_A_LEARN, x, 0) != 0 {
                        dbg.println_text(b"valve machine command not accepted");
                    }
                }
                _ => dbg.println_text(b"to few arguments"),
            }
            return CMD_LEARN;
        } else if req.is(b"open") || req.is(b"close") {
            let open = req.is(b"open");
            match (argc == 2, x, y) {
                (true, Some(x), Some(y)) if y <= 100 => {
                    let cmd = if open { CMD_A_OPEN } else { CMD_A_CLOSE };
                    if env.appsetaction(cmd, x, y as u8) != 0 {
                        dbg.println_text(b"valve machine not idle");
                    }
                }
                _ => dbg.println_text(b"to few arguments"),
            }
            return if open { CMD_OPEN } else { CMD_CLOSE };
        } else if req.is(b"settar") {
            match (argc == 2, x, y) {
                (true, Some(x), Some(y)) => {
                    if x < ACTUATOR_COUNT && y <= 100 {
                        env.set_target_position(x, y as u8);
                        dbg.print(b"set valve ");
                        dbg.print_signed(i32::from(x), 10);
                        dbg.print(b" to ");
                        dbg.println_signed(i32::from(y), DEC);
                    }
                }
                _ => dbg.println_text(b"to few arguments"),
            }
            return CMD_CLOSE;
        } else if req.is(b"smux") || req.is(b"sdir") {
            // not while the valve machine works
            if let (true, Some(x)) = (argc == 1, x) {
                if !env.valve_idle() {
                    dbg.println_text(b"valve machine busy");
                } else if req.is(b"smux") {
                    // MUX_ON() / MUX_OFF()
                    pins.set(Out::Mux, (x != 0) == self.mux_on_high);
                } else {
                    // DIR_ON() / DIR_OFF()
                    pins.set(Out::Dir, x != 0);
                }
            }
        } else if req.is(b"sena") {
            // sena ch 1 switches the valve PSU and output ch on for at most 2 s (less when the
            // current exceeds 60 mA), only while the valve machine is idle and not in safe mode;
            // sena ch 0 switches the output off
            if let (true, Some(x), Some(y)) = (argc == 2, x, y) {
                if x < TERM_ENA_CHANNELS {
                    let ch = x as u8;
                    if y == 0 {
                        if self.manual_active && self.manual_channel == ch {
                            self.manual_off(pins);
                        } else {
                            pins.set(ena(ch), false);
                        }
                    } else if !env.valve_idle() || env.sysstat_safe_mode() {
                        dbg.println_text(b"valve machine busy");
                    } else {
                        if self.manual_active {
                            self.manual_off(pins);
                        }
                        self.manual_channel = ch;
                        self.manual_start_ms = clock.millis();
                        self.manual_active = true;
                        psu(pins, true);
                        pins.set(ena(ch), true);
                    }
                }
            }
        } else if req.is(b"getone") {
            env.print_sensordata(dbg);
        } else if req.is(b"seteep") {
            // RAM holds fallbacks while the EEPROM could not be read: a write would destroy the
            // stored layout
            if env.eeprom_state() == EEP_STATE_READ_FAILED {
                dbg.println_text(b"eeprom not readable, write blocked");
            } else {
                Self::write_eeprom_baselayout(dbg, env);
                dbg.println_text(b"set eeprom layout");
            }
        } else if req.is(b"saveep") {
            env.eeprom_changed(CHANGED_ALL);
            dbg.println_text(b"saved eeprom layout");
        } else if req.is(b"stsnx") || req.is(b"stsny") {
            let first = req.is(b"stsnx");
            match (argc == 2, x, y) {
                (true, Some(x), Some(y)) => {
                    dbg.println_text(if first {
                        b"comm: set 1st sensor index"
                    } else {
                        b"comm: set 2nd sensor index"
                    });
                    if env.comm_set_valve_sensor_index(x, if first { 1 } else { 2 }, y) != 0 {
                        dbg.println_text(b"invalid valve or sensor index");
                    }
                }
                _ => dbg.println_text(b"to few arguments"),
            }
        } else if req.is(b"stvls") {
            // stvls valve address1 address2: 00-00-00-00-00-00-00-00 clears the slot
            dbg.println_text(b"set valve sensors by address");
            match (argc == 3, x) {
                (true, Some(x)) => {
                    if env.comm_set_valve_sensors(x, req.arg(1), req.arg(2)) == 0 {
                        // match sensor address to valve struct at runtime, otherwise restart
                        // needed
                        env.app_match_sensors();
                    } else {
                        dbg.println_text(b"invalid valve index");
                    }
                }
                _ => dbg.println_text(b"to few arguments"),
            }
        } else if req.is(b"gvlon") {
            // the 1st and 2nd sensor address of a valve
            dbg.print(b"cmd: get 1st and 2nd onewire sensor addresses");
            match (argc == 1, x) {
                (true, Some(x)) if x < ACTUATOR_COUNT => {
                    dbg.print(b" - ");
                    dbg.print(b"gvlon");
                    dbg.print(b" ");
                    dbg.print_signed(i32::from(x), DEC);
                    dbg.print(b" ");
                    env.comm_print_valve_sensor_ids(dbg, x, b' ');
                    dbg.println_text(b" ");
                }
                _ => dbg.println_text(b" - error"),
            }
        } else if req.is(b"gvers") {
            dbg.print(b"Version: ");
            dbg.println_text(self.id.version);
        } else if req.is(b"stons") {
            dbg.print(b"start new 1-wire search");
            env.temp_command(TEMP_CMD_NEWSEARCH);
        } else if req.is(b"stlnt") {
            dbg.print(b"set valve learning time to ");
            match (argc == 1, req.arg_u32(0, 0, u32::MAX)) {
                (true, Some(xu32)) => {
                    if env.comm_set_learntime(xu32) == 0 {
                        dbg.println_unsigned(xu32, DEC);
                    } else {
                        dbg.println_text(b"- error");
                    }
                }
                _ => dbg.println_text(b"- error"),
            }
        } else if req.is(b"staop") || req.is(b"staln") {
            let open = req.is(b"staop");
            dbg.print(if open {
                b"got open valve request for "
            } else {
                b"start learning for valve "
            });
            match (argc == 1, x) {
                (true, Some(x)) => {
                    let r = if open {
                        env.app_set_valveopen(x)
                    } else {
                        env.app_set_valvelearning(x)
                    };
                    if r == 0 {
                        dbg.println_signed(i32::from(x), DEC);
                    } else {
                        dbg.println_text(b"- error");
                    }
                }
                _ => dbg.println_text(b"- error"),
            }
        } else if req.is(b"smotc") {
            dbg.print(b"got set motor characteristics request ");
            match (argc == 2, x, y) {
                (true, Some(x), Some(y)) => {
                    let current = env.motor_get_params();
                    let mut params = current;
                    let values = [
                        u32::from(x),
                        u32::from(y),
                        u32::from(params.start_on_power),
                        0,
                        0,
                    ];
                    if apply_motor_params_request(&mut params, 3, &values) == ParamsRequest::Applied
                    {
                        if !same_motor_params(&params, &current) {
                            env.motor_set_params(&params);
                            env.eeprom_changed(CHANGED_MOTOR);
                        }
                        dbg.println_text(b"- valid");
                    } else {
                        dbg.println_text(b"- values out of bounds");
                    }
                }
                _ => dbg.println_text(b"- error"),
            }
        } else if req.is(b"gmotc") {
            let m = env.motor_globals();
            dbg.print(b"got get motor characteristics request - low: ");
            dbg.print_unsigned(u32::from(m.low_fac), DEC);
            dbg.println_text(b" high: ");
            dbg.println_unsigned(u32::from(m.high_fac), DEC);
        } else if req.is(b"stdet") {
            // stdet 255: every valve is detected again; another valve is not supported (the
            // app would need a rework)
            dbg.print(b"got detect valve status request");
            match (argc == 1, x) {
                (true, Some(x)) => {
                    if x == 255 {
                        env.app_scan_valves();
                        dbg.println_text(b" - reset all valves");
                    } else {
                        dbg.println_text(b" - error");
                    }
                    dbg.print(b"stdet");
                    dbg.println_text(b" ");
                }
                _ => dbg.println_text(b" - error"),
            }
        } else {
            dbg.println_text(b"unknown command");
            return CMD_NONE;
        }
        0
    }

    /// `terminal_supervise`, main loop 10 ms branch: switches off a motor output enabled from
    /// the terminal when its time or current limit is reached
    pub fn supervise(
        &mut self,
        dbg: &mut impl Print,
        pins: &impl Pins,
        clock: &impl Clock,
        env: &impl TerminalEnv,
    ) {
        if self.manual_active
            && manual_enable_expired(self.manual_start_ms, clock.millis(), env.analog_current())
        {
            self.manual_off(pins);
            dbg.println_text(b"sena: output off");
        }
    }

    /// `terminal_manual_active`: true while a motor output is enabled from the terminal (the
    /// app hands no command to the valve machine meanwhile)
    pub fn manual_active(&self) -> bool {
        self.manual_active
    }

    /// `WriteEEPROMBaselayout`: the base fields get their defaults, written by the EEPROM loop
    fn write_eeprom_baselayout(dbg: &mut impl Print, env: &mut impl TerminalEnv) {
        dbg.println_text(b"write EEPROM layout...");
        let layout = &mut env.eep_content().cfg.layout;
        layout.b_slave = 0;
        // strncpy: the name and zeros up to the size of the field
        layout.descr = [0; 25];
        layout.descr[..SYSTEM_NAME.len()].copy_from_slice(SYSTEM_NAME);
        layout.one_wire_cfg = [0; 3];
        env.eeprom_changed(CHANGED_ALL);
    }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_mut;
