//! The application stage (docs/rust/GLUE-DESIGN-STM.md §5.2, §5.4): the IWDG set-up first (the
//! boot stage started it at the end of the window; the same values again), the HSE probe, the
//! MPU stack guard, `embassy_stm32::init`, the hardware of the glue (pin modes, the state the
//! interrupts share, the serial ports), then the controller of the glue: `setup_system()` and
//! `loop_system()` for ever, its own watchdog feed.
#![forbid(unsafe_code)]

use embassy_stm32::adc::Adc;
use embassy_stm32::exti::{ExtiInput, TriggerEdge};
use embassy_stm32::gpio::{Flex, Input, Level, Output, OutputOpenDrain, Pull, Speed};
use embassy_stm32::timer::low_level::Timer;
use embassy_stm32::usart::{Config as UartConfig, Uart};
use stm32_metapac as pac;
use vdm_stm_boot::capture::read_guard;
use vdm_stm_boot::stage::probe_app_hse;
use vdm_stm_boot::BootToken;
use vdm_stm_glue::board::BoardRev;
use vdm_stm_glue::communication::FirmwareId;
use vdm_stm_glue::eeprom24::{I2cEeprom, DEVICEADDRESS};
use vdm_stm_glue::hal::{Out, Pins};
use vdm_stm_glue::i2c_master::I2cV1;
use vdm_stm_glue::motor::MotorShared;
use vdm_stm_glue::ow_devices::LineBus;
use vdm_stm_glue::serial::PortSerial;
use vdm_stm_glue::sysstat::Sysstat;
use vdm_stm_glue::system::{Controller, Hardware, Shared};

use crate::board::{self, Fw, FwAdc, FwBoard, FwNoinit, FwTimer, FwUsart, FwWatchdog};
use crate::boot_hw::{id_field, watchdog_start, Regs};
use crate::i2c::FwI2c;
use crate::isr;
use crate::one_wire::FwOneWire;
use crate::{clocks, fault, id, noinit};

#[cfg(feature = "c1")]
const BOARD: BoardRev = BoardRev::C1;
#[cfg(feature = "c2")]
const BOARD: BoardRev = BoardRev::C2;

/// The application stage; never returns.
#[inline(never)]
pub fn run(token: BootToken) -> ! {
    // `IWatchdog.begin(8000000)` of setup_system(), before the clock set-up it now covers
    watchdog_start();
    let hse = probe_app_hse(&mut Regs, token.hse());
    fault::stack_guard();
    let p = embassy_stm32::init(clocks::config(hse));
    board::set_boot_ms(token.boot_ms());
    // the cycle counter of the 1-Wire slots
    if let Some(mut core) = cortex_m::Peripherals::take() {
        core.DCB.enable_trace();
        core.DWT.enable_cycle_counter();
    }

    // pin modes (include/hardware.h); the levels are the glue's, through BSRR. The boot stage
    // drove the valve outputs safe already: the same levels here.
    keep(Output::new(p.PA5, Level::Low, Speed::Low));
    keep(Output::new(p.PA6, Level::Low, Speed::Low));
    keep(Output::new(p.PA7, Level::Low, Speed::Low));
    keep(Output::new(p.PB0, Level::Low, Speed::Low));
    keep(Output::new(p.PA15, Level::Low, Speed::Low));
    keep(Output::new(p.PB3, Level::Low, Speed::Low));
    keep(Output::new(p.PA8, Level::Low, Speed::Low));
    keep(Output::new(p.PB1, Level::Low, Speed::Low));
    // valve PSU: latch high before open drain (valve_pins_safe)
    keep(OutputOpenDrain::new(p.PB9, Level::High, Speed::Low));
    // the LED keeps the level the boot window left (pinMode does not touch the latch)
    let led = if FwBoard.latch(Out::Led) {
        Level::High
    } else {
        Level::Low
    };
    keep(Output::new(p.PC13, led, Speed::Low));
    keep(Input::new(p.PB2, Pull::Up));
    // REVIN: EXTI4 on the rising edge, detached until a motor start attaches it
    keep(ExtiInput::new_blocking(
        p.PA4,
        p.EXTI4,
        Pull::None,
        TriggerEdge::Rising,
    ));
    isr::rev_irq(false);
    // 1-Wire on PB10: open drain, released, input buffer on
    let mut one_wire = Flex::new(p.PB10);
    one_wire.set_high();
    one_wire.set_as_input_output(Speed::Medium);
    keep(one_wire);

    // the state of the interrupts, before any of them runs
    isr::ADC.init(FwAdc {
        adc: Adc::new(p.ADC1),
        current: p.PA0,
        reference: p.PA1,
    });
    isr::MOTOR.init(MotorShared::new(BOARD));

    // USART1 (ESP, PA9/PA10) and USART6 (terminal, PA11/PA12): 115200 8N1, receive interrupts on
    let mut config = UartConfig::default();
    config.baudrate = 115_200;
    match Uart::new_blocking(p.USART1, p.PA10, p.PA9, config) {
        Ok(uart) => keep(uart),
        Err(_) => fault::on_panic(),
    }
    match Uart::new_blocking(p.USART6, p.PA12, p.PA11, config) {
        Ok(uart) => keep(uart),
        Err(_) => fault::on_panic(),
    }
    isr::priorities();
    isr::usart_irqs_on();

    let hw = Hardware::<Fw> {
        board: FwBoard,
        esp: PortSerial {
            port: &isr::ESP,
            usart: FwUsart(pac::USART1),
        },
        dbg: PortSerial {
            port: &isr::DBG,
            usart: FwUsart(pac::USART6),
        },
        noinit: FwNoinit,
        watchdog: FwWatchdog,
        tim1: FwTimer::Tim1(Timer::new(p.TIM1)),
        tim2: FwTimer::Tim2(Timer::new(p.TIM2)),
        eeprom: I2cEeprom::new(I2cV1::new(FwI2c, FwBoard), FwBoard, DEVICEADDRESS),
        i2c: FwI2c,
        one_wire: LineBus::new(FwOneWire, FwBoard),
    };
    let reset = token.reset();
    let sysstat = Sysstat::new(
        reset.reason,
        reset.resets,
        reset.safe_mode,
        read_guard(&noinit::read()),
    );
    let mut controller = Controller::new(
        hw,
        Shared {
            motor: &isr::MOTOR,
            flags: &isr::FLAGS,
            esp_port: &isr::ESP,
        },
        sysstat,
        firmware_id(),
        BOARD,
    );
    controller.set_fault_record(fault::last_record());
    controller.run()
}

/// A driver whose set-up stays: its drop would undo it.
fn keep<T>(driver: T) {
    core::mem::forget(driver);
}

/// Version and tag from the ID block in flash (the bytes the ESP validated), the build field.
fn firmware_id() -> FirmwareId {
    FirmwareId {
        version: id_field(id::LAYOUT.version, id::LAYOUT.version_len),
        tag: id_field(id::LAYOUT.tag, id::LAYOUT.tag_len),
        build: id::BUILD,
    }
}

