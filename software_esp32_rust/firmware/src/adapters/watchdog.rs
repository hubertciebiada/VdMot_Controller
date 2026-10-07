//! The task watchdog of the threads, the thread spawner (GLUE-DESIGN-ESP.md 2.1) and the boot
//! deadline timer (6.4).

use core::cell::RefCell;
use core::ffi::{c_void, CStr};
use std::ffi::CString;

use esp_idf_svc::hal::cpu::Core;
use esp_idf_svc::hal::task::thread::ThreadSpawnConfiguration;
use esp_idf_svc::hal::task::watchdog::{TWDTDriver, WatchdogSubscription};
use esp_idf_svc::sys;
use vdm_esp_glue::app::{Spawner, TaskSpec};
use vdm_esp_glue::port::Watchdog;

/// The TWDT subscription of one thread (`esp_task_wdt_add` on that thread). Never dropped: the
/// threads run forever.
pub struct TaskWatchdog(RefCell<WatchdogSubscription<'static>>);

impl Watchdog for TaskWatchdog {
    fn feed(&self) {
        if let Ok(mut s) = self.0.try_borrow_mut() {
            let _ = s.feed();
        }
    }
}

/// Starts the threads of `app::TASKS`: ThreadSpawnConfiguration with the spec's name, priority
/// and core (the name becomes the FreeRTOS task name that `xTaskGetHandle` finds), the stack of
/// the spec through `std::thread::Builder`, and the TWDT subscription made on the new thread.
pub struct EspSpawner {
    twdt: TWDTDriver<'static>,
}

impl EspSpawner {
    /// The TWDT reconfigured as the glue wants it (driver kept for the subscriptions).
    pub fn new(twdt: TWDTDriver<'static>) -> Self {
        EspSpawner { twdt }
    }
}

impl Spawner<'static, TaskWatchdog> for EspSpawner {
    fn spawn(&mut self, spec: &TaskSpec, body: Box<dyn FnOnce(TaskWatchdog) + Send + 'static>) {
        // the task name must outlive the task: leaked once per thread at boot
        let name: Option<&'static CStr> = CString::new(spec.name)
            .ok()
            .map(|n| &*Box::leak(n.into_boxed_c_str()));
        let core = if spec.core == 0 {
            Core::Core0
        } else {
            Core::Core1
        };
        let conf = ThreadSpawnConfiguration {
            name,
            stack_size: spec.stack_bytes as usize,
            priority: spec.priority,
            inherit: false,
            pin_to_core: Some(core),
            ..Default::default()
        };
        if conf.set().is_err() {
            panic!("thread configuration of {} refused", spec.name);
        }
        let driver: &'static mut TWDTDriver<'static> = Box::leak(Box::new(self.twdt.clone()));
        let spawned = std::thread::Builder::new()
            .stack_size(spec.stack_bytes as usize)
            .spawn(move || {
                let Ok(sub) = driver.watch_current_task() else {
                    panic!("TWDT subscription refused");
                };
                body(TaskWatchdog(RefCell::new(sub)));
            });
        if spawned.is_err() {
            panic!("thread {} not created", spec.name);
        }
    }
}

/// The callback of the boot deadline: the argument is the leaked closure.
unsafe extern "C" fn boot_deadline_fired(arg: *mut c_void) {
    // SAFETY: `arm_boot_deadline` passes a leaked `Box<dyn Fn()>` behind a thin pointer.
    let f = unsafe { &*(arg as *const Box<dyn Fn() + Send + Sync>) };
    f();
}

/// Arms the one-shot boot deadline (GLUE-DESIGN-ESP.md 6.4 step 4): `fire` runs on the
/// esp_timer task after `ms`. False when the timer could not be created or started.
pub fn arm_boot_deadline(ms: u32, fire: Box<dyn Fn() + Send + Sync>) -> bool {
    let arg: &'static Box<dyn Fn() + Send + Sync> = Box::leak(Box::new(fire));
    let args = sys::esp_timer_create_args_t {
        callback: Some(boot_deadline_fired),
        arg: arg as *const Box<dyn Fn() + Send + Sync> as *mut c_void,
        dispatch_method: sys::esp_timer_dispatch_t_ESP_TIMER_TASK,
        name: c"boot_deadline".as_ptr(),
        skip_unhandled_events: false,
    };
    let mut timer: sys::esp_timer_handle_t = core::ptr::null_mut();
    // SAFETY: valid arguments; the callback argument lives forever.
    unsafe {
        sys::esp_timer_create(&args, &mut timer) == sys::ESP_OK
            && sys::esp_timer_start_once(timer, u64::from(ms) * 1000) == sys::ESP_OK
    }
}
