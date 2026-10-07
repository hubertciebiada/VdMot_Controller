//! Interop run of `mqtt_conn` against a real Mosquitto (GLUE-DESIGN-ESP.md 7, risk 11): connect
//! with the last will, retained publish and its clear, QoS 1 inbound, keepalive over idle
//! periods, a broker restart, the will on a dropped connection. Ignored by default (needs the
//! `mosquitto` binary of the tools/rust image); run it with
//!
//! ```text
//! bash tools/rust/docker.sh interop software_esp32_rust
//! ```
// host test code: the stack rule of the glue (design 2.4) is for the device
#![allow(clippy::large_stack_frames)]

use std::io::{ErrorKind, Read, Write};
use std::net::{Shutdown, TcpListener, ToSocketAddrs};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::thread::sleep;
use std::time::{Duration, Instant};

use vdm_esp_glue::mqtt_conn::{
    ConnectArgs, MqttConn, Will, MQTT_CONNECTED, MQTT_CONNECTION_LOST, MQTT_CONNECTION_TIMEOUT,
};
use vdm_esp_glue::port::{Clock, TcpConnector, TcpRead, TcpStream};

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
    let mut p = vec![0x32, body.len() as u8];
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
