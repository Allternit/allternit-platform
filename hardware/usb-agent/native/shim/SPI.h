// Native board: SPI STUB (step 2). Every transfer goes nowhere and reads 0.
// Step 4 replaces this one header and adds runtime/epd_model.cpp (the
// SSD1681 model that decodes the stream with the DC and CS pins through
// native::gpioOnWrite() and drives BUSY with native::gpioDrive()).
// Signatures copy arduino-esp32 3.3.12 SPI.h.
#pragma once

#include <Arduino.h>

#define SPI_MODE0 0
#define SPI_MODE1 1
#define SPI_MODE2 2
#define SPI_MODE3 3
#define SPI_LSBFIRST 0
#define SPI_MSBFIRST 1

class SPISettings {
 public:
  SPISettings() : _clock(1000000), _bitOrder(SPI_MSBFIRST), _dataMode(SPI_MODE0) {}
  SPISettings(uint32_t clock, uint8_t bitOrder, uint8_t dataMode) : _clock(clock), _bitOrder(bitOrder), _dataMode(dataMode) {}
  uint32_t _clock;
  uint8_t _bitOrder;
  uint8_t _dataMode;
};

class SPIClass {
 public:
  bool begin(int8_t sck = -1, int8_t miso = -1, int8_t mosi = -1, int8_t ss = -1) { return true; }
  void beginTransaction(SPISettings settings) {}
  void endTransaction(void) {}
  uint8_t transfer(uint8_t data) { return 0; }
};

inline SPIClass SPI;
