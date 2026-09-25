// Fake ESP-IDF 4.4 esp_eth.h: start/stop of the Ethernet driver behind ETH (fakes::net()). The
// handle is the one ARDUINO_EVENT_ETH_CONNECTED carries (fakes::net().ethHandle).
#pragma once

#include "esp_err.h"

typedef void* esp_eth_handle_t;

esp_err_t esp_eth_start(esp_eth_handle_t hdl);
esp_err_t esp_eth_stop(esp_eth_handle_t hdl);
