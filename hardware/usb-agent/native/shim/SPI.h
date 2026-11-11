// Native board: SPI. Signatures copy arduino-esp32 3.3.12 SPI.h.
//
// Every call is forwarded to the SSD1681 panel model in
// runtime/epd_model.cpp (step 4). The panel pins come from the EPD_*_PIN
// macros, which only the link-board-epd variant defines. Without them no
// panel is attached: transfers go nowhere and read 0.
//
// SPIClass::begin() in the real core also drives the SS pin; the firmware sets
// CS itself, so the shim leaves the pin alone.
#pragma once

#include <Arduino.h>

#define SPI_MODE0 0
#define SPI_MODE1 1
#define SPI_MODE2 2
#define SPI_MODE3 3
#define SPI_LSBFIRST 0
#define SPI_MSBFIRST 1

// Implemented in runtime/epd_model.cpp.
namespace native {
void epdAttach(int sck, int mosi, int cs, int dc, int rst, int busy);
void epdSpiBegin(int sck, int miso, int mosi, int ss);
void epdSpiTransaction(bool open, uint32_t clock, uint8_t bitOrder, uint8_t dataMode);
uint8_t epdSpiTransfer(uint8_t data);
}  // namespace native

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
  SPIClass() {
#if defined(EPD_SCK_PIN) && defined(EPD_MOSI_PIN) && defined(EPD_CS_PIN) && defined(EPD_DC_PIN) && defined(EPD_RST_PIN) && defined(EPD_BUSY_PIN)
    // At static-init time, so the panel pins are quiet before the firmware's
    // first pinMode() on them.
    native::epdAttach(EPD_SCK_PIN, EPD_MOSI_PIN, EPD_CS_PIN, EPD_DC_PIN, EPD_RST_PIN, EPD_BUSY_PIN);
#endif
  }
  bool begin(int8_t sck = -1, int8_t miso = -1, int8_t mosi = -1, int8_t ss = -1) {
    native::epdSpiBegin(sck, miso, mosi, ss);
    return true;
  }
  void beginTransaction(SPISettings settings) { native::epdSpiTransaction(true, settings._clock, settings._bitOrder, settings._dataMode); }
  void endTransaction(void) { native::epdSpiTransaction(false, 0, 0, 0); }
  uint8_t transfer(uint8_t data) { return native::epdSpiTransfer(data); }
};

inline SPIClass SPI;
