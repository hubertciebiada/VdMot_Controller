//! VdMot Revamped ESP32 firmware in Rust: the risk spike.
//!
//! Exercises what the port depends on, at a realistic image size, and the boot guard that makes
//! an OTA update safe on devices whose bootloader has no app rollback:
//! the boot guard (first, before any driver), the STM reset release of the C++ firmware, NVS
//! (`vdmrev` read only, never erased), LittleFS (the C++ firmware's files), the STM UART
//! (UART2, RX IO5, TX IO17, 115200), Ethernet (LAN8720 RMII, or OpenETH under QEMU), the WiFi STA
//! driver (feature `wifi`), SNTP, esp_http_server with /api/health, the OTA upload and the
//! switch-back route.
//!
//! Features: `wifi` (default), `qemu` (OpenETH, marker file on LittleFS), `fail-boot` (panics right
//! after the boot guard counted the boot) and `short-deadline` (confirmation within 60 s): the
//! boot guard tests of tools/rust/esp/qemu.

// the ESP-IDF cfgs (esp_idf_*) come from the build script, unknown to check-cfg
#![allow(unexpected_cfgs)]

mod boot_guard;
mod net;
mod storage;
mod web;

use std::time::Duration;

use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::gpio::{AnyIOPin, PinDriver};
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::hal::uart::{config::Config as UartConfig, UartDriver};
use esp_idf_svc::hal::units::Hertz;

esp_idf_sys::esp_app_desc! {}

fn main() {
    esp_idf_sys::link_patches();
    println!("vdm-esp-fw {} boot", env!("CARGO_PKG_VERSION"));

    // First, before any driver: count this boot of an unconfirmed image, switch back when its
    // attempts are used up.
    boot_guard::start();

    #[cfg(feature = "fail-boot")]
    panic!("fail-boot: injected failure after the boot guard");

    #[allow(unreachable_code)]
    run();
}

fn run() {
    let Ok(peripherals) = Peripherals::take() else {
        panic!("peripherals taken twice");
    };
    let pins = peripherals.pins;

    // As the C++ firmware's initVariant(): STM BOOT0 (IO14) LOW, then NRST released (IO15 LOW
    // through the inverting transistor), so the STM never stays in reset after an ESP boot.
    let mut stm_boot0 = PinDriver::output(pins.gpio14).ok();
    let mut stm_reset = PinDriver::output(pins.gpio15).ok();
    if let Some(p) = stm_boot0.as_mut() {
        let _ = p.set_low();
    }
    if let Some(p) = stm_reset.as_mut() {
        let _ = p.set_low();
    }

    let sysloop = match EspSystemEventLoop::take() {
        Ok(s) => s,
        Err(e) => panic!("system event loop: {e}"),
    };

    let nvs_summary = match storage::nvs_partition() {
        Ok(part) => storage::vdmrev_summary(&part),
        Err(e) => {
            println!("nvs: not available ({e}), left as it is");
            format!("{{\"open\":false,\"error\":{}}}", e.code())
        }
    };
    println!("nvs: vdmrev {nvs_summary}");

    let fs = match storage::mount_fs() {
        Ok(fs) => {
            let files = storage::list_fs();
            println!("fs: mounted {}, {} entries", storage::FS_ROOT, files.len());
            for (name, len) in &files {
                println!("fs:   {name} {len}");
            }
            #[cfg(feature = "qemu")]
            if let Err(e) = storage::append_marker(&format!("boot uptime {} s", boot_guard::uptime_s())) {
                println!("fs: marker not written: {e}");
            }
            Some((fs, files.len()))
        }
        Err(e) => {
            println!("fs: mount failed ({e}), not formatted");
            None
        }
    };
    let fs_json = fs.as_ref().map_or_else(
        || "{\"mounted\":false}".to_string(),
        |(_, n)| format!("{{\"mounted\":true,\"entries\":{n}}}"),
    );
    let _ = web::STORAGE_JSON.set(format!("{{\"nvs\":{nvs_summary},\"fs\":{fs_json}}}"));

    let uart = UartDriver::new(
        peripherals.uart2,
        pins.gpio17,
        pins.gpio5,
        Option::<AnyIOPin>::None,
        Option::<AnyIOPin>::None,
        &UartConfig::default().baudrate(Hertz(115_200)),
    );
    match &uart {
        Ok(_) => println!("uart: UART2 open (RX IO5, TX IO17, 115200)"),
        Err(e) => println!("uart: open failed ({e})"),
    }

    let eth = net::start_ethernet(&sysloop);
    match &eth {
        Ok(_) => println!("net: ethernet started"),
        Err(e) => println!("net: ethernet failed ({e})"),
    }

    #[cfg(feature = "wifi")]
    let wifi = match net::init_wifi(peripherals.modem, &sysloop) {
        Ok(w) => {
            println!("net: wifi STA driver initialised (not started)");
            Some(w)
        }
        Err(e) => {
            println!("net: wifi init failed ({e})");
            None
        }
    };

    let sntp = net::start_sntp();
    if let Err(e) = &sntp {
        println!("net: sntp failed ({e})");
    }

    let server = web::start();
    match &server {
        Ok(_) => println!("web: http server on port 80"),
        Err(e) => println!("web: http server failed ({e:?})"),
    }

    // Everything above lives as long as the firmware: the main task only reports.
    let mut last_report = 0;
    loop {
        std::thread::sleep(Duration::from_secs(1));
        let up = boot_guard::uptime_s();
        if up >= last_report + 60 {
            last_report = up;
            println!("alive: uptime {up} s, guard {}", boot_guard::status_json());
        }
        #[cfg(feature = "wifi")]
        let _ = &wifi;
        let _ = (&fs, &uart, &eth, &sntp, &server, &stm_boot0, &stm_reset);
    }
}
