//! Network of the spike: Ethernet (LAN8720 on the ESP32 EMAC, or OpenETH under QEMU), the WiFi
//! STA driver (initialised, not connected), SNTP.
//!
//! The Ethernet driver is set up through the ESP-IDF API directly, with the values of Arduino-ESP32
//! 2.0.7's `ETH.begin(1, 16, 23, 18, ETH_PHY_LAN8720, ETH_CLOCK_GPIO0_IN)` that the C++ firmware
//! uses: PHY address 1, PHY reset GPIO16 (on the WT32-ETH01 the enable of the 50 MHz oscillator;
//! the driver pulses it LOW for 150 us before the MAC init, which restarts the clock the EMAC
//! reset needs), MDC 23, MDIO 18, RMII clock input on GPIO0, EMAC software reset timeout
//! 1000 ms (esp-idf-svc's `EthDriver` fixes it at 100 ms).

use core::sync::atomic::{AtomicU32, Ordering};

use esp_idf_svc::eventloop::{EspSubscription, EspSystemEventLoop, System};
use esp_idf_svc::handle::RawHandle;
use esp_idf_svc::netif::{EspNetif, IpEvent, NetifStack};
use esp_idf_svc::sntp::EspSntp;
use esp_idf_sys::{self as sys, esp, EspError};

use crate::boot_guard;

/// IPv4 address of the Ethernet interface (`u32::from(Ipv4Addr)`), 0 = none.
static ETH_IP: AtomicU32 = AtomicU32::new(0);

pub fn eth_ip() -> Option<core::net::Ipv4Addr> {
    let raw = ETH_IP.load(Ordering::SeqCst);
    (raw != 0).then(|| core::net::Ipv4Addr::from(raw))
}

/// The running Ethernet interface; kept for the lifetime of the firmware (never stopped).
pub struct Ethernet {
    _netif: EspNetif,
    _ip_events: EspSubscription<'static, System>,
    _driver: sys::esp_eth_handle_t,
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
    let emac = sys::eth_esp32_emac_config_t {
        __bindgen_anon_1: sys::eth_esp32_emac_config_t__bindgen_ty_1 {
            smi_gpio: sys::emac_esp_smi_gpio_config_t { mdc_num: 23, mdio_num: 18 },
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
        phy_addr: 1,
        reset_timeout_ms: 100,
        autonego_timeout_ms: 4000,
        reset_gpio_num: 16,
        ..Default::default()
    };
    // SAFETY: the configs outlive the calls; the drivers copy what they keep.
    unsafe { (sys::esp_eth_mac_new_esp32(&emac, &mac_config()), sys::esp_eth_phy_new_lan87xx(&phy)) }
}

#[cfg(feature = "qemu")]
fn mac_and_phy() -> (*mut sys::esp_eth_mac_t, *mut sys::esp_eth_phy_t) {
    // as ESP-IDF's QEMU examples: OpenETH MAC, DP83848 PHY model, address auto, no reset pin
    let phy = sys::eth_phy_config_t {
        phy_addr: sys::ESP_ETH_PHY_ADDR_AUTO,
        reset_timeout_ms: 100,
        autonego_timeout_ms: 100,
        reset_gpio_num: -1,
        ..Default::default()
    };
    // SAFETY: as above.
    unsafe { (sys::esp_eth_mac_new_openeth(&mac_config()), sys::esp_eth_phy_new_dp83848(&phy)) }
}

/// Installs and starts the Ethernet driver with a DHCP client interface. An IP marks the
/// network health evidence of the boot guard.
pub fn start_ethernet(sysloop: &EspSystemEventLoop) -> Result<Ethernet, EspError> {
    let (mac, phy) = mac_and_phy();
    if mac.is_null() || phy.is_null() {
        return Err(EspError::from_infallible::<{ sys::ESP_FAIL }>());
    }
    let cfg = sys::esp_eth_config_t { mac, phy, check_link_period_ms: 2000, ..Default::default() };
    let mut handle: sys::esp_eth_handle_t = core::ptr::null_mut();
    // SAFETY: valid config and out pointer.
    esp!(unsafe { sys::esp_eth_driver_install(&cfg, &mut handle) })?;

    let netif = EspNetif::new(NetifStack::Eth)?;
    let netif_handle = netif.handle() as usize;
    // SAFETY: the glue binds the installed driver to the interface.
    esp!(unsafe { sys::esp_netif_attach(netif.handle(), sys::esp_eth_new_netif_glue(handle).cast()) })?;

    let ip_events = sysloop.subscribe::<IpEvent, _>(move |event| {
        if let IpEvent::DhcpIpAssigned(a) = event {
            if a.netif_handle() as usize == netif_handle {
                let ip = a.ip();
                ETH_IP.store(u32::from(ip), Ordering::SeqCst);
                println!("net: ethernet ip {ip}");
                boot_guard::note_net_up();
            }
        }
    })?;
    // SAFETY: installed driver.
    esp!(unsafe { sys::esp_eth_start(handle) })?;
    Ok(Ethernet { _netif: netif, _ip_events: ip_events, _driver: handle })
}

/// WiFi STA driver, initialised and configured without credentials, not started (the C++ firmware
/// starts it only with the interface wifi or as the fallback; the spike measures its size).
#[cfg(feature = "wifi")]
pub fn init_wifi(
    modem: esp_idf_svc::hal::modem::Modem<'static>,
    sysloop: &EspSystemEventLoop,
) -> Result<esp_idf_svc::wifi::EspWifi<'static>, EspError> {
    use esp_idf_svc::wifi::{ClientConfiguration, Configuration, EspWifi};
    // no NVS partition: nothing of the WiFi driver goes into the operator's NVS
    let mut wifi = EspWifi::new(modem, sysloop.clone(), None)?;
    wifi.set_configuration(&Configuration::Client(ClientConfiguration::default()))?;
    Ok(wifi)
}

/// SNTP with ESP-IDF's default server (the firmware takes the configured one).
pub fn start_sntp() -> Result<EspSntp<'static>, EspError> {
    EspSntp::new_default()
}
