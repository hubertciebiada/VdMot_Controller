//! The application stage (docs/rust/GLUE-DESIGN-STM.md §5.2, §5.4): the IWDG first (B6: the
//! window is over), the HSE probe, `embassy_stm32::init`, then the main loop.
//!
//! TODO(glue): this is the skeleton the glue port replaces (main_loop.rs, communication.rs
//! with the interrupt-driven rings of §4.1, the watchdog feed tied to the valve loop). Today it
//! answers the ESP's start-up requests `gvers`, `gproto` and `ghwin` byte for byte as C++ 2.1.7
//! (`communication_loop`, `communication_dispatch`) and feeds the IWDG in the 10 ms branch.

use embassy_stm32::mode::Blocking;
use embassy_stm32::usart::{Config, Uart};
use embassy_time::{Duration, Instant};
use stm32_metapac as pac;
use vdm_stm_boot::stage::probe_app_hse;
use vdm_stm_boot::BootToken;
use vdm_stm_core::buf_writer::StaticBufWriter;
use vdm_stm_core::line_assembler::StaticLineAssembler;
use vdm_stm_core::replies_v2::format_protocol_version;
use vdm_stm_core::tokenizer::Tokenizer;

use crate::boot_hw::{flash_bytes, iwdg_key, Regs};
use crate::{clocks, fault, id};

const IWDG_ENABLE: u16 = 0x5555;
const IWDG_RELOAD: u16 = 0xAAAA;
const IWDG_START: u16 = 0xCCCC;
/// `IWatchdog.begin(8000000)`: prescaler /64, reload 3999 = 8 s at 32 kHz (5.4-15 s LSI)
const IWDG_PR_DIV64: u32 = 4;
const IWDG_RLR_8S: u32 = 3999;

/// `COMM_LINE_SIZE`, `NO_OF_ARGS`, `COMM_LINE_TIMEOUT_MS` of communication.cpp
const COMM_LINE_SIZE: usize = 128;
const NO_OF_ARGS: u8 = 5;
const COMM_LINE_TIMEOUT_MS: u64 = 100;
/// the 10 ms branch of `loop_system` runs when more than 10 ms passed
const BRANCH_10MS: Duration = Duration::from_millis(10);

const USART1_SR_RXNE: u32 = 1 << 5;

/// The application stage; never returns.
#[inline(never)]
pub fn run(token: BootToken) -> ! {
    watchdog_start();
    let hse = probe_app_hse(&mut Regs, token.hse());
    let p = embassy_stm32::init(clocks::config(hse));
    let mut config = Config::default();
    config.baudrate = 115_200;
    let Ok(mut uart) = Uart::new_blocking(p.USART1, p.PA10, p.PA9, config) else {
        fault::on_panic()
    };

    let mut line = StaticLineAssembler::<COMM_LINE_SIZE>::default();
    let mut last_byte = Instant::now();
    let mut last_10ms = Instant::now();
    loop {
        // communication_loop: bytes up to a complete line, then the request
        while !line.has_line() {
            let Some(byte) = rx() else { break };
            line.push(byte);
            last_byte = Instant::now();
        }
        if line.has_line() {
            dispatch(line.line(), &mut uart);
            line.release();
        } else if line.partial()
            && last_byte.elapsed() > Duration::from_millis(COMM_LINE_TIMEOUT_MS)
        {
            line.reset();
        }

        // 10 ms branch. TODO(glue): reload only while the valve loop (TIM2) advances
        if last_10ms.elapsed() > BRANCH_10MS {
            last_10ms = Instant::now();
            iwdg_key(IWDG_RELOAD);
        }
    }
}

/// `IWatchdog.begin(8000000)` on the registers, before `embassy_stm32::init` (§5.4).
fn watchdog_start() {
    iwdg_key(IWDG_START);
    iwdg_key(IWDG_ENABLE);
    pac::IWDG
        .pr()
        .write_value(pac::iwdg::regs::Pr(IWDG_PR_DIV64));
    pac::IWDG
        .rlr()
        .write_value(pac::iwdg::regs::Rlr(IWDG_RLR_8S));
    // PVU, RVU: the new values reach the LSI domain within a few LSI cycles
    for _ in 0..100_000 {
        if pac::IWDG.sr().read().0 == 0 {
            break;
        }
    }
    iwdg_key(IWDG_RELOAD);
}

fn rx() -> Option<u8> {
    let u = pac::USART1;
    (u.sr().read().0 & USART1_SR_RXNE != 0).then(|| u.dr().read().0 as u8)
}

/// `communication_dispatch` for the requests of the skeleton; unknown commands get no reply.
fn dispatch(line: &[u8], uart: &mut Uart<'_, Blocking>) {
    let mut req = Tokenizer::default();
    if !req.parse(line, NO_OF_ARGS) {
        return;
    }
    let mut out = StaticBufWriter::<96>::default();
    if req.is(b"gvers") {
        // "gvers <version>_<board revision> <build> " CR LF
        let mut version = [0u8; vdm_stm_boot::id_block::VERSION_MAX];
        let mut tag = [0u8; 3];
        let version = read_id(id::LAYOUT.version, id::LAYOUT.version_len, &mut version);
        let tag = read_id(id::LAYOUT.tag, id::LAYOUT.tag_len, &mut tag);
        out.append(b"gvers ");
        out.append(version);
        out.append(b"_");
        out.append(tag);
        out.append(b" ");
        out.append(id::BUILD);
        out.append(b" \r\n");
    } else if req.is(b"ghwin") {
        // "ghwin <HAL_GetDEVID()> " CR LF
        out.append(b"ghwin ");
        out.append_unsigned(u32::from(pac::DBGMCU.idcode().read().dev_id()) & 0xFFF);
        out.append(b" \r\n");
    } else if req.is(b"gproto") {
        if !format_protocol_version(&mut out) {
            return;
        }
        out.append(b"\r\n");
    } else {
        return;
    }
    let _ = uart.blocking_write(out.as_bytes());
    let _ = uart.blocking_flush();
}

/// A field of the ID block in flash (the bytes the ESP validated).
fn read_id(offset: usize, len: usize, buf: &mut [u8]) -> &[u8] {
    let n = len.min(buf.len());
    let field = &mut buf[..n];
    flash_bytes(offset, field);
    field
}
