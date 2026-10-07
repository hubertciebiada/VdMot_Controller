//! Clock (esp_timer, FreeRTOS delays) and WallClock (newlib time and TZ).

use std::sync::{Mutex, PoisonError};

use esp_idf_svc::hal::delay::FreeRtos;
use esp_idf_svc::sys;
use vdm_esp_glue::port::{Clock, LocalTime, WallClock};

/// `millis()`, the uptime and task delays.
pub struct EspClock;

impl Clock for EspClock {
    fn now_ms(&self) -> u32 {
        // SAFETY: plain getter. Wraps at 2^32 like millis().
        (unsafe { sys::esp_timer_get_time() } / 1000) as u32
    }
    fn uptime_s(&self) -> u32 {
        // SAFETY: plain getter.
        (unsafe { sys::esp_timer_get_time() } / 1_000_000) as u32
    }
    fn sleep_ms(&self, ms: u32) {
        // rounds up to ticks (1 ms at CONFIG_FREERTOS_HZ=1000); 0 yields. Never
        // std::thread::sleep: ESP-IDF's usleep busy-waits below one tick.
        FreeRtos::delay_ms(ms);
    }
}

/// `gettimeofday`, `TZ` + `tzset`, `localtime_r`. newlib keeps the TZ state global: one lock
/// around a zone change and every conversion.
pub struct EspWall {
    tz: Mutex<()>,
}

impl EspWall {
    pub const fn new() -> Self {
        EspWall { tz: Mutex::new(()) }
    }
}

impl WallClock for EspWall {
    fn epoch(&self) -> i64 {
        let mut tv = sys::timeval::default();
        // SAFETY: valid out pointer, no time zone wanted.
        unsafe { sys::gettimeofday(&mut tv, core::ptr::null_mut()) };
        tv.tv_sec
    }
    fn set_time_zone(&self, posix: &str) {
        let _g = self.tz.lock().unwrap_or_else(PoisonError::into_inner);
        let mut buf = [0u8; 96];
        let Some(rule) = super::c_name(posix, &mut buf) else {
            return;
        };
        // SAFETY: NUL-terminated strings; newlib copies the value.
        unsafe {
            sys::setenv(c"TZ".as_ptr(), rule.as_ptr(), 1);
            sys::tzset();
        }
    }
    fn local_time(&self, epoch: i64) -> Option<LocalTime> {
        let _g = self.tz.lock().unwrap_or_else(PoisonError::into_inner);
        let t: sys::time_t = epoch as sys::time_t;
        let mut tm = sys::tm::default();
        // SAFETY: valid in and out pointers; localtime_r returns null on failure.
        if unsafe { sys::localtime_r(&t, &mut tm) }.is_null() {
            return None;
        }
        Some(LocalTime {
            valid: true,
            year: (tm.tm_year + 1900) as u16,
            month: (tm.tm_mon + 1) as u8,
            mday: tm.tm_mday as u8,
            wday: tm.tm_wday as u8,
            hour: tm.tm_hour as u8,
            minute: tm.tm_min as u8,
            second: tm.tm_sec as u8,
            epoch,
        })
    }
}
