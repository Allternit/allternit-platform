// Native board: the three TinyUSB device calls the firmware makes.
// The USB host state comes from `usb mounted|asleep|none` (default mounted).
// tud_remote_wakeup() succeeds only while asleep and with `usb-wake on`
// (default off); the host then resumes 20 ms later.
#pragma once

#include <stdbool.h>

#ifdef __cplusplus
extern "C" {
#endif

bool tud_mounted(void);
bool tud_suspended(void);
bool tud_remote_wakeup(void);

#ifdef __cplusplus
}
#endif
