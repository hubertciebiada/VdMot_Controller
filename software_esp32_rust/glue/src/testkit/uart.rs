//! Fake UART (Serial2) with an optional peer on the other end, and the GPIO pins.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};

use super::{lock, FakeClock, Journal};
use crate::port::{InputPin, OutputPin, Uart};

/// Something on the other end of the UART (the fake STM).
pub(crate) trait SerialPeer: Send {
    /// The port was (re)configured.
    fn on_configure(&mut self, _baud: u32, _even_parity: bool) {}
    /// The device sent bytes at `now_ms`.
    fn on_tx(&mut self, data: &[u8], now_ms: u64, rx: &mut UartRx);
    /// Before every look at the RX side: bytes that are due go into `rx`.
    fn poll(&mut self, _now_ms: u64, _rx: &mut UartRx) {}
}

/// The RX side as a peer sees it: bytes queued with the fake time they arrive.
#[derive(Default)]
pub(crate) struct UartRx {
    queue: VecDeque<(u64, u8)>,
}

impl UartRx {
    /// Queues bytes that arrive at `at_ms`.
    pub(crate) fn inject_at(&mut self, at_ms: u64, bytes: &[u8]) {
        self.queue.extend(bytes.iter().map(|&b| (at_ms, b)));
    }
}

/// State of the port.
pub(crate) struct UartState {
    pub(crate) open: bool,
    pub(crate) baud: u32,
    pub(crate) even_parity: bool,
    pub(crate) configures: u32,
    /// RX ring capacity once configured (Arduino-ESP32's default 256 before).
    pub(crate) rx_capacity: usize,
    /// Due bytes in the ring.
    ring: VecDeque<u8>,
    rx: UartRx,
    /// Bytes lost to a full ring.
    pub(crate) rx_dropped: usize,
    /// Every byte written.
    pub(crate) tx: Vec<u8>,
    /// `write` accepts at most this many bytes per call.
    pub(crate) tx_accept: usize,
    peer: Option<Box<dyn SerialPeer>>,
}

/// `Serial2` of a boot.
#[derive(Clone)]
pub(crate) struct FakeUart {
    clock: FakeClock,
    state: Arc<Mutex<UartState>>,
}

impl FakeUart {
    pub(crate) fn new(clock: FakeClock) -> Self {
        FakeUart {
            clock,
            state: Arc::new(Mutex::new(UartState {
                open: false,
                baud: 0,
                even_parity: false,
                configures: 0,
                rx_capacity: 256,
                ring: VecDeque::new(),
                rx: UartRx::default(),
                rx_dropped: 0,
                tx: Vec::new(),
                tx_accept: usize::MAX,
                peer: None,
            })),
        }
    }
    pub(crate) fn state(&self) -> MutexGuard<'_, UartState> {
        lock(&self.state)
    }
    /// Puts `peer` on the other end.
    pub(crate) fn attach(&self, peer: Box<dyn SerialPeer>) {
        lock(&self.state).peer = Some(peer);
    }
    /// Queues bytes that arrive at `at_ms` (`None` = now).
    pub(crate) fn inject(&self, bytes: &[u8], at_ms: Option<u64>) {
        let at = at_ms.unwrap_or_else(|| self.clock.ms());
        lock(&self.state).rx.inject_at(at, bytes);
    }
    /// The bytes written since the last call.
    pub(crate) fn take_tx(&self) -> Vec<u8> {
        std::mem::take(&mut lock(&self.state).tx)
    }
    /// Bytes waiting in the ring now.
    pub(crate) fn available(&self) -> usize {
        let mut s = lock(&self.state);
        Self::pump(&mut s, self.clock.ms());
        s.ring.len()
    }
    fn pump(s: &mut UartState, now: u64) {
        if let Some(mut p) = s.peer.take() {
            p.poll(now, &mut s.rx);
            s.peer = Some(p);
        }
        while let Some(&(due, b)) = s.rx.queue.front() {
            if due > now {
                break;
            }
            s.rx.queue.pop_front();
            if s.ring.len() < s.rx_capacity {
                s.ring.push_back(b);
            } else {
                s.rx_dropped += 1;
            }
        }
    }
}

impl Uart for FakeUart {
    fn configure(&mut self, baud: u32, even_parity: bool) {
        let mut s = lock(&self.state);
        s.open = true;
        s.baud = baud;
        s.even_parity = even_parity;
        s.configures += 1;
        s.rx_capacity = crate::board::STM_RX_BUFFER_SIZE;
        s.ring.clear();
        let now = self.clock.ms();
        while s.rx.queue.front().is_some_and(|&(due, _)| due <= now) {
            s.rx.queue.pop_front();
        }
        if let Some(p) = s.peer.as_mut() {
            p.on_configure(baud, even_parity);
        }
    }
    fn read(&mut self, out: &mut [u8]) -> usize {
        let mut s = lock(&self.state);
        if !s.open {
            return 0;
        }
        Self::pump(&mut s, self.clock.ms());
        let n = out.len().min(s.ring.len());
        for (o, b) in out.iter_mut().zip(s.ring.drain(..n)) {
            *o = b;
        }
        n
    }
    fn write(&mut self, data: &[u8]) -> usize {
        let mut s = lock(&self.state);
        if !s.open {
            return 0;
        }
        let n = data.len().min(s.tx_accept);
        s.tx.extend_from_slice(&data[..n]);
        let now = self.clock.ms();
        if let Some(mut p) = s.peer.take() {
            p.on_tx(&data[..n], now, &mut s.rx);
            s.peer = Some(p);
        }
        n
    }
}

// ---------------------------------------------------------------- GPIO

type InputFn = Box<dyn Fn(u64) -> bool + Send>;
type WriteHook = Box<dyn FnMut(u8, bool) + Send>;

#[derive(Default)]
struct GpioState {
    levels: BTreeMap<u8, bool>,
    writes: Vec<(u8, bool)>,
    inputs: BTreeMap<u8, InputFn>,
    on_write: Option<WriteHook>,
}

/// The GPIO pins of a boot: outputs record their writes (journal `gpio 15=1`), inputs read a
/// level the test sets (HIGH by default: the pull-up).
#[derive(Clone)]
pub(crate) struct FakeGpio {
    clock: FakeClock,
    state: Arc<Mutex<GpioState>>,
    journal: Journal,
}

impl FakeGpio {
    pub(crate) fn new(clock: FakeClock, journal: Journal) -> Self {
        FakeGpio {
            clock,
            state: Arc::default(),
            journal,
        }
    }
    /// An output pin.
    pub(crate) fn output(&self, pin: u8) -> FakeOutputPin {
        FakeOutputPin {
            gpio: self.clone(),
            pin,
        }
    }
    /// An input pin.
    pub(crate) fn input(&self, pin: u8) -> FakeInputPin {
        FakeInputPin {
            gpio: self.clone(),
            pin,
        }
    }
    /// The last level written to `pin`.
    pub(crate) fn level(&self, pin: u8) -> Option<bool> {
        lock(&self.state).levels.get(&pin).copied()
    }
    /// Every write in order.
    pub(crate) fn writes(&self) -> Vec<(u8, bool)> {
        lock(&self.state).writes.clone()
    }
    /// `pin` reads LOW while `low(now_ms)` is true.
    pub(crate) fn set_input(&self, pin: u8, low: impl Fn(u64) -> bool + Send + 'static) {
        lock(&self.state).inputs.insert(pin, Box::new(low));
    }
    /// Runs `hook` after every write (pin, level), e.g. a peer watching NRST.
    pub(crate) fn on_write(&self, hook: impl FnMut(u8, bool) + Send + 'static) {
        lock(&self.state).on_write = Some(Box::new(hook));
    }
}

/// An output pin of [`FakeGpio`].
pub(crate) struct FakeOutputPin {
    gpio: FakeGpio,
    pin: u8,
}

impl OutputPin for FakeOutputPin {
    fn set(&mut self, high: bool) {
        let mut s = lock(&self.gpio.state);
        s.levels.insert(self.pin, high);
        s.writes.push((self.pin, high));
        self.gpio
            .journal
            .note(format!("gpio {}={}", self.pin, u8::from(high)));
        if let Some(h) = s.on_write.as_mut() {
            h(self.pin, high);
        }
    }
}

/// An input pin of [`FakeGpio`].
pub(crate) struct FakeInputPin {
    gpio: FakeGpio,
    pin: u8,
}

impl InputPin for FakeInputPin {
    fn is_low(&mut self) -> bool {
        let now = self.gpio.clock.ms();
        lock(&self.gpio.state)
            .inputs
            .get(&self.pin)
            .is_some_and(|f| f(now))
    }
}
