//! MQTT 3.1.1 client connection with the semantics of PubSubClient 2.8 (knolleary), the library
//! the C++ `mqtt_client.cpp` drove over Arduino-ESP32's `WiFiClient` (docs/rust/GLUE-DESIGN-ESP.md
//! 3.1, row `mqtt_conn`): CONNECT with will and clean-session flag, the CONNACK wait of the
//! socket timeout, keepalive PINGREQ and its timeout (state -4), one inbound packet per
//! [`MqttConn::poll`], QoS 0 publishes built in one buffer (a message that does not fit is
//! refused), QoS 1 inbound messages acknowledged after the callback, oversized inbound packets
//! dropped, SUBSCRIBE without waiting for the SUBACK, the state codes -4..5 (event 203, `mqttRc`).
//!
//! Kept from the library: a CONNECT is answered by any 4-byte packet whose fourth byte is the
//! return code; a failed string length check in `connect` closes the socket without a state;
//! a byte that does not arrive within the socket timeout ends the packet where it is (the rest
//! of it is read as the next packet); the topic reaches the callback as a C string (cut at its
//! first NUL). Changed: the waits poll every millisecond where the library spun; a PUBLISH whose
//! topic length runs past the packet reaches no callback (the library moved memory past the
//! packet for it); docs/rust/PORT-NOTES.md lists both.

use crate::port::{Clock, TcpConnector, TcpRead, TcpStream};

/// The server did not answer within the socket timeout (CONNACK or PINGRESP).
pub const MQTT_CONNECTION_TIMEOUT: i32 = -4;
/// The connection broke while connected.
pub const MQTT_CONNECTION_LOST: i32 = -3;
/// The TCP connection could not be opened.
pub const MQTT_CONNECT_FAILED: i32 = -2;
/// Not connected (also after `disconnect` or an invalid remaining length).
pub const MQTT_DISCONNECTED: i32 = -1;
/// Connected.
pub const MQTT_CONNECTED: i32 = 0;
/// CONNACK 1: unacceptable protocol version.
pub const MQTT_CONNECT_BAD_PROTOCOL: i32 = 1;
/// CONNACK 2: identifier rejected.
pub const MQTT_CONNECT_BAD_CLIENT_ID: i32 = 2;
/// CONNACK 3: server unavailable.
pub const MQTT_CONNECT_UNAVAILABLE: i32 = 3;
/// CONNACK 4: bad user name or password.
pub const MQTT_CONNECT_BAD_CREDENTIALS: i32 = 4;
/// CONNACK 5: not authorized.
pub const MQTT_CONNECT_UNAUTHORIZED: i32 = 5;

/// PubSubClient's default keepalive in seconds (`MQTT_KEEPALIVE`).
pub const DEFAULT_KEEP_ALIVE_S: u16 = 15;
/// PubSubClient's default socket timeout in seconds (`MQTT_SOCKET_TIMEOUT`).
pub const DEFAULT_SOCKET_TIMEOUT_S: u16 = 15;
/// TCP connect timeout: `WiFiClient`'s default (C++ `mqtt::kConnectTimeoutMs`).
pub const CONNECT_TIMEOUT_MS: u32 = 3000;
/// Room for the fixed header in front of a packet in the buffer (`MQTT_MAX_HEADER_SIZE`).
pub const MAX_HEADER_SIZE: usize = 5;
/// Smallest buffer: a CONNECT preamble and a PUBACK always fit.
pub const MIN_BUFFER_SIZE: u16 = 16;
/// Bytes read from the socket at a time (WiFiClient kept an RX buffer as well).
const RX_CHUNK: usize = 512;

const CONNECT: u8 = 0x10;
const PUBLISH: u8 = 0x30;
const PUBACK: u8 = 0x40;
/// SUBSCRIBE with the QoS 1 bits its fixed header needs (`MQTTSUBSCRIBE | MQTTQOS1`).
const SUBSCRIBE_QOS1: u8 = 0x82;
const PINGREQ: u8 = 0xC0;
const PINGRESP: u8 = 0xD0;
const DISCONNECT: u8 = 0xE0;
const QOS1: u8 = 1 << 1;
/// CONNECT flag: a will follows.
const WILL_FLAG: u8 = 0x04;

/// The last will of a session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Will<'a> {
    /// Topic (a C string: cut at its first NUL).
    pub topic: &'a [u8],
    /// QoS 0..2 (written into the flags as given).
    pub qos: u8,
    /// Retained.
    pub retain: bool,
    /// Message (a C string).
    pub message: &'a [u8],
}

/// The arguments of `PubSubClient::connect(id, user, pass, willTopic, willQos, willRetain,
/// willMessage, cleanSession)`; strings are C strings (cut at their first NUL).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConnectArgs<'a> {
    /// Client identifier.
    pub id: &'a [u8],
    /// User name; `None` = none.
    pub user: Option<&'a [u8]>,
    /// Password; only sent with a user.
    pub password: Option<&'a [u8]>,
    /// Last will; `None` = none.
    pub will: Option<Will<'a>>,
    /// Clean session.
    pub clean_session: bool,
}

/// The C string view of `s`: up to its first NUL.
fn c_str(s: &[u8]) -> &[u8] {
    match s.iter().position(|&b| b == 0) {
        Some(n) => s.get(..n).unwrap_or(s),
        None => s,
    }
}

/// One MQTT connection (PubSubClient with its `WiFiClient`).
pub struct MqttConn<N: TcpConnector, C: Clock> {
    net: N,
    clock: C,
    stream: Option<N::Stream>,
    /// The packet buffer (PubSubClient's `buffer`, `bufferSize` bytes).
    buffer: Vec<u8>,
    rx: Vec<u8>,
    rx_pos: usize,
    rx_len: usize,
    keep_alive_s: u16,
    socket_timeout_s: u16,
    next_msg_id: u16,
    last_out: u32,
    last_in: u32,
    ping_outstanding: bool,
    state: i32,
}

impl<N: TcpConnector, C: Clock> MqttConn<N, C> {
    /// A client with a packet buffer of `buffer_size` bytes (at least [`MIN_BUFFER_SIZE`]),
    /// allocated at boot and never resized (C++ `setBufferSize(kBufferSize)` in `mqtt::begin`).
    pub fn new(net: N, clock: C, buffer_size: u16) -> Self {
        MqttConn {
            net,
            clock,
            stream: None,
            buffer: vec![0; usize::from(buffer_size.max(MIN_BUFFER_SIZE))],
            rx: vec![0; RX_CHUNK],
            rx_pos: 0,
            rx_len: 0,
            keep_alive_s: DEFAULT_KEEP_ALIVE_S,
            socket_timeout_s: DEFAULT_SOCKET_TIMEOUT_S,
            next_msg_id: 0,
            last_out: 0,
            last_in: 0,
            ping_outstanding: false,
            state: MQTT_DISCONNECTED,
        }
    }

    /// The packet buffer size.
    pub fn buffer_size(&self) -> usize {
        self.buffer.len()
    }

    /// Keepalive in seconds (sent in CONNECT, drives PINGREQ).
    pub fn set_keep_alive(&mut self, seconds: u16) {
        self.keep_alive_s = seconds;
    }

    /// How long a CONNACK or a byte of a packet may take, in seconds.
    pub fn set_socket_timeout(&mut self, seconds: u16) {
        self.socket_timeout_s = seconds;
    }

    /// The state: [`MQTT_CONNECTED`], a negative client state or a CONNACK return code.
    pub fn state(&self) -> i32 {
        self.state
    }

    /// `WiFiClient::stop()`: closes the socket (its unread bytes go too).
    fn stop(&mut self) {
        if let Some(mut s) = self.stream.take() {
            s.close();
        }
        self.rx_pos = 0;
        self.rx_len = 0;
    }

    /// Closes the socket and leaves the state as it is: the next [`MqttConn::connected`] turns a
    /// connected state into [`MQTT_CONNECTION_LOST`] (the C++ glue's `gNet.stop()` after a failed
    /// publish).
    pub fn drop_socket(&mut self) {
        self.stop();
    }

    fn socket_connected(&mut self) -> bool {
        self.stream.as_mut().is_some_and(|s| s.connected())
    }

    /// The session is up. A socket that is gone turns a connected state into
    /// [`MQTT_CONNECTION_LOST`].
    pub fn connected(&mut self) -> bool {
        if self.socket_connected() {
            return self.state == MQTT_CONNECTED;
        }
        if self.state == MQTT_CONNECTED {
            self.state = MQTT_CONNECTION_LOST;
            self.stop();
        }
        false
    }

    /// `WiFiClient::available() > 0`.
    fn available(&mut self) -> bool {
        if self.rx_pos < self.rx_len {
            return true;
        }
        let Some(s) = self.stream.as_mut() else {
            return false;
        };
        match s.read(&mut self.rx) {
            TcpRead::Data(n) if n > 0 => {
                self.rx_pos = 0;
                self.rx_len = n.min(self.rx.len());
                true
            }
            _ => false,
        }
    }

    /// Waits until a byte is there, at most the socket timeout counted from `since`.
    fn wait_available(&mut self, since: u32) -> bool {
        let limit = u32::from(self.socket_timeout_s) * 1000;
        loop {
            if self.available() {
                return true;
            }
            if self.clock.now_ms().wrapping_sub(since) >= limit {
                return false;
            }
            self.clock.sleep_ms(1);
        }
    }

    /// `readByte`: the next byte within the socket timeout.
    fn read_byte(&mut self) -> Option<u8> {
        let since = self.clock.now_ms();
        if !self.wait_available(since) {
            return None;
        }
        let b = self.rx.get(self.rx_pos).copied();
        self.rx_pos += 1;
        b
    }

    fn put(&mut self, at: usize, b: u8) {
        if let Some(slot) = self.buffer.get_mut(at) {
            *slot = b;
        }
    }

    fn at(&self, at: usize) -> u8 {
        self.buffer.get(at).copied().unwrap_or(0)
    }

    /// `readPacket`: one packet into the buffer; its length there (0: nothing usable) and the
    /// length of its remaining-length field.
    fn read_packet(&mut self) -> (usize, usize) {
        let Some(first) = self.read_byte() else {
            return (0, 0);
        };
        self.put(0, first);
        let mut len = 1usize;
        let is_publish = first & 0xF0 == PUBLISH;
        let mut multiplier = 1u32;
        let mut length = 0u32;
        loop {
            if len == 5 {
                // invalid remaining length encoding: kill the connection
                self.state = MQTT_DISCONNECTED;
                self.stop();
                return (0, 0);
            }
            let Some(digit) = self.read_byte() else {
                return (0, 0);
            };
            self.put(len, digit);
            len += 1;
            length += u32::from(digit & 127) * multiplier;
            multiplier <<= 7;
            if digit & 128 == 0 {
                break;
            }
        }
        let llen = len - 1;
        let mut start = 0;
        if is_publish {
            // the topic length, read whatever the remaining length says
            for _ in 0..2 {
                let Some(b) = self.read_byte() else {
                    return (0, llen);
                };
                self.put(len, b);
                len += 1;
            }
            start = 2;
        }
        let mut idx = len;
        for _ in start..length {
            let Some(digit) = self.read_byte() else {
                return (0, llen);
            };
            // the bytes beyond the buffer are read and not stored
            if let Some(slot) = self.buffer.get_mut(len) {
                *slot = digit;
                len += 1;
            }
            idx += 1;
        }
        if idx > self.buffer.len() {
            len = 0; // too long for the buffer: ignored
        }
        (len, llen)
    }

    /// `buildHeader` + one socket write of the packet whose variable part is
    /// `buffer[MAX_HEADER_SIZE..MAX_HEADER_SIZE + length]`.
    fn write_packet(&mut self, header: u8, length: usize) -> bool {
        let mut len_buf = [0u8; 4];
        let mut llen = 0;
        let mut rest = length as u16;
        loop {
            let mut digit = (rest & 127) as u8;
            rest >>= 7;
            if rest > 0 {
                digit |= 0x80;
            }
            len_buf[llen] = digit;
            llen += 1;
            if rest == 0 {
                break;
            }
        }
        let start = MAX_HEADER_SIZE - 1 - llen;
        self.put(start, header);
        for (i, &d) in len_buf[..llen].iter().enumerate() {
            self.put(start + 1 + i, d);
        }
        let end = MAX_HEADER_SIZE + length;
        let ok = match (self.stream.as_mut(), self.buffer.get(start..end)) {
            (Some(s), Some(bytes)) => s.write_all(bytes),
            _ => false,
        };
        self.last_out = self.clock.now_ms();
        ok
    }

    /// `writeString`: a C string with its 16-bit length at `pos`; the position after it.
    fn write_string(&mut self, s: &[u8], pos: usize) -> usize {
        let s = c_str(s);
        let n = s.len();
        self.put(pos, (n >> 8) as u8);
        self.put(pos + 1, n as u8);
        for (i, &b) in s.iter().enumerate() {
            self.put(pos + 2 + i, b);
        }
        pos + 2 + n
    }

    /// `CHECK_STRING_LENGTH`: the string fits after `pos`.
    fn fits(&self, pos: usize, s: &[u8]) -> bool {
        let n = c_str(s).len().min(self.buffer.len());
        pos + 2 + n <= self.buffer.len()
    }

    /// Connects to `host:port` (DNS and TCP within [`CONNECT_TIMEOUT_MS`]) and opens the session;
    /// true when connected (also when it already was). False: the TCP connection failed
    /// ([`MQTT_CONNECT_FAILED`]), no answer within the socket timeout
    /// ([`MQTT_CONNECTION_TIMEOUT`]), a CONNACK refusal (its code), or a string that does not fit
    /// the buffer (the state stays).
    pub fn connect(&mut self, host: &str, port: u16, a: &ConnectArgs) -> bool {
        if self.connected() {
            return true;
        }
        if !self.socket_connected() {
            self.stop();
            match self.net.connect(host, port, CONNECT_TIMEOUT_MS) {
                Some(s) => self.stream = Some(s),
                None => {
                    self.state = MQTT_CONNECT_FAILED;
                    return false;
                }
            }
        }
        self.next_msg_id = 1;
        let Some(len) = self.build_connect(a) else {
            self.stop();
            return false;
        };
        self.write_packet(CONNECT, len - MAX_HEADER_SIZE);
        self.await_connack()
    }

    /// The CONNECT packet in the buffer; its end, `None` when a string does not fit
    /// (`CHECK_STRING_LENGTH`).
    fn build_connect(&mut self, a: &ConnectArgs) -> Option<usize> {
        const PREAMBLE: [u8; 7] = [0x00, 0x04, b'M', b'Q', b'T', b'T', 4];
        if let Some(dst) = self.buffer.get_mut(MAX_HEADER_SIZE..MAX_HEADER_SIZE + 7) {
            dst.copy_from_slice(&PREAMBLE);
        }
        // the library's `0x04 | (willQos << 3) | (willRetain << 5)`: the shifted fields never
        // set bit 2, so adding the will flag is the same OR
        let mut flags = match a.will {
            Some(w) => WILL_FLAG + ((w.qos << 3) | (u8::from(w.retain) << 5)),
            None => 0,
        };
        if a.clean_session {
            flags |= 0x02;
        }
        if a.user.is_some() {
            flags |= 0x80;
            if a.password.is_some() {
                flags |= 0x40;
            }
        }
        let [hi, lo] = self.keep_alive_s.to_be_bytes();
        self.put(12, flags);
        self.put(13, hi);
        self.put(14, lo);
        let mut len = self.checked_string(15, a.id)?;
        if let Some(w) = a.will {
            len = self.checked_string(len, w.topic)?;
            len = self.checked_string(len, w.message)?;
        }
        if let Some(u) = a.user {
            len = self.checked_string(len, u)?;
            if let Some(p) = a.password {
                len = self.checked_string(len, p)?;
            }
        }
        Some(len)
    }

    /// `CHECK_STRING_LENGTH` + `writeString`: `None` when the string does not fit after `pos`.
    fn checked_string(&mut self, pos: usize, s: &[u8]) -> Option<usize> {
        self.fits(pos, s).then(|| self.write_string(s, pos))
    }

    /// Waits for the answer to the CONNECT that just went out.
    fn await_connack(&mut self) -> bool {
        let now = self.clock.now_ms();
        self.last_in = now;
        self.last_out = now;
        if !self.wait_available(self.last_in) {
            self.state = MQTT_CONNECTION_TIMEOUT;
            self.stop();
            return false;
        }
        let (n, _) = self.read_packet();
        if n == 4 {
            let rc = self.at(3);
            if rc == 0 {
                self.last_in = self.clock.now_ms();
                self.ping_outstanding = false;
                self.state = MQTT_CONNECTED;
                return true;
            }
            self.state = i32::from(rc);
        }
        self.stop();
        false
    }

    /// Sends DISCONNECT, closes the socket: [`MQTT_DISCONNECTED`] (no last will).
    pub fn disconnect(&mut self) {
        if let Some(s) = self.stream.as_mut() {
            s.write_all(&[DISCONNECT, 0]);
        }
        self.state = MQTT_DISCONNECTED;
        self.stop();
        let now = self.clock.now_ms();
        self.last_in = now;
        self.last_out = now;
    }

    /// Publishes `payload` to `topic` (a C string) with QoS 0; false when not connected, when
    /// 5 + 2 + topic + payload exceeds the buffer, or when the socket write fails.
    pub fn publish(&mut self, topic: &[u8], payload: &[u8], retained: bool) -> bool {
        if !self.connected() {
            return false;
        }
        let topic = c_str(topic);
        let needed = MAX_HEADER_SIZE + 2 + topic.len().min(self.buffer.len()) + payload.len();
        if self.buffer.len() < needed {
            return false;
        }
        let mut len = self.write_string(topic, MAX_HEADER_SIZE);
        for &b in payload {
            self.put(len, b);
            len += 1;
        }
        // `MQTTPUBLISH | 1` for a retained message: bit 0 of 0x30 is clear, so the sum is the OR
        self.write_packet(PUBLISH + u8::from(retained), len - MAX_HEADER_SIZE)
    }

    /// Subscribes to `filter` (a C string) with `qos` 0 or 1, without waiting for the SUBACK;
    /// false for a QoS above 1, a filter that does not fit, when not connected or when the
    /// socket write fails.
    pub fn subscribe(&mut self, filter: &[u8], qos: u8) -> bool {
        let filter = c_str(filter);
        if qos > 1 || self.buffer.len() < 9 + filter.len().min(self.buffer.len()) {
            return false;
        }
        if !self.connected() {
            return false;
        }
        self.next_msg_id = self.next_msg_id.wrapping_add(1).max(1);
        let [hi, lo] = self.next_msg_id.to_be_bytes();
        self.put(MAX_HEADER_SIZE, hi);
        self.put(MAX_HEADER_SIZE + 1, lo);
        let len = self.write_string(filter, MAX_HEADER_SIZE + 2);
        self.put(len, qos);
        self.write_packet(SUBSCRIBE_QOS1, len + 1 - MAX_HEADER_SIZE)
    }

    /// `PubSubClient::loop()`: keepalive (a PINGREQ after `keep_alive` seconds without traffic in
    /// either direction; another such period without the PINGRESP ends the session with
    /// [`MQTT_CONNECTION_TIMEOUT`]), then at most one inbound packet: a PUBLISH goes to
    /// `on_message(topic, payload)` (QoS 1 acknowledged after it), a PINGREQ is answered, a
    /// PINGRESP clears the outstanding ping, everything else is dropped. False when not
    /// connected or the session ended.
    pub fn poll(&mut self, mut on_message: impl FnMut(&[u8], &[u8])) -> bool {
        if !self.connected() {
            return false;
        }
        let t = self.clock.now_ms();
        let period = u32::from(self.keep_alive_s) * 1000;
        if t.wrapping_sub(self.last_in) > period || t.wrapping_sub(self.last_out) > period {
            if self.ping_outstanding {
                self.state = MQTT_CONNECTION_TIMEOUT;
                self.stop();
                return false;
            }
            if let Some(s) = self.stream.as_mut() {
                s.write_all(&[PINGREQ, 0]);
            }
            self.last_out = t;
            self.last_in = t;
            self.ping_outstanding = true;
        }
        if !self.available() {
            return true;
        }
        let (len, llen) = self.read_packet();
        if len == 0 {
            return self.connected();
        }
        self.last_in = t;
        match self.at(0) & 0xF0 {
            PUBLISH => self.deliver(len, llen, t, &mut on_message),
            PINGREQ => {
                if let Some(s) = self.stream.as_mut() {
                    s.write_all(&[PINGRESP, 0]);
                }
            }
            PINGRESP => self.ping_outstanding = false,
            _ => {}
        }
        true
    }

    /// Hands a received PUBLISH of `len` buffer bytes to the callback (and acknowledges QoS 1).
    fn deliver(
        &mut self,
        len: usize,
        llen: usize,
        t: u32,
        on_message: &mut impl FnMut(&[u8], &[u8]),
    ) {
        let tl = usize::from(u16::from_be_bytes([self.at(llen + 1), self.at(llen + 2)]));
        let topic_start = llen + 3;
        let topic_end = topic_start + tl;
        let qos1 = self.at(0) & 0x06 == QOS1;
        let payload_start = topic_end + if qos1 { 2 } else { 0 };
        if payload_start > len {
            return; // the topic length runs past the packet
        }
        let msg_id = u16::from_be_bytes([self.at(topic_end), self.at(topic_end + 1)]);
        let (Some(topic), Some(payload)) = (
            self.buffer.get(topic_start..topic_end),
            self.buffer.get(payload_start..len),
        ) else {
            return;
        };
        on_message(c_str(topic), payload);
        if qos1 {
            let [hi, lo] = msg_id.to_be_bytes();
            if let Some(s) = self.stream.as_mut() {
                s.write_all(&[PUBACK, 2, hi, lo]);
            }
            self.last_out = t;
        }
    }
}

#[cfg(test)]
mod tests;
