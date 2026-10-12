//! The goldens of the C++ glue_system scenarios (design §7.4): `glue/tests/golden/<slug>.txt`,
//! written by `tools/rust/stm/golden` (recorder.cpp holds the format), and the transcript the
//! Rust bench records in the same shape. A system test compares its whole transcript with the
//! C++ one: every byte both UARTs carried, at the same millisecond, the EEPROM and the no-init
//! cells at the end of every boot. A gvlvy reply is compared on the values of the golden's reply:
//! protocol 3 adds values at the end (PROTOCOL_V2.md, extension rule), the C++ goldens carry 25.
//!
//! `VDM_GOLDEN_RECORD=1` makes a system test write its transcript as its golden instead of
//! comparing it, marked `recorder rust`; it keeps a golden of the C++ recorder unless the value
//! is `force`. RUST_RECORDED names the goldens written so.

use std::fmt::Write as _;
use std::format;
use std::string::{String, ToString};
use std::vec::Vec;

use crate::hal::NOINIT_SIZE;
use crate::test_support::eeprom_fake::EEPROM_SIZE;

/// A reset of the case (testkit::Reset).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reset {
    PowerOn,
    Pin,
    Software,
    Watchdog,
}

impl Reset {
    pub fn name(self) -> &'static str {
        match self {
            Reset::PowerOn => "power-on",
            Reset::Pin => "pin",
            Reset::Software => "software",
            Reset::Watchdog => "watchdog",
        }
    }
}

/// USART1 in, USART1 out, USART6 out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    Rx,
    Tx,
    Dbg,
}

impl Dir {
    fn name(self) -> &'static str {
        match self {
            Dir::Rx => "rx",
            Dir::Tx => "tx",
            Dir::Dbg => "dbg",
        }
    }

    fn parse(s: &str) -> Option<Dir> {
        match s {
            "rx" => Some(Dir::Rx),
            "tx" => Some(Dir::Tx),
            "dbg" => Some(Dir::Dbg),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event {
    pub ms: u32,
    pub dir: Dir,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BootRecord {
    pub index: u32,
    pub reset: Reset,
    pub events: Vec<Event>,
    /// "case", "reboot software", ...
    pub end: String,
    pub eeprom: Vec<u8>,
    pub noinit: Vec<u8>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Transcript {
    pub case: String,
    /// written by the Rust bench (`recorder rust`), not by the C++ recorder
    pub rust: bool,
    pub boots: Vec<BootRecord>,
}

/// The goldens the Rust bench wrote (`VDM_GOLDEN_RECORD`): the cases that calibrate a valve,
/// since the coast probes of the calibration (GLUE-DESIGN-STM §7.4).
pub const RUST_RECORDED: [&str; 9] = [
    "calib__w2_5_after_a_calibration_and_a_reset_the_counts_position_and_sta",
    "calib__w2_6_after_a_power_cycle_the_counts_come_back_a_reference_move_i",
    "calib__w2_8_a_calibration_that_ended_blocked_needs_a_full_calibration_a",
    "motion__c_1_a_valve_jammed_at_25_makes_one_calibration_series_and_one_fa",
    "motion__c_2_failsafe_30_after_a_power_cycle_with_a_silent_esp_reference",
    "motion__c_2_failsafe_50_after_a_power_cycle_with_a_silent_esp_reference",
    "motion__k2_c_1_a_blocked_valve_makes_one_failsafe_move_and_waits_for_its",
    "motion__s8_4_a_target_change_of_another_valve_goes_between_two_calibrati",
    "proto3__k2_3_a_calibration_with_every_stroke_too_short_ends_blocked_at_t",
];

/// The bytes as the recorder writes them.
pub fn escape(bytes: &[u8]) -> String {
    let mut s = String::new();
    for &c in bytes {
        match c {
            b'\r' => s.push_str("\\r"),
            b'\n' => s.push_str("\\n"),
            b'\t' => s.push_str("\\t"),
            b'\\' => s.push_str("\\\\"),
            b'"' => s.push_str("\\\""),
            0x20..=0x7E => s.push(char::from(c)),
            _ => {
                let _ = write!(s, "\\x{c:02x}");
            }
        }
    }
    s
}

fn unescape(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'\\' {
            out.push(b[i]);
            i += 1;
            continue;
        }
        match b[i + 1] {
            b'r' => out.push(b'\r'),
            b'n' => out.push(b'\n'),
            b't' => out.push(b'\t'),
            b'x' => {
                let hex = std::str::from_utf8(&b[i + 2..i + 4]).unwrap();
                out.push(u8::from_str_radix(hex, 16).unwrap());
                i += 2;
            }
            other => out.push(other),
        }
        i += 2;
    }
    out
}

fn hex_bytes(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

/// Reads a golden file (lines may end in CR LF after a checkout on Windows).
pub fn parse(text: &str) -> Transcript {
    let mut t = Transcript::default();
    for raw in text.lines() {
        let line = raw.trim_end_matches('\r');
        let (key, rest) = line.split_once(' ').unwrap_or((line, ""));
        match key {
            "case" => t.case = rest.to_string(),
            "recorder" if rest == "rust" => t.rust = true,
            "boot" => {
                let (index, reset) = rest.split_once(' ').unwrap();
                let reset = [Reset::PowerOn, Reset::Pin, Reset::Software, Reset::Watchdog]
                    .into_iter()
                    .find(|r| r.name() == reset)
                    .unwrap_or_else(|| panic!("reset {reset}"));
                t.boots.push(BootRecord {
                    index: index.parse().unwrap(),
                    reset,
                    events: Vec::new(),
                    end: String::new(),
                    eeprom: std::vec![0xFF; EEPROM_SIZE],
                    noinit: Vec::new(),
                });
            }
            "t" => {
                let mut parts = rest.splitn(3, ' ');
                let ms = parts.next().unwrap().parse().unwrap();
                let dir = Dir::parse(parts.next().unwrap()).unwrap();
                let quoted = parts.next().unwrap();
                let inner = &quoted[1..quoted.len() - 1];
                t.boots.last_mut().unwrap().events.push(Event {
                    ms,
                    dir,
                    bytes: unescape(inner),
                });
            }
            "end" => t.boots.last_mut().unwrap().end = rest.to_string(),
            "e" => {
                let (addr, hex) = rest.split_once(' ').unwrap();
                let addr = usize::from_str_radix(addr, 16).unwrap();
                let row = hex_bytes(hex);
                t.boots.last_mut().unwrap().eeprom[addr..addr + row.len()].copy_from_slice(&row);
            }
            "noinit" => t.boots.last_mut().unwrap().noinit = hex_bytes(rest),
            "" => {}
            other => panic!("golden line {other:?}"),
        }
    }
    t
}

fn path(slug: &str) -> String {
    format!("{}/tests/golden/{slug}.txt", env!("CARGO_MANIFEST_DIR"))
}

/// The golden of a case by its slug (`tools/rust/stm/golden/recorder.cpp` slug()).
pub fn load(slug: &str) -> Transcript {
    let path = path(slug);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    parse(&text)
}

/// The transcript in the format of the recorder.
pub fn render(t: &Transcript) -> String {
    let mut s = format!("case {}\n", t.case);
    if t.rust {
        s.push_str("recorder rust\n");
    }
    for b in &t.boots {
        let _ = writeln!(s, "boot {} {}", b.index, b.reset.name());
        for e in &b.events {
            let _ = writeln!(s, "{}", show(e));
        }
        let _ = writeln!(s, "end {}", b.end);
        for row in eeprom_rows(&b.eeprom) {
            let _ = writeln!(s, "{row}");
        }
        let _ = writeln!(s, "noinit {}", escape_hex(&b.noinit));
    }
    s
}

/// Writes the transcript as the golden of a case (`VDM_GOLDEN_RECORD`).
pub fn store(slug: &str, t: &Transcript, force: bool) {
    store_at(&path(slug), t, force);
}

/// Writes the transcript at `path`, marked `recorder rust`; a golden of the C++ recorder there
/// stays unless `force`.
fn store_at(path: &str, t: &Transcript, force: bool) {
    let cpp = std::fs::read_to_string(path).is_ok_and(|old| !parse(&old).rust);
    assert!(
        force || !cpp,
        "{path}: a golden of the C++ recorder; VDM_GOLDEN_RECORD=force replaces it"
    );
    let t = Transcript {
        rust: true,
        ..t.clone()
    };
    std::fs::write(path, render(&t)).unwrap_or_else(|e| panic!("{path}: {e}"));
}

/// The values of a gvlvy reply of one line on USART1, None for any other event.
fn gvlvy_values(e: &Event) -> Option<Vec<&[u8]>> {
    let one_line =
        e.bytes.ends_with(b"\r\n") && e.bytes.iter().filter(|&&b| b == b'\n').count() == 1;
    if e.dir != Dir::Tx || !e.bytes.starts_with(b"gvlvy ") || !one_line {
        return None;
    }
    Some(
        e.bytes[6..e.bytes.len() - 2]
            .split(|&b| b == b' ')
            .collect(),
    )
}

/// The Rust event `r` matches the golden's `g`: a gvlvy reply on the values the golden's reply
/// carries, at the same millisecond; every other event byte for byte.
fn matches(g: &Event, r: &Event) -> bool {
    match (gvlvy_values(g), gvlvy_values(r)) {
        (Some(gv), Some(rv)) => g.ms == r.ms && rv.get(..gv.len()) == Some(&gv[..]),
        _ => g == r,
    }
}

fn show(e: &Event) -> String {
    format!("t {} {} \"{}\"", e.ms, e.dir.name(), escape(&e.bytes))
}

fn eeprom_rows(image: &[u8]) -> Vec<String> {
    image
        .chunks(32)
        .enumerate()
        .filter(|(_, row)| row.iter().any(|&b| b != 0xFF))
        .map(|(i, row)| {
            let hex: String = row.iter().map(|b| format!("{b:02x}")).collect();
            format!("e {:04x} {hex}", i * 32)
        })
        .collect()
}

/// The first difference between the C++ golden and the Rust transcript, as text; None when they
/// are equal.
pub fn diff(golden: &Transcript, rust: &Transcript) -> Option<String> {
    if golden.case != rust.case {
        return Some(format!(
            "case name: C++ {:?}, Rust {:?}",
            golden.case, rust.case
        ));
    }
    for (i, (g, r)) in golden.boots.iter().zip(&rust.boots).enumerate() {
        if (g.index, g.reset) != (r.index, r.reset) {
            return Some(format!(
                "boot {i}: C++ boot {} {}, Rust boot {} {}",
                g.index,
                g.reset.name(),
                r.index,
                r.reset.name()
            ));
        }
        for (k, (ge, re)) in g.events.iter().zip(&r.events).enumerate() {
            if !matches(ge, re) {
                let before: Vec<String> =
                    g.events[k.saturating_sub(3)..k].iter().map(show).collect();
                return Some(format!(
                    "boot {i} event {k}:\n  C++  {}\n  Rust {}\n  after:\n    {}",
                    show(ge),
                    show(re),
                    before.join("\n    ")
                ));
            }
        }
        if g.events.len() != r.events.len() {
            let n = g.events.len().min(r.events.len());
            let extra = |es: &[Event]| es.get(n).map_or("(none)".to_string(), show);
            return Some(format!(
                "boot {i}: C++ {} events, Rust {}; first extra: C++ {}, Rust {}",
                g.events.len(),
                r.events.len(),
                extra(&g.events),
                extra(&r.events)
            ));
        }
        if g.end != r.end {
            return Some(format!("boot {i} end: C++ {:?}, Rust {:?}", g.end, r.end));
        }
        if g.eeprom != r.eeprom {
            let (gr, rr) = (eeprom_rows(&g.eeprom), eeprom_rows(&r.eeprom));
            let first = gr.iter().zip(&rr).find(|(a, b)| a != b).map_or_else(
                || format!("rows C++ {}, Rust {}", gr.len(), rr.len()),
                |(a, b)| format!("\n  C++  {a}\n  Rust {b}"),
            );
            return Some(format!("boot {i} EEPROM differs: {first}"));
        }
        if g.noinit != r.noinit {
            let at = g
                .noinit
                .iter()
                .zip(&r.noinit)
                .position(|(a, b)| a != b)
                .unwrap_or(0);
            return Some(format!(
                "boot {i} no-init cells differ at byte {at}:\n  C++  {}\n  Rust {}",
                escape_hex(&g.noinit),
                escape_hex(&r.noinit)
            ));
        }
    }
    if golden.boots.len() != rust.boots.len() {
        return Some(format!(
            "C++ {} boots, Rust {}",
            golden.boots.len(),
            rust.boots.len()
        ));
    }
    None
}

fn escape_hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// The record a bench writes at the end of a boot.
pub fn boot_end(
    index: u32,
    reset: Reset,
    events: Vec<Event>,
    end: &str,
    eeprom: &[u8],
    noinit: &[u8; NOINIT_SIZE],
) -> BootRecord {
    BootRecord {
        index,
        reset,
        events,
        end: end.to_string(),
        eeprom: eeprom.to_vec(),
        noinit: noinit.to_vec(),
    }
}

#[test]
fn golden_escape_and_parse_round_trip() {
    let bytes = b"gvers 2\r\n\"\\\x01\xff\tz".to_vec();
    assert_eq!(unescape(&escape(&bytes)), bytes);
    let text = "case system: x\r\nboot 0 power-on\r\nt 3011 dbg \"a\\r\\n\"\r\n\
                end reboot software\r\ne 0020 ffffffff1111d007ffffffffffffffffffffffffffffffffffffffffffffffff\r\n\
                noinit a5a5\r\nboot 1 software\r\nend case\r\n";
    let t = parse(text);
    assert_eq!(t.case, "system: x");
    assert_eq!(t.boots.len(), 2);
    assert_eq!(t.boots[0].events[0].bytes, b"a\r\n".to_vec());
    assert_eq!(t.boots[0].events[0].ms, 3011);
    assert_eq!(t.boots[0].events[0].dir, Dir::Dbg);
    assert_eq!(t.boots[0].end, "reboot software");
    assert_eq!(&t.boots[0].eeprom[0x24..0x28], &[0x11, 0x11, 0xd0, 0x07]);
    assert_eq!(t.boots[0].eeprom[0x23], 0xFF);
    assert_eq!(t.boots[0].noinit, vec![0xa5, 0xa5]);
    assert_eq!(t.boots[1].reset, Reset::Software);
    assert_eq!(diff(&t, &t), None);
    let mut other = t.clone();
    other.boots[0].events[0].ms = 3012;
    assert!(diff(&t, &other).unwrap().contains("event 0"));
}

#[test]
fn golden_render_writes_every_golden_as_the_recorder() {
    let dir = format!("{}/tests/golden", env!("CARGO_MANIFEST_DIR"));
    let mut n = 0;
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        let text = std::fs::read_to_string(&path)
            .unwrap()
            .replace("\r\n", "\n");
        assert_eq!(render(&parse(&text)), text, "{}", path.display());
        n += 1;
    }
    assert_eq!(n, 27);
}

#[test]
fn golden_the_goldens_marked_as_written_by_the_rust_bench_are_the_named_ones() {
    let dir = format!("{}/tests/golden", env!("CARGO_MANIFEST_DIR"));
    let mut rust: Vec<String> = Vec::new();
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if parse(&std::fs::read_to_string(&path).unwrap()).rust {
            rust.push(path.file_stem().unwrap().to_str().unwrap().to_string());
        }
    }
    rust.sort();
    assert_eq!(rust, RUST_RECORDED);
}

#[test]
fn golden_store_keeps_a_golden_of_the_cpp_recorder_unless_forced() {
    let path = std::env::temp_dir().join(format!("vdm-golden-{}.txt", std::process::id()));
    let path = path.to_str().unwrap();
    let cpp = "case system: x\nboot 0 power-on\nend case\nnoinit a5\n";
    std::fs::write(path, cpp).unwrap();
    let mut t = parse(cpp);
    t.boots[0].end = "reboot software".to_string();
    let kept = std::panic::catch_unwind(|| store_at(path, &t, false));
    assert!(kept.is_err());
    assert_eq!(std::fs::read_to_string(path).unwrap(), cpp);
    store_at(path, &t, true);
    let written = std::fs::read_to_string(path).unwrap();
    assert!(written.starts_with("case system: x\nrecorder rust\nboot 0"));
    assert!(parse(&written).rust);
    // a golden the Rust bench wrote is written again without force
    t.boots[0].end = "case".to_string();
    store_at(path, &t, false);
    assert_eq!(
        parse(&std::fs::read_to_string(path).unwrap()).boots[0].end,
        "case"
    );
    // a new golden
    std::fs::remove_file(path).unwrap();
    store_at(path, &t, false);
    assert!(parse(&std::fs::read_to_string(path).unwrap()).rust);
    std::fs::remove_file(path).unwrap();
}

fn tx(bytes: &[u8]) -> Event {
    Event {
        ms: 7,
        dir: Dir::Tx,
        bytes: bytes.to_vec(),
    }
}

#[test]
fn golden_a_gvlvy_reply_is_compared_on_the_values_of_the_golden_reply() {
    let v25 = "gvlvy 0 1 30 30 20 3600 3600 0 0 0 0 0 0 0 0 0 0 0 0 32 0 50 30 0 0";
    let g = tx(format!("{v25}\r\n").as_bytes());
    assert_eq!(gvlvy_values(&g).unwrap().len(), 25);
    // values 26 and 27 of protocol 3
    let v27 = tx(format!("{v25} 54 61\r\n").as_bytes());
    assert!(matches(&g, &v27));
    assert!(matches(&g, &g));
    // a golden of 27 values compares them all
    assert!(matches(&v27, &v27));
    assert!(!matches(&v27, &tx(format!("{v25} 54 62\r\n").as_bytes())));
    assert!(!matches(&v27, &g));
    // value 25 differs, the time differs
    assert!(!matches(&g, &tx(format!("{v25}1 54 61\r\n").as_bytes())));
    assert!(!matches(
        &g,
        &Event {
            ms: 8,
            ..v27.clone()
        }
    ));
    // only a gvlvy reply of one line on USART1
    for other in [
        tx(format!("gvlvx{} 54\r\n", &v25[5..]).as_bytes()),
        tx(format!("{v25} 54\r\n{v25} 54\r\n").as_bytes()),
        tx(format!("{v25} 54").as_bytes()),
        Event {
            dir: Dir::Dbg,
            ..tx(format!("{v25} 54\r\n").as_bytes())
        },
    ] {
        assert_eq!(gvlvy_values(&other), None);
        assert!(!matches(&g, &other));
        assert!(matches(&other, &other));
    }
    let mut t = Transcript::default();
    t.boots.push(boot_end(
        0,
        Reset::PowerOn,
        std::vec![g],
        "case",
        &[],
        &[0; NOINIT_SIZE],
    ));
    let mut longer = t.clone();
    longer.boots[0].events[0] = v27;
    assert_eq!(diff(&t, &longer), None);
    assert!(diff(&longer, &t).unwrap().contains("event 0"));
}
