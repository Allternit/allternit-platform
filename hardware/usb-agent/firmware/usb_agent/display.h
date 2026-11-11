// Allternit Link face display (contract hardware/link/README.md section 3.5).
//
// LinkDisplay is the only thing the firmware knows about a panel. The main
// loop never waits on it: it keeps one pending frame (the newest wins) and
// calls show() only when busy() is false, so the arm button is read as often
// as without a panel.
//
// The shipped image uses NullDisplay. Build with -DLINK_DISPLAY_SSD1681 and
// the EPD_*_PIN values to drive a 200 x 200 SSD1681-class e-paper module.
//
// Ssd1681Display is our own driver, written from the SSD1681 controller
// datasheet's command table. No code was taken from GxEPD2 (GPL-3.0), the
// Waveshare examples (no licence file) or any other display library. It has
// never been run on a panel: no panel was on hand when it was written.
#pragma once

#include <Arduino.h>

class LinkDisplay {
 public:
  virtual ~LinkDisplay() {}
  virtual bool begin() = 0;                              // false: no panel answered; the firmware keeps running
  virtual uint16_t width() const = 0;
  virtual uint16_t height() const = 0;
  virtual bool busy() = 0;                               // a refresh is still running
  virtual void show(const uint8_t *bits, bool full) = 0; // starts a refresh and returns at once
  virtual void sleep() = 0;                              // panel deep sleep; the picture stays
};

class NullDisplay : public LinkDisplay {
 public:
  bool begin() override { return false; }
  uint16_t width() const override { return 200; }
  uint16_t height() const override { return 200; }
  bool busy() override { return false; }
  void show(const uint8_t *, bool) override {}
  void sleep() override {}
};

#ifdef LINK_DISPLAY_SSD1681
#include <SPI.h>

#if !defined(EPD_CS_PIN) || !defined(EPD_DC_PIN) || !defined(EPD_RST_PIN) || !defined(EPD_BUSY_PIN) || !defined(EPD_SCK_PIN) || !defined(EPD_MOSI_PIN)
#error "LINK_DISPLAY_SSD1681 needs EPD_CS_PIN, EPD_DC_PIN, EPD_RST_PIN, EPD_BUSY_PIN, EPD_SCK_PIN and EPD_MOSI_PIN"
#endif

// SSD1681 commands used here (datasheet command table).
namespace ssd1681 {
const uint8_t DRIVER_OUTPUT = 0x01;   // MUX lines and gate scan order
const uint8_t DEEP_SLEEP = 0x10;      // 0x01 = deep sleep mode 1 (RAM kept)
const uint8_t DATA_ENTRY = 0x11;      // address counter direction
const uint8_t SW_RESET = 0x12;
const uint8_t TEMP_SENSOR = 0x18;     // 0x80 = internal sensor
const uint8_t MASTER_ACTIVATE = 0x20; // run the update sequence set by 0x22
const uint8_t UPDATE_CONTROL_2 = 0x22;
const uint8_t WRITE_RAM_BW = 0x24;    // new picture; bit 1 = white
const uint8_t WRITE_RAM_RED = 0x26;   // the previous picture, used by partial (mode 2) updates
const uint8_t BORDER = 0x3C;
const uint8_t RAM_X_RANGE = 0x44;
const uint8_t RAM_Y_RANGE = 0x45;
const uint8_t RAM_X_COUNTER = 0x4E;
const uint8_t RAM_Y_COUNTER = 0x4F;
const uint8_t SEQ_FULL = 0xF7;        // clock, analog, temperature, LUT mode 1, display, power off
const uint8_t SEQ_PARTIAL = 0xFF;     // the same with display mode 2 (differential)
}  // namespace ssd1681

class Ssd1681Display : public LinkDisplay {
 public:
  bool begin() override {
    pinMode(EPD_CS_PIN, OUTPUT);
    pinMode(EPD_DC_PIN, OUTPUT);
    pinMode(EPD_RST_PIN, OUTPUT);
    pinMode(EPD_BUSY_PIN, INPUT);
    digitalWrite(EPD_CS_PIN, HIGH);
    SPI.begin(EPD_SCK_PIN, -1, EPD_MOSI_PIN, EPD_CS_PIN);
    ok_ = init();
    return ok_;
  }
  uint16_t width() const override { return 200; }
  uint16_t height() const override { return 200; }
  bool busy() override { return ok_ && digitalRead(EPD_BUSY_PIN) == HIGH; }

  // Writes both RAM planes over SPI (about 10 KB, a few ms) and starts the
  // update. It does not wait for the update: busy() reports it.
  void show(const uint8_t *bits, bool full) override {
    if (!ok_) return;
    if (asleep_ && !init()) return;  // deep sleep needs a hardware reset to wake
    using namespace ssd1681;
    // Partial updates compare the new picture with the previous one.
    writePlane(WRITE_RAM_RED, full || !havePrev_ ? bits : prev_);
    writePlane(WRITE_RAM_BW, bits);
    command(UPDATE_CONTROL_2);
    data(full || !havePrev_ ? SEQ_FULL : SEQ_PARTIAL);
    command(MASTER_ACTIVATE);
    memcpy(prev_, bits, sizeof prev_);
    havePrev_ = true;
  }

  void sleep() override {
    if (!ok_ || asleep_ || busy()) return;
    command(ssd1681::DEEP_SLEEP);
    data(0x01);
    asleep_ = true;
  }

 private:
  bool ok_ = false;
  bool asleep_ = false;
  bool havePrev_ = false;
  uint8_t prev_[5000];

  void command(uint8_t c) {
    SPI.beginTransaction(SPISettings(10000000, MSBFIRST, SPI_MODE0));
    digitalWrite(EPD_DC_PIN, LOW);
    digitalWrite(EPD_CS_PIN, LOW);
    SPI.transfer(c);
    digitalWrite(EPD_CS_PIN, HIGH);
    SPI.endTransaction();
  }

  void data(uint8_t d) {
    SPI.beginTransaction(SPISettings(10000000, MSBFIRST, SPI_MODE0));
    digitalWrite(EPD_DC_PIN, HIGH);
    digitalWrite(EPD_CS_PIN, LOW);
    SPI.transfer(d);
    digitalWrite(EPD_CS_PIN, HIGH);
    SPI.endTransaction();
  }

  // Our buffer uses 1 = black; the controller's black/white RAM uses 1 = white.
  void writePlane(uint8_t ram, const uint8_t *bits) {
    using namespace ssd1681;
    command(RAM_X_COUNTER);
    data(0x00);
    command(RAM_Y_COUNTER);
    data(0x00);
    data(0x00);
    command(ram);
    SPI.beginTransaction(SPISettings(10000000, MSBFIRST, SPI_MODE0));
    digitalWrite(EPD_DC_PIN, HIGH);
    digitalWrite(EPD_CS_PIN, LOW);
    for (size_t i = 0; i < sizeof prev_; i++) SPI.transfer((uint8_t)~bits[i]);
    digitalWrite(EPD_CS_PIN, HIGH);
    SPI.endTransaction();
  }

  // Waits for BUSY to fall, at most `ms`. Used only during begin() and wake.
  bool waitIdle(uint32_t ms) {
    uint32_t start = millis();
    while (digitalRead(EPD_BUSY_PIN) == HIGH) {
      if (millis() - start > ms) return false;
      delay(1);
    }
    return true;
  }

  bool init() {
    using namespace ssd1681;
    digitalWrite(EPD_RST_PIN, LOW);
    delay(10);
    digitalWrite(EPD_RST_PIN, HIGH);
    delay(10);
    if (!waitIdle(100)) return false;
    command(SW_RESET);
    if (!waitIdle(100)) return false;  // no panel, or BUSY stuck high
    command(DRIVER_OUTPUT);  // 200 gate lines: MUX = 199
    data(0xC7);
    data(0x00);
    data(0x00);
    command(DATA_ENTRY);     // X increment, Y increment
    data(0x03);
    command(RAM_X_RANGE);    // 25 bytes per row
    data(0x00);
    data(0x18);
    command(RAM_Y_RANGE);    // rows 0 to 199
    data(0x00);
    data(0x00);
    data(0xC7);
    data(0x00);
    command(BORDER);
    data(0x05);
    command(TEMP_SENSOR);
    data(0x80);
    asleep_ = false;
    havePrev_ = false;  // RAM content is unknown after a reset: the next show is full
    return true;
  }
};
#endif  // LINK_DISPLAY_SSD1681
