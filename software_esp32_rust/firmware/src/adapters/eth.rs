//! Ethernet: the LAN8720 on the ESP32 EMAC (or OpenETH under Espressif QEMU, feature `qemu`),
//! set up through the ESP-IDF calls with the values of Arduino-ESP32 2.0.7's
//! `ETH.begin(1, 16, 23, 18, ETH_PHY_LAN8720, ETH_CLOCK_GPIO0_IN)` that the C++ firmware uses:
//! PHY address 1, PHY "power" GPIO16 (the enable of the 50 MHz oscillator on the WT32-ETH01,
//! pulsed by the PHY reset), MDC 23, MDIO 18, RMII clock in on GPIO0, an EMAC software reset
//! timeout of 1000 ms (esp-idf-svc's `EthDriver` fixes 100 ms). The interface events reach the
//! port's state through atomics (the `sys_evt` task); the C++ flags of `net::onEvent`.

use core::ffi::c_void;
use core::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, Ordering};

use esp_idf_svc::eth::EthEvent;
use esp_idf_svc::eventloop::{EspSubscription, EspSystemEventLoop, System};
use esp_idf_svc::hal::delay::FreeRtos;
use esp_idf_svc::handle::RawHandle;
use esp_idf_svc::netif::{EspNetif, IpEvent, NetifStack};
use esp_idf_svc::sys;
use vdm_esp_glue::port::{Ethernet, IpInfo, IpSetup};

/// The flags the event callbacks keep (C++ `gEthLink`, `gEthIp`, `gGotIpCount`, `gEthHandle`).
struct EthState {
    link: AtomicBool,
    ip: AtomicBool,
    got_ip: AtomicU32,
    /// The driver handle of the first CONNECTED (the watchdog's restart needs a link first).
    connected: AtomicPtr<c_void>,
}

/// The Ethernet interface.
pub struct EthPort {
    sysloop: EspSystemEventLoop,
    state: &'static EthState,
    netif: Option<EspNetif>,
    handle: sys::esp_eth_handle_t,
    _subs: Option<(
        EspSubscription<'static, System>,
        EspSubscription<'static, System>,
    )>,
}

// SAFETY: the driver handle is used from the app thread only; the netif and the subscriptions
// are Send.
unsafe impl Send for EthPort {}

impl EthPort {
    pub fn new(sysloop: EspSystemEventLoop) -> Self {
        EthPort {
            sysloop,
            state: Box::leak(Box::new(EthState {
                link: AtomicBool::new(false),
                ip: AtomicBool::new(false),
                got_ip: AtomicU32::new(0),
                connected: AtomicPtr::new(core::ptr::null_mut()),
            })),
            netif: None,
            handle: core::ptr::null_mut(),
            _subs: None,
        }
    }
}

fn mac_config() -> sys::eth_mac_config_t {
    sys::eth_mac_config_t {
        sw_reset_timeout_ms: 1000,
        rx_task_stack_size: 4096,
        rx_task_prio: 15,
        flags: 0,
    }
}

#[cfg(not(feature = "qemu"))]
fn mac_and_phy() -> (*mut sys::esp_eth_mac_t, *mut sys::esp_eth_phy_t) {
    use vdm_esp_glue::board::{ETH_MDC, ETH_MDIO, ETH_PHY_ADDR, ETH_PHY_POWER};
    let emac = sys::eth_esp32_emac_config_t {
        __bindgen_anon_1: sys::eth_esp32_emac_config_t__bindgen_ty_1 {
            smi_gpio: sys::emac_esp_smi_gpio_config_t {
                mdc_num: i32::from(ETH_MDC),
                mdio_num: i32::from(ETH_MDIO),
            },
        },
        interface: sys::eth_data_interface_t_EMAC_DATA_INTERFACE_RMII,
        clock_config: sys::eth_mac_clock_config_t {
            rmii: sys::eth_mac_clock_config_t__bindgen_ty_2 {
                clock_mode: sys::emac_rmii_clock_mode_t_EMAC_CLK_EXT_IN,
                clock_gpio: sys::emac_rmii_clock_gpio_t_EMAC_CLK_IN_GPIO,
            },
        },
        dma_burst_len: sys::eth_mac_dma_burst_len_t_ETH_DMA_BURST_LEN_32,
        intr_priority: 0,
        ..Default::default()
    };
    let phy = sys::eth_phy_config_t {
        phy_addr: i32::from(ETH_PHY_ADDR),
        reset_timeout_ms: 100,
        autonego_timeout_ms: 4000,
        reset_gpio_num: i32::from(ETH_PHY_POWER),
        ..Default::default()
    };
    // SAFETY: the configs outlive the calls; the drivers copy what they keep.
    unsafe {
        (
            sys::esp_eth_mac_new_esp32(&emac, &mac_config()),
            sys::esp_eth_phy_new_lan87xx(&phy),
        )
    }
}

#[cfg(feature = "qemu")]
fn mac_and_phy() -> (*mut sys::esp_eth_mac_t, *mut sys::esp_eth_phy_t) {
    // ESP-IDF's QEMU examples: OpenETH MAC, the DP83848 PHY model, address auto, no reset pin
    let phy = sys::eth_phy_config_t {
        phy_addr: sys::ESP_ETH_PHY_ADDR_AUTO,
        reset_timeout_ms: 100,
        autonego_timeout_ms: 100,
        reset_gpio_num: -1,
        ..Default::default()
    };
    // SAFETY: as above.
    unsafe {
        (
            sys::esp_eth_mac_new_openeth(&mac_config()),
            sys::esp_eth_phy_new_dp83848(&phy),
        )
    }
}

/// QEMU only: the OpenETH model is not reset by a software restart (the ESP32 resets its EMAC in
/// `esp_restart_noos`): its interrupt stays enabled and its mode register set, so the next boot
/// takes an interrupt without a handler or aborts in the driver's sanity check. A shutdown
/// handler (run by `esp_restart`, not by a panic) stops the driver first.
#[cfg(feature = "qemu")]
fn qemu_stop_on_restart(handle: sys::esp_eth_handle_t) {
    static HANDLE: AtomicPtr<c_void> = AtomicPtr::new(core::ptr::null_mut());
    unsafe extern "C" fn stop() {
        let h = HANDLE.load(Ordering::SeqCst);
        if !h.is_null() {
            // SAFETY: the started driver of this boot.
            unsafe { sys::esp_eth_stop(h.cast()) };
        }
    }
    HANDLE.store(handle.cast(), Ordering::SeqCst);
    // SAFETY: a static function.
    unsafe { sys::esp_register_shutdown_handler(Some(stop)) };
}

/// The address of the glue (first octet in the low byte) as `esp_ip4_addr_t` (network order).
fn ip4(addr: u32) -> sys::esp_ip4_addr_t {
    sys::esp_ip4_addr_t { addr }
}

/// Host name, and a static address when `setup` has one (Arduino `ETH.config`: DHCP client
/// stopped, address, mask, gateway and DNS set before the interface starts).
pub(super) fn configure_netif(netif: *mut sys::esp_netif_t, setup: &IpSetup) {
    let mut b = [0u8; 40];
    // SAFETY: a created interface that has not started; NUL-terminated name.
    unsafe {
        if let Some(name) = super::c_name(setup.hostname, &mut b) {
            sys::esp_netif_set_hostname(netif, name.as_ptr());
        }
        if let Some(f) = setup.fixed {
            sys::esp_netif_dhcpc_stop(netif);
            let info = sys::esp_netif_ip_info_t {
                ip: ip4(f.ip),
                netmask: ip4(f.mask),
                gw: ip4(f.gateway),
            };
            sys::esp_netif_set_ip_info(netif, &info);
            let mut dns = sys::esp_netif_dns_info_t::default();
            dns.ip.u_addr.ip4 = ip4(f.dns);
            dns.ip.type_ = sys::ESP_IPADDR_TYPE_V4 as u8;
            sys::esp_netif_set_dns_info(
                netif,
                sys::esp_netif_dns_type_t_ESP_NETIF_DNS_MAIN,
                &mut dns,
            );
        }
    }
}

/// Address, mask, gateway and DNS of `netif` (zeros when unknown).
pub(super) fn netif_info(netif: Option<&EspNetif>) -> IpInfo {
    let Some(n) = netif else {
        return IpInfo::default();
    };
    let mut ip = sys::esp_netif_ip_info_t::default();
    let mut dns = sys::esp_netif_dns_info_t::default();
    // SAFETY: a valid interface and out pointers.
    unsafe {
        sys::esp_netif_get_ip_info(n.handle(), &mut ip);
        sys::esp_netif_get_dns_info(
            n.handle(),
            sys::esp_netif_dns_type_t_ESP_NETIF_DNS_MAIN,
            &mut dns,
        );
        IpInfo {
            ip: ip.ip.addr,
            mask: ip.netmask.addr,
            gateway: ip.gw.addr,
            dns: dns.ip.u_addr.ip4.addr,
        }
    }
}

impl Ethernet for EthPort {
    fn begin(&mut self, setup: &IpSetup) -> bool {
        let (mac, phy) = mac_and_phy();
        if mac.is_null() || phy.is_null() {
            return false;
        }
        let cfg = sys::esp_eth_config_t {
            mac,
            phy,
            check_link_period_ms: 2000,
            ..Default::default()
        };
        let mut handle: sys::esp_eth_handle_t = core::ptr::null_mut();
        // SAFETY: valid config and out pointer.
        if unsafe { sys::esp_eth_driver_install(&cfg, &mut handle) } != sys::ESP_OK {
            return false;
        }
        let Ok(netif) = EspNetif::new(NetifStack::Eth) else {
            return false;
        };
        configure_netif(netif.handle(), setup);
        // SAFETY: the glue binds the installed driver to the interface.
        let attached = unsafe {
            sys::esp_netif_attach(netif.handle(), sys::esp_eth_new_netif_glue(handle).cast())
        };
        if attached != sys::ESP_OK {
            return false;
        }
        let st = self.state;
        let own = handle as usize;
        let eth = self.sysloop.subscribe::<EthEvent, _>(move |e| {
            if e.handle() as usize != own {
                return;
            }
            match e {
                EthEvent::Connected(h) => {
                    st.connected.store(h as *mut c_void, Ordering::SeqCst);
                    st.link.store(true, Ordering::SeqCst);
                }
                EthEvent::Disconnected(_) | EthEvent::Stopped(_) => {
                    st.link.store(false, Ordering::SeqCst);
                    st.ip.store(false, Ordering::SeqCst);
                }
                _ => {}
            }
        });
        let netif_handle = netif.handle() as usize;
        let ip = self.sysloop.subscribe::<IpEvent, _>(move |e| {
            if let IpEvent::DhcpIpAssigned(a) = e {
                if a.netif_handle() as usize == netif_handle {
                    st.ip.store(true, Ordering::SeqCst);
                    st.got_ip.fetch_add(1, Ordering::SeqCst);
                }
            }
        });
        let (Ok(eth), Ok(ip)) = (eth, ip) else {
            return false;
        };
        // SAFETY: installed driver.
        if unsafe { sys::esp_eth_start(handle) } != sys::ESP_OK {
            return false;
        }
        #[cfg(feature = "qemu")]
        qemu_stop_on_restart(handle);
        self.handle = handle;
        self.netif = Some(netif);
        self._subs = Some((eth, ip));
        true
    }
    fn restart(&mut self) -> bool {
        let h = self.state.connected.load(Ordering::SeqCst);
        if h.is_null() {
            return false;
        }
        // SAFETY: the started driver; C++ ignored the results of stop and start.
        unsafe { sys::esp_eth_stop(h.cast()) };
        FreeRtos::delay_ms(200);
        // SAFETY: as above.
        unsafe { sys::esp_eth_start(h.cast()) };
        true
    }
    fn link(&self) -> bool {
        self.state.link.load(Ordering::SeqCst)
    }
    fn has_ip(&self) -> bool {
        self.state.ip.load(Ordering::SeqCst)
    }
    fn got_ip_count(&self) -> u32 {
        self.state.got_ip.load(Ordering::SeqCst)
    }
    fn info(&self) -> IpInfo {
        netif_info(self.netif.as_ref())
    }
    fn mac(&self) -> [u8; 6] {
        self.netif
            .as_ref()
            .and_then(|n| n.get_mac().ok())
            .unwrap_or_default()
    }
}
