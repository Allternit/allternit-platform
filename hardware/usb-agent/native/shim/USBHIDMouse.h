// Native board: USBHIDMouse.h (USBHIDRelativeMouse). Signatures copy the
// core, including the int8_t parameters, so a value that does not fit
// truncates as on the board. Behaviour follows the core: press() and
// release() send a report only when the button state changes, move() always
// sends one. Each report is a `hid` event:
//   {"ev":"hid","t":…,"dev":"mouse","buttons":b,"x":x,"y":y,"wheel":w,"pan":p,"sent":0|1}
// sent=0: the phone did not get it (USB not mounted, or suspended).
#pragma once

#include <Arduino.h>

#define MOUSE_LEFT     0x01
#define MOUSE_RIGHT    0x02
#define MOUSE_MIDDLE   0x04
#define MOUSE_BACKWARD 0x08
#define MOUSE_FORWARD  0x10
#define MOUSE_ALL      0x1F

class USBHIDMouseBase {
 public:
  void begin(void);
  void press(uint8_t b = MOUSE_LEFT);      // press LEFT by default
  void release(uint8_t b = MOUSE_LEFT);    // release LEFT by default
  bool isPressed(uint8_t b = MOUSE_LEFT);  // check LEFT by default
  virtual void buttons(uint8_t b) = 0;

 protected:
  uint8_t _buttons = 0;
};

class USBHIDRelativeMouse : public USBHIDMouseBase {
 public:
  void move(int8_t x, int8_t y, int8_t wheel = 0, int8_t pan = 0);
  void buttons(uint8_t b) override;
};

typedef USBHIDRelativeMouse USBHIDMouse;
