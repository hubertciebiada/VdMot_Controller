//! The STM32 on the other end of Serial2 for the stm_link and app tests (C++
//! `test/native/glue/support/fake_stm.cpp`): answers the ESP-STM text protocol with the golden
//! replies of the core's test support (`test_support::stm_golden`, the valve of a request
//! substituted), watches NRST (GPIO 15: HIGH holds the STM in reset, the release boots it) and,
//! for flash runs, hands every byte and the NRST line to the AN3155 simulator of the core's test
//! support (`test_support::sim_stm`). Both come through the core feature `test-support`.
//!
//! Defaults: protocol 2, version "2.0.0-revamped_C2", no 1-Wire sensors, EEPROM idle (eepst 1),
//! replies 3 ms after the request line. Commands above the protocol stay unanswered, like a v1
//! STM that ignores gproto. gstat/gstax report an uptime counted from the last NRST release
//! (from 100 s at the attach); slcfg/sfspo/stlnt store what glcfg/gtlnt read back (defaults
//! 60 min, 50 %, 604800 s).
//!
//! Port form: the FakeGpio has one write hook, so the fake keeps the NRST writes with their
//! times itself ([`FakeStm::nrst_writes`]; C++ tests chained their own hook).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};

use vdm_esp_core::common::VALVE_COUNT;
use vdm_esp_core::stm_codec::{cmd_from_name, cmd_min_protocol, Cmd};
use vdm_esp_core::stm_flasher::FlashTransport;
use vdm_esp_core::test_support::sim_stm::SimStm;
use vdm_esp_core::test_support::stm_golden::STM_GOLDEN;

use super::uart::{SerialPeer, UartRx};
use super::{lock, FakeClock, FakeGpio, FakeUart};
use crate::board::STM_RESET_PIN;

/// Reply of one command: fn(request line without CR LF, now, the RX side) -> reply line ("" =
/// none; the function may queue bytes itself).
pub(crate) type Answer = Box<dyn FnMut(&str, u64, &mut UartRx) -> String + Send>;

/// One request line the STM received.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Request {
    /// without CR LF
    pub(crate) line: String,
    pub(crate) at_ms: u64,
}

struct StmState {
    clock: FakeClock,
    protocol: u8,
    version: String,
    silent: bool,
    too_old: bool,
    eep: u8,
    /// uptime 0 of the STM (NRST release)
    boot_at_ms: u64,
    /// uptime at `boot_at_ms`
    uptime_base_s: u64,
    lease_timeout: u32,
    failsafe: [u32; VALVE_COUNT as usize],
    learn_time: u32,
    answers: BTreeMap<String, Answer>,
    rx_line: Vec<u8>,
    requests: Vec<Request>,
    held: bool,
    resets: u32,
    boot_until_ms: u64,
    use_sim: bool,
    sim: Option<SimStm>,
    reply_delay_ms: u64,
    boot_ms: u64,
    nrst: Vec<(u64, bool)>,
}

/// The fake STM; clones share it (one is the peer of the UART, one the NRST hook).
#[derive(Clone)]
pub(crate) struct FakeStm(Arc<Mutex<StmState>>);

/// The UART side of the fake.
struct Peer(FakeStm);

fn split(line: &str) -> Vec<String> {
    line.split(' ')
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect()
}

/// First golden line of `cmd` that is not an error form.
fn golden(cmd: &str) -> String {
    STM_GOLDEN
        .iter()
        .find(|g| {
            g.strip_prefix(cmd)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with(' '))
                && !g.contains(" err")
        })
        .map_or_else(String::new, |g| g.to_string())
}

/// The golden line with its first field (the valve) replaced.
fn with_valve(line: &str, valve: &str) -> String {
    let Some(a) = line.find(' ') else {
        return line.to_string();
    };
    match line[a + 1..].find(' ') {
        Some(b) => format!("{}{}{}", &line[..=a], valve, &line[a + 1 + b..]),
        None => format!("{}{}", &line[..=a], valve),
    }
}

fn stoul(s: &str) -> u32 {
    let digits: String = s.chars().take_while(char::is_ascii_digit).collect();
    digits
        .parse()
        .unwrap_or_else(|_| panic!("not a number: {s:?}"))
}

impl FakeStm {
    /// Attaches to `uart` and the NRST pin of `gpio` (the GPIO write hook is the fake's).
    pub(crate) fn attach(uart: &FakeUart, gpio: &FakeGpio, clock: &FakeClock) -> FakeStm {
        let stm = FakeStm(Arc::new(Mutex::new(StmState {
            clock: clock.clone(),
            protocol: 2,
            version: String::new(),
            silent: false,
            too_old: false,
            eep: 1,
            boot_at_ms: clock.ms(),
            uptime_base_s: 100,
            lease_timeout: 60,
            failsafe: [50; VALVE_COUNT as usize],
            learn_time: 604_800,
            answers: BTreeMap::new(),
            rx_line: Vec::new(),
            requests: Vec::new(),
            held: false,
            resets: 0,
            boot_until_ms: 0,
            use_sim: false,
            sim: None,
            reply_delay_ms: 3,
            boot_ms: 100,
            nrst: Vec::new(),
        })));
        stm.protocol(2);
        uart.attach(Box::new(Peer(stm.clone())));
        let hook = stm.clone();
        gpio.on_write(move |pin, level| {
            if pin == STM_RESET_PIN {
                hook.on_nrst(level);
            }
        });
        stm
    }

    fn s(&self) -> MutexGuard<'_, StmState> {
        lock(&self.0)
    }

    // ---- controls

    /// 1, 2 or 3; also picks the default version.
    pub(crate) fn protocol(&self, p: u8) {
        let mut s = self.s();
        s.protocol = p;
        s.version = match p {
            0 | 1 => "1.4.9_Dev_C2 1712345678 ",
            2 => "2.0.0-revamped_C2 1712345678 ",
            _ => "2.1.0-revamped_C2 1712345678 ",
        }
        .to_string();
    }
    /// gvers reply without the command, e.g. "1.4.9_C2 17".
    pub(crate) fn version(&self, text: &str) {
        self.s().version = text.to_string();
    }
    /// No replies at all.
    pub(crate) fn silent(&self, on: bool) {
        self.s().silent = on;
    }
    /// Firmware older than the minimum: answers only "gvers 1.3.5_C2", stgtp and gtgtp.
    pub(crate) fn too_old(&self, on: bool) {
        self.s().too_old = on;
    }
    /// eepst reply (1 idle, 0 a write is pending).
    pub(crate) fn eep_state(&self, n: u8) {
        self.s().eep = n;
    }
    /// Reply of one command.
    pub(crate) fn answer(
        &self,
        cmd: &str,
        f: impl FnMut(&str, u64, &mut UartRx) -> String + Send + 'static,
    ) {
        self.s().answers.insert(cmd.to_string(), Box::new(f));
    }
    /// After an NRST release nothing is answered before this.
    pub(crate) fn set_boot_ms(&self, ms: u64) {
        self.s().boot_ms = ms;
    }
    /// Delay of the replies after the request line.
    pub(crate) fn set_reply_delay_ms(&self, ms: u64) {
        self.s().reply_delay_ms = ms;
    }
    /// The STM restarts on its own now (a brown-out, its watchdog): silent for the boot time,
    /// its uptime from 0 again (Rust addition).
    pub(crate) fn reboot(&self) {
        let mut s = self.s();
        let now = s.clock.ms();
        s.boot_until_ms = now + s.boot_ms;
        s.boot_at_ms = now;
        s.uptime_base_s = 0;
        s.rx_line.clear();
    }
    /// Flash runs: every byte and the NRST line go to the AN3155 simulator (created on first
    /// use: it fills a 512 KiB flash model).
    pub(crate) fn use_simulator(&self, on: bool) {
        let mut s = self.s();
        if on && s.sim.is_none() {
            let now = s.clock.ms() as u32;
            s.sim = Some(SimStm::new(now));
        }
        s.use_sim = on;
    }
    /// Works on the simulator (created when missing).
    pub(crate) fn with_sim<R>(&self, f: impl FnOnce(&mut SimStm) -> R) -> R {
        let mut s = self.s();
        let now = s.clock.ms() as u32;
        f(s.sim.get_or_insert_with(|| SimStm::new(now)))
    }

    // ---- observation

    /// Every request line in order.
    pub(crate) fn requests(&self) -> Vec<Request> {
        self.s().requests.clone()
    }
    /// Request lines of one command, in order.
    pub(crate) fn requests_of(&self, cmd: &str) -> Vec<String> {
        self.s()
            .requests
            .iter()
            .filter(|r| split(&r.line).first().is_some_and(|t| t == cmd))
            .map(|r| r.line.clone())
            .collect()
    }
    /// NRST released after being held.
    pub(crate) fn resets(&self) -> u32 {
        self.s().resets
    }
    /// NRST holds the STM in reset.
    pub(crate) fn held(&self) -> bool {
        self.s().held
    }
    /// Every write to NRST: (time, level HIGH).
    pub(crate) fn nrst_writes(&self) -> Vec<(u64, bool)> {
        self.s().nrst.clone()
    }

    fn on_nrst(&self, high: bool) {
        let mut s = self.s();
        let now = s.clock.ms();
        s.nrst.push((now, high));
        if let Some(sim) = s.sim.as_mut() {
            sim.now = now as u32;
            sim.set_reset(high);
        }
        if high {
            s.held = true;
            s.rx_line.clear();
            return;
        }
        if s.held {
            s.held = false;
            s.resets += 1;
            s.boot_until_ms = now + s.boot_ms;
            s.boot_at_ms = now;
            s.uptime_base_s = 0;
        }
    }
}

impl StmState {
    fn on_line(&mut self, line: &str, now: u64, rx: &mut UartRx) {
        self.requests.push(Request {
            line: line.to_string(),
            at_ms: now,
        });
        if self.held || self.silent || now < self.boot_until_ms {
            return;
        }
        let t = split(line);
        let Some((cmd, args)) = t.split_first() else {
            return;
        };
        let out = self.reply(cmd, args, line, now, rx);
        if out.is_empty() {
            return;
        }
        rx.inject_at(now + self.reply_delay_ms, format!("{out}\r\n").as_bytes());
    }

    fn reply(
        &mut self,
        cmd: &str,
        args: &[String],
        line: &str,
        now: u64,
        rx: &mut UartRx,
    ) -> String {
        if let Some(f) = self.answers.get_mut(cmd) {
            return f(line, now, rx);
        }
        let valve = args.first().map_or("0", String::as_str).to_string();
        if self.too_old {
            return match cmd {
                "gvers" => "gvers 1.3.5_C2".to_string(),
                "stgtp" => "stgtp".to_string(),
                "gtgtp" => format!("gtgtp {valve} 50 "),
                _ => String::new(),
            };
        }
        let c = cmd_from_name(cmd.as_bytes());
        if c == Cmd::None || cmd_min_protocol(c) > self.protocol {
            return String::new();
        }
        match cmd {
            "gvers" => return format!("gvers {}", self.version),
            "gproto" => return format!("gproto {}", self.protocol),
            "eepst" => return format!("eepst {} ", self.eep),
            "gstat" | "gstax" => {
                let up = self.uptime_base_s + (now - self.boot_at_ms) / 1000;
                return with_valve(&golden(cmd), &up.to_string());
            }
            "slcfg" => {
                if let Some(a) = args.first() {
                    self.lease_timeout = stoul(a);
                }
                return "slcfg ok".to_string();
            }
            "glcfg" => {
                let mut out = format!("glcfg {}", self.lease_timeout);
                for p in self.failsafe {
                    out.push_str(&format!(" {p}"));
                }
                return out;
            }
            "stlnt" => {
                if let Some(a) = args.first() {
                    self.learn_time = stoul(a);
                }
                return "stlnt".to_string();
            }
            "gtlnt" => return format!("gtlnt {}", self.learn_time),
            "gonec" | "gowvc" => return format!("{cmd} 0 "),
            "gvlon" => {
                let zero = "00-00-00-00-00-00-00-00";
                if valve == "255" {
                    let ids = vec![zero; 2 * VALVE_COUNT as usize].join(",");
                    return format!("gvlon 12 {ids}");
                }
                return format!("gvlon {valve} {zero} {zero} ");
            }
            _ => {}
        }
        if cmd == "sfspo" && args.len() == 2 {
            let pct = stoul(&args[1]);
            for (v, f) in self.failsafe.iter_mut().enumerate() {
                if valve == "255" || valve == v.to_string() {
                    *f = pct;
                }
            }
        }
        match cmd {
            "svmov" | "sfspo" | "sstop" => format!("{cmd} {valve} ok"),
            "stvls" => format!("stvls {valve}"),
            "gvlvd" | "gvlvx" | "gvlvy" | "gtgtp" | "gprof" => with_valve(&golden(cmd), &valve),
            // the bare acknowledgements (stons, staln, reset, ...)
            _ => {
                let g = golden(cmd);
                if g.is_empty() {
                    cmd.to_string()
                } else {
                    g
                }
            }
        }
    }
}

impl SerialPeer for Peer {
    fn on_configure(&mut self, baud: u32, even_parity: bool) {
        if let Some(sim) = self.0.s().sim.as_mut() {
            sim.configure(baud, even_parity);
        }
    }
    fn on_tx(&mut self, data: &[u8], now_ms: u64, rx: &mut UartRx) {
        let mut s = self.0.s();
        if s.use_sim {
            if let Some(sim) = s.sim.as_mut() {
                sim.now = now_ms as u32;
                sim.write(data);
            }
            return;
        }
        for &c in data {
            match c {
                b'\n' => {
                    let line =
                        String::from_utf8_lossy(&std::mem::take(&mut s.rx_line)).into_owned();
                    s.on_line(&line, now_ms, rx);
                }
                b'\r' => {}
                _ => s.rx_line.push(c),
            }
        }
    }
    fn poll(&mut self, now_ms: u64, rx: &mut UartRx) {
        let mut s = self.0.s();
        if !s.use_sim {
            return;
        }
        let Some(sim) = s.sim.as_mut() else {
            return;
        };
        sim.now = now_ms as u32;
        let mut buf = [0u8; 256];
        loop {
            let n = sim.read(&mut buf);
            if n == 0 {
                break;
            }
            rx.inject_at(now_ms, &buf[..n]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn golden_lines_and_the_valve_substitution() {
        assert_eq!(
            golden("gvlvd"),
            "gvlvd 3 42 18 1 215 -500 57 3120 3350 230 0 "
        );
        assert_eq!(golden("stgtp"), "stgtp");
        assert_eq!(golden("svmov"), ""); // only error forms
        assert_eq!(golden("gvl"), ""); // a prefix is not a command
        assert_eq!(with_valve("gtgtp 3 50 ", "7"), "gtgtp 7 50 ");
        assert_eq!(with_valve("stvls 3", "9"), "stvls 9");
        assert_eq!(with_valve("stgtp", "9"), "stgtp");
        assert_eq!(split("  a b  c "), vec!["a", "b", "c"]);
        assert_eq!(stoul("42x"), 42);
    }
}
