// Native board: Arduino.h stand-in (design decisions 1 to 3).
//
// Only what usb_agent.ino, relay_link.h and display.h use. Signatures copy
// arduino-esp32 3.3.12 (check-signatures.sh), with one deliberate host
// mapping: the core declares `unsigned long millis()`, and `unsigned long` is
// 32 bits on Xtensa but 64 bits here, so millis() returns uint32_t and wraps
// as it does on the board.
#pragma once

#include <ctype.h>
#include <math.h>
#include <stdarg.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "esp_system.h"
#include "IPAddress.h"
#include "Print.h"
#include "WString.h"

#if !defined(__CHAR_UNSIGNED__)
#error "Build the native board with -funsigned-char: Xtensa char is unsigned (decision 2)"
#endif

#define IRAM_ATTR
#define ARDUINO_ISR_ATTR

// esp32-hal-gpio.h values.
#define LOW 0x0
#define HIGH 0x1
#define INPUT 0x01
#define OUTPUT 0x03
#define PULLUP 0x04
#define INPUT_PULLUP 0x05
#define PULLDOWN 0x08
#define INPUT_PULLDOWN 0x09
#define OPEN_DRAIN 0x10
#define OUTPUT_OPEN_DRAIN 0x13
#define DISABLED 0x00
#define RISING 0x01
#define FALLING 0x02
#define CHANGE 0x03
#define ONLOW 0x04
#define ONHIGH 0x05

#define LSBFIRST 0
#define MSBFIRST 1

#define NUM_DIGITAL_PINS 49
#define NOT_AN_INTERRUPT -1
#define digitalPinToInterrupt(p) ((((uint8_t)(p)) < NUM_DIGITAL_PINS) ? (p) : NOT_AN_INTERRUPT)

#define constrain(amt, low, high) ((amt) < (low) ? (low) : ((amt) > (high) ? (high) : (amt)))

// Time (esp32-hal.h). millis(): see the note at the top.
uint32_t millis();
void delay(uint32_t);
void delayMicroseconds(uint32_t us);

// GPIO (esp32-hal-gpio.h).
void pinMode(uint8_t pin, uint8_t mode);
void digitalWrite(uint8_t pin, uint8_t val);
int digitalRead(uint8_t pin);
void attachInterrupt(uint8_t pin, void (*)(void), int mode);
void detachInterrupt(uint8_t pin);

// Esp.h
class EspClass {
 public:
  void restart();
};
extern EspClass ESP;

// USB CDC serial (USBCDC.h; the FQBN uses CDCOnBoot=cdc, so Serial is USB
// CDC). Here it is the master side of a pty; the host opens the slave path
// printed in the `boot` event.
class USBCDC : public Stream {
 public:
  void begin(unsigned long baud = 0);
  int available(void) override;
  int read(void) override;
  void flush(void);
  size_t write(uint8_t) override;
  size_t write(const uint8_t *buffer, size_t size) override;
  using Print::write;
};
extern USBCDC USBSerial;
#define Serial USBSerial
