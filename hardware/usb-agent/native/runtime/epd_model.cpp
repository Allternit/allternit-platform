// Native board: a model of a 200 x 200 SSD1681 e-paper panel (plan step 4).
//
// Written from the SSD1681 datasheet's command table only (command numbers,
// parameter counts, the 0x11 entry modes, the 0x22 control bits). It reads the
// SPI byte stream from shim/SPI.h, the DC and CS levels from the GPIO table,
// and the RST pin through native::gpioOnWrite(). It drives BUSY with
// native::gpioDrive() and reports a refresh as a `frame` event.
//
// Strict where a real panel is. Each of these emits `epd-error` with a short
// reason and does not change the picture: an SPI byte with CS high, with RST
// low, before any hardware reset, while BUSY is high or in deep sleep; data
// with no command selected; too many parameter bytes; an unknown command; a
// RAM write outside the window or the 200 x 200 area; a refresh before the
// planes were written; a refresh with the driver output not set to 200 lines.
// One error is reported per command, not one per byte.
//
// Timing assumed (the datasheet gives ranges, not one value):
//   hardware reset: pulse of at least 5 ms; BUSY high for 5 ms after RST rises
//   software reset (0x12): BUSY high for 5 ms
//   refresh: BUSY high for `epd busy <ms>` when set, else 1500 ms (full) or
//   300 ms (partial)
//   SPI: mode 0, MSB first, at most 10 MHz
// BUSY is high at once when a refresh starts and falls by board time, so
// `clock +<ms>` shortens it.
#include <Arduino.h>
#include <SPI.h>

#include <atomic>
#include <mutex>
#include <string>
#include <thread>

#include "native_runtime.h"

namespace native {
namespace {

const int W_BYTES = 25;
const int ROWS = 200;
const int PLANE = W_BYTES * ROWS;  // 5000
const uint32_t RESET_BUSY_MS = 5;
const uint32_t FULL_BUSY_MS = 1500;
const uint32_t PARTIAL_BUSY_MS = 300;
const uint32_t MIN_RESET_LOW_MS = 5;
const uint32_t MAX_SPI_HZ = 10000000;

// Parameter bytes per command; -1 = a RAM stream; -2 = not in the table.
int paramCount(int c) {
  switch (c) {
    case 0x01: return 3;   // driver output control
    case 0x03: return 1;   // gate driving voltage
    case 0x04: return 3;   // source driving voltage
    case 0x0C: return 5;   // booster soft start
    case 0x10: return 1;   // deep sleep mode
    case 0x11: return 1;   // data entry mode
    case 0x12: return 0;   // software reset
    case 0x18: return 1;   // temperature sensor control
    case 0x1A: return 2;   // write temperature register
    case 0x20: return 0;   // master activation
    case 0x21: return 2;   // display update control 1
    case 0x22: return 1;   // display update control 2
    case 0x24: return -1;  // write RAM (black / white)
    case 0x26: return -1;  // write RAM (red / previous)
    case 0x2C: return 1;   // VCOM
    case 0x3A: return 1;   // dummy line period
    case 0x3B: return 1;   // gate line width
    case 0x3C: return 1;   // border waveform
    case 0x44: return 2;   // RAM X range
    case 0x45: return 4;   // RAM Y range
    case 0x4E: return 1;   // RAM X counter
    case 0x4F: return 2;   // RAM Y counter
    default: return -2;
  }
}

std::string hexByte(int v) {
  static const char *d = "0123456789abcdef";
  std::string s = "0x";
  s += d[(v >> 4) & 15];
  s += d[v & 15];
  return s;
}

struct Model {
  std::mutex mu;
  bool attached = false;
  int sck = -1, mosi = -1, cs = -1, dc = -1, rst = -1, busyPin = -1;
  bool spiBegun = false;
  bool inTx = false;
  bool txOk = true;
  std::string txBad;

  // Reset and power.
  bool rstLow = false;
  uint32_t rstLowAt = 0;
  bool wasReset = false;
  bool asleep = false;

  // Command decoder.
  int cmd = -1;
  int got = 0;
  uint8_t params[8] = {0};
  bool latched = false;

  // Registers.
  int mux = 0;  // number of gate lines, from 0x01
  uint8_t entry = 0x03;
  int xs = 0, xe = W_BYTES - 1, ys = 0, ye = ROWS - 1;
  int cx = 0, cy = 0;
  uint8_t upd2 = 0;

  // RAM and picture. bw/red are the RAM bytes (1 = white). shown is what a
  // person sees, 1 = black, the face renderer's format.
  uint8_t bw[PLANE], red[PLANE], shown[PLANE];
  int bwCount = 0, redCount = 0;  // distinct bytes written since reset
  bool bwSeen[PLANE], redSeen[PLANE];

  // BUSY.
  bool busy = false;
  uint32_t busyUntil = 0;
  bool thread = false;

  Model() {
    memset(bw, 0xFF, sizeof bw);
    memset(red, 0xFF, sizeof red);
    memset(shown, 0x00, sizeof shown);
    clearRam();
  }

  void clearRam() {
    memset(bwSeen, 0, sizeof bwSeen);
    memset(redSeen, 0, sizeof redSeen);
    bwCount = redCount = 0;
  }

  void fail(const std::string &why) {
    if (latched) return;
    latched = true;
    emit("epd-error", "\"reason\":" + jsonString(why));
  }

  void setBusy(uint32_t ms) {
    busy = true;
    busyUntil = boardMillis() + ms;
    gpioDrive(busyPin, 1, false);
    if (!thread) {
      thread = true;
      std::thread([this]() {
        for (;;) {
          sleepRealMs(1);
          std::lock_guard<std::mutex> lock(mu);
          if (busy && (int32_t)(boardMillis() - busyUntil) >= 0) {
            busy = false;
            gpioDrive(busyPin, 0, false);
          }
        }
      }).detach();
    }
  }

  void defaults() {
    cmd = -1;
    got = 0;
    mux = 0;
    entry = 0x03;
    xs = 0; xe = W_BYTES - 1; ys = 0; ye = ROWS - 1;
    cx = cy = 0;
    upd2 = 0;
    clearRam();
  }

  void hardReset() {
    defaults();
    asleep = false;
    wasReset = true;
    setBusy(RESET_BUSY_MS);
  }

  // ── RAM ───────────────────────────────────────────────────────────────────
  void ramByte(bool isBw, uint8_t b) {
    int xlo = xs < xe ? xs : xe, xhi = xs < xe ? xe : xs;
    int ylo = ys < ye ? ys : ye, yhi = ys < ye ? ye : ys;
    bool inside = cx >= xlo && cx <= xhi && cy >= ylo && cy <= yhi && cx < W_BYTES && cy < ROWS && cx >= 0 && cy >= 0;
    if (!inside) {
      fail(std::string("ram write outside window (x=") + std::to_string(cx) + " y=" + std::to_string(cy) + ")");
    } else {
      int i = cy * W_BYTES + cx;
      if (isBw) {
        bw[i] = b;
        if (!bwSeen[i]) { bwSeen[i] = true; bwCount++; }
      } else {
        red[i] = b;
        if (!redSeen[i]) { redSeen[i] = true; redCount++; }
      }
    }
    bool xInc = entry & 1, yInc = entry & 2, yFirst = entry & 4;
    auto stepX = [&]() {
      cx += xInc ? 1 : -1;
      bool wrapped = cx > xhi || cx < xlo;
      if (wrapped) cx = xInc ? xlo : xhi;
      return wrapped;
    };
    auto stepY = [&]() {
      cy += yInc ? 1 : -1;
      bool wrapped = cy > yhi || cy < ylo;
      if (wrapped) cy = yInc ? ylo : yhi;
      return wrapped;
    };
    if (!yFirst) {
      // The counter walks off the last row: the next write is outside.
      if (stepX()) { cy += yInc ? 1 : -1; }
    } else {
      if (stepY()) { cx += xInc ? 1 : -1; }
    }
  }

  // ── refresh ───────────────────────────────────────────────────────────────
  void activate() {
    if (mux != ROWS) {
      fail(mux == 0 ? "refresh before driver output control (0x01) was set" : "driver output is " + std::to_string(mux) + " lines, panel has 200");
      return;
    }
    bool display = upd2 & 0x04, partial = upd2 & 0x08;
    if (!display) return;  // a sequence with no display step changes nothing visible
    if (bwCount < PLANE) {
      fail(bwCount == 0 ? "refresh before RAM 0x24 was written" : "refresh with RAM 0x24 incomplete (" + std::to_string(bwCount) + " of 5000 bytes)");
      return;
    }
    if (partial && redCount < PLANE) {
      fail(redCount == 0 ? "partial refresh before RAM 0x26 was written" : "partial refresh with RAM 0x26 incomplete (" + std::to_string(redCount) + " of 5000 bytes)");
      return;
    }
    if (!partial) {
      for (int i = 0; i < PLANE; i++) shown[i] = (uint8_t)~bw[i];
    } else {
      // Differential: a pixel that differs between the planes takes the new
      // value, an equal pixel stays as it is.
      for (int i = 0; i < PLANE; i++) {
        uint8_t changed = bw[i] ^ red[i];
        shown[i] = (uint8_t)((shown[i] & ~changed) | (~bw[i] & changed));
      }
    }
    emit("frame", std::string("\"full\":") + (partial ? "false" : "true") + ",\"hex\":\"" + hex(shown, PLANE) + "\"");
    uint32_t ms = epdBusyMs();
    setBusy(ms ? ms : (partial ? PARTIAL_BUSY_MS : FULL_BUSY_MS));
  }

  // ── decoder ───────────────────────────────────────────────────────────────
  void finishParams() {
    switch (cmd) {
      case 0x01: mux = (params[0] | ((params[1] & 1) << 8)) + 1; break;
      case 0x10:
        if (params[0] == 0x00 || params[0] == 0x01 || params[0] == 0x03) {
          asleep = true;
          if (params[0] == 0x03) clearRam();
        } else {
          fail("deep sleep mode " + hexByte(params[0]) + " is not in the command table");
        }
        break;
      case 0x11: entry = params[0] & 7; break;
      case 0x22: upd2 = params[0]; break;
      case 0x44:
        xs = params[0] & 0x1F; xe = params[1] & 0x1F;
        if (xs >= W_BYTES || xe >= W_BYTES) fail("RAM X range beyond the 200 pixel panel");
        break;
      case 0x45:
        ys = params[0] | ((params[1] & 1) << 8);
        ye = params[2] | ((params[3] & 1) << 8);
        if (ys >= ROWS || ye >= ROWS) fail("RAM Y range beyond the 200 row panel");
        break;
      case 0x4E: cx = params[0] & 0x1F; break;
      case 0x4F: cy = params[0] | ((params[1] & 1) << 8); break;
      default: break;
    }
  }

  void byteIn(uint8_t b) {
    bool isData = gpioLevel(dc) == 1;
    if (!isData) latched = false;  // a new command: report its errors afresh
    if (gpioLevel(cs) != 0) { fail("SPI byte with CS high"); return; }
    if (gpioLevel(rst) == 0) { fail("SPI byte while RST is low"); return; }
    if (!wasReset) { fail("SPI byte before any hardware reset"); return; }
    if (busy) { fail(isData ? "data while BUSY is high" : "command " + hexByte(b) + " while BUSY is high"); cmd = -1; return; }
    if (asleep) { fail("SPI byte in deep sleep (needs a hardware reset)"); cmd = -1; return; }
    if (!isData) {
      if (cmd >= 0 && got < paramCount(cmd) && paramCount(cmd) > 0) {
        fail("command " + hexByte(cmd) + " got " + std::to_string(got) + " of " + std::to_string(paramCount(cmd)) + " parameter bytes");
        latched = false;
      }
      int n = paramCount(b);
      cmd = b;
      got = 0;
      if (n == -2) { fail("unknown command " + hexByte(b)); cmd = -1; return; }
      if (b == 0x12) {
        defaults();
        setBusy(RESET_BUSY_MS);
      } else if (b == 0x20) {
        activate();
      }
      return;
    }
    if (cmd < 0) { fail("data with no command selected"); return; }
    int n = paramCount(cmd);
    if (n == -1) { ramByte(cmd == 0x24, b); return; }
    if (got >= n) { fail("extra data byte after command " + hexByte(cmd)); return; }
    params[got++] = b;
    if (got == n) finishParams();
  }
};

Model &model() {
  static Model *m = new Model();  // never destroyed: the BUSY thread may outlive exit()
  return *m;
}

void onWrite(int pin, int level) {
  Model &m = model();
  std::lock_guard<std::mutex> lock(m.mu);
  if (!m.attached || pin != m.rst) return;
  if (level == 0) {
    if (!m.rstLow) { m.rstLow = true; m.rstLowAt = boardMillis(); }
  } else if (m.rstLow) {
    m.rstLow = false;
    uint32_t held = boardMillis() - m.rstLowAt;
    if (held < MIN_RESET_LOW_MS) {
      m.latched = false;
      m.fail("reset pulse of " + std::to_string(held) + " ms is shorter than " + std::to_string(MIN_RESET_LOW_MS));
    }
    m.hardReset();
  }
}

}  // namespace

void epdAttach(int sck, int mosi, int cs, int dc, int rst, int busy) {
  Model &m = model();
  std::lock_guard<std::mutex> lock(m.mu);
  m.attached = true;
  m.sck = sck; m.mosi = mosi; m.cs = cs; m.dc = dc; m.rst = rst; m.busyPin = busy;
  gpioQuiet(cs);
  gpioQuiet(dc);
  gpioQuiet(rst);
  gpioQuiet(busy);
  gpioOnWrite(onWrite);
}

void epdSpiBegin(int sck, int miso, int mosi, int ss) {
  Model &m = model();
  std::lock_guard<std::mutex> lock(m.mu);
  if (!m.attached) return;
  m.spiBegun = true;
  if (sck != m.sck || mosi != m.mosi) {
    m.latched = false;
    m.fail("SPI.begin pins sck=" + std::to_string(sck) + " mosi=" + std::to_string(mosi) + " are not the panel's");
  }
}

void epdSpiTransaction(bool open, uint32_t clock, uint8_t bitOrder, uint8_t dataMode) {
  Model &m = model();
  std::lock_guard<std::mutex> lock(m.mu);
  if (!m.attached) return;
  m.inTx = open;
  if (!open) return;
  m.txOk = true;
  if (dataMode != SPI_MODE0) { m.txOk = false; m.txBad = "SPI mode " + std::to_string(dataMode) + ", the panel needs mode 0"; }
  else if (bitOrder != SPI_MSBFIRST) { m.txOk = false; m.txBad = "SPI is LSB first, the panel needs MSB first"; }
  else if (clock > MAX_SPI_HZ) { m.txOk = false; m.txBad = "SPI clock " + std::to_string(clock) + " Hz is above 10 MHz"; }
}

uint8_t epdSpiTransfer(uint8_t data) {
  Model &m = model();
  std::lock_guard<std::mutex> lock(m.mu);
  if (!m.attached) return 0;
  if (!m.spiBegun) { m.latched = false; m.fail("SPI.transfer before SPI.begin"); return 0; }
  if (!m.inTx) { m.latched = false; m.fail("SPI.transfer outside beginTransaction"); return 0; }
  if (!m.txOk) { m.latched = false; m.fail(m.txBad); return 0; }
  m.byteIn(data);
  return 0;
}

}  // namespace native
