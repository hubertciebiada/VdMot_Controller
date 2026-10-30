//! A scripted STM at the text-line level for the StmSession tests (port of
//! test/native/support/line_stm.h): answers request lines (without CR LF) like the firmware of a
//! protocol, keeps the targets, the assembly holds, the lease settings and the learn time it was
//! given, and reports an uptime counted from its last reset.
//!
//! A scripted answer ([`LineStm::answers`]) gets the line and an [`AnswerCtx`]: the C++ lambdas
//! capture the rig's clock and the STM's targets by reference, which a boxed Rust closure stored
//! in the STM itself cannot.

use std::boxed::Box;
use std::collections::BTreeMap;
use std::format;
use std::string::{String, ToString};
use std::vec::Vec;

/// What a scripted answer may read besides the line.
#[derive(Clone, Copy, Debug)]
pub struct AnswerCtx {
    /// the time of the request (the rig's `now`)
    pub now: u64,
    /// the targets the STM holds
    pub target: [u8; 12],
}

/// Reply of one command: fn(line, ctx) -> reply ("" = none).
pub type Answer = Box<dyn FnMut(&str, &AnswerCtx) -> String>;

/// `std::stoi`/`std::stoul` of the tests: optional sign and leading digits (the requests carry
/// plain numbers); no digit at all is a test error.
fn stoi(s: &str) -> i64 {
    let (neg, digits) = match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s.strip_prefix('+').unwrap_or(s)),
    };
    let n = digits.bytes().take_while(u8::is_ascii_digit).count();
    let v: i64 = digits[..n].parse().expect("a number");
    if neg {
        -v
    } else {
        v
    }
}

pub struct LineStm {
    // ---- behaviour
    /// 1 (legacy: no gproto), 2 or 3
    pub protocol: u8,
    /// gvers reply without the command; "" = per protocol
    pub version: String,
    /// no replies at all
    pub silent: bool,
    /// answers only gvers 1.3.5_C2, stgtp and gtgtp
    pub too_old: bool,
    /// eepst reply (1 idle)
    pub eep: u8,
    /// uptime at the last reset
    pub uptime_base_s: u32,
    /// gstat/gstax reset counter
    pub resets: u32,
    /// gstax/slhbt lease state
    pub lease: u8,
    pub lease_timeout: u32,
    pub failsafe: [u32; 12],
    pub learn_time: u32,
    pub target: [u8; 12],
    pub assembly: [bool; 12],
    /// Rust 2.2.8: the service hold after a svmov and the keep of `skeep`
    pub svc_hold: [bool; 12],
    pub keep: [bool; 12],
    /// the last move of each valve: dir, requested, counted, stop (the gvlvy record)
    pub last_move: [(u8, u32, u32, u8); 12],
    pub answers: BTreeMap<String, Answer>,

    // ---- observation
    pub lines: Vec<String>,

    boot_at_ms: u64,
}

impl Default for LineStm {
    fn default() -> Self {
        Self {
            protocol: 3,
            version: String::new(),
            silent: false,
            too_old: false,
            eep: 1,
            uptime_base_s: 100,
            resets: 3,
            lease: 1,
            lease_timeout: 60,
            failsafe: [50; 12],
            learn_time: 604_800,
            target: [50; 12],
            assembly: [false; 12],
            svc_hold: [false; 12],
            keep: [false; 12],
            last_move: [LineStm::GOLDEN_MOVE; 12],
            answers: BTreeMap::new(),
            lines: Vec::new(),
            boot_at_ms: 0,
        }
    }
}

impl LineStm {
    /// The last move of the gvlvx/gvlvy golden: close, 3000 requested, 1450 counted, early
    /// end stop.
    pub const GOLDEN_MOVE: (u8, u32, u32, u8) = (1, 3000, 1450, 3);

    pub fn reset(&mut self, now_ms: u64) {
        self.boot_at_ms = now_ms;
        self.uptime_base_s = 0;
        self.target = [50; 12];
        self.assembly = [false; 12];
        self.svc_hold = [false; 12];
        self.keep = [false; 12];
        self.last_move = [Self::GOLDEN_MOVE; 12];
    }

    /// stgtp, staop, staln of a valve (255: all) end its service hold and keep.
    fn end_hold(&mut self, v: i64) {
        for k in 0..12 {
            if v == 255 || v == k {
                self.svc_hold[k as usize] = false;
                self.keep[k as usize] = false;
            }
        }
    }

    /// Scripts the reply of `cmd` (C++ `answers[cmd] = ...`).
    pub fn set_answer(&mut self, cmd: &str, f: impl FnMut(&str, &AnswerCtx) -> String + 'static) {
        self.answers.insert(cmd.to_string(), Box::new(f));
    }

    /// The lines of `cmd`: the command alone or followed by a space.
    pub fn lines_of(&self, cmd: &str) -> Vec<String> {
        self.lines
            .iter()
            .filter(|l| {
                l.strip_prefix(cmd)
                    .is_some_and(|rest| rest.is_empty() || rest.starts_with(' '))
            })
            .cloned()
            .collect()
    }

    pub fn answer(&mut self, line: &str, now_ms: u64) -> String {
        self.lines.push(line.to_string());
        if self.silent {
            return String::new();
        }
        let t: Vec<&str> = line.split(' ').filter(|s| !s.is_empty()).collect();
        let Some(&cmd) = t.first() else {
            return String::new();
        };
        let ctx = AnswerCtx {
            now: now_ms,
            target: self.target,
        };
        if let Some(f) = self.answers.get_mut(cmd) {
            return f(line, &ctx);
        }
        let v = if t.len() > 1 { stoi(t[1]) } else { 0 };
        // the valve index when `one`
        let one = (0..12).contains(&v);
        let vi = v.clamp(0, 11) as usize;
        if self.too_old {
            if cmd == "gvers" {
                return "gvers 1.3.5_C2".to_string();
            }
            if cmd == "stgtp" && one && t.len() > 2 {
                self.target[vi] = stoi(t[2]) as u8;
            }
            if cmd == "stgtp" {
                return "stgtp".to_string();
            }
            if cmd == "gtgtp" && one {
                return format!("gtgtp {} {} ", t[1], self.target[vi]);
            }
            return String::new();
        }
        if cmd == "gproto" {
            return if self.protocol >= 2 {
                format!("gproto {}", self.protocol)
            } else {
                String::new()
            };
        }
        if cmd == "gvers" {
            if !self.version.is_empty() {
                return format!("gvers {}", self.version);
            }
            if self.protocol <= 1 {
                return "gvers 1.4.9_Dev_C2 1712345678 ".to_string();
            }
            return if self.protocol == 2 {
                "gvers 2.0.0-revamped_C2 1712345678 ".to_string()
            } else {
                "gvers 2.1.0-revamped_C2 1712345678 ".to_string()
            };
        }
        let up = u64::from(self.uptime_base_s) + now_ms.wrapping_sub(self.boot_at_ms) / 1000;
        if cmd == "gstat" && self.protocol >= 2 {
            return format!("gstat {up} {} 2 0 0 1", self.resets);
        }
        if cmd == "gstax" && self.protocol >= 3 {
            return format!(
                "gstax {up} {} 2 0 0 1 {} 3540 1 {} 0 0 0 0 0 0 0 0 0 0 0 0 0",
                self.resets, self.lease, self.lease_timeout
            );
        }
        if cmd == "eepst" {
            return format!("eepst {} ", self.eep);
        }
        if cmd == "ghwin" {
            return "ghwin 1073 ".to_string();
        }
        if cmd == "gmotc" {
            return "gmotc 17 17 50 3000 0 ".to_string();
        }
        if cmd == "gtlnm" {
            return "gtlnm 2000 ".to_string();
        }
        if cmd == "gcalx" && self.protocol >= 2 {
            return "gcalx 1 10 40".to_string();
        }
        if cmd == "gonec" || cmd == "gowvc" {
            return format!("{cmd} 0 ");
        }
        if cmd == "gvlst" {
            return "gvlst 12 1,1,1,1,1,1,1,1,1,1,1,1, ".to_string();
        }
        if cmd == "gvlon" {
            let z = "00-00-00-00-00-00-00-00";
            if t.len() > 1 && t[1] == "255" {
                let ids: Vec<&str> = (0..24).map(|_| z).collect();
                return format!("gvlon 12 {}", ids.join(","));
            }
            return format!("gvlon {} {z} {z} ", t[1]);
        }
        if cmd == "gtgtp" && one {
            return format!("gtgtp {} {} ", t[1], self.target[vi]);
        }
        if cmd == "gvlvd" && one {
            return format!("gvlvd {} 42 18 1 215 -500 57 3120 3350 230 0 ", t[1]);
        }
        if cmd == "gvlvx" && self.protocol >= 2 && one {
            return self.valve_ex(vi, false);
        }
        if cmd == "gvlvy" && self.protocol >= 3 && one {
            return self.valve_ex(vi, true);
        }
        if cmd == "stgtp" && one && t.len() > 2 {
            self.target[vi] = stoi(t[2]) as u8;
            self.assembly[vi] = false;
            self.end_hold(v);
            return "stgtp".to_string();
        }
        if cmd == "staop" {
            for k in 0..12 {
                if v == 255 || v == k {
                    self.target[k as usize] = 100;
                    self.assembly[k as usize] = true;
                }
            }
            self.end_hold(v);
            return "staop".to_string();
        }
        if cmd == "staln" {
            self.end_hold(v);
            return "staln".to_string();
        }
        if self.protocol >= 3 {
            if cmd == "slhbt" {
                return format!("slhbt {} 3540", self.lease);
            }
            if cmd == "slcfg" {
                self.lease_timeout = stoi(t[1]) as u32;
                return "slcfg ok".to_string();
            }
            if cmd == "sfspo" {
                for k in 0..12 {
                    if v == 255 || v == k {
                        self.failsafe[k as usize] = stoi(t[2]) as u32;
                    }
                }
                return format!("sfspo {} ok", t[1]);
            }
            if cmd == "glcfg" {
                let mut out = format!("glcfg {}", self.lease_timeout);
                for p in self.failsafe {
                    out += &format!(" {p}");
                }
                return out;
            }
            if cmd == "gtlnt" {
                return format!("gtlnt {}", self.learn_time);
            }
            if cmd == "sstop" {
                return format!("sstop {} ok", t[1]);
            }
            if cmd == "ssafe" {
                return "ssafe ok".to_string();
            }
            if cmd == "sbrak" && t.len() > 3 && (one || v == 255) {
                return format!("sbrak {} ok", t[1]);
            }
            if cmd == "skeep" && t.len() > 2 && (one || v == 255) {
                let on = stoi(t[2]) != 0;
                for k in 0..12 {
                    if v == 255 || v == k {
                        self.keep[k as usize] = on;
                    }
                }
                return format!("skeep {} ok", t[1]);
            }
        }
        if cmd == "stlnt" {
            self.learn_time = stoi(t[1]) as u32;
            return "stlnt".to_string();
        }
        if cmd == "svmov" && self.protocol >= 2 {
            // The move ends at once: a close meets the end stop after 97 pulses, an opening
            // move counts its pulses; the valve stays held (the keep stays).
            if one && t.len() > 3 {
                let (dir, counts) = (stoi(t[2]) as u8, stoi(t[3]) as u32);
                self.last_move[vi] = if dir == 1 {
                    (1, counts, counts.min(97), 2)
                } else {
                    (0, counts, counts, 1)
                };
                self.svc_hold[vi] = true;
            }
            return format!("svmov {} ok", t[1]);
        }
        if matches!(
            cmd,
            "stons" | "masns" | "staln" | "stdet" | "smotc" | "stlnm"
        ) {
            return cmd.to_string();
        }
        String::new()
    }

    fn valve_ex(&self, v: usize, v3: bool) -> String {
        let (dir, req, cnt, stop) = self.last_move[v];
        let mut s = format!(
            "{}{v} 1 40 {} 21 3120 3350 -230 0 57 0 0 0 {dir} {req} {cnt} {stop} 412 8123",
            if v3 { "gvlvy " } else { "gvlvx " },
            self.target[v]
        );
        if v3 {
            // disjoint bits: `+` is the `|`
            let hold = u16::from(self.assembly[v]) * 256
                + u16::from(self.svc_hold[v]) * 512
                + u16::from(self.keep[v]) * 1024;
            s += &format!(" {hold} 0 50 {} 0 0", self.target[v]);
        }
        s
    }
}
