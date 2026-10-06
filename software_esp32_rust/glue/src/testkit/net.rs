//! Fake network: TCP connections to scripted peers, UDP, the Ethernet and WiFi interfaces, SNTP
//! and the ping sessions.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};

use super::{lock, FakeClock, Journal};
use crate::port::{
    Ethernet, IpInfo, IpSetup, Pinger, Sntp, TcpConnector, TcpRead, TcpStream, Udp, Wifi,
};

// ---------------------------------------------------------------- TCP

/// What the device's end of a connection sees next.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Inbound {
    Data(Vec<u8>),
    Close,
}

/// The shared state of one connection.
#[derive(Default)]
pub(crate) struct Wire {
    /// Bytes and the close of the peer, each due at a fake time (ms since boot).
    inbound: VecDeque<(u64, Inbound)>,
    /// Every byte the device wrote.
    pub(crate) written: Vec<u8>,
    /// The device closed its end.
    pub(crate) closed_by_device: bool,
    /// The peer closed (its close was delivered).
    pub(crate) closed_by_peer: bool,
    /// Writes fail from now on (a stalled or dead peer: WiFiClient's 10 s write timeout).
    pub(crate) fail_writes: bool,
}

impl Wire {
    /// Queues bytes for the device, due at `at_ms`.
    pub(crate) fn send_at(&mut self, at_ms: u64, bytes: &[u8]) {
        self.inbound
            .push_back((at_ms, Inbound::Data(bytes.to_vec())));
    }
    /// Queues the peer's close, due at `at_ms` (after the bytes queued before it).
    pub(crate) fn close_at(&mut self, at_ms: u64) {
        self.inbound.push_back((at_ms, Inbound::Close));
    }
    /// Bytes queued for the device and not read yet.
    pub(crate) fn pending(&self) -> usize {
        self.inbound
            .iter()
            .map(|(_, i)| match i {
                Inbound::Data(d) => d.len(),
                Inbound::Close => 0,
            })
            .sum()
    }
}

/// The other end of a connection: a server under the test's control.
pub(crate) trait TcpPeer: Send {
    /// The connection was opened at `now_ms`.
    fn on_connect(&mut self, _wire: &mut Wire, _now_ms: u64) {}
    /// The device wrote `data` at `now_ms`.
    fn on_data(&mut self, wire: &mut Wire, data: &[u8], now_ms: u64);
    /// The device closed its end.
    fn on_close(&mut self, _now_ms: u64) {}
    /// Before every look at the inbound side: time-driven actions.
    fn poll(&mut self, _wire: &mut Wire, _now_ms: u64) {}
}

type PeerFactory = Box<dyn FnMut() -> Box<dyn TcpPeer> + Send>;

/// One `connect` call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ConnectRecord {
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) timeout_ms: u32,
    pub(crate) at_ms: u64,
}

#[derive(Default)]
struct TcpState {
    listeners: BTreeMap<(String, u16), PeerFactory>,
    connects: Vec<ConnectRecord>,
    refuse_next: u32,
    /// Fake time a connect takes (DNS + handshake), or the whole timeout when refused.
    connect_ms: u64,
    wires: Vec<Arc<Mutex<Wire>>>,
}

/// TCP connections of a boot: `connect` reaches the peer registered for host and port, others
/// are refused (after the timeout, like an unanswered SYN).
#[derive(Clone)]
pub(crate) struct FakeTcp {
    clock: FakeClock,
    state: Arc<Mutex<TcpState>>,
    journal: Journal,
}

impl FakeTcp {
    pub(crate) fn new(clock: FakeClock, journal: Journal) -> Self {
        FakeTcp {
            clock,
            state: Arc::default(),
            journal,
        }
    }
    /// Registers a server: every connection to `host:port` gets a new peer from `factory`.
    pub(crate) fn listen(
        &self,
        host: &str,
        port: u16,
        factory: impl FnMut() -> Box<dyn TcpPeer> + Send + 'static,
    ) {
        lock(&self.state)
            .listeners
            .insert((host.to_string(), port), Box::new(factory));
    }
    /// The next `n` connects fail.
    pub(crate) fn refuse_next(&self, n: u32) {
        lock(&self.state).refuse_next = n;
    }
    /// Fake time a successful connect takes.
    pub(crate) fn set_connect_ms(&self, ms: u64) {
        lock(&self.state).connect_ms = ms;
    }
    /// Every connect so far.
    pub(crate) fn connects(&self) -> Vec<ConnectRecord> {
        lock(&self.state).connects.clone()
    }
    /// The wire of the `i`-th successful connection.
    pub(crate) fn wire(&self, i: usize) -> Arc<Mutex<Wire>> {
        lock(&self.state).wires[i].clone()
    }
    /// Successful connections so far.
    pub(crate) fn connections(&self) -> usize {
        lock(&self.state).wires.len()
    }
}

impl TcpConnector for FakeTcp {
    type Stream = FakeTcpStream;
    fn connect(&self, host: &str, port: u16, timeout_ms: u32) -> Option<FakeTcpStream> {
        let mut st = lock(&self.state);
        st.connects.push(ConnectRecord {
            host: host.to_string(),
            port,
            timeout_ms,
            at_ms: self.clock.ms(),
        });
        self.journal.note(format!("tcp connect {host}:{port}"));
        let refused = st.refuse_next > 0;
        if refused {
            st.refuse_next -= 1;
        }
        let factory = st.listeners.get_mut(&(host.to_string(), port));
        let (Some(factory), false) = (factory, refused) else {
            drop(st);
            self.clock.advance_ms(u64::from(timeout_ms));
            return None;
        };
        let mut peer = factory();
        let connect_ms = st.connect_ms;
        let wire = Arc::new(Mutex::new(Wire::default()));
        st.wires.push(wire.clone());
        drop(st);
        self.clock.advance_ms(connect_ms);
        peer.on_connect(&mut lock(&wire), self.clock.ms());
        Some(FakeTcpStream {
            clock: self.clock.clone(),
            wire,
            peer: Arc::new(Mutex::new(peer)),
        })
    }
}

/// The device's end of a fake connection.
pub(crate) struct FakeTcpStream {
    clock: FakeClock,
    wire: Arc<Mutex<Wire>>,
    peer: Arc<Mutex<Box<dyn TcpPeer>>>,
}

impl FakeTcpStream {
    fn poll(&self) -> MutexGuard<'_, Wire> {
        let now = self.clock.ms();
        let mut w = lock(&self.wire);
        lock(&self.peer).poll(&mut w, now);
        w
    }
}

impl TcpStream for FakeTcpStream {
    fn read(&mut self, out: &mut [u8]) -> TcpRead {
        let now = self.clock.ms();
        let mut w = self.poll();
        if w.closed_by_device {
            return TcpRead::Closed;
        }
        let mut n = 0;
        while n < out.len() {
            match w.inbound.front_mut() {
                Some((due, Inbound::Data(d))) if *due <= now => {
                    let k = d.len().min(out.len() - n);
                    out[n..n + k].copy_from_slice(&d[..k]);
                    d.drain(..k);
                    n += k;
                    if d.is_empty() {
                        w.inbound.pop_front();
                    }
                }
                _ => break,
            }
        }
        if n > 0 {
            return TcpRead::Data(n);
        }
        match w.inbound.front() {
            Some((due, Inbound::Close)) if *due <= now => {
                w.inbound.pop_front();
                w.closed_by_peer = true;
                TcpRead::Closed
            }
            _ if w.closed_by_peer => TcpRead::Closed,
            _ => TcpRead::Empty,
        }
    }
    fn connected(&mut self) -> bool {
        let now = self.clock.ms();
        let w = self.poll();
        if w.closed_by_device || w.closed_by_peer {
            return false;
        }
        // like a peek: a close is seen once the bytes before it were read
        !matches!(w.inbound.front(), Some((due, Inbound::Close)) if *due <= now)
    }
    fn write_all(&mut self, data: &[u8]) -> bool {
        let now = self.clock.ms();
        let mut w = self.poll();
        if w.closed_by_device || w.closed_by_peer || w.fail_writes {
            return false;
        }
        w.written.extend_from_slice(data);
        lock(&self.peer).on_data(&mut w, data, now);
        true
    }
    fn close(&mut self) {
        let mut w = lock(&self.wire);
        if !w.closed_by_device {
            w.closed_by_device = true;
            lock(&self.peer).on_close(self.clock.ms());
        }
    }
}

impl Drop for FakeTcpStream {
    fn drop(&mut self) {
        self.close();
    }
}

// ---------------------------------------------------------------- UDP

#[derive(Default)]
struct UdpState {
    sent: Vec<(u32, u16, Vec<u8>)>,
    fail: bool,
}

/// UDP socket: records the datagrams.
#[derive(Clone, Default)]
pub(crate) struct FakeUdp(Arc<Mutex<UdpState>>);

impl FakeUdp {
    pub(crate) fn sent(&self) -> Vec<(u32, u16, Vec<u8>)> {
        lock(&self.0).sent.clone()
    }
    pub(crate) fn set_fail(&self, fail: bool) {
        lock(&self.0).fail = fail;
    }
}

impl Udp for FakeUdp {
    fn send_to(&mut self, ip: u32, port: u16, data: &[u8]) -> bool {
        let mut s = lock(&self.0);
        if s.fail {
            return false;
        }
        s.sent.push((ip, port, data.to_vec()));
        true
    }
}

// ---------------------------------------------------------------- interfaces

/// One `begin` of an interface.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SetupRecord {
    pub(crate) hostname: String,
    pub(crate) fixed: Option<IpInfo>,
}

impl SetupRecord {
    fn of(s: &IpSetup) -> Self {
        SetupRecord {
            hostname: s.hostname.to_string(),
            fixed: s.fixed,
        }
    }
}

/// Ethernet state the test drives (links, addresses) and the calls of the glue.
pub(crate) struct EthState {
    pub(crate) begin_ok: bool,
    pub(crate) restart_ok: bool,
    pub(crate) begins: Vec<SetupRecord>,
    pub(crate) restarts: u32,
    pub(crate) link: bool,
    pub(crate) has_ip: bool,
    pub(crate) got_ip: u32,
    pub(crate) info: IpInfo,
    pub(crate) mac: [u8; 6],
}

impl Default for EthState {
    fn default() -> Self {
        EthState {
            begin_ok: true,
            restart_ok: true,
            begins: Vec::new(),
            restarts: 0,
            link: false,
            has_ip: false,
            got_ip: 0,
            info: IpInfo::default(),
            mac: [0xA8, 0x03, 0x2A, 0xA1, 0xB2, 0xC3],
        }
    }
}

/// The Ethernet interface of a boot.
#[derive(Clone, Default)]
pub(crate) struct FakeEthernet(Arc<Mutex<EthState>>);

impl FakeEthernet {
    pub(crate) fn state(&self) -> MutexGuard<'_, EthState> {
        lock(&self.0)
    }
    /// Link up and an address assigned (a GOT_IP event).
    pub(crate) fn got_ip(&self, info: IpInfo) {
        let mut s = lock(&self.0);
        s.link = true;
        s.has_ip = true;
        s.got_ip += 1;
        s.info = info;
    }
    /// Link down (DISCONNECTED): the address is gone.
    pub(crate) fn link_down(&self) {
        let mut s = lock(&self.0);
        s.link = false;
        s.has_ip = false;
    }
}

impl Ethernet for FakeEthernet {
    fn begin(&mut self, setup: &IpSetup) -> bool {
        let mut s = lock(&self.0);
        s.begins.push(SetupRecord::of(setup));
        s.begin_ok
    }
    fn restart(&mut self) -> bool {
        let mut s = lock(&self.0);
        s.restarts += 1;
        s.restart_ok
    }
    fn link(&self) -> bool {
        lock(&self.0).link
    }
    fn has_ip(&self) -> bool {
        lock(&self.0).has_ip
    }
    fn got_ip_count(&self) -> u32 {
        lock(&self.0).got_ip
    }
    fn info(&self) -> IpInfo {
        lock(&self.0).info
    }
    fn mac(&self) -> [u8; 6] {
        lock(&self.0).mac
    }
}

/// WiFi state the test drives and the calls of the glue.
pub(crate) struct WifiState {
    pub(crate) begin_ok: bool,
    pub(crate) begins: Vec<SetupRecord>,
    pub(crate) connects: Vec<(Vec<u8>, Vec<u8>)>,
    pub(crate) reconnects: u32,
    pub(crate) stops: u32,
    pub(crate) up: bool,
    pub(crate) got_ip: u32,
    pub(crate) unrequested_disconnects: u32,
    pub(crate) info: IpInfo,
    pub(crate) rssi: i8,
    pub(crate) mac: [u8; 6],
}

impl Default for WifiState {
    fn default() -> Self {
        WifiState {
            begin_ok: true,
            begins: Vec::new(),
            connects: Vec::new(),
            reconnects: 0,
            stops: 0,
            up: false,
            got_ip: 0,
            unrequested_disconnects: 0,
            info: IpInfo::default(),
            rssi: -60,
            mac: [0x24, 0x0A, 0xC4, 0x12, 0x34, 0x56],
        }
    }
}

/// The WiFi station of a boot.
#[derive(Clone, Default)]
pub(crate) struct FakeWifi(Arc<Mutex<WifiState>>);

impl FakeWifi {
    pub(crate) fn state(&self) -> MutexGuard<'_, WifiState> {
        lock(&self.0)
    }
    /// The station got an address.
    pub(crate) fn got_ip(&self, info: IpInfo) {
        let mut s = lock(&self.0);
        s.up = true;
        s.got_ip += 1;
        s.info = info;
    }
    /// The access point dropped the station (not requested by the glue).
    pub(crate) fn drop_station(&self) {
        let mut s = lock(&self.0);
        s.up = false;
        s.unrequested_disconnects += 1;
    }
}

impl Wifi for FakeWifi {
    fn begin(&mut self, setup: &IpSetup) -> bool {
        let mut s = lock(&self.0);
        s.begins.push(SetupRecord::of(setup));
        s.begin_ok
    }
    fn connect(&mut self, ssid: &[u8], password: &[u8]) {
        lock(&self.0)
            .connects
            .push((ssid.to_vec(), password.to_vec()));
    }
    fn reconnect(&mut self) {
        lock(&self.0).reconnects += 1;
    }
    fn stop(&mut self) {
        let mut s = lock(&self.0);
        s.stops += 1;
        s.up = false;
    }
    fn up(&self) -> bool {
        lock(&self.0).up
    }
    fn got_ip_count(&self) -> u32 {
        lock(&self.0).got_ip
    }
    fn take_unrequested_disconnect(&mut self) -> bool {
        let mut s = lock(&self.0);
        if s.unrequested_disconnects == 0 {
            return false;
        }
        s.unrequested_disconnects -= 1;
        true
    }
    fn info(&self) -> IpInfo {
        lock(&self.0).info
    }
    fn rssi(&self) -> i8 {
        lock(&self.0).rssi
    }
    fn mac(&self) -> [u8; 6] {
        lock(&self.0).mac
    }
}

/// SNTP state: the servers configured and the syncs the test delivers.
#[derive(Default)]
pub(crate) struct SntpState {
    pub(crate) configured: Vec<Option<String>>,
    pub(crate) syncs: u32,
    pub(crate) last_epoch: u32,
}

/// The SNTP client of a boot.
#[derive(Clone, Default)]
pub(crate) struct FakeSntp(Arc<Mutex<SntpState>>);

impl FakeSntp {
    pub(crate) fn state(&self) -> MutexGuard<'_, SntpState> {
        lock(&self.0)
    }
    /// An answer of the server: the sync notification (the test sets the wall clock itself).
    pub(crate) fn sync(&self, epoch: u32) {
        let mut s = lock(&self.0);
        s.syncs += 1;
        s.last_epoch = epoch;
    }
}

impl Sntp for FakeSntp {
    fn configure(&mut self, server: Option<&str>) {
        lock(&self.0).configured.push(server.map(str::to_string));
    }
    fn sync_count(&self) -> u32 {
        lock(&self.0).syncs
    }
    fn last_sync_epoch(&self) -> u32 {
        lock(&self.0).last_epoch
    }
}

/// Ping sessions: scripted answers (true = reply, false = timeout), one per started session.
pub(crate) struct PingState {
    pub(crate) start_ok: bool,
    pub(crate) answers: VecDeque<bool>,
    /// Answer when `answers` is empty.
    pub(crate) default_answer: bool,
    pub(crate) started: Vec<u32>,
    pub(crate) deletes: u32,
    active: bool,
    done: bool,
    replies: u32,
}

impl Default for PingState {
    fn default() -> Self {
        PingState {
            start_ok: true,
            answers: VecDeque::new(),
            default_answer: true,
            started: Vec::new(),
            deletes: 0,
            active: false,
            done: false,
            replies: 0,
        }
    }
}

/// The ping sessions of a boot.
#[derive(Clone, Default)]
pub(crate) struct FakePinger(Arc<Mutex<PingState>>);

impl FakePinger {
    pub(crate) fn state(&self) -> MutexGuard<'_, PingState> {
        lock(&self.0)
    }
    /// The running session gets its answer (reply or timeout) and reports done.
    pub(crate) fn step(&self) {
        let mut s = lock(&self.0);
        if !s.active || s.done {
            return;
        }
        let reply = s.answers.pop_front().unwrap_or(s.default_answer);
        s.replies += u32::from(reply);
        s.done = true;
    }
}

impl Pinger for FakePinger {
    fn start(&mut self, ip: u32) -> bool {
        let mut s = lock(&self.0);
        s.started.push(ip);
        if !s.start_ok {
            return false;
        }
        s.active = true;
        s.done = false;
        s.replies = 0;
        true
    }
    fn done(&self) -> bool {
        lock(&self.0).done
    }
    fn replies(&self) -> u32 {
        lock(&self.0).replies
    }
    fn delete(&mut self) {
        let mut s = lock(&self.0);
        s.deletes += 1;
        s.active = false;
        s.done = false;
    }
}
