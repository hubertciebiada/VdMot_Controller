# VdMot Revamped in Rust: ESP32 glue design

Binding for `software_esp32_rust/glue` (crate `vdm-esp-glue`) and `software_esp32_rust/firmware`
(crate `vdm-esp-fw`). It maps the C++ glue of 2.1.7 (`software_esp32_revamped/src/*.cpp`) onto
ports, threads and ESP-IDF. `docs/rust/PORTING.md` applies; D§n is section n of
`software_esp32_revamped/DESIGN.md`, which stays binding. Every behaviour that changes is listed in
§4.7 and §7.

Fixed inputs: esp-idf-svc 0.53, esp-idf-hal 0.47, esp-idf-sys 0.38.1, ESP-IDF 5.5.5 (std);
esp_http_server instead of AsyncTCP/AsyncWebServer; an own MQTT 3.1.1 client with the PubSubClient
semantics of `mqtt_client.cpp` over a TCP port; LittleFS (joltwallet component) at `/littlefs`; NVS
and OTA through ESP-IDF; the boot guard of §6 instead of PENDING_VERIFY (D§16).

| Rule | Consequence |
|---|---|
| Every decision lives in `glue` (std, host-tested, mutation-gated) | `firmware` holds the port adapters (one ESP-IDF call per method, no branch beyond error mapping) and `main`; it is neither host-tested nor mutated |
| No global mutable state in `glue` | every file-static of the C++ glue becomes a field of a module struct; `firmware/src/main.rs` owns the instances; a test builds its own device, so `cargo test` runs cases in parallel threads (no fork runner) |
| Time, sleeping, files, sockets, flash, heap only through ports (§1) | `glue` uses no `std::time`, `std::thread::sleep`, `std::fs`, `std::net` |
| No panic on any input (PORTING.md) | firmware builds with `panic = "abort"`: a panic is a reset with reason PANIC |
| No infallible allocation after boot | §2.4 |
| One glue module per C++ glue file, same name | `web_server.cpp` → `glue/src/web_server.rs` (+ submodules); logic that came from an Arduino library gets a module of its own (§3) |

Contents: 1 Ports · 2 Threads, shared data, memory · 3 Module map · 4 HTTP · 5 Tests and
mutation gate · 6 Boot guard · 7 Open risks and decisions

---

## 1. Ports

### 1.1 Traits (`vdm_esp_glue::port`)

IPv4 addresses are `u32` with the first octet in the low byte (lwIP order, as in core). Byte
strings from outside are `&[u8]`.

```rust
pub trait Clock: Send + Sync {
    fn now_ms(&self) -> u32;                       // millis(): wraps at 2^32
    fn uptime_s(&self) -> u32;
    fn sleep_ms(&self, ms: u32);                   // task delay, 0 = yield
}
pub trait WallClock: Send + Sync {
    fn epoch(&self) -> i64;                        // gettimeofday(); 1970-based before SNTP
    fn set_time_zone(&self, posix: &str);
    fn local_time(&self, epoch: i64) -> Option<LocalTime>;   // core LocalTime (with wday)
}
pub trait Watchdog { fn feed(&self); }             // TWDT subscription of the calling thread
pub trait Console: Send + Sync { fn line(&self, text: &[u8]); }   // Serial.println mirror

pub trait Uart: Send {                             // Serial2, owned by the stm thread
    fn configure(&mut self, baud: u32, even_parity: bool);  // 8N1/8E1, RX ring 2048, TX ring 512, RX cleared
    fn read(&mut self, out: &mut [u8]) -> usize;   // never blocks
    fn write(&mut self, data: &[u8]) -> usize;     // into the TX ring, never waits beyond it
}
pub trait OutputPin: Send { fn set(&mut self, high: bool); }
pub trait InputPin: Send { fn is_low(&mut self) -> bool; }   // pull-up configured by the adapter

pub trait Nvs: Send + Sync {
    type Ns: NvsNamespace;
    fn open(&self, namespace: &str, write: bool) -> Option<Self::Ns>;
}
pub trait NvsNamespace {                           // every set/remove/erase commits (Preferences)
    fn get_int(&self, key: &str, kind: NvsInt) -> Option<i64>;  // U8 I8 U16 I16 U32 I32 I64, no conversion
    fn set_int(&mut self, key: &str, kind: NvsInt, v: i64) -> bool;
    fn blob_len(&self, key: &str) -> Option<usize>;
    fn get_blob(&self, key: &str, out: &mut [u8]) -> Option<usize>;   // None: absent or larger than out
    fn set_blob(&mut self, key: &str, data: &[u8]) -> bool;
    fn str_len(&self, key: &str) -> Option<usize>;                    // with the NUL
    fn get_str(&self, key: &str, out: &mut [u8]) -> Option<usize>;
    fn remove(&mut self, key: &str) -> bool;
    fn erase_all(&mut self) -> bool;
}

pub trait Fs: Send + Sync {                        // paths below the mount point: "/stm/x.bin"
    type File: FsFile;
    fn mount(&self) -> bool;                       // never formats
    fn format(&self) -> bool;
    fn open(&self, path: &str, mode: OpenMode) -> Option<Self::File>;  // Read | Write (truncate) | Append
    fn exists(&self, path: &str) -> bool;
    fn mkdir(&self, path: &str) -> bool;
    fn remove(&self, path: &str) -> bool;          // false while open
    fn rename(&self, from: &str, to: &str) -> bool;   // replaces `to`; false while either is open (EBUSY)
    fn list(&self, dir: &str, visit: &mut dyn FnMut(&FsEntry) -> bool);  // name, size, dir; LittleFS order
    fn usage(&self) -> (u32, u32);                 // total, used bytes
}
pub trait FsFile {                                 // closed on drop; unbuffered (no stdio layer)
    fn read(&mut self, out: &mut [u8]) -> usize;
    fn write(&mut self, data: &[u8]) -> usize;
    fn seek(&mut self, pos: u32) -> bool;
    fn size(&self) -> u32;
}

pub enum TcpRead { Data(usize), Empty, Closed }
pub trait TcpStream: Send {
    fn read(&mut self, out: &mut [u8]) -> TcpRead; // never blocks
    fn connected(&mut self) -> bool;               // peek, never blocks (WiFiClient::connected)
    fn write_all(&mut self, data: &[u8]) -> bool;  // waits at most the write timeout (10 s, WiFiClient)
    fn close(&mut self);
}
pub trait TcpConnector: Send + Sync {
    type Stream: TcpStream;
    fn connect(&self, host: &str, port: u16, timeout_ms: u32) -> Option<Self::Stream>;  // DNS + connect
}
pub trait Udp: Send { fn send_to(&mut self, ip: u32, port: u16, data: &[u8]) -> bool; }

pub enum BodyRead { Data(usize), End, Timeout, Closed }  // Timeout: 5 s without data (the glue retries)
pub trait HttpRequest {                            // one request, on the esp_http_server task
    fn method(&self) -> HttpMethod;                // core enum: Get, Post, Delete, Other (HEAD: no body sent)
    fn target(&self) -> &[u8];                     // raw "path?query", not decoded
    fn header(&self, name: &str, out: &mut [u8]) -> Option<usize>;   // full length; out gets what fits
    fn content_length(&self) -> usize;
    fn remote_ip(&self) -> u32;
    fn local_ip(&self) -> u32;
    fn read_body(&mut self, out: &mut [u8]) -> BodyRead;
    fn respond(&mut self, status: u16, content_type: &str, headers: &[(&str, &str)],
               body: &[u8]) -> bool;               // Content-Length response
    fn begin_chunked(&mut self, status: u16, content_type: &str, headers: &[(&str, &str)]) -> bool;
    fn chunk(&mut self, data: &[u8]) -> bool;      // an empty chunk ends the response
}
pub trait HttpServer: Send { fn start(&mut self) -> bool; }   // registers the catch-all handler (§4.1)

pub struct SlotInfo { pub address: u32, pub size: u32, pub app: Option<AppId> }  // AppId: app_elf_sha256[..8]
pub trait Ota: Send + Sync {
    type Update: OtaUpdate;
    fn running(&self) -> SlotInfo;
    fn other(&self) -> Option<SlotInfo>;           // the update slot = the boot guard's fallback
    fn begin(&self) -> Result<Self::Update, EspErr>;          // into `other`, erased sector by sector
    fn set_boot(&self, address: u32) -> Result<(), EspErr>;   // verifies the image before otadata
    fn running_image_size(&self) -> u32;           // ESP.getSketchSize()
}
pub trait OtaUpdate: Send {
    fn write(&mut self, data: &[u8]) -> Result<(), EspErr>;  // first byte must be 0xE9
    fn finish(self) -> Result<(), EspErr>;         // verify (checksum, SHA-256) + select for boot
    fn abort(self);
}
pub trait Md5: Send { fn reset(&mut self); fn update(&mut self, d: &[u8]); fn digest(&mut self) -> [u8; 16]; }

pub struct HeapStats { pub free: u32, pub min_free: u32, pub largest: u32 }   // MALLOC_CAP_8BIT
pub trait System: Send + Sync {
    fn restart(&self) -> !;
    fn reset_reason(&self) -> u8;                  // esp_reset_reason_t number (event 100 arg1)
    fn heap(&self) -> HeapStats;
    fn stack_min_free(&self, task: &str) -> Option<u32>;      // bytes; None while the task does not exist
    fn base_mac(&self) -> [u8; 6];
}
pub trait HeapGate: Send + Sync { fn grant(&self, bytes: usize) -> bool; }   // §2.4

pub struct IpInfo { pub ip: u32, pub mask: u32, pub gateway: u32, pub dns: u32 }
pub struct IpSetup<'a> { pub hostname: &'a str, pub fixed: Option<IpInfo> }  // None = DHCP
pub trait Ethernet: Send {
    fn begin(&mut self, setup: &IpSetup) -> bool;  // false: "eth init failed"
    fn restart(&mut self) -> bool;                 // stop, 200 ms, start; false before the first link
    fn link(&self) -> bool;                        // CONNECTED .. DISCONNECTED/STOP
    fn has_ip(&self) -> bool;                      // GOT_IP since the last CONNECTED
    fn got_ip_count(&self) -> u32;                 // GOT_IP events, static configurations included
    fn info(&self) -> IpInfo;
    fn mac(&self) -> [u8; 6];
}
pub trait Wifi: Send {
    fn begin(&mut self, setup: &IpSetup) -> bool;  // STA, nothing in the WiFi NVS, host name before start
    fn connect(&mut self, ssid: &[u8], password: &[u8]);     // WiFi.begin()
    fn reconnect(&mut self);
    fn stop(&mut self);
    fn up(&self) -> bool;                          // STA GOT_IP .. DISCONNECTED/LOST_IP/STOP
    fn got_ip_count(&self) -> u32;
    fn take_unrequested_disconnect(&mut self) -> bool;       // for the one retry of D§16
    fn info(&self) -> IpInfo;
    fn rssi(&self) -> i8;
    fn mac(&self) -> [u8; 6];
}
pub trait Sntp: Send {
    fn configure(&mut self, server: Option<&str>); // (re)start polling `server`; None stops SNTP
    fn sync_count(&self) -> u32;                   // sync notifications so far
    fn last_sync_epoch(&self) -> u32;
}
pub trait Pinger: Send {                           // one probe = one esp_ping session (D§16)
    fn start(&mut self, ip: u32) -> bool;          // 1 echo, 32 B, timeout 1 s
    fn done(&self) -> bool;                        // reply or timeout reported
    fn replies(&self) -> u32;
    fn delete(&mut self);
}
pub trait Rtc: Send + Sync {                       // RTC slow memory that survives software resets
    fn load(&self, offset: usize, out: &mut [u8]);
    fn store(&self, offset: usize, data: &[u8]);
}
```

A `Platform` trait bundles the associated port types; the glue is generic over it (one
instantiation in the firmware, one in the tests). Only `HttpRequest` is passed as `&mut dyn`.

### 1.2 Firmware implementation

| Port | `firmware/src/adapters` (esp-idf-svc 0.53 / esp-idf-hal 0.47 / esp-idf-sys 0.38.1) |
|---|---|
| Clock | `esp_idf_sys::esp_timer_get_time()`; `esp_idf_hal::delay::FreeRtos::delay_ms` (rounds up to ticks; `CONFIG_FREERTOS_HZ=1000` as Arduino-ESP32 2.0.7). Never `std::thread::sleep`: ESP-IDF's `usleep` busy-waits below one tick |
| WallClock | `gettimeofday`, `std::env::set_var("TZ", ..)` + `esp_idf_sys::tzset()`, `esp_idf_sys::localtime_r`; one `Mutex` around TZ change and conversion (newlib keeps the TZ state global) |
| Watchdog | `esp_idf_hal::task::watchdog::TWDTDriver::new(p.twdt, ..)` in `main` with `TWDTConfig::new()` (idle task of core 0 from sdkconfig), `duration` 30 s, `panic_on_trigger` (the driver reconfigures the TWDT that ESP-IDF started); per thread, on that thread, `watch_current_task()` of a leaked clone of the driver → `WatchdogSubscription<'static>` in a `RefCell` (the port feeds through `&self`); the subscriptions are never dropped (the threads never end) |
| Console | `std::io::stdout` (UART0 console, 115200) |
| Uart | `esp_idf_hal::uart::UartDriver::new(p.uart2, gpio17, gpio5, None, None, &Config::new().baudrate(Hertz(115_200)).rx_fifo_size(2048).tx_fifo_size(512).queue_size(0))` (no event queue; pins and sizes from `glue::board`); `read(buf, NON_BLOCK)` (`Err(ESP_ERR_TIMEOUT)` = 0 bytes); `write`; `change_baudrate`, `change_parity(ParityEven / ParityNone)`, `clear_rx` (Arduino's `end()`/`begin()` cleared RX) |
| OutputPin, InputPin | IO15/IO14 owned as `AnyOutputPin`, routed to the GPIO matrix once (`esp_rom_gpio_pad_select_gpio`: both boot as JTAG pads) with the output still disabled; every `set` writes `gpio_set_level` and then `gpio_set_direction(OUTPUT)`, so the level is always set before the direction (`digitalWrite` before `pinMode`; `PinDriver::output` would enable the output at its creation); `PinDriver::input(gpio2, Pull::Up)` + `is_low()` |
| Nvs | raw `nvs_*` of the default partition for everything (`EspNvs` adds nothing the port needs, and its `set_blob` erases the key before writing: a power cut in between would lose the old `cfg`): `nvs_flash_init` by `main` (§6.4; on `NO_FREE_PAGES` / `NEW_VERSION_FOUND` `nvs_flash_erase` and init again, as Arduino's `initArduino`, any other error leaves the partition alone), `nvs_open`, `nvs_get_u8` … `nvs_get_i64`, `nvs_set_*` + `nvs_commit`, `nvs_get_blob` (length with a null buffer), `nvs_set_blob` + `nvs_commit`, `nvs_get_str` through a heap buffer of the string's size (strings are bytes, not necessarily UTF-8), `nvs_erase_key`/`nvs_erase_all` + `nvs_commit`; keys and namespaces as C strings in a 16-byte stack buffer |
| Fs | raw `esp_vfs_littlefs_register(&esp_vfs_littlefs_conf_t { base_path: "/littlefs", partition_label: "spiffs", format_if_mount_failed: 0 })`, `esp_littlefs_format("spiffs")`, `esp_littlefs_info("spiffs")`; component `joltwallet/littlefs` 1.22.3 through `[[package.metadata.esp-idf-sys.extra_components]]`; files through `std::fs` with the path "/littlefs" + the glue's path in a 160-byte stack buffer (`open()`/`write()` reach `lfs_file_*` directly: no stdio buffer, LittleFS's 512 B cache per open file); `std::fs::read_dir` for `list`; `remove` tries `remove_file`, then `remove_dir` |
| TcpConnector, TcpStream, Udp | `std::net::TcpStream::connect_timeout` after `ToSocketAddrs` (lwIP DNS); the socket stays blocking with `set_write_timeout(10 s)` and `set_nodelay(true)` (WiFiClient); reads and `connected` use `esp_idf_sys::lwip_recv(fd, .., MSG_DONTWAIT)` (with `MSG_PEEK` for `connected`) on the raw fd, `EWOULDBLOCK` = empty / still connected; `close` shuts the socket down and drops it; `std::net::UdpSocket` bound once in `main` (after `NetifStack::initialize()`: lwIP must exist before the first socket), `send_to` |
| HttpServer, HttpRequest | `http::server::EspHttpServer::new(&Configuration { http_port: 80, stack_size, core: Some(Core::Core0), max_open_sockets: 4, max_uri_handlers: 1, lru_purge_enable: true, uri_match_wildcard: true, keep_alive: None, .. })` owns the server; the catch-all handler is registered on its raw handle with `esp_idf_sys::httpd_register_uri_handler(server.handle(), &httpd_uri_t { uri: c"/*", method: HTTP_ANY (INT_MAX), .. })` and works on `*mut httpd_req_t`: `httpd_req_get_hdr_value_len` + `_str` through a 257-byte buffer (a value is read as at most its first 256 bytes: the web server reads no more of any header), `req.uri`, `req.method` (GET, POST, DELETE, else Other), `req.content_len`, `httpd_req_recv` (`End` once Content-Length bytes arrived, also for an empty buffer; `HTTPD_SOCK_ERR_TIMEOUT` = Timeout; 0 or another error = Closed), `httpd_req_to_sockfd` + `lwip_getpeername`/`lwip_getsockname`. Responses go out as AsyncWebServer framed them, built by `glue::http_parse::response_head` (status line with its reason phrase, Content-Length, Content-Type unless empty, the handler's headers, `Accept-Ranges: none`; chunked: `Transfer-Encoding: chunked` and `chunk_size_line`) and sent with `httpd_send` in a loop; HEAD gets the head only. Not `httpd_resp_send`: it always writes a Content-Type line (204 and 304 have none) and sends every header in pieces. A request the web server leaves unanswered, or a send that fails, returns `ESP_FAIL` (the server closes the connection). The log tag `httpd_txrx` is set to errors only: a client that resets its connection is no fault. `EspHttpConnection` is not used (§4.1) |
| Ota | raw `esp_idf_sys::esp_ota_*`: `esp_ota_get_running_partition`, `esp_ota_get_next_update_partition(null)`, `esp_ota_get_partition_description` (`AppId` = `app_elf_sha256[..8]`; `None` without a readable descriptor), `esp_ota_begin(part, OTA_WITH_SEQUENTIAL_WRITES, ..)`, `esp_ota_write`, `esp_ota_end` (image + SHA-256 check), `esp_ota_set_boot_partition` (checks the image before otadata; `set_boot(address)` finds the slot among the running and the other one), `esp_ota_abort`, `esp_ota_mark_app_valid_cancel_rollback` (error ignored); `esp_image_verify(ESP_IMAGE_VERIFY_SILENT, ..)` for `verify` and once for `running_image_size` (cached). Not `ota::EspOta`: its `initiate_update` erases the whole slot up front (`OTA_SIZE_UNKNOWN`, seconds of blocking in the HTTP handler; Arduino's `Update` erased per sector) and it cannot select an arbitrary slot |
| Md5 | ROM MD5: `esp_idf_sys::esp_rom_md5_init/update/final` |
| System | `esp_idf_hal::reset::restart()`; `esp_idf_sys::esp_reset_reason()` (numbers 1..10 as in IDF 4.4); `heap_caps_get_free_size / get_minimum_free_size / get_largest_free_block(MALLOC_CAP_8BIT)`; `xTaskGetHandle` + `uxTaskGetStackHighWaterMark` (bytes on ESP-IDF); `esp_efuse_mac_get_default` |
| HeapGate | always `true`; the glue then calls `Vec::try_reserve_exact` |
| Ethernet | the MAC and PHY through the ESP-IDF calls with the values of Arduino-ESP32 2.0.7's `ETH.begin(1, 16, 23, 18, ETH_PHY_LAN8720, ETH_CLOCK_GPIO0_IN)`: `esp_eth_mac_new_esp32` (RMII, SMI MDC 23 / MDIO 18, clock in on GPIO0, EMAC software reset timeout 1000 ms as Arduino; `EthDriver::new_rmii` fixes 100 ms) and `esp_eth_phy_new_lan87xx` (address 1, reset GPIO16 = the enable of the 50 MHz oscillator, which Arduino passed as the PHY power pin), `esp_eth_driver_install`, an `EspNetif::new(NetifStack::Eth)` (DHCP client) with `esp_netif_set_hostname` before the start and, for a static address, `esp_netif_dhcpc_stop` + `esp_netif_set_ip_info` + `esp_netif_set_dns_info` (Arduino `ETH.config`), `esp_netif_attach(esp_eth_new_netif_glue)`, `esp_eth_start`. `EthEvent::{Connected, Disconnected, Stopped}` of this driver and `IpEvent::DhcpIpAssigned` of this netif through `EspSystemEventLoop::subscribe` into atomics, as the C++ `net::onEvent` flags (CONNECTED: link and the handle of the first link; GOT_IP: address and count; DISCONNECTED/STOP: neither). Restart: `esp_eth_stop`, 200 ms, `esp_eth_start` on the handle of the first CONNECTED (false before it, results ignored as in C++). `esp_netif_get_ip_info` / `_get_dns_info`, `EspNetif::get_mac`. Feature `qemu`: `esp_eth_mac_new_openeth` + `esp_eth_phy_new_dp83848` (address auto, no reset pin) and a shutdown handler that stops the driver before `esp_restart` (QEMU does not reset the OpenETH model on a software restart), and the default log level set to errors (the OpenETH interrupt handler of ESP-IDF 5.5 logs a dropped frame with an early warning whose strings are in flash: a "Cache error" panic when it meets a flash access under load) |
| Wifi | feature `wifi` (default): `wifi::WifiDriver::new(Modem::steal(), sysloop, None)` (no NVS partition: nothing in the WiFi NVS, like `WiFi.persistent(false)`) wrapped by `EspWifi::wrap_all` with an STA netif configured like the Ethernet one (host name before start; `CONFIG_ESP_WIFI_SOFTAP_SUPPORT=n`, so no AP netif), `esp_wifi_set_mode(STA)` + `esp_wifi_start`; credentials through raw `esp_wifi_set_config(WIFI_IF_STA, ..)` with the fields of Arduino-ESP32 2.0.7's `wifi_sta_config()` (fast scan, by signal, RSSI -127, PMF capable, WPA2-PSK minimum with a password; the SSID and password are bytes), after `esp_wifi_disconnect`, then `esp_wifi_connect`; `reconnect` = disconnect + connect; `stop` drops the driver (deinit frees its ~33 KB, like `WiFi.mode(WIFI_OFF)`), the next `begin` builds it again; RSSI from `get_ap_info().signal_strength`; `WifiEvent::StaDisconnected` with a reason other than `ASSOC_LEAVE` (8, a disconnect the firmware asked for) counts as unrequested (Arduino's retry trigger), `IpEvent` of the STA netif. Without the feature (`nowifi` build) a station that is never built |
| Sntp | `sntp::EspSntp::new_with_callback(&SntpConf { servers: [server], operating_mode: Poll, sync_mode: Immediate }, ..)`, the callback (lwIP thread) counts syncs and keeps the epoch in atomics; one instance at a time: a server change drops it (`sntp_stop`) and creates a new one; `CONFIG_LWIP_SNTP_MAX_SERVERS=1` |
| Pinger | raw `esp_ping_new_session` / `esp_ping_start` / `esp_ping_delete_session` (`count 1, timeout_ms 1000, interval_ms 1000, data_size 32`, the rest of `ESP_PING_DEFAULT_CONFIG`: TTL 64, task 2560 B, prio 2); `on_ping_success` / `on_ping_timeout` set atomics. Each session has its own task and raw socket, freed within about 1 s of the delete, as in C++; `delete` without a session does nothing. Not `EspPing::ping`, which blocks the caller for the whole probe |
| Rtc | one `#[link_section = ".rtc_noinit.vdm"] static mut` block of `app::RTC_LEN` bytes (NOLOAD; kept by `esp_restart`, panic and watchdog resets, garbage after power-on) behind a `Mutex`, read and written byte by byte (volatile) |
| Spawner, boot deadline | `app::Spawner` of `main`: per `app::TASKS` entry `ThreadSpawnConfiguration { name, priority, pin_to_core, inherit: false }.set()` (the name as a leaked C string) and `std::thread::Builder::new().stack_size(..)`; the TWDT subscription made on the new thread. The boot deadline: `esp_timer_create` (dispatch on the esp_timer task) + `esp_timer_start_once(60 s)` calling `app::boot_deadline` |

Every adapter call that returns an `EspError` maps it to the port's `bool`/`Option`/`EspErr`; the
error number is kept where an event or a document shows it.

---

## 2. Threads, shared data, memory

### 2.1 Threads (D§3)

Threads are spawned by `main` with `esp_idf_hal::task::thread::ThreadSpawnConfiguration { name:
Some(c"stm"), priority, pin_to_core: Some(Core::Core1), inherit: false, .. }.set()` followed by
`std::thread::Builder::new().stack_size(n).spawn(..)`: the builder's stack size wins over the
configuration's, and the configuration's name becomes the FreeRTOS task name (`xTaskGetHandle`
finds it; `Builder::name` does not). A glue task is a struct with `start()` and `pass() -> u32`
(the delay of the next pass in ms); the thread body is `start(); loop { feed; d = pass(); sleep(d) }`,
and the glue feeds the watchdog inside long passes as the C++ does (MQTT after every publish).

| Thread | Created by | Core | Prio | Stack | Peak in QEMU | C++ size (peak measured) | TWDT | Loop | Runs |
|---|---|---|---|---|---|---|---|---|---|
| `main` | ESP-IDF `app_main` | 0 | 1 | `CONFIG_ESP_MAIN_TASK_STACK_SIZE` 32768, freed when `main` returns | 27,336 | `loopTask` 8192 | no; boot deadline §6.4 | once | NRST release, NVS, boot guard, boot order of D§3, spawns, returns |
| `stm` | main | 1 | 5 | 8192 | 6,000 | 6656 (2676) | yes | 2 ms | D§3 `stm` |
| `app` | main | 1 | 3 | 10240 | 8,060 | 7168 (3136) | yes | 100 ms | D§3 `app`, HTTP server start, boot guard confirm and switch |
| `mqtt` | main | 1 | 2 | 16384 | 12,532 (a discovery run) | 7168 (3200) | yes, also after every publish | 20 / 100 / 500 ms | D§3 `mqtt` |
| `httpd` | `EspHttpServer` | 0 | 5 (fixed, §4.1) | 15360 | 11,868 (a config save) | `async_tcp` 8960 (3532) | no: it blocks in `select()` for hours | event driven | every HTTP handler, uploads, log download |
| `sys_evt` | ESP-IDF | 0 | 20 | 3072 (`CONFIG_ESP_SYSTEM_EVENT_TASK_STACK_SIZE=2560` + 512) | 960 | 2560 (1076) | no | event driven | the adapters' ETH/WiFi/IP event callbacks (atomics only); `arduino_events` (4096 B) is gone |
| `tiT` | lwIP | 0 | 18 | 3072 (`CONFIG_LWIP_TCPIP_TASK_STACK_SIZE=2560` + 512, as 2.1.7) | 1,440 | 3072 (1736) | no | event driven | lwIP, DHCP, SNTP callback |
| `esp_timer` | ESP-IDF | 0 | 22 | default (`CONFIG_ESP_TIMER_TASK_STACK_SIZE`) | not watched | 4608 (784) | no | | boot deadline timer |
| `ping` | `esp_ping_start` | any | 2 | 2560 | not watched | same | no | | one probe, at most every 60 s |

- The sizes are the peaks measured in QEMU on 2026-10-07 (scenarios api, mqtt and soak of
  §5.5: every GET route, config saves, an STM image upload, the log download, an MQTT session
  with the HA discovery run, 10 parallel clients; no STM, WiFi off) plus about 25 %, rounded up
  to 1 KiB, and not below the first build (the C++ sizes plus about 25 %). The peaks are high
  where LTO inlines whole constructors and steps into one frame: `main` builds every module in a
  21 KB frame, the web server's `serve` takes 3.8 KB, a discovery run puts its 4.4 KB context on
  the `mqtt` stack once (§2.4). Before the first release the measuring campaign on the device
  under `tools/loadtest.py` (3 and 10 workers, WiFi scanning next to Ethernet, an STM flash, a
  discovery run) checks them; `StackLow` (max(512 B, stack / 8)) watches the same seven tasks as
  D§3, with `httpd` in place of `async_tcp` and without `arduino_events`.
- `CONFIG_FREERTOS_HZ=1000` is binding: at the IDF default of 100 Hz a 2 ms delay is 0 ticks and the
  `stm` thread (prio 5) would starve core 1.
- TWDT: 30 s, panic, idle task of core 0 watched (Arduino-ESP32 2.0.7: `CHECK_IDLE_TASK_CPU0=y`,
  CPU1 off). Blocking limits of D§3 stay; HTTP handlers block only on their own socket (§4.2).

### 2.2 Shared data

All cross-thread state lives in `glue::shared` objects created by `main` and handed to the module
structs as `&'static` (leaked boxes); tests create them per case. `std::sync::Mutex` is a FreeRTOS
mutex with priority inheritance. No lock is held while another is taken; readers copy out, then act.
The ESP32 has no 64-bit atomics, so `i64` values sit behind a mutex.

| Data | Writer → readers | C++ | Rust |
|---|---|---|---|
| STM commands | web, mqtt, app → stm | FreeRTOS queue 16, non-blocking | `CommandQueue`: `Mutex<heapless::Deque<StmCommand, 16>>`, `try_push` (full → 503 / MQTT reject / latch, D§3), `pop` |
| STM snapshot (~3 KB) | stm (≤ every 100 ms on change) → web, mqtt | mutex + copy into static copies | `Mutex<Box<StmSnapshot>>`; readers `clone_from` into their own boot-allocated box when `revision` moved |
| Profile store (12 × 260 B) | stm → web, mqtt | under the snapshot mutex | in the same `Mutex` as the snapshot |
| Link state, flash active, protocol, support, revision, STM save state | stm, app | `volatile` | `AtomicU8` / `AtomicBool` / `AtomicU32` |
| Active config (2.5 KB) + revision | storage (`apply_config`) → all | mutex + 2.5 KB copies | `Mutex<Box<Config>>` + `AtomicU32`; `with_config(\|c\| ..)` reads the parts a task keeps without a copy (§2.4) |
| Event ring (32 × 48 B) + log statistics | any → logger sinks, web, mqtt | mutex | `Mutex<EventLog>` |
| Calibration info (i64 epochs) | app → web, mqtt | portMUX | `Mutex<CalibInfo>` |
| MQTT status, regulator input, calibration ends | mqtt → web, stm (1 / s) | portMUX | `Mutex` each |
| Restart request, OTA health, upload flags | any / app / httpd | portMUX, `volatile` | `Mutex<RestartRequest>`, `Mutex<OtaHealthInfo>`, atomics |
| Net info, health, trial info; trial requests; inbound counter | app, httpd, event callbacks | portMUX, `volatile` | `Mutex` for the structs, atomics for flags and counters |
| Target and calibration-result hand-over | stm → app | portMUX | `Mutex<Option<..>>` |
| Image index, upload and copy state | httpd, app, stm | FS mutex | `Mutex<ImageIndex>` inside `storage` |
| RTC records | stm, mqtt, net, boot guard | direct | `Rtc` port with its own lock (§6.1 layout) |

### 2.3 RAM budget (no PSRAM; 263 KB of 8-bit heap)

| Item | C++ 2.1.7 | Rust |
|---|---|---|
| Static DRAM (.data + .bss) | 54.5 KB (CHANGELOG 2.1.6) | 32.0 KB (14.9 + 17.1, `esp_idf_size`) |
| Task stacks (stm, app, mqtt, HTTP) | 6656 + 7168 + 7168 + 8960 = 29 952 B | 8192 + 10240 + 16384 + 15360 = 50 176 B (§2.1) |
| `arduino_events` | 4096 | 0 |
| HTTP response buffers | 2 × 12 KB | 1 × 12 KB (§4.3) |
| Web working set | ~9.6 KB, kept from the first request | same parts, ~9.6 KB |
| Boot allocations (session, snapshots, profile store, config copies, ...) | 29 KB | same set and sizes (core structs are ported field by field) |
| PubSubClient buffer | 2304 | 2304 (`mqtt_conn`) |
| Event ring | 32 × 48 B | 32 × 48 B |
| Per request | AsyncWebServer request/response objects, header `String`s, send buffers | esp_http_server header scratch (≤ 1024 B, freed at the end of the request), one session struct per connection |

Net of the rows above: about 18 KB more free heap than 2.1.7 (−22.5 KB static DRAM, +20.2 KB
stacks, −4 KB `arduino_events`, −12 KB response buffer), before the differences of ESP-IDF 5.5 and
Rust std.

Acceptance on the device (Ethernet, idle, after boot): free heap ≥ C++ 2.1.7 minus 10 KB (2.1.7:
~123 KB free, ~108 KB minimum), and the minimum under `tools/loadtest.py` with 10 workers not below
the 2.1.7 figure (at least 80 KB under parallel web load, CHANGELOG 2.1.6).

Measured in QEMU on 2026-10-07 (scenarios mqtt and soak of §5.5; `heap_caps_*` of
`MALLOC_CAP_8BIT` through /api/health). Indicative only: OpenETH takes 8.0 KB of DMA buffers where
the device's EMAC takes 10.9 KB (about 2.9 KB less free on the device), and there is no STM, no
WiFi and no real network latency.

| Point | Free | Minimum since boot | Largest block |
|---|---|---|---|
| 60 s after boot, Ethernet up, MQTT connected | 129 420 | 115 412 | 110 592 |
| after a config save (`POST /api/config`) | 130 112 | 112 860 | 110 592 |
| after the HA discovery run | 129 448 | 115 276 | 110 592 |
| loadtest.py, 3 workers, 180 s, a config POST every 15 s: 141 requests, 135 × 200, 6 client timeouts (30 s), no 503 | 114 872 … 124 312 | 92 228 | 106 496 … 110 592 |
| loadtest.py, 10 workers, 180 s, the same POSTs: 274 requests, 240 × 200, 34 client timeouts, no 503 | 113 652 … 122 264 | 92 184 | 102 400 … 110 592 |
| 10 s after each load | 130 100 and 130 092 | — | 110 592 |

The load figures are those of the final build (the soak scenario of the last full run); request
counts depend on the host's load, an earlier run with 613 and 473 requests read the same heap
figures within 10 KB.

Both acceptance figures hold in QEMU: idle 129.4 KB against ≥ ~113 KB, and 92.2 KB under load
against 80 KB; no 503 and no restart under either load, the heap back after it. The device
campaign of §2.1 decides.

Options to shrink the stacks without changing a function, not taken (the margin is not needed;
they touch gated core and glue modules). Frame sizes from the `entry` instructions of the QEMU
build's ELF:

| Option | Where | Frames today | Expected |
|---|---|---|---|
| a. Build the discovery context and the `Config` blocks in their heap blocks without a stack temporary (a `const` value pushed into the reserved slot; zeroed in place would need the `unsafe` the glue forbids) | glue `heap`, `mqtt_client` | `MqttClient::start_run` 4.6 KB + `DiscoveryContext::default` 4.4 KB on the 12.5 KB `mqtt` peak; `try_block::<Config>` 3.3 KB + `Config::default` 3.0 KB on the 8.1 KB `app` peak | `mqtt` peak -5 to -8 KB (stack 16 → 10 KB), `app` -3 KB (10 → 8 KB) |
| b. `clone_from` of `Config` and `StmSnapshot` without a full temporary (field by field, or `Copy` and an assignment) | core `config`, `stm_types` | `Config::clone_from` 3.5 KB on the 11.9 KB `httpd` peak (a config save), `StmSnapshot::clone_from` 3.0 KB on `stm` and the status routes | `httpd` -3 KB (15 → 12 KB); `stm` -3 KB (stays 8 KB, its floor) |
| c. `#[inline(never)]` on the route handlers, so the frame of `serve` (3.8 KB) is not their union | glue `web_server` | `WebServe::serve` 3.8 KB | `httpd` -1 to -2 KB |
| d. `Fs::list` through raw `opendir`/`readdir`/`stat` | firmware `adapters/fs.rs` | `LittleFs::list` 2.1 KB (std `read_dir` and `metadata`) | -1.5 KB on `/api/files` and the image index |

a to c together: about 10 to 13 KB of stack, about 8 to 10 KB of heap after a new measurement with
the 25 % margin.

### 2.4 Heap rules

- Boot: the long-lived blocks of D§9 are `Box`ed in `main`; a failure aborts (the C++ `bootAlloc`).
- After boot only the transient blocks of D§9 are allocated, through `glue::heap`: `HeapGate::grant`
  (the test fake scripts failures and records sizes, as `fakes::heap()` did) and then
  `Vec::try_reserve_exact`, so a failure gives the C++ outcome (503 `busy` "out of memory", the next
  pass tries again, defaults with reason 102). The blocks: a JSON body (its Content-Length, ≤ 8 KB),
  the config blob buffers (4 KB + 1.5 KB, also the import report text), a discovery run (context
  3.4 KB + payload 2 KB), the 1 KiB last_good chunk, a profile copy with its JSON (0.9 KB), and the
  web working set and response buffer at their first use (kept).
- Typed blocks are created with `try_reserve_exact(1)` and `push(T::EMPTY)` (a `const` value); the
  worst case puts the value on the stack once, which the stack budget covers (DiscoveryContext
  3.4 KB on the `mqtt` stack). No `unsafe` in `glue`.
- Gone: the 2.5 KB `Config` copies of a config reload (app, stm_link, mqtt, net revert) read under
  the config lock with `with_config`; the patched copy of `POST /api/config` is applied to the
  web's own config copy, which is reloaded when the patch fails (§4.4).
- Big values never on a stack: `clippy::large_stack_frames` (threshold 1024 B) and
  `clippy::large_stack_arrays` are errors in `glue`; the one exception above is allowed by name.
- No `String`/`Vec` growth and no `format!` in steady state: `heapless::String<N>` and the core
  writers into fixed buffers. Static DRAM counts like heap: long-lived state goes into boot boxes,
  not into `static`s.

---

## 3. Module map

| C++ glue file | Rust glue module(s) | Core modules it drives | Stays in `firmware/` |
|---|---|---|---|
| `main.cpp` (hooks, `setup`, `loop`) | none: the boot order is `app::setup` | — | `main.rs`: NRST release first, NVS init, boot guard (§6.4), platform construction, thread spawn |
| `board.h` | `board.rs` (pins, UART ring sizes, factory pin timing) | — | taking the pins from `Peripherals` |
| `boot_alloc.h` | `heap.rs` (boot boxes, fallible helpers) | — | nothing (Bluetooth is off: `CONFIG_BT_ENABLED=n`, no DRAM to release) |
| `app.cpp` | `app.rs` (`TASKS` table, setup, app task, resources, heap guard, factory latch, health), `shared.rs` | factory_reset, sys_health, json_api (HealthSnapshot), version | spawning from `app::TASKS` |
| `stm_link.cpp` | `stm_link.rs` (session port, flash transport, stm task) | stm_session (owns line_assembler, stm_codec, link_policy, poll_planner, valve_model, lease_client, target_store, reset_gate, health_monitor, calib_schedule's LearnTimeSync), stm_flasher | UART2, IO14, IO15 |
| `stm_service.cpp` | `stm_service.rs` | calib_schedule, target_store, lease_client (records) | — |
| `mqtt_client.cpp` | `mqtt_client.rs` (task), `mqtt_conn.rs` (new: PubSubClient 2.8 semantics) | mqtt_topics, mqtt_policy, mqtt_values, event_limiter, ha_discovery, event_log, json_writer, config | TCP socket |
| `web_server.cpp` | `web_server.rs` (+ `web_server/views.rs`, `uploads.rs`), `http_parse.rs` (new: AsyncWebServer's URL decoding, query, media type; multipart), `json_body.rs` (new: ArduinoJson 6.21.6 subset) | json_api, web_guard, legacy_http, file_manager, config, image_store, stm_flasher (names, board check), version | esp_http_server adapter, generated assets |
| `net.cpp` | `net.rs` | net_policy, net_trial, config (network fields) | Ethernet, WiFi, SNTP, ping, netif events |
| `storage.cpp` | `storage.rs` (+ `storage/config_files.rs`, `images.rs`, `files.rs`) | config (encode, decode, repair, validate), legacy_import, file_manager, image_store, stm_flasher (`validateImage`), json_writer | NVS, LittleFS |
| `logger.cpp` | `logger.rs` | event_log, log_sink | console, UDP |
| `ota.cpp` | `ota.rs` (+ `ota/update.rs`: the Arduino `Update` contract) | ota_policy, restart_gate, sys_health | esp_ota, ROM MD5 |
| — | `boot_guard.rs` (new, §6) | ota_policy (`OtaValidator` stays) | otadata, RTC |

### 3.1 Logic of the C++ glue without a core equivalent

D§1 wants no decision logic in the glue, yet the glue files hold some, and some came from the
Arduino libraries. All of it moves into the glue modules above and is tested there (§5):

| Module | Logic |
|---|---|
| `app` | factory pin sampling (2 ms settle, 50 ms samples around core `PinHold`); boot order; task table; app-task cadence (1 s, 10 s, 100 ms); resource sampling with lazy task lookup; heap guard "blocked" (upload, flash, restart pending); config-change fan-out (logger, net); `flashActive` derived from the flash phase |
| `stm_link` | config trust (defaults not saved since boot are untrusted); 4 commands and 4 × 128 B UART reads per pass; 1 s cadence; flash branch; bounded input discard (64 reads) |
| `stm_service` | 10 s calibration tick; second `localtime` pass for the UTC offset at the slot; attempt id matching of results; target saver plumbing; RTC record writes |
| `mqtt_client` | slot layout (0 common, 1..12 valves, 13..46 temps, 47..54 volts, 55 STM, 56 system); full-publish budget (4 slots or one valve per pass); on-change cursor (2 per pass); diag budget (4); paced counters (10 s); STM start tolerance (60 s); profile CRC with zeroed padding; inbound queue (4, payload cut at 33 B) and its overflow rejects; target latch with cursor; clean-session flag; LWT; subscription one filter at a time; offline on a pending restart; drop the connection after a failed publish; client id from the eFuse MAC; discovery memory and gate wiring; `common/message` from Warning+ events |
| `mqtt_conn` (PubSubClient 2.8) | CONNECT with will (`offline`, QoS 0, retained) and clean-session flag; CONNACK wait 5 s; keepalive PINGREQ and timeout (state -4); one packet per `loop()`; QoS-0 publish into a 2304 B buffer (larger → false); QoS-1 PUBACK; oversized inbound packets dropped; SUBSCRIBE without waiting for SUBACK; state codes -4..5 (events 203, `mqttRc`) |
| `web_server` | guard and limit order (415, 411, 413, 409 for uploads; 415, 413 for bodies); legacy refusal order; error JSON; status, valve and sensor views (60 s stale rule, volt milli value with clamp); events halving loop; strict query numbers; JSON field validators (`intField`, `boolField`, `onlyKeys`, target rounding); per-handler checks (service move, sensors, motor merge with snapshot values, breakaway, flash request, factory reset, discovery); upload state (one file per request, first failure wins, storage/OTA result → HTTP code); MD5 sources in order (query `md5`, form field `md5`, query `MD5`, form field `MD5`, header `X-Update-MD5`, header `X-MD5`; form fields count only before the file part); log stream; assets and ETag; config save and dry run |
| `http_parse` (AsyncWebServer 1.6.2) | path and query split; `urlDecode` (`%` + the next two characters through `strtol`, so non-hex gives what `strtol` parses; `+` → space; a `%` with fewer than two characters after it is kept); first query parameter of a name wins; media type = header value up to the first `;`; multipart: boundary, part headers, form fields before the file part, file data in chunks, closing boundary |
| `json_body` (ArduinoJson 6.21.6) | the deserializer for the small bodies (target, service move, sensors, motor, STM reset, flash, factory reset, discovery, `/setvalve`): grammar incl. its leniencies, zero-copy strings, a 32-slot pool (`StaticJsonDocument<512>` on the 32-bit target, 16 B per slot), nesting limit 10, error names (`EmptyInput`, `IncompleteInput`, `InvalidInput`, `NoMemory`, `TooDeep`), `is<long long>` / `is<double>` / `is<bool>` / string queries, duplicate keys as the library resolves them |
| `net` | interface choice, WiFi fallback after 30 s, back-off 5..60 s, Ethernet down timing; static IP with `effectiveDns`; NetUp/NetDown events with reconnect count and `upSince`; time-sync step; ping session lifecycle (10 s limit); evidence from counters (DHCP lease only with DHCP); watchdog actions (interface mask, restart detail in minutes); RTC restart count (`VNWD`); trial at boot (Start, RevertNow, Stale, NotStored), confirm/revert requests, `reconfigure` (armed this boot, unstored record, time change); inbound filter (loopback, own IP); `localTime` validity (≥ 2020); one WiFi retry after the first unrequested disconnect since boot (Arduino behaviour, D§16) |
| `storage` | load order with error codes 100/101/102; backup pair protocol (`.tmp`, rename order, cut detection); `sameFile`; kept unknown `cfgx` records; legacy reader type probing (u8, i8, u16, i16, u32, i32, i64); factory reset (latch kept, `imported` set; boot guard keys kept, §6.5); image index (slots, leftovers, limits); upload (space reserve, size limit, CRC); last_good copy (8 × 1 KiB per pass, space check); image scan; file list (root + one level, 32 entries); legacy image removal in batches of 8; import report |
| `logger` | syslog batches (32 per pass, own cursor); file sink: lazy open, rotation loop with deferral (8 KB slack), gap lines, per-line cursor; flush policy wiring; statistics; epoch validity (≥ 2020); console mirror |
| `ota` | upload checks (busy, flash or image upload, partition, Content-Length ≤ partition + 16 KB, MD5 form); first-wins restart request (an upload replaces a pending rollback); loopback self-check (connect 1 s, answer 3 s, ≥ 12 bytes, core `httpStatusOk`); restart sequence (flash wait, gate, target flush except factory reset, event 323, confirmation of user restarts, `otaStm`, log flush, restart or switch); reboot severity by reason |
| `ota/update` (Arduino `Update`) | error codes 0..12 and their texts (copied from `Updater.cpp` of Arduino-ESP32 2.0.7), shown by event 107 and the HTTP detail; MD5 over the written bytes compared at the end (`MD5 Check Failed`); magic byte; `Update.end(true)` semantics for an unknown size |
| `boot_guard` | all of §6 |

---

## 4. HTTP on esp_http_server

### 4.1 Server

`EspHttpServer` starts and owns the server. Its connection API (`fn_handler`, `EspHttpConnection`)
is not used: in 0.53.0 it sends every non-empty response chunked and drops a `Content-Length`
header (the C++ answers carry Content-Length), its `read` panics once a response was initiated, and
a raw `httpd_resp_send` inside one of its handlers would be followed by the wrapper's closing chunk.
So the adapter registers one raw handler on `server.handle()` (§1.2); the handler wraps the
`httpd_req_t` into the `HttpRequest` port and calls `Web::handle`.

| `Configuration` field (→ `httpd_config_t`) or sdkconfig | Value | Why |
|---|---|---|
| `http_port` | 80 | |
| `core` | `Some(Core::Core0)` | `async_tcp`'s core; stm/app/mqtt keep core 1 |
| task priority | 5, fixed by `EspHttpServer` (`async_tcp` had 3) | on core 0 only the ping tasks and the idle task run below it |
| `stack_size`, `task_caps` | 15360 (§2.1), internal 8-bit (default) | every handler runs here |
| `max_open_sockets` | 4 (default) | the connection cap of 2.1.3 (`VDM_MAX_CONN`) |
| `lru_purge_enable` | true (default) | a fifth client closes the least recently used session instead of waiting for a dropped SYN to be retried |
| backlog, `recv_wait_timeout`, `send_wait_timeout` | 5, 5 s, 5 s, fixed by `EspHttpServer` | connections wait in the backlog while a long request runs; the timeouts bound a stalled client (§4.7) |
| `uri_match_wildcard`, handler | true; one raw `httpd_uri_t { uri: "/*", method: HTTP_ANY }` | routing stays with core `matchApiRoute` / `matchLegacyRoute`, so every method gets the firmware's own JSON 404/405 |
| `max_uri_handlers`, `max_resp_headers` | 1, 8 (default) | at most 4 response headers are used |
| `keep_alive` | `None` | TCP keep-alive probes off, as in C++ |
| `CONFIG_HTTPD_MAX_REQ_HDR_LEN`, `CONFIG_HTTPD_MAX_URI_LEN` | 1024, 512 (defaults) | larger requests get the server's own 431 / 414 (§4.7) |
| `CONFIG_LWIP_MAX_SOCKETS` | 16 (Arduino-ESP32 2.0.7) | ≥ `max_open_sockets` + 3 (checked by `httpd_start`) + MQTT, syslog, self-check, ping |

The server starts in the app thread once the network has an IP (`HttpServer::start`, retried every
second until it succeeds), as `web::begin()` did.

### 4.2 Request pipeline (`web_server::Web::handle`)

1. `net.note_inbound_http(remote_ip)` (every request that reaches the handler, refused ones too).
2. Working set: allocated by the first request and kept (§2.4); no memory → `503 busy "out of
   memory"`. The config copy is refreshed when the revision moved.
3. Refusal, before any body byte is read: outside `/api/` the legacy table (410 `gone` with the
   replacement, 405, the alias guard, a body on any other path → 404/405); inside `/api/` the route,
   the K5 guard (Host, Origin, X-VdMot, Content-Type; event 213 once per verdict per 60 s), then the
   upload limits (multipart, Content-Length 1..limit, no upload/flash running) or the body limits
   (no multipart, ≤ 8 KB). A refused request is answered at once; esp_http_server reads and drops
   the rest of the body after the handler returns.
4. Static assets and 410 answers skip the guard (D§12).
5. Body routes: Content-Length bytes into a heap buffer of that size (`try_reserve_exact`; no memory
   → `503 busy "out of memory"`), then the handler. Upload routes: §4.5.
6. The handler answers exactly once (the fake request asserts it, §5.1). The body buffer is dropped
   when `handle` returns.

Handlers run one after another on the `httpd` thread, as they did on `async_tcp`; the scratch
buffer of D§12 stays shared. A handler waits only on its own socket (`httpd_req_recv`,
`httpd_send`, each bounded by 5 s) and takes locks only to copy shared data.

### 4.3 Buffers

| C++ 2.1.7 | Rust |
|---|---|
| 2 × 12 KB response slots, a slot released in `onDisconnect` after the response left | 1 × 12 KB response buffer: `httpd_send` returns when lwIP has copied the bytes, so the buffer is free for the next request |
| Scratch (views, event/image/file lists, status and health snapshots, health text, a profile) | the same, as one boxed `enum Scratch` sized by its largest variant (no unsafe reinterpretation). *Implementation:* the lists are a heap block of the request, the snapshots and the profile values on the handler's stack (PORT-NOTES.md, web_server "Design deviation (4.3)") |
| `StaticJsonDocument<512>` | the 32-slot pool of `json_body` in the working set |
| Guard detail 160 B, snapshot and config copies | same |
| `/api/health`: 1 KB text copied into an AsyncWebServer `String` | text built in the scratch and sent from there. *Implementation:* built in the response buffer and sent from there |
| JSON body: heap buffer of its Content-Length, one body at a time (409 `busy` for a second) | same buffer; with serial handling a second body never meets the first |
| `POST /api/config`: slot reserved first (503 `busy`, nothing applied), patched heap copy | the response buffer is always free; the patch goes into the web config copy, which is reloaded from the active config when the patch, the validation or the save fails |
| `beginResponse_P` (Content-Length, the slot's bytes) | the head of AsyncWebServer's `_assembleHead` (`http_parse::response_head`: "HTTP/1.1 NNN Text" from its reason table, Content-Length, Content-Type, the headers, `Accept-Ranges: none`) and the body, sent with `httpd_send` (§1.2) |

### 4.4 JSON bodies

- At most 8 KB (413 before reading), read whole into the body buffer.
- `POST /api/config` and `?dryRun=1` keep core `applyConfigJson` (strict parser, D§7 apply semantics).
- Every other body goes through `json_body` with the ArduinoJson 6.21.6 behaviour the C++ glue saw
  on the device: the error detail of a malformed body is ArduinoJson's error name, and the
  `NoMemory` boundary is 32 members (the 32-bit slot size; the C++ host tests computed it with the
  host's 64-bit slots).
- Parity of `json_body` is proven against the vendored `test/native/third_party/ArduinoJson-v6.21.6.h`:
  a corpus of bodies (valid, lenient, malformed, deep, large, duplicate keys) runs through a small
  C++ program built with `-m32` (the device's 32-bit slot layout) in the native container, and its
  results are a golden file of the Rust tests.

### 4.5 Uploads

| Step | STM image (`POST /api/stm/images`) | ESP image (`POST /api/ota/esp`) |
|---|---|---|
| Limits (§4.2 step 3) | multipart, Content-Length 1..512 KiB + 8 KB | multipart, Content-Length 1..partition size + 8 KB |
| Begin | `storage.image_upload_begin(filename, content_length)` at the first file byte | `ota.upload_begin(content_length, md5)`; refused during a boot-guard trial (§6.5) |
| Data | `read_body` into the response buffer → `http_parse` multipart → `image_upload_write` (512 B LittleFS writes, CRC, size limit) | → `OtaUpdate::write` (sequential erase), MD5 update |
| End | closing boundary → `image_upload_end` → `201` image JSON | → MD5 compare, `finish` (verify, select) → `200 {"result":"ok","restart":true}`, restart in 1 s |
| Failure | first failure wins; `upload_failed` with the C++ codes (400, 409, 413, 500, 507) and texts | same; `Update` codes and texts (§3.1) |
| Client gone | storage/OTA aborted, no answer | same |

The whole upload runs in the handler (§4.7 item 4 and decision 7.3). The loop sleeps one tick per
received chunk so the idle task of core 0 runs during long flash writes (TWDT idle check).

### 4.6 Assets, ETag/304, 410, log download

- Static assets: `tools/gen_web_assets.py` gets a Rust output mode (`--rust OUT.rs`) that writes the
  same gzip bytes (level 9, mtime 0) and ETags (CRC32 of the gzip bytes) as the C++ header, as
  `&'static [u8]` tables; the firmware passes the table to `Web`, tests pass their own. A GET with a
  matching `If-None-Match` gets `304` (Content-Length 0); otherwise `200` with `Content-Encoding:
  gzip`, `ETag`, `Cache-Control: no-cache`, sent from flash without a copy. *Implementation:* no
  `--rust` mode; `glue/build.rs` (feature `dashboard`) imports `gen_web_assets.py` and calls its
  functions (`collect`, `gzip_bytes`, the content types, the budget).
- 410 table and legacy aliases: core `legacy_http`, answered before the guard and the body.
- `GET /api/log`: one download at a time (409 `busy`), `logger.request_flush()`, then
  `begin_chunked` and `chunk` (Transfer-Encoding chunked as in C++, the chunk framing of
  `http_parse` sent with `httpd_send`) reading
  `events.1.log` then `events.log` through the response buffer. An open log file blocks the rotation
  (`rename` fails with EBUSY), as D§9 describes for C++. *Implementation:* the server serves one
  request at a time, so a second download waits in the backlog and the 409 is never sent (§4.7
  row 4).

### 4.7 Behaviour that changes

| # | Behaviour | C++ 2.1.7 | Rust | Proposal |
|---|---|---|---|---|
| 1 | Both response slots busy | 503 `busy` "response buffers in use" | never: one buffer, serial handlers | accept; API.md drops that 503 cause |
| 2 | A second JSON body while one is received | 409 `busy` "body" | never | accept |
| 3 | Refusal changed between guard and handler | 503 `retry` "state changed" | never (one call decides) | accept; API.md drops `retry` |
| 4 | Concurrency | requests interleaved by AsyncTCP; an upload or a log download did not hold others back | one request at a time: an upload (up to the OTA image size), a log download (≤ 144 KB) or a slow client (≤ 5 s per socket wait) delays the others, who wait in the backlog; nothing is refused | accept for logs; uploads: decision 7.3 |
| 5 | Connections | closed after every response; a fifth client's SYN was dropped (backlog 4) | HTTP/1.1 keep-alive; a fifth client purges the least recently used session | accept |
| 6 | Oversized request head | no limit | header block > 1024 B → 431, URI > 512 B → 414, unknown method string → 400, each with the server's text/html body | accept; 1024 B covers browsers (the device sets no cookies) |
| 7 | Malformed multipart (no closing boundary, epilogue, bytes after the body) | the library never handled the request: no answer until the client gave up | 400 `upload_failed` "incomplete file" / "no file in request" | accept |
| 8 | Header value parsing | AsyncWebServer took the text after `": "` (one space assumed) | esp_http_server's value without leading blanks | accept |
| 9 | A body that stalls | waited for the client | the glue gives up after 3 receive timeouts of 5 s in a row: no answer, upload aborted | accept |
| 10 | ESP upload while the new image is on trial | accepted (the devices never reached PENDING_VERIFY) | `409 upload_failed "image on trial"` (§6.5) | decision 7.5 |
| 11 | Switch back to the other image | not possible | `POST /api/system/ota/switch-back` (§6.3) | decision 7.6 |

Kept 1:1: every document and error body, status codes otherwise, Content-Length framing, the
chunked log, ETags, the 410 table, the guard, the limits, the reason phrases.

---

## 5. Tests and mutation gate

### 5.1 Harness (`glue/src/testkit`, `#[cfg(test)]`)

| C++ (`test/native/glue`, `tools/native/testkit`) | Rust |
|---|---|
| fork-per-case runner, file statics reset by the fork | no globals in `glue`: each `#[test]` builds a `FakeBoard` and a `Device`; cases run in parallel threads |
| `hal/` fakes of Arduino, ESP-IDF, FreeRTOS, AsyncWebServer, PubSubClient, LittleFS, NVS, Update | fakes of the ports of §1 only: `FakeClock`, `FakeWall` (POSIX TZ rules), `FakeUart` with a peer, pins, `FakeNvs` (typed entries, failure sets), `FakeFs` (tree, scripted failures per op/path, open handles, EBUSY on rename/remove of an open file, usage), `FakeTcp` (scripted peers), `FakeOta` (two slots, otadata, a bootloader without rollback), `FakeHeap` (scripted grants, recorded sizes), net fakes (links, IPs, events, SNTP syncs, ping answers), `FakeRtc` |
| `siblings/` fakes of the other glue modules | not needed: modules talk through `glue::shared` objects, which are plain data; a test uses the real shared objects and inspects them |
| `support/fake_stm.cpp` (golden protocol-3 replies) | `testkit/fake_stm.rs`, a peer of `FakeUart` |
| `fakes::http` request driver (AsyncWebServer parsing quirks) | `FakeRequest`: method, target, headers, body in segments, remote/local IP; records the answer; asserts exactly one answer |
| PubSubClient fake | `FakeBroker`: a `TcpStream` peer that decodes MQTT 3.1.1 packets, records CONNECT/PUBLISH/SUBSCRIBE and scripts CONNACK, inbound PUBLISH, drops and stalls |
| journal of side effects | journal of the fakes (same idea: ordered entries such as `gpio 15=0`, `nvs set vdmrev/boots`) |
| invariants after every case (no RTOS violation, no open critical section, every request answered once) | at `Device` drop: every `FakeRequest` answered once, no poisoned lock, no leaked file handle |

### 5.2 Suites

Glue tests follow PORTING.md: `test/native/glue/test_<stem>.cpp` → `glue/src/<module>/tests.rs`,
`test_<stem>__<part>.cpp` and `test_<stem>_<topic>.cpp` → `tests_<part>.rs` / `tests_<topic>.rs`.
Every case that tests the glue's own behaviour is ported with all its assertions; cases that test
how the glue copes with an Arduino library that is gone are retired, and their subject gets new
cases against the Rust mechanism (list approved by the operator, decision 7.4).

| C++ file (cases) | Covers | Rust | Port |
|---|---|---|---|
| `test_app.cpp` (29), `__mut` (4) | boot sequence, factory pin, tasks, queue, snapshot plumbing, app task, resources, heap guard, health; exact periods, never-blocking queue | `app/tests.rs`, `tests_mut.rs` | all; task creation is checked on `app::TASKS` and the spawn order |
| `test_app__eq.cpp` (1) | `xTaskGetHandle(nullptr)` | — | retired (FreeRTOS call gone) |
| `test_main.cpp` (4) | Arduino hooks | — | retired; "NRST released first" moves into `app/tests.rs` (journal order) |
| `test_stm_link.cpp` (25), `__mut` (7) | UART/NRST wiring, bounded reads, task loop against the fake STM, session port, a whole flash, discard bound, read limits, 1 s cadence, policy reset interval | `stm_link/tests*.rs` | all |
| `test_stm_service.cpp` (20) | scheduled calibration confirmed by the STM, NVS copy of targets, RTC records across software restarts, boot choice | `stm_service/tests.rs` | all (multi-boot) |
| `test_mqtt_client.cpp` (39), `__mut` (59), `__eq` (5), `__gate` (10) | session, LWT, subscriptions, inbound and retained leftovers, regulator, values, events, discovery run and list file, pass-by-pass layout, budgets, boundaries, back-off, plans, rare inputs, discovery memory and gate | `mqtt_client/tests*.rs` against `FakeBroker` | all; PubSubClient call counts become packet assertions |
| — | PubSubClient 2.8 behaviour | `mqtt_conn/tests.rs` (new) | new: packet bytes, CONNACK timeout, keepalive and -4, buffer limit, QoS-1 PUBACK, oversized packet, state codes |
| `test_net.cpp` (43), `test_net_edges.cpp` (37), `__eq` (1) | interfaces, events, WiFi fallback and back-off, time, reachability, watchdog stages, trial, clock, RTC restart count | `net/tests*.rs` | all; Arduino event ids become fake event counters |
| `test_ota.cpp` (41), `__mut` (14) | upload steps, restart scheduling and path, self-check, validation, rollback, edge timings | `ota/tests*.rs` | all; "rollback" assertions become boot-guard switch assertions |
| — | Arduino `Update` contract | `ota/update/tests.rs` (new) | new: codes, texts, MD5, magic byte |
| `test_storage.cpp` (16), `test_storage_config.cpp` (49), `test_storage_images.cpp` (24), `test_storage_values.cpp` (30) | mount, load table, cfgx, backups, import report, factory reset, files, image store, NVS values, legacy reader | `storage/tests_*.rs` | all |
| `test_logger.cpp` (20), `__mut` (3) | RAM log, console, file sink, rotation, gaps, failures, syslog, statistics | `logger/tests*.rs` | all |
| `test_web_server.cpp` (43), `__mut_a` (21), `__mut_a_views` (15), `__mut_b1` (20), `__mut_b2` (22), `__work` (14) | routing, guard, slots, assets, uploads, aliases, 2.1 routes, documents vs core, STM actions, config save, health, log download, body buffer, working set | `web_server/tests*.rs` | all except the slot pool (503 `busy` with two slots), the marks of refused bodies (409 `busy`), "state changed" (503 `retry`) and concurrent bodies; those subjects get cases for the serial server (§4.7 rows 1-3) |
| `test_web_server__eq.cpp` (7) | pipelined bytes, multipart quirks of the library, recycled request addresses | `http_parse/tests.rs` | retired as library quirks; new multipart and URL cases (framing across reads, missing closing boundary, two files, fields after the file, `%`/`+` decoding) |
| — | ArduinoJson behaviour | `json_body/tests.rs` (new) | new: golden corpus (§4.4) |
| `selftest.cpp` (39) | harness: isolation, multi-boot hand-over, invariants, fake behaviour | `testkit/tests.rs` | rewritten for the Rust fakes (same subjects) |
| — | boot guard | `boot_guard/tests.rs` (new) | new: every row of §6.2/§6.3/§6.6 |

### 5.3 Multi-boot simulation

- `FakeBoard` holds what a reset keeps: NVS, the LittleFS tree, the OTA slots with otadata, the RTC
  block. `Device::boot(&board, reset)` builds the glue on it and runs `app::setup`.
- `System::restart()` is `-> !`; the fake panics with a `Restarted` payload, and
  `testkit::run(|| ..)` catches it (`catch_unwind`) and hands the board to the next boot, as
  `glue::run` and `testkit::reboot` did.
- Reset kinds: `Software`, `Pin`, `Panic`, `TaskWdt` keep the RTC block; `PowerOn` fills it with
  0xA5. `System::reset_reason` reports the kind (`ESP_RST_SW`, `_EXT`, `_PANIC`, `_TASK_WDT`,
  `_POWERON`).
- The fake bootloader has no rollback support (the devices' bootloader): it boots the otadata
  slot when its image is valid, else the other valid slot, and ignores image states. A slot holds a
  glue image (an `AppId`), a foreign image (the C++ firmware: the boot is recorded, the case stops
  there) or a broken one.
- A case can crash a boot at any point (`board.crash(Reset::Panic)`), lose power between two NVS
  writes (scripted NVS failure + `PowerOn`), and run up to 8 boots (the C++ limit).

### 5.4 Mutation gate for `vdm-esp-glue`

- `bash tools/rust/docker.sh mutate software_esp32_rust vdm-esp-glue`, then
  `tools/rust/mutation_gate.py`: 95 % overall and per file, unviable mutants not counted, reports in
  `tools/rust/mutation/vdm-esp-glue.report.json`, equivalents with reasons in
  `tools/rust/mutation/equivalents/vdm-esp-glue.json`.
- Mutated: every file of `glue/src` except `#[cfg(test)]` modules (`testkit`, `tests*.rs`).
  `#[cfg_attr(test, mutants::skip)]` only for code the host cannot reach, with the reason; none is
  expected, because every hardware effect is behind a port.
- cargo-mutants runs the whole glue test suite per mutant (the C++ ran one `glue_<stem>` per file),
  so tests stay fast (the whole suite well below 30 s) and loops in tests are bounded (a mutant that
  breaks a loop ends as a timeout, which the gate counts as killed). Full runs are sharded
  (`--shard k/n`); development runs use `--in-diff`.
- `vdm-esp-fw` is outside the workspace and outside the gate. Review rule for its adapters: no
  state machine, no loop except FFI retries, no decision; a branch that is more than error mapping
  moves into the glue with a port method.

### 5.5 End to end in QEMU (`vdm-esp-fw`)

`tools/rust/esp/docker.sh qemu` runs the firmware (feature `qemu`: OpenETH in place of the
LAN8720) in Espressif's QEMU with the devices' bootloader and partition table and the C++ 2.1.7
release image in the other slot; the scenarios are listed in tools/rust/README.md, their details
and the limits of the C++ image in QEMU in the harness docstring (`tools/rust/esp/qemu/harness.py`).

Proven there (2026-10-07):

- Boot chain (risk 7.1): the devices' bootloader starts the ESP-IDF 5.5 image (DIO, 80 MHz, 4 MB
  header) from otadata as the C++ firmware leaves it after an upload; trial, confirmation after
  120 s of health, otadata VALID, a power cycle boots it confirmed; the switch back
  (`POST /api/system/ota/switch-back`), the boot limit (a panic after the guard: 3 counted boots,
  the 4th switches) and the boot deadline (a hang before the app thread) all start the C++
  firmware; the bootloader never touches the image states. The guard decides before the first
  LittleFS line and the first event of a boot (a partition of random bytes: the glue formats it,
  disk version 2.0, and the firmware comes up), so a failure in any later step is a counted boot.
  The network watchdog's restart counts as a boot (scenario netwatch). A trial that needs the
  STM and never sees it switches back after 15 min (restart reason 4, "rollback, missing stm")
  without another counted boot (scenario health, run on 2026-10-07; not in the default set).
- OTA Rust → C++: the C++ release image through `POST /api/ota/esp` (multipart, as the dashboard),
  written byte-identical into the empty slot, and started.
- Data interchange (risk 7.2): a LittleFS image of the C++ toolchain's mklittlefs with the C++
  layout is mounted, its config backup restored, its log appended to, and the result is read by
  mklittlefs and littlefs-python as disk version 2.0; an NVS written by ESP-IDF's generator with
  the C++ codec's blobs serves the same config as the C++ codec, and the config the Rust firmware
  saves decodes with the C++ codec (NVS page version 0xfe).
- HTTP: every GET route against the document structure of `mock_api.py`, the 404/405/410
  refusals, config dry run, save and refusal, an STM image upload and delete, the chunked log, the
  dashboard files (gzip, ETag, 304 without Content-Type).
- MQTT against Mosquitto: connect after a config save, status online, values, the HA discovery
  run (the broker holds every config the device reports), a reconnect after a broker restart.
- Load: `tools/loadtest.py` with 3 and 10 workers and config saves, no restart, no 503, the heap
  back after the load; the heap and stack figures of §2.3 and §2.1 come from these runs.

Not provable in QEMU, and where it is covered instead:

| Item | Why not | Covered by |
|---|---|---|
| The C++ firmware reading the NVS and LittleFS a Rust image wrote, on the device | its ESP-IDF 4.4.4 reads the flash in QIO, which this QEMU answers with garbage (it erases a valid NVS as unreadable, a valid LittleFS does not mount) | the C++ codec of `lib/core` compiled natively, ESP-IDF's NVS tools and mklittlefs on the flash files of the scenarios |
| A software restart after the first minute of uptime (a QEMU limit) | the next boot takes an interrupt without a handler in ESP-IDF's startup (`esp_timer_init`) and loops before main (a user restart at 30 s of uptime boots, at 70 s it loops); a restart into the C++ image after Ethernet ran also hung once in its startup. The chip resets its timer groups and its EMAC in `esp_restart` (`esp_restart_noos` in `esp_system/port/soc/esp32/system_internal.c`: `DPORT_TIMERS_RST` through `DPORT_PERIP_RST_EN_REG`, `DPORT_EMAC_RST` through `DPORT_CORE_RST_EN_REG`), the QEMU model does not | the scenarios end the QEMU process at every restart that leads into the C++ image or comes late, check the flash and power on a new one; the early restarts of the Rust image (boot limit, deadline) run through. On the device it is a first-flash check: after the first OTA to Rust let the image run at least 10 min, then a user restart and a power cycle must both come back, since a loop before `main` is the one failure the boot guard cannot catch |
| The C++ firmware's whole boot after a switch | its setup runs a factory reset (GPIO2 reads LOW here) on garbage flash reads; in most runs it mounts LittleFS and prints its first events within seconds, in others (mostly after a power-on) it stalls for minutes | the start is proven by the bootloader starting the C++ image and its ESP-IDF 4.4.4 startup line; its LittleFS mount and boot event are logged when they come |
| The C++ firmware uploading the Rust image (C++ → Rust OTA) | the C++ image panics in its Ethernet init (QEMU emulates OpenETH, not the EMAC), so it has no network | otadata written as the C++ upload leaves it (sequence 2, NEW); the C++ upload checks the image with Arduino's `Update` (magic byte, then ESP-IDF 4.4's `esp_image_verify` in `esp_ota_end`), the image format code the devices' bootloader of the same era runs when it loads the Rust image in QEMU |
| The LAN8720 adapter (EMAC, RMII clock on GPIO0, PHY reset on GPIO16) | no EMAC model | the operator's first flash |
| WiFi | no WiFi model | the operator's first flash |
| The STM link, an STM flash, the factory pin | no STM; GPIO2 reads LOW (the harness sets `frLatch`) | glue host tests with the fake STM (§5.1), Renode for the STM images |
| Stack and heap figures of the device | OpenETH buffers instead of the EMAC DMA, WiFi off, no STM traffic | the measuring campaign of §2.1 |

---

## 6. Boot guard

The devices' bootloader (serial-flashed with the legacy firmware) has no rollback support: an image
never reaches PENDING_VERIFY, so D§16's validation never runs on them. The boot guard does the
rollback in the application. It keeps D§16's health criteria (`OtaValidator`: net, http self-check,
stm when NVS `otaStm` said the link was up at the upload; 120 s healthy confirms, 15 min gives up)
and adds a boot limit, a boot deadline and a manual switch back.

State of the running image (each image runs the same machine when it boots):

| From | Event | To |
|---|---|---|
| boot | `AppId` = `otaOk` | Confirmed |
| boot | `AppId` ≠ `otaOk`, no trial record for it (or one that says `SwitchedBack`) | Trial, boot 1 |
| Trial, boot n | any reset before the confirmation | Trial, boot n + 1; above 3 → SwitchedBack, restart into the fallback |
| Trial | 120 s healthy, or a user restart while net, a fresh passing self-check (and stm when required) are up | Confirmed |
| Trial | 15 min without 120 s of health | SwitchedBack, restart into the fallback |
| Trial | no valid fallback, or the fallback is the image last switched away from | Confirmed (event 107 arg1 -3) |
| any | `POST /api/system/ota/switch-back` | restart into the fallback (SwitchedBack when on trial); from a confirmed image no image stays confirmed, so the target runs a trial with this image as its fallback |

### 6.1 Records

| Record | Where | Content |
|---|---|---|
| `otaOk` | NVS `vdmrev`, blob 16 B | "VDOK", the `AppId` of the confirmed image, CRC32; removed when that image is left by an upload or by a manual switch, so it runs a trial when it comes back (also after the C++ firmware ran in between) |
| `otaTrial` | NVS `vdmrev`, blob 32 B | "VDOT", version 1, state (`Trial`, `SwitchedBack`), boots, flags (bit0 stm required, bit1 an image switched away from is recorded), `AppId` of the image on trial, `AppId` of the last image the guard switched away from, fallback slot address, CRC32 |
| Guard mirror | RTC block, first record, 28 B | "VBGD", `AppId` on trial, boots, breadcrumb of the last switch (`AppId` switched away from, reason 1 boot limit / 2 health), CRC32 |
| Other RTC records | RTC block after the mirror | net watchdog count ("VNWD"), desired targets (46 B), ESP lease emulation, HA status: the C++ formats, each with its own check |

The C++ firmware ignores both NVS keys. The RTC layout of a C++ image and of a Rust image differ, so
an OTA between them loses the RTC records (each fails its check and counts as power-on garbage);
desired targets come from NVS, which the restart path writes before every restart.

`AppId` = the first 8 bytes of `esp_app_desc_t.app_elf_sha256` of a slot (`Ota::running`/`other`).
`esptool elf2image --elf-sha256-offset 0xb0` writes it; an image built without it has all zero
bytes there (every legacy 1.4.x image does) and names no build: the guard treats a running image
with it as unknown and stays out (`UNKNOWN_APP`, as without a readable description), and
`tools/rust/esp/docker.sh size` and `export` fail on such an image (`tools/rust/esp/app_id.py`).
A fallback is identified by its slot, so a legacy image stays a valid fallback.

### 6.2 Decision at boot

Runs in `main` right after the NRST release and the NVS init, before LittleFS, the network and the
threads.

| Step | Condition | Action |
|---|---|---|
| 1 | running `AppId` = `otaOk` | confirmed: drop a stale `otaTrial`; no trial. A breadcrumb in the RTC mirror (a switch away from another image) becomes event 107 arg1 -4, arg2 = its reason, once the logger runs |
| 2 | no `otaTrial` for this `AppId`, or one in state `SwitchedBack` (the failed image was uploaded again) | new trial: boots = 1, fallback = `Ota::other()`, stm required = NVS `otaStm`; the "switched away from" `AppId` carries over from the old record; `otaStm` is erased at every boot |
| 3 | `otaTrial` in state `Trial` for this `AppId` | boots = 1 + max(NVS boots, RTC mirror boots when its check and `AppId` match) |
| 4 | the fallback slot has no valid app (`other().app` is `None`), or it holds the image the guard last switched away from | no switch possible: confirm the running image (`otaOk` := running, `otaTrial` removed), event 107 arg1 -3 once the logger runs, as C++ "keep running this one" |
| 5 | boots > 3 | switch: `otaTrial.state` := `SwitchedBack`, breadcrumb (reason 1), `Ota::set_boot(fallback)`, `System::restart()`. A refused `set_boot` (image does not verify) → step 4 |
| 6 | otherwise | write `otaTrial` and the RTC mirror; the trial runs: `OtaValidator::begin(pending = true, stm required)` in `app::setup` |

A write that fails (NVS full or broken) does not stop the boot: the RTC mirror still counts boots
across software, panic and watchdog resets; a power cycle then restarts the count.

### 6.3 At run time

| Event | Action |
|---|---|
| `OtaValidator` → MarkValid (120 s healthy) | `otaOk` := running `AppId`, `otaTrial` removed, RTC mirror cleared, event 108 (seconds after boot) |
| `OtaValidator` → Rollback (15 min without 120 s of health) | restart reason 4 with the missing checks (D§16 path: STM EEPROM gate, target flush, log flush, MQTT offline); at its end `otaTrial` := `SwitchedBack`, breadcrumb (reason 2), `set_boot(fallback)`, restart; a refused `set_boot` → event 107 arg1 -3, trial ended as confirmed, keep running |
| User restart (reasons 0 and 3) during a trial | `confirmBeforeRestart`: net up, a passing loopback self-check younger than 30 s and (stm when required) → confirm first; otherwise the restart counts as a boot. C++ needed no self-check: its reason 0 came over HTTP, here `cmd/restart` of MQTT is reason 0 too and proves no web server |
| `POST /api/system/ota/switch-back` `{"confirm":"switch-back"}` | refused with 400 `confirm_required`, 409 `busy` (upload or flash), 409 `restarting`, 409 `no_fallback` (`other().app` is `None`); else restart reason 7 (new, Info) → at the end of the restart path `set_boot(fallback)` and restart; on trial `SwitchedBack` and the fallback becomes `otaOk`, from a confirmed image `otaOk` is removed (the target may be the image that failed its last trial or one that never ran: it runs a trial with this image as its fallback). Works in any state: it is the remedy for a confirmed image that misbehaves |
| ESP upload during a trial | refused: `409 upload_failed "image on trial"`; the upload would overwrite the fallback |
| Heap guard, network watchdog, TWDT or panic reset during a trial | counts as a boot |
| Boot deadline: `setup` has not finished (first app-task pass) 60 s after the start of `main` | `esp_timer` one-shot → `System::restart()`; counts as a boot |

Trace: the 15 min case logs `reboot_requested` reason 4 with the missing checks and flushes the log
before the switch, as D§16 does. The boot-limit case switches before the logger runs; a Rust
fallback image logs event 107 arg1 -4 from the breadcrumb, a C++ fallback image only its own `boot`
event (reset reason `sw`).

### 6.4 Place in `main`

1. NRST released (IO14 LOW, IO15 LOW; level before direction), the earliest point a Rust image has
   (Arduino used `initVariant`), on the bare pins (`stm_link::release_stm_reset`); the STM link
   takes the pins in step 5. R6 unchanged.
2. Console, `nvs_flash_init` (through the NVS adapter; erase-and-init only for the two errors
   Arduino-ESP32 erased on: no free pages, new version found).
3. Boot guard decision (§6.2). A switch restarts here, before any risky subsystem runs: before
   it `main` builds only the port adapters (plain values), so a boot that dies later is always
   counted.
4. `Shared` (the deadline reads its app state), then the boot deadline armed (60 s).
5. The modules of `app::wiring` over leaked `&'static` objects (the storage objects, the STM link
   with the UART2 driver, the system event loop and lwIP before the syslog socket of the sinks,
   the rest), then `app::setup`: the D§3 boot order (logger, factory pin, LittleFS, config, `boot`
   event, sinks, `stm_service`, net, OTA state with the trial, MQTT, watchdog), then the TWDT
   driver and the threads (`app::spawn_tasks`).
6. The first app-task pass disarms the deadline; `main` returns and its stack is freed.

### 6.5 Interactions

| With | Rule |
|---|---|
| OTA upload | goes to the fallback slot, so it is refused while a trial runs (decision 7.5) and allowed otherwise; the uploaded image's first boot finds a new `AppId` and starts its trial with the uploading image as fallback. `otaStm` is written by the restart path as in C++ and read once into `otaTrial`; the restart path also removes `otaOk` (the uploading image, installed again later by another firmware, runs a trial) |
| First Rust boot after the C++ firmware uploaded it | no `otaOk` (C++ never writes it) → trial with the C++ image as fallback |
| User restarts | §6.3; the restart path is unchanged otherwise |
| Network trial (D§16) | a network change during an OTA trial restarts with reason 0 → confirms the image when healthy; a network revert restarts with reason 5 → counts as a boot |
| Factory reset (HTTP or GPIO2) | keeps `frLatch`, `otaOk` and `otaTrial` (Rust only; a C++ factory reset erases them, and the next Rust boot validates itself again) |
| STM flash | a switch at run time waits like every restart (no restart during a flash); the boot-time switch happens before the STM link starts |
| Ping-pong | an automatic switch never goes back to the image that failed the last trial (§6.2 step 4); a manual switch is always allowed, and from a confirmed image its target runs a trial |

### 6.6 Coverage

| Case | Covered |
|---|---|
| Image panics, hits the TWDT, the interrupt watchdog or a brownout loop after `main` started | yes: boot limit (3 boots) |
| Image hangs in `setup` without a reset | yes: boot deadline |
| Image runs but has no working network, HTTP server or STM link (when it was up at the upload) | yes: 15 min |
| A hung HTTP server on a confirmed image | no (as C++: the self-check runs during a trial only) |
| Crash before `main`: bootloader handoff, flash mode or size, ESP-IDF startup, chip revision check, a component init function | **no**: the image loops in its early boot until a serial reflash. Covered only by the QEMU boot-chain test with the devices' bootloader and partition table before any device gets the image (risk 7.1) |
| A defect that shows after the 120 s confirmation (leak after days, a rare route) | no; heap guard and TWDT restart the image, the manual switch back is the remedy while HTTP works |
| A healthy-looking image that drives the valves wrong | no; manual switch back |
| Fallback image that does not verify | no switch (§6.2 step 4). The ESP-IDF bootloader tries the other slot when the selected image does not load; the QEMU test of risk 7.1 confirms it for the devices' bootloader |
| A genuine network outage during the 15 min | switches to the previous image although the new one was fine (as D§16 rolls back); the new image can be uploaded again |
| Three power cycles before the 120 s confirmation | counted as unconfirmed boots: the guard switches back although the image may be fine |

---

## 7. Open risks and decisions

Decided on 2026-10-06 (the operator may revert): 3 (a) synchronous uploads, checked with the
dashboard during an OTA in QEMU; 4 the retired cases as listed; 5 as written; 6 in the Rust
firmware only — the same route in the C++ firmware is a separate change on the operator's word;
10 parity; 13 as written, reviewed against the firmware spike. 11: `mosquitto` joins the test
image for the interop run.

1. **Risk: boot chain.** The devices boot through the legacy serial-flashed bootloader
   (`software_esp32/bootloader_dio_40m.bin`) and partition table (`software_esp32/partitions.bin`).
   An ESP-IDF 5.5 image that fails before `main` is a serial reflash in the cabinet. Gate: QEMU boot
   of that bootloader + table + C++ 2.1.7 → OTA to Rust → OTA back to C++, with the image header of
   the C++ build (DIO, 80 MHz, 4 MB), `CONFIG_BOOTLOADER_APP_ROLLBACK_ENABLE=n`. Passed on
   2026-10-07 within the limits of the C++ image in QEMU (§5.5).
2. **Risk: data interchange C++ ↔ Rust.** NVS pages (IDF 4.4 ↔ 5.5) and LittleFS must stay readable
   both ways, or the C++ formats LittleFS at its next mount. Binding LittleFS settings (Arduino-ESP32
   2.0.7's sdkconfig): page 256, read/write/lookahead 128, cache 512, block cycles 512, name length
   64, mtime in seconds, and `CONFIG_LITTLEFS_MULTIVERSION=y` + `CONFIG_LITTLEFS_DISK_VERSION_2_0=y`
   (esp_littlefs 1.22 writes disk format 2.1 otherwise). Gate: the QEMU run of item 1 with a filled
   NVS and LittleFS (config, backups, STM images, logs, discovery list). Passed on 2026-10-07; the
   C++ side read through its codec and tools outside QEMU (§5.5).
3. **Decision: uploads in the HTTP thread.** (a) Synchronous in the handler (§4.5): no extra RAM,
   other requests wait for the upload; (b) `httpd_req_async_handler_begin` and a worker thread
   spawned for the upload (~6 KB stack while it runs), so the dashboard keeps polling. Recommendation
   (a), measured with the dashboard during an OTA; (b) only if the dashboard misbehaves.
4. **Decision: retired glue tests.** The AsyncWebServer and Arduino specific cases of §5.2 (slot
   pool, marks, `retry`, concurrent bodies, library multipart quirks, Arduino hooks, `xTaskGetHandle`)
   are retired and replaced; every other glue case is ported with all assertions. Approve the list.
5. **Decision: boot guard parameters.** 3 boots, 15 min (D§16), boot deadline 60 s, trial detection
   by `AppId` against `otaOk`, no automatic switch back to the image that failed the last trial,
   ESP uploads refused during a trial (`409 upload_failed "image on trial"`). Recommendation: as
   written.
6. **Decision: new API surface.** `POST /api/system/ota/switch-back` (`{"confirm":"switch-back"}`;
   202 `{"result":"restarting"}`, 400 `confirm_required`, 409 `busy` / `restarting` /
   `no_fallback`), event 109 reason 7 (switch back, Info), event 107 arg1 -4 (the previous image
   failed its trial; arg2 1 boot limit, 2 health), `409 upload_failed "image on trial"`. Event numbers
   and arguments are only appended (D§13). Recommendation: add the route and the event values to the
   C++ firmware too (2.1.8), so API.md and DESIGN.md stay one contract and a C++ image can switch
   back to a Rust one.
7. **Risk: flash size.** The image must stay below 1,228,800 B (`tools/check_image_size.py`;
   `tools/rust/esp/docker.sh size` fails above it). Fixed: fat LTO, one codegen unit,
   `panic = "abort"`, `CONFIG_COMPILER_OPTIMIZATION_SIZE`, Bluetooth, SoftAP and IPv6 off, IDF logs
   from warnings up. Measured on 2026-10-07 with the whole firmware (default features: WiFi and
   the dashboard; the C++ 2.1.7 image has 1,154,960 B):

   | Option | Image | Headroom | Status |
   |---|---|---|---|
   | 1. first build: `opt-level = "s"`, std with its default features | 1,363,152 B | -134,352 B, also over the 1,310,720 B slot | replaced by 3 |
   | 2. std without its default features (`build-std-features = []`): no backtrace symbolizer (gimli, addr2line, demangler), which runs only with `RUST_BACKTRACE` set and needs the ELF file; a panic still prints its message, ESP-IDF its register dump | 1,210,944 B | 17,856 B | adopted |
   | 3. 2 + `opt-level = "z"` | 1,187,680 B | 41,120 B | adopted: the release build |
   | 4. 3 + `panic = "immediate-abort"` | 1,147,776 B | 81,024 B | not adopted: a Rust panic no longer prints its message, only the register dump; the operator's call |
   | 5. 3 without WiFi (`nowifi`) | 822,304 B | 406,496 B | reference only: WiFi costs about 365 KB |

   Largest parts of 3: the Rust object (core, glue, std, esp-idf-svc; one LTO unit) about 500 KB
   with 42.6 KB of gzipped dashboard, libnet80211 117 KB, mbedcrypto 71 KB, libpp 69 KB, lwIP
   58 KB, libc 54 KB. Left without functional loss: `CONFIG_NEWLIB_NANO_FORMAT` (the ROM printf).
   The finished firmware with option 3: 1,187,456 B (headroom 41,344 B); without WiFi 823,104 B.
8. **Risk: RAM.** The QEMU figures of §2.3 are indicative only; the acceptance of §2.3 on the
   device decides.
   The httpd header scratch is reallocated in 128 B steps per request (ESP-IDF 5.5), the only
   per-request heap churn left; its effect on fragmentation shows in `minLargest` under the load
   test.
9. **Risk: HTTP serialization.** A stalled or slow client holds the only HTTP thread up to 5 s per
   socket wait (§4.7 row 4). Accepted for a LAN device; the dashboard's parallel pollers wait instead
   of getting 503.
10. **Decision: ArduinoJson parity.** `json_body` reproduces ArduinoJson 6.21.6 for the small bodies
    (error names, 32-slot limit, leniencies) so the glue tests port unchanged. Alternative: a strict
    parser with other error details for malformed bodies. Recommendation: parity.
11. **Risk: MQTT client.** `mqtt_conn` replaces a mature library. Mitigation: the PubSubClient cases
    of §5.2 plus an interop run against Mosquitto (connect, LWT, retained clear, keepalive, broker
    restart); the container image of `tools/rust` gets `mosquitto` for it.
12. **Risk: stack sizes.** The sizes come from the QEMU peaks (§2.1), without an STM, WiFi or an
    STM flash; the measuring campaign on the device is part of the release checklist, and
    `StackLow` stays on.
13. **Decision: sdkconfig values that the glue relies on** (`firmware/sdkconfig.defaults`):
    `CONFIG_FREERTOS_HZ=1000`; TWDT 30 s with panic, idle check of core 0 only; flash DIO, 80 MHz,
    4 MB; `CONFIG_BOOTLOADER_APP_ROLLBACK_ENABLE=n`; `CONFIG_LWIP_MAX_SOCKETS=16`; `CONFIG_LWIP_IPV6=n`;
    `CONFIG_LWIP_SNTP_MAX_SERVERS=1`, `CONFIG_LWIP_SNTP_UPDATE_DELAY=10800000` (Arduino-ESP32 2.0.7;
    the IDF 5.5 default is 1 h) and the SNTP startup delay off; WiFi 4 static RX buffers and dynamic
    TX buffers (D§9), `CONFIG_ESP_WIFI_SOFTAP_SUPPORT=n`; Ethernet DMA 10 + 10 × 512 B;
    `CONFIG_ESP_SYSTEM_EVENT_TASK_STACK_SIZE=2560` and `CONFIG_LWIP_TCPIP_TASK_STACK_SIZE=2560`
    (+ 512 each); `CONFIG_ESP_MAIN_TASK_STACK_SIZE=32768` (§2.1); `CONFIG_SPIRAM=n`,
    `CONFIG_BT_ENABLED=n`; the LittleFS values of item 2; HTTPD header 1024 B, URI 512 B.
    Recommendation: these values, reviewed with the firmware spike; the firmware sets them.
14. **Risk: crate sources.** docs.rs has no build of esp-idf-svc 0.53.0 or esp-idf-hal 0.47.0, and
    both moved into the esp-rs/esp-idf monorepo (the esp-idf-svc repository is archived since
    2026-09-19). The API names in §1.2 and §4.1 were checked against the published 0.53.0 / 0.47.0
    sources and ESP-IDF v5.5.5; the pinned versions are not affected.
