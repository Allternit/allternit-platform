// Native board: esp_task_wdt.h. The watchdog runs on real time (decision 6):
// a subscribed task that does not call esp_task_wdt_reset() within
// timeout_ms emits a `wdt` event and the board reboots with
// ESP_RST_TASK_WDT (decision 5). delay() does not feed it, as on the board.
#pragma once

#include <stdbool.h>
#include <stdint.h>

#include "esp_system.h"

#ifdef __cplusplus
extern "C" {
#endif

typedef void *TaskHandle_t;

typedef struct {
    uint32_t timeout_ms;
    uint32_t idle_core_mask;
    bool trigger_panic;
} esp_task_wdt_config_t;

esp_err_t esp_task_wdt_reconfigure(const esp_task_wdt_config_t *config);
esp_err_t esp_task_wdt_add(TaskHandle_t task_handle);
esp_err_t esp_task_wdt_reset(void);

#ifdef __cplusplus
}
#endif
