//! esp_http_server (GLUE-DESIGN-ESP.md 4.1): `EspHttpServer` starts and owns the server, one raw
//! catch-all handler (`/*`, any method) wraps each `httpd_req_t` into the `HttpRequest` port and
//! hands it to the web server. Responses go out as AsyncWebServer framed them
//! (`http_parse::response_head`) through `httpd_send`: `httpd_resp_send` always writes a
//! Content-Type line (204 and 304 have none) and sends every header in pieces. A request the web
//! server leaves unanswered (client gone or stalled) closes its connection (ESP_FAIL).

use core::ffi::{c_void, CStr};

use esp_idf_svc::hal::cpu::Core;
use esp_idf_svc::handle::RawHandle;
use esp_idf_svc::http::server::{Configuration, EspHttpServer};
use esp_idf_svc::sys;
use vdm_esp_glue::http_parse::{chunk_size_line, response_head, CHUNK_LINE_MAX, CRLF, LAST_CHUNK};
use vdm_esp_glue::port::{BodyRead, HttpMethod, HttpRequest, HttpServer};

/// `HTTP_ANY` of esp_http_server.h (`INT_MAX`, not in the bindings).
const HTTP_ANY: sys::httpd_method_t = i32::MAX as sys::httpd_method_t;
/// Room for the head of a response (the status line, Content-Length and -Type, at most four
/// headers of the web server).
const HEAD_MAX: usize = 512;
/// A header value is read as its first bytes (the web server reads at most 256 of any header).
const HEADER_VALUE_MAX: usize = 256;

/// What serves a request: the web server behind its lock (only the HTTP thread takes it).
pub trait Serve: Sync {
    fn serve(&self, req: &mut dyn HttpRequest);
}

/// The HTTP server port: started by the app thread once the network has an address.
pub struct HttpServerPort {
    serve: &'static &'static dyn Serve,
    server: Option<EspHttpServer<'static>>,
}

// SAFETY: the server is started and kept by the app thread only (never stopped); the handler
// context is a shared reference to a Sync value.
unsafe impl Send for HttpServerPort {}

impl HttpServerPort {
    pub fn new(serve: &'static dyn Serve) -> Self {
        HttpServerPort {
            serve: Box::leak(Box::new(serve)),
            server: None,
        }
    }
}

impl HttpServer for HttpServerPort {
    fn start(&mut self) -> bool {
        if self.server.is_some() {
            return true;
        }
        // A client that ends its connection with a reset is no fault (AsyncTCP printed nothing):
        // no console warning per connection.
        // SAFETY: plain setter with a static tag.
        unsafe {
            sys::esp_log_level_set(c"httpd_txrx".as_ptr(), sys::esp_log_level_t_ESP_LOG_ERROR)
        };
        let conf = Configuration {
            http_port: 80,
            core: Some(Core::Core0),
            stack_size: vdm_esp_glue::app::HTTPD_STACK_BYTES as usize,
            max_open_sockets: vdm_esp_glue::web_server::MAX_CONNECTIONS,
            max_uri_handlers: 1,
            max_resp_headers: 8,
            lru_purge_enable: true,
            uri_match_wildcard: true,
            keep_alive: None,
            ..Default::default()
        };
        let Ok(server) = EspHttpServer::new(&conf) else {
            return false;
        };
        let uri = sys::httpd_uri_t {
            uri: c"/*".as_ptr(),
            method: HTTP_ANY,
            handler: Some(handle_request),
            user_ctx: self.serve as *const &'static dyn Serve as *mut c_void,
        };
        // SAFETY: a running server; the context lives forever. A refused registration drops
        // (stops) the server: the next start tries again.
        if unsafe { sys::httpd_register_uri_handler(server.handle(), &uri) } != sys::ESP_OK {
            return false;
        }
        self.server = Some(server);
        true
    }
}

/// The handler of every request (the httpd task).
unsafe extern "C" fn handle_request(req: *mut sys::httpd_req_t) -> sys::esp_err_t {
    // SAFETY: esp_http_server passes a valid request for the duration of the call, with the
    // user context of the registration.
    let (serve, mut r) = unsafe {
        let serve = &*((*req).user_ctx as *const &'static dyn Serve);
        (serve, EspRequest::new(req))
    };
    serve.serve(&mut r);
    if r.answered && !r.failed {
        sys::ESP_OK
    } else {
        sys::ESP_FAIL
    }
}

/// One request: the raw `httpd_req_t` for the duration of the handler.
struct EspRequest {
    req: *mut sys::httpd_req_t,
    /// Body bytes not read yet.
    remaining: usize,
    /// HEAD: the head of the answer without its body.
    head_only: bool,
    answered: bool,
    /// A send failed: the connection is closed after the handler.
    failed: bool,
}

impl EspRequest {
    /// # Safety
    /// `req` must be valid for the life of the value.
    unsafe fn new(req: *mut sys::httpd_req_t) -> Self {
        // SAFETY: valid request (caller).
        let (len, method) = unsafe { ((*req).content_len, (*req).method) };
        EspRequest {
            req,
            remaining: len,
            head_only: method == sys::http_method_HTTP_HEAD as i32,
            answered: false,
            failed: false,
        }
    }

    /// The socket of the request.
    fn fd(&self) -> i32 {
        // SAFETY: valid request.
        unsafe { sys::httpd_req_to_sockfd(self.req) }
    }

    /// One end of the connection (`lwip_getpeername` / `lwip_getsockname`), 0 when unknown.
    fn addr(&self, peer: bool) -> u32 {
        let mut a = sys::sockaddr_in::default();
        let mut len = core::mem::size_of::<sys::sockaddr_in>() as sys::socklen_t;
        let p = &mut a as *mut sys::sockaddr_in as *mut sys::sockaddr;
        // SAFETY: an IPv4 socket (CONFIG_LWIP_IPV6=n) and a buffer of its address size.
        let r = unsafe {
            if peer {
                sys::lwip_getpeername(self.fd(), p, &mut len)
            } else {
                sys::lwip_getsockname(self.fd(), p, &mut len)
            }
        };
        // s_addr is in network order: its first octet in the low byte, as the glue wants
        if r == 0 {
            a.sin_addr.s_addr
        } else {
            0
        }
    }

    /// Every byte of `data` (`httpd_send` sends what the socket takes).
    fn send(&mut self, mut data: &[u8]) -> bool {
        while !data.is_empty() && !self.failed {
            // SAFETY: valid request and buffer.
            let n = unsafe { sys::httpd_send(self.req, data.as_ptr().cast(), data.len()) };
            match usize::try_from(n) {
                Ok(n) if n > 0 => data = data.get(n..).unwrap_or_default(),
                _ => self.failed = true,
            }
        }
        !self.failed
    }

    /// The head of an answer (`length` None: chunked).
    fn send_head(
        &mut self,
        status: u16,
        content_type: &str,
        length: Option<usize>,
        headers: &[(&str, &str)],
    ) -> bool {
        self.answered = true;
        let mut head = [0u8; HEAD_MAX];
        match response_head(status, content_type, length, headers, &mut head) {
            Some(n) => self.send(head.get(..n).unwrap_or_default()),
            None => {
                self.failed = true;
                false
            }
        }
    }
}

impl HttpRequest for EspRequest {
    fn method(&self) -> HttpMethod {
        // SAFETY: valid request.
        let m = unsafe { (*self.req).method } as sys::http_method;
        match m {
            sys::http_method_HTTP_GET => HttpMethod::Get,
            sys::http_method_HTTP_POST => HttpMethod::Post,
            sys::http_method_HTTP_DELETE => HttpMethod::Delete,
            _ => HttpMethod::Other,
        }
    }
    fn target(&self) -> &[u8] {
        // SAFETY: the URI of the request is a NUL-terminated array inside it.
        unsafe { CStr::from_ptr((*self.req).uri.as_ptr()) }.to_bytes()
    }
    fn header(&self, name: &str, out: &mut [u8]) -> Option<usize> {
        let mut nb = [0u8; 32];
        let field = super::c_name(name, &mut nb)?;
        let mut v = [0u8; HEADER_VALUE_MAX + 1];
        // SAFETY: valid request, NUL-terminated field, buffer and its size. The value is
        // truncated to the buffer (ESP_ERR_HTTPD_RESULT_TRUNC); absent: ESP_ERR_NOT_FOUND.
        let (len, err) = unsafe {
            let len = sys::httpd_req_get_hdr_value_len(self.req, field.as_ptr());
            let err = sys::httpd_req_get_hdr_value_str(
                self.req,
                field.as_ptr(),
                v.as_mut_ptr().cast(),
                v.len(),
            );
            (len, err)
        };
        if err != sys::ESP_OK && err != sys::ESP_ERR_HTTPD_RESULT_TRUNC {
            return None;
        }
        let n = len.min(out.len()).min(HEADER_VALUE_MAX);
        out.get_mut(..n)?.copy_from_slice(v.get(..n)?);
        Some(len)
    }
    fn content_length(&self) -> usize {
        // SAFETY: valid request.
        unsafe { (*self.req).content_len }
    }
    fn remote_ip(&self) -> u32 {
        self.addr(true)
    }
    fn local_ip(&self) -> u32 {
        self.addr(false)
    }
    fn read_body(&mut self, out: &mut [u8]) -> BodyRead {
        if self.remaining == 0 {
            return BodyRead::End;
        }
        if out.is_empty() {
            return BodyRead::Data(0);
        }
        let want = out.len().min(self.remaining);
        // SAFETY: valid request and buffer of `want` bytes.
        let n = unsafe { sys::httpd_req_recv(self.req, out.as_mut_ptr().cast(), want) };
        match n {
            n if n > 0 => {
                self.remaining -= n as usize;
                BodyRead::Data(n as usize)
            }
            sys::HTTPD_SOCK_ERR_TIMEOUT => BodyRead::Timeout,
            _ => BodyRead::Closed,
        }
    }
    fn respond(
        &mut self,
        status: u16,
        content_type: &str,
        headers: &[(&str, &str)],
        body: &[u8],
    ) -> bool {
        self.send_head(status, content_type, Some(body.len()), headers)
            && (self.head_only || self.send(body))
    }
    fn begin_chunked(&mut self, status: u16, content_type: &str, headers: &[(&str, &str)]) -> bool {
        self.send_head(status, content_type, None, headers)
    }
    fn chunk(&mut self, data: &[u8]) -> bool {
        if self.head_only {
            return !self.failed;
        }
        if data.is_empty() {
            return self.send(LAST_CHUNK);
        }
        let mut line = [0u8; CHUNK_LINE_MAX];
        let n = chunk_size_line(data.len(), &mut line);
        self.send(line.get(..n).unwrap_or_default()) && self.send(data) && self.send(CRLF)
    }
}
