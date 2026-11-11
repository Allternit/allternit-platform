// Native board: esp_random.h, filled from the host's entropy source.
#pragma once

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

uint32_t esp_random(void);
void esp_fill_random(void *buf, size_t len);

#ifdef __cplusplus
}
#endif
