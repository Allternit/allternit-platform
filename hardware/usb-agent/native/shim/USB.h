// Native board: USB.h (ESPUSB). Names are stored; begin() does nothing else.
#pragma once

#include <Arduino.h>

class ESPUSB {
 public:
  bool productName(const char *name);
  bool manufacturerName(const char *name);
  bool begin();
};
extern ESPUSB USB;
