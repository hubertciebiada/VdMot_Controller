//! The gateway probe: one `esp_ping` session per probe (1 echo, 32 B, timeout 1 s), as C++. The
//! session's own task (2560 B, priority 2) reports through the callbacks into atomics; a
//! deleted session frees its task and raw socket within about 1 s.

use core::ffi::c_void;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use esp_idf_svc::sys;
use vdm_esp_glue::port::Pinger;

struct PingState {
    done: AtomicBool,
    replies: AtomicU32,
}

unsafe extern "C" fn on_success(_h: sys::esp_ping_handle_t, arg: *mut c_void) {
    // SAFETY: the argument is the leaked state of the port.
    let st = unsafe { &*(arg as *const PingState) };
    st.replies.fetch_add(1, Ordering::SeqCst);
    st.done.store(true, Ordering::SeqCst);
}

unsafe extern "C" fn on_timeout(_h: sys::esp_ping_handle_t, arg: *mut c_void) {
    // SAFETY: as above.
    let st = unsafe { &*(arg as *const PingState) };
    st.done.store(true, Ordering::SeqCst);
}

pub struct PingPort {
    state: &'static PingState,
    session: sys::esp_ping_handle_t,
}

// SAFETY: the session handle is used from the app thread only.
unsafe impl Send for PingPort {}

impl PingPort {
    pub fn new() -> Self {
        PingPort {
            state: Box::leak(Box::new(PingState {
                done: AtomicBool::new(false),
                replies: AtomicU32::new(0),
            })),
            session: core::ptr::null_mut(),
        }
    }
}

impl Pinger for PingPort {
    fn start(&mut self, ip: u32) -> bool {
        self.state.done.store(false, Ordering::SeqCst);
        self.state.replies.store(0, Ordering::SeqCst);
        // the glue's order is lwIP's (first octet in the low byte)
        #[cfg(not(esp_idf_lwip_ipv6))]
        let target = sys::ip4_addr_t { addr: ip };
        #[cfg(esp_idf_lwip_ipv6)]
        let target = sys::ip_addr_t {
            u_addr: sys::ip_addr__bindgen_ty_1 {
                ip4: sys::ip4_addr_t { addr: ip },
            },
            type_: 0,
        };
        let config = sys::esp_ping_config_t {
            count: 1,
            interval_ms: 1000,
            timeout_ms: 1000,
            data_size: 32,
            tos: 0,
            ttl: 64,
            target_addr: target,
            task_stack_size: 2560,
            task_prio: 2,
            interface: 0,
        };
        let cbs = sys::esp_ping_callbacks_t {
            cb_args: self.state as *const PingState as *mut c_void,
            on_ping_success: Some(on_success),
            on_ping_timeout: Some(on_timeout),
            on_ping_end: None,
        };
        let mut h: sys::esp_ping_handle_t = core::ptr::null_mut();
        // SAFETY: valid config and callbacks; the state lives forever.
        unsafe {
            if sys::esp_ping_new_session(&config, &cbs, &mut h) != sys::ESP_OK {
                return false;
            }
            self.session = h;
            sys::esp_ping_start(h) == sys::ESP_OK
        }
    }
    fn done(&self) -> bool {
        self.state.done.load(Ordering::SeqCst)
    }
    fn replies(&self) -> u32 {
        self.state.replies.load(Ordering::SeqCst)
    }
    fn delete(&mut self) {
        if self.session.is_null() {
            return;
        }
        // SAFETY: a session of this port, deleted once.
        unsafe { sys::esp_ping_delete_session(self.session) };
        self.session = core::ptr::null_mut();
    }
}
