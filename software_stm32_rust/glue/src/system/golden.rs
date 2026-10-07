//! The goldens of the C++ glue_system scenarios (design §7.4): `glue/tests/golden/<slug>.txt`,
//! written by `tools/rust/stm/golden` (recorder.cpp holds the format), and the transcript the
//! Rust bench records in the same shape. A system test compares its whole transcript with the
//! C++ one: every byte both UARTs carried, at the same millisecond, the EEPROM and the no-init
//! cells at the end of every boot.

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
    pub boots: Vec<BootRecord>,
}

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

/// The golden of a case by its slug (`tools/rust/stm/golden/recorder.cpp` slug()).
pub fn load(slug: &str) -> Transcript {
    let path = format!("{}/tests/golden/{slug}.txt", env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    parse(&text)
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
            if ge != re {
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
