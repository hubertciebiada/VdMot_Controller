//! Renode scenario E11 (docs/rust/GLUE-DESIGN-STM.md §5.7): the Rust ESP flasher
//! (`vdm_esp_core::stm_flasher`, D9 and F9: sector 0 in a pass of its own, first for an image
//! with a valid record of its application part, last otherwise) on the host, in lock-step with
//! the emulated STM32. The Renode model `VdmEsp.cs` starts this program and drives it once per
//! millisecond of virtual time; the flasher's transport is that model's end of USART1 and NRST.
//!
//!   vdm-e11 <board tag> <job>...        job: <image.bin> or <image.bin>@<version>
//!
//! The jobs run one after the other, 1 s apart. `@<version>` flashes a copy of the image whose
//! ID-block version string is replaced by one of the same length (so the new image answers
//! another gvers). Each job ends Done when the flasher saw the new application answer gvers
//! with the image's board tag; this program also requires its version.
//!
//! Line protocol (stdin -> stdout), one exchange per tick:
//!   T <now ms> <received bytes as hex | ->
//!   [L <text> | R <result> | END ok|fail]...  O <bytes to send as hex | -> <NRST 0|1> <phase>

use std::collections::VecDeque;
use std::io::{self, BufRead, Write};

use vdm_esp_core::stm_flasher::{
    flash_error_name, flash_phase_name, validate_image, FlashError, FlashImage, FlashOptions,
    FlashPhase, FlashTransport, ImageInfo, StmFlasher,
};

/// pause between two jobs
const PAUSE_MS: u32 = 1000;

/// The flasher's side of USART1 and NRST for one tick.
#[derive(Default)]
struct Port {
    rx: VecDeque<u8>,
    tx: Vec<u8>,
    nrst: bool,
    log: Vec<String>,
}

impl FlashTransport for Port {
    fn configure(&mut self, baud: u32, even_parity: bool) {
        self.log.push(format!(
            "uart {baud} {}",
            if even_parity { "8E1" } else { "8N1" }
        ));
    }

    fn write(&mut self, data: &[u8]) -> usize {
        self.tx.extend_from_slice(data);
        data.len()
    }

    fn read(&mut self, out: &mut [u8]) -> usize {
        let n = out.len().min(self.rx.len());
        for b in out.iter_mut().take(n) {
            *b = self.rx.pop_front().unwrap_or(0);
        }
        n
    }

    fn discard_input(&mut self) {
        self.rx.clear();
    }

    fn set_reset(&mut self, asserted: bool) {
        self.nrst = asserted;
    }
}

/// The image bytes and the copy of its sector 0 the flasher keeps (D9).
struct Image(Vec<u8>, Vec<u8>);

impl FlashImage for Image {
    fn size(&self) -> u32 {
        self.0.len() as u32
    }

    fn read(&mut self, offset: u32, out: &mut [u8]) -> bool {
        let start = offset as usize;
        match self.0.get(start..start + out.len()) {
            Some(src) => {
                out.copy_from_slice(src);
                true
            }
            None => false,
        }
    }

    fn hold_low(&mut self, len: u32) -> bool {
        self.1.clear();
        match self.0.get(..len as usize) {
            Some(src) => {
                self.1.extend_from_slice(src);
                true
            }
            None => false,
        }
    }

    fn low(&self) -> &[u8] {
        &self.1
    }
}

struct Job {
    path: String,
    image: Image,
    version: String,
}

fn text(t: &[u8]) -> String {
    String::from_utf8_lossy(t).into_owned()
}

/// Reads the image, patches its version string if asked; the version gvers must report.
fn load(spec: &str) -> Result<Job, String> {
    let (path, patch) = match spec.split_once('@') {
        Some((p, v)) => (p, Some(v)),
        None => (spec, None),
    };
    let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    let mut image = Image(bytes, Vec::new());
    let mut info = ImageInfo::default();
    let e = validate_image(&mut image, 0, true, &mut info);
    if e != FlashError::None {
        return Err(format!("{path}: {}", flash_error_name(e)));
    }
    let old = text(&info.version);
    let version = match patch {
        None => old,
        Some(new) => {
            if new.len() != old.len() {
                return Err(format!("{path}: {new} is not as long as {old}"));
            }
            let find = [b"\0".as_slice(), old.as_bytes(), b"\0"].concat();
            let at = image
                .0
                .windows(find.len())
                .position(|w| w == find.as_slice())
                .ok_or(format!("{path}: version {old} not found"))?;
            image.0[at + 1..at + 1 + new.len()].copy_from_slice(new.as_bytes());
            new.to_string()
        }
    };
    Ok(Job {
        path: path.to_string(),
        image,
        version,
    })
}

fn hex(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return "-".to_string();
    }
    bytes.iter().map(|b| format!("{b:02X}")).collect()
}

fn unhex(s: &str) -> Vec<u8> {
    if s == "-" {
        return Vec::new();
    }
    (0..s.len() / 2)
        .filter_map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok())
        .collect()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("usage: vdm-e11 <board tag> <image.bin>[@<version>]...");
        std::process::exit(2);
    }
    let board = args[0].clone();
    let mut jobs = Vec::new();
    for spec in &args[1..] {
        match load(spec) {
            Ok(job) => jobs.push(job),
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(2);
            }
        }
    }

    let mut opt = FlashOptions::default();
    opt.board_hw.extend_from_slice(board.as_bytes()).ok();
    let mut flasher = StmFlasher::new();
    let mut port = Port::default();
    let mut next = 0usize;
    let mut idle_since: Option<u32> = Some(0);
    let mut failed = false;
    let mut ended = false;
    let mut last_phase = FlashPhase::Idle;

    let stdin = io::stdin();
    let mut out = io::BufWriter::new(io::stdout().lock());
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let mut parts = line.split_whitespace();
        if parts.next() != Some("T") {
            continue;
        }
        let now: u32 = parts.next().and_then(|t| t.parse().ok()).unwrap_or(0);
        port.rx.extend(unhex(parts.next().unwrap_or("-")));

        if flasher.active() {
            if let Some(job) = jobs.get_mut(next - 1) {
                flasher.step(&mut port, &mut job.image, now);
            }
            let st = flasher.status();
            if st.phase != last_phase {
                port.log
                    .push(format!("phase {} at {now} ms", flash_phase_name(st.phase)));
                last_phase = st.phase;
            }
            if !flasher.active() {
                let job = &jobs[next - 1];
                let app = &st.app_version;
                let app_text = format!(
                    "{}.{}.{}{}_{}",
                    app.major,
                    app.minor,
                    app.patch,
                    text(&app.suffix),
                    text(&app.hw)
                );
                let want = format!("{}_{}", job.version, board);
                let ok = st.phase == FlashPhase::Done && app_text == want;
                writeln!(
                    out,
                    "R job {} {} {} error {} at {} pid 0x{:03X} bootloader 0x{:02X} bytes {} app {} want {} {}",
                    next,
                    job.path,
                    flash_phase_name(st.phase),
                    flash_error_name(st.error),
                    flash_phase_name(st.error_phase),
                    st.chip_pid,
                    st.bootloader_version,
                    st.bytes_total,
                    app_text,
                    want,
                    if ok { "ok" } else { "FAIL" }
                )
                .ok();
                failed |= !ok;
                idle_since = Some(now);
            }
        } else if let Some(since) = idle_since {
            if next < jobs.len() && !failed {
                if now.wrapping_sub(since) >= PAUSE_MS {
                    port.log.push(format!(
                        "job {} {} version {} at {now} ms",
                        next + 1,
                        jobs[next].path,
                        jobs[next].version
                    ));
                    flasher.begin(&opt, now);
                    next += 1;
                    idle_since = None;
                    if let Some(job) = jobs.get_mut(next - 1) {
                        flasher.step(&mut port, &mut job.image, now);
                    }
                }
            } else if !ended {
                writeln!(out, "END {}", if failed { "fail" } else { "ok" }).ok();
                ended = true;
            }
        }

        for l in port.log.drain(..) {
            writeln!(out, "L {l}").ok();
        }
        let phase = if ended {
            "end"
        } else {
            flash_phase_name(flasher.status().phase)
        };
        writeln!(out, "O {} {} {}", hex(&port.tx), u8::from(port.nrst), phase).ok();
        port.tx.clear();
        out.flush().ok();
    }
}
