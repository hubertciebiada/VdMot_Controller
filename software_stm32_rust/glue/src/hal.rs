//! The hardware the glue runs on (docs/rust/GLUE-DESIGN-STM.md §1.3). `firmware/src/board.rs`
//! implements these traits on embassy-stm32 and the PAC; the glue tests implement them on fakes.
//! The traits carry no decisions: every rule stays in the glue.

/// Outputs (C++ `hardware.h`): ENA0..ENA5 (PA5, PA6, PA7, PB0, PA15, PB3), DIR (PA8), MUX (PB1),
/// PSU enable (PB9, open drain, low = on), LED (PC13).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Out {
    Ena0,
    Ena1,
    Ena2,
    Ena3,
    Ena4,
    Ena5,
    Dir,
    Mux,
    PsuEna,
    Led,
}

/// Inputs: BUTTON (PB2, pull-up), REVIN (PA4, motor revolution pulses).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum In {
    Button,
    RevIn,
}

/// `digitalWrite`/`digitalRead`; callable from any context (atomic BSRR writes). The pin modes
/// are firmware set-up.
pub trait Pins {
    fn set(&self, pin: Out, high: bool);
    /// the output latch (LED toggle)
    fn latch(&self, pin: Out) -> bool;
    fn read(&self, pin: In) -> bool;
}

/// The two conversions of `TimerHandler0`: (PA0 motor current, PA1 reference / 2), 12 bit.
pub trait CurrentAdc {
    fn sample(&mut self) -> (u16, u16);
}

/// EXTI4 on REVIN, rising edge (`attachInterrupt`/`detachInterrupt`).
pub trait RevIrq {
    fn attach(&self);
    fn detach(&self);
}

/// `millis`, `micros`, `delay`, `delayMicroseconds`.
pub trait Clock {
    fn millis(&self) -> u32;
    fn micros(&self) -> u32;
    fn delay_ms(&self, ms: u32);
    fn delay_us(&self, us: u32);
}

/// One USART (`HardwareSerial`): the receive ring filled by the RX interrupt and the transmit
/// ring.
pub trait Serial {
    fn available(&self) -> usize;
    fn read(&mut self) -> Option<u8>;
    /// blocks while the TX ring is full
    fn write(&mut self, bytes: &[u8]);
    /// TX ring empty and TC set
    fn flush(&mut self);
}

/// Result of an I2C write (`TwoWire::endTransmission`).
pub type WireStatus = u8;
pub const WIRE_OK: WireStatus = 0;
/// NACK (address or data)
pub const WIRE_NACK: WireStatus = 2;
/// any other bus error
pub const WIRE_ERROR: WireStatus = 4;
/// a phase took longer than 100 ms (`I2C_TIMEOUT_TICK`)
pub const WIRE_TIMEOUT: WireStatus = 5;

/// `Wire` on I2C1 (PB6 SCL, PB7 SDA), 100 kHz: a write and a read are separate transactions
/// with a STOP between them (`endTransmission` + `requestFrom`).
pub trait I2cMaster {
    /// address phase, then the bytes; an empty write only probes the address
    fn write(&mut self, addr: u8, bytes: &[u8]) -> WireStatus;
    /// bytes received
    fn read(&mut self, addr: u8, buf: &mut [u8]) -> usize;
    /// end, recover the bus (`i2c_bus::recover`), begin
    fn restart(&mut self);
}

/// The 1-Wire line (PB10, open drain) with its slot timing.
pub trait OneWireLine {
    /// reset pulse; true if a device answered with a presence pulse
    fn reset(&mut self) -> bool;
    fn write_bit(&mut self, bit: bool);
    fn read_bit(&mut self) -> bool;
}

/// The independent watchdog (`IWatchdog.reload()`).
pub trait Watchdog {
    fn reload(&mut self);
}

/// Bytes of the no-init RAM cells of C++ 2.1.7 (warm state, reset guard, reset counter).
pub const NOINIT_SIZE: usize = 212;

/// The no-init RAM region, copied as bytes (design §3.2).
pub trait NoinitStore {
    fn read(&self) -> [u8; NOINIT_SIZE];
    fn write(&mut self, image: &[u8; NOINIT_SIZE]);
}

/// `HAL_NVIC_SystemReset`, `HAL_GetDEVID`.
pub trait System {
    fn reset(&self) -> !;
    fn dev_id(&self) -> u16;
}

/// A control timer (STM32_TimerInterrupt `attachInterruptInterval`, design §2.4): TIM1 every
/// 1000 us runs `motor::timer_handler0`, TIM2 every 10000 us `motor::valve_loop`; one firmware
/// implementation per timer.
pub trait ControlTimer {
    fn attach_interval(&mut self, interval_us: u32) -> bool;
}
