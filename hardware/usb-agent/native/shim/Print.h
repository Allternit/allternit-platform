// Native board: Print and Stream, written for this project (signatures from
// arduino-esp32 3.3.12 Print.h and Stream.h; only what the firmware uses).
#pragma once

#include <stddef.h>
#include <stdint.h>

#include "WString.h"

class Print {
 public:
  virtual ~Print() {}
  virtual size_t write(uint8_t) = 0;
  virtual size_t write(const uint8_t *buffer, size_t size);
  size_t write(const char *buffer, size_t size) {
    return write((const uint8_t *)buffer, size);
  }

  size_t printf(const char *format, ...) __attribute__((format(printf, 2, 3)));
  size_t print(const String &);
  size_t print(const char[]);
  size_t print(char);
  size_t println(const String &s);
  size_t println(const char[]);
  size_t println(void);
};

class Stream : public Print {
 protected:
  unsigned long _timeout = 1000;

 public:
  virtual int available() = 0;
  virtual int read() = 0;
  void setTimeout(unsigned long timeout);  // sets maximum milliseconds to wait for stream data, default is 1 second
  unsigned long getTimeout(void) {
    return _timeout;
  }
};
