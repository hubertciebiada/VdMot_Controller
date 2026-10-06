//! The 1-Wire devices (`software_stm32/src/owDevices.cpp`, `include/owDevices.h`): enumeration of
//! the DS18x20 temperature sensors and DS2438 monitors in search order, the temperature state
//! machine of the main loop (one step per call of the 10 ms branch), the re-scan 24 h after the
//! last enumeration, the sensor JSON of the debug terminal.

use crate::dallas::{DallasTemperature, DEVICE_DISCONNECTED_RAW};
use crate::ds2438::{Ds2438, DS2438_BEGIN_RETRIES};
use crate::hal::{Clock, OneWireLine};
use crate::onewire::{crc8, DeviceAddress, OneWire};
use crate::print::{Print, DEC, HEX};
use vdm_stm_core::temp_filter::{filter_temperature, TempTrack};

/// usable temperature sensors: 2 per valve and 10 more (C++ MAXONEWIRECNT)
pub const MAXONEWIRECNT: usize = 34;
/// usable DS2438 monitors (C++ MAXDS2438CNT)
pub const MAXDS2438CNT: usize = 8;

// temp_command()
pub const TEMP_CMD_NONE: i32 = 0x00;
pub const TEMP_CMD_NEWSEARCH: i32 = 0x01;
pub const TEMP_CMD_LOCK: i32 = 0x02;
pub const TEMP_CMD_UNLOCK: i32 = 0x03;

const DS2438_FAMILY: u8 = 0x26;
/// upper bound of search passes per enumeration
const MAX_ONEWIRE_SEARCH: u8 = 64;
/// temperature (0.1 degC) of a sensor that was not read yet
pub const TEMP_UNKNOWN: i32 = -500;
/// the bus is enumerated again after 24 h
const OW_RESCAN_S: u32 = 86400;
/// T_WAIT steps on top of the conversion time / 10 ms
const CONV_WAIT_STEPS: u32 = 20;

/// One temperature sensor (C++ `tempsensor`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TempSensor {
    /// 0.1 degC
    pub temperature: i32,
    pub address: DeviceAddress,
}

/// One DS2438 monitor (C++ `voltsensor`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VoltSensor {
    /// VAD in 10 mV
    pub vad: i32,
    pub address: DeviceAddress,
}

/// The devices found (C++ globals `tempsensors[]`, `voltsensors[]`, `noOfDevices`,
/// `noOfDS18Devices`, `noOfDS2438Devices`), read by communication, app and the terminal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OwSensors {
    pub tempsensors: [TempSensor; MAXONEWIRECNT],
    pub voltsensors: [VoltSensor; MAXDS2438CNT],
    /// every device with a valid ROM CRC (search passes of the last enumeration)
    pub no_of_devices: u8,
    pub no_of_ds18_devices: u8,
    pub no_of_ds2438_devices: u8,
}

impl Default for OwSensors {
    fn default() -> Self {
        OwSensors {
            tempsensors: [TempSensor::default(); MAXONEWIRECNT],
            voltsensors: [VoltSensor::default(); MAXDS2438CNT],
            no_of_devices: 0,
            no_of_ds18_devices: 0,
            no_of_ds2438_devices: 0,
        }
    }
}

/// The bus with the library objects of owDevices.cpp: `oneWire` (OneWire), `sensors`
/// (DallasTemperature) and `bm` (DS2438) share one line. The C++ suites link fakes of these
/// libraries; [`LineBus`] is the firmware's.
pub trait OwBus {
    /// `oneWire.reset_search()`
    fn reset_search(&mut self);
    /// `oneWire.search(addr)` (normal search)
    fn search(&mut self, addr: &mut DeviceAddress) -> bool;
    /// `sensors.begin()`
    fn sensors_begin(&mut self);
    /// `sensors.getDeviceCount()`
    fn sensors_device_count(&self) -> u8;
    /// `sensors.setWaitForConversion(flag)`
    fn sensors_set_wait_for_conversion(&mut self, flag: bool);
    /// `sensors.requestTemperatures()`
    fn sensors_request_temperatures(&mut self);
    /// `sensors.getResolution()`
    fn sensors_resolution(&self) -> u8;
    /// `sensors.getTemp(addr)`: 1/128 degC or DEVICE_DISCONNECTED_RAW
    fn sensors_get_temp(&mut self, addr: &DeviceAddress) -> i16;
    /// `bm.begin()`
    fn bm_begin(&mut self) -> bool;
    /// `bm.setAddress(addr)`
    fn bm_set_address(&mut self, addr: &DeviceAddress);
    /// `bm.readVAD()`: V
    fn bm_read_vad(&mut self) -> f32;
}

/// The firmware's bus: the libraries of onewire.rs, dallas.rs and ds2438.rs on one line.
pub struct LineBus<L, C> {
    pub ow: OneWire<L>,
    pub sensors: DallasTemperature,
    pub bm: Ds2438,
    pub clock: C,
}

impl<L: OneWireLine, C: Clock> LineBus<L, C> {
    pub fn new(line: L, clock: C) -> Self {
        LineBus {
            ow: OneWire::new(line),
            sensors: DallasTemperature::default(),
            bm: Ds2438::default(),
            clock,
        }
    }
}

impl<L: OneWireLine, C: Clock> OwBus for LineBus<L, C> {
    fn reset_search(&mut self) {
        self.ow.reset_search();
    }

    fn search(&mut self, addr: &mut DeviceAddress) -> bool {
        self.ow.search(addr, true)
    }

    fn sensors_begin(&mut self) {
        self.sensors.begin(&mut self.ow);
    }

    fn sensors_device_count(&self) -> u8 {
        self.sensors.get_device_count()
    }

    fn sensors_set_wait_for_conversion(&mut self, flag: bool) {
        self.sensors.set_wait_for_conversion(flag);
    }

    fn sensors_request_temperatures(&mut self) {
        self.sensors.request_temperatures(&mut self.ow, &self.clock);
    }

    fn sensors_resolution(&self) -> u8 {
        self.sensors.get_resolution()
    }

    fn sensors_get_temp(&mut self, addr: &DeviceAddress) -> i16 {
        DallasTemperature::get_temp(&mut self.ow, addr)
    }

    fn bm_begin(&mut self) -> bool {
        self.bm.begin(&mut self.ow, DS2438_BEGIN_RETRIES)
    }

    fn bm_set_address(&mut self, addr: &DeviceAddress) {
        self.bm.set_address(addr);
    }

    fn bm_read_vad(&mut self) -> f32 {
        self.bm.read_vad(&mut self.ow, &self.clock)
    }
}

/// Calls of owDevices.cpp into other glue modules.
pub trait OwDevicesEnv {
    /// app.cpp: a temperature cycle completed
    fn app_temp_cycle_done(&mut self);
    /// app.cpp: the sensor indexes of the valves follow a new search order
    fn app_match_sensors(&mut self) -> i16;
    /// the temperature lock (C++ `lock` of owDevices.cpp): the valve state machine sets it
    /// while a valve moves (motor `IsrFlags`), no new conversion starts meanwhile
    fn temp_locked(&self) -> bool;
    fn set_temp_lock(&mut self, locked: bool);
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum TState {
    #[default]
    Init,
    Idle,
    Request,
    Output,
    ReadVad,
    Wait,
    Search,
}

/// The module state (the C++ statics of owDevices.cpp and of `temperature_loop`).
#[derive(Clone, Debug)]
pub struct OwDevices {
    pub sensors: OwSensors,
    /// TEMP_CMD_NONE or TEMP_CMD_NEWSEARCH
    temp_cmd: i32,
    /// failed reads and the last good value of each sensor (core filter_temperature)
    temptrack: [TempTrack; MAXONEWIRECNT],
    /// seconds since the last enumeration
    scan_age_s: u32,
    /// millis() of the last second counted in scan_age_s
    scan_second_ms: u32,
    tempstate: TState,
    substate: u8,
    devcnt: usize,
    dev_ds2438_cnt: usize,
    timer: u32,
}

impl Default for OwDevices {
    fn default() -> Self {
        OwDevices {
            sensors: OwSensors::default(),
            temp_cmd: TEMP_CMD_NONE,
            temptrack: [TempTrack::default(); MAXONEWIRECNT],
            scan_age_s: 0,
            scan_second_ms: 0,
            tempstate: TState::Init,
            substate: 0,
            devcnt: 0,
            dev_ds2438_cnt: 0,
            timer: 0,
        }
    }
}

/// `printAddress` (a debug line, printed with appDebug): "{ 28,  0F, ... }"
pub fn print_address(dbg: &mut impl Print, device_address: &DeviceAddress) {
    dbg.print(b"{");
    for (i, &b) in device_address.iter().enumerate() {
        dbg.print(b" ");
        if b < 16 {
            dbg.print(b"0");
        }
        dbg.print_unsigned(u32::from(b), HEX);
        if i < 7 {
            dbg.print(b", ");
        }
    }
    dbg.print(b" }");
}

impl OwDevices {
    /// `setDeviceAddress`: enumerates the bus once and fills tempsensors (DS18 family) and
    /// voltsensors (DS2438) in search order; devices beyond the array sizes are ignored, at
    /// most MAX_ONEWIRE_SEARCH search passes.
    pub fn set_device_address(&mut self, bus: &mut impl OwBus, clock: &impl Clock) {
        let mut addr: DeviceAddress = [0; 8];
        let mut devices: u8 = 0;
        let mut ds18: usize = 0;
        let mut ds2438: usize = 0;

        bus.reset_search();
        for _ in 0..MAX_ONEWIRE_SEARCH {
            if !bus.search(&mut addr) {
                break;
            }
            if crc8(&addr[..7]) != addr[7] {
                continue;
            }
            devices += 1;
            if addr[0] == DS2438_FAMILY {
                if let Some(v) = self.sensors.voltsensors.get_mut(ds2438) {
                    v.address = addr;
                    ds2438 += 1;
                }
            } else if DallasTemperature::valid_family(&addr) {
                if let Some(t) = self.sensors.tempsensors.get_mut(ds18) {
                    // a different sensor at this index must not inherit the old temperature
                    if t.address != addr {
                        t.address = addr;
                        t.temperature = TEMP_UNKNOWN;
                        self.temptrack[ds18] = TempTrack::default();
                    }
                    ds18 += 1;
                }
            }
        }

        // forget sensors that are no longer present
        for t in self.sensors.tempsensors.iter_mut().skip(ds18) {
            t.address = [0; 8];
            t.temperature = TEMP_UNKNOWN;
        }
        for v in self.sensors.voltsensors.iter_mut().skip(ds2438) {
            v.address = [0; 8];
            v.vad = 0;
        }

        self.sensors.no_of_devices = devices;
        self.sensors.no_of_ds18_devices = ds18 as u8;
        self.sensors.no_of_ds2438_devices = ds2438 as u8;
        self.scan_age_s = 0;
        self.scan_second_ms = clock.millis();
    }

    /// `temperature_setup`: no sensor known yet, the libraries started, the bus enumerated
    pub fn setup(&mut self, bus: &mut impl OwBus, clock: &impl Clock) {
        for t in self.sensors.tempsensors.iter_mut() {
            t.address = [0; 8];
            t.temperature = TEMP_UNKNOWN;
        }
        bus.sensors_begin();
        bus.sensors_set_wait_for_conversion(false);
        self.set_device_address(bus, clock);
        bus.bm_begin();
    }

    /// `temperature_loop`: one step of the temperature cycle: request the conversion, wait
    /// for it (CONV_WAIT_STEPS + conversion time / 10 calls), read one sensor per call (a
    /// failed read is repeated once), then one DS2438 per call; a search replaces the cycle
    /// when requested (stons) or 24 h after the last one, never while the lock holds.
    pub fn loop_(&mut self, bus: &mut impl OwBus, clock: &impl Clock, env: &mut impl OwDevicesEnv) {
        match self.tempstate {
            // C++ also sets the timer to CONV_INTERVALL / 10 here: the request sets it before
            // any wait reads it
            TState::Init => self.tempstate = TState::Idle,

            TState::Idle => {
                // new sensors appear without a stons, only while no motor runs (lock)
                if self.temp_cmd == TEMP_CMD_NONE
                    && !env.temp_locked()
                    && self.ow_scan_age_s(clock) >= OW_RESCAN_S
                {
                    self.temp_cmd = TEMP_CMD_NEWSEARCH;
                }
                if self.temp_cmd == TEMP_CMD_NEWSEARCH {
                    self.substate = 0;
                    self.tempstate = TState::Search;
                } else if !env.temp_locked() {
                    self.tempstate = TState::Request;
                }
            }

            TState::Request => {
                bus.sensors_request_temperatures();
                let resolution = bus.sensors_resolution();
                self.timer = CONV_WAIT_STEPS
                    + u32::from(DallasTemperature::millis_to_wait_for_conversion(resolution)) / 10;
                self.tempstate = TState::Wait;
            }

            TState::Wait => {
                if self.timer > 0 {
                    self.timer -= 1;
                } else {
                    self.devcnt = 0;
                    self.tempstate = TState::Output;
                }
            }

            TState::Output => {
                let count = usize::from(self.sensors.no_of_ds18_devices).min(MAXONEWIRECNT);
                if self.devcnt < count {
                    // read by ROM address: bus indexes also count DS2438 devices; a failed
                    // read is repeated once, a failure after it keeps the last good value for
                    // 2 cycles
                    let address = self.sensors.tempsensors[self.devcnt].address;
                    let mut raw = bus.sensors_get_temp(&address);
                    if raw == DEVICE_DISCONNECTED_RAW {
                        raw = bus.sensors_get_temp(&address);
                    }
                    self.sensors.tempsensors[self.devcnt].temperature =
                        filter_temperature(&mut self.temptrack[self.devcnt], raw);
                } else if self.sensors.no_of_ds2438_devices > 0 {
                    self.tempstate = TState::ReadVad;
                    self.dev_ds2438_cnt = 0;
                } else {
                    self.tempstate = TState::Idle;
                    env.app_temp_cycle_done();
                }
                self.devcnt += 1;
            }

            TState::ReadVad => {
                let count = usize::from(self.sensors.no_of_ds2438_devices).min(MAXDS2438CNT);
                if self.dev_ds2438_cnt < count {
                    let v = &mut self.sensors.voltsensors[self.dev_ds2438_cnt];
                    bus.bm_set_address(&v.address);
                    let vad = bus.bm_read_vad();
                    // 10 mV resolution: int = 100 * float (float arithmetic, truncated)
                    v.vad = (100.0f32 * vad) as i32;
                    self.dev_ds2438_cnt += 1;
                } else {
                    self.tempstate = TState::Idle;
                    env.app_temp_cycle_done();
                }
            }

            TState::Search => match self.substate {
                // start a new search
                0 => {
                    bus.sensors_begin();
                    self.substate = 1;
                    self.sensors.no_of_ds18_devices = 0;
                    self.sensors.no_of_ds2438_devices = 0;
                }
                // read the device count
                1 => {
                    self.sensors.no_of_devices = bus.sensors_device_count();
                    self.devcnt = 0;
                    self.substate = 2;
                }
                // the addresses of all devices
                _ => {
                    self.set_device_address(bus, clock);
                    // sensor indexes of the valves refer to the new search order
                    env.app_match_sensors();
                    self.temp_cmd = TEMP_CMD_NONE;
                    self.tempstate = TState::Idle;
                }
            },
        }
    }

    /// `print_sensordata`: {"cnt":n,"sns":[{"temp":t,"add":"28 ff .. .."},...]} (the format
    /// of the former ArduinoJson output) straight to out: every sensor is listed
    pub fn print_sensordata(&self, out: &mut impl Print) {
        const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";
        let count = usize::from(self.sensors.no_of_ds18_devices).min(MAXONEWIRECNT);

        out.print(b"{\"cnt\":");
        out.print_signed(count as i32, DEC);
        if count > 0 {
            out.print(b",\"sns\":[");
        }
        for (i, t) in self.sensors.tempsensors.iter().take(count).enumerate() {
            if i > 0 {
                out.print_char(b',');
            }
            out.print(b"{\"temp\":");
            out.print_signed(t.temperature, DEC);
            out.print(b",\"add\":\"");
            for (x, &b) in t.address.iter().enumerate() {
                out.print_char(HEX_DIGITS[usize::from(b >> 4)]);
                out.print_char(HEX_DIGITS[usize::from(b & 0x0F)]);
                if x < 7 {
                    out.print_char(b' ');
                }
            }
            out.print(b"\"}");
        }
        if count > 0 {
            out.print_char(b']');
        }
        out.println_char(b'}');
    }

    /// `temp_command`: TEMP_CMD_LOCK / TEMP_CMD_UNLOCK set the lock, another command is taken
    /// when none waits (TEMP_CMD_NEWSEARCH)
    pub fn temp_command(&mut self, command: i32, env: &mut impl OwDevicesEnv) {
        if command == TEMP_CMD_LOCK {
            env.set_temp_lock(true);
        } else if command == TEMP_CMD_UNLOCK {
            env.set_temp_lock(false);
        } else if self.temp_cmd == TEMP_CMD_NONE {
            self.temp_cmd = command;
        }
    }

    /// `temp_locked`: the temperature state machine is locked (TEMP_CMD_LOCK)
    pub fn temp_locked(&self, env: &impl OwDevicesEnv) -> bool {
        env.temp_locked()
    }

    /// `ow_scan_age_s`: seconds since the last 1-Wire enumeration, counted in whole seconds
    pub fn ow_scan_age_s(&mut self, clock: &impl Clock) -> u32 {
        let seconds = clock.millis().wrapping_sub(self.scan_second_ms) / 1000;
        self.scan_second_ms = self.scan_second_ms.wrapping_add(seconds.wrapping_mul(1000));
        self.scan_age_s = self.scan_age_s.wrapping_add(seconds);
        self.scan_age_s
    }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_line;
#[cfg(test)]
mod tests_mut;
#[cfg(test)]
mod tests_s7;
