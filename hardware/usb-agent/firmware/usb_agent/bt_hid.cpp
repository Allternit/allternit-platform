#include "bt_hid.h"
#include <BLEDevice.h>
#include <BLEUtils.h>
#include <BLEServer.h>
#include <BLEHIDDevice.h>
#include <BLESecurity.h>

// Byte for byte the report map of HidReports.DESCRIPTOR (Android app) and
// HIDReportMap.bytes (Mac tool); written from the HID 1.11 spec and Usage Tables.
static const uint8_t REPORT_MAP[] = {
    0x05, 0x01, 0x09, 0x06, 0xA1, 0x01, 0x85, 0x01,                          // Keyboard, report 1
    0x05, 0x07, 0x19, 0xE0, 0x29, 0xE7, 0x15, 0x00, 0x25, 0x01,
    0x75, 0x01, 0x95, 0x08, 0x81, 0x02,                                      // modifier byte
    0x95, 0x01, 0x75, 0x08, 0x81, 0x01,                                      // reserved byte
    0x95, 0x05, 0x75, 0x01, 0x05, 0x08, 0x19, 0x01, 0x29, 0x05, 0x91, 0x02,  // LEDs out
    0x95, 0x01, 0x75, 0x03, 0x91, 0x01,                                      // LED padding
    0x95, 0x06, 0x75, 0x08, 0x15, 0x00, 0x25, 0x65,
    0x05, 0x07, 0x19, 0x00, 0x29, 0x65, 0x81, 0x00,                          // 6 key slots
    0xC0,
    0x05, 0x01, 0x09, 0x02, 0xA1, 0x01, 0x85, 0x02,                          // Mouse, report 2
    0x09, 0x01, 0xA1, 0x00,
    0x05, 0x09, 0x19, 0x01, 0x29, 0x03, 0x15, 0x00, 0x25, 0x01,
    0x95, 0x03, 0x75, 0x01, 0x81, 0x02,                                      // buttons
    0x95, 0x01, 0x75, 0x05, 0x81, 0x01,                                      // padding
    0x05, 0x01, 0x09, 0x30, 0x09, 0x31, 0x09, 0x38,
    0x15, 0x81, 0x25, 0x7F, 0x75, 0x08, 0x95, 0x03, 0x81, 0x06,              // X, Y, wheel
    0xC0, 0xC0,
};

#if !defined(CONFIG_NIMBLE_ENABLED)
#error "bt_hid.cpp needs the NimBLE stack (the arduino-esp32 3.x default on ESP32-S3)"
#endif

static const int MAX_BONDS = 4;
static BLEHIDDevice *hid = nullptr;
static BLECharacteristic *kbIn = nullptr, *mouseIn = nullptr;
static BLEServer *server = nullptr;
static volatile bool linked = false, encrypted = false;
static volatile uint16_t connId = 0;
static uint32_t pairingUntil = 0;
static bool wantAdvertising = false;
static ble_addr_t snapshot[MAX_BONDS];  // bonds that existed when the pairing window last closed
static int snapshotN = 0;
static bool started = false;

static int bondList(ble_addr_t *out, int max) {
  int n = 0;
  if (ble_store_util_bonded_peers(out, &n, max) != 0) return 0;
  return n;
}

static int bondCount() {
  int n = 0;
  ble_store_util_count(BLE_STORE_OBJ_TYPE_PEER_SEC, &n);
  return n;
}

static void takeSnapshot() { snapshotN = bondList(snapshot, MAX_BONDS); }

static bool sameAddr(const ble_addr_t &a, const ble_addr_t &b) { return a.type == b.type && !memcmp(a.val, b.val, 6); }

static bool inSnapshot(const ble_addr_t &addr) {
  for (int i = 0; i < snapshotN; i++) if (sameAddr(snapshot[i], addr)) return true;
  return false;
}

static bool pairingOpen() {
  return bondCount() == 0 || (pairingUntil && (int32_t)(pairingUntil - millis()) > 0);
}

class BtServerCallbacks : public BLEServerCallbacks {
  void onConnect(BLEServer *srv, ble_gap_conn_desc *desc) override {
    linked = true;
    encrypted = false;
    connId = desc->conn_handle;
    // With bonds and no pairing window, only the bonded iPhone may stay.
    if (!pairingOpen() && !inSnapshot(desc->peer_id_addr)) {
      srv->disconnect(desc->conn_handle);
      return;
    }
    BLESecurity::startSecurity(desc->conn_handle);  // ask the iPhone to pair or re-encrypt
  }
  void onDisconnect(BLEServer *, ble_gap_conn_desc *) override {
    linked = false;
    encrypted = false;
    wantAdvertising = true;  // restarted from btService, not in the BLE task
  }
};

class BtSecurityCallbacks : public BLESecurityCallbacks {
  void onAuthenticationComplete(ble_gap_conn_desc *desc, int status) override {
    if (status != 0 || !desc->sec_state.encrypted) {
      encrypted = false;
      return;
    }
    if (inSnapshot(desc->peer_id_addr)) {
      encrypted = true;  // the bonded iPhone came back
    } else if (pairingOpen()) {
      // A new iPhone paired in the pairing window: it replaces every older bond (one host).
      ble_addr_t list[MAX_BONDS];
      int n = bondList(list, MAX_BONDS);
      for (int i = 0; i < n; i++) if (!sameAddr(list[i], desc->peer_id_addr)) ble_store_util_delete_peer(&list[i]);
      pairingUntil = 0;
      takeSnapshot();
      encrypted = true;
    } else {
      // A stranger paired outside the pairing window: forget it and hang up.
      ble_store_util_delete_peer(&desc->peer_id_addr);
      encrypted = false;
      if (server) server->disconnect(desc->conn_handle);
    }
  }
  uint32_t onPassKeyRequest() override { return 0; }
  void onPassKeyNotify(uint32_t) override {}
  bool onConfirmPIN(uint32_t) override { return true; }  // Just Works: never asked
};

void btStart(const char *name) {
  if (started) return;
  started = true;
  BLEDevice::init(name);
  BLESecurity *sec = new BLESecurity();
  sec->setCapability(ESP_IO_CAP_NONE);          // Just Works, like a real keyboard
  sec->setAuthenticationMode(true, false, true);  // bond, no MITM, secure connections
  BLEDevice::setSecurityCallbacks(new BtSecurityCallbacks());
  server = BLEDevice::createServer();
  server->setCallbacks(new BtServerCallbacks());
  hid = new BLEHIDDevice(server);
  hid->manufacturer()->setValue("Allternit");
  hid->pnp(0x02, 0x303a, 0x1001, 0x0101);
  hid->hidInfo(0x00, 0x01);
  hid->reportMap((uint8_t *)REPORT_MAP, sizeof REPORT_MAP);
  kbIn = hid->inputReport(1);
  mouseIn = hid->inputReport(2);
  hid->outputReport(1);  // keyboard LEDs; the phone may write it, we ignore it
  hid->setBatteryLevel(100);
  hid->startServices();
  BLEAdvertising *adv = BLEDevice::getAdvertising();
  adv->setAppearance(HID_KEYBOARD);
  adv->addServiceUUID(hid->hidService()->getUUID());
  adv->setScanResponse(true);
  takeSnapshot();
  BLEDevice::startAdvertising();
}

BtState btState() {
  if (!started) return BT_OFF;
  if (linked && encrypted) return BT_CONNECTED;
  return bondCount() > 0 ? BT_BONDED : BT_PAIRING;
}

const char *btStateName() {
  switch (btState()) {
    case BT_PAIRING: return "pairing";
    case BT_BONDED: return "bonded";
    case BT_CONNECTED: return "connected";
    default: return "off";
  }
}

bool btConnected() { return btState() == BT_CONNECTED; }

void btOpenPairing(uint32_t ms) {
  pairingUntil = millis() + ms;
  if (!linked) wantAdvertising = true;
}

void btForget() {
  if (!started) return;
  ble_addr_t list[MAX_BONDS];
  int n = bondList(list, MAX_BONDS);
  for (int i = 0; i < n; i++) ble_store_util_delete_peer(&list[i]);
  snapshotN = 0;
  pairingUntil = 0;
  encrypted = false;
  if (linked && server) server->disconnect(connId);
}

void btService() {
  if (!started) return;
  if (pairingUntil && (int32_t)(millis() - pairingUntil) >= 0) {
    pairingUntil = 0;
    takeSnapshot();
  }
  if (wantAdvertising && !linked) {
    wantAdvertising = false;
    BLEDevice::startAdvertising();
  }
}

static void notify(BLECharacteristic *c, const uint8_t *data, size_t n) {
  if (!c || !btConnected()) return;
  c->setValue((uint8_t *)data, n);
  c->notify();
}

static int8_t clamp8(int v) { return (int8_t)constrain(v, -127, 127); }
static uint8_t mouseButtons = 0;

void btMouseMove(int dx, int dy, int wheel) {
  uint8_t r[4] = {mouseButtons, (uint8_t)clamp8(dx), (uint8_t)clamp8(dy), (uint8_t)clamp8(wheel)};
  notify(mouseIn, r, sizeof r);
}

void btMouseButtons(uint8_t buttons) {
  mouseButtons = buttons & 0x07;
  uint8_t r[4] = {mouseButtons, 0, 0, 0};
  notify(mouseIn, r, sizeof r);
}

void btKeys(uint8_t modifiers, const uint8_t *keys, size_t n) {
  uint8_t r[8] = {modifiers, 0, 0, 0, 0, 0, 0, 0};
  for (size_t i = 0; i < n && i < 6; i++) r[2 + i] = keys[i];
  notify(kbIn, r, sizeof r);
}

void btReleaseAll() {
  mouseButtons = 0;
  uint8_t m[4] = {0, 0, 0, 0};
  notify(mouseIn, m, sizeof m);
  uint8_t k[8] = {0};
  notify(kbIn, k, sizeof k);
}

bool btAsciiToHid(char c, uint8_t *mod, uint8_t *usage) {
  static const char *SHIFTED = ")!@#$%^&*(";
  *mod = 0;
  if (c >= 'a' && c <= 'z') { *usage = 0x04 + (c - 'a'); return true; }
  if (c >= 'A' && c <= 'Z') { *mod = 0x02; *usage = 0x04 + (c - 'A'); return true; }
  if (c >= '1' && c <= '9') { *usage = 0x1E + (c - '1'); return true; }
  if (c == '0') { *usage = 0x27; return true; }
  const char *s = strchr(SHIFTED, c);
  if (c && s) { *mod = 0x02; *usage = s - SHIFTED == 0 ? 0x27 : 0x1E + (s - SHIFTED - 1); return true; }
  struct Pair { char c; uint8_t usage; bool shift; };
  static const Pair P[] = {
      {' ', 0x2C, false}, {'\n', 0x28, false}, {'\t', 0x2B, false},
      {'-', 0x2D, false}, {'_', 0x2D, true}, {'=', 0x2E, false}, {'+', 0x2E, true},
      {'[', 0x2F, false}, {'{', 0x2F, true}, {']', 0x30, false}, {'}', 0x30, true},
      {'\\', 0x31, false}, {'|', 0x31, true}, {';', 0x33, false}, {':', 0x33, true},
      {'\'', 0x34, false}, {'"', 0x34, true}, {'`', 0x35, false}, {'~', 0x35, true},
      {',', 0x36, false}, {'<', 0x36, true}, {'.', 0x37, false}, {'>', 0x37, true},
      {'/', 0x38, false}, {'?', 0x38, true},
  };
  for (const Pair &p : P) if (p.c == c) { *usage = p.usage; *mod = p.shift ? 0x02 : 0; return true; }
  return false;
}
