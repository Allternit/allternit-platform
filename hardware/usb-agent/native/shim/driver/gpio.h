// Native board: driver/gpio.h (only gpio_get_level; it reads the same pin
// table as digitalRead()).
#pragma once

#ifdef __cplusplus
extern "C" {
#endif

typedef enum {
    GPIO_NUM_NC = -1,
    GPIO_NUM_0 = 0,
    GPIO_NUM_MAX = 49,
} gpio_num_t;

int gpio_get_level(gpio_num_t gpio_num);

#ifdef __cplusplus
}
#endif
