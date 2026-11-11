// Native board: USBHIDKeyboard.h. Each call is one `hid` event:
//   {"ev":"hid","t":…,"dev":"keyboard","call":"press|release|write|releaseAll","ch":c,"sent":0|1}
// (no "ch" for releaseAll). sent=0: the phone did not get the report.
// press()/write() return 0 for an ASCII code the en_US layout has no key
// for, as the core does; nothing is sent then.
#pragma once

#include <Arduino.h>

#define KEY_LEFT_CTRL   0x80
#define KEY_LEFT_SHIFT  0x81
#define KEY_LEFT_ALT    0x82
#define KEY_LEFT_GUI    0x83
#define KEY_RIGHT_CTRL  0x84
#define KEY_RIGHT_SHIFT 0x85
#define KEY_RIGHT_ALT   0x86
#define KEY_RIGHT_GUI   0x87

#define KEY_UP_ARROW     0xDA
#define KEY_DOWN_ARROW   0xD9
#define KEY_LEFT_ARROW   0xD8
#define KEY_RIGHT_ARROW  0xD7
#define KEY_BACKSPACE    0xB2
#define KEY_TAB          0xB3
#define KEY_RETURN       0xB0
#define KEY_ESC          0xB1

extern const uint8_t KeyboardLayout_en_US[];

class USBHIDKeyboard {
 public:
  void begin(const uint8_t *layout = KeyboardLayout_en_US);
  size_t write(uint8_t k);
  size_t press(uint8_t k);
  size_t release(uint8_t k);
  void releaseAll(void);
};
