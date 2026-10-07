//! TCP connections (`WiFiClient` of Arduino-ESP32 2.0.7) and the UDP socket of syslog over the
//! std sockets of lwIP.

use std::io::Write;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpStream, ToSocketAddrs, UdpSocket};
use std::os::fd::AsRawFd;
use std::time::Duration;

use esp_idf_svc::sys;
use vdm_esp_glue::port::{TcpConnector, TcpRead, TcpStream as PortStream, Udp};

/// Opens TCP connections: lwIP DNS, then a connect with a timeout.
pub struct Tcp;

impl TcpConnector for Tcp {
    type Stream = TcpConn;
    fn connect(&self, host: &str, port: u16, timeout_ms: u32) -> Option<TcpConn> {
        let addr = (host, port)
            .to_socket_addrs()
            .ok()?
            .find(SocketAddr::is_ipv4)?;
        let s =
            TcpStream::connect_timeout(&addr, Duration::from_millis(u64::from(timeout_ms))).ok()?;
        // WiFiClient: 10 s write timeout, Nagle off
        let _ = s.set_write_timeout(Some(Duration::from_secs(10)));
        let _ = s.set_nodelay(true);
        Some(TcpConn(Some(s)))
    }
}

/// A connected socket: blocking writes (bounded by the write timeout), non-blocking reads on
/// the raw descriptor. `close` drops the socket.
pub struct TcpConn(Option<TcpStream>);

impl TcpConn {
    /// `lwip_recv` with `flags` (always non-blocking): >0 bytes, 0 closed, <0 errno.
    fn recv(&self, out: &mut [u8], flags: i32) -> Option<isize> {
        let fd = self.0.as_ref()?.as_raw_fd();
        // SAFETY: an open descriptor and a valid buffer.
        Some(unsafe {
            sys::lwip_recv(
                fd,
                out.as_mut_ptr().cast(),
                out.len(),
                flags | sys::MSG_DONTWAIT as i32,
            )
        })
    }
}

fn would_block() -> bool {
    matches!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(e) if e == sys::EWOULDBLOCK as i32 || e == sys::EAGAIN as i32
    )
}

impl PortStream for TcpConn {
    fn read(&mut self, out: &mut [u8]) -> TcpRead {
        match self.recv(out, 0) {
            Some(n) if n > 0 => TcpRead::Data(n as usize),
            Some(n) if n < 0 && would_block() => TcpRead::Empty,
            _ => TcpRead::Closed,
        }
    }
    fn connected(&mut self) -> bool {
        let mut b = [0u8; 1];
        match self.recv(&mut b, sys::MSG_PEEK as i32) {
            Some(n) if n > 0 => true,
            Some(n) if n < 0 => would_block(),
            _ => false,
        }
    }
    fn write_all(&mut self, data: &[u8]) -> bool {
        self.0.as_mut().is_some_and(|s| s.write_all(data).is_ok())
    }
    fn close(&mut self) {
        if let Some(s) = self.0.take() {
            let _ = s.shutdown(std::net::Shutdown::Both);
        }
    }
}

/// The UDP socket of syslog, bound once at boot.
pub struct UdpPort(Option<UdpSocket>);

impl UdpPort {
    pub fn new() -> Self {
        UdpPort(UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0)).ok())
    }
}

impl Udp for UdpPort {
    fn send_to(&mut self, ip: u32, port: u16, data: &[u8]) -> bool {
        let to = SocketAddrV4::new(super::ipv4(ip), port);
        self.0
            .as_ref()
            .is_some_and(|s| s.send_to(data, to).is_ok_and(|n| n == data.len()))
    }
}
