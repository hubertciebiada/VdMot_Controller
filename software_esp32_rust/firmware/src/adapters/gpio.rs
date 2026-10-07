//! The pins and the UART of the STM link: NRST (IO15) and BOOT0 (IO14), the factory pin (IO2,
//! pull-up), Serial2 (UART2, TX IO17, RX IO5).

use esp_idf_svc::hal::delay::NON_BLOCK;
use esp_idf_svc::hal::gpio::{AnyIOPin, AnyOutputPin, Input, Pin, PinDriver, Pull};
use esp_idf_svc::hal::uart::config::{Config, Parity};
use esp_idf_svc::hal::uart::UartDriver;
use esp_idf_svc::hal::units::Hertz;
use esp_idf_svc::sys;
use vdm_esp_glue::port::{InputPin, OutputPin, Uart};

/// A push-pull output whose level is written before its direction, as `digitalWrite` before
/// `pinMode`: the first `set` never glitches (IO15 holds the STM in reset while HIGH).
pub struct OutPin {
    num: i32,
    _pin: AnyOutputPin<'static>,
}

impl OutPin {
    /// The pin routed to the GPIO matrix (IO14 and IO15 boot as JTAG pads), still an input.
    pub fn new(pin: AnyOutputPin<'static>) -> Self {
        let num = pin.pin() as i32;
        // SAFETY: the pin is owned here; the output stays disabled until the first `set`.
        unsafe { sys::esp_rom_gpio_pad_select_gpio(num as u32) };
        OutPin { num, _pin: pin }
    }
}

impl OutputPin for OutPin {
    fn set(&mut self, high: bool) {
        // SAFETY: an owned GPIO; level first, then the output enable (idempotent).
        unsafe {
            sys::gpio_set_level(self.num, u32::from(high));
            sys::gpio_set_direction(self.num, sys::gpio_mode_t_GPIO_MODE_OUTPUT);
        }
    }
}

/// An input with the internal pull-up.
pub struct InPin(PinDriver<'static, Input>);

impl InPin {
    pub fn new(pin: AnyIOPin<'static>) -> Option<Self> {
        PinDriver::input(pin, Pull::Up).ok().map(InPin)
    }
}

impl InputPin for InPin {
    fn is_low(&mut self) -> bool {
        self.0.is_low()
    }
}

/// Serial2 of the STM link: RX ring 2048 B, TX ring 512 B, no event queue.
pub struct StmUart(UartDriver<'static>);

impl StmUart {
    pub fn new(
        uart: esp_idf_svc::hal::uart::UART2<'static>,
        tx: AnyOutputPin<'static>,
        rx: AnyIOPin<'static>,
    ) -> Option<Self> {
        let conf = Config::new()
            .baudrate(Hertz(vdm_esp_glue::board::STM_BAUD))
            .rx_fifo_size(vdm_esp_glue::board::STM_RX_BUFFER_SIZE)
            .tx_fifo_size(vdm_esp_glue::board::STM_TX_BUFFER_SIZE)
            .queue_size(0);
        UartDriver::new(
            uart,
            tx,
            rx,
            Option::<AnyIOPin>::None,
            Option::<AnyOutputPin>::None,
            &conf,
        )
        .ok()
        .map(StmUart)
    }
}

impl Uart for StmUart {
    fn configure(&mut self, baud: u32, even_parity: bool) {
        let parity = if even_parity {
            Parity::ParityEven
        } else {
            Parity::ParityNone
        };
        let _ = self.0.change_baudrate(Hertz(baud));
        let _ = self.0.change_parity(parity);
        // Arduino's end()/begin() dropped what was received before
        let _ = self.0.clear_rx();
    }
    fn read(&mut self, out: &mut [u8]) -> usize {
        // Err(ESP_ERR_TIMEOUT): nothing received
        self.0.read(out, NON_BLOCK).unwrap_or(0)
    }
    fn write(&mut self, data: &[u8]) -> usize {
        self.0.write(data).unwrap_or(0)
    }
}
