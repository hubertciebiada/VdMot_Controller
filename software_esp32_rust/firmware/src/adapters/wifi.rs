//! The WiFi station (feature `wifi`; without it every `begin` fails, as a station that cannot
//! be built). Nothing goes into the WiFi NVS (`WiFi.persistent(false)`); the credentials are set
//! with the fields of Arduino-ESP32 2.0.7's `wifi_sta_config()` through `esp_wifi_set_config`
//! (the SSID and password are bytes, not UTF-8); `stop` drops the driver, which frees its
//! memory (`WiFi.mode(WIFI_OFF)`); the next `begin` builds it again.

#[cfg(feature = "wifi")]
mod imp {
    use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

    use esp_idf_svc::eventloop::{EspSubscription, EspSystemEventLoop, System};
    use esp_idf_svc::hal::modem::Modem;
    use esp_idf_svc::handle::RawHandle;
    use esp_idf_svc::netif::{EspNetif, IpEvent, NetifStack};
    use esp_idf_svc::sys;
    use esp_idf_svc::wifi::{EspWifi, WifiDriver, WifiEvent};
    use vdm_esp_glue::port::{IpInfo, IpSetup, Wifi};

    /// `WIFI_REASON_ASSOC_LEAVE`: a disconnect the firmware asked for.
    const REASON_ASSOC_LEAVE: u16 = 8;

    /// The flags of the event callbacks (C++ `gWifiUp`, `gGotIpCount`, Arduino's first-connect
    /// retry trigger).
    struct WifiState {
        up: AtomicBool,
        got_ip: AtomicU32,
        unrequested: AtomicU32,
    }

    struct Station {
        wifi: EspWifi<'static>,
        _subs: (
            EspSubscription<'static, System>,
            EspSubscription<'static, System>,
        ),
    }

    pub struct WifiPort {
        sysloop: EspSystemEventLoop,
        state: &'static WifiState,
        station: Option<Station>,
    }

    // SAFETY: the station is used from the app thread only.
    unsafe impl Send for WifiPort {}

    impl WifiPort {
        pub fn new(sysloop: EspSystemEventLoop, _modem: Modem<'static>) -> Self {
            WifiPort {
                sysloop,
                state: Box::leak(Box::new(WifiState {
                    up: AtomicBool::new(false),
                    got_ip: AtomicU32::new(0),
                    unrequested: AtomicU32::new(0),
                })),
                station: None,
            }
        }

        fn build(&self, setup: &IpSetup) -> Option<Station> {
            // SAFETY: one driver at a time: the old one is dropped before (`stop`, `begin`).
            let modem = unsafe { Modem::steal() };
            let driver = WifiDriver::new(modem, self.sysloop.clone(), None).ok()?;
            let netif = EspNetif::new(NetifStack::Sta).ok()?;
            super::super::eth::configure_netif(netif.handle(), setup);
            let wifi = EspWifi::wrap_all(driver, netif).ok()?;
            let st = self.state;
            let sta = wifi.sta_netif().handle() as usize;
            let w = self.sysloop.subscribe::<WifiEvent, _>(move |e| match e {
                WifiEvent::StaDisconnected(d) => {
                    st.up.store(false, Ordering::SeqCst);
                    if d.reason() != REASON_ASSOC_LEAVE {
                        st.unrequested.fetch_add(1, Ordering::SeqCst);
                    }
                }
                WifiEvent::StaStopped => st.up.store(false, Ordering::SeqCst),
                _ => {}
            });
            let i = self.sysloop.subscribe::<IpEvent, _>(move |e| match e {
                IpEvent::DhcpIpAssigned(a) if a.netif_handle() as usize == sta => {
                    st.up.store(true, Ordering::SeqCst);
                    st.got_ip.fetch_add(1, Ordering::SeqCst);
                }
                IpEvent::DhcpIpDeassigned(h) if h as usize == sta => {
                    st.up.store(false, Ordering::SeqCst)
                }
                _ => {}
            });
            let (w, i) = (w.ok()?, i.ok()?);
            // SAFETY: an initialised driver: station mode, then start.
            unsafe {
                if sys::esp_wifi_set_mode(sys::wifi_mode_t_WIFI_MODE_STA) != sys::ESP_OK
                    || sys::esp_wifi_start() != sys::ESP_OK
                {
                    return None;
                }
            }
            Some(Station {
                wifi,
                _subs: (w, i),
            })
        }
    }

    impl Wifi for WifiPort {
        fn begin(&mut self, setup: &IpSetup) -> bool {
            self.station = None;
            self.station = self.build(setup);
            self.station.is_some()
        }
        fn connect(&mut self, ssid: &[u8], password: &[u8]) {
            if self.station.is_none() {
                return;
            }
            let mut conf = sys::wifi_config_t::default();
            // SAFETY: the station member of the union; Arduino's wifi_sta_config() defaults
            // (fast scan, by signal, RSSI -127, PMF capable, WPA2-PSK minimum with a password).
            unsafe {
                let sta = &mut conf.sta;
                let n = ssid.len().min(sta.ssid.len());
                sta.ssid[..n].copy_from_slice(&ssid[..n]);
                let p = password.len().min(sta.password.len());
                sta.password[..p].copy_from_slice(&password[..p]);
                sta.scan_method = sys::wifi_scan_method_t_WIFI_FAST_SCAN;
                sta.sort_method = sys::wifi_sort_method_t_WIFI_CONNECT_AP_BY_SIGNAL;
                sta.threshold.rssi = -127;
                sta.threshold.authmode = if password.is_empty() {
                    sys::wifi_auth_mode_t_WIFI_AUTH_OPEN
                } else {
                    sys::wifi_auth_mode_t_WIFI_AUTH_WPA2_PSK
                };
                sta.pmf_cfg.capable = true;
                sys::esp_wifi_disconnect();
                sys::esp_wifi_set_config(sys::wifi_interface_t_WIFI_IF_STA, &mut conf);
                sys::esp_wifi_connect();
            }
        }
        fn reconnect(&mut self) {
            if self.station.is_some() {
                // SAFETY: a started station (WiFi.reconnect: disconnect, connect).
                unsafe {
                    sys::esp_wifi_disconnect();
                    sys::esp_wifi_connect();
                }
            }
        }
        fn stop(&mut self) {
            self.station = None;
            self.state.up.store(false, Ordering::SeqCst);
        }
        fn up(&self) -> bool {
            self.state.up.load(Ordering::SeqCst)
        }
        fn got_ip_count(&self) -> u32 {
            self.state.got_ip.load(Ordering::SeqCst)
        }
        fn take_unrequested_disconnect(&mut self) -> bool {
            self.state
                .unrequested
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
                .is_ok()
        }
        fn info(&self) -> IpInfo {
            super::super::eth::netif_info(self.station.as_ref().map(|s| s.wifi.sta_netif()))
        }
        fn rssi(&self) -> i8 {
            self.station
                .as_ref()
                .and_then(|s| s.wifi.get_ap_info().ok())
                .map_or(0, |ap| ap.signal_strength)
        }
        fn mac(&self) -> [u8; 6] {
            let mut mac = [0u8; 6];
            // SAFETY: 6-byte buffer; the station MAC is derived from the eFuse MAC.
            unsafe { sys::esp_read_mac(mac.as_mut_ptr(), sys::esp_mac_type_t_ESP_MAC_WIFI_STA) };
            mac
        }
    }
}

#[cfg(not(feature = "wifi"))]
mod imp {
    use vdm_esp_glue::port::{IpInfo, IpSetup, Wifi};

    /// No WiFi in this build: the station cannot be built.
    pub struct WifiPort;

    impl Wifi for WifiPort {
        fn begin(&mut self, _setup: &IpSetup) -> bool {
            false
        }
        fn connect(&mut self, _ssid: &[u8], _password: &[u8]) {}
        fn reconnect(&mut self) {}
        fn stop(&mut self) {}
        fn up(&self) -> bool {
            false
        }
        fn got_ip_count(&self) -> u32 {
            0
        }
        fn take_unrequested_disconnect(&mut self) -> bool {
            false
        }
        fn info(&self) -> IpInfo {
            IpInfo::default()
        }
        fn rssi(&self) -> i8 {
            0
        }
        fn mac(&self) -> [u8; 6] {
            [0; 6]
        }
    }
}

pub use imp::WifiPort;
