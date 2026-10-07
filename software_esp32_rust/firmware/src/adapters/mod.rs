//! The port adapters of the firmware (docs/rust/GLUE-DESIGN-ESP.md 1.2): one type per port of
//! `vdm_esp_glue::port`, each method one ESP-IDF call or a few in a fixed order, no decision
//! beyond mapping an error to the port's result. Everything with logic lives in the glue, where
//! it is host-tested and mutation-gated; this crate is neither.

mod eth;
mod fs;
mod gpio;
mod http;
mod nvs;
mod ota;
mod ping;
mod sntp;
mod system;
mod tcp;
mod time;
mod watchdog;
mod wifi;

pub use eth::EthPort;
pub use fs::LittleFs;
pub use gpio::{InPin, OutPin, StmUart};
pub use http::{HttpServerPort, Serve};
pub use nvs::EspNvs;
pub use ota::{EspOta, RomMd5};
pub use ping::PingPort;
pub use sntp::SntpPort;
pub use system::{AlwaysGrant, EspSystem, RtcBlock, Stdout};
pub use tcp::{Tcp, UdpPort};
pub use time::{EspClock, EspWall};
pub use watchdog::{arm_boot_deadline, EspSpawner, TaskWatchdog};
pub use wifi::WifiPort;

use vdm_esp_glue::port::Platform;

/// The port types of the firmware: the one instantiation of the glue's [`Platform`] besides the
/// test fakes.
pub struct Fw;

impl Platform for Fw {
    type Clock = EspClock;
    type WallClock = EspWall;
    type Watchdog = TaskWatchdog;
    type Console = Stdout;
    type Uart = StmUart;
    type OutputPin = OutPin;
    type InputPin = InPin;
    type Nvs = EspNvs;
    type Fs = LittleFs;
    type Tcp = Tcp;
    type Udp = UdpPort;
    type HttpServer = HttpServerPort;
    type Ota = EspOta;
    type Md5 = RomMd5;
    type System = EspSystem;
    type HeapGate = AlwaysGrant;
    type Ethernet = EthPort;
    type Wifi = WifiPort;
    type Sntp = SntpPort;
    type Pinger = PingPort;
    type Rtc = RtcBlock;
}

/// IPv4 address of the glue (first octet in the low byte, lwIP order) as an `Ipv4Addr`.
pub fn ipv4(ip: u32) -> core::net::Ipv4Addr {
    core::net::Ipv4Addr::from(ip.to_le_bytes())
}

/// A NUL-terminated copy of `s` in `buf`; `None` when it does not fit or holds a NUL.
pub fn c_name<'b>(s: &str, buf: &'b mut [u8]) -> Option<&'b core::ffi::CStr> {
    let bytes = s.as_bytes();
    let n = bytes.len();
    buf.get_mut(..n)?.copy_from_slice(bytes);
    *buf.get_mut(n)? = 0;
    core::ffi::CStr::from_bytes_with_nul(buf.get(..=n)?).ok()
}
