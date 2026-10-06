//! Image check of the STM32 firmware (docs/rust/GLUE-DESIGN-STM.md §5.8): ELF layout (C4),
//! size (C5) and the D9 sector-0 proof of one image. The ESP side (C1-C3) runs the C++
//! stm_flasher.cpp on the same image (esp_validate.cpp); tools/rust/stm/image_check.sh runs
//! both on the four images.
//!
//!   vdm-stm-image-check <image.elf> <image.bin> <f401|f411> <C1|C2> <version>

mod elf;
mod sector0;
mod thumb;

use std::process::ExitCode;

use elf::Elf;

/// D12: the image budget (5 erase sectors, as the C++ images).
const BUDGET: u32 = 128 * 1024;
const ID_BLOCK: u32 = 0x0800_0200;
const NOINIT: (u32, u32) = (0x2000_3234, 0x2000_3308);

struct Checks {
    failures: u32,
}

impl Checks {
    fn check(&mut self, ok: bool, what: String) {
        println!("  {} {what}", if ok { "ok  " } else { "FAIL" });
        if !ok {
            self.failures += 1;
        }
    }
}

/// Printable runs ending in NUL, as the ESP scan sees them: (offset, text); runs of 32 or more
/// characters are not kept.
fn runs(bin: &[u8]) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut start = 0;
    for (i, &b) in bin.iter().enumerate() {
        if b == 0 {
            if i > start && i - start < 32 {
                out.push((start, String::from_utf8_lossy(&bin[start..i]).into_owned()));
            }
            start = i + 1;
        } else if !(0x20..=0x7E).contains(&b) {
            start = i + 1;
        }
    }
    out
}

/// The ESP's version grammar in short: M.m.p, then a suffix starting with - _ +.
fn version_like(s: &str) -> bool {
    let mut parts = s.splitn(3, '.');
    let (Some(a), Some(b), Some(rest)) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    let digits = |t: &str| !t.is_empty() && t.len() <= 5 && t.bytes().all(|c| c.is_ascii_digit());
    let p = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    let (patch, suffix) = rest.split_at(p);
    digits(a)
        && digits(b)
        && digits(patch)
        && (suffix.is_empty()
            || (suffix.starts_with(['-', '_', '+'])
                && suffix
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"._+-".contains(&c))))
}

fn count(hay: &[u8], needle: &[u8]) -> usize {
    hay.windows(needle.len()).filter(|w| *w == needle).count()
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 6 {
        eprintln!(
            "usage: {} <image.elf> <image.bin> <f401|f411> <C1|C2> <version>",
            args[0]
        );
        return ExitCode::from(2);
    }
    let (elf_path, bin_path, chip, tag, version) =
        (&args[1], &args[2], &args[3], &args[4], &args[5]);
    let elf = match std::fs::read(elf_path)
        .map_err(|e| e.to_string())
        .and_then(Elf::parse)
    {
        Ok(e) => e,
        Err(e) => {
            eprintln!("{elf_path}: {e}");
            return ExitCode::from(2);
        }
    };
    let bin = match std::fs::read(bin_path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("{bin_path}: {e}");
            return ExitCode::from(2);
        }
    };
    let ram_top = if chip == "f401" {
        0x2001_0000
    } else {
        0x2002_0000
    };
    let mut c = Checks { failures: 0 };
    println!("{elf_path}: layout, size, sector 0");

    // C4: vectors
    let sp = elf.u32(0x0800_0000).unwrap_or(0);
    let reset = elf.u32(0x0800_0004).unwrap_or(0);
    let reset_sym = elf.symbol("Reset").map_or(0, |s| s.value | 1);
    c.check(
        sp == ram_top,
        format!("C4 SP0 {sp:#010x} (expected {ram_top:#010x})"),
    );
    c.check(
        reset == reset_sym && reset & 1 == 1,
        format!("C4 reset vector {reset:#010x} = Reset"),
    );
    // C4: the bin is the flash image of the ELF
    let same = elf
        .loaded()
        .filter(|s| s.addr >= 0x0800_0000 && s.addr < 0x0900_0000 && s.size > 0)
        .all(|s| {
            elf.bytes(s.addr, s.size)
                == bin.get((s.addr - 0x0800_0000) as usize..(s.end() - 0x0800_0000) as usize)
        });
    c.check(same, "C4 .bin holds the flash sections of the ELF".into());

    // C4: ID block first, one marker
    let id = elf.section(".vdm_id");
    c.check(
        id.is_some_and(|s| s.addr == ID_BLOCK && s.size <= 64),
        format!(
            "C4 .vdm_id at {:#010x}, {} bytes (at 0x08000200, at most 64)",
            id.map_or(0, |s| s.addr),
            id.map_or(0, |s| s.size)
        ),
    );
    let first = runs(&bin).into_iter().find(|(_, s)| version_like(s));
    c.check(
        first.as_ref().is_some_and(|(at, s)| {
            let at = 0x0800_0000 + *at as u32;
            id.is_some_and(|i| i.contains(at)) && s == version
        }),
        format!(
            "C4 first version-like run {:?} in the ID block (expected \"{version}\")",
            first
        ),
    );
    let markers = count(&bin, b"VDM-HW:");
    let tagged = count(&bin, format!("VDM-HW:{tag}\0").as_bytes());
    c.check(
        markers == 1 && tagged == 1,
        format!("C4 one VDM-HW: marker ({markers}), VDM-HW:{tag}"),
    );
    c.check(
        count(&bin, b"DEADBEEF") >= 1 && count(&bin, b"BEEFIT") >= 1,
        "C4 DEADBEEF and BEEFIT present".into(),
    );

    // C4: no-init cells at the C++ 2.1.7 addresses, nothing allocated there
    for (name, want) in [
        ("__vdm_noinit", 0x2000_3234),
        ("__vdm_guard_cell", 0x2000_32E8),
        ("__vdm_reset_cell", 0x2000_32FC),
    ] {
        let got = elf.symbol(name).map(|s| s.value);
        let shown = got.map_or_else(|| "missing".to_string(), |v| format!("{v:#010x}"));
        c.check(
            got == Some(want),
            format!("C4 {name} = {shown} (expected {want:#010x})"),
        );
    }
    let inside: Vec<&str> = elf
        .sections
        .iter()
        .filter(|s| {
            s.flags & elf::SHF_ALLOC != 0 && s.size > 0 && s.addr < NOINIT.1 && s.end() > NOINIT.0
        })
        .map(|s| s.name.as_str())
        .collect();
    c.check(
        inside.is_empty(),
        format!("C4 no section in NOINIT 0x20003234..0x20003308 {inside:?}"),
    );
    let ram: Vec<String> = [".data", ".bss", ".uninit"]
        .iter()
        .filter_map(|n| elf.section(n))
        .filter(|s| s.size > 0 && (s.addr < NOINIT.1 || s.end() > ram_top))
        .map(|s| s.name.clone())
        .collect();
    c.check(
        ram.is_empty(),
        format!("C4 .data, .bss, .uninit inside RAM 0x20003308..{ram_top:#010x} {ram:?}"),
    );

    // C5: size
    let size = bin.len() as u32;
    c.check(
        size <= BUDGET,
        format!(
            "C5 size {size} B = {:.1} % of the 128 KiB budget (D12)",
            f64::from(size) * 100.0 / f64::from(BUDGET)
        ),
    );

    // D9: boot stage in sector 0
    let boot_end = elf.symbol("__vdm_boot_end").map_or(u32::MAX, |s| s.value);
    let boot_start = elf.symbol("__vdm_boot_start").map_or(0, |s| s.value);
    c.check(
        boot_end <= sector0::SECTOR0_END,
        format!(
            "D9 .vdm_boot {boot_start:#010x}..{boot_end:#010x} ({} B) inside sector 0",
            boot_end.wrapping_sub(boot_start)
        ),
    );
    let stop: Vec<u32> = elf
        .symbols
        .iter()
        .filter(|s| {
            s.kind == elf::STT_FUNC
                && (s.name.ends_with("3app3run") || s.name.contains("3app3run17h"))
        })
        .map(|s| s.value & !1)
        .collect();
    c.check(
        stop.len() == 1,
        format!(
            "D9 application entry app::run found ({} symbols)",
            stop.len()
        ),
    );
    // the start-up code copies .data: its LMA and VMA are the only data it may address
    let reset_fn = elf.symbol("Reset").map_or(0, |s| s.value & !1);
    let allowed: Vec<(u32, u32)> = ["__sidata", "__sdata"]
        .iter()
        .filter_map(|n| elf.symbol(n))
        .map(|s| (reset_fn, s.value))
        .collect();
    let report = sector0::check(&elf, boot_end, &stop, &allowed);
    for p in &report.problems {
        println!("       {p}");
    }
    c.check(
        report.problems.is_empty() && !report.reached.is_empty() && report.top <= boot_end,
        format!(
            "D9 boot stage reaches {} functions; their code and flash data end at {:#010x}, inside .vdm_boot",
            report.reached.len(),
            report.top
        ),
    );
    for i in &report.indirect {
        println!("       indirect branch (target not followed): {i}");
    }
    if std::env::var_os("VDM_LIST_BOOT").is_some() {
        for (a, (n, s)) in &report.reached {
            println!("       {a:#010x} {s:5} {n}");
        }
    }
    println!("  {}", if c.failures == 0 { "PASS" } else { "FAILED" });
    if c.failures == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
