//! The 1-Wire libraries behind [`OwBus`] as the C++ owDevices suites link them: the fakes of
//! OneWire and DallasTemperature on a list of devices (`fake::oneWireBus`, `fake_board.cpp`)
//! and the link-seam stub of DS2438 (`stub_ds2438.cpp`), which logs into the call log of the
//! stub env.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::dallas::{DallasTemperature, DEVICE_DISCONNECTED_RAW};
use crate::onewire::{crc8, DeviceAddress};
use crate::ow_devices::OwBus;
use crate::test_support::stub_log::CallLog;

/// A device of the bus (fake::OneWireDevice).
#[derive(Clone, Debug)]
pub struct FakeOwDevice {
    pub rom: DeviceAddress,
    /// temperature in 1/128 degC
    pub raw: i16,
    /// bits
    pub resolution: u8,
    pub present: bool,
    /// the next reads return DEVICE_DISCONNECTED_RAW
    pub fail_reads: u32,
}

pub struct FakeOwBus {
    /// search order
    pub devices: Vec<FakeOwDevice>,
    pub log: Rc<RefCell<CallLog>>,
    // OneWire
    pub search_pos: usize,
    /// search() calls
    pub searches: u32,
    // DallasTemperature
    pub device_count: u8,
    pub bit_resolution: u8,
    pub wait_for_conversion: bool,
    pub begins: u32,
    pub requests: u32,
    /// getTemp() calls
    pub reads: u32,
    // DS2438 (stub)
    /// readVAD() of an address; -10 (a failed read) for an address without one
    pub vad: HashMap<DeviceAddress, f32>,
    pub bm_begin: bool,
    pub bm_address: DeviceAddress,
}

impl FakeOwBus {
    pub fn new(log: Rc<RefCell<CallLog>>) -> Self {
        FakeOwBus {
            devices: Vec::new(),
            log,
            search_pos: 0,
            searches: 0,
            device_count: 0,
            bit_resolution: 9,
            wait_for_conversion: true,
            begins: 0,
            requests: 0,
            reads: 0,
            vad: HashMap::new(),
            bm_begin: true,
            bm_address: [0; 8],
        }
    }

    /// A device with a valid ROM: family, serial, 0, 0, 0, 0, 0, CRC (fake::addOneWire);
    /// its index.
    pub fn add_one_wire(&mut self, family: u8, serial: u8, raw: i16) -> usize {
        let mut rom = [family, serial, 0, 0, 0, 0, 0, 0];
        rom[7] = crc8(&rom[..7]);
        self.devices.push(FakeOwDevice {
            rom,
            raw,
            resolution: 12,
            present: true,
            fail_reads: 0,
        });
        self.devices.len() - 1
    }

    fn find(&mut self, rom: &DeviceAddress) -> Option<&mut FakeOwDevice> {
        self.devices.iter_mut().find(|d| d.present && d.rom == *rom)
    }
}

/// "xx-xx-xx-xx-xx-xx-xx-xx" in lower case, as the stub logs addresses.
pub fn dash_address(a: &DeviceAddress) -> String {
    a.iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join("-")
}

impl OwBus for FakeOwBus {
    fn reset_search(&mut self) {
        self.search_pos = 0;
    }

    fn search(&mut self, addr: &mut DeviceAddress) -> bool {
        self.searches += 1;
        while self.search_pos < self.devices.len() {
            let d = &self.devices[self.search_pos];
            self.search_pos += 1;
            if d.present {
                *addr = d.rom;
                return true;
            }
        }
        false
    }

    /// enumerates like the library: every valid address counts, the DS18 family sets the
    /// resolution
    fn sensors_begin(&mut self) {
        self.begins += 1;
        self.device_count = 0;
        let mut a = [0u8; 8];
        self.reset_search();
        while self.search(&mut a) {
            if !DallasTemperature::valid_address(&a) {
                continue;
            }
            self.device_count += 1;
            if !DallasTemperature::valid_family(&a) {
                continue;
            }
            let bits = self.find(&a).map_or(0, |d| d.resolution);
            if bits > self.bit_resolution {
                self.bit_resolution = bits;
            }
        }
    }

    fn sensors_device_count(&self) -> u8 {
        self.device_count
    }

    fn sensors_set_wait_for_conversion(&mut self, flag: bool) {
        self.wait_for_conversion = flag;
    }

    /// (the C++ fake waits the conversion time when wait_for_conversion is set: the glue
    /// never requests so)
    fn sensors_request_temperatures(&mut self) {
        self.requests += 1;
    }

    fn sensors_resolution(&self) -> u8 {
        self.bit_resolution
    }

    fn sensors_get_temp(&mut self, addr: &DeviceAddress) -> i16 {
        self.reads += 1;
        let Some(d) = self.find(addr) else {
            return DEVICE_DISCONNECTED_RAW;
        };
        if d.fail_reads > 0 {
            d.fail_reads -= 1;
            return DEVICE_DISCONNECTED_RAW;
        }
        d.raw
    }

    fn bm_begin(&mut self) -> bool {
        self.log.borrow_mut().log("DS2438::begin(3)");
        self.bm_begin
    }

    fn bm_set_address(&mut self, addr: &DeviceAddress) {
        self.log
            .borrow_mut()
            .log(format!("DS2438::setAddress({})", dash_address(addr)));
        self.bm_address = *addr;
    }

    fn bm_read_vad(&mut self) -> f32 {
        self.log.borrow_mut().log("DS2438::readVAD()");
        self.vad.get(&self.bm_address).copied().unwrap_or(-10.0)
    }
}
