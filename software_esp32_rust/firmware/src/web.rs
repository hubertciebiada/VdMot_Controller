//! HTTP server of the spike (ESP-IDF esp_http_server through esp-idf-svc):
//! - `GET /api/health`: version, uptime, heap, network, boot guard, storage; answering it is the
//!   HTTP health evidence of the boot guard;
//! - `POST /api/ota`: the raw app image as the body (Content-Length), streamed into the inactive
//!   slot (EspOta), validated by esp_ota_end, selected, restart after 1 s; refused while the
//!   running image is on trial;
//! - `POST /api/system/switch-back`: selects the other slot when its image validates, restart
//!   after 1 s.

use core::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use esp_idf_svc::http::server::{Configuration, EspHttpConnection, EspHttpServer, Request};
use esp_idf_svc::http::Method;
use esp_idf_svc::io::{EspIOError, Write};
use esp_idf_svc::ota::EspOta;
use esp_idf_sys as sys;

use crate::{boot_guard, net};

/// Largest image the slot takes (app0/app1 are 1280 KiB).
const SLOT_BYTES: usize = 0x14_0000;

static UPLOAD_ACTIVE: AtomicBool = AtomicBool::new(false);
/// Storage facts gathered at boot, shown by /api/health.
pub static STORAGE_JSON: OnceLock<String> = OnceLock::new();

pub fn start() -> Result<EspHttpServer<'static>, EspIOError> {
    let conf = Configuration { stack_size: 8192, max_open_sockets: 4, ..Default::default() };
    let mut server = EspHttpServer::new(&conf)?;
    server.fn_handler("/api/health", Method::Get, |req| -> Result<(), EspIOError> {
        let body = health_json();
        req.into_response(200, None, &[("Content-Type", "application/json")])?
            .write_all(body.as_bytes())?;
        boot_guard::note_http_answered();
        Ok(())
    })?;
    server.fn_handler("/api/ota", Method::Post, ota_upload)?;
    server.fn_handler("/api/system/switch-back", Method::Post, |req| -> Result<(), EspIOError> {
        match boot_guard::select_other_slot("http") {
            Ok(slot) => {
                reply(req, 200, &format!("switching back to {slot}, restarting\n"))?;
                restart_soon();
            }
            Err(e) => reply(req, 409, &format!("{e}\n"))?,
        }
        Ok(())
    })?;
    Ok(server)
}

fn reply(req: Request<&mut EspHttpConnection<'_>>, status: u16, text: &str) -> Result<(), EspIOError> {
    req.into_response(status, None, &[("Content-Type", "text/plain")])?.write_all(text.as_bytes())?;
    Ok(())
}

fn restart_soon() {
    let _ = std::thread::Builder::new().stack_size(3072).spawn(|| {
        std::thread::sleep(Duration::from_millis(1000));
        // SAFETY: plain restart.
        unsafe { sys::esp_restart() };
    });
}

fn ota_upload(mut req: Request<&mut EspHttpConnection<'_>>) -> Result<(), EspIOError> {
    if boot_guard::on_trial() {
        return reply(req, 409, "running image on trial, upload refused until it is confirmed\n");
    }
    let len: usize = req
        .connection()
        .header("Content-Length")
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(0);
    if len == 0 || len > SLOT_BYTES {
        return reply(req, 413, "Content-Length must be 1..1310720\n");
    }
    if UPLOAD_ACTIVE.swap(true, Ordering::SeqCst) {
        return reply(req, 409, "another upload is running\n");
    }
    let result = stream_image(&mut req, len);
    UPLOAD_ACTIVE.store(false, Ordering::SeqCst);
    match result {
        Ok(()) => {
            println!("ota: {len} B written and selected, restarting");
            reply(req, 200, &format!("ok, {len} B, restarting\n"))?;
            restart_soon();
            Ok(())
        }
        Err(e) => {
            println!("ota: failed: {e}");
            reply(req, 400, &format!("{e}\n"))
        }
    }
}

fn stream_image(req: &mut Request<&mut EspHttpConnection<'_>>, len: usize) -> Result<(), String> {
    let mut ota = EspOta::new().map_err(|e| format!("ota: {e}"))?;
    let mut update = ota.initiate_update_with_known_size(len).map_err(|e| format!("ota begin: {e}"))?;
    let mut buf = vec![0u8; 4096];
    let mut written = 0usize;
    while written < len {
        let want = buf.len().min(len - written);
        let n = match req.read(&mut buf[..want]) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) => {
                let _ = update.abort();
                return Err(format!("receive: {e:?}"));
            }
        };
        if let Err(e) = update.write(&buf[..n]) {
            let _ = update.abort();
            return Err(format!("flash write: {e}"));
        }
        written += n;
    }
    if written != len {
        let _ = update.abort();
        return Err(format!("body ended after {written} of {len} B"));
    }
    // esp_ota_end verifies the image (segments, checksum, SHA-256), then the slot is selected
    update.complete().map_err(|e| format!("image rejected: {e}"))
}

fn health_json() -> String {
    // SAFETY: plain getters.
    let (free, min_free, largest) = unsafe {
        (
            sys::heap_caps_get_free_size(sys::MALLOC_CAP_8BIT),
            sys::heap_caps_get_minimum_free_size(sys::MALLOC_CAP_8BIT),
            sys::heap_caps_get_largest_free_block(sys::MALLOC_CAP_8BIT),
        )
    };
    let ip = net::eth_ip().map_or_else(|| "null".to_string(), |ip| format!("\"{ip}\""));
    format!(
        "{{\"version\":\"{}\",\"uptimeS\":{},\"heap\":{{\"free\":{free},\"minFree\":{min_free},\"largest\":{largest}}},\"ethIp\":{ip},\"slot\":\"{}\",\"guard\":{},\"storage\":{}}}\n",
        env!("CARGO_PKG_VERSION"),
        boot_guard::uptime_s(),
        running_slot(),
        boot_guard::status_json(),
        STORAGE_JSON.get().map_or("null", String::as_str),
    )
}

fn running_slot() -> String {
    // SAFETY: plain lookup; the label is a NUL-terminated array of the partition record.
    unsafe {
        let p = sys::esp_ota_get_running_partition();
        if p.is_null() {
            return "?".into();
        }
        core::ffi::CStr::from_ptr((*p).label.as_ptr()).to_string_lossy().into_owned()
    }
}
