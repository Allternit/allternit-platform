// Native board: time, restart, reset reason, MAC, random, task watchdog,
// USB names, the HID recorder and USB CDC serial on the pty.
#include <Arduino.h>
#include <USB.h>
#include <USBHIDKeyboard.h>
#include <USBHIDMouse.h>
#include <esp_mac.h>
#include <esp_random.h>
#include <esp_task_wdt.h>
#include <errno.h>
#include <native_runtime.h>
#include <poll.h>
#include <unistd.h>

#include <deque>
#include <mutex>
#include <string>

#ifdef __APPLE__
#include <sys/random.h>
#endif

using namespace native;

// ── time (esp32-hal.h) ──────────────────────────────────────────────────────

uint32_t millis() {
  return boardMillis();
}

void delay(uint32_t ms) {
  sleepRealMs(ms);  // real time, so 8 ms pacing is real (decision 6)
}

void delayMicroseconds(uint32_t us) {
  timespec ts = {(time_t)(us / 1000000), (long)(us % 1000000) * 1000L};
  while (nanosleep(&ts, &ts) != 0 && errno == EINTR) {}
}

// ── restart and reset reason ────────────────────────────────────────────────

EspClass ESP;

void EspClass::restart() {
  native::reboot(ESP_RST_SW);
}

extern "C" esp_reset_reason_t esp_reset_reason(void) {
  return (esp_reset_reason_t)bootReason();
}

// ── MAC ─────────────────────────────────────────────────────────────────────

static bool parseMac(const char *s, uint8_t out[6]) {
  int got = 0;
  for (const char *p = s; *p && got < 12; p++) {
    char c = *p;
    int v;
    if (c >= '0' && c <= '9') v = c - '0';
    else if (c >= 'a' && c <= 'f') v = c - 'a' + 10;
    else if (c >= 'A' && c <= 'F') v = c - 'A' + 10;
    else if (c == ':' || c == '-') continue;
    else return false;
    if (got % 2 == 0) out[got / 2] = (uint8_t)(v << 4);
    else out[got / 2] |= (uint8_t)v;
    got++;
  }
  return got == 12;
}

static void stationMac(uint8_t mac[6]) {
  static const uint8_t fallback[6] = {0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff};
  const char *env = getenv("LINK_NATIVE_MAC");
  if (!env || !parseMac(env, mac)) memcpy(mac, fallback, 6);
}

extern "C" esp_err_t esp_read_mac(uint8_t *mac, esp_mac_type_t type) {
  if (!mac) return ESP_ERR_INVALID_ARG;
  stationMac(mac);
  // The other interfaces derive from the base MAC on a board; only the
  // station MAC is used by the firmware.
  if (type != ESP_MAC_WIFI_STA) mac[5] = (uint8_t)(mac[5] + (int)type);
  return ESP_OK;
}

namespace native {
std::string boardIdFromMac() {
  uint8_t mac[6];
  stationMac(mac);
  return "usba-" + hex(mac, 6);
}
}  // namespace native

// ── random ──────────────────────────────────────────────────────────────────

extern "C" void esp_fill_random(void *buf, size_t len) {
  uint8_t *p = (uint8_t *)buf;
  while (len) {
    size_t n = len > 256 ? 256 : len;
    if (getentropy(p, n) != 0) abort();
    p += n;
    len -= n;
  }
}

extern "C" uint32_t esp_random(void) {
  uint32_t v;
  esp_fill_random(&v, sizeof v);
  return v;
}

// ── task watchdog ───────────────────────────────────────────────────────────

extern "C" esp_err_t esp_task_wdt_reconfigure(const esp_task_wdt_config_t *config) {
  if (!config || config->timeout_ms == 0) return ESP_ERR_INVALID_ARG;
  wdtConfigure(config->timeout_ms);
  return ESP_OK;
}

extern "C" esp_err_t esp_task_wdt_add(TaskHandle_t task_handle) {
  (void)task_handle;  // NULL = the calling task, the only one here
  wdtSubscribe();
  return ESP_OK;
}

extern "C" esp_err_t esp_task_wdt_reset(void) {
  return wdtFeed() ? ESP_OK : ESP_ERR_NOT_FOUND;
}

// ── USB device names ────────────────────────────────────────────────────────

ESPUSB USB;
bool ESPUSB::productName(const char *) { return true; }
bool ESPUSB::manufacturerName(const char *) { return true; }
bool ESPUSB::begin() { return true; }

// ── HID recorder ────────────────────────────────────────────────────────────

void USBHIDMouseBase::begin(void) {}

void USBHIDMouseBase::press(uint8_t b) {
  this->buttons(_buttons | b);
}

void USBHIDMouseBase::release(uint8_t b) {
  this->buttons(_buttons & ~b);
}

bool USBHIDMouseBase::isPressed(uint8_t b) {
  return (b & _buttons) > 0;
}

void USBHIDRelativeMouse::buttons(uint8_t b) {
  if (b != _buttons) {
    _buttons = b;
    move(0, 0);
  }
}

void USBHIDRelativeMouse::move(int8_t x, int8_t y, int8_t wheel, int8_t pan) {
  emit("hid", "\"dev\":\"mouse\",\"buttons\":" + std::to_string(_buttons) + ",\"x\":" + std::to_string(x) +
                  ",\"y\":" + std::to_string(y) + ",\"wheel\":" + std::to_string(wheel) + ",\"pan\":" + std::to_string(pan) +
                  ",\"sent\":" + (usbDelivers() ? "1" : "0"));
}

// Which ASCII codes the en_US layout has a key for: backspace, tab, newline,
// escape and 0x20 to 0x7e. Only "has a key" is modelled here.
const uint8_t KeyboardLayout_en_US[128] = {
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0,
    1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
    1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
    1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0,
};

static bool keyExists(uint8_t k) {
  return k >= 0x80 || KeyboardLayout_en_US[k];
}

static void keyEvent(const char *call, int ch) {
  std::string f = "\"dev\":\"keyboard\",\"call\":\"" + std::string(call) + "\"";
  if (ch >= 0) f += ",\"ch\":" + std::to_string(ch);
  f += std::string(",\"sent\":") + (usbDelivers() ? "1" : "0");
  emit("hid", f);
}

void USBHIDKeyboard::begin(const uint8_t *) {}

size_t USBHIDKeyboard::press(uint8_t k) {
  if (!keyExists(k)) return 0;
  keyEvent("press", k);
  return 1;
}

size_t USBHIDKeyboard::release(uint8_t k) {
  if (!keyExists(k)) return 0;
  keyEvent("release", k);
  return 1;
}

size_t USBHIDKeyboard::write(uint8_t k) {
  if (!keyExists(k)) return 0;  // press() fails, release() sends nothing either
  keyEvent("write", k);         // one call = a key-down and a key-up report
  return 1;
}

void USBHIDKeyboard::releaseAll(void) {
  keyEvent("releaseAll", -1);
}

// ── USB CDC serial on the pty ───────────────────────────────────────────────
// The rx queue holds 256 bytes; past that, bytes stay in the pty, as USB CDC
// leaves them with the host (NAK) instead of dropping them. A write waits at
// most 100 ms for room, then drops the rest, like the CDC tx timeout.

USBCDC USBSerial;
static int g_master = -1;
static std::deque<uint8_t> g_rx;
static const size_t RX_QUEUE = 256;

namespace native {
void serialInit(int masterFd) {
  g_master = masterFd;
}
}  // namespace native

static void fillRx() {
  if (g_master < 0 || g_rx.size() >= RX_QUEUE) return;
  uint8_t buf[RX_QUEUE];
  ssize_t n = ::read(g_master, buf, RX_QUEUE - g_rx.size());
  if (n > 0) g_rx.insert(g_rx.end(), buf, buf + n);
  // EAGAIN: nothing waiting. EIO: no slave open. Both are "no data".
}

void USBCDC::begin(unsigned long) {}

int USBCDC::available(void) {
  fillRx();
  return (int)g_rx.size();
}

int USBCDC::read(void) {
  if (g_rx.empty()) fillRx();
  if (g_rx.empty()) return -1;
  uint8_t c = g_rx.front();
  g_rx.pop_front();
  return c;
}

void USBCDC::flush(void) {}

size_t USBCDC::write(uint8_t c) {
  return write(&c, 1);
}

size_t USBCDC::write(const uint8_t *buffer, size_t size) {
  if (g_master < 0) return 0;
  size_t done = 0;
  uint64_t deadline = realMonoMs() + 100;
  while (done < size) {
    ssize_t n = ::write(g_master, buffer + done, size - done);
    if (n > 0) {
      done += (size_t)n;
      continue;
    }
    if (n < 0 && errno == EINTR) continue;
    if (n < 0 && errno != EAGAIN) break;
    uint64_t now = realMonoMs();
    if (now >= deadline) break;
    pollfd p = {g_master, POLLOUT, 0};
    poll(&p, 1, (int)(deadline - now));
  }
  return done;
}
