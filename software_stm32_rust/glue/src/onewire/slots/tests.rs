// New cases (no C++ counterpart: the C++ suites never run the bit slots of OneWire.cpp): the
// slots of the library against a pin model with a clock in microseconds and devices on the
// line, and delayMicroseconds on a cycle counter. The times are the library's (OneWire.cpp
// reset, write_bit, read_bit).

use std::cell::Cell;
use std::vec;
use std::vec::Vec;

use super::*;

/// What the slots did to the pin, with the time in microseconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    Low(u32),
    Release(u32),
    /// a read of the line and the level it gave
    Sample(u32, bool),
    /// the masked window begins (true) or ends (false)
    Mask(bool),
}

/// The pin, the clock and the line: the pull-up raises it unless the pin or a device pulls it
/// low.
struct Pin {
    t: u32,
    ops: Vec<Op>,
    masked: bool,
    low_since: Option<u32>,
    /// a device pulls the line low in [from, to)
    pulls: Vec<(u32, u32)>,
    /// a device answers a reset pulse: low from 30 us to 150 us after the release
    device: bool,
    /// bits a device sends in read slots: a 0 holds the line low for 30 us from the master's
    /// falling edge
    send: Vec<bool>,
}

impl Pin {
    fn new() -> Self {
        Pin {
            t: 0,
            ops: Vec::new(),
            masked: false,
            low_since: None,
            pulls: Vec::new(),
            device: false,
            send: Vec::new(),
        }
    }

    fn with_device() -> Self {
        Pin {
            device: true,
            ..Pin::new()
        }
    }

    fn pulled(&self) -> bool {
        self.pulls
            .iter()
            .any(|&(from, to)| from <= self.t && self.t < to)
    }
}

impl OneWirePin for Pin {
    fn drive_low(&mut self) {
        self.ops.push(Op::Low(self.t));
        self.low_since = Some(self.t);
        if !self.send.is_empty() && !self.send.remove(0) {
            self.pulls.push((self.t, self.t + 30));
        }
    }

    fn release(&mut self) {
        self.ops.push(Op::Release(self.t));
        if let Some(since) = self.low_since.take() {
            if self.device && self.t - since >= 480 {
                self.pulls.push((self.t + 30, self.t + 150));
            }
        }
    }

    fn level(&mut self) -> bool {
        let high = self.low_since.is_none() && !self.pulled();
        self.ops.push(Op::Sample(self.t, high));
        high
    }

    fn delay_us(&mut self, us: u32) {
        self.t += us;
    }

    fn masked<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        assert!(!self.masked, "nested masked window");
        self.ops.push(Op::Mask(true));
        self.masked = true;
        let r = f(self);
        self.masked = false;
        self.ops.push(Op::Mask(false));
        r
    }
}

fn reset(pin: Pin) -> (bool, Pin) {
    let mut line = PinLine(pin);
    let present = line.reset();
    (present, line.0)
}

// ---- reset

#[test]
fn reset_with_a_device_480_us_low_the_sample_70_us_after_the_release_then_410_us() {
    let (present, pin) = reset(Pin::with_device());
    assert!(present);
    assert_eq!(
        pin.ops,
        vec![
            Op::Release(0),
            Op::Sample(2, true),
            Op::Mask(true),
            Op::Low(2),
            Op::Mask(false),
            Op::Mask(true),
            Op::Release(482),
            Op::Sample(552, false),
            Op::Mask(false),
        ]
    );
    assert_eq!(pin.t, 962);
}

#[test]
fn reset_without_a_device_finds_the_line_high_no_presence() {
    let (present, pin) = reset(Pin::new());
    assert!(!present);
    assert_eq!(pin.ops[7], Op::Sample(552, true));
    assert_eq!(pin.t, 962);
}

#[test]
fn reset_of_a_line_held_low_gives_up_after_124_waits_of_2_us_without_a_pulse() {
    let mut pin = Pin::with_device();
    pin.pulls.push((0, 100_000));
    let (present, pin) = reset(pin);
    assert!(!present);
    let mut want = vec![Op::Release(0)];
    want.extend((1..=124).map(|i| Op::Sample(2 * i, false)));
    assert_eq!(pin.ops, want);
    assert_eq!(pin.t, 248);
}

#[test]
fn reset_takes_a_line_that_rises_at_the_last_wait() {
    // the 124th read (248 us) sees it high: the reset pulse follows at once
    let mut pin = Pin::with_device();
    pin.pulls.push((0, 248));
    let (present, pin) = reset(pin);
    assert!(present);
    assert_eq!(pin.ops[124], Op::Sample(248, true));
    assert_eq!(pin.ops[126], Op::Low(248));
    assert_eq!(pin.t, 248 + 480 + 70 + 410);
}

#[test]
fn reset_gives_up_on_a_line_that_rises_one_microsecond_too_late() {
    let mut pin = Pin::with_device();
    pin.pulls.push((0, 249));
    let (present, pin) = reset(pin);
    assert!(!present);
    assert_eq!(pin.ops.last(), Some(&Op::Sample(248, false)));
    assert!(!pin.ops.iter().any(|o| matches!(o, Op::Low(_))));
}

// ---- write and read slots

fn write(bit: bool) -> Pin {
    let mut line = PinLine(Pin::new());
    line.write_bit(bit);
    line.0
}

#[test]
fn write_1_is_10_us_low_inside_the_masked_window_then_55_us() {
    let pin = write(true);
    assert_eq!(
        pin.ops,
        vec![Op::Mask(true), Op::Low(0), Op::Release(10), Op::Mask(false)]
    );
    assert_eq!(pin.t, 65);
}

#[test]
fn write_0_is_65_us_low_inside_the_masked_window_then_5_us() {
    let pin = write(false);
    assert_eq!(
        pin.ops,
        vec![Op::Mask(true), Op::Low(0), Op::Release(65), Op::Mask(false)]
    );
    assert_eq!(pin.t, 70);
}

#[test]
fn read_is_3_us_low_and_samples_10_us_after_the_release_inside_the_window_then_53_us() {
    let mut pin = Pin::new();
    pin.send = vec![false, true];
    let mut line = PinLine(pin);
    assert!(!line.read_bit());
    assert_eq!(
        line.0.ops,
        vec![
            Op::Mask(true),
            Op::Low(0),
            Op::Release(3),
            Op::Sample(13, false),
            Op::Mask(false)
        ]
    );
    assert_eq!(line.0.t, 66);
    // a 1: the device leaves the line to the pull-up
    assert!(line.read_bit());
    assert_eq!(line.0.ops[8], Op::Sample(66 + 13, true));
    assert_eq!(line.0.t, 132);
}

// ---- delayMicroseconds on the cycle counter

/// A counter that advances by `step` on every read after the first.
struct Counter {
    now: Cell<u32>,
    step: u32,
    reads: Cell<u32>,
}

impl Counter {
    fn new(start: u32, step: u32) -> Self {
        Counter {
            now: Cell::new(start),
            step,
            reads: Cell::new(0),
        }
    }
}

impl CycleCounter for Counter {
    fn cycles(&self) -> u32 {
        if self.reads.get() > 0 {
            self.now.set(self.now.get().wrapping_add(self.step));
        }
        self.reads.set(self.reads.get() + 1);
        self.now.get()
    }
}

#[test]
fn spin_us_waits_until_us_times_the_clock_cycles_passed() {
    // 10 us at 84 MHz: 840 cycles; one cycle per read: the first read and 840 more
    let c = Counter::new(1000, 1);
    spin_us(&c, 10, 84);
    assert_eq!(c.reads.get(), 841);
    assert_eq!(c.now.get() - 1000, 840);
    // 96 MHz, 7 cycles per read: until 960 passed (138 reads after the first)
    let c = Counter::new(0, 7);
    spin_us(&c, 10, 96);
    assert_eq!(c.reads.get(), 1 + 138);
    assert!(c.now.get() >= 960 && c.now.get() < 960 + 7);
}

#[test]
fn spin_us_counts_across_the_wrap_of_the_counter() {
    let start = u32::MAX - 100;
    let c = Counter::new(start, 1);
    spin_us(&c, 3, 84);
    assert_eq!(c.reads.get(), 1 + 252);
    assert_eq!(c.now.get(), start.wrapping_add(252));
}

#[test]
fn spin_us_of_0_reads_twice() {
    let c = Counter::new(5, 1);
    spin_us(&c, 0, 84);
    assert_eq!(c.reads.get(), 2);
}
