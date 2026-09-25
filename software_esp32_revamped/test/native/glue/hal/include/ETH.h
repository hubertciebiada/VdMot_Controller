// Fake Arduino-ESP32 2.0.7 ETH.h (LAN8720 on the WT32-ETH01): calls recorded in fakes::net(),
// addresses and link from its knobs. The driver's events (ETH_START, CONNECTED, GOT_IP, ...) are
// fired by the test through fakes::net().fire().
#pragma once

#include <stdint.h>

#include "IPAddress.h"
#include "WString.h"
#include "WiFi.h"

#ifndef ETH_PHY_ADDR
#define ETH_PHY_ADDR 0
#endif
#ifndef ETH_PHY_POWER
#define ETH_PHY_POWER -1
#endif
#ifndef ETH_PHY_MDC
#define ETH_PHY_MDC 23
#endif
#ifndef ETH_PHY_MDIO
#define ETH_PHY_MDIO 18
#endif

typedef enum {
  ETH_CLOCK_GPIO0_IN,
  ETH_CLOCK_GPIO0_OUT,
  ETH_CLOCK_GPIO16_OUT,
  ETH_CLOCK_GPIO17_OUT
} eth_clock_mode_t;
typedef enum {
  ETH_PHY_LAN8720,
  ETH_PHY_TLK110,
  ETH_PHY_RTL8201,
  ETH_PHY_DP83848,
  ETH_PHY_DM9051,
  ETH_PHY_KSZ8041,
  ETH_PHY_KSZ8081,
  ETH_PHY_MAX
} eth_phy_type_t;

#define ETH_PHY_TYPE ETH_PHY_LAN8720
#define ETH_CLK_MODE ETH_CLOCK_GPIO0_IN

class ETHClass {
 public:
  bool begin(uint8_t phy_addr = ETH_PHY_ADDR, int power = ETH_PHY_POWER, int mdc = ETH_PHY_MDC,
             int mdio = ETH_PHY_MDIO, eth_phy_type_t type = ETH_PHY_TYPE,
             eth_clock_mode_t clk_mode = ETH_CLK_MODE, bool use_mac_from_efuse = false);
  bool config(IPAddress local_ip, IPAddress gateway, IPAddress subnet,
              IPAddress dns1 = (uint32_t)0x00000000, IPAddress dns2 = (uint32_t)0x00000000);
  const char* getHostname();
  bool setHostname(const char* hostname);
  bool fullDuplex();
  bool linkUp();
  uint8_t linkSpeed();
  IPAddress localIP();
  IPAddress subnetMask();
  IPAddress gatewayIP();
  IPAddress dnsIP(uint8_t dns_no = 0);
  uint8_t* macAddress(uint8_t* mac);
  String macAddress();
};

extern ETHClass ETH;
