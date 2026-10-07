//! Ports: every way the glue reaches time, sleep, files, flash, sockets, the heap and the chip
//! (docs/rust/GLUE-DESIGN-ESP.md section 1). `firmware` implements them with one ESP-IDF call per
//! method; the tests implement them with the fakes of `testkit`.
//!
//! IPv4 addresses are `u32` with the first octet in the low byte (lwIP order, as in core). Byte
//! strings from outside (names, values, bodies, packets) are `&[u8]` and never assumed UTF-8.

pub use vdm_esp_core::common::LocalTime;
pub use vdm_esp_core::json_api::HttpMethod;

// ---------------------------------------------------------------- time, watchdog, console

/// Monotonic time and task delays.
pub trait Clock: Send + Sync {
    /// `millis()`: milliseconds since boot, wrapping at 2^32.
    fn now_ms(&self) -> u32;
    /// Seconds since boot (does not wrap within the life of a device).
    fn uptime_s(&self) -> u32;
    /// Task delay of `ms` milliseconds (rounded up to ticks); 0 yields to tasks of the same
    /// priority.
    fn sleep_ms(&self, ms: u32);
}

/// Wall clock and time zone (newlib `gettimeofday`, `TZ`, `localtime_r`).
pub trait WallClock: Send + Sync {
    /// Seconds since 1970 (`gettimeofday`); counts from 1970-01-01 at boot until SNTP sets it.
    fn epoch(&self) -> i64;
    /// Sets the POSIX TZ rule (`setenv("TZ")` + `tzset()`), e.g. `CET-1CEST,M3.5.0,M10.5.0/3`.
    fn set_time_zone(&self, posix: &str);
    /// Local time of `epoch` under the rule set last (with the weekday); `None` when the
    /// conversion fails.
    fn local_time(&self, epoch: i64) -> Option<LocalTime>;
}

/// Task watchdog (TWDT) subscription of the calling thread.
pub trait Watchdog {
    /// Resets the watchdog of this thread.
    fn feed(&self);
}

/// The serial console (UART0, 115200): the `Serial.println` mirror of the log.
pub trait Console: Send + Sync {
    /// Writes `text` and CR LF.
    fn line(&self, text: &[u8]);
}

// ---------------------------------------------------------------- UART and GPIO

/// `Serial2`, owned by the stm thread.
pub trait Uart: Send {
    /// (Re)opens the port: 8N1, or 8E1 with `even_parity`; RX ring 2048 B, TX ring 512 B; bytes
    /// received before are dropped.
    fn configure(&mut self, baud: u32, even_parity: bool);
    /// Copies received bytes into `out` and returns their count; never blocks.
    fn read(&mut self, out: &mut [u8]) -> usize;
    /// Queues bytes into the TX ring and returns how many fitted; never waits for the wire.
    fn write(&mut self, data: &[u8]) -> usize;
}

/// A push-pull output (NRST on IO15, BOOT0 on IO14).
pub trait OutputPin: Send {
    /// Drives the pin HIGH (`true`) or LOW. The adapter writes the level before it switches the
    /// pin to an output, so the first call never glitches.
    fn set(&mut self, high: bool);
}

/// An input with the pull-up configured by the adapter (the factory pin IO2).
pub trait InputPin: Send {
    /// The pin reads LOW.
    fn is_low(&mut self) -> bool;
}

// ---------------------------------------------------------------- NVS

/// Integer types of NVS entries. A read asks for one type and finds only an entry of exactly
/// that type (no conversion, as `nvs_get_*`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum NvsInt {
    /// `nvs_get_u8`.
    U8 = 0,
    /// `nvs_get_i8`.
    I8 = 1,
    /// `nvs_get_u16`.
    U16 = 2,
    /// `nvs_get_i16`.
    I16 = 3,
    /// `nvs_get_u32`.
    U32 = 4,
    /// `nvs_get_i32`.
    I32 = 5,
    /// `nvs_get_i64`.
    I64 = 6,
}

/// The NVS partition.
pub trait Nvs: Send + Sync {
    /// An open namespace.
    type Ns: NvsNamespace;
    /// Opens `namespace` read-only or for writing (`nvs_open`); a read-only open of a namespace
    /// that does not exist fails, a write open creates it. `None` when the open fails.
    fn open(&self, namespace: &str, write: bool) -> Option<Self::Ns>;
}

/// One open NVS namespace (closed on drop). Every set, remove and erase commits at once, as the
/// Arduino `Preferences` class did; a key has at most 15 characters.
pub trait NvsNamespace {
    /// The integer `key` of exactly the type `kind`; `None` when absent or of another type.
    fn get_int(&self, key: &str, kind: NvsInt) -> Option<i64>;
    /// Stores `v` as type `kind` (truncated to the type) and commits; false on failure (also a
    /// read-only namespace).
    fn set_int(&mut self, key: &str, kind: NvsInt, v: i64) -> bool;
    /// Length of the blob `key`; `None` when absent or not a blob.
    fn blob_len(&self, key: &str) -> Option<usize>;
    /// Copies the blob `key` into `out` and returns its length; `None` when absent, not a blob
    /// or longer than `out`.
    fn get_blob(&self, key: &str, out: &mut [u8]) -> Option<usize>;
    /// Stores the blob and commits; false on failure.
    fn set_blob(&mut self, key: &str, data: &[u8]) -> bool;
    /// Length of the string `key` including its NUL; `None` when absent or not a string.
    fn str_len(&self, key: &str) -> Option<usize>;
    /// Copies the string `key` without its NUL into `out` and returns its length; `None` when
    /// absent, not a string or longer than `out`. The bytes are not necessarily UTF-8.
    fn get_str(&self, key: &str, out: &mut [u8]) -> Option<usize>;
    /// Erases `key` and commits; false when it did not exist or the erase failed.
    fn remove(&mut self, key: &str) -> bool;
    /// Erases every key of the namespace and commits; false on failure.
    fn erase_all(&mut self) -> bool;
}

// ---------------------------------------------------------------- LittleFS

/// How a file is opened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenMode {
    /// Read from the start; the file must exist.
    Read,
    /// Created or truncated; the parent directory must exist.
    Write,
    /// Created if missing, writes go to the end.
    Append,
}

/// One entry of a directory listing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FsEntry<'a> {
    /// Name without the directory (bytes as stored).
    pub name: &'a [u8],
    /// File size in bytes (0 for a directory).
    pub size: u32,
    /// The entry is a directory.
    pub dir: bool,
}

/// The LittleFS partition (`spiffs`, mounted at `/littlefs`). Paths are below the mount point
/// and absolute: `"/stm/x.bin"`.
pub trait Fs: Send + Sync {
    /// An open file.
    type File: FsFile;
    /// Mounts the partition; never formats. False when the mount fails.
    fn mount(&self) -> bool;
    /// Formats the partition (all files gone); false on failure.
    fn format(&self) -> bool;
    /// Opens `path`; `None` when it fails (missing file for `Read`, missing parent, a directory).
    fn open(&self, path: &str, mode: OpenMode) -> Option<Self::File>;
    /// A file or directory `path` exists.
    fn exists(&self, path: &str) -> bool;
    /// Creates the directory `path` (the parent must exist); false when it fails or exists.
    fn mkdir(&self, path: &str) -> bool;
    /// Removes the file or empty directory `path`; false when it fails, also while the file is
    /// open.
    fn remove(&self, path: &str) -> bool;
    /// Renames `from` to `to`, replacing `to`; false when it fails, also while either is open
    /// (EBUSY).
    fn rename(&self, from: &str, to: &str) -> bool;
    /// Calls `visit` for every entry of the directory `dir` in LittleFS order until it returns
    /// false.
    fn list(&self, dir: &str, visit: &mut dyn FnMut(&FsEntry) -> bool);
    /// Total and used bytes of the partition.
    fn usage(&self) -> (u32, u32);
}

/// An open file: closed on drop; unbuffered (no stdio layer: every call reaches LittleFS).
pub trait FsFile {
    /// Reads up to `out.len()` bytes at the current position; 0 at the end or on failure.
    fn read(&mut self, out: &mut [u8]) -> usize;
    /// Writes `data` and returns the bytes written (fewer when the partition is full or the
    /// write fails).
    fn write(&mut self, data: &[u8]) -> usize;
    /// Moves the position to `pos` bytes from the start; false on failure.
    fn seek(&mut self, pos: u32) -> bool;
    /// Size of the file in bytes.
    fn size(&self) -> u32;
}

// ---------------------------------------------------------------- TCP and UDP

/// Result of a non-blocking read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TcpRead {
    /// This many bytes were copied.
    Data(usize),
    /// Nothing received yet; the connection is open.
    Empty,
    /// The connection is closed (by the peer or an error).
    Closed,
}

/// A connected TCP socket (`WiFiClient` of Arduino-ESP32 2.0.7). Closed on drop.
pub trait TcpStream: Send {
    /// Copies received bytes into `out`; never blocks.
    fn read(&mut self, out: &mut [u8]) -> TcpRead;
    /// The socket is still connected (`WiFiClient::connected`: a non-blocking peek); never
    /// blocks.
    fn connected(&mut self) -> bool;
    /// Sends all of `data`; waits at most the write timeout (10 s, `WiFiClient`); false when not
    /// all of it was sent.
    fn write_all(&mut self, data: &[u8]) -> bool;
    /// Closes the socket (`WiFiClient::stop`); later reads report `Closed`.
    fn close(&mut self);
}

/// Opens TCP connections.
pub trait TcpConnector: Send + Sync {
    /// The connection type.
    type Stream: TcpStream;
    /// Resolves `host` (a name or a dotted address) and connects within `timeout_ms`; `None`
    /// when either fails.
    fn connect(&self, host: &str, port: u16, timeout_ms: u32) -> Option<Self::Stream>;
}

/// A UDP socket (bound once; syslog).
pub trait Udp: Send {
    /// Sends one datagram; false when it could not be queued.
    fn send_to(&mut self, ip: u32, port: u16, data: &[u8]) -> bool;
}

// ---------------------------------------------------------------- HTTP

/// Result of a body read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BodyRead {
    /// This many bytes were copied.
    Data(usize),
    /// The whole body (Content-Length) was read.
    End,
    /// 5 s without data (`httpd_req_recv` timeout); the glue may try again.
    Timeout,
    /// The client is gone.
    Closed,
}

/// One HTTP request on the esp_http_server task, passed to the handler as `&mut dyn`.
pub trait HttpRequest {
    /// The method.
    fn method(&self) -> HttpMethod;
    /// The raw request target `"path?query"`, not decoded.
    fn target(&self) -> &[u8];
    /// Copies what fits of the value of header `name` (case-insensitive) into `out` and returns
    /// the full length of the value; `None` when the header is absent.
    fn header(&self, name: &str, out: &mut [u8]) -> Option<usize>;
    /// The Content-Length (0 without a body).
    fn content_length(&self) -> usize;
    /// Address of the client.
    fn remote_ip(&self) -> u32;
    /// Local address of the connection (the interface the request came in on).
    fn local_ip(&self) -> u32;
    /// Reads the next body bytes into `out`.
    fn read_body(&mut self, out: &mut [u8]) -> BodyRead;
    /// Sends the whole response with a Content-Length; false when the client is gone.
    fn respond(
        &mut self,
        status: u16,
        content_type: &str,
        headers: &[(&str, &str)],
        body: &[u8],
    ) -> bool;
    /// Starts a chunked response (Transfer-Encoding: chunked); false when the client is gone.
    fn begin_chunked(&mut self, status: u16, content_type: &str, headers: &[(&str, &str)]) -> bool;
    /// Sends one chunk of a chunked response; an empty chunk ends the response.
    fn chunk(&mut self, data: &[u8]) -> bool;
}

/// The HTTP server (esp_http_server).
pub trait HttpServer: Send {
    /// Starts the server with the one catch-all handler; false on failure (retried later).
    fn start(&mut self) -> bool;
}

// ---------------------------------------------------------------- OTA, MD5

/// An ESP-IDF error code (`esp_err_t`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EspErr(pub i32);

impl EspErr {
    /// `ESP_FAIL`.
    pub const FAIL: EspErr = EspErr(-1);
    /// `ESP_ERR_NO_MEM`.
    pub const NO_MEM: EspErr = EspErr(0x101);
    /// `ESP_ERR_INVALID_ARG`.
    pub const INVALID_ARG: EspErr = EspErr(0x102);
    /// `ESP_ERR_INVALID_STATE`.
    pub const INVALID_STATE: EspErr = EspErr(0x103);
    /// `ESP_ERR_INVALID_SIZE`.
    pub const INVALID_SIZE: EspErr = EspErr(0x104);
    /// `ESP_ERR_NOT_FOUND`.
    pub const NOT_FOUND: EspErr = EspErr(0x105);
    /// `ESP_ERR_OTA_PARTITION_CONFLICT`.
    pub const OTA_PARTITION_CONFLICT: EspErr = EspErr(0x1501);
    /// `ESP_ERR_OTA_VALIDATE_FAILED`: the image does not verify.
    pub const OTA_VALIDATE_FAILED: EspErr = EspErr(0x1503);
    /// `ESP_ERR_FLASH_OP_FAIL`.
    pub const FLASH_OP_FAIL: EspErr = EspErr(0x6001);
}

/// Identity of an app image: the first 8 bytes of a SHA-256 of the image. The firmware spike's
/// guard reads the SHA-256 appended to the image (`esp_partition_get_sha256`), the design named
/// `esp_app_desc_t.app_elf_sha256`; either identifies the same build, the glue only compares.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AppId(pub [u8; 8]);

/// One OTA app slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SlotInfo {
    /// Flash address of the partition.
    pub address: u32,
    /// Partition size in bytes.
    pub size: u32,
    /// The app in the slot (`esp_ota_get_partition_description`); `None` when the slot holds no
    /// readable app description. A description alone does not prove that the image verifies.
    pub app: Option<AppId>,
}

/// The OTA slots and the boot selection (otadata).
pub trait Ota: Send + Sync {
    /// An update in progress.
    type Update: OtaUpdate;
    /// The slot the running image was loaded from.
    fn running(&self) -> SlotInfo;
    /// The other slot: the target of an update and the boot guard's fallback; `None` without a
    /// second app partition.
    fn other(&self) -> Option<SlotInfo>;
    /// Starts an update into `other()` (sequential writes: sectors are erased as written).
    fn begin(&self) -> Result<Self::Update, EspErr>;
    /// Selects the slot at `address` for the next boot; the image is verified first
    /// (`OTA_VALIDATE_FAILED` when it does not).
    fn set_boot(&self, address: u32) -> Result<(), EspErr>;
    /// The image in the slot at `address` validates (`esp_image_verify`: segments, checksum,
    /// SHA-256); reads the whole image, so the boot guard calls it once per boot.
    fn verify(&self, address: u32) -> bool;
    /// Marks the running image valid in otadata (`esp_ota_mark_app_valid_cancel_rollback`): a
    /// bootloader with rollback support may have started it as PENDING_VERIFY; the error when
    /// there is nothing to mark is ignored.
    fn mark_valid(&self);
    /// Size of the running image (`ESP.getSketchSize()`).
    fn running_image_size(&self) -> u32;
}

/// An update in progress (`esp_ota_begin` .. `esp_ota_end`).
pub trait OtaUpdate: Send {
    /// Writes the next bytes (erasing sectors as needed); the first byte of an image must be
    /// 0xE9 (`OTA_VALIDATE_FAILED` otherwise).
    fn write(&mut self, data: &[u8]) -> Result<(), EspErr>;
    /// Verifies the written image (segments, checksum, SHA-256) and selects it for the next boot.
    fn finish(self) -> Result<(), EspErr>;
    /// Abandons the update; the slot keeps what was written.
    fn abort(self);
}

/// MD5 (the ROM implementation).
pub trait Md5: Send {
    /// Starts a new digest.
    fn reset(&mut self);
    /// Adds bytes.
    fn update(&mut self, data: &[u8]);
    /// The digest of the bytes added since the last reset.
    fn digest(&mut self) -> [u8; 16];
}

// ---------------------------------------------------------------- system, heap, RTC

/// Heap figures of the 8-bit capable heap (`MALLOC_CAP_8BIT`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HeapStats {
    /// Free bytes now.
    pub free: u32,
    /// Lowest free bytes since boot.
    pub min_free: u32,
    /// Largest free block.
    pub largest: u32,
}

/// The chip.
pub trait System: Send + Sync {
    /// Restarts the chip (`esp_restart`); never returns.
    fn restart(&self) -> !;
    /// Reset reason of this boot, the `esp_reset_reason_t` number (event 100 arg1).
    fn reset_reason(&self) -> u8;
    /// Heap figures.
    fn heap(&self) -> HeapStats;
    /// Lowest free stack of the task `task` in bytes; `None` while no such task exists.
    fn stack_min_free(&self, task: &str) -> Option<u32>;
    /// The factory MAC (eFuse, base of the interface MACs).
    fn base_mac(&self) -> [u8; 6];
}

/// Permission for a transient heap block (GLUE-DESIGN-ESP.md 2.4). The firmware always grants;
/// the glue then allocates with `try_reserve_exact`, so a refused grant or a failed reservation
/// gives the C++ out-of-memory outcome.
pub trait HeapGate: Send + Sync {
    /// May the glue allocate `bytes` now.
    fn grant(&self, bytes: usize) -> bool;
}

/// RTC slow memory that survives software, panic and watchdog resets (garbage after power-on).
pub trait Rtc: Send + Sync {
    /// Copies `out.len()` bytes from `offset`.
    fn load(&self, offset: usize, out: &mut [u8]);
    /// Stores `data` at `offset`.
    fn store(&self, offset: usize, data: &[u8]);
}

// ---------------------------------------------------------------- network

/// Address configuration of an interface.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IpInfo {
    /// Own address.
    pub ip: u32,
    /// Netmask.
    pub mask: u32,
    /// Gateway.
    pub gateway: u32,
    /// DNS server.
    pub dns: u32,
}

/// How an interface starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IpSetup<'a> {
    /// DHCP host name (set before the interface starts).
    pub hostname: &'a str,
    /// Static addresses; `None` = DHCP.
    pub fixed: Option<IpInfo>,
}

/// The Ethernet interface (LAN8720).
pub trait Ethernet: Send {
    /// Installs and starts the driver; false: "eth init failed".
    fn begin(&mut self, setup: &IpSetup) -> bool;
    /// Stops the driver, waits 200 ms, starts it; false before the first link or on failure.
    fn restart(&mut self) -> bool;
    /// The link is up (CONNECTED until DISCONNECTED or STOP).
    fn link(&self) -> bool;
    /// An address was assigned since the last CONNECTED.
    fn has_ip(&self) -> bool;
    /// GOT_IP events so far, static configurations included.
    fn got_ip_count(&self) -> u32;
    /// Current address configuration.
    fn info(&self) -> IpInfo;
    /// The interface MAC.
    fn mac(&self) -> [u8; 6];
}

/// The WiFi station.
pub trait Wifi: Send {
    /// Builds and starts the station (nothing in the WiFi NVS, host name before the start);
    /// false on failure.
    fn begin(&mut self, setup: &IpSetup) -> bool;
    /// Connects to `ssid` with `password` (`WiFi.begin()`).
    fn connect(&mut self, ssid: &[u8], password: &[u8]);
    /// Connects again with the last credentials.
    fn reconnect(&mut self);
    /// Stops the station and frees the driver.
    fn stop(&mut self);
    /// The station has an address (GOT_IP until DISCONNECTED, LOST_IP or STOP).
    fn up(&self) -> bool;
    /// GOT_IP events so far.
    fn got_ip_count(&self) -> u32;
    /// True once per disconnect the glue did not request, since the last call (for the one
    /// retry of D§16).
    fn take_unrequested_disconnect(&mut self) -> bool;
    /// Current address configuration.
    fn info(&self) -> IpInfo;
    /// Signal strength of the access point in dBm.
    fn rssi(&self) -> i8;
    /// The station MAC.
    fn mac(&self) -> [u8; 6];
}

/// SNTP client (one server).
pub trait Sntp: Send {
    /// (Re)starts polling `server`; `None` stops SNTP.
    fn configure(&mut self, server: Option<&str>);
    /// Sync notifications so far.
    fn sync_count(&self) -> u32;
    /// Epoch of the last sync (0 before the first).
    fn last_sync_epoch(&self) -> u32;
}

/// One gateway probe = one esp_ping session (1 echo, 32 B, timeout 1 s).
pub trait Pinger: Send {
    /// Creates and starts a session to `ip`; false when it could not be started.
    fn start(&mut self, ip: u32) -> bool;
    /// The session reported a reply or a timeout.
    fn done(&self) -> bool;
    /// Replies of the session.
    fn replies(&self) -> u32;
    /// Deletes the session (its task and socket go within about 1 s).
    fn delete(&mut self);
}

// ---------------------------------------------------------------- platform

/// The port types of one platform: one instantiation in the firmware, one in the tests. The glue
/// is generic over it.
pub trait Platform: 'static {
    /// [`Clock`].
    type Clock: Clock;
    /// [`WallClock`].
    type WallClock: WallClock;
    /// [`Watchdog`].
    type Watchdog: Watchdog;
    /// [`Console`].
    type Console: Console;
    /// [`Uart`].
    type Uart: Uart;
    /// [`OutputPin`].
    type OutputPin: OutputPin;
    /// [`InputPin`].
    type InputPin: InputPin;
    /// [`Nvs`].
    type Nvs: Nvs;
    /// [`Fs`].
    type Fs: Fs;
    /// [`TcpConnector`].
    type Tcp: TcpConnector;
    /// [`Udp`].
    type Udp: Udp;
    /// [`HttpServer`].
    type HttpServer: HttpServer;
    /// [`Ota`].
    type Ota: Ota;
    /// [`Md5`].
    type Md5: Md5;
    /// [`System`].
    type System: System;
    /// [`HeapGate`].
    type HeapGate: HeapGate;
    /// [`Ethernet`].
    type Ethernet: Ethernet;
    /// [`Wifi`].
    type Wifi: Wifi;
    /// [`Sntp`].
    type Sntp: Sntp;
    /// [`Pinger`].
    type Pinger: Pinger;
    /// [`Rtc`].
    type Rtc: Rtc;
}

// ---------------------------------------------------------------- shared references

// A module struct owns its ports; the firmware hands it `&'static` adapters, a test `&` fakes.

impl<T: Clock + ?Sized> Clock for &T {
    fn now_ms(&self) -> u32 {
        (**self).now_ms()
    }
    fn uptime_s(&self) -> u32 {
        (**self).uptime_s()
    }
    fn sleep_ms(&self, ms: u32) {
        (**self).sleep_ms(ms)
    }
}

impl<T: WallClock + ?Sized> WallClock for &T {
    fn epoch(&self) -> i64 {
        (**self).epoch()
    }
    fn set_time_zone(&self, posix: &str) {
        (**self).set_time_zone(posix)
    }
    fn local_time(&self, epoch: i64) -> Option<LocalTime> {
        (**self).local_time(epoch)
    }
}

impl<T: Console + ?Sized> Console for &T {
    fn line(&self, text: &[u8]) {
        (**self).line(text)
    }
}

impl<T: Nvs + ?Sized> Nvs for &T {
    type Ns = T::Ns;
    fn open(&self, namespace: &str, write: bool) -> Option<Self::Ns> {
        (**self).open(namespace, write)
    }
}

impl<T: Fs + ?Sized> Fs for &T {
    type File = T::File;
    fn mount(&self) -> bool {
        (**self).mount()
    }
    fn format(&self) -> bool {
        (**self).format()
    }
    fn open(&self, path: &str, mode: OpenMode) -> Option<Self::File> {
        (**self).open(path, mode)
    }
    fn exists(&self, path: &str) -> bool {
        (**self).exists(path)
    }
    fn mkdir(&self, path: &str) -> bool {
        (**self).mkdir(path)
    }
    fn remove(&self, path: &str) -> bool {
        (**self).remove(path)
    }
    fn rename(&self, from: &str, to: &str) -> bool {
        (**self).rename(from, to)
    }
    fn list(&self, dir: &str, visit: &mut dyn FnMut(&FsEntry) -> bool) {
        (**self).list(dir, visit)
    }
    fn usage(&self) -> (u32, u32) {
        (**self).usage()
    }
}

impl<T: TcpConnector + ?Sized> TcpConnector for &T {
    type Stream = T::Stream;
    fn connect(&self, host: &str, port: u16, timeout_ms: u32) -> Option<Self::Stream> {
        (**self).connect(host, port, timeout_ms)
    }
}

impl<T: Ota + ?Sized> Ota for &T {
    type Update = T::Update;
    fn running(&self) -> SlotInfo {
        (**self).running()
    }
    fn other(&self) -> Option<SlotInfo> {
        (**self).other()
    }
    fn begin(&self) -> Result<Self::Update, EspErr> {
        (**self).begin()
    }
    fn set_boot(&self, address: u32) -> Result<(), EspErr> {
        (**self).set_boot(address)
    }
    fn verify(&self, address: u32) -> bool {
        (**self).verify(address)
    }
    fn mark_valid(&self) {
        (**self).mark_valid()
    }
    fn running_image_size(&self) -> u32 {
        (**self).running_image_size()
    }
}

impl<T: System + ?Sized> System for &T {
    fn restart(&self) -> ! {
        (**self).restart()
    }
    fn reset_reason(&self) -> u8 {
        (**self).reset_reason()
    }
    fn heap(&self) -> HeapStats {
        (**self).heap()
    }
    fn stack_min_free(&self, task: &str) -> Option<u32> {
        (**self).stack_min_free(task)
    }
    fn base_mac(&self) -> [u8; 6] {
        (**self).base_mac()
    }
}

impl<T: HeapGate + ?Sized> HeapGate for &T {
    fn grant(&self, bytes: usize) -> bool {
        (**self).grant(bytes)
    }
}

impl<T: Rtc + ?Sized> Rtc for &T {
    fn load(&self, offset: usize, out: &mut [u8]) {
        (**self).load(offset, out)
    }
    fn store(&self, offset: usize, data: &[u8]) {
        (**self).store(offset, data)
    }
}

impl<T: Md5 + ?Sized> Md5 for &mut T {
    fn reset(&mut self) {
        (**self).reset()
    }
    fn update(&mut self, data: &[u8]) {
        (**self).update(data)
    }
    fn digest(&mut self) -> [u8; 16] {
        (**self).digest()
    }
}
