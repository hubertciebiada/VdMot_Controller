//! VdMot Revamped ESP32 firmware in Rust: `main` (docs/rust/GLUE-DESIGN-ESP.md 2.1 and 6.4).
//!
//! Every decision lives in the glue (`vdm-esp-glue`, host-tested); this crate holds the port
//! adapters (`adapters`) and the boot: the STM released from reset on the bare pins, NVS, the
//! boot guard's decision (a switch restarts here), the boot deadline, then the modules of
//! `app::wiring` built over leaked `&'static` objects in the order of DESIGN.md section 3,
//! `app::setup`, the threads of `app::TASKS`; then `main` returns and its stack is freed. Nothing
//! that can fail or hang runs before the guard: a boot that dies there is never counted. The HTTP
//! server's task starts in the app thread once the network has an address.
//!
//! Features: `wifi` (default; without it the station cannot be built), `qemu` (OpenETH of
//! Espressif QEMU instead of the LAN8720), and the boot guard tests of tools/rust/esp/qemu:
//! `fail-boot` (a panic right after the guard counted the boot) and `hang-setup` (the boot never
//! reaches the app thread: the boot deadline restarts).

// the ESP-IDF cfgs (esp_idf_*) come from the build script, unknown to check-cfg
#![allow(unexpected_cfgs)]

mod adapters;

use std::sync::{Mutex, PoisonError};

use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::hal::task::watchdog::{TWDTConfig, TWDTDriver};
use esp_idf_svc::netif::NetifStack;
use esp_idf_svc::sys;
use vdm_esp_glue::app::wiring::{self, FwWeb, Modules, OtaWire, Ports, Threads, WebBegin, Wire};
use vdm_esp_glue::app::{self, TASK_WDT_TIMEOUT_S};
use vdm_esp_glue::boot_guard::{BootGuard, BootVerdict, BOOT_DEADLINE_MS};
use vdm_esp_glue::port::HttpRequest;
use vdm_esp_glue::shared::Shared;
use vdm_esp_glue::stm_link::release_stm_reset;

use adapters::{
    AlwaysGrant, EspClock, EspNvs, EspOta, EspSpawner, EspSystem, EspWall, EthPort, Fw,
    HttpServerPort, InPin, LittleFs, OutPin, PingPort, RomMd5, RtcBlock, Serve, SntpPort, Stdout,
    StmUart, Tcp, UdpPort, WifiPort,
};

esp_idf_sys::esp_app_desc! {}

/// A value that lives as long as the firmware.
fn leak<T>(v: T) -> &'static T {
    Box::leak(Box::new(v))
}

/// The web server behind its lock: only the HTTP thread takes it.
struct WebServe(Mutex<FwWeb<'static, Fw>>);

impl Serve for WebServe {
    fn serve(&self, req: &mut dyn HttpRequest) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .handle(req);
    }
}

fn main() {
    esp_idf_sys::link_patches();
    println!("vdm-esp-fw {} boot", env!("CARGO_PKG_VERSION"));

    let Ok(p) = Peripherals::take() else {
        panic!("peripherals taken twice");
    };

    // 1. the STM released from reset on the bare pins (IO14, then IO15 LOW; level before
    //    direction), before anything that can take time or fail: R6, the IO15 pull-up holds it
    //    while the ESP boots. The pins go into the STM link later.
    let mut nrst = OutPin::new(p.pins.gpio15.degrade_output());
    let mut boot0 = OutPin::new(p.pins.gpio14.degrade_output());
    release_stm_reset(&mut boot0, &mut nrst);

    // the ports every thread shares (plain values)
    let clock = leak(EspClock);
    let system = leak(EspSystem);
    let nvs = leak(EspNvs);
    let ota = leak(EspOta::new());
    let rtc = leak(RtcBlock::new());
    let ports: Ports<'static, Fw> = Ports {
        clock,
        wall: leak(EspWall::new()),
        console: leak(Stdout),
        nvs,
        fs: leak(LittleFs),
        tcp: leak(Tcp),
        ota,
        system,
        heap: leak(AlwaysGrant),
        rtc,
    };

    // 2. NVS (erased and initialised again only for the two errors Arduino-ESP32 erased on)
    let nvs_err = nvs.init();
    if nvs_err != sys::ESP_OK {
        println!("nvs: init error {nvs_err}, running without the stored settings");
    }

    // 3. the boot guard: a switch to the fallback restarts here. Every failure from here on
    //    ends a boot the guard counted.
    let (guard, report) = BootGuard::boot(nvs, ota, rtc, system);
    match report.verdict {
        BootVerdict::Confirmed => println!("boot guard: confirmed"),
        BootVerdict::Trial {
            boots,
            stm_required,
        } => println!("boot guard: trial, boot {boots} of 3, stm required {stm_required}"),
    }
    if cfg!(feature = "fail-boot") {
        panic!("fail-boot: injected failure after the boot guard");
    }

    // 4. the boot deadline: no first pass of the app thread within 60 s restarts (a boot the
    //    guard counts); it reads the app state of the shared objects
    let shared: &'static Shared = leak(Shared::new());
    let app_shared = &shared.app;
    if !adapters::arm_boot_deadline(
        BOOT_DEADLINE_MS,
        Box::new(move || app::boot_deadline(app_shared, system)),
    ) {
        println!("boot deadline not armed");
    }
    if cfg!(feature = "hang-setup") {
        // the boot never reaches the app thread
        loop {
            esp_idf_svc::hal::delay::FreeRtos::delay_ms(1000);
        }
    }

    // 5. the modules of the threads and the app (DESIGN.md section 3): the storage objects, the
    //    STM link with Serial2, the event loop and lwIP before the first socket (the syslog
    //    socket of the sinks, as Arduino's initArduino), the rest
    let storage = leak(wiring::storage(ports, shared));
    let tx = p.pins.gpio17.degrade_output();
    let Some(uart) = StmUart::new(p.uart2, tx, p.pins.gpio5.degrade_input_output()) else {
        panic!("UART2 not available");
    };
    let mut link = wiring::stm_link(ports, shared, storage, uart, nrst, boot0);
    let Ok(sysloop) = EspSystemEventLoop::take() else {
        panic!("no system event loop");
    };
    if NetifStack::initialize().is_err() {
        panic!("no TCP/IP stack");
    }
    let logger = leak(ports.logger(shared));
    let sinks = leak(wiring::sinks(ports, logger, shared, UdpPort::new()));
    let service = leak(Mutex::new(wiring::stm_service(ports, shared, storage)));
    let mut mqtt = wiring::mqtt(ports, shared, storage);
    let net = wiring::net(
        ports,
        shared,
        storage,
        EthPort::new(sysloop.clone()),
        wifi_port(sysloop.clone(), p.modem),
        SntpPort::new(),
        PingPort::new(),
    );
    let ota_svc = wiring::ota(
        ports,
        shared,
        OtaWire::new(Wire::new(ports, shared, storage), sinks, service),
        guard,
    );
    let upload = wiring::web_upload(ports, shared, storage, sinks, service, RomMd5::new());
    let web = wiring::web(
        ports,
        shared,
        storage,
        upload,
        vdm_esp_glue::web_server::assets::DASHBOARD,
    );
    let serve: &'static WebServe = leak(WebServe(Mutex::new(web)));
    let modules = Modules::new(
        Wire::new(ports, shared, storage),
        sinks,
        service,
        net,
        ota_svc,
        report,
        WebBegin::new(HttpServerPort::new(serve)),
    );
    let Some(factory_pin) = InPin::new(p.pins.gpio2.degrade_input_output()) else {
        panic!("IO2 not available");
    };
    let mut app = wiring::app(ports, shared, factory_pin, modules);
    app.setup(&mut Threads {
        link: &mut link,
        mqtt: &mut mqtt,
    });

    // 6. the task watchdog (30 s, panic, idle task of core 0) and the threads
    // the idle tasks watched come from sdkconfig (CONFIG_ESP_TASK_WDT_CHECK_IDLE_TASK_CPU0)
    let mut twdt_conf = TWDTConfig::new();
    twdt_conf.duration = core::time::Duration::from_secs(u64::from(TASK_WDT_TIMEOUT_S));
    twdt_conf.panic_on_trigger = true;
    let Ok(twdt) = TWDTDriver::new(p.twdt, &twdt_conf) else {
        panic!("task watchdog not configured");
    };
    app::spawn_tasks(&mut EspSpawner::new(twdt), clock, link, app, mqtt);

    // SAFETY: plain getter of the calling task.
    let free = unsafe { sys::uxTaskGetStackHighWaterMark(core::ptr::null_mut()) };
    println!("main: done, stack min free {free} B");
}

#[cfg(feature = "wifi")]
fn wifi_port(
    sysloop: EspSystemEventLoop,
    modem: esp_idf_svc::hal::modem::Modem<'static>,
) -> WifiPort {
    WifiPort::new(sysloop, modem)
}

#[cfg(not(feature = "wifi"))]
fn wifi_port(
    _sysloop: EspSystemEventLoop,
    _modem: esp_idf_svc::hal::modem::Modem<'static>,
) -> WifiPort {
    WifiPort
}
