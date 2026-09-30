// Tests of src/net.cpp for a switch of the interface at run time: the settings apply at the
// restart that follows, the service passes before it run with the new settings.
#include <IPAddress.h>
#include <WiFi.h>

#include "glue_test.h"
#include "net.h"

namespace {

const uint32_t kIp = IPAddress(192, 168, 1, 20);

vdm::Config wifiConfig(const char* ssid) {
  vdm::Config c;
  vdm::setDefaults(c);
  vdm::copyString(c.station, sizeof c.station, "Heating Floor");
  c.net.iface = vdm::NetInterface::Wifi;
  vdm::copyString(c.net.ssid, sizeof c.net.ssid, ssid);
  return c;
}

void tick(uint32_t t) {
  fakes::setMs(t);
  net::service(t, false);
  fakes::net().pingStep();
}

}  // namespace

TEST_CASE("net: WiFi lost after a switch from Wifi to Auto reconnects at once before the restart") {
  glue::begin();
  vdm::Config c = wifiConfig("home");
  net::begin(c);  // Ethernet is not started with iface Wifi
  REQUIRE(fakes::net().wifiBegins == 1);
  fakes::net().wifiIp = kIp;
  fakes::net().fire(ARDUINO_EVENT_WIFI_STA_GOT_IP);
  tick(1000);
  REQUIRE(net::info().state == vdm::NetState::Wifi);
  vdm::Config autoCfg = c;
  autoCfg.net.iface = vdm::NetInterface::Auto;
  net::reconfigure(autoCfg);
  REQUIRE(sib::ota().restartRequests.size() == 1);  // the new interface needs a restart
  CHECK(sib::ota().restartRequests[0].delayMs == 1500);
  fakes::net().fire(ARDUINO_EVENT_WIFI_STA_DISCONNECTED);
  tick(2000);
  // without a started Ethernet driver Auto wants WiFi at once, not after 30 s
  CHECK(fakes::net().wifiBegins == 2);
  CHECK(fakes::net().ethBegins.empty());
}
