//! The chip (restart, reset reason, heap, task stacks, factory MAC), the heap gate, the RTC block
//! and the console.

use core::mem::MaybeUninit;
use core::ptr::{addr_of, addr_of_mut};
use std::io::Write;
use std::sync::{Mutex, PoisonError};

use esp_idf_svc::sys;
use vdm_esp_glue::app::RTC_LEN;
use vdm_esp_glue::port::{Console, HeapGate, HeapStats, Rtc, System};

/// The chip.
pub struct EspSystem;

impl System for EspSystem {
    fn restart(&self) -> ! {
        esp_idf_svc::hal::reset::restart()
    }
    fn reset_reason(&self) -> u8 {
        // SAFETY: plain getter; the numbers 1..10 are the ones of IDF 4.4.
        unsafe { sys::esp_reset_reason() as u8 }
    }
    fn heap(&self) -> HeapStats {
        let caps = sys::MALLOC_CAP_8BIT;
        // SAFETY: plain getters.
        unsafe {
            HeapStats {
                free: sys::heap_caps_get_free_size(caps) as u32,
                min_free: sys::heap_caps_get_minimum_free_size(caps) as u32,
                largest: sys::heap_caps_get_largest_free_block(caps) as u32,
            }
        }
    }
    fn stack_min_free(&self, task: &str) -> Option<u32> {
        let mut b = [0u8; 16];
        let name = super::c_name(task, &mut b)?;
        // SAFETY: NUL-terminated name; the handle is used at once (tasks are never deleted).
        unsafe {
            let h = sys::xTaskGetHandle(name.as_ptr());
            if h.is_null() {
                return None;
            }
            // bytes on ESP-IDF (StackType_t is uint8_t)
            Some(sys::uxTaskGetStackHighWaterMark(h) as u32)
        }
    }
    fn base_mac(&self) -> [u8; 6] {
        let mut mac = [0u8; 6];
        // SAFETY: 6-byte buffer.
        unsafe { sys::esp_efuse_mac_get_default(mac.as_mut_ptr()) };
        mac
    }
}

/// Always grants: the glue then allocates with `try_reserve_exact`.
pub struct AlwaysGrant;

impl HeapGate for AlwaysGrant {
    fn grant(&self, _bytes: usize) -> bool {
        true
    }
}

// RTC slow memory that the startup code does not initialise: kept by esp_restart, panic and
// watchdog resets, garbage after a power-on (each record has its own check). The layout is the
// glue's (app::RTC_*).
#[link_section = ".rtc_noinit.vdm"]
static mut RTC_BLOCK: MaybeUninit<[u8; RTC_LEN]> = MaybeUninit::uninit();

/// The RTC block of the glue's records.
pub struct RtcBlock {
    lock: Mutex<()>,
}

impl RtcBlock {
    pub const fn new() -> Self {
        RtcBlock {
            lock: Mutex::new(()),
        }
    }
}

impl Rtc for RtcBlock {
    fn load(&self, offset: usize, out: &mut [u8]) {
        let _g = self.lock.lock().unwrap_or_else(PoisonError::into_inner);
        let base = addr_of!(RTC_BLOCK) as *const u8;
        for (i, b) in out.iter_mut().enumerate() {
            let at = offset + i;
            if at < RTC_LEN {
                // SAFETY: inside the block; volatile, the startup code does not initialise it.
                *b = unsafe { core::ptr::read_volatile(base.add(at)) };
            }
        }
    }
    fn store(&self, offset: usize, data: &[u8]) {
        let _g = self.lock.lock().unwrap_or_else(PoisonError::into_inner);
        let base = addr_of_mut!(RTC_BLOCK) as *mut u8;
        for (i, b) in data.iter().enumerate() {
            let at = offset + i;
            if at < RTC_LEN {
                // SAFETY: inside the block, under the lock.
                unsafe { core::ptr::write_volatile(base.add(at), *b) };
            }
        }
    }
}

/// The serial console (UART0, 115200): the `Serial.println` mirror of the log.
pub struct Stdout;

impl Console for Stdout {
    fn line(&self, text: &[u8]) {
        let mut o = std::io::stdout().lock();
        let _ = o.write_all(text);
        let _ = o.write_all(b"\r\n");
    }
}
