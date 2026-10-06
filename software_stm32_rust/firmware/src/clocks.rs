//! Clocks of the application stage (docs/rust/GLUE-DESIGN-STM.md §5.3): the PLL frequencies of
//! the STM32duino BlackPill variants, from the HSE when it started (probe of
//! vdm_stm_boot::stage::probe_app_hse), else from the HSI (D5; the C++ hangs there).

use embassy_stm32::rcc::{
    AHBPrescaler, APBPrescaler, Hse, HseMode, LsConfig, Pll, PllMul, PllPDiv, PllPreDiv,
    PllQDiv, PllSource, Sysclk,
};
use embassy_stm32::time::Hertz;

/// F401: 84 MHz (APB1 42, APB2 84), USB/SDIO clock 48 MHz.
#[cfg(feature = "f401")]
const MUL: PllMul = PllMul::MUL336;
#[cfg(feature = "f401")]
const DIVP: PllPDiv = PllPDiv::DIV4;
#[cfg(feature = "f401")]
const DIVQ: PllQDiv = PllQDiv::DIV7;
/// F411: 96 MHz (APB1 48, APB2 96), 48 MHz.
#[cfg(feature = "f411")]
const MUL: PllMul = PllMul::MUL192;
#[cfg(feature = "f411")]
const DIVP: PllPDiv = PllPDiv::DIV2;
#[cfg(feature = "f411")]
const DIVQ: PllQDiv = PllQDiv::DIV4;

/// The embassy configuration; `hse`: the 25 MHz crystal is running and ready.
pub fn config(hse: bool) -> embassy_stm32::Config {
    let mut c = embassy_stm32::Config::default();
    let rcc = &mut c.rcc;
    if hse {
        rcc.hse = Some(Hse {
            freq: Hertz(25_000_000),
            mode: HseMode::Oscillator,
        });
        rcc.pll_src = PllSource::HSE;
    } else {
        rcc.hse = None;
        rcc.pll_src = PllSource::HSI;
    }
    rcc.pll = Some(Pll {
        // 1 MHz PLL input from either source
        prediv: if hse { PllPreDiv::DIV25 } else { PllPreDiv::DIV16 },
        mul: MUL,
        divp: Some(DIVP),
        divq: Some(DIVQ),
        divr: None,
    });
    rcc.sys = Sysclk::PLL1_P;
    rcc.ahb_pre = AHBPrescaler::DIV1;
    rcc.apb1_pre = APBPrescaler::DIV2;
    rcc.apb2_pre = APBPrescaler::DIV1;
    // B4: embassy's default starts the LSI for the RTC and resets the backup domain when its
    // RTC selection differs; the C++ never touches the backup domain
    rcc.ls = LsConfig::off();
    c
}
