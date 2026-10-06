//! D9 (docs/rust/GLUE-DESIGN-STM.md §8): the boot stage and everything it calls lie in flash
//! sector 0 (0x08000000-0x08003FFF), so an ESP flash that writes sector 0 last and stops
//! before it leaves a working boot window.
//!
//! The walk starts at the reset vector and at every core exception vector (the fault handlers
//! end in the IWDG reset that brings the boot stage back) and follows calls, branches and
//! function pointers (literals and MOVW/MOVT pairs that hold a code address) of every function
//! it reaches, except into the application entry. Every reached function and every flash datum
//! it addresses must lie below `limit`: the end of `.vdm_boot`, which the linker script keeps
//! inside sector 0. Below it, not merely below 0x08004000: `.text` and `.rodata` move out of
//! sector 0 as the application grows. No reached function may touch `.data` either (its initial
//! values are copied from outside sector 0), except the start-up code that copies it.

use std::collections::{BTreeMap, BTreeSet};

use crate::elf::{Elf, STT_FUNC};
use crate::thumb::{decode, len, Op};

pub const FLASH: u32 = 0x0800_0000;
pub const SECTOR0_END: u32 = 0x0800_4000;

#[derive(Debug, Default)]
pub struct Report {
    /// reached functions: start -> (name, size)
    pub reached: BTreeMap<u32, (String, u32)>,
    pub problems: Vec<String>,
    /// end of the highest flash byte the boot stage uses (code or data)
    pub top: u32,
    pub indirect: Vec<String>,
}

struct Func {
    start: u32,
    end: u32,
    name: String,
}

/// Code ranges of a function without its literal pools (`$d` .. next `$t`).
fn code_ranges(elf: &Elf, f: &Func) -> Vec<(u32, u32)> {
    let mut marks: Vec<(u32, bool)> = elf
        .symbols
        .iter()
        .filter(|s| s.value >= f.start && s.value < f.end)
        .filter_map(|s| {
            if s.name == "$t" || s.name.starts_with("$t.") {
                Some((s.value & !1, true))
            } else if s.name == "$d" || s.name.starts_with("$d.") {
                Some((s.value, false))
            } else {
                None
            }
        })
        .collect();
    marks.sort();
    let mut ranges = Vec::new();
    let mut code_from = Some(f.start);
    for (at, code) in marks {
        match (code, code_from) {
            (false, Some(from)) => {
                if at > from {
                    ranges.push((from, at));
                }
                code_from = None;
            }
            (true, None) => code_from = Some(at),
            _ => {}
        }
    }
    if let Some(from) = code_from {
        ranges.push((from, f.end));
    }
    ranges
}

fn functions(elf: &Elf) -> Vec<Func> {
    let mut fs: Vec<Func> = elf
        .symbols
        .iter()
        .filter(|s| s.kind == STT_FUNC && s.size > 0)
        .map(|s| Func {
            start: s.value & !1,
            end: (s.value & !1) + s.size,
            name: s.name.clone(),
        })
        .collect();
    fs.sort_by_key(|f| f.start);
    fs.dedup_by_key(|f| f.start);
    fs
}

fn containing(fs: &[Func], addr: u32) -> Option<&Func> {
    let i = fs.partition_point(|f| f.start <= addr);
    fs.get(i.checked_sub(1)?).filter(|f| addr < f.end)
}

/// The name and the end of the data object at `addr` (a sized symbol), else (`None`, addr + 1).
fn data_object(elf: &Elf, addr: u32) -> (Option<&str>, u32) {
    elf.symbols
        .iter()
        .filter(|s| s.size > 0 && s.kind != STT_FUNC && addr >= s.value && addr < s.value + s.size)
        .map(|s| (Some(s.name.as_str()), s.value + s.size))
        .next()
        .unwrap_or((None, addr + 1))
}

/// Walks the boot stage. `stop`: functions not followed (the application entry);
/// `allowed`: (function, value) pairs exempt from the data rules (the start-up code's `.data`
/// copy: its LMA and its VMA).
pub fn check(elf: &Elf, limit: u32, stop: &[u32], allowed: &[(u32, u32)]) -> Report {
    let fs = functions(elf);
    let mut report = Report::default();
    let flash_end = elf
        .loaded()
        .filter(|s| s.addr >= FLASH && s.addr < 0x1000_0000)
        .map(|s| s.end())
        .max()
        .unwrap_or(FLASH);
    let data = elf.section(".data").map(|s| (s.addr, s.end()));

    let mut todo: Vec<u32> = (1..16u32)
        .filter_map(|i| elf.u32(FLASH + 4 * i))
        .filter(|&v| v != 0 && v & 1 == 1)
        .map(|v| v & !1)
        .collect();
    let mut seen = BTreeSet::new();
    while let Some(addr) = todo.pop() {
        let Some(f) = containing(&fs, addr) else {
            report
                .problems
                .push(format!("branch to {addr:#010x}: no function there"));
            continue;
        };
        if stop.contains(&f.start) || !seen.insert(f.start) {
            continue;
        }
        report
            .reached
            .insert(f.start, (f.name.clone(), f.end - f.start));
        report.top = report.top.max(f.end);
        if f.end > limit {
            report.problems.push(format!(
                "{} at {:#010x}..{:#010x} is outside the boot stage (ends {limit:#010x})",
                f.name, f.start, f.end
            ));
        }
        let use_address = |value: u32, report: &mut Report, todo: &mut Vec<u32>| {
            if allowed.contains(&(f.start, value)) {
                return;
            }
            if value >= FLASH && value < flash_end {
                if let Some(target) = containing(&fs, value & !1) {
                    if value & 1 == 1 || target.start == value {
                        todo.push(value & !1);
                        return;
                    }
                }
                let (name, end) = data_object(elf, value);
                report.top = report.top.max(end);
                if end > limit {
                    report.problems.push(format!(
                        "{} reads flash data {} at {value:#010x} outside the boot stage",
                        f.name,
                        name.unwrap_or("?")
                    ));
                }
            } else if let Some((s, e)) = data {
                if value >= s && value < e {
                    report.problems.push(format!(
                        "{} uses .data at {value:#010x} (initial values outside sector 0)",
                        f.name
                    ));
                }
            }
        };
        let mut movw: [Option<u16>; 16] = [None; 16];
        for (from, to) in code_ranges(elf, f) {
            let mut pc = from;
            while pc < to {
                let Some(hw1) = elf.u16(pc) else { break };
                let n = len(hw1);
                let hw2 = if n == 4 {
                    elf.u16(pc + 2).unwrap_or(0)
                } else {
                    0
                };
                let inside = |a: u32| a >= f.start && a < f.end;
                match decode(hw1, hw2, pc) {
                    Op::Call(t) | Op::Branch(t) => {
                        if !inside(t) {
                            todo.push(t);
                        }
                    }
                    Op::Literal { addr, word } => {
                        if !inside(addr) {
                            use_address(addr, &mut report, &mut todo);
                        }
                        if word {
                            if let Some(v) = elf.u32(addr) {
                                use_address(v, &mut report, &mut todo);
                            }
                        }
                    }
                    Op::Adr(a) => {
                        if !inside(a) {
                            use_address(a, &mut report, &mut todo);
                        }
                    }
                    Op::MovW { rd, imm } => movw[usize::from(rd)] = Some(imm),
                    Op::MovT { rd, imm } => {
                        if let Some(lo) = movw[usize::from(rd)].take() {
                            use_address(
                                (u32::from(imm) << 16) | u32::from(lo),
                                &mut report,
                                &mut todo,
                            );
                        }
                    }
                    Op::Indirect => report.indirect.push(format!("{} at {pc:#010x}", f.name)),
                    Op::Table | Op::Return | Op::Other => {}
                }
                pc += n;
            }
        }
    }
    report
}
