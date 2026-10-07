//! The HAL traits of the glue on the STM32F401/F411 BlackPill (docs/rust/GLUE-DESIGN-STM.md
//! §1.3) and the platform of the controller. Register writes only: every decision is the
//! glue's. The pin modes are set up once by `app::run` (embassy drivers, then forgotten so the
//! modes stay); the writes go through BSRR, atomic, so TIM1, TIM2, EXTI and the main loop may
//! all switch the outputs.
#![forbid(unsafe_code)]

use core::sync::atomic::{AtomicU32, Ordering};

use cortex_m::peripheral::SCB;
use cortex_m::register::{basepri, primask};
use embassy_stm32::adc::{Adc, SampleTime};
use embassy_stm32::peripherals::{ADC1, PA0, PA1, TIM1, TIM2};
use embassy_stm32::timer::low_level::Timer;
use embassy_stm32::Peri;
use embassy_time::{block_for, Duration, Instant};
use stm32_metapac as pac;
use stm32_metapac::gpio::regs::Bsrr;
use stm32_metapac::timer::vals::Urs;
use vdm_stm_glue::eeprom24::I2cEeprom;
use vdm_stm_glue::hal::{
    Clock, ControlTimer, CurrentAdc, In, NoinitStore, Out, Pins, RevIrq, System, Watchdog,
    NOINIT_SIZE,
};
use vdm_stm_glue::hw_timer;
use vdm_stm_glue::motor::MotorShared;
use vdm_stm_glue::ow_devices::LineBus;
use vdm_stm_glue::serial::{PortSerial, UsartTx};
use vdm_stm_glue::system::Platform;

use crate::boot_hw::iwdg_key;
use crate::i2c::FwI2c;
use crate::isr::{self, IsrCell, TimerIrq, PRIO_USART};
use crate::noinit;
use crate::one_wire::FwOneWire;

/// The milliseconds of the boot stage: `millis()` counts from the reset, as the C++ HAL tick.
static BOOT_MS: AtomicU32 = AtomicU32::new(0);

pub fn set_boot_ms(ms: u32) {
    BOOT_MS.store(ms, Ordering::Relaxed);
}

/// The board: pins, the EXTI line of REVIN, the clock, the system reset.
#[derive(Clone, Copy)]
pub struct FwBoard;

/// GPIOA..C
#[derive(Clone, Copy)]
enum Port {
    A,
    B,
    C,
}

fn regs(port: Port) -> pac::gpio::Gpio {
    match port {
        Port::A => pac::GPIOA,
        Port::B => pac::GPIOB,
        Port::C => pac::GPIOC,
    }
}

/// include/hardware.h
fn out_pin(pin: Out) -> (Port, u32) {
    match pin {
        Out::Ena0 => (Port::A, 5),
        Out::Ena1 => (Port::A, 6),
        Out::Ena2 => (Port::A, 7),
        Out::Ena3 => (Port::B, 0),
        Out::Ena4 => (Port::A, 15),
        Out::Ena5 => (Port::B, 3),
        Out::Dir => (Port::A, 8),
        Out::Mux => (Port::B, 1),
        Out::PsuEna => (Port::B, 9),
        Out::Led => (Port::C, 13),
    }
}

fn in_pin(pin: In) -> (Port, u32) {
    match pin {
        In::Button => (Port::B, 2),
        In::RevIn => (Port::A, 4),
    }
}

impl Pins for FwBoard {
    fn set(&self, pin: Out, high: bool) {
        let (port, bit) = out_pin(pin);
        let mask = if high { 1 << bit } else { 1 << (bit + 16) };
        regs(port).bsrr().write_value(Bsrr(mask));
    }

    fn latch(&self, pin: Out) -> bool {
        let (port, bit) = out_pin(pin);
        regs(port).odr().read().0 & (1 << bit) != 0
    }

    fn read(&self, pin: In) -> bool {
        let (port, bit) = in_pin(pin);
        regs(port).idr().read().0 & (1 << bit) != 0
    }
}

impl RevIrq for FwBoard {
    fn attach(&self) {
        isr::rev_irq(true);
    }

    fn detach(&self) {
        isr::rev_irq(false);
    }
}

impl Clock for FwBoard {
    fn millis(&self) -> u32 {
        (Instant::now().as_millis() as u32).wrapping_add(BOOT_MS.load(Ordering::Relaxed))
    }

    fn micros(&self) -> u32 {
        (Instant::now().as_micros() as u32)
            .wrapping_add(BOOT_MS.load(Ordering::Relaxed).wrapping_mul(1000))
    }

    fn delay_ms(&self, ms: u32) {
        block_for(Duration::from_millis(u64::from(ms)));
    }

    fn delay_us(&self, us: u32) {
        block_for(Duration::from_micros(u64::from(us)));
    }
}

impl System for FwBoard {
    fn reset(&self) -> ! {
        SCB::sys_reset()
    }

    fn dev_id(&self) -> u16 {
        pac::DBGMCU.idcode().read().dev_id() & 0xFFF
    }
}

/// The C++ 2.1.7 no-init cells (noinit.rs).
#[derive(Clone, Copy)]
pub struct FwNoinit;

impl NoinitStore for FwNoinit {
    fn read(&self) -> [u8; NOINIT_SIZE] {
        noinit::read()
    }

    fn write(&mut self, image: &[u8; NOINIT_SIZE]) {
        noinit::write(image);
    }
}

/// `IWatchdog.reload()`
pub struct FwWatchdog;

impl Watchdog for FwWatchdog {
    fn reload(&mut self) {
        iwdg_key(0xAAAA);
    }
}

/// TIM1 (1 ms) or TIM2 (10 ms): the embassy driver keeps the timer clock on.
pub enum FwTimer {
    Tim1(Timer<'static, TIM1>),
    Tim2(Timer<'static, TIM2>),
}

impl ControlTimer for FwTimer {
    /// `attachInterruptInterval(us, cb)`: PSC and ARR of `HardwareTimer::setOverflow` (glue
    /// hw_timer), counter from 0, the update interrupt at P14, the counter running.
    fn attach_interval(&mut self, interval_us: u32) -> bool {
        let (regs, clk, irq) = match self {
            FwTimer::Tim1(t) => (t.regs_core(), t.get_clock_frequency().0, TimerIrq::Tim1),
            FwTimer::Tim2(t) => (t.regs_core(), t.get_clock_frequency().0, TimerIrq::Tim2),
        };
        let o = hw_timer::overflow(clk, interval_us);
        regs.cr1().modify(|w| w.set_cen(false));
        regs.psc().write_value(o.psc as u16);
        regs.arr().write(|w| w.set_arr(o.arr as u16));
        regs.cnt().write(|w| w.set_cnt(0));
        // load PSC and ARR without an update interrupt
        regs.cr1().modify(|w| w.set_urs(Urs::COUNTER_ONLY));
        regs.egr().write(|w| w.set_ug(true));
        regs.cr1().modify(|w| w.set_urs(Urs::ANY_EVENT));
        regs.sr().modify(|w| w.set_uif(false));
        regs.dier().modify(|w| w.set_uie(true));
        isr::timer_irq_on(irq);
        regs.cr1().modify(|w| w.set_cen(true));
        true
    }
}

/// USART1 or USART6 behind a glue serial port.
#[derive(Clone, Copy)]
pub struct FwUsart(pub pac::usart::Usart);

impl UsartTx for FwUsart {
    fn start(&self) {
        self.0.cr1().modify(|w| w.set_txeie(true));
    }

    /// PRIMASK set (cortex-m: the exceptions are "inactive"), or BASEPRI masks the USART
    /// priority: its interrupt cannot run
    fn masked(&self) -> bool {
        let level = basepri::read();
        primask::read().is_inactive() || (level != 0 && level <= PRIO_USART as u8)
    }

    fn sr(&self) -> u32 {
        self.0.sr().read().0
    }

    fn send(&self, byte: u8) {
        self.0
            .dr()
            .write_value(pac::usart::regs::Dr(u32::from(byte)));
    }
}

/// The two conversions of `TimerHandler0`: PA0 (current) then PA1 (reference / 2), 15 cycles.
pub struct FwAdc {
    pub adc: Adc<'static, ADC1>,
    pub current: Peri<'static, PA0>,
    pub reference: Peri<'static, PA1>,
}

impl CurrentAdc for FwAdc {
    fn sample(&mut self) -> (u16, u16) {
        let current = self
            .adc
            .blocking_read(&mut self.current, SampleTime::CYCLES15);
        let reference = self
            .adc
            .blocking_read(&mut self.reference, SampleTime::CYCLES15);
        (current, reference)
    }
}

/// The hardware of the controller on the device.
pub struct Fw;

impl Platform for Fw {
    type Board = FwBoard;
    type Serial = PortSerial<'static, FwUsart>;
    type Noinit = FwNoinit;
    type Watchdog = FwWatchdog;
    type Timer = FwTimer;
    type Eeprom = I2cEeprom<FwI2c, FwBoard>;
    type I2c = FwI2c;
    type OneWire = LineBus<FwOneWire, FwBoard>;
    type Motor = IsrCell<MotorShared>;
}
