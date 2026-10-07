//! Edge cases of `stm_link` (C++ `test_stm_link__mut.cpp`): a whole flash through the UART
//! transport, the bounded input discard, the command and UART read limits of one pass, the
//! one-second cadence, the interval between policy resets.
// host test code: the stack rule of the glue (design 2.4) is for the device
#![allow(clippy::large_stack_frames)]

use std::sync::{Arc, Mutex};

use super::rig::{command, flash, image, Rig};
use super::*;
use crate::port::Fs;
use crate::testkit::lock as tlock;
use crate::testkit::uart::{SerialPeer, UartRx};
use vdm_esp_core::event_log::EventCode;
use vdm_esp_core::link_policy::LinkState;
use vdm_esp_core::stm_types::StmCommandType;

/// The other end of Serial2 once the flasher opened it at 8E1: `per_poll` bytes arrive on every
/// look at the RX side, for `burst` looks (`None`: endlessly).
#[derive(Default)]
struct FloodState {
    per_poll: usize,
    burst: Option<u32>,
    armed: bool,
    polls: u32,
}

struct Flood(Arc<Mutex<FloodState>>);

impl SerialPeer for Flood {
    fn on_configure(&mut self, _baud: u32, even_parity: bool) {
        if even_parity {
            tlock(&self.0).armed = true;
        }
    }
    fn on_tx(&mut self, _data: &[u8], _now_ms: u64, _rx: &mut UartRx) {}
    fn poll(&mut self, now_ms: u64, rx: &mut UartRx) {
        let mut s = tlock(&self.0);
        if !s.armed {
            return;
        }
        s.polls += 1;
        match s.burst {
            Some(0) => return,
            Some(n) => s.burst = Some(n - 1),
            None => {}
        }
        rx.inject_at(now_ms, &vec![b'z'; s.per_poll]);
    }
}

/// Starts a blank flash (board C2, no STM answering) at 1000 ms with `flood` on the UART;
/// returns the looks at the RX side and the bytes left in it after the loop pass that opened
/// the UART for the bootloader (the flasher discards the input there).
fn discard_against(per_poll: usize, burst: Option<u32>) -> (u32, usize) {
    let rig = Rig::new();
    rig.dev.fs.put("/stm/c2.bin", &image("C2"));
    assert!(rig.dev.fs.mount());
    let mut link = rig.link();
    link.begin();
    let flood = Arc::new(Mutex::new(FloodState {
        per_poll,
        burst,
        ..FloodState::default()
    }));
    rig.dev.uart.attach(Box::new(Flood(flood.clone())));
    let seen = Arc::new(Mutex::new(None::<(u32, usize)>));
    {
        let (clock, host, uart, flood, seen) = (
            rig.dev.clock.clone(),
            rig.host.clone(),
            rig.dev.uart.clone(),
            flood.clone(),
            seen.clone(),
        );
        rig.dev.clock.on_sleep(move |_| {
            if clock.ms() == 1000 {
                host.s().to_receive.push_back(flash("c2", true, "C2"));
            }
            let f = tlock(&flood);
            let mut s = tlock(&seen);
            if f.armed && s.is_none() {
                *s = Some((f.polls, uart.ring_len()));
            }
        });
    }
    rig.run_task(&mut link, 1100);
    assert!(tlock(&flood).armed);
    let s = *tlock(&seen);
    s.expect("the flasher opened the UART at 8E1")
}

#[test]
fn task_a_blank_flash_runs_through_the_uart_to_the_new_firmware() {
    let rig = Rig::new();
    let img = image("C2");
    rig.dev.fs.put("/stm/c2.bin", &img);
    assert!(rig.dev.fs.mount());
    let stm = rig.stm();
    let mut link = rig.link();
    link.begin();
    let link_at_start = Arc::new(Mutex::new(None::<LinkState>));
    {
        let (clock, host, stm, at_start) = (
            rig.dev.clock.clone(),
            rig.host.clone(),
            stm.clone(),
            link_at_start.clone(),
        );
        rig.dev.clock.on_sleep(move |_| {
            if clock.ms() != 8000 {
                return;
            }
            // the link is up with the text protocol; the flash talks to the bootloader
            // simulator
            *tlock(&at_start) = Some(host.s().published.link);
            stm.use_simulator(true);
            stm.with_sim(|sim| {
                sim.boot_pin_resets = 1;
                sim.app_reply = b"gvers 2.1.0-revamped_C2 1712345678 ".to_vec();
            });
            host.s().to_receive.push_back(flash("c2", true, "C2"));
        });
    }
    rig.run_task(&mut link, 90_000);
    assert_eq!(*tlock(&link_at_start), Some(LinkState::Up));
    assert!(!rig.host.has(EventCode::StmFlashFailed));
    assert_eq!(rig.host.with_code(EventCode::StmFlashDone).len(), 1);
    stm.with_sim(|sim| {
        assert_eq!(&sim.flash[..img.len()], &img[..]);
        assert_eq!(sim.configs.last(), Some(&(115_200, false)));
    });
    // the flashed image becomes the last good one
    assert_eq!(rig.host.s().last_good_copies, vec![b"c2".to_vec()]);
    assert_eq!(rig.dev.fs.open_handles(), 0);
}

#[test]
fn flash_the_input_discard_reads_at_most_64_times_64_bytes() {
    // C++: 40 bytes per look and two looks per round (available() and read()): 128 looks and
    // 1024 bytes left. The port's read is the only look: 80 bytes per look give the same
    // 16 bytes per round that the 64-byte reads leave behind.
    let (polls, left) = discard_against(80, None);
    assert_eq!(polls, 64);
    assert_eq!(left, 1024);
    assert_eq!((DISCARD_READS, DISCARD_CHUNK), (64, 64));
}

#[test]
fn flash_the_input_discard_stops_once_nothing_is_left_even_after_one_byte() {
    // C++ counted 3 looks (available, read, available); the port reads until a read is empty
    let (polls, left) = discard_against(1, Some(1));
    assert_eq!(polls, 2);
    assert_eq!(left, 0);
}

#[test]
fn task_one_pass_takes_at_most_4_commands() {
    let rig = Rig::new();
    let mut link = rig.link();
    link.begin();
    for i in 0..6 {
        let mut c = command(StmCommandType::SetTarget, 0);
        c.pos = 10 + i;
        rig.host.s().to_receive.push_back(c);
    }
    rig.run_passes(&mut link, 1);
    let left: Vec<u8> = rig.host.s().to_receive.iter().map(|c| c.pos).collect();
    assert_eq!(left, vec![14, 15]);
    assert_eq!(COMMANDS_PER_PASS, 4);
}

#[test]
fn task_a_single_byte_read_on_its_own_is_part_of_the_reply() {
    let rig = Rig::new();
    let stm = rig.stm();
    stm.protocol(2);
    // passes run at even milliseconds: "2" arrives alone between "gproto " and CR LF
    stm.answer("gproto", |_, now, rx| {
        rx.inject_at(now + 3, b"gproto ");
        rx.inject_at(now + 5, b"2");
        rx.inject_at(now + 7, b"\r\n");
        String::new()
    });
    let mut link = rig.link();
    link.begin();
    rig.run_task(&mut link, 10_000);
    assert_eq!(rig.host.s().published.proto, 2);
    assert_eq!(rig.host.s().published.link, LinkState::Up);
}

#[test]
fn task_the_second_tick_runs_every_1000_ms_of_the_loop_clock() {
    let rig = Rig::new();
    let _stm = rig.stm();
    let mut link = rig.link();
    link.begin();
    let ticks = Arc::new(Mutex::new(Vec::<u64>::new()));
    {
        let (clock, host, ticks) = (rig.dev.clock.clone(), rig.host.clone(), ticks.clone());
        let mut seen = 0;
        rig.dev.clock.on_sleep(move |_| {
            let pass = clock.ms() - 2; // the pass that just ran
            let n = host.s().lease_records.len();
            if n != seen {
                tlock(&ticks).push(pass);
            }
            seen = n;
            // from here on the passes run at odd milliseconds
            if clock.ms() == 1502 {
                clock.advance_ms(1);
            }
        });
    }
    rig.run_task(&mut link, 3600);
    assert_eq!(*tlock(&ticks), vec![1000, 2001, 3001]);
    assert_eq!(SECOND_MS, 1000);
}

#[test]
fn task_the_next_policy_reset_waits_10_min_from_the_nrst_release() {
    let rig = Rig::new();
    let stm = rig.stm();
    stm.silent(true);
    let mut link = rig.link();
    link.begin();
    rig.run_task(&mut link, 720_000);
    let released: Vec<u64> = stm
        .nrst_writes()
        .iter()
        .filter(|(_, high)| !high)
        .map(|(t, _)| *t)
        .collect();
    // release_reset() in begin(), then two policy resets
    assert_eq!(released.len(), 3);
    assert!(released[1] > 60_000);
    assert!(released[2] - released[1] >= 600_000);
    assert!(released[2] - released[1] < 610_000);
}

#[test]
fn task_a_pass_returns_the_2_ms_delay_and_feeds_the_watchdog() {
    // Rust addition: the pass is the loop body of the thread
    let rig = Rig::new();
    let mut link = rig.link();
    link.begin();
    link.start(rig.dev.watchdog.clone());
    assert_eq!(rig.dev.watchdog.feeds(), 0);
    assert_eq!(link.pass(), PASS_DELAY_MS);
    assert_eq!(PASS_DELAY_MS, 2);
    assert_eq!(rig.dev.watchdog.feeds(), 1);
    assert!(rig.dev.clock.sleeps().is_empty());
}
