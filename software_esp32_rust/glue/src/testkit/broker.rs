//! Fake MQTT 3.1.1 broker: a [`TcpPeer`] that decodes the device's packets, records CONNECT,
//! PUBLISH, SUBSCRIBE, PUBACK, PINGREQ and DISCONNECT, and answers as scripted (CONNACK codes,
//! inbound PUBLISH, raw bytes, drops and stalls).

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard};

use super::net::{TcpPeer, Wire};
use super::{lock, FakeTcp};

/// A CONNECT as the broker decoded it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Connect {
    pub(crate) protocol: Vec<u8>,
    pub(crate) level: u8,
    pub(crate) flags: u8,
    pub(crate) keep_alive: u16,
    pub(crate) client_id: Vec<u8>,
    pub(crate) will_topic: Option<Vec<u8>>,
    pub(crate) will_message: Option<Vec<u8>>,
    pub(crate) user: Option<Vec<u8>>,
    pub(crate) password: Option<Vec<u8>>,
}

impl Connect {
    pub(crate) fn clean_session(&self) -> bool {
        self.flags & 0x02 != 0
    }
    pub(crate) fn will_qos(&self) -> u8 {
        (self.flags >> 3) & 3
    }
    pub(crate) fn will_retain(&self) -> bool {
        self.flags & 0x20 != 0
    }
}

/// A PUBLISH from the device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Publish {
    pub(crate) topic: Vec<u8>,
    pub(crate) payload: Vec<u8>,
    pub(crate) qos: u8,
    pub(crate) retain: bool,
    pub(crate) msg_id: Option<u16>,
}

/// Script and records of the broker.
pub(crate) struct BrokerState {
    /// CONNACK return code per CONNECT in order (`None`: no answer at all); after the list: 0.
    pub(crate) connack: VecDeque<Option<u8>>,
    /// Session-present flag of the CONNACKs.
    pub(crate) session_present: bool,
    /// Delay of every answer in ms.
    pub(crate) reply_ms: u64,
    /// PINGREQ gets a PINGRESP.
    pub(crate) ping_resp: bool,
    /// SUBSCRIBE gets a SUBACK.
    pub(crate) sub_ack: bool,
    pub(crate) connects: Vec<Connect>,
    pub(crate) published: Vec<Publish>,
    pub(crate) subscribed: Vec<(u16, Vec<u8>, u8)>,
    pub(crate) pubacks: Vec<u16>,
    pub(crate) pingreqs: u32,
    pub(crate) disconnects: u32,
    /// Packets the broker could not decode.
    pub(crate) garbage: u32,
    /// Connections the device closed.
    pub(crate) closes: u32,
    outbox: VecDeque<Vec<u8>>,
    drop_now: bool,
}

impl Default for BrokerState {
    fn default() -> Self {
        BrokerState {
            connack: VecDeque::new(),
            session_present: false,
            reply_ms: 1,
            ping_resp: true,
            sub_ack: true,
            connects: Vec::new(),
            published: Vec::new(),
            subscribed: Vec::new(),
            pubacks: Vec::new(),
            pingreqs: 0,
            disconnects: 0,
            garbage: 0,
            closes: 0,
            outbox: VecDeque::new(),
            drop_now: false,
        }
    }
}

impl BrokerState {
    /// Messages published to `topic`.
    pub(crate) fn published_to(&self, topic: &[u8]) -> Vec<Publish> {
        self.published
            .iter()
            .filter(|p| p.topic == topic)
            .cloned()
            .collect()
    }
}

/// The broker: one shared state behind every connection to it.
#[derive(Clone, Default)]
pub(crate) struct FakeBroker(Arc<Mutex<BrokerState>>);

/// MQTT remaining-length encoding.
pub(crate) fn remaining_length(mut n: usize, out: &mut Vec<u8>) {
    loop {
        let mut d = (n % 128) as u8;
        n /= 128;
        if n > 0 {
            d |= 0x80;
        }
        out.push(d);
        if n == 0 {
            break;
        }
    }
}

/// A complete PUBLISH packet (from the broker to the device).
pub(crate) fn publish_packet(
    topic: &[u8],
    payload: &[u8],
    qos: u8,
    retain: bool,
    msg_id: u16,
) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&(topic.len() as u16).to_be_bytes());
    body.extend_from_slice(topic);
    if qos > 0 {
        body.extend_from_slice(&msg_id.to_be_bytes());
    }
    body.extend_from_slice(payload);
    let mut p = vec![0x30 | (qos << 1) | u8::from(retain)];
    remaining_length(body.len(), &mut p);
    p.extend_from_slice(&body);
    p
}

impl FakeBroker {
    /// Serves `host:port` of `tcp`.
    pub(crate) fn attach(&self, tcp: &FakeTcp, host: &str, port: u16) {
        let state = self.0.clone();
        tcp.listen(host, port, move || {
            Box::new(BrokerConn {
                state: state.clone(),
                rx: Vec::new(),
            })
        });
    }
    pub(crate) fn state(&self) -> MutexGuard<'_, BrokerState> {
        lock(&self.0)
    }
    /// Sends a PUBLISH to the device on the current connection.
    pub(crate) fn send_publish(
        &self,
        topic: &[u8],
        payload: &[u8],
        qos: u8,
        retain: bool,
        id: u16,
    ) {
        self.send_raw(&publish_packet(topic, payload, qos, retain, id));
    }
    /// Sends raw bytes to the device on the current connection.
    pub(crate) fn send_raw(&self, bytes: &[u8]) {
        lock(&self.0).outbox.push_back(bytes.to_vec());
    }
    /// Closes the current connection (FIN).
    pub(crate) fn drop_connection(&self) {
        lock(&self.0).drop_now = true;
    }
}

struct BrokerConn {
    state: Arc<Mutex<BrokerState>>,
    rx: Vec<u8>,
}

/// Cursor over one packet body.
struct Body<'a> {
    b: &'a [u8],
    i: usize,
}

impl Body<'_> {
    fn u8(&mut self) -> Option<u8> {
        let v = *self.b.get(self.i)?;
        self.i += 1;
        Some(v)
    }
    fn u16(&mut self) -> Option<u16> {
        Some(u16::from(self.u8()?) << 8 | u16::from(self.u8()?))
    }
    fn bytes(&mut self) -> Option<Vec<u8>> {
        let n = usize::from(self.u16()?);
        let v = self.b.get(self.i..self.i + n)?.to_vec();
        self.i += n;
        Some(v)
    }
    fn rest(&self) -> Vec<u8> {
        self.b.get(self.i..).unwrap_or(&[]).to_vec()
    }
}

/// Splits one complete packet off `rx`: (first byte, body).
fn take_packet(rx: &mut Vec<u8>) -> Option<(u8, Vec<u8>)> {
    let first = *rx.first()?;
    let mut len = 0usize;
    let mut mult = 1usize;
    let mut i = 1;
    loop {
        let d = *rx.get(i)?;
        len += usize::from(d & 0x7F) * mult;
        mult *= 128;
        i += 1;
        if d & 0x80 == 0 {
            break;
        }
        if i > 4 {
            rx.clear(); // not MQTT: drop everything
            return None;
        }
    }
    if rx.len() < i + len {
        return None;
    }
    let body = rx[i..i + len].to_vec();
    rx.drain(..i + len);
    Some((first, body))
}

fn decode_connect(body: &[u8]) -> Option<Connect> {
    let mut b = Body { b: body, i: 0 };
    let mut c = Connect {
        protocol: b.bytes()?,
        level: b.u8()?,
        flags: b.u8()?,
        keep_alive: b.u16()?,
        client_id: b.bytes()?,
        ..Connect::default()
    };
    if c.flags & 0x04 != 0 {
        c.will_topic = Some(b.bytes()?);
        c.will_message = Some(b.bytes()?);
    }
    if c.flags & 0x80 != 0 {
        c.user = Some(b.bytes()?);
    }
    if c.flags & 0x40 != 0 {
        c.password = Some(b.bytes()?);
    }
    Some(c)
}

impl BrokerConn {
    fn handle(&mut self, st: &mut BrokerState, wire: &mut Wire, first: u8, body: &[u8], now: u64) {
        let at = now + st.reply_ms;
        match first & 0xF0 {
            0x10 => {
                let Some(c) = decode_connect(body) else {
                    st.garbage += 1;
                    return;
                };
                st.connects.push(c);
                if let Some(rc) = st.connack.pop_front().unwrap_or(Some(0)) {
                    wire.send_at(at, &[0x20, 0x02, u8::from(st.session_present), rc]);
                }
            }
            0x30 => {
                let qos = (first >> 1) & 3;
                let mut b = Body { b: body, i: 0 };
                let Some(topic) = b.bytes() else {
                    st.garbage += 1;
                    return;
                };
                let msg_id = if qos > 0 { b.u16() } else { None };
                st.published.push(Publish {
                    topic,
                    payload: b.rest(),
                    qos,
                    retain: first & 1 != 0,
                    msg_id,
                });
                if let (1, Some(id)) = (qos, msg_id) {
                    let [hi, lo] = id.to_be_bytes();
                    wire.send_at(at, &[0x40, 0x02, hi, lo]);
                }
            }
            0x40 => {
                let mut b = Body { b: body, i: 0 };
                match b.u16() {
                    Some(id) => st.pubacks.push(id),
                    None => st.garbage += 1,
                }
            }
            0x80 => {
                let mut b = Body { b: body, i: 0 };
                let Some(id) = b.u16() else {
                    st.garbage += 1;
                    return;
                };
                let mut granted = Vec::new();
                while let (Some(f), Some(q)) = (b.bytes(), b.u8()) {
                    st.subscribed.push((id, f, q));
                    granted.push(q);
                }
                if st.sub_ack {
                    let mut p = vec![0x90];
                    remaining_length(2 + granted.len(), &mut p);
                    p.extend_from_slice(&id.to_be_bytes());
                    p.extend_from_slice(&granted);
                    wire.send_at(at, &p);
                }
            }
            0xC0 => {
                st.pingreqs += 1;
                if st.ping_resp {
                    wire.send_at(at, &[0xD0, 0x00]);
                }
            }
            0xE0 => st.disconnects += 1,
            _ => st.garbage += 1,
        }
    }
}

impl TcpPeer for BrokerConn {
    fn on_data(&mut self, wire: &mut Wire, data: &[u8], now_ms: u64) {
        self.rx.extend_from_slice(data);
        let state = self.state.clone();
        let mut st = lock(&state);
        while let Some((first, body)) = take_packet(&mut self.rx) {
            self.handle(&mut st, wire, first, &body, now_ms);
        }
    }
    fn on_close(&mut self, _now_ms: u64) {
        lock(&self.state).closes += 1;
    }
    fn poll(&mut self, wire: &mut Wire, now_ms: u64) {
        let mut st = lock(&self.state);
        while let Some(p) = st.outbox.pop_front() {
            wire.send_at(now_ms, &p);
        }
        if st.drop_now {
            st.drop_now = false;
            wire.close_at(now_ms);
        }
    }
}
