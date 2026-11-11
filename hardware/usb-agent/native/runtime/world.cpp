// Native board: the world outside the board, set from the control thread
// (decision 8): GPIO levels driven from outside, the USB host, Wi-Fi in the
// air, the mDNS table, the panel's busy time, and the clock skew.
//
// Board-side models that read this world: Wi-Fi station (WiFi.h), mDNS
// (ESPmDNS.h), GPIO (Arduino.h, driver/gpio.h), TinyUSB (tusb.h).
#include <Arduino.h>
#include <ESPmDNS.h>
#include <WiFi.h>
#include <arpa/inet.h>
#include <driver/gpio.h>
#include <native_runtime.h>
#include <netinet/in.h>
#include <tusb.h>

#include <atomic>
#include <map>
#include <mutex>
#include <sstream>
#include <string>
#include <vector>

namespace native {

static std::mutex g_world;  // guards everything below; never held while sleeping or calling an ISR

// ── helpers ─────────────────────────────────────────────────────────────────

static std::vector<std::string> words(const std::string &line) {
  std::vector<std::string> out;
  std::istringstream in(line);
  std::string w;
  while (in >> w) out.push_back(w);
  return out;
}

static bool parseUint(const std::string &s, uint64_t max, uint64_t *out) {
  if (s.empty() || s.size() > 12) return false;
  uint64_t v = 0;
  for (char c : s) {
    if (c < '0' || c > '9') return false;
    v = v * 10 + (uint64_t)(c - '0');
  }
  if (v > max) return false;
  *out = v;
  return true;
}

static bool parseHexText(const std::string &h, std::string *out) {
  if (h.size() % 2) return false;
  std::string s;
  for (size_t i = 0; i < h.size(); i += 2) {
    auto nib = [](char c) -> int {
      if (c >= '0' && c <= '9') return c - '0';
      if (c >= 'a' && c <= 'f') return c - 'a' + 10;
      if (c >= 'A' && c <= 'F') return c - 'A' + 10;
      return -1;
    };
    int hi = nib(h[i]), lo = nib(h[i + 1]);
    if (hi < 0 || lo < 0) return false;
    s += (char)((hi << 4) | lo);
  }
  *out = s;
  return true;
}

static std::string hexOf(const std::string &s) {
  return hex((const uint8_t *)s.data(), s.size());
}

// ── GPIO ────────────────────────────────────────────────────────────────────

struct Pin {
  int mode = -1;     // -1: never set
  int latch = 0;     // output register; 0 after reset
  int driven = -1;   // level held from outside (`pin n v`), -1 none
  void (*isr)(void) = nullptr;
  int isrMode = 0;
  bool quiet = false;
};
static Pin g_pins[NUM_DIGITAL_PINS];
static GpioWriteHook g_writeHook = nullptr;

static int levelLocked(int pin) {
  const Pin &p = g_pins[pin];
  if (p.mode == OUTPUT || p.mode == OUTPUT_OPEN_DRAIN) return p.latch;
  if (p.driven >= 0) return p.driven;
  if (p.mode == INPUT_PULLUP || p.mode == PULLUP) return 1;
  return 0;  // INPUT with nothing driving it, or never configured
}

int gpioLevel(int pin) {
  if (pin < 0 || pin >= NUM_DIGITAL_PINS) return 0;
  std::lock_guard<std::mutex> lock(g_world);
  return levelLocked(pin);
}

void gpioDrive(int pin, int level, bool fireInterrupt) {
  if (pin < 0 || pin >= NUM_DIGITAL_PINS) return;
  void (*isr)(void) = nullptr;
  {
    std::lock_guard<std::mutex> lock(g_world);
    int before = levelLocked(pin);
    g_pins[pin].driven = level ? 1 : 0;
    int after = levelLocked(pin);
    Pin &p = g_pins[pin];
    if (fireInterrupt && p.isr && before != after) {
      switch (p.isrMode) {
        case CHANGE: isr = p.isr; break;
        case RISING: if (after == 1) isr = p.isr; break;
        case FALLING: if (after == 0) isr = p.isr; break;
        case ONHIGH: if (after == 1) isr = p.isr; break;
        case ONLOW: if (after == 0) isr = p.isr; break;
        default: break;
      }
    }
  }
  if (isr) isr();  // on the control thread, like an interrupt that can land at any time
}

void gpioQuiet(int pin) {
  if (pin < 0 || pin >= NUM_DIGITAL_PINS) return;
  std::lock_guard<std::mutex> lock(g_world);
  g_pins[pin].quiet = true;
}

void gpioOnWrite(GpioWriteHook hook) {
  std::lock_guard<std::mutex> lock(g_world);
  g_writeHook = hook;
}

// ── USB host ────────────────────────────────────────────────────────────────

static UsbState g_usb = USB_MOUNTED;
static bool g_usbWake = false;
static uint64_t g_resumeAt = 0;  // real ms; 0 = no resume pending

static void serviceResumeLocked() {
  if (g_resumeAt && realMonoMs() >= g_resumeAt) {
    g_resumeAt = 0;
    if (g_usb == USB_ASLEEP) g_usb = USB_MOUNTED;
  }
}

UsbState usbState() {
  std::lock_guard<std::mutex> lock(g_world);
  serviceResumeLocked();
  return g_usb;
}

bool usbDelivers() {
  return usbState() == USB_MOUNTED;
}

// ── Wi-Fi (station) ─────────────────────────────────────────────────────────

static const uint32_t WIFI_JOIN_MS = 500;  // board time from "network seen" to connected

struct AirNet {
  std::string ssid, pass;  // pass "" = open
};
static std::vector<AirNet> g_air;
static bool g_staHasTarget = false;
static std::string g_staSsid, g_staPass;
static bool g_staConnected = false;
static bool g_staJoinPending = false;
static uint32_t g_staJoinAt = 0;
static bool g_staAutoReconnect = false;

static void wifiEvent(const char *state, const std::string &ssid) {
  emit("wifi", "\"state\":" + jsonString(state) + ",\"ssid\":" + jsonString(hexOf(ssid)));
}

// Updates the station; returns events to emit after the lock is released.
static wl_status_t staStatusLocked(std::vector<std::pair<const char *, std::string>> *events) {
  if (!g_staHasTarget) return g_staConnected ? WL_CONNECTED : WL_DISCONNECTED;
  bool seen = false, match = false;
  for (const auto &n : g_air) {
    if (n.ssid != g_staSsid) continue;
    seen = true;
    if (n.pass == g_staPass) match = true;
  }
  uint32_t now = boardMillis();
  if (g_staConnected && !match) {
    g_staConnected = false;
    g_staJoinPending = false;
    events->push_back({"disconnected", g_staSsid});
    if (!g_staAutoReconnect) {
      g_staHasTarget = false;
      return WL_CONNECTION_LOST;
    }
  }
  if (!g_staConnected && match) {
    if (!g_staJoinPending) {
      g_staJoinPending = true;
      g_staJoinAt = now + WIFI_JOIN_MS;
    }
    if ((int32_t)(now - g_staJoinAt) >= 0) {
      g_staConnected = true;
      g_staJoinPending = false;
      events->push_back({"connected", g_staSsid});
    }
  }
  if (!match) g_staJoinPending = false;
  if (g_staConnected) return WL_CONNECTED;
  if (!seen) return WL_NO_SSID_AVAIL;
  if (!match) return WL_CONNECT_FAILED;
  return WL_DISCONNECTED;
}

static wl_status_t staStatus() {
  std::vector<std::pair<const char *, std::string>> events;
  wl_status_t s;
  {
    std::lock_guard<std::mutex> lock(g_world);
    s = staStatusLocked(&events);
  }
  for (auto &e : events) wifiEvent(e.first, e.second);
  return s;
}

bool wifiConnected() {
  return staStatus() == WL_CONNECTED;
}

// ── mDNS table ──────────────────────────────────────────────────────────────

struct MdnsEntry {
  std::string ip;
  uint16_t port;
  std::string desk;
};
static std::vector<MdnsEntry> g_mdns;
static uint32_t g_mdnsStallMs = 0;  // one-shot
static bool g_mdnsBegun = false;
static std::vector<MdnsEntry> g_mdnsResults;

// ── panel knob ──────────────────────────────────────────────────────────────

static std::atomic<uint32_t> g_epdBusyMs{0};
uint32_t epdBusyMs() {
  return g_epdBusyMs.load();
}

// ── hosts (LINK_NATIVE_HOSTS) ───────────────────────────────────────────────

static bool fillAddr(const std::string &ip, uint16_t port, sockaddr_storage *out, socklen_t *outLen) {
  memset(out, 0, sizeof *out);
  sockaddr_in *v4 = (sockaddr_in *)out;
  if (inet_pton(AF_INET, ip.c_str(), &v4->sin_addr) == 1) {
    v4->sin_family = AF_INET;
    v4->sin_port = htons(port);
#ifdef __APPLE__
    v4->sin_len = sizeof *v4;
#endif
    *outLen = sizeof *v4;
    return true;
  }
  sockaddr_in6 *v6 = (sockaddr_in6 *)out;
  if (inet_pton(AF_INET6, ip.c_str(), &v6->sin6_addr) == 1) {
    v6->sin6_family = AF_INET6;
    v6->sin6_port = htons(port);
#ifdef __APPLE__
    v6->sin6_len = sizeof *v6;
#endif
    *outLen = sizeof *v6;
    return true;
  }
  return false;
}

bool resolveHost(const char *host, uint16_t port, sockaddr_storage *out, socklen_t *outLen) {
  if (!host || !wifiConnected()) return false;
  std::string want = host;
  if (const char *map = getenv("LINK_NATIVE_HOSTS")) {
    std::string all = map;
    size_t at = 0;
    while (at <= all.size()) {
      size_t end = all.find(',', at);
      if (end == std::string::npos) end = all.size();
      std::string item = all.substr(at, end - at);
      at = end + 1;
      size_t eq = item.find('='), colon = item.rfind(':');
      if (eq == std::string::npos || colon == std::string::npos || colon < eq) continue;
      if (item.substr(0, eq) != want) continue;
      uint64_t p;
      if (!parseUint(item.substr(colon + 1), 65535, &p)) return false;
      return fillAddr(item.substr(eq + 1, colon - eq - 1), (uint16_t)p, out, outLen);
    }
  }
  return fillAddr(want, port, out, outLen);  // an IP literal; there is no DNS here
}

// ── control commands ────────────────────────────────────────────────────────

std::string control(const std::string &line) {
  std::vector<std::string> w = words(line);
  if (w.empty()) return "";
  const std::string &op = w[0];
  uint64_t v;

  if (op == "pin") {
    if (w.size() != 3 || !parseUint(w[1], NUM_DIGITAL_PINS - 1, &v) || (w[2] != "0" && w[2] != "1")) return "usage: pin <n> <0|1>";
    gpioDrive((int)v, w[2] == "1", true);
    return "";
  }
  if (op == "usb") {
    if (w.size() != 2) return "usage: usb mounted|asleep|none";
    std::lock_guard<std::mutex> lock(g_world);
    if (w[1] == "mounted") g_usb = USB_MOUNTED;
    else if (w[1] == "asleep") g_usb = USB_ASLEEP;
    else if (w[1] == "none") g_usb = USB_NONE;
    else return "usage: usb mounted|asleep|none";
    g_resumeAt = 0;
    return "";
  }
  if (op == "usb-wake") {
    if (w.size() != 2 || (w[1] != "on" && w[1] != "off")) return "usage: usb-wake on|off";
    std::lock_guard<std::mutex> lock(g_world);
    g_usbWake = w[1] == "on";
    return "";
  }
  if (op == "clock") {
    if (w.size() != 2 || w[1].size() < 2 || w[1][0] != '+' || !parseUint(w[1].substr(1), 1ull << 40, &v)) return "usage: clock +<ms>";
    addSkew(v);
    return "";
  }
  if (op == "wifi") {
    if (w.size() == 2 && w[1] == "clear") {
      {
        std::lock_guard<std::mutex> lock(g_world);
        g_air.clear();
      }
      return "";
    }
    if (w.size() != 4 || w[1] != "air") return "usage: wifi air <ssidhex> <passhex|-> | wifi clear";
    AirNet n;
    if (!parseHexText(w[2], &n.ssid) || n.ssid.empty() || n.ssid.size() > 32) return "bad ssid hex";
    if (w[3] != "-" && (!parseHexText(w[3], &n.pass) || n.pass.empty() || n.pass.size() > 63)) return "bad pass hex";
    std::lock_guard<std::mutex> lock(g_world);
    for (auto &e : g_air) {
      if (e.ssid == n.ssid) {
        e.pass = n.pass;
        return "";
      }
    }
    g_air.push_back(n);
    return "";
  }
  if (op == "mdns") {
    if (w.size() == 2 && w[1] == "clear") {
      std::lock_guard<std::mutex> lock(g_world);
      g_mdns.clear();
      return "";
    }
    if (w.size() == 3 && w[1] == "stall") {
      if (!parseUint(w[2], 600000, &v)) return "usage: mdns stall <ms>";
      std::lock_guard<std::mutex> lock(g_world);
      g_mdnsStallMs = (uint32_t)v;
      return "";
    }
    if (w.size() == 5 && w[1] == "set") {
      in_addr a;
      if (inet_pton(AF_INET, w[2].c_str(), &a) != 1) return "bad mdns ip";
      if (!parseUint(w[3], 65535, &v) || v == 0) return "bad mdns port";
      std::lock_guard<std::mutex> lock(g_world);
      for (auto &e : g_mdns) {
        if (e.desk == w[4]) {
          e.ip = w[2];
          e.port = (uint16_t)v;
          return "";
        }
      }
      g_mdns.push_back({w[2], (uint16_t)v, w[4]});
      return "";
    }
    return "usage: mdns set <ip> <port> <deskId> | mdns clear | mdns stall <ms>";
  }
  if (op == "epd") {
    if (w.size() != 3 || w[1] != "busy" || !parseUint(w[2], 600000, &v)) return "usage: epd busy <ms>";
    g_epdBusyMs = (uint32_t)v;
    return "";
  }
  return "unknown command";
}

std::string worldCommands() {
  std::lock_guard<std::mutex> lock(g_world);
  serviceResumeLocked();
  std::string out;
  auto add = [&](const std::string &c) { out += c + "\n"; };
  for (int i = 0; i < NUM_DIGITAL_PINS; i++) {
    if (g_pins[i].driven >= 0) add("pin " + std::to_string(i) + " " + std::to_string(g_pins[i].driven));
  }
  if (g_usb == USB_ASLEEP) add("usb asleep");
  if (g_usb == USB_NONE) add("usb none");
  if (g_usbWake) add("usb-wake on");
  for (const auto &n : g_air) add("wifi air " + hexOf(n.ssid) + " " + (n.pass.empty() ? std::string("-") : hexOf(n.pass)));
  for (const auto &e : g_mdns) add("mdns set " + e.ip + " " + std::to_string(e.port) + " " + e.desk);
  if (g_epdBusyMs.load()) add("epd busy " + std::to_string(g_epdBusyMs.load()));
  return out;
}

}  // namespace native

using namespace native;

// ── GPIO API (esp32-hal-gpio.h, driver/gpio.h) ──────────────────────────────

void pinMode(uint8_t pin, uint8_t mode) {
  if (pin >= NUM_DIGITAL_PINS) return;  // the core logs "Invalid pin" and does nothing
  bool announce;
  int latch;
  {
    std::lock_guard<std::mutex> lock(g_world);
    g_pins[pin].mode = mode;
    announce = (mode == OUTPUT || mode == OUTPUT_OPEN_DRAIN) && !g_pins[pin].quiet;
    latch = g_pins[pin].latch;
  }
  if (announce) emit("pin", "\"n\":" + std::to_string(pin) + ",\"v\":" + std::to_string(latch));
}

void digitalWrite(uint8_t pin, uint8_t val) {
  if (pin >= NUM_DIGITAL_PINS) return;
  int level = val ? 1 : 0;
  bool announce;
  GpioWriteHook hook;
  {
    std::lock_guard<std::mutex> lock(g_world);
    Pin &p = g_pins[pin];
    announce = (p.mode == OUTPUT || p.mode == OUTPUT_OPEN_DRAIN) && !p.quiet && p.latch != level;
    p.latch = level;
    hook = g_writeHook;
  }
  if (announce) emit("pin", "\"n\":" + std::to_string(pin) + ",\"v\":" + std::to_string(level));
  if (hook) hook(pin, level);
}

int digitalRead(uint8_t pin) {
  return gpioLevel(pin);
}

void attachInterrupt(uint8_t pin, void (*fn)(void), int mode) {
  if (pin >= NUM_DIGITAL_PINS) return;
  std::lock_guard<std::mutex> lock(g_world);
  g_pins[pin].isr = fn;
  g_pins[pin].isrMode = mode;
}

void detachInterrupt(uint8_t pin) {
  if (pin >= NUM_DIGITAL_PINS) return;
  std::lock_guard<std::mutex> lock(g_world);
  g_pins[pin].isr = nullptr;
}

extern "C" int gpio_get_level(gpio_num_t gpio_num) {
  return gpioLevel((int)gpio_num);
}

// ── TinyUSB device state (tusb.h) ───────────────────────────────────────────

extern "C" bool tud_mounted(void) {
  return usbState() != USB_NONE;
}

extern "C" bool tud_suspended(void) {
  return usbState() == USB_ASLEEP;
}

extern "C" bool tud_remote_wakeup(void) {
  std::lock_guard<std::mutex> lock(g_world);
  serviceResumeLocked();
  if (g_usb != USB_ASLEEP || !g_usbWake) return false;  // the host did not enable remote wakeup
  if (!g_resumeAt) g_resumeAt = realMonoMs() + 20;
  return true;
}

// ── WiFi (station) ──────────────────────────────────────────────────────────

WiFiClass WiFi;

wl_status_t WiFiClass::begin(const char *ssid, const char *passphrase, int32_t, const uint8_t *, bool tryConnect) {
  if (!ssid || !*ssid || strlen(ssid) > 32) return WL_CONNECT_FAILED;
  if (passphrase && strlen(passphrase) > 64) return WL_CONNECT_FAILED;
  std::vector<std::pair<const char *, std::string>> events;
  std::string target = ssid;
  {
    std::lock_guard<std::mutex> lock(g_world);
    if (g_staConnected) events.push_back({"disconnected", g_staSsid});
    g_staConnected = false;
    g_staJoinPending = false;
    g_staHasTarget = tryConnect;
    g_staSsid = target;
    g_staPass = passphrase ? passphrase : "";
  }
  for (auto &e : events) wifiEvent(e.first, e.second);
  if (tryConnect) wifiEvent("joining", target);
  return WL_DISCONNECTED;
}

wl_status_t WiFiClass::status() {
  return staStatus();
}

bool WiFiClass::disconnect(bool, bool, unsigned long) {
  std::vector<std::pair<const char *, std::string>> events;
  {
    std::lock_guard<std::mutex> lock(g_world);
    if (g_staConnected) events.push_back({"disconnected", g_staSsid});
    g_staConnected = false;
    g_staJoinPending = false;
    g_staHasTarget = false;
  }
  for (auto &e : events) wifiEvent(e.first, e.second);
  return true;
}

bool WiFiClass::setAutoReconnect(bool autoReconnect) {
  std::lock_guard<std::mutex> lock(g_world);
  g_staAutoReconnect = autoReconnect;
  return true;
}

bool WiFiClass::setHostname(const char *) {
  return true;
}

bool WiFiClass::setSleep(bool) {
  return true;
}

// ── mDNS ────────────────────────────────────────────────────────────────────

MDNSResponder MDNS;

bool MDNSResponder::begin(const String &hostName) {
  if (hostName.length() == 0) return false;
  std::lock_guard<std::mutex> lock(g_world);
  g_mdnsBegun = true;
  return true;
}

int MDNSResponder::queryService(char *service, char *proto) {
  if (!service || !service[0] || !proto || !proto[0]) return 0;
  std::string srv = service[0] == '_' ? service : std::string("_") + service;
  std::string prt = proto[0] == '_' ? proto : std::string("_") + proto;
  uint32_t wait;
  {
    std::lock_guard<std::mutex> lock(g_world);
    g_mdnsResults.clear();
    wait = g_mdnsStallMs ? g_mdnsStallMs : 3000;
    g_mdnsStallMs = 0;
  }
  sleepRealMs(wait);  // the core waits out the 3000 ms query; the watchdog is not fed
  bool online = wifiConnected();
  std::lock_guard<std::mutex> lock(g_world);
  if (!g_mdnsBegun || !online || srv != "_allternit-usba" || prt != "_tcp") return 0;
  for (const auto &e : g_mdns) {
    if (g_mdnsResults.size() == 20) break;
    g_mdnsResults.push_back(e);
  }
  return (int)g_mdnsResults.size();
}

IPAddress MDNSResponder::address(int idx) {
  std::lock_guard<std::mutex> lock(g_world);
  if (idx < 0 || (size_t)idx >= g_mdnsResults.size()) return IPAddress();
  return IPAddress(g_mdnsResults[(size_t)idx].ip.c_str());
}

uint16_t MDNSResponder::port(int idx) {
  std::lock_guard<std::mutex> lock(g_world);
  if (idx < 0 || (size_t)idx >= g_mdnsResults.size()) return 0;
  return g_mdnsResults[(size_t)idx].port;
}

String MDNSResponder::txt(int idx, const char *key) {
  std::lock_guard<std::mutex> lock(g_world);
  if (idx < 0 || (size_t)idx >= g_mdnsResults.size() || !key) return "";
  if (strcmp(key, "id") == 0) return String(g_mdnsResults[(size_t)idx].desk.c_str());
  return "";
}
