// Native board: esp_mac.h. The station MAC is LINK_NATIVE_MAC
// ("aa:bb:cc:dd:ee:ff" or "aabbccddeeff"; default aa:bb:cc:dd:ee:ff).
#pragma once

#include <stdint.h>

#include "esp_system.h"

#ifdef __cplusplus
extern "C" {
#endif

typedef enum {
    ESP_MAC_WIFI_STA,
    ESP_MAC_WIFI_SOFTAP,
    ESP_MAC_BT,
    ESP_MAC_ETH,
    ESP_MAC_IEEE802154,
    ESP_MAC_BASE,
    ESP_MAC_EFUSE_FACTORY,
    ESP_MAC_EFUSE_CUSTOM,
    ESP_MAC_EFUSE_EXT,
} esp_mac_type_t;

esp_err_t esp_read_mac(uint8_t *mac, esp_mac_type_t type);

#ifdef __cplusplus
}
#endif
