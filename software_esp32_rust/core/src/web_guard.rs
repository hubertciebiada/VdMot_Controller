//! Request guard of the HTTP API: Host, Origin, the X-VdMot marker and the JSON Content-Type,
//! checked in this order for every /api/* request and the legacy aliases before anything is
//! buffered (port of `vdm/web_guard.h`). Hardware-free.

use crate::common::{c_str, elapsed_ms, format_ipv4, parse_ipv4, TextBuf};
use crate::json_api::HttpMethod;

/// At most this many bytes of a refused Host or Origin are echoed in the detail text.
const GUARD_ECHO_MAX: usize = 48;

#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GuardScope {
    #[default]
    Static = 0,
    Api = 1,
    LegacyRead = 2,
    LegacyWrite = 3,
}

impl GuardScope {
    pub fn from_raw(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Static),
            1 => Some(Self::Api),
            2 => Some(Self::LegacyRead),
            3 => Some(Self::LegacyWrite),
            _ => None,
        }
    }
}

/// The verdict numbers are event 213's arg1 (1 host, 2 origin, 3 header, 4 content type).
#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GuardVerdict {
    #[default]
    Allow = 0,
    BadHost = 1,
    BadOrigin = 2,
    MissingHeader = 3,
    BadContentType = 4,
}

impl GuardVerdict {
    pub fn from_raw(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Allow),
            1 => Some(Self::BadHost),
            2 => Some(Self::BadOrigin),
            3 => Some(Self::MissingHeader),
            4 => Some(Self::BadContentType),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GuardRequest<'a> {
    pub method: HttpMethod,
    pub scope: GuardScope,
    /// StmImageUpload / EspOta route
    pub upload: bool,
    /// Content-Length > 0
    pub has_body: bool,
    /// Host header value; an absent header is the empty value (C++ a null pointer)
    pub host: &'a [u8],
    /// None = no Origin header
    pub origin: Option<&'a [u8]>,
    /// X-VdMot value, None = absent
    pub marker: Option<&'a [u8]>,
    /// media type without parameters, None = absent
    pub content_type: Option<&'a [u8]>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HostPolicy<'a> {
    /// connection's local address (legacy uint32 layout)
    pub local_ip: u32,
    /// current interface address
    pub iface_ip: u32,
    /// build_hostname(station); a C string
    pub hostname: &'a [u8],
    /// web.allowedHosts; a C string
    pub allowed: &'a [u8],
}

fn digits_and_dots(s: &[u8]) -> bool {
    s.iter().all(|&c| c.is_ascii_digit() || c == b'.')
}

/// `s` without its leading and trailing spaces (only ' ', as the C++).
fn trim_spaces(mut s: &[u8]) -> &[u8] {
    while let [b' ', rest @ ..] = s {
        s = rest;
    }
    while let [rest @ .., b' '] = s {
        s = rest;
    }
    s
}

/// Entry `s` of web.allowedHosts against the stripped host (`host_ip`: its address when it is
/// one). An entry of digits and dots is an address and matches only an address; an empty one
/// matches nothing.
fn entry_matches(s: &[u8], host: &[u8], host_ip: Option<u32>) -> bool {
    if digits_and_dots(s) {
        return matches!((parse_ipv4(s), host_ip), (Some(ip), Some(h)) if ip == h);
    }
    host_ip.is_none() && s.eq_ignore_ascii_case(host)
}

/// The comma-separated list, entries trimmed of spaces.
fn list_matches(list: &[u8], host: &[u8], host_ip: Option<u32>) -> bool {
    list.split(|&c| c == b',')
        .any(|entry| entry_matches(trim_spaces(entry), host, host_ip))
}

/// `s` without `prefix`, compared ASCII case-insensitively.
fn strip_prefix_no_case<'s>(s: &'s [u8], prefix: &[u8]) -> Option<&'s [u8]> {
    let (head, tail) = s.split_at_checked(prefix.len())?;
    head.eq_ignore_ascii_case(prefix).then_some(tail)
}

/// Host rule: absent/empty ok. One ":<1..5 digits>" suffix and one trailing '.' are stripped;
/// a dotted IPv4 must equal local_ip, iface_ip or an IPv4 entry of `allowed` (address 0 never
/// matches); a name must equal (case-insensitive) hostname, hostname + ".local" (the device
/// announces no mDNS name, but a local DNS may serve it) or a name entry of `allowed`.
/// Anything else (IPv6 literals included) fails. C++ a null host: the empty one.
pub fn host_allowed(host: &[u8], p: &HostPolicy<'_>) -> bool {
    if host.is_empty() {
        return true;
    }
    // one ":<1..5 digits>" suffix
    let mut h = host;
    if let Some(at) = host.iter().position(|&c| c == b':') {
        let (name, port) = host.split_at(at);
        let digits = port.get(1..).unwrap_or_default();
        if digits.is_empty() || digits.len() > 5 || !digits.iter().all(u8::is_ascii_digit) {
            return false;
        }
        h = name;
    }
    let h = h.strip_suffix(b".").unwrap_or(h);
    let allowed = c_str(p.allowed);
    if digits_and_dots(h) {
        // also the empty host: no address
        return match parse_ipv4(h) {
            Some(ip) if ip != 0 => {
                ip == p.local_ip || ip == p.iface_ip || list_matches(allowed, h, Some(ip))
            }
            _ => false,
        };
    }
    let name = c_str(p.hostname);
    if !name.is_empty() {
        if h.eq_ignore_ascii_case(name) {
            return true;
        }
        if let Some((head, tail)) = h.split_at_checked(name.len()) {
            if head.eq_ignore_ascii_case(name) && tail.eq_ignore_ascii_case(b".local") {
                return true;
            }
        }
    }
    list_matches(allowed, h, None)
}

/// Origin rule: "http://" or "https://" (case-insensitive) followed by a non-empty host part
/// without '/' that [`host_allowed`] accepts. "null" fails; no Origin header (None) passes.
pub fn origin_allowed(origin: Option<&[u8]>, p: &HostPolicy<'_>) -> bool {
    let Some(origin) = origin else {
        return true;
    };
    let Some(host) = strip_prefix_no_case(origin, b"http://")
        .or_else(|| strip_prefix_no_case(origin, b"https://"))
    else {
        return false;
    };
    !host.is_empty() && !host.contains(&b'/') && host_allowed(host, p)
}

/// Static: always Allow. Else Host, Origin (when present), X-VdMot == "1" for POST/DELETE in
/// scope Api, Content-Type application/json (case-insensitive) for a POST with a body that is
/// not an upload.
pub fn check_request(r: &GuardRequest<'_>, p: &HostPolicy<'_>) -> GuardVerdict {
    if r.scope == GuardScope::Static {
        return GuardVerdict::Allow;
    }
    if !host_allowed(r.host, p) {
        return GuardVerdict::BadHost;
    }
    if !origin_allowed(r.origin, p) {
        return GuardVerdict::BadOrigin;
    }
    let write = matches!(r.method, HttpMethod::Post | HttpMethod::Delete);
    if r.scope == GuardScope::Api && write && r.marker != Some(b"1".as_slice()) {
        return GuardVerdict::MissingHeader;
    }
    if r.method == HttpMethod::Post
        && r.has_body
        && !r.upload
        && !r
            .content_type
            .is_some_and(|t| t.eq_ignore_ascii_case(b"application/json"))
    {
        return GuardVerdict::BadContentType;
    }
    GuardVerdict::Allow
}

/// "host_not_allowed", "origin_not_allowed", "header_required", "unsupported_media_type";
/// "" for Allow.
pub fn guard_error_code(v: GuardVerdict) -> &'static str {
    match v {
        GuardVerdict::Allow => "",
        GuardVerdict::BadHost => "host_not_allowed",
        GuardVerdict::BadOrigin => "origin_not_allowed",
        GuardVerdict::MissingHeader => "header_required",
        GuardVerdict::BadContentType => "unsupported_media_type",
    }
}

/// 403 / 415, 200 for Allow.
pub fn guard_http_status(v: GuardVerdict) -> u16 {
    match v {
        GuardVerdict::Allow => 200,
        GuardVerdict::BadHost | GuardVerdict::BadOrigin | GuardVerdict::MissingHeader => 403,
        GuardVerdict::BadContentType => 415,
    }
}

/// Detail text of a refusal: BadHost "<host, max 48>: use <local IP> or add the name to
/// web.allowedHosts" (local IP = local_ip, else iface_ip); BadOrigin "<origin, max 48>";
/// MissingHeader "X-VdMot: 1"; BadContentType "application/json required"; Allow "". The echo
/// ends at a NUL in the host or origin (C `%.*s`). Returns the length; a text that does not fit
/// is cut to `out.len() - 1` bytes (C++ snprintf).
pub fn guard_detail(
    v: GuardVerdict,
    r: &GuardRequest<'_>,
    p: &HostPolicy<'_>,
    out: &mut [u8],
) -> usize {
    let echo = if v == GuardVerdict::BadHost {
        r.host
    } else {
        r.origin.unwrap_or_default()
    };
    let echo = c_str(echo.get(..GUARD_ECHO_MAX).unwrap_or(echo));
    let mut w = TextBuf::new(out);
    match v {
        GuardVerdict::Allow => {}
        GuardVerdict::BadHost => {
            let mut ip = [0u8; 16];
            let n = format_ipv4(
                if p.local_ip != 0 {
                    p.local_ip
                } else {
                    p.iface_ip
                },
                &mut ip,
            );
            w.push_bytes(echo);
            w.push_bytes(b": use ");
            w.push_bytes(ip.get(..n).unwrap_or_default());
            w.push_bytes(b" or add the name to web.allowedHosts");
        }
        GuardVerdict::BadOrigin => {
            w.push_bytes(echo);
        }
        GuardVerdict::MissingHeader => {
            w.push_bytes(b"X-VdMot: 1");
        }
        GuardVerdict::BadContentType => {
            w.push_bytes(b"application/json required");
        }
    }
    w.len()
}

/// At most one "yes" per key and interval (event throttling).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RepeatLimiter {
    interval_ms: u32,
    seen: [bool; RepeatLimiter::KEYS as usize],
    last_ms: [u32; RepeatLimiter::KEYS as usize],
}

impl RepeatLimiter {
    pub const KEYS: u8 = 8;

    /// C++ default for interval_ms: 60000 ([`Default`]).
    pub fn new(interval_ms: u32) -> Self {
        Self {
            interval_ms,
            seen: [false; Self::KEYS as usize],
            last_ms: [0; Self::KEYS as usize],
        }
    }

    /// First call per key true, then true again once interval_ms passed since the last true;
    /// key >= KEYS false.
    pub fn allow(&mut self, key: u8, now_ms: u32) -> bool {
        let k = usize::from(key);
        let (Some(seen), Some(last)) = (self.seen.get_mut(k), self.last_ms.get_mut(k)) else {
            return false;
        };
        if *seen && elapsed_ms(now_ms, *last) < self.interval_ms {
            return false;
        }
        *seen = true;
        *last = now_ms;
        true
    }
}

impl Default for RepeatLimiter {
    fn default() -> Self {
        Self::new(60_000)
    }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_mut;
