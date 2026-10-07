//! SNTP (one server, `CONFIG_LWIP_SNTP_MAX_SERVERS=1`): one `EspSntp` at a time; a server
//! change drops it (`sntp_stop`) and creates a new one. The sync callback (lwIP thread) counts
//! syncs and keeps the epoch in atomics.

use core::sync::atomic::{AtomicU32, Ordering};

use esp_idf_svc::sntp::{EspSntp, OperatingMode, SntpConf, SyncMode};
use vdm_esp_glue::port::Sntp;

struct SyncState {
    count: AtomicU32,
    epoch: AtomicU32,
}

pub struct SntpPort {
    state: &'static SyncState,
    sntp: Option<EspSntp<'static>>,
}

impl SntpPort {
    pub fn new() -> Self {
        SntpPort {
            state: Box::leak(Box::new(SyncState {
                count: AtomicU32::new(0),
                epoch: AtomicU32::new(0),
            })),
            sntp: None,
        }
    }
}

impl Sntp for SntpPort {
    fn configure(&mut self, server: Option<&str>) {
        // the old client stops before the new one starts (EspSntp is one instance at a time)
        self.sntp = None;
        let Some(server) = server else {
            return;
        };
        let conf = SntpConf {
            servers: [server],
            operating_mode: OperatingMode::Poll,
            sync_mode: SyncMode::Immediate,
        };
        let st = self.state;
        self.sntp = EspSntp::new_with_callback(&conf, move |t| {
            st.epoch.store(t.as_secs() as u32, Ordering::SeqCst);
            st.count.fetch_add(1, Ordering::SeqCst);
        })
        .ok();
    }
    fn sync_count(&self) -> u32 {
        self.state.count.load(Ordering::SeqCst)
    }
    fn last_sync_epoch(&self) -> u32 {
        self.state.epoch.load(Ordering::SeqCst)
    }
}
