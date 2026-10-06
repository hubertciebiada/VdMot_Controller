// The cases of test/native/glue/test_fakes.cpp that pin the fakes of this port's suites: UART,
// Wire, the 24LC64, the 1-Wire libraries of the owDevices suite and the stub registry. (The
// Print cases are in print/tests.rs; time, pins, EXTI, timers, watchdog and the valve sim belong
// to the fake board of the motor suites; the runner hooks to the system suite.)

use std::cell::RefCell;
use std::rc::Rc;

use super::eeprom_fake::FakeEeprom;
use super::io_fakes::{Ev, FakeBoard, FakeSerial, RX_RING_SIZE};
use super::ow_fakes::FakeOwBus;
use super::stub_log::CallLog;
use super::stubs::Stubs;
use crate::communication::CommunicationEnv;
use crate::dallas::{DallasTemperature, DEVICE_DISCONNECTED_RAW};
use crate::eeprom24::EepromDevice;
use crate::hal::Serial;
use crate::i2c_bus::Wire;
use crate::ow_devices::OwBus;
use vdm_stm_core::uart_errors::{
    UART_ERROR_FRAMING, UART_ERROR_NOISE, UART_ERROR_OVERRUN, UART_ERROR_PARITY,
};

// ---------------------------------------------------------------- UART

#[test]
fn uart_bytes_arrive_in_order_across_the_wrap_of_the_ring() {
    // C++ also loses the bytes injected before begin(): the Rust port has no begin, the
    // firmware enables the reception at its set-up
    let mut port = FakeSerial::new();
    let mut sent = Vec::new();
    let mut got = Vec::new();
    for round in 0..3u8 {
        let chunk = vec![b'a' + round; 700];
        port.inject(&chunk);
        sent.extend_from_slice(&chunk);
        while port.available() > 0 {
            got.push(port.read().unwrap());
        }
    }
    assert_eq!(got, sent);
    assert_eq!(port.read(), None);
    assert_eq!(port.read_calls, 2101);
}

#[test]
fn uart_the_ring_holds_serial_rx_buffer_size_minus_1_bytes_later_bytes_are_dropped() {
    let mut port = FakeSerial::new();
    port.inject(&vec![b'x'; RX_RING_SIZE + 5]);
    assert_eq!(port.available(), RX_RING_SIZE - 1);
    assert_eq!(port.errors.dropped, 6);
}

#[test]
fn uart_the_rx_interrupt_sees_the_hal_error_of_its_byte() {
    // the HAL error bits
    assert_eq!(UART_ERROR_PARITY, 0x01);
    assert_eq!(UART_ERROR_NOISE, 0x02);
    assert_eq!(UART_ERROR_FRAMING, 0x04);
    assert_eq!(UART_ERROR_OVERRUN, 0x08);
    let mut port = FakeSerial::new();
    port.inject(b"a");
    port.next_error = UART_ERROR_FRAMING;
    port.inject(b"bc");
    assert_eq!(port.errors.framing, 1);
    assert_eq!(
        port.errors.overrun + port.errors.noise + port.errors.dropped,
        0
    );
    assert_eq!(port.available(), 3);
    assert_eq!(port.next_error, 0);
    // C++ also replaces the RX callback through getHandle() and finds the core's handler again
    // after begin(): the Rust firmware calls comm_rx_irq from its USART1 interrupt
}

#[test]
fn uart_read_bytes_and_end_have_no_rust_form() {
    // C++ "readBytes() of missing bytes waits its timeout in fake time" and "end() drops the
    // received bytes and stops the reception": neither is part of the Serial trait. What the
    // trait has: written bytes, the flushes
    let mut port = FakeSerial::new();
    port.write(b"ab");
    port.flush();
    port.write(b"c");
    assert_eq!(port.take_tx(), "abc");
    assert_eq!(port.take_tx(), "");
    assert_eq!(port.flushes, 1);
}

// ---------------------------------------------------------------- Wire

#[test]
fn wire_begin_and_end_are_recorded() {
    let mut board = FakeBoard::new();
    Wire::begin(&mut board);
    Wire::end(&mut board);
    // C++ also records the pins of setSDA/setSCL: the driver owns PB6/PB7
    assert_eq!(board.events(), vec![Ev::WireBegin, Ev::WireEnd]);
    assert!(!board.0.borrow().wire_running);
}

// ---------------------------------------------------------------- 24LC64

#[test]
fn eeprom_erased_written_and_read_back_transfers_logged() {
    let mut chip = FakeEeprom::default();
    let mut b = [0u8; 1];
    assert_eq!(chip.read_block(100, &mut b), 1);
    assert_eq!(b[0], 0xFF);
    // 13 address bits: wraps to 0
    assert_eq!(chip.write_block(8191, &[1, 2, 3]), 0);
    let mut back = [0u8; 3];
    assert_eq!(chip.read_block(8191, &mut back), 3);
    assert_eq!(back[2], 3);
    assert_eq!(chip.bytes[0], 2);
    assert_eq!(chip.ops.len(), 3);
    assert!(chip.ops[1].write);
    assert_eq!(chip.ops[1].address, 8191);
    assert_eq!(chip.ops[1].length, 3);
}

#[test]
fn eeprom_fail_reads_from_2_count_1_fails_exactly_read_2_failing_writes_change_nothing() {
    let mut chip = FakeEeprom {
        fail_reads_from: 2,
        fail_reads_count: 1,
        ..FakeEeprom::default()
    };
    let mut b = [7u8, 7];
    assert_eq!(chip.read_block(0, &mut b), 2);
    b[0] = 7;
    assert_eq!(chip.read_block(0, &mut b), 0);
    assert_eq!(b[0], 7);
    assert_eq!(chip.read_block(0, &mut b), 2);
    chip.fail_writes_from = 1;
    assert_eq!(chip.write_block(5, &[9]), 2);
    assert_eq!(chip.write_block(5, &[9]), 2);
    assert_eq!(chip.bytes[5], 0xFF);
    assert!(!chip.ops.last().unwrap().ok);
}

// ---------------------------------------------------------------- 1-Wire

#[test]
fn one_wire_search_in_list_order_crc_of_lib_core_dallas_temperature_on_the_bus() {
    let mut bus = FakeOwBus::new(Rc::new(RefCell::new(CallLog::default())));
    bus.add_one_wire(0x28, 1, 21 * 128);
    bus.add_one_wire(0x26, 2, 0);
    let gone = bus.add_one_wire(0x28, 3, 0);
    bus.devices[gone].present = false;
    bus.sensors_begin();
    assert_eq!(bus.sensors_device_count(), 2);
    assert_eq!(bus.sensors_resolution(), 12);
    assert_eq!(
        DallasTemperature::millis_to_wait_for_conversion(bus.sensors_resolution()),
        750
    );
    let mut a = [0u8; 8];
    bus.reset_search();
    assert!(bus.search(&mut a));
    assert_eq!(a[1], 1);
    assert!(DallasTemperature::valid_address(&a));
    assert!(DallasTemperature::valid_family(&a));
    assert_eq!(bus.sensors_get_temp(&a), 21 * 128);
    assert!(bus.search(&mut a));
    assert!(!DallasTemperature::valid_family(&a));
    assert!(!bus.search(&mut a));
    a[7] ^= 1;
    assert!(!DallasTemperature::valid_address(&a));
    // a device of another family does not raise the resolution
    let mut bus = FakeOwBus::new(Rc::new(RefCell::new(CallLog::default())));
    let v = bus.add_one_wire(0x26, 1, 0);
    bus.devices[v].resolution = 12;
    let t = bus.add_one_wire(0x28, 2, 0);
    bus.devices[t].resolution = 10;
    bus.sensors_begin();
    assert_eq!(bus.sensors_resolution(), 10);
    // a device with a bad CRC is not counted
    bus.devices[t].rom[7] ^= 1;
    bus.sensors_begin();
    assert_eq!(bus.sensors_device_count(), 1);
}

#[test]
fn one_wire_get_temp_gives_device_disconnected_raw_for_a_failing_read_and_an_absent_sensor() {
    let mut bus = FakeOwBus::new(Rc::new(RefCell::new(CallLog::default())));
    let s = bus.add_one_wire(0x28, 1, 85 * 128);
    bus.devices[s].fail_reads = 1;
    let rom = bus.devices[s].rom;
    assert_eq!(bus.sensors_get_temp(&rom), DEVICE_DISCONNECTED_RAW);
    assert_eq!(bus.sensors_get_temp(&rom), 85 * 128);
    bus.devices[s].present = false;
    assert_eq!(bus.sensors_get_temp(&rom), DEVICE_DISCONNECTED_RAW);
    assert_eq!(bus.reads, 3);
    // the DS2438 stub: its reading by address, -10 without one
    bus.bm_set_address(&rom);
    assert_eq!(bus.bm_read_vad(), -10.0);
    bus.vad.insert(rom, 1.5);
    assert_eq!(bus.bm_read_vad(), 1.5);
    bus.bm_begin = false;
    assert!(!bus.bm_begin());
}

// ---------------------------------------------------------------- stubs

#[test]
fn stubs_a_new_registry_has_an_empty_log_and_every_knob_at_its_default() {
    // C++ glue::begin() resets the registry of the linked stubs; each Rust case makes its own
    let mut stubs = Stubs::default();
    assert!(stubs.calls.calls.is_empty());
    stubs.app.set_valve_open = -1;
    stubs.valves[3].status = 7;
    assert_eq!(CommunicationEnv::app_set_valveopen(&mut stubs, 3), -1);
    assert_eq!(stubs.calls.calls, vec!["app_set_valveopen(3)"]);
    let stubs = Stubs::default();
    assert!(stubs.calls.calls.is_empty());
    assert_eq!(stubs.app.set_valve_open, 0);
    assert_eq!(stubs.valves[3].status, 0);
    assert_eq!(stubs.calls.calls_of("app_set_valveopen").len(), 0);
}
