//! The boot stage on the registers (docs/rust/GLUE-DESIGN-STM.md §5.2): the traits of
//! vdm-stm-boot on the PAC, polled, without interrupts, embassy or flash accesses (B2, B4),
//! and the jump into the ROM bootloader. The decisions (order, HSE limit, BRR, the window)
//! are in vdm_stm_boot::run; this file maps them to registers.
//!
//! Every wait for a hardware flag is bounded (B2). D9: the image check proves that this code
//! and everything it calls lies in sector 0.

use cortex_m::peripheral::SYST;
use stm32_metapac as pac;
use stm32_metapac::{gpio, iwdg, rcc, syscfg, usart};

use vdm_stm_boot::app_check::RECORD_WORDS;
use vdm_stm_boot::id_block::{BootId, ID_BLOCK_ADDR};
use vdm_stm_boot::{BootEnd, BootHw, BootIo, BootToken, ClockIo, FlashRead, NOINIT_LEN};

use crate::id::LAYOUT;
use crate::noinit;

// RCC (RM0368 / RM0383 §6.3)
const CR_HSION: u32 = 1 << 0;
const CR_HSIRDY: u32 = 1 << 1;
const CR_HSEON: u32 = 1 << 16;
const CR_HSERDY: u32 = 1 << 17;
const CR_HSEBYP: u32 = 1 << 18;
const CR_CSSON: u32 = 1 << 19;
const CR_PLLON: u32 = 1 << 24;
const CR_PLLI2SON: u32 = 1 << 26;
const CFGR_SW: u32 = 0b11;
const CFGR_SW_HSE: u32 = 0b01;
const CFGR_SWS: u32 = 0b11 << 2;
const CFGR_SWS_HSI: u32 = 0b00 << 2;
const CFGR_SWS_HSE: u32 = 0b01 << 2;
const CSR_RMVF: u32 = 1 << 24;
const AHB1ENR_GPIOA: u32 = 1 << 0;
const AHB1ENR_GPIOB: u32 = 1 << 1;
const AHB1ENR_GPIOC: u32 = 1 << 2;
const APB2_USART1: u32 = 1 << 4;
const APB2ENR_SYSCFG: u32 = 1 << 14;

// USART1 (§19.6)
const SR_RXNE: u32 = 1 << 5;
const SR_TC: u32 = 1 << 6;
const SR_TXE: u32 = 1 << 7;
const CR1_RE: u32 = 1 << 2;
const CR1_TE: u32 = 1 << 3;
const CR1_PCE: u32 = 1 << 10;
const CR1_M: u32 = 1 << 12;
const CR1_UE: u32 = 1 << 13;

// IWDG (§17.4): `IWatchdog.begin(8000000)`, prescaler /64, reload 3999 = 8 s at 32 kHz
// (5.4-15 s with the LSI spread)
const IWDG_ENABLE: u16 = 0x5555;
const IWDG_RELOAD: u16 = 0xAAAA;
const IWDG_START: u16 = 0xCCCC;
const IWDG_PR_DIV64: u32 = 4;
const IWDG_RLR_8S: u32 = 3999;

// SysTick
const SYST_ENABLE: u32 = 1 << 0;
const SYST_CLKSOURCE_CPU: u32 = 1 << 2;
const SYST_COUNTFLAG: u32 = 1 << 16;

// pins (include/hardware.h) as bit masks of their port. Masks, not arrays: a constant array
// lands in .rodata, which lies outside sector 0 in a full image (D9, image check).
/// ENA0..2 and ENA4 (PA5, PA6, PA7, PA15)
pub const ENA_PORT_A_MASK: u32 = (1 << 5) | (1 << 6) | (1 << 7) | (1 << 15);
/// ENA3 and ENA5 (PB0, PB3)
pub const ENA_PORT_B_MASK: u32 = (1 << 0) | (1 << 3);
/// valve PSU enable (PB9, open drain, low = on)
pub const PSU_PB_MASK: u32 = 1 << 9;
const LED_PC_MASK: u32 = 1 << 13;
/// USART1 TX PA9, RX PA10
const USART1_PA_MASK: u32 = (1 << 9) | (1 << 10);
const AF_USART1: u32 = 7;

/// Polls of a hardware flag that settles within a few clock cycles (switch of SYSCLK, HSI
/// on) or within one character at 115200 Bd (TXE, TC): far above both on any clock here.
const SPIN_LIMIT: u32 = 200_000;

/// B8: magic, first address, length and CRC-32 of the application part (sectors 1..n), at
/// 0x08000240 in sector 0 (memory/vdm.x, design §5.11). The placeholder fails the check; after the
/// link `vdm-stm-image-check patch` (tools/rust/stm/build_images.sh) writes the record of the
/// image into the .elf and the .bin.
#[used]
#[link_section = ".vdm_app_check"]
static APP_CHECK: [u32; RECORD_WORDS] = [u32::MAX; RECORD_WORDS];

/// The boot stage registers (no state: the registers are the state).
pub struct Regs;

/// Steps 1 to 6 of the boot stage; returns only without a handshake (B1: the only source of
/// a `BootToken`). A function of its own: Renode E8 raises a fault at its entry.
#[inline(never)]
pub fn run() -> BootToken {
    match vdm_stm_boot::run(&mut Regs) {
        BootEnd::Timeout(token) => token,
        BootEnd::Update => jump_to_bootloader(),
    }
}

fn spin_until(mut done: impl FnMut() -> bool) {
    for _ in 0..SPIN_LIMIT {
        if done() {
            return;
        }
    }
}

/// The 2-bit fields (MODER, OSPEEDR, PUPDR) of the pins in `pins`: (field mask, `value` in
/// every field).
fn fields2(pins: u32, value: u32) -> (u32, u32) {
    let mut mask = 0;
    let mut set = 0;
    for p in 0..16 {
        if pins & (1 << p) != 0 {
            mask |= 0b11 << (2 * p);
            set |= value << (2 * p);
        }
    }
    (mask, set)
}

/// MODER 01 (output), push-pull or open drain, no pull.
fn outputs(port: gpio::Gpio, pins: u32, open_drain: bool) {
    let (mask, output) = fields2(pins, 0b01);
    port.otyper().modify(|w| {
        if open_drain {
            w.0 |= pins
        } else {
            w.0 &= !pins
        }
    });
    port.pupdr().modify(|w| w.0 &= !mask);
    port.moder().modify(|w| w.0 = (w.0 & !mask) | output);
}

fn gpio_clocks_on() {
    pac::RCC
        .ahb1enr()
        .modify(|w| w.0 |= AHB1ENR_GPIOA | AHB1ENR_GPIOB | AHB1ENR_GPIOC);
    // the clock is active two cycles after the enable (RM0368 §6.3.10)
    let _ = pac::RCC.ahb1enr().read();
}

fn syst() -> &'static cortex_m::peripheral::syst::RegisterBlock {
    // SAFETY: SysTick registers at their fixed address; the boot stage runs alone
    unsafe { &*SYST::PTR }
}

impl ClockIo for Regs {
    fn tick_start(&mut self, reload: u32) {
        let s = syst();
        // SAFETY: plain register writes, SysTick is not used by anything else in the boot stage
        unsafe {
            s.csr.write(0);
            s.rvr.write(reload);
            s.cvr.write(0);
            s.csr.write(SYST_ENABLE | SYST_CLKSOURCE_CPU);
        }
    }

    fn tick(&mut self) -> bool {
        syst().csr.read() & SYST_COUNTFLAG != 0
    }

    fn tick_stop(&mut self) {
        let s = syst();
        // SAFETY: as tick_start; back to the reset values (as JumpToBootloader of the C++)
        unsafe {
            s.csr.write(0);
            s.rvr.write(0);
            s.cvr.write(0);
        }
    }

    fn hse_on(&mut self) {
        pac::RCC.cr().modify(|w| w.0 |= CR_HSEON);
    }

    fn hse_ready(&mut self) -> bool {
        pac::RCC.cr().read().0 & CR_HSERDY != 0
    }

    fn hse_off(&mut self) {
        pac::RCC.cr().modify(|w| w.0 &= !CR_HSEON);
    }

    fn sysclk_hse(&mut self) {
        // 25 MHz needs no flash wait state (reset value 0)
        pac::RCC
            .cfgr()
            .modify(|w| w.0 = (w.0 & !CFGR_SW) | CFGR_SW_HSE);
        spin_until(|| pac::RCC.cfgr().read().0 & CFGR_SWS == CFGR_SWS_HSE);
    }
}

impl BootIo for Regs {
    fn led_begin(&mut self) {
        gpio_clocks_on();
        outputs(pac::GPIOC, LED_PC_MASK, false);
    }

    fn set_led(&mut self, high: bool) {
        pac::GPIOC.bsrr().write_value(gpio::regs::Bsrr(if high {
            LED_PC_MASK
        } else {
            LED_PC_MASK << 16
        }));
    }

    fn led(&self) -> bool {
        pac::GPIOC.odr().read().0 & LED_PC_MASK != 0
    }

    fn uart_begin(&mut self, brr: u16) {
        pac::RCC.apb2enr().modify(|w| w.0 |= APB2_USART1);
        let _ = pac::RCC.apb2enr().read();
        // PA9 TX, PA10 RX: AF7, high speed, pull-up (STM32duino PinMap_UART of the C++)
        let a = pac::GPIOA;
        let (mask, af) = fields2(USART1_PA_MASK, 0b10);
        let (_, high) = fields2(USART1_PA_MASK, 0b11);
        let (_, up) = fields2(USART1_PA_MASK, 0b01);
        // AFRH: pins 9 and 10 at bits 4..8 and 8..12
        a.afr(1)
            .modify(|w| w.0 = (w.0 & !0x0FF0) | (AF_USART1 << 4) | (AF_USART1 << 8));
        a.otyper().modify(|w| w.0 &= !USART1_PA_MASK);
        a.ospeedr().modify(|w| w.0 |= high);
        a.pupdr().modify(|w| w.0 = (w.0 & !mask) | up);
        a.moder().modify(|w| w.0 = (w.0 & !mask) | af);
        // 115200 8E1: 9-bit words with the parity bit, even parity, 1 stop bit
        let u = pac::USART1;
        u.cr1().write_value(usart::regs::Cr1(0));
        u.cr2().write_value(usart::regs::Cr2Usart(0));
        u.cr3().write_value(usart::regs::Cr3Usart(0));
        u.brr().write_value(usart::regs::Brr(u32::from(brr)));
        u.cr1()
            .write_value(usart::regs::Cr1(CR1_UE | CR1_M | CR1_PCE | CR1_TE | CR1_RE));
    }

    fn rx(&mut self) -> Option<u8> {
        let u = pac::USART1;
        // RXNE also with PE, FE, NE or ORE: SR then DR clears them; the byte is kept as in C++
        if u.sr().read().0 & SR_RXNE != 0 {
            Some(u.dr().read().0 as u8)
        } else {
            None
        }
    }

    fn tx(&mut self, byte: u8) {
        let u = pac::USART1;
        spin_until(|| u.sr().read().0 & SR_TXE != 0);
        u.dr().write_value(usart::regs::Dr(u32::from(byte)));
    }

    fn flush(&mut self) {
        let u = pac::USART1;
        spin_until(|| u.sr().read().0 & SR_TC != 0);
    }

    fn uart_end(&mut self) {
        pac::RCC.apb2rstr().modify(|w| w.0 |= APB2_USART1);
        pac::RCC.apb2rstr().modify(|w| w.0 &= !APB2_USART1);
    }
}

impl BootHw for Regs {
    fn reset_flags(&mut self) -> u32 {
        pac::RCC.csr().read().0
    }

    fn clear_reset_flags(&mut self) {
        pac::RCC.csr().modify(|w| w.0 |= CSR_RMVF);
    }

    fn noinit(&mut self) -> [u8; NOINIT_LEN] {
        noinit::read()
    }

    fn set_noinit(&mut self, cells: &[u8; NOINIT_LEN]) {
        noinit::write(cells);
    }

    fn outputs_safe(&mut self) {
        gpio_clocks_on();
        // PSU: latch high before open drain, as valve_pins_safe (reset latch LOW would switch
        // the valve PSU on for a moment), and once more after
        pac::GPIOB.bsrr().write_value(gpio::regs::Bsrr(PSU_PB_MASK));
        outputs(pac::GPIOB, PSU_PB_MASK, true);
        pac::GPIOB.bsrr().write_value(gpio::regs::Bsrr(PSU_PB_MASK));
        // ENA0..5 latches low, then push-pull outputs (PA15, PB3 leave JTAG)
        pac::GPIOA
            .bsrr()
            .write_value(gpio::regs::Bsrr(ENA_PORT_A_MASK << 16));
        pac::GPIOB
            .bsrr()
            .write_value(gpio::regs::Bsrr(ENA_PORT_B_MASK << 16));
        outputs(pac::GPIOA, ENA_PORT_A_MASK, false);
        outputs(pac::GPIOB, ENA_PORT_B_MASK, false);
    }

    fn boot_id(&mut self) -> BootId {
        // zeros, then the flash bytes: a copy of BootId::STANDARD would come from .rodata
        let mut id = BootId {
            pattern: [0; 8],
            reply: [0; 6],
        };
        flash_bytes(LAYOUT.pattern, &mut id.pattern);
        flash_bytes(LAYOUT.reply, &mut id.reply);
        id
    }

    fn watchdog_start(&mut self) {
        watchdog_start();
    }

    fn app_record(&mut self) -> [u32; RECORD_WORDS] {
        // SAFETY: the record in sector 0; volatile, as the patch step changed it after the link
        unsafe { core::ptr::read_volatile(core::ptr::addr_of!(APP_CHECK)) }
    }
}

impl FlashRead for Regs {
    fn flash_read(&mut self, addr: u32, out: &mut [u8]) {
        for (i, b) in out.iter_mut().enumerate() {
            // SAFETY: the application part of the image, at most APP_MAX_LEN above APP_START
            // (vdm_stm_boot::app_check): inside the flash of both chips
            *b = unsafe { core::ptr::read_volatile((addr as usize + i) as *const u8) };
        }
    }
}

/// `IWatchdog.begin(8000000)` on the registers: the boot stage at the end of a window without
/// handshake, the application stage again before `embassy_stm32::init` (§5.4). A second call
/// changes nothing: the start key leaves a running IWDG running, PR and RLR get the same values,
/// the reload restarts the 8 s.
pub fn watchdog_start() {
    iwdg_key(IWDG_START);
    iwdg_key(IWDG_ENABLE);
    pac::IWDG.pr().write_value(iwdg::regs::Pr(IWDG_PR_DIV64));
    pac::IWDG.rlr().write_value(iwdg::regs::Rlr(IWDG_RLR_8S));
    // PVU, RVU: the new values reach the LSI domain within a few LSI cycles
    spin_until(|| pac::IWDG.sr().read().0 == 0);
    iwdg_key(IWDG_RELOAD);
}

/// Volatile reads from the ID block in flash: the bytes the ESP validated, not constants the
/// compiler could fold.
pub fn flash_bytes(offset: usize, out: &mut [u8]) {
    let base = ID_BLOCK_ADDR as usize + offset;
    for (i, b) in out.iter_mut().enumerate() {
        // SAFETY: inside the ID block (offset + len <= LAYOUT.len <= 64) in flash
        *b = unsafe { core::ptr::read_volatile((base + i) as *const u8) };
    }
}

/// A field of the ID block in flash as a slice (`gvers` and the banner of the application read
/// version and tag from it). `offset + len` beyond the block gives an empty slice.
#[cfg(feature = "app")]
pub fn id_field(offset: usize, len: usize) -> &'static [u8] {
    if offset.saturating_add(len) > LAYOUT.len {
        return &[];
    }
    // SAFETY: inside the ID block (checked above), in flash, never written by the firmware (B4)
    unsafe { core::slice::from_raw_parts((ID_BLOCK_ADDR as usize + offset) as *const u8, len) }
}

/// Step 6a, as `JumpToBootloader` of the C++ (`HAL_RCC_DeInit`, SysTick off,
/// `__disable_irq`, MEMRMP, MSP, jump). VTOR stays 0: with MEMRMP = 01 address 0 is the
/// system memory (R2: only hardware proves the ROM bootloader start, §5.9).
pub fn jump_to_bootloader() -> ! {
    // clocks back to the reset state: HSI on and SYSCLK, prescalers 1, HSE and PLLs off
    pac::RCC.cr().modify(|w| w.0 |= CR_HSION);
    spin_until(|| pac::RCC.cr().read().0 & CR_HSIRDY != 0);
    pac::RCC.cfgr().write_value(rcc::regs::Cfgr(0));
    spin_until(|| pac::RCC.cfgr().read().0 & CFGR_SWS == CFGR_SWS_HSI);
    pac::RCC
        .cr()
        .modify(|w| w.0 &= !(CR_HSEON | CR_HSEBYP | CR_CSSON | CR_PLLON | CR_PLLI2SON));
    let mut regs = Regs;
    regs.tick_stop();
    cortex_m::interrupt::disable();
    // system memory at address 0
    pac::RCC.apb2enr().modify(|w| w.0 |= APB2ENR_SYSCFG);
    let _ = pac::RCC.apb2enr().read();
    pac::SYSCFG.memrm().write_value(syscfg::regs::Memrm(0b01));
    // SAFETY: the ROM bootloader's vector table at 0x1FFF0000 (AN2606); never returns
    unsafe { cortex_m::asm::bootload(0x1FFF_0000 as *const u32) }
}

/// IWDG key register (fault.rs, app.rs).
pub fn iwdg_key(key: u16) {
    pac::IWDG.kr().write_value(iwdg::regs::Kr(u32::from(key)));
}
