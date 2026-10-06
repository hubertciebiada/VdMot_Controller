//! Fake HTTP: a request driver for the handlers (the esp_http_server side of one request) and
//! the server start.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use super::lock;
use crate::port::{BodyRead, HttpMethod, HttpRequest, HttpServer};

/// The answer a handler gave.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Response {
    /// `respond` and `begin_chunked` calls: exactly one is the invariant.
    pub(crate) answers: u32,
    pub(crate) status: u16,
    pub(crate) content_type: String,
    pub(crate) headers: Vec<(String, String)>,
    pub(crate) body: Vec<u8>,
    pub(crate) chunked: bool,
    /// Non-empty chunks sent.
    pub(crate) chunks: u32,
    /// A chunked response was ended (empty chunk).
    pub(crate) ended: bool,
}

impl Response {
    /// Value of a response header ("" when absent).
    pub(crate) fn header(&self, name: &str) -> &str {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map_or("", |(_, v)| v.as_str())
    }
}

/// One request: method, target, headers, the body in segments, addresses; records the answer
/// and, when dropped, asserts that it was answered exactly once (or not at all after the client
/// went away).
pub(crate) struct FakeRequest {
    pub(crate) method: HttpMethod,
    pub(crate) target: Vec<u8>,
    pub(crate) headers: Vec<(String, Vec<u8>)>,
    pub(crate) body: Vec<u8>,
    /// Content-Length; `None` = the length of `body`.
    pub(crate) content_length: Option<usize>,
    /// At most this many bytes per `read_body` (a TCP segment).
    pub(crate) segment: usize,
    pub(crate) remote_ip: u32,
    pub(crate) local_ip: u32,
    /// `read_body` reports `Timeout` once each time the position is one of these.
    pub(crate) stalls: VecDeque<usize>,
    /// The client goes away once this many body bytes were read.
    pub(crate) gone_at: Option<usize>,
    /// The client is gone (a read reported `Closed`).
    pub(crate) client_gone: bool,
    /// Skip the answer check at drop (a test that looks at a broken handler on purpose).
    pub(crate) unchecked: bool,
    pos: usize,
    pub(crate) response: Response,
}

impl FakeRequest {
    pub(crate) fn new(method: HttpMethod, target: &str) -> Self {
        FakeRequest {
            method,
            target: target.as_bytes().to_vec(),
            headers: vec![("Host".to_string(), b"vdmot.local".to_vec())],
            body: Vec::new(),
            content_length: None,
            segment: 1460,
            remote_ip: 0x3201_A8C0, // 192.168.1.50
            local_ip: 0x0701_A8C0,  // 192.168.1.7
            stalls: VecDeque::new(),
            gone_at: None,
            client_gone: false,
            unchecked: false,
            pos: 0,
            response: Response::default(),
        }
    }
    pub(crate) fn get(target: &str) -> Self {
        Self::new(HttpMethod::Get, target)
    }
    pub(crate) fn post(target: &str, body: &[u8], content_type: &str) -> Self {
        let mut r = Self::new(HttpMethod::Post, target);
        r.body = body.to_vec();
        r.with_header("Content-Type", content_type)
    }
    /// Adds a header.
    pub(crate) fn with_header(mut self, name: &str, value: &str) -> Self {
        self.headers
            .push((name.to_string(), value.as_bytes().to_vec()));
        self
    }
    /// Body bytes read so far.
    pub(crate) fn body_read(&self) -> usize {
        self.pos
    }
}

impl Drop for FakeRequest {
    fn drop(&mut self) {
        if std::thread::panicking() || self.unchecked {
            return;
        }
        let ok = self.response.answers == 1 || (self.response.answers == 0 && self.client_gone);
        assert!(
            ok,
            "HTTP request {} not answered exactly once: {} answers",
            String::from_utf8_lossy(&self.target),
            self.response.answers
        );
    }
}

impl HttpRequest for FakeRequest {
    fn method(&self) -> HttpMethod {
        self.method
    }
    fn target(&self) -> &[u8] {
        &self.target
    }
    fn header(&self, name: &str, out: &mut [u8]) -> Option<usize> {
        let (_, v) = self
            .headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))?;
        let n = v.len().min(out.len());
        out[..n].copy_from_slice(&v[..n]);
        Some(v.len())
    }
    fn content_length(&self) -> usize {
        self.content_length.unwrap_or(self.body.len())
    }
    fn remote_ip(&self) -> u32 {
        self.remote_ip
    }
    fn local_ip(&self) -> u32 {
        self.local_ip
    }
    fn read_body(&mut self, out: &mut [u8]) -> BodyRead {
        if self.client_gone || self.gone_at.is_some_and(|g| self.pos >= g) {
            self.client_gone = true;
            return BodyRead::Closed;
        }
        if self.stalls.front() == Some(&self.pos) {
            self.stalls.pop_front();
            return BodyRead::Timeout;
        }
        let total = self.content_length();
        if self.pos >= total {
            return BodyRead::End;
        }
        let mut n = out.len().min(self.segment).min(total - self.pos);
        if let Some(g) = self.gone_at {
            n = n.min(g - self.pos);
        }
        let src = self.body.get(self.pos..).unwrap_or(&[]);
        let k = n.min(src.len());
        out[..k].copy_from_slice(&src[..k]);
        if k == 0 {
            // a Content-Length beyond the bytes the client sends: it stalls
            return BodyRead::Timeout;
        }
        self.pos += k;
        BodyRead::Data(k)
    }
    fn respond(
        &mut self,
        status: u16,
        content_type: &str,
        headers: &[(&str, &str)],
        body: &[u8],
    ) -> bool {
        let r = &mut self.response;
        r.answers += 1;
        r.status = status;
        r.content_type = content_type.to_string();
        r.headers = headers
            .iter()
            .map(|(n, v)| (n.to_string(), v.to_string()))
            .collect();
        r.body = body.to_vec();
        !self.client_gone
    }
    fn begin_chunked(&mut self, status: u16, content_type: &str, headers: &[(&str, &str)]) -> bool {
        let gone = self.client_gone;
        let r = &mut self.response;
        r.answers += 1;
        r.status = status;
        r.chunked = true;
        r.content_type = content_type.to_string();
        r.headers = headers
            .iter()
            .map(|(n, v)| (n.to_string(), v.to_string()))
            .collect();
        !gone
    }
    fn chunk(&mut self, data: &[u8]) -> bool {
        let r = &mut self.response;
        if data.is_empty() {
            r.ended = true;
        } else {
            r.chunks += 1;
            r.body.extend_from_slice(data);
        }
        !self.client_gone
    }
}

#[derive(Default)]
struct ServerState {
    refuse: bool,
    starts: u32,
}

/// The HTTP server of a boot.
#[derive(Clone, Default)]
pub(crate) struct FakeHttpServer(Arc<Mutex<ServerState>>);

impl FakeHttpServer {
    pub(crate) fn starts(&self) -> u32 {
        lock(&self.0).starts
    }
    pub(crate) fn set_refuse(&self, refuse: bool) {
        lock(&self.0).refuse = refuse;
    }
}

impl HttpServer for FakeHttpServer {
    fn start(&mut self) -> bool {
        let mut s = lock(&self.0);
        s.starts += 1;
        !s.refuse
    }
}
