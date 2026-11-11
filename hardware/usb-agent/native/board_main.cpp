// Native board: the one translation unit that holds the firmware.
//
// The firmware is compiled UNCHANGED (design decision 1): this file includes
// Arduino.h, as the Arduino build does for a sketch, and then the .ino. Its
// own includes (<WiFi.h>, "USB.h", …) resolve to ./shim through -Ishim; its
// local headers (face.h, display.h, relay_link.h) resolve next to the .ino
// through -I$(FIRMWARE_DIR). The runtime (runtime/*.cpp) calls setup() and
// loop(). Nothing here changes the firmware's behaviour.
#include <Arduino.h>

#include "usb_agent.ino"
