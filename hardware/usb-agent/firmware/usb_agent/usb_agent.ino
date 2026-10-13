// Allternit USB Agent firmware (ESP32-S3).
//
// The board plugs into a phone's USB-C port and shows up as a standard USB
// mouse + keyboard (iOS AssistiveTouch turns the mouse into a pointer). It
// dials Allternit Desktop over Wi-Fi and runs bounded input commands. It holds
// no logic and no model: Desktop decides, the board only moves, clicks and
// types — and only while a person has armed it with its button.
//
// Protocol USBA/1 — the Desktop side and a line-for-line TypeScript twin of
// this file live in surfaces/allternit-desktop/src/main/phone-usb-agent-protocol.ts.
//
//   board → hello usba/1 <boardId> <fw> <resetReason> <bn>
//   desk  → challenge <sn> <HMAC(K, "srv|id|bn|sn")>
//   board → auth <HMAC(K, "brd|id|bn|sn")>              (only if Desktop proved K)
//   desk  → ready                                        board → s armed=<0|1> usb=<…>
//   desk  → c <seq> <command> <HMAC(SK, "c|seq|command")[:16 bytes]>
//   board → r <seq> ok | r <seq> err <reason>
//
// SK = HMAC(K, "ses|bn|sn"), fresh per connection. Sequence numbers must
// strictly increase within a session. A bad tag or malformed line drops the
// connection. K is written over USB serial at setup (see `prov` below) and
// never leaves the board.
//
// Safety, in firmware:
//   - Boots disarmed. Only the button arms it; Desktop can disarm but never arm.
//   - Disarmed, link lost, or no authenticated traffic for LEASE_MS: release
//     every key and button and refuse input.
//   - Disarms itself after ARM_IDLE_MS without input, and after
//     UNLINKED_DISARM_MS without an authenticated session.
//   - Every command is bounded (move ±4000, type ≤200 chars, key combos ≤4).
//   - A watchdog reboots a hung board (it comes back disarmed).
//
// Build: hardware/usb-agent/flash.sh (arduino-cli, esp32 core 3.x,
// FQBN esp32:esp32:esp32s3:USBMode=default,CDCOnBoot=cdc).
// Based on esp32-iphone-agent's hid_bridge.ino (MIT, Henry Yuan) — see ../../NOTICE.

#include <WiFi.h>
#include <ESPmDNS.h>
#include <Preferences.h>
#include <esp_mac.h>
#include <esp_random.h>
#include <esp_task_wdt.h>
#include <mbedtls/md.h>
#include "USB.h"
#include "USBHIDMouse.h"
#include "USBHIDKeyboard.h"
#include "tusb.h"

#define FIRMWARE_VERSION "1.0.0"

#ifndef ARM_BUTTON_PIN
#define ARM_BUTTON_PIN 0   // BOOT button on the XIAO ESP32S3 and most S3 boards
#endif
#ifndef ARM_LED_PIN
#define ARM_LED_PIN 21     // XIAO ESP32S3 user LED (active low); -1 for none
#endif
#ifndef ARM_LED_ACTIVE_LOW
#define ARM_LED_ACTIVE_LOW 1
#endif

const int STEP = 10;                 // max pointer delta per HID report while moving
const int STEP_MS = 8;               // constant pace keeps iOS pointer acceleration predictable
const int HOME_REPORTS = 50;
const long MOVE_LIMIT = 4000;
const size_t TYPE_LIMIT = 200;
const size_t MAX_LINE = 512;
const uint32_t WATCHDOG_MS = 8000;
const uint32_t LEASE_MS = 10000;               // no authenticated traffic for this long: let go of everything
const uint32_t HANDSHAKE_MS = 5000;
const uint32_t ARM_IDLE_MS = 30UL * 60 * 1000;  // armed but unused for 30 min: disarm
const uint32_t UNLINKED_DISARM_MS = 60000;     // armed with no session for a minute: disarm
const uint16_t DEFAULT_PORT = 7300;

USBHIDMouse mouse;
USBHIDKeyboard keyboard;
WiFiClient deskLink;
Preferences prefs;

// ── configuration (from `prov` over USB serial) ─────────────────────────────

struct Config {
  bool provisioned = false;
  String ssid, pass, host, desk;
  uint16_t port = DEFAULT_PORT;
  uint8_t key[32];
} cfg;

String boardId;

// ── state ───────────────────────────────────────────────────────────────────

bool armed = false;
bool authed = false;
uint8_t sessionKey[32];
uint32_t lastSeq = 0;
String boardNonce;
uint32_t helloAt = 0, lastAuthedAt = 0, lastInputAt = 0, lastSessionAt = 0;
String lastUsb = "";
bool lastArmedSent = false;

// ── helpers ─────────────────────────────────────────────────────────────────

static const char *HEXCH = "0123456789abcdef";

String toHex(const uint8_t *b, size_t n) {
  String s;
  s.reserve(n * 2);
  for (size_t i = 0; i < n; i++) {
    s += HEXCH[b[i] >> 4];
    s += HEXCH[b[i] & 15];
  }
  return s;
}

int hexVal(char c) {
  if (c >= '0' && c <= '9') return c - '0';
  if (c >= 'a' && c <= 'f') return c - 'a' + 10;
  return -1;
}

// Lowercase hex only; false on odd length or a bad digit.
bool fromHex(const String &s, uint8_t *out, size_t maxLen, size_t *outLen) {
  if (s.length() % 2 || s.length() / 2 > maxLen) return false;
  for (size_t i = 0; i < s.length() / 2; i++) {
    int hi = hexVal(s[2 * i]), lo = hexVal(s[2 * i + 1]);
    if (hi < 0 || lo < 0) return false;
    out[i] = (hi << 4) | lo;
  }
  *outLen = s.length() / 2;
  return true;
}

void hmac(const uint8_t *key, const String &msg, uint8_t out[32]) {
  mbedtls_md_context_t ctx;
  mbedtls_md_init(&ctx);
  mbedtls_md_setup(&ctx, mbedtls_md_info_from_type(MBEDTLS_MD_SHA256), 1);
  mbedtls_md_hmac_starts(&ctx, key, 32);
  mbedtls_md_hmac_update(&ctx, (const uint8_t *)msg.c_str(), msg.length());
  mbedtls_md_hmac_finish(&ctx, out);
  mbedtls_md_free(&ctx);
}

// Constant-time comparison of two lowercase hex strings.
bool equalHex(const String &a, const String &b) {
  if (a.length() != b.length()) return false;
  uint8_t diff = 0;
  for (size_t i = 0; i < a.length(); i++) diff |= a[i] ^ b[i];
  return diff == 0;
}

String randomHex16() {
  uint8_t b[16];
  esp_fill_random(b, sizeof b);
  return toHex(b, sizeof b);
}

void setLed(bool on) {
  if (ARM_LED_PIN < 0) return;
  digitalWrite(ARM_LED_PIN, (on ^ ARM_LED_ACTIVE_LOW) ? HIGH : LOW);
}

void releaseAll() {
  mouse.release(MOUSE_LEFT);
  keyboard.releaseAll();
}

void setArmed(bool on) {
  armed = on;
  if (!on) releaseAll();
  if (on) lastInputAt = millis();
  setLed(on);
}

const char *usbState() {
  if (!tud_mounted()) return "none";
  return tud_suspended() ? "asleep" : "mounted";
}

void sendState(bool force) {
  if (!authed || !deskLink.connected()) return;
  String usb = usbState();
  if (!force && usb == lastUsb && armed == lastArmedSent) return;
  lastUsb = usb;
  lastArmedSent = armed;
  deskLink.printf("s armed=%d usb=%s\n", armed ? 1 : 0, usb.c_str());
}

// A locked or sleeping phone suspends USB; sending HID reports then can stall.
// Ask it to wake up, and report "asleep" right away if it does not.
bool usbReady() {
  if (tud_suspended()) {
    tud_remote_wakeup();
    for (int i = 0; i < 50 && tud_suspended(); i++) delay(10);
  }
  return tud_mounted() && !tud_suspended();
}

// ── input commands ──────────────────────────────────────────────────────────

// The arm button is read inside long commands too, so pressing it stops a
// swipe or a long text part-way, not only between commands.
bool buttonDown() {
  return digitalRead(ARM_BUTTON_PIN) == LOW;
}

bool disarmedByHeldPress = false;

bool stoppedByButton() {
  if (!buttonDown()) return false;
  setArmed(false);
  disarmedByHeldPress = true;  // serviceButton() sees the same press next; don't re-arm on it
  return true;
}

bool movePaced(long dx, long dy) {
  while (dx || dy) {
    if (stoppedByButton()) return false;
    int sx = constrain(dx, -STEP, STEP), sy = constrain(dy, -STEP, STEP);
    mouse.move(sx, sy);
    dx -= sx;
    dy -= sy;
    delay(STEP_MS);
    esp_task_wdt_reset();
  }
  return true;
}

uint8_t keyCode(const String &k) {
  if (k == "enter") return KEY_RETURN;
  if (k == "esc") return KEY_ESC;
  if (k == "tab") return KEY_TAB;
  if (k == "backspace") return KEY_BACKSPACE;
  if (k == "space") return ' ';
  if (k == "up") return KEY_UP_ARROW;
  if (k == "down") return KEY_DOWN_ARROW;
  if (k == "left") return KEY_LEFT_ARROW;
  if (k == "right") return KEY_RIGHT_ARROW;
  if (k == "cmd") return KEY_LEFT_GUI;
  if (k == "ctrl") return KEY_LEFT_CTRL;
  if (k == "alt") return KEY_LEFT_ALT;
  if (k == "shift") return KEY_LEFT_SHIFT;
  if (k.length() == 1 && ((k[0] >= 'a' && k[0] <= 'z') || (k[0] >= '0' && k[0] <= '9'))) return k[0];
  return 0;
}

bool pressCombo(String combo) {
  uint8_t codes[4];
  int n = 0;
  combo += '+';
  for (int i; (i = combo.indexOf('+')) >= 0; combo = combo.substring(i + 1)) {
    if (n == 4) return false;
    uint8_t c = keyCode(combo.substring(0, i));
    if (!c) return false;
    codes[n++] = c;
  }
  if (n == 0) return false;
  for (int i = 0; i < n; i++) keyboard.press(codes[i]);
  delay(30);
  keyboard.releaseAll();
  return true;
}

bool parseLong(const String &s, long *out) {
  if (!s.length() || s.length() > 6) return false;
  char *end;
  *out = strtol(s.c_str(), &end, 10);
  return *end == '\0';
}

// Runs one authenticated command; returns "ok" or "err <reason>".
String run(const String &command) {
  int sp = command.indexOf(' ');
  String op = sp < 0 ? command : command.substring(0, sp);
  String arg = sp < 0 ? "" : command.substring(sp + 1);

  if (op == "ping") return "ok";
  if (op == "disarm") {
    setArmed(false);
    return "ok";
  }
  if (!armed) {
    releaseAll();
    return "err disarmed";
  }
  if (!usbReady()) return "err phone usb asleep";
  lastInputAt = millis();

  if (op == "home") {
    for (int i = 0; i < HOME_REPORTS; i++) {
      if (stoppedByButton()) return "err disarmed";
      mouse.move(-127, -127);
      delay(2);
    }
    return "ok";
  }
  if (op == "press") { mouse.press(MOUSE_LEFT); return "ok"; }
  if (op == "release") { mouse.release(MOUSE_LEFT); return "ok"; }
  if (op == "click") { mouse.press(MOUSE_LEFT); delay(60); mouse.release(MOUSE_LEFT); return "ok"; }
  if (op == "move" || op == "scroll") {
    int sp2 = arg.indexOf(' ');
    long a, b;
    if (sp2 < 0 || !parseLong(arg.substring(0, sp2), &a) || !parseLong(arg.substring(sp2 + 1), &b)) return "err bad numbers";
    if (op == "move") {
      if (labs(a) > MOVE_LIMIT || labs(b) > MOVE_LIMIT) return "err out of bounds";
      if (!movePaced(a, b)) return "err disarmed";
    } else {
      if (labs(a) > 50 || labs(b) > 50) return "err out of bounds";
      mouse.move(0, 0, a, b);
    }
    return "ok";
  }
  if (op == "type") {
    uint8_t buf[TYPE_LIMIT];
    size_t n;
    if (!fromHex(arg, buf, TYPE_LIMIT, &n) || n == 0) return "err bad text";
    for (size_t i = 0; i < n; i++) if (buf[i] < 0x20 || buf[i] > 0x7e) return "err bad text";
    for (size_t i = 0; i < n; i++) {
      if (stoppedByButton()) return "err disarmed";
      keyboard.write(buf[i]);
      esp_task_wdt_reset();
    }
    return "ok";
  }
  if (op == "key") return pressCombo(arg) ? "ok" : "err unknown key";
  return "err unknown command";
}

// ── the Wi-Fi link ──────────────────────────────────────────────────────────

String lineBuf;

void dropLink() {
  releaseAll();
  authed = false;
  lastSeq = 0;
  memset(sessionKey, 0, sizeof sessionKey);
  deskLink.stop();
}

void onLinkLine(const String &line) {
  if (!authed) {
    // challenge <sn:32 hex> <proof:64 hex>
    if (!line.startsWith("challenge ") || line.length() != 10 + 32 + 1 + 64) return dropLink();
    String sn = line.substring(10, 42);
    String proof = line.substring(43);
    uint8_t mac[32];
    hmac(cfg.key, "srv|" + boardId + "|" + boardNonce + "|" + sn, mac);
    if (!equalHex(proof, toHex(mac, 32))) return dropLink();  // not our Desktop
    hmac(cfg.key, "ses|" + boardNonce + "|" + sn, sessionKey);
    hmac(cfg.key, "brd|" + boardId + "|" + boardNonce + "|" + sn, mac);
    deskLink.print("auth " + toHex(mac, 32) + "\n");
    authed = true;
    lastSeq = 0;
    lastAuthedAt = lastSessionAt = millis();
    return;
  }
  if (line == "ready") {
    sendState(true);
    return;
  }
  // c <seq> <command…> <tag:32 hex>
  int first = line.indexOf(' ');
  int second = line.indexOf(' ', first + 1);
  int last = line.lastIndexOf(' ');
  if (!line.startsWith("c ") || second < 0 || last <= second || line.length() - last - 1 != 32) return dropLink();
  String seqText = line.substring(2, second);
  String command = line.substring(second + 1, last);
  String tag = line.substring(last + 1);
  if (seqText.length() == 0 || seqText.length() > 10) return dropLink();
  for (size_t i = 0; i < seqText.length(); i++) if (!isdigit(seqText[i])) return dropLink();
  uint32_t seq = strtoul(seqText.c_str(), nullptr, 10);
  uint8_t mac[32];
  hmac(sessionKey, "c|" + seqText + "|" + command, mac);
  if (!equalHex(tag, toHex(mac, 16))) return dropLink();
  lastAuthedAt = lastSessionAt = millis();
  if (seq <= lastSeq) {
    deskLink.printf("r %s err replay\n", seqText.c_str());
    return;
  }
  lastSeq = seq;
  String reply = run(command);
  deskLink.print("r " + seqText + " " + reply + "\n");
  sendState(false);
}

// Finds Desktop: the address from setup first (fast), then mDNS in case the
// computer's IP changed. The mDNS query blocks for a while, so it runs at most
// every 15 s and the button stays responsive in between.
bool connectDesktop() {
  if (deskLink.connect(cfg.host.c_str(), cfg.port, 500)) return true;
  static uint32_t lastQuery = 0;
  if (lastQuery && millis() - lastQuery < 15000) return false;
  lastQuery = millis();
  int n = MDNS.queryService("allternit-usba", "tcp");
  for (int i = 0; i < n; i++) {
    if (MDNS.txt(i, "id") != cfg.desk) continue;
    if (deskLink.connect(MDNS.address(i), MDNS.port(i), 500)) {
      cfg.host = MDNS.address(i).toString();  // remember for next time (not persisted)
      cfg.port = MDNS.port(i);
      return true;
    }
  }
  return false;
}

const char *resetReason() {
  switch (esp_reset_reason()) {
    case ESP_RST_POWERON: return "power-on";
    case ESP_RST_BROWNOUT: return "brownout";
    case ESP_RST_TASK_WDT: case ESP_RST_INT_WDT: case ESP_RST_WDT: return "watchdog";
    case ESP_RST_PANIC: return "crash";
    case ESP_RST_SW: return "software";
    case ESP_RST_USB: return "usb";
    default: return "other";
  }
}

void serviceLink() {
  if (WiFi.status() != WL_CONNECTED) {
    if (authed) dropLink();
    return;
  }
  if (!deskLink.connected()) {
    if (authed) dropLink();
    static uint32_t lastTry = 0;
    if (millis() - lastTry < 1000) return;
    lastTry = millis();
    if (!connectDesktop()) return;
    deskLink.setNoDelay(true);
    boardNonce = randomHex16();
    helloAt = millis();
    lineBuf = "";
    deskLink.printf("hello usba/1 %s %s %s %s\n", boardId.c_str(), FIRMWARE_VERSION, resetReason(), boardNonce.c_str());
    return;
  }
  if (!authed && millis() - helloAt > HANDSHAKE_MS) return dropLink();
  while (deskLink.available()) {
    char c = deskLink.read();
    if (c == '\r') continue;
    if (c != '\n') {
      if (lineBuf.length() >= MAX_LINE) return dropLink();
      lineBuf += c;
      continue;
    }
    String line = lineBuf;
    lineBuf = "";
    if (line.length()) onLinkLine(line);
    if (!deskLink.connected()) return;
  }
}

// ── provisioning over USB serial (needs the board in hand) ──────────────────

String serialBuf;

void loadConfig() {
  prefs.begin("usba", true);
  cfg.provisioned = prefs.getBool("ok", false);
  cfg.ssid = prefs.getString("ssid", "");
  cfg.pass = prefs.getString("pass", "");
  cfg.host = prefs.getString("host", "");
  cfg.desk = prefs.getString("desk", "");
  cfg.port = prefs.getUShort("port", DEFAULT_PORT);
  if (prefs.getBytes("key", cfg.key, 32) != 32) cfg.provisioned = false;
  prefs.end();
}

struct Pending {
  String ssid, pass, host, desk;
  long port = -1;
  uint8_t key[32];
  bool hasKey = false, hasSsid = false, hasPass = false;
} pending;

void provReply(const char *s) {
  Serial.print("prov ");
  Serial.print(s);
  Serial.print("\n");
}

void onSerialLine(String line) {
  line.trim();
  if (!line.startsWith("prov ")) return;
  line = line.substring(5);
  if (line == "info") {
    Serial.printf("prov info %s %s provisioned=%d\n", boardId.c_str(), FIRMWARE_VERSION, cfg.provisioned ? 1 : 0);
    return;
  }
  if (line == "wipe") {
    prefs.begin("usba", false);
    prefs.clear();
    prefs.end();
    provReply("ok");
    delay(100);
    ESP.restart();
  }
  if (line == "commit") {
    if (!pending.hasKey || !pending.hasSsid || !pending.hasPass || pending.host == "" || pending.port < 1024 || pending.desk == "") return provReply("err incomplete");
    prefs.begin("usba", false);
    prefs.putString("ssid", pending.ssid);
    prefs.putString("pass", pending.pass);
    prefs.putString("host", pending.host);
    prefs.putString("desk", pending.desk);
    prefs.putUShort("port", (uint16_t)pending.port);
    prefs.putBytes("key", pending.key, 32);
    prefs.putBool("ok", true);
    prefs.end();
    provReply("ok");
    Serial.flush();
    delay(200);
    ESP.restart();  // comes back disarmed with the new settings
  }
  if (!line.startsWith("set ")) return provReply("err unknown");
  line = line.substring(4);
  int sp = line.indexOf(' ');
  String k = sp < 0 ? line : line.substring(0, sp);
  String v = sp < 0 ? "" : line.substring(sp + 1);  // "set pass" alone = open network
  if (v.length() == 0 && k != "pass") return provReply("err missing value");
  uint8_t buf[64];
  size_t n;
  if (k == "ssid" || k == "pass") {
    if (!fromHex(v, buf, k == "ssid" ? 32 : 63, &n) || (k == "ssid" && n == 0)) return provReply("err bad value");
    String s;
    for (size_t i = 0; i < n; i++) s += (char)buf[i];
    if (k == "ssid") { pending.ssid = s; pending.hasSsid = true; } else { pending.pass = s; pending.hasPass = true; }
  } else if (k == "host") {
    IPAddress ip;
    if (!ip.fromString(v)) return provReply("err bad value");
    pending.host = v;
  } else if (k == "port") {
    long p;
    if (!parseLong(v, &p) || p < 1024 || p > 65535) return provReply("err bad value");
    pending.port = p;
  } else if (k == "desk") {
    if (v.length() < 4 || v.length() > 40) return provReply("err bad value");
    for (size_t i = 0; i < v.length(); i++) if (!(isalnum(v[i]) || v[i] == '-')) return provReply("err bad value");
    pending.desk = v;
  } else if (k == "key") {
    if (!fromHex(v, pending.key, 32, &n) || n != 32) return provReply("err bad value");
    pending.hasKey = true;
  } else {
    return provReply("err unknown setting");
  }
  provReply("ok");
}

void serviceSerial() {
  while (Serial.available()) {
    char c = Serial.read();
    if (c == '\r') continue;
    if (c != '\n') {
      if (serialBuf.length() < MAX_LINE) serialBuf += c;
      continue;
    }
    onSerialLine(serialBuf);
    serialBuf = "";
  }
}

// ── arm button ──────────────────────────────────────────────────────────────

void serviceButton() {
  static bool lastLevel = HIGH;
  static uint32_t changedAt = 0;
  static bool handled = false;
  bool level = digitalRead(ARM_BUTTON_PIN);
  if (level == HIGH) disarmedByHeldPress = false;  // that press is over, seen here or not
  if (level != lastLevel) {
    lastLevel = level;
    changedAt = millis();
    handled = false;
  }
  if (level == LOW && !handled && millis() - changedAt > 40) {  // pressed and debounced
    handled = true;
    if (disarmedByHeldPress) {
      disarmedByHeldPress = false;  // this press already disarmed mid-command
    } else {
      setArmed(!armed);
    }
    sendState(true);
  }
}

// ── main ────────────────────────────────────────────────────────────────────

void setup() {
  uint8_t mac[6];
  esp_read_mac(mac, ESP_MAC_WIFI_STA);
  boardId = "usba-" + toHex(mac, 6);

  pinMode(ARM_BUTTON_PIN, INPUT_PULLUP);
  if (ARM_LED_PIN >= 0) pinMode(ARM_LED_PIN, OUTPUT);
  setArmed(false);

  Serial.begin(115200);
  mouse.begin();
  keyboard.begin();
  USB.productName("Allternit USB Agent");
  USB.manufacturerName("Allternit");
  USB.begin();

  esp_task_wdt_config_t wdt = {.timeout_ms = WATCHDOG_MS, .idle_core_mask = 0, .trigger_panic = true};
  esp_task_wdt_reconfigure(&wdt);
  esp_task_wdt_add(NULL);

  loadConfig();
  if (!cfg.provisioned) return;  // waits for `prov` over USB
  WiFi.setHostname(boardId.c_str());
  WiFi.setSleep(false);  // modem sleep adds ~100 ms per command
  WiFi.setAutoReconnect(true);
  WiFi.begin(cfg.ssid.c_str(), cfg.pass.c_str());
  MDNS.begin(boardId.c_str());
}

void loop() {
  esp_task_wdt_reset();
  serviceSerial();
  serviceButton();
  if (!cfg.provisioned) {
    delay(10);
    return;
  }
  serviceLink();

  uint32_t now = millis();
  // Lease: no authenticated traffic for a while means nobody is driving; let go.
  if (authed && now - lastAuthedAt > LEASE_MS) dropLink();
  if (armed && !authed && now - lastSessionAt > UNLINKED_DISARM_MS) setArmed(false);
  if (armed && now - lastInputAt > ARM_IDLE_MS) setArmed(false);
  sendState(false);
  delay(1);
}
