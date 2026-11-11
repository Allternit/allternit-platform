// Allternit USB Agent firmware (ESP32-S3).
//
// The board plugs into a phone's USB-C port and shows up as a standard USB
// mouse + keyboard (iOS AssistiveTouch turns the mouse into a pointer). It
// dials Allternit Desktop over Wi-Fi and runs bounded input commands. It holds
// no logic and no model: Desktop decides, the board only moves, clicks and
// types — and only while a person has armed it with its button.
//
// Protocol USBA/2 (contract: hardware/link/README.md section 2) — the Desktop
// side and a line-for-line TypeScript twin of this file (ReferenceBoard) live
// in surfaces/allternit-desktop/src/main/phone-usb-agent-protocol.ts.
//
//   board → hello usba/2 <boardId> <fw> <resetReason> <bn>
//   desk  → challenge <sn> <HMAC(K, "srv2|id|bn|sn")>
//   board → auth <HMAC(K, "brd2|id|bn|sn")>             (only if Desktop proved K)
//   desk  → ready                                        board → s <fields> n=<count> <tag>
//   desk  → c <seq> <command> <HMAC(SK, "c|seq|command")[:16 bytes]>
//   board → r <seq> <ok | err reason> <HMAC(SK, "r|seq|result")[:16 bytes]>
//   board → s armed= usb= dock= hid= link= face= prog= n=<count> <HMAC(SK, "s|<fields> n=<count>")[:16 bytes]>
//
// SK = HMAC(K, "ses2|bn|sn"), fresh per connection. EK = HMAC(K, "enc2|bn|sn")
// opens `typex` text (AES-256-GCM), so the cloud relay cannot read what the
// bot types. Sequence numbers must strictly increase within a session; state
// lines count up from n=1. A bad tag or malformed line drops the connection.
// K is written over USB serial at setup (see `prov` below) and never leaves
// the board.
//
// Links: the LAN first (a TCP dial to Desktop, as in USBA/1), then the cloud
// relay (relay_link.h) when `relay` and `rtok` are set and no LAN session
// formed within 5 s of joining Wi-Fi.
//
// Face: a 200 x 200 1-bit picture (face.h) on an optional e-paper panel
// (display.h). `face`, `prog` and `label` move nothing, so they run while
// disarmed; they sit above the armed check in run() and never touch the
// idle timer.
//
// Safety, in firmware:
//   - Boots disarmed. Only the button arms it; Desktop can disarm but never arm.
//   - Disarmed, link lost, or no authenticated traffic for LEASE_MS: release
//     every key and button and refuse input.
//   - Disarms itself after ARM_IDLE_MS without input, and after
//     UNLINKED_DISARM_MS without an authenticated session.
//   - Every command is bounded (move ±4000, type ≤200 chars, key combos ≤4).
//   - A watchdog reboots a hung board (it comes back disarmed).
//   - The dock stop line (STOP_PIN, default none) only ever disarms.
//
// Build: hardware/usb-agent/flash.sh (arduino-cli, esp32 core 3.x,
// FQBN esp32:esp32:esp32s3:USBMode=default,CDCOnBoot=cdc).
// Based on esp32-iphone-agent's hid_bridge.ino (MIT, Henry Yuan) — see ../../NOTICE.
// face.h, display.h and relay_link.h are ours.

#include <WiFi.h>
#include <ESPmDNS.h>
#include <Preferences.h>
#include <esp_mac.h>
#include <esp_random.h>
#include <esp_task_wdt.h>
#include <driver/gpio.h>
#include <mbedtls/gcm.h>
#include <mbedtls/md.h>
#include "USB.h"
#include "USBHIDMouse.h"
#include "USBHIDKeyboard.h"
#include "tusb.h"
#include "face.h"
#include "display.h"
#include "relay_link.h"

#define FIRMWARE_VERSION "1.1.0"
#define USBA_HELLO "usba/2"

#ifndef ARM_BUTTON_PIN
#define ARM_BUTTON_PIN 0   // BOOT button on the XIAO ESP32S3 and most S3 boards
#endif
#ifndef ARM_LED_PIN
#define ARM_LED_PIN 21     // XIAO ESP32S3 user LED (active low); -1 for none
#endif
#ifndef ARM_LED_ACTIVE_LOW
#define ARM_LED_ACTIVE_LOW 1
#endif
// Dock contacts (contract section 7.1). Both default to none.
#ifndef DOCK_DETECT_PIN
#define DOCK_DETECT_PIN -1  // senses VDOCK; dock=1 after 100 ms active
#endif
#ifndef DOCK_DETECT_ACTIVE_LEVEL
#define DOCK_DETECT_ACTIVE_LEVEL HIGH
#endif
#ifndef STOP_PIN
#define STOP_PIN -1         // STOP_N, active low; 40 ms low disarms. Never arms.
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
const uint32_t RELAY_AFTER_WIFI_MS = 5000;     // LAN first: the relay only after 5 s on Wi-Fi with no LAN session
const uint32_t RELAY_MIN_GAP_MS = 5000;        // ticket requests at most every 5 s ...
const uint32_t RELAY_MAX_GAP_MS = 60000;       // ... doubling to 60 s after each failure
const uint32_t WIFI_NEXT_NETWORK_MS = 15000;   // try the next saved network after this long unconnected
const uint8_t FULL_REFRESH_EVERY = 20;         // partial refreshes before a full one
const int SAVED_NETWORKS = 4;                  // ssid/pass plus ssid1..3/pass1..3

USBHIDMouse mouse;
USBHIDKeyboard keyboard;
WiFiClient deskLink;
RelayLink relay;
Preferences prefs;

// ── configuration (from `prov` over USB serial) ─────────────────────────────

struct Config {
  bool provisioned = false;
  String ssid[SAVED_NETWORKS], pass[SAVED_NETWORKS];  // [0] is the setup network
  String host, desk;
  uint16_t port = DEFAULT_PORT;
  uint8_t key[32];
  String relay;     // relay host; empty = no relay
  String rtokHex;   // 64 lowercase hex
} cfg;

String boardId;

// ── state ───────────────────────────────────────────────────────────────────

enum LinkMode : uint8_t { LINK_NONE, LINK_LAN, LINK_CLOUD };

bool armed = false;
bool authed = false;
LinkMode linkMode = LINK_NONE;
uint8_t sessionKey[32];
uint8_t textKey[32];
uint32_t lastSeq = 0;
uint32_t stateCount = 0;  // n= of the last state line this session
String boardNonce;
uint32_t helloAt = 0, lastAuthedAt = 0, lastInputAt = 0, lastSessionAt = 0;
String lastStateFields = "";

// Face, progress and label (contract sections 2.6, 2.8, 3).
FaceName face = FACE_DEFAULT;
uint8_t prog = 0;
String label;  // RAM only; Phase 1 does nothing else with it
bool docked = false;

NullDisplay nullDisplay;
#ifdef LINK_DISPLAY_SSD1681
Ssd1681Display panel;
LinkDisplay *display = &panel;
#else
LinkDisplay *display = &nullDisplay;
#endif
bool displayOk = false;
bool faceDirty = true, faceForceFull = true;
uint8_t partialRefreshes = 0;
String shownFace = "";
uint8_t frame[FACE_BYTES];

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

void markFace() {
  faceDirty = true;
}

// Arming shows `default`; any disarm (button, `disarm`, a timer, the stop
// input) shows `stopped` with prog 0. The armed mark follows `armed`.
void setArmed(bool on) {
  armed = on;
  if (!on) releaseAll();
  if (on) lastInputAt = millis();
  setLed(on);
  if (on) {
    face = FACE_DEFAULT;
  } else {
    face = FACE_STOPPED;
    prog = 0;
  }
  markFace();
}

const char *usbState() {
  if (!tud_mounted()) return "none";
  return tud_suspended() ? "asleep" : "mounted";
}

// ── the link: LAN (TCP lines) or cloud (one WebSocket text frame per line) ──

bool linkUp() {
  if (linkMode == LINK_LAN) return deskLink.connected();
  if (linkMode == LINK_CLOUD) return relay.connected();
  return false;
}

void linkSend(const String &line) {
  if (linkMode == LINK_LAN) deskLink.print(line + "\n");
  else if (linkMode == LINK_CLOUD) relay.sendText(line);
}

String tagHex(const String &msg) {
  uint8_t mac[32];
  hmac(sessionKey, msg, mac);
  return toHex(mac, 16);
}

String stateFields() {
  String usb = usbState();
  String s = "armed=";
  s += armed ? "1" : "0";
  s += " usb=" + usb;
  s += " dock=";
  s += docked ? "1" : "0";
  s += " hid=";
  s += usb != "none" ? "usb" : "none";  // Phase 1 images have no Bluetooth
  s += " link=";
  s += linkMode == LINK_CLOUD ? "cloud" : "lan";
  s += " face=";
  s += faceName(face);
  s += " prog=" + String(prog);
  return s;
}

// Sends a signed state line when a field changed (or always, with force).
void sendState(bool force) {
  if (!authed || !linkUp()) return;
  String fields = stateFields();
  if (!force && fields == lastStateFields) return;
  lastStateFields = fields;
  stateCount++;
  String body = fields + " n=" + String(stateCount);
  linkSend("s " + body + " " + tagHex("s|" + body));
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

bool stopLineLow() {
  return STOP_PIN >= 0 && digitalRead(STOP_PIN) == LOW;
}

bool disarmedByHeldPress = false;

// A press must never be missed, even while loop() is blocked (a relay TLS
// connect, an mDNS query). An interrupt latches every press: a falling edge
// after the line was quiet for 40 ms (so bounce on press and on release is
// ignored). The latch only ever DISARMS. Arming stays with the debounced
// level check in serviceButton(), exactly as before, so a press still
// toggles: armed -> disarmed at once, disarmed -> armed after 40 ms held.
volatile bool pressLatched = false;
volatile uint32_t buttonEdgeAt = 0;

void IRAM_ATTR onButtonEdge() {
  uint32_t now = millis();
  bool quiet = now - buttonEdgeAt >= 40;
  buttonEdgeAt = now;
  if (quiet && gpio_get_level((gpio_num_t)ARM_BUTTON_PIN) == 0) pressLatched = true;
}

// Runs at the top of loop(), before any line is handled, and from inside the
// relay connect's waits. A latched press disarms an armed board; on a
// disarmed board it is dropped (the level check arms, as today).
void serviceButtonLatch() {
  if (!pressLatched) return;
  pressLatched = false;
  if (!armed) return;
  setArmed(false);
  if (buttonDown()) disarmedByHeldPress = true;  // the level check sees this same press; don't re-arm on it
  sendState(true);
}

bool stoppedByButton() {
  if (stopLineLow()) {  // the dock stop line: disarm only
    setArmed(false);
    return true;
  }
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

// An optional '-' then 1 to 6 digits (ReferenceBoard: /^-?\d{1,6}$/).
bool isIntText(const String &s) {
  size_t start = s.length() && s[0] == '-' ? 1 : 0;
  size_t digits = s.length() - start;
  if (digits < 1 || digits > 6) return false;
  for (size_t i = start; i < s.length(); i++) if (!isdigit(s[i])) return false;
  return true;
}

bool isInputOp(const String &op) {
  return op == "home" || op == "move" || op == "press" || op == "release" || op == "click" ||
         op == "scroll" || op == "type" || op == "typex" || op == "key";
}

String typeBytes(const uint8_t *buf, size_t n) {
  for (size_t i = 0; i < n; i++) if (buf[i] < 0x20 || buf[i] > 0x7e) return "err bad text";
  for (size_t i = 0; i < n; i++) {
    if (stoppedByButton()) return "err disarmed";
    keyboard.write(buf[i]);
    esp_task_wdt_reset();
  }
  return "ok";
}

// typex: AES-256-GCM under EK. IV = 4 zero bytes + seq (8 bytes, big-endian),
// AAD = "c|<seq>|typex", hex = ciphertext || 16-byte tag (contract section 2.7).
String typeSealed(uint32_t seq, const String &arg) {
  uint8_t sealed[TYPE_LIMIT + 16];
  size_t n;
  if (!fromHex(arg, sealed, sizeof sealed, &n) || n < 17) return "err bad text";
  size_t len = n - 16;
  uint8_t iv[12] = {0};
  for (int i = 0; i < 8; i++) iv[4 + i] = (uint8_t)(((uint64_t)seq) >> (56 - 8 * i));
  String aad = "c|" + String(seq) + "|typex";
  uint8_t plain[TYPE_LIMIT];
  mbedtls_gcm_context gcm;
  mbedtls_gcm_init(&gcm);
  int rc = mbedtls_gcm_setkey(&gcm, MBEDTLS_CIPHER_ID_AES, textKey, 256);
  if (rc == 0) {
    rc = mbedtls_gcm_auth_decrypt(&gcm, len, iv, sizeof iv, (const uint8_t *)aad.c_str(), aad.length(), sealed + len, 16, sealed, plain);
  }
  mbedtls_gcm_free(&gcm);
  if (rc != 0) return "err bad text";
  String result = typeBytes(plain, len);
  memset(plain, 0, sizeof plain);
  return result;
}

// Runs one authenticated command; returns "ok" or "err <reason>".
String run(uint32_t seq, const String &command) {
  int sp = command.indexOf(' ');
  String op = sp < 0 ? command : command.substring(0, sp);
  String arg = sp < 0 ? "" : command.substring(sp + 1);

  if (op == "ping") return "ok";
  if (op == "disarm") {
    setArmed(false);
    return "ok";
  }
  // Display commands move nothing: allowed while disarmed. They must not arm,
  // send a HID report, call usbReady() or touch lastInputAt. They answer as
  // soon as the request is stored; the panel refreshes later from loop().
  if (op == "face") {
    FaceName f;
    if (!faceFromName(arg.c_str(), &f)) return "err unknown face";
    face = f;
    markFace();
    return "ok";
  }
  if (op == "prog") {
    if (!isIntText(arg)) return "err bad numbers";
    long v = arg.toInt();
    if (v < 0 || v > 5 || arg.length() != 1) return "err out of bounds";
    prog = (uint8_t)v;
    markFace();
    return "ok";
  }
  if (op == "label") {
    if (arg == "-") {
      label = "";
      return "ok";
    }
    uint8_t buf[24];
    size_t n;
    if (!fromHex(arg, buf, sizeof buf, &n) || n == 0) return "err bad text";
    String text;
    for (size_t i = 0; i < n; i++) {
      if (buf[i] < 0x20 || buf[i] > 0x7e) return "err bad text";
      text += (char)buf[i];
    }
    label = text;
    return "ok";
  }
  if (!armed) {
    releaseAll();
    return "err disarmed";
  }
  if (!usbReady()) return "err phone usb asleep";
  // Only a real input command resets the idle timer, so nothing unknown keeps a board armed.
  if (!isInputOp(op)) return "err unknown command";
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
    return typeBytes(buf, n);
  }
  if (op == "typex") return typeSealed(seq, arg);
  if (op == "key") return pressCombo(arg) ? "ok" : "err unknown key";
  return "err unknown command";
}

// ── the Wi-Fi link ──────────────────────────────────────────────────────────

String lineBuf;
uint32_t wifiJoinedAt = 0;
uint32_t relayGapMs = RELAY_MIN_GAP_MS, nextRelayAt = 0;
bool relaySessionFormed = false;

void dropLink() {
  releaseAll();
  authed = false;
  lastSeq = 0;
  stateCount = 0;
  lastStateFields = "";
  memset(sessionKey, 0, sizeof sessionKey);
  memset(textKey, 0, sizeof textKey);
  if (linkMode == LINK_LAN) deskLink.stop();
  if (linkMode == LINK_CLOUD) relay.stop();
  linkMode = LINK_NONE;
}

void onLinkLine(const String &line) {
  if (!authed) {
    // challenge <sn:32 hex> <proof:64 hex>
    if (!line.startsWith("challenge ") || line.length() != 10 + 32 + 1 + 64) return dropLink();
    String sn = line.substring(10, 42);
    String proof = line.substring(43);
    uint8_t mac[32];
    hmac(cfg.key, "srv2|" + boardId + "|" + boardNonce + "|" + sn, mac);
    if (!equalHex(proof, toHex(mac, 32))) return dropLink();  // not our Desktop
    hmac(cfg.key, "ses2|" + boardNonce + "|" + sn, sessionKey);
    hmac(cfg.key, "enc2|" + boardNonce + "|" + sn, textKey);
    hmac(cfg.key, "brd2|" + boardId + "|" + boardNonce + "|" + sn, mac);
    linkSend("auth " + toHex(mac, 32));
    authed = true;
    lastSeq = 0;
    stateCount = 0;
    lastStateFields = "";
    lastAuthedAt = lastSessionAt = millis();
    if (linkMode == LINK_CLOUD) {
      relaySessionFormed = true;
      relayGapMs = RELAY_MIN_GAP_MS;
    }
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
  if (!equalHex(tag, tagHex("c|" + seqText + "|" + command))) return dropLink();
  lastAuthedAt = lastSessionAt = millis();
  if (seq <= lastSeq) {
    linkSend("r " + seqText + " err replay " + tagHex("r|" + seqText + "|err replay"));
    return;
  }
  lastSeq = seq;
  serviceButtonLatch();  // a press latched while this line was in flight wins
  String result = run(seq, command);
  if (!linkUp()) return;
  linkSend("r " + seqText + " " + result + " " + tagHex("r|" + seqText + "|" + result));
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

void sendHello() {
  boardNonce = randomHex16();
  helloAt = millis();
  lineBuf = "";
  char hello[96];
  snprintf(hello, sizeof hello, "hello " USBA_HELLO " %s %s %s %s", boardId.c_str(), FIRMWARE_VERSION, resetReason(), boardNonce.c_str());
  linkSend(hello);
}

bool relayConfigured() {
  return cfg.relay.length() > 0 && cfg.rtokHex.length() == 64;
}

void relayFailed() {
  nextRelayAt = millis() + relayGapMs;
  relayGapMs = relayGapMs * 2 > RELAY_MAX_GAP_MS ? RELAY_MAX_GAP_MS : relayGapMs * 2;
}

// Rotates through the saved networks while none is joined.
void serviceWifi() {
  static uint32_t triedAt = 0;
  static int index = 0;
  if (WiFi.status() == WL_CONNECTED) return;
  if (millis() - triedAt < WIFI_NEXT_NETWORK_MS) return;
  triedAt = millis();
  for (int step = 1; step <= SAVED_NETWORKS; step++) {
    int next = (index + step) % SAVED_NETWORKS;
    if (cfg.ssid[next].length() == 0) continue;
    index = next;
    WiFi.disconnect();
    WiFi.begin(cfg.ssid[index].c_str(), cfg.pass[index].c_str());
    return;
  }
}

void serviceLink() {
  if (WiFi.status() != WL_CONNECTED) {
    if (linkMode != LINK_NONE) dropLink();
    wifiJoinedAt = 0;
    serviceWifi();
    return;
  }
  if (!wifiJoinedAt) wifiJoinedAt = millis() | 1;

  if (linkMode == LINK_NONE || !linkUp()) {
    if (linkMode != LINK_NONE) {
      bool cloudWithoutSession = linkMode == LINK_CLOUD && !relaySessionFormed;
      dropLink();
      if (cloudWithoutSession) relayFailed();
    }
    // The LAN first, as in USBA/1.
    static uint32_t lastTry = 0;
    if (millis() - lastTry >= 1000) {
      lastTry = millis();
      if (connectDesktop()) {
        linkMode = LINK_LAN;
        deskLink.setNoDelay(true);
        sendHello();
        return;
      }
    }
    // The relay, when set up, after 5 s on Wi-Fi with no LAN session.
    if (relayConfigured() && millis() - wifiJoinedAt > RELAY_AFTER_WIFI_MS && (int32_t)(millis() - nextRelayAt) >= 0) {
      nextRelayAt = millis() + relayGapMs;
      relaySessionFormed = false;
      if (relay.open(cfg.relay, boardId, cfg.rtokHex)) {
        linkMode = LINK_CLOUD;
        helloAt = millis();  // the handshake limit covers the wait for the relay's ready frame too
      } else {
        relayFailed();
      }
    }
    return;
  }

  if (linkMode == LINK_CLOUD && !relay.ready()) {
    String ignored;
    int r = relay.poll(ignored);
    if (r < 0 || millis() - helloAt > HANDSHAKE_MS) {
      dropLink();
      relayFailed();
      return;
    }
    if (relay.ready()) sendHello();  // nothing is sent before the ready frame
    return;
  }
  if (!authed && millis() - helloAt > HANDSHAKE_MS) {
    bool cloud = linkMode == LINK_CLOUD;
    dropLink();
    if (cloud) relayFailed();
    return;
  }

  if (linkMode == LINK_CLOUD) {
    String line;
    int r;
    while ((r = relay.poll(line)) > 0) {
      onLinkLine(line);
      if (linkMode == LINK_NONE) return;
    }
    if (r < 0) dropLink();
    return;
  }
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
    if (linkMode == LINK_NONE || !deskLink.connected()) return;
  }
}

// ── provisioning over USB serial (needs the board in hand) ──────────────────

String serialBuf;

// NVS keys for saved network i: "ssid"/"pass" for 0, "ssid1".."ssid3" after.
String netKey(const char *base, int i) {
  return i == 0 ? String(base) : String(base) + String(i);
}

void loadConfig() {
  prefs.begin("usba", true);
  cfg.provisioned = prefs.getBool("ok", false);
  for (int i = 0; i < SAVED_NETWORKS; i++) {
    cfg.ssid[i] = prefs.getString(netKey("ssid", i).c_str(), "");
    cfg.pass[i] = prefs.getString(netKey("pass", i).c_str(), "");
  }
  cfg.host = prefs.getString("host", "");
  cfg.desk = prefs.getString("desk", "");
  cfg.port = prefs.getUShort("port", DEFAULT_PORT);
  cfg.relay = prefs.getString("relay", "");
  cfg.rtokHex = prefs.getString("rtok", "");
  if (cfg.relay.length() == 0 || cfg.rtokHex.length() != 64) cfg.relay = cfg.rtokHex = "";
  if (prefs.getBytes("key", cfg.key, 32) != 32) cfg.provisioned = false;
  prefs.end();
}

struct Pending {
  String ssid[SAVED_NETWORKS], pass[SAVED_NETWORKS];
  String host, desk, relay, rtok;
  long port = -1;
  uint8_t key[32];
  bool hasKey = false, hasSsid[SAVED_NETWORKS] = {false}, hasPass[SAVED_NETWORKS] = {false};
} pending;

// "ssid" -> 0, "ssid1".."ssid3" -> 1..3, else -1 (same for "pass").
int netIndex(const String &k, const char *base) {
  size_t n = strlen(base);
  if (!k.startsWith(base)) return -1;
  if (k.length() == n) return 0;
  if (k.length() == n + 1 && k[n] >= '1' && k[n] <= '3') return k[n] - '0';
  return -1;
}

bool validRelayHost(const String &v) {
  if (v.length() < 1 || v.length() > 63) return false;
  for (size_t i = 0; i < v.length(); i++) {
    char c = v[i];
    if (!((c >= 'a' && c <= 'z') || (c >= '0' && c <= '9') || c == '.' || c == '-')) return false;
  }
  return true;
}

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
    if (!pending.hasKey || !pending.hasSsid[0] || !pending.hasPass[0] || pending.host == "" || pending.port < 1024 || pending.desk == "") return provReply("err incomplete");
    prefs.begin("usba", false);
    for (int i = 0; i < SAVED_NETWORKS; i++) {
      if (pending.hasSsid[i]) {
        prefs.putString(netKey("ssid", i).c_str(), pending.ssid[i]);
        prefs.putString(netKey("pass", i).c_str(), pending.pass[i]);  // "" = open network
      } else {
        prefs.remove(netKey("ssid", i).c_str());
        prefs.remove(netKey("pass", i).c_str());
      }
    }
    // The relay is stored only when both its host and token were set.
    if (pending.relay.length() && pending.rtok.length() == 64) {
      prefs.putString("relay", pending.relay);
      prefs.putString("rtok", pending.rtok);
    } else {
      prefs.remove("relay");
      prefs.remove("rtok");
    }
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
  if (v.length() == 0 && netIndex(k, "pass") < 0) return provReply("err missing value");
  uint8_t buf[64];
  size_t n;
  int si = netIndex(k, "ssid"), pi = netIndex(k, "pass");
  if (si >= 0 || pi >= 0) {
    bool isSsid = si >= 0;
    if (!fromHex(v, buf, isSsid ? 32 : 63, &n) || (isSsid && n == 0)) return provReply("err bad value");
    String s;
    for (size_t i = 0; i < n; i++) s += (char)buf[i];
    if (isSsid) { pending.ssid[si] = s; pending.hasSsid[si] = true; } else { pending.pass[pi] = s; pending.hasPass[pi] = true; }
  } else if (k == "relay") {
    if (!validRelayHost(v)) return provReply("err bad value");
    pending.relay = v;
  } else if (k == "rtok") {
    uint8_t tok[32];
    if (!fromHex(v, tok, 32, &n) || n != 32) return provReply("err bad value");
    pending.rtok = v;
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

// ── dock contacts ───────────────────────────────────────────────────────────

// dock=1 once the detect input has been active for 100 ms.
void serviceDock() {
  if (DOCK_DETECT_PIN < 0) return;
  static uint32_t activeSince = 0;
  if (digitalRead(DOCK_DETECT_PIN) != DOCK_DETECT_ACTIVE_LEVEL) {
    activeSince = 0;
    docked = false;
    return;
  }
  if (!activeSince) activeSince = millis() | 1;
  if (millis() - activeSince >= 100) docked = true;
}

// STOP_N low for 40 ms disarms, exactly like a button press that disarms. It
// never arms, and it works with the bank's software hung.
void serviceStop() {
  if (STOP_PIN < 0) return;
  static uint32_t lowSince = 0;
  if (!stopLineLow()) {
    lowSince = 0;
    return;
  }
  if (!lowSince) lowSince = millis() | 1;
  if (millis() - lowSince >= 40 && armed) {
    setArmed(false);
    sendState(true);
  }
}

// ── face display ────────────────────────────────────────────────────────────

// Never waits on the panel: renders the newest state when the panel is idle.
// Full refresh at boot, for `stopped`, and after every 20 partial refreshes.
void serviceDisplay() {
  if (!faceDirty || !displayOk || display->busy()) return;
  String key = String(faceName(face)) + "|" + String(prog) + "|" + (armed ? "1" : "0");
  faceDirty = false;
  if (!faceForceFull && key == shownFace) return;
  shownFace = key;
  bool full = faceForceFull || face == FACE_STOPPED || partialRefreshes >= FULL_REFRESH_EVERY;
  partialRefreshes = full ? 0 : partialRefreshes + 1;
  faceForceFull = false;
  renderFace(frame, face, prog, armed);
  display->show(frame, full);
}

// ── arm button ──────────────────────────────────────────────────────────────

void serviceButton() {
  serviceButtonLatch();
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
  attachInterrupt(digitalPinToInterrupt(ARM_BUTTON_PIN), onButtonEdge, CHANGE);
  relayWaitHook = serviceButtonLatch;
  if (ARM_LED_PIN >= 0) pinMode(ARM_LED_PIN, OUTPUT);
  if (DOCK_DETECT_PIN >= 0) pinMode(DOCK_DETECT_PIN, INPUT);
  if (STOP_PIN >= 0) pinMode(STOP_PIN, INPUT_PULLUP);
  // Boot: disarmed, face `default`, prog 0, full refresh.
  armed = false;
  setLed(false);
  face = FACE_DEFAULT;
  prog = 0;
  faceDirty = faceForceFull = true;
  displayOk = display->begin();

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
  WiFi.begin(cfg.ssid[0].c_str(), cfg.pass[0].c_str());
  MDNS.begin(boardId.c_str());
}

void loop() {
  esp_task_wdt_reset();
  serviceSerial();
  serviceButton();
  serviceStop();
  serviceDock();
  serviceDisplay();
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
