//! Interop run of `mqtt_conn` against a real Mosquitto (GLUE-DESIGN-ESP.md 7, risk 11): connect
//! with the last will, retained publish and its clear, QoS 1 inbound, keepalive over idle
//! periods, a broker restart, the will on a dropped connection; and the MQTT task end to end
//! (`mqtt_client`: session with its last will, subscriptions, a target command and its retained
//! clear, a button confirmed by the broker's echo, the HA status, discovery runs, a QoS 1 command
//! kept by the persistent session while the task was away). Ignored by default (needs the
//! `mosquitto` binary of the tools/rust image); run it with
//!
//! ```text
//! bash tools/rust/docker.sh interop software_esp32_rust
//! ```
// host test code: the stack rule of the glue (design 2.4) is for the device
#![allow(clippy::large_stack_frames)]

use std::collections::HashMap;
use std::io::{ErrorKind, Read, Write};
use std::net::{Shutdown, TcpListener, ToSocketAddrs};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::sleep;
use std::time::{Duration, Instant};

use vdm_esp_core::common::{copy_string, LocalTime};
use vdm_esp_core::config::{Config, MqttMode};
use vdm_esp_core::event_log::{
    event_default_severity, make_event, Event, EventCode, EventFilter, EventLog,
};
use vdm_esp_core::failsafe::HaStatus;
use vdm_esp_core::json_api::MqttState;
use vdm_esp_core::link_policy::LinkState;
use vdm_esp_core::stm_codec::Profile;
use vdm_esp_core::stm_types::{StmCommand, StmCommandType, StmSnapshot};
use vdm_esp_glue::mqtt_client::{MqttClient, MqttHost, MqttPorts, MqttShared, LIST_FILE};
use vdm_esp_glue::mqtt_conn::{
    ConnectArgs, MqttConn, Will, MQTT_CONNECTED, MQTT_CONNECTION_LOST, MQTT_CONNECTION_TIMEOUT,
};
use vdm_esp_glue::port::{
    Clock, Fs, FsEntry, FsFile, HeapGate, OpenMode, Rtc, TcpConnector, TcpRead, TcpStream, Watchdog,
};

// ---------------------------------------------------------------- std ports

struct StdClock(Instant);

impl Clock for StdClock {
    fn now_ms(&self) -> u32 {
        (self.0.elapsed().as_millis() & 0xFFFF_FFFF) as u32
    }
    fn uptime_s(&self) -> u32 {
        self.0.elapsed().as_secs() as u32
    }
    fn sleep_ms(&self, ms: u32) {
        sleep(Duration::from_millis(u64::from(ms)));
    }
}

struct StdTcp;

struct StdStream(std::net::TcpStream);

impl TcpConnector for StdTcp {
    type Stream = StdStream;
    fn connect(&self, host: &str, port: u16, timeout_ms: u32) -> Option<StdStream> {
        let addr = (host, port).to_socket_addrs().ok()?.next()?;
        let s = std::net::TcpStream::connect_timeout(
            &addr,
            Duration::from_millis(u64::from(timeout_ms)),
        )
        .ok()?;
        s.set_write_timeout(Some(Duration::from_secs(10))).ok()?;
        s.set_nonblocking(true).ok()?;
        Some(StdStream(s))
    }
}

impl TcpStream for StdStream {
    fn read(&mut self, out: &mut [u8]) -> TcpRead {
        match self.0.read(out) {
            Ok(0) => TcpRead::Closed,
            Ok(n) => TcpRead::Data(n),
            Err(e) if e.kind() == ErrorKind::WouldBlock => TcpRead::Empty,
            Err(_) => TcpRead::Closed,
        }
    }
    fn connected(&mut self) -> bool {
        let mut b = [0u8; 1];
        match self.0.peek(&mut b) {
            Ok(0) => false,
            Ok(_) => true,
            Err(e) => e.kind() == ErrorKind::WouldBlock,
        }
    }
    fn write_all(&mut self, data: &[u8]) -> bool {
        let start = Instant::now();
        let mut rest = data;
        while !rest.is_empty() {
            match self.0.write(rest) {
                Ok(0) => return false,
                Ok(n) => rest = &rest[n..],
                Err(e) if e.kind() == ErrorKind::WouldBlock => {
                    if start.elapsed() > Duration::from_secs(10) {
                        return false;
                    }
                    sleep(Duration::from_millis(1));
                }
                Err(_) => return false,
            }
        }
        true
    }
    fn close(&mut self) {
        let _ = self.0.shutdown(Shutdown::Both);
    }
}

// ---------------------------------------------------------------- broker

/// One broker port per test process run; the tests run one after another.
static SERIAL: Mutex<()> = Mutex::new(());

struct Broker {
    port: u16,
    conf: std::path::PathBuf,
    child: Option<Child>,
}

impl Broker {
    fn start() -> Broker {
        let port = TcpListener::bind("127.0.0.1:0")
            .and_then(|l| l.local_addr())
            .map(|a| a.port())
            .expect("a free port");
        let conf = std::env::temp_dir().join(format!("vdmot-mosquitto-{port}.conf"));
        std::fs::write(
            &conf,
            format!("listener {port} 127.0.0.1\nallow_anonymous true\npersistence false\n"),
        )
        .expect("mosquitto config");
        let mut b = Broker {
            port,
            conf,
            child: None,
        };
        b.spawn();
        b
    }
    fn spawn(&mut self) {
        let child = Command::new("mosquitto")
            .arg("-c")
            .arg(&self.conf)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("mosquitto (tools/rust image)");
        self.child = Some(child);
        let deadline = Instant::now() + Duration::from_secs(10);
        while std::net::TcpStream::connect(("127.0.0.1", self.port)).is_err() {
            assert!(Instant::now() < deadline, "mosquitto did not start");
            sleep(Duration::from_millis(20));
        }
    }
    fn stop(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

impl Drop for Broker {
    fn drop(&mut self) {
        self.stop();
        let _ = std::fs::remove_file(&self.conf);
    }
}

// ---------------------------------------------------------------- helpers

type Conn = MqttConn<StdTcp, StdClock>;

fn client(keep_alive_s: u16) -> Conn {
    let mut c = MqttConn::new(StdTcp, StdClock(Instant::now()), 2304);
    c.set_socket_timeout(5);
    c.set_keep_alive(keep_alive_s);
    c
}

fn args<'a>(id: &'a [u8], will: Option<Will<'a>>, clean: bool) -> ConnectArgs<'a> {
    ConnectArgs {
        id,
        user: None,
        password: None,
        will,
        clean_session: clean,
    }
}

const STATUS: &[u8] = b"vdmot-test/status";

fn device_will() -> Option<Will<'static>> {
    Some(Will {
        topic: STATUS,
        qos: 0,
        retain: true,
        message: b"offline",
    })
}

/// Polls `c` for `ms` milliseconds; the messages that arrived.
fn collect(c: &mut Conn, ms: u64) -> Vec<(Vec<u8>, Vec<u8>)> {
    let mut got = Vec::new();
    let end = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < end {
        c.poll(|t, p| got.push((t.to_vec(), p.to_vec())));
        sleep(Duration::from_millis(5));
    }
    got
}

/// A raw MQTT client that publishes one QoS 1 message and waits for its PUBACK.
fn publish_qos1(port: u16, topic: &[u8], payload: &[u8], id: u16) {
    publish_qos1_with(port, topic, payload, id, false);
}

/// [`publish_qos1`] with the retain flag.
fn publish_qos1_with(port: u16, topic: &[u8], payload: &[u8], id: u16, retain: bool) {
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).expect("raw client");
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let client_id = b"raw-pub";
    let mut body = vec![0, 4, b'M', b'Q', b'T', b'T', 4, 0x02, 0, 30];
    body.extend_from_slice(&(client_id.len() as u16).to_be_bytes());
    body.extend_from_slice(client_id);
    let mut p = vec![0x10, body.len() as u8];
    p.extend_from_slice(&body);
    s.write_all(&p).unwrap();
    let mut connack = [0u8; 4];
    s.read_exact(&mut connack).unwrap();
    assert_eq!(connack, [0x20, 2, 0, 0]);
    let mut body = Vec::new();
    body.extend_from_slice(&(topic.len() as u16).to_be_bytes());
    body.extend_from_slice(topic);
    body.extend_from_slice(&id.to_be_bytes());
    body.extend_from_slice(payload);
    let mut p = vec![0x32 + u8::from(retain), body.len() as u8];
    p.extend_from_slice(&body);
    s.write_all(&p).unwrap();
    let mut puback = [0u8; 4];
    s.read_exact(&mut puback).unwrap();
    assert_eq!(puback[0], 0x40);
    s.write_all(&[0xE0, 0]).unwrap();
}

// ---------------------------------------------------------------- cases

#[test]
#[ignore = "needs mosquitto: see the module doc"]
fn connect_with_will_retained_publish_and_its_clear() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let broker = Broker::start();
    let mut device = client(60);
    assert!(device.connect(
        "127.0.0.1",
        broker.port,
        &args(b"vdmot-dev", device_will(), false)
    ));
    assert_eq!(device.state(), MQTT_CONNECTED);
    assert!(device.publish(STATUS, b"online", true));
    assert!(device.publish(b"vdmot-test/cmd/target", b"55", true));
    // a second client sees both retained messages
    let mut watcher = client(60);
    assert!(watcher.connect("127.0.0.1", broker.port, &args(b"watcher", None, true)));
    assert!(watcher.subscribe(b"vdmot-test/#", 0));
    let got = collect(&mut watcher, 500);
    assert!(
        got.contains(&(STATUS.to_vec(), b"online".to_vec())),
        "{got:?}"
    );
    assert!(
        got.contains(&(b"vdmot-test/cmd/target".to_vec(), b"55".to_vec())),
        "{got:?}"
    );
    // the device clears the retained command with an empty retained publish: the watcher gets
    // the empty message, a new subscriber gets nothing
    assert!(device.publish(b"vdmot-test/cmd/target", b"", true));
    let got = collect(&mut watcher, 500);
    assert_eq!(got, vec![(b"vdmot-test/cmd/target".to_vec(), Vec::new())]);
    let mut late = client(60);
    assert!(late.connect("127.0.0.1", broker.port, &args(b"late", None, true)));
    assert!(late.subscribe(b"vdmot-test/cmd/#", 0));
    assert!(collect(&mut late, 500).is_empty());
    device.disconnect();
    // a clean disconnect does not publish the will
    let got = collect(&mut watcher, 300);
    assert!(got.is_empty(), "{got:?}");
}

#[test]
#[ignore = "needs mosquitto: see the module doc"]
fn qos1_inbound_is_delivered_and_acknowledged() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let broker = Broker::start();
    let mut device = client(60);
    assert!(device.connect(
        "127.0.0.1",
        broker.port,
        &args(b"vdmot-dev", device_will(), false)
    ));
    assert!(device.subscribe(b"vdmot-test/valves/+/target", 1));
    collect(&mut device, 200); // the SUBACK
    publish_qos1(broker.port, b"vdmot-test/valves/3/target", b"42.5", 7);
    let got = collect(&mut device, 500);
    assert_eq!(
        got,
        vec![(b"vdmot-test/valves/3/target".to_vec(), b"42.5".to_vec())]
    );
    // acknowledged: a reconnect of the persistent session does not get it again
    device.disconnect();
    assert!(device.connect(
        "127.0.0.1",
        broker.port,
        &args(b"vdmot-dev", device_will(), false)
    ));
    assert!(collect(&mut device, 500).is_empty());
}

#[test]
#[ignore = "needs mosquitto: see the module doc"]
fn keepalive_holds_an_idle_session() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let broker = Broker::start();
    let mut device = client(2);
    assert!(device.connect(
        "127.0.0.1",
        broker.port,
        &args(b"vdmot-dev", device_will(), true)
    ));
    // 7 s idle: the broker drops a client after 1.5 keepalive periods without a packet
    collect(&mut device, 7000);
    assert!(device.connected());
    assert!(device.publish(b"vdmot-test/alive", b"1", false));
}

#[test]
#[ignore = "needs mosquitto: see the module doc"]
fn a_broker_restart_is_noticed_and_reconnected() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let mut broker = Broker::start();
    let mut device = client(2);
    assert!(device.connect(
        "127.0.0.1",
        broker.port,
        &args(b"vdmot-dev", device_will(), false)
    ));
    broker.stop();
    // the closed socket or the missing PINGRESP ends the session
    let end = Instant::now() + Duration::from_secs(10);
    while device.poll(|_, _| {}) {
        assert!(Instant::now() < end, "the session outlived the broker");
        sleep(Duration::from_millis(20));
    }
    assert!(
        [MQTT_CONNECTION_LOST, MQTT_CONNECTION_TIMEOUT].contains(&device.state()),
        "state {}",
        device.state()
    );
    assert!(!device.connected());
    broker.spawn();
    assert!(device.connect(
        "127.0.0.1",
        broker.port,
        &args(b"vdmot-dev", device_will(), false)
    ));
    assert!(device.publish(STATUS, b"online", true));
}

#[test]
#[ignore = "needs mosquitto: see the module doc"]
fn a_dropped_connection_publishes_the_will() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let broker = Broker::start();
    let mut watcher = client(60);
    assert!(watcher.connect("127.0.0.1", broker.port, &args(b"watcher", None, true)));
    assert!(watcher.subscribe(STATUS, 0));
    collect(&mut watcher, 200);
    let mut device = client(60);
    assert!(device.connect(
        "127.0.0.1",
        broker.port,
        &args(b"vdmot-dev", device_will(), false)
    ));
    assert!(device.publish(STATUS, b"online", true));
    assert_eq!(
        collect(&mut watcher, 300),
        vec![(STATUS.to_vec(), b"online".to_vec())]
    );
    // the glue drops the socket after a failed publish: no DISCONNECT, the broker sends the will
    device.drop_socket();
    assert!(!device.connected());
    assert_eq!(device.state(), MQTT_CONNECTION_LOST);
    let got = collect(&mut watcher, 1000);
    assert_eq!(got, vec![(STATUS.to_vec(), b"offline".to_vec())]);
}

// ---------------------------------------------------------------- the MQTT task

/// LittleFS in memory: the list file of the discovery runs.
#[derive(Clone, Default)]
struct MemFs(Arc<Mutex<HashMap<String, Vec<u8>>>>);

struct MemFile {
    fs: MemFs,
    path: String,
    pos: usize,
    write: bool,
}

impl MemFs {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        self.0.lock().unwrap().get(path).cloned()
    }
}

impl Fs for MemFs {
    type File = MemFile;
    fn mount(&self) -> bool {
        true
    }
    fn format(&self) -> bool {
        self.0.lock().unwrap().clear();
        true
    }
    fn open(&self, path: &str, mode: OpenMode) -> Option<MemFile> {
        let mut m = self.0.lock().unwrap();
        let pos = match mode {
            OpenMode::Read => {
                m.get(path)?;
                0
            }
            OpenMode::Write => {
                m.insert(path.to_string(), Vec::new());
                0
            }
            OpenMode::Append => m.entry(path.to_string()).or_default().len(),
        };
        Some(MemFile {
            fs: self.clone(),
            path: path.to_string(),
            pos,
            write: mode != OpenMode::Read,
        })
    }
    fn exists(&self, path: &str) -> bool {
        self.0.lock().unwrap().contains_key(path)
    }
    fn mkdir(&self, _path: &str) -> bool {
        true
    }
    fn remove(&self, path: &str) -> bool {
        self.0.lock().unwrap().remove(path).is_some()
    }
    fn rename(&self, from: &str, to: &str) -> bool {
        let mut m = self.0.lock().unwrap();
        match m.remove(from) {
            Some(d) => {
                m.insert(to.to_string(), d);
                true
            }
            None => false,
        }
    }
    fn list(&self, _dir: &str, _visit: &mut dyn FnMut(&FsEntry) -> bool) {}
    fn usage(&self) -> (u32, u32) {
        (0x17_0000, 0)
    }
}

impl FsFile for MemFile {
    fn read(&mut self, out: &mut [u8]) -> usize {
        let m = self.fs.0.lock().unwrap();
        let data = m.get(&self.path).map(|d| d.as_slice()).unwrap_or_default();
        let rest = data.get(self.pos..).unwrap_or_default();
        let n = rest.len().min(out.len());
        out[..n].copy_from_slice(&rest[..n]);
        self.pos += n;
        n
    }
    fn write(&mut self, data: &[u8]) -> usize {
        if !self.write {
            return 0;
        }
        let mut m = self.fs.0.lock().unwrap();
        let file = m.entry(self.path.clone()).or_default();
        file.truncate(self.pos);
        file.extend_from_slice(data);
        self.pos += data.len();
        data.len()
    }
    fn seek(&mut self, pos: u32) -> bool {
        self.pos = pos as usize;
        true
    }
    fn size(&self) -> u32 {
        self.read_len() as u32
    }
}

impl MemFile {
    fn read_len(&self) -> usize {
        self.fs
            .0
            .lock()
            .unwrap()
            .get(&self.path)
            .map_or(0, Vec::len)
    }
}

/// The heap always grants.
struct Grant;

impl HeapGate for Grant {
    fn grant(&self, _bytes: usize) -> bool {
        true
    }
}

/// RTC memory: the HA status record.
struct MemRtc(Mutex<Vec<u8>>);

impl Rtc for MemRtc {
    fn load(&self, offset: usize, out: &mut [u8]) {
        out.copy_from_slice(&self.0.lock().unwrap()[offset..offset + out.len()]);
    }
    fn store(&self, offset: usize, data: &[u8]) {
        self.0.lock().unwrap()[offset..offset + data.len()].copy_from_slice(data);
    }
}

/// No task watchdog on the host.
struct NoWatchdog;

impl Watchdog for NoWatchdog {
    fn feed(&self) {}
}

/// [`StdTcp`] that keeps a handle of the last socket, so the test can cut the connection the way
/// a dead network does (no DISCONNECT: the broker publishes the last will).
#[derive(Default)]
struct CuttableTcp(Mutex<Option<std::net::TcpStream>>);

impl CuttableTcp {
    fn cut(&self) {
        if let Some(s) = self.0.lock().unwrap().take() {
            let _ = s.shutdown(Shutdown::Both);
        }
    }
}

impl TcpConnector for CuttableTcp {
    type Stream = StdStream;
    fn connect(&self, host: &str, port: u16, timeout_ms: u32) -> Option<StdStream> {
        let s = StdTcp.connect(host, port, timeout_ms)?;
        *self.0.lock().unwrap() = s.0.try_clone().ok();
        Some(s)
    }
}

/// The other glue modules as the task sees them: an active config, an STM snapshot, the event
/// log, the command queue.
struct HostState {
    cfg: Config,
    snap: StmSnapshot,
    log: EventLog<64>,
    submitted: Vec<StmCommand>,
    ha_layout: u8,
}

struct Host {
    start: Instant,
    s: Mutex<HostState>,
}

impl Host {
    /// Mode MQTT + HA on the broker at `port`, valve 1 active, the STM link up and settled.
    fn new(port: u16) -> Host {
        let mut cfg = Config::default();
        cfg.mqtt.mode = MqttMode::MqttHa;
        copy_string(&mut cfg.mqtt.host, b"127.0.0.1");
        cfg.mqtt.port = port;
        cfg.mqtt.keep_alive_s = 30;
        copy_string(&mut cfg.station, b"VdMot");
        cfg.valves[0].active = true;
        let mut snap = StmSnapshot {
            link: LinkState::Up,
            proto: 3,
            sensors_settled: true,
            ..StmSnapshot::default()
        };
        snap.valves[0].known = true;
        Host {
            start: Instant::now(),
            s: Mutex::new(HostState {
                cfg,
                snap,
                log: EventLog::new(),
                submitted: Vec::new(),
                ha_layout: 2,
            }),
        }
    }
    fn submitted(&self) -> Vec<StmCommand> {
        self.s.lock().unwrap().submitted.clone()
    }
    fn discovery_runs(&self) -> usize {
        let s = self.s.lock().unwrap();
        let f = EventFilter::default();
        let mut out = vec![Event::default(); 64];
        let mut next = 0;
        let n = s.log.read(&f, &mut out, &mut next);
        out[..n]
            .iter()
            .filter(|e| e.code == EventCode::HaDiscoverySent)
            .count()
    }
}

impl MqttHost for Host {
    fn uptime_s(&self) -> u32 {
        self.start.elapsed().as_secs() as u32
    }
    fn submit(&self, cmd: &StmCommand) -> bool {
        self.s.lock().unwrap().submitted.push(cmd.clone());
        true
    }
    fn stm_snapshot_revision(&self) -> u32 {
        1
    }
    fn read_stm_snapshot(&self, out: &mut StmSnapshot) {
        out.clone_from(&self.s.lock().unwrap().snap);
    }
    fn read_profile(&self, _valve: u8, out: &mut Profile) {
        *out = Profile::default();
    }
    fn calib_next_epoch(&self) -> i64 {
        0
    }
    fn config_revision(&self) -> u32 {
        1
    }
    fn with_config(&self, f: &mut dyn FnMut(&Config)) {
        f(&self.s.lock().unwrap().cfg);
    }
    fn fs_ready(&self) -> bool {
        true
    }
    fn ha_cleanup_done(&self) -> bool {
        true
    }
    fn set_ha_cleanup_done(&self) {}
    fn ha_layout(&self) -> u8 {
        self.s.lock().unwrap().ha_layout
    }
    fn set_ha_layout(&self, layout: u8) {
        self.s.lock().unwrap().ha_layout = layout;
    }
    fn log(&self, code: EventCode, valve: u8, arg1: i32, arg2: i32, text: &[u8]) -> u32 {
        let e = make_event(code, event_default_severity(code), valve, arg1, arg2, text);
        self.s.lock().unwrap().log.append(&e)
    }
    fn read_events_since(&self, since: u32, out: &mut [Event]) -> (usize, u32) {
        let f = EventFilter {
            since_seq: since,
            ..EventFilter::default()
        };
        let mut next = 0;
        let n = self.s.lock().unwrap().log.read(&f, out, &mut next);
        (n, next)
    }
    fn net_up(&self) -> bool {
        true
    }
    fn net_ip(&self) -> u32 {
        u32::from_le_bytes([127, 0, 0, 1])
    }
    fn local_time(&self) -> LocalTime {
        LocalTime::default()
    }
    fn restart_pending(&self) -> bool {
        false
    }
    fn request_restart(&self, _reason: u8, _delay_ms: u32) {}
}

type Task<'a> = MqttClient<'a, StdClock, MemFs, CuttableTcp, Grant, MemRtc, NoWatchdog, &'a Host>;

/// A message the watcher received: topic, payload.
type Msg = (Vec<u8>, Vec<u8>);

/// Runs the task with its own pass delays (real time) and polls the watcher meanwhile, until
/// `done` or `ms` passed; whether `done` was reached.
fn run_task(
    task: &mut Task<'_>,
    watcher: &mut Conn,
    seen: &mut Vec<Msg>,
    ms: u64,
    done: &dyn Fn(&[Msg]) -> bool,
) -> bool {
    let end = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < end {
        let d = task.pass();
        let wake = Instant::now() + Duration::from_millis(u64::from(d));
        while Instant::now() < wake {
            watcher.poll(|t, p| seen.push((t.to_vec(), p.to_vec())));
            sleep(Duration::from_millis(1));
        }
        if done(seen) {
            return true;
        }
    }
    false
}

fn saw(seen: &[Msg], topic: &[u8], payload: &[u8]) -> bool {
    seen.iter().any(|(t, p)| t == topic && p == payload)
}

#[test]
#[ignore = "needs mosquitto: see the module doc"]
fn the_mqtt_task_end_to_end() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let broker = Broker::start();
    let mut watcher = client(60);
    assert!(watcher.connect("127.0.0.1", broker.port, &args(b"watcher", None, true)));
    assert!(watcher.subscribe(b"VdMot/#", 0));
    assert!(watcher.subscribe(b"homeassistant/#", 0));
    collect(&mut watcher, 200);
    let clock = StdClock(Instant::now());
    let fs = MemFs::default();
    let tcp = CuttableTcp::default();
    let rtc = MemRtc(Mutex::new(vec![0; 64]));
    let host = Host::new(broker.port);
    let shared = MqttShared::default();
    let ports = MqttPorts {
        clock: &clock,
        fs: &fs,
        tcp: &tcp,
        gate: &Grant,
        rtc: &rtc,
        rtc_offset: 0,
        mac: [0x24, 0x0A, 0xC4, 0x12, 0x34, 0x56],
    };
    let mut task: Task<'_> = MqttClient::new(ports, &host, &shared);
    task.begin();
    task.start(NoWatchdog);
    let mut seen = Vec::new();

    // the session: online after the last will is set, the values, the discovery run on connect
    assert!(run_task(
        &mut task,
        &mut watcher,
        &mut seen,
        15_000,
        &|_| { host.discovery_runs() == 1 }
    ));
    let st = shared.status();
    assert_eq!(st.state, MqttState::Connected);
    assert_eq!(st.client_id.as_slice(), b"VdMot-123456");
    assert!(saw(&seen, b"VdMot/status", b"online"));
    assert!(saw(&seen, b"VdMot/stm/status", b"online"));
    let state_config = b"homeassistant/text/VdMot/state/config";
    assert!(
        seen.iter().any(|(t, p)| t == state_config
            && String::from_utf8_lossy(p).contains("\"unique_id\":\"VdMot.common.state\"")),
        "no state config"
    );
    let list = fs.read(LIST_FILE).expect("the discovery list");
    assert!(String::from_utf8_lossy(&list).contains("homeassistant/text/VdMot/state/config\n"));
    // the configs are retained: a late subscriber gets them
    let mut late = client(60);
    assert!(late.connect("127.0.0.1", broker.port, &args(b"late", None, true)));
    assert!(late.subscribe(&state_config[..], 0));
    assert_eq!(collect(&mut late, 300).len(), 1);
    late.disconnect();

    // a retained target command (QoS 1): submitted, its retained message cleared
    publish_qos1_with(broker.port, b"VdMot/valves/1/target/set", b"42", 11, true);
    assert!(run_task(&mut task, &mut watcher, &mut seen, 3000, &|_| {
        !host.submitted().is_empty()
    }));
    let cmd = &host.submitted()[0];
    assert_eq!(
        (cmd.kind, cmd.valve, cmd.pos),
        (StmCommandType::SetTarget, 0, 42)
    );
    run_task(&mut task, &mut watcher, &mut seen, 300, &|_| false);
    assert!(saw(&seen, b"VdMot/valves/1/target/set", b""));
    let mut late = client(60);
    assert!(late.connect("127.0.0.1", broker.port, &args(b"late", None, true)));
    assert!(late.subscribe(b"VdMot/valves/+/target/set", 0));
    assert!(
        collect(&mut late, 300).is_empty(),
        "the command is still retained"
    );
    late.disconnect();

    // a retained button: cleared, confirmed by the broker's echo of the clear, then submitted
    publish_qos1_with(broker.port, b"VdMot/cmd/detect", b"PRESS", 12, true);
    assert!(run_task(&mut task, &mut watcher, &mut seen, 3000, &|_| {
        host.submitted().len() == 2
    }));
    assert_eq!(host.submitted()[1].kind, StmCommandType::Detect);
    run_task(&mut task, &mut watcher, &mut seen, 300, &|_| false); // the watcher's copy
    assert!(saw(&seen, b"VdMot/cmd/detect", b""));

    // Home Assistant goes away and comes back: offline, then a discovery run on its birth
    publish_qos1(broker.port, b"homeassistant/status", b"offline", 13);
    assert!(run_task(&mut task, &mut watcher, &mut seen, 3000, &|_| {
        shared.regulator_state().ha == HaStatus::Offline
    }));
    publish_qos1(broker.port, b"homeassistant/status", b"online", 14);
    assert!(run_task(
        &mut task,
        &mut watcher,
        &mut seen,
        15_000,
        &|_| { host.discovery_runs() == 2 }
    ));
    assert_eq!(shared.regulator_state().ha, HaStatus::Online);

    // the network fails: the broker publishes the last will; a QoS 1 command sent meanwhile is
    // kept by the persistent session and arrives after the reconnect
    tcp.cut();
    let will = collect(&mut watcher, 1000);
    assert!(saw(&will, b"VdMot/status", b"offline"), "{will:?}");
    publish_qos1(broker.port, b"VdMot/valves/1/target/set", b"17", 15);
    assert!(run_task(
        &mut task,
        &mut watcher,
        &mut seen,
        10_000,
        &|_| { host.submitted().iter().any(|c| c.pos == 17) }
    ));
    assert_eq!(shared.status().reconnects, 2);
}
