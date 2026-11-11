#!/usr/bin/env bash
# Native board: every shimmed signature the firmware relies on must appear,
# word for word, both in the shim header and in the installed arduino-esp32
# core 3.3.12 (plan step 2, decision 2). A shim whose parameter types drift
# from the core (int instead of int8_t, say) fails here, and so does a core
# update that changes one.
#
# Comparison: comments are dropped and whitespace is folded on both sides,
# then the declaration must be a substring of each header. A line of the
# form `core text => shim text` is a deliberate host mapping (millis()).
#
# Runs only where the core is installed; elsewhere it says so and exits 0
# without claiming success. Override the locations with ARDUINO_ESP32_CORE
# (…/hardware/esp32/3.3.12) and ARDUINO_ESP32_LIBS (…/esp32s3-libs/3.3.12/include).
set -euo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
SHIM="$HERE/shim"
VERSION=3.3.12

find_dir() {
  for d in "$@"; do
    [ -d "$d" ] && { echo "$d"; return 0; }
  done
  return 1
}
CORE=${ARDUINO_ESP32_CORE:-$(find_dir "$HOME/Library/Arduino15/packages/esp32/hardware/esp32/$VERSION" "$HOME/.arduino15/packages/esp32/hardware/esp32/$VERSION" || true)}
LIBS=${ARDUINO_ESP32_LIBS:-$(find_dir "$HOME/Library/Arduino15/packages/esp32/tools/esp32s3-libs/$VERSION/include" "$HOME/.arduino15/packages/esp32/tools/esp32s3-libs/$VERSION/include" || true)}
if [ -z "$CORE" ] || [ -z "$LIBS" ] || [ ! -d "$CORE" ] || [ ! -d "$LIBS" ]; then
  echo "signatures skipped: arduino-esp32 core $VERSION is not installed here"
  exit 0
fi

normalize() {
  perl -0777 -pe 's{/\*.*?\*/}{ }gs; s{//[^\n]*}{ }g; s/\s+/ /g; s/ ?([(),*&;=\[\]{}:]) ?/$1/g; s/^ //; s/ $//' "$@"
}

# root | core header | shim header | declaration
TABLE=$(cat <<'EOF'
core|cores/esp32/esp32-hal.h|Arduino.h|unsigned long millis(); => uint32_t millis();
core|cores/esp32/esp32-hal.h|Arduino.h|void delay(uint32_t);
core|cores/esp32/esp32-hal.h|Arduino.h|void delayMicroseconds(uint32_t us);
core|cores/esp32/esp32-hal-gpio.h|Arduino.h|void pinMode(uint8_t pin, uint8_t mode);
core|cores/esp32/esp32-hal-gpio.h|Arduino.h|void digitalWrite(uint8_t pin, uint8_t val);
core|cores/esp32/esp32-hal-gpio.h|Arduino.h|int digitalRead(uint8_t pin);
core|cores/esp32/esp32-hal-gpio.h|Arduino.h|void attachInterrupt(uint8_t pin, void (*)(void), int mode);
core|cores/esp32/esp32-hal-gpio.h|Arduino.h|void detachInterrupt(uint8_t pin);
core|cores/esp32/esp32-hal-gpio.h|Arduino.h|#define LOW 0x0
core|cores/esp32/esp32-hal-gpio.h|Arduino.h|#define HIGH 0x1
core|cores/esp32/esp32-hal-gpio.h|Arduino.h|#define INPUT 0x01
core|cores/esp32/esp32-hal-gpio.h|Arduino.h|#define OUTPUT 0x03
core|cores/esp32/esp32-hal-gpio.h|Arduino.h|#define INPUT_PULLUP 0x05
core|cores/esp32/esp32-hal-gpio.h|Arduino.h|#define CHANGE 0x03
core|cores/esp32/esp32-hal-gpio.h|Arduino.h|#define FALLING 0x02
core|cores/esp32/Arduino.h|Arduino.h|#define constrain(amt, low, high) ((amt) < (low) ? (low) : ((amt) > (high) ? (high) : (amt)))
core|cores/esp32/Arduino.h|Arduino.h|#define MSBFIRST 1
core|cores/esp32/Esp.h|Arduino.h|void restart();
core|cores/esp32/USBCDC.h|Arduino.h|void begin(unsigned long baud = 0);
core|cores/esp32/USBCDC.h|Arduino.h|int available(void)
core|cores/esp32/USBCDC.h|Arduino.h|int read(void)
core|cores/esp32/USBCDC.h|Arduino.h|void flush(void);
core|cores/esp32/HardwareSerial.h|Arduino.h|#define Serial USBSerial
core|cores/esp32/Print.h|Print.h|size_t printf(const char *format, ...) __attribute__((format(printf, 2, 3)));
core|cores/esp32/Print.h|Print.h|size_t print(const String &);
core|cores/esp32/Print.h|Print.h|size_t print(const char[]);
core|cores/esp32/Stream.h|Print.h|void setTimeout(unsigned long timeout);
core|cores/esp32/WString.h|WString.h|String(const char *cstr = "");
core|cores/esp32/WString.h|WString.h|explicit String(char c);
core|cores/esp32/WString.h|WString.h|explicit String(unsigned char, unsigned char base = 10);
core|cores/esp32/WString.h|WString.h|explicit String(int, unsigned char base = 10);
core|cores/esp32/WString.h|WString.h|explicit String(unsigned int, unsigned char base = 10);
core|cores/esp32/WString.h|WString.h|explicit String(long, unsigned char base = 10);
core|cores/esp32/WString.h|WString.h|explicit String(unsigned long, unsigned char base = 10);
core|cores/esp32/WString.h|WString.h|bool reserve(unsigned int size);
core|cores/esp32/WString.h|WString.h|inline unsigned int length(void) const {
core|cores/esp32/WString.h|WString.h|inline void clear(void) {
core|cores/esp32/WString.h|WString.h|bool startsWith(const String &prefix) const;
core|cores/esp32/WString.h|WString.h|bool startsWith(const String &prefix, unsigned int offset) const;
core|cores/esp32/WString.h|WString.h|bool endsWith(const String &suffix) const;
core|cores/esp32/WString.h|WString.h|char operator[](unsigned int index) const;
core|cores/esp32/WString.h|WString.h|char &operator[](unsigned int index);
core|cores/esp32/WString.h|WString.h|int indexOf(char ch) const;
core|cores/esp32/WString.h|WString.h|int indexOf(char ch, unsigned int fromIndex) const;
core|cores/esp32/WString.h|WString.h|int indexOf(const String &str) const;
core|cores/esp32/WString.h|WString.h|int indexOf(const String &str, unsigned int fromIndex) const;
core|cores/esp32/WString.h|WString.h|int lastIndexOf(char ch) const;
core|cores/esp32/WString.h|WString.h|String substring(unsigned int beginIndex) const {
core|cores/esp32/WString.h|WString.h|String substring(unsigned int beginIndex, unsigned int endIndex) const;
core|cores/esp32/WString.h|WString.h|void replace(const String &find, const String &replace);
core|cores/esp32/WString.h|WString.h|void replace(const char *find, const char *replace) {
core|cores/esp32/WString.h|WString.h|void remove(unsigned int index, unsigned int count);
core|cores/esp32/WString.h|WString.h|void toLowerCase(void);
core|cores/esp32/WString.h|WString.h|void trim(void);
core|cores/esp32/WString.h|WString.h|long toInt(void) const;
core|cores/esp32/IPAddress.h|IPAddress.h|IPAddress(const char *address);
core|cores/esp32/IPAddress.h|IPAddress.h|bool fromString(const char *address);
core|cores/esp32/IPAddress.h|IPAddress.h|bool fromString(const String &address) {
core|cores/esp32/IPAddress.h|IPAddress.h|String toString(bool includeZone = false) const;
core|libraries/WiFi/src/WiFiType.h|WiFi.h|WL_CONNECTED = 3,
core|libraries/WiFi/src/WiFiSTA.h|WiFi.h|wl_status_t begin(const char *ssid, const char *passphrase = NULL, int32_t channel = 0, const uint8_t *bssid = NULL, bool tryConnect = true);
core|libraries/WiFi/src/WiFiSTA.h|WiFi.h|wl_status_t status();
core|libraries/WiFi/src/WiFiSTA.h|WiFi.h|bool disconnect(bool wifioff = false, bool eraseap = false, unsigned long timeoutLength = 100);
core|libraries/WiFi/src/WiFiSTA.h|WiFi.h|bool setAutoReconnect(bool autoReconnect);
core|libraries/WiFi/src/WiFiGeneric.h|WiFi.h|static bool setHostname(const char *hostname);
core|libraries/WiFi/src/WiFiGeneric.h|WiFi.h|bool setSleep(bool enabled);
core|libraries/WiFi/src/WiFiClient.h|WiFi.h|typedef NetworkClient WiFiClient;
core|libraries/Network/src/NetworkClient.h|WiFi.h|int connect(IPAddress ip, uint16_t port, int32_t timeout_ms);
core|libraries/Network/src/NetworkClient.h|WiFi.h|int connect(const char *host, uint16_t port, int32_t timeout_ms);
core|libraries/Network/src/NetworkClient.h|WiFi.h|size_t write(const uint8_t *buf, size_t size)
core|libraries/Network/src/NetworkClient.h|WiFi.h|int available()
core|libraries/Network/src/NetworkClient.h|WiFi.h|int read()
core|libraries/Network/src/NetworkClient.h|WiFi.h|void stop();
core|libraries/Network/src/NetworkClient.h|WiFi.h|uint8_t connected();
core|libraries/Network/src/NetworkClient.h|WiFi.h|int setNoDelay(bool nodelay);
core|libraries/NetworkClientSecure/src/NetworkClientSecure.h|NetworkClientSecure.h|int connect(const char *host, uint16_t port, int32_t timeout)
core|libraries/NetworkClientSecure/src/NetworkClientSecure.h|NetworkClientSecure.h|void setHandshakeTimeout(unsigned long handshake_timeout)
core|libraries/NetworkClientSecure/src/NetworkClientSecure.h|NetworkClientSecure.h|void useBuiltinCACertBundle()
core|libraries/ESPmDNS/src/ESPmDNS.h|ESPmDNS.h|bool begin(const String &hostName);
core|libraries/ESPmDNS/src/ESPmDNS.h|ESPmDNS.h|bool begin(const char *hostName) {
core|libraries/ESPmDNS/src/ESPmDNS.h|ESPmDNS.h|int queryService(char *service, char *proto);
core|libraries/ESPmDNS/src/ESPmDNS.h|ESPmDNS.h|int queryService(const char *service, const char *proto) {
core|libraries/ESPmDNS/src/ESPmDNS.h|ESPmDNS.h|IPAddress address(int idx);
core|libraries/ESPmDNS/src/ESPmDNS.h|ESPmDNS.h|uint16_t port(int idx);
core|libraries/ESPmDNS/src/ESPmDNS.h|ESPmDNS.h|String txt(int idx, const char *key);
core|libraries/ESPmDNS/src/ESPmDNS.h|ESPmDNS.h|extern MDNSResponder MDNS;
core|libraries/Preferences/src/Preferences.h|Preferences.h|bool begin(const char *name, bool readOnly = false, const char *partition_label = NULL);
core|libraries/Preferences/src/Preferences.h|Preferences.h|void end();
core|libraries/Preferences/src/Preferences.h|Preferences.h|bool clear();
core|libraries/Preferences/src/Preferences.h|Preferences.h|bool remove(const char *key);
core|libraries/Preferences/src/Preferences.h|Preferences.h|size_t putUShort(const char *key, uint16_t value);
core|libraries/Preferences/src/Preferences.h|Preferences.h|size_t putBool(const char *key, bool value);
core|libraries/Preferences/src/Preferences.h|Preferences.h|size_t putString(const char *key, const char *value);
core|libraries/Preferences/src/Preferences.h|Preferences.h|size_t putString(const char *key, String value);
core|libraries/Preferences/src/Preferences.h|Preferences.h|size_t putBytes(const char *key, const void *value, size_t len);
core|libraries/Preferences/src/Preferences.h|Preferences.h|uint16_t getUShort(const char *key, uint16_t defaultValue = 0);
core|libraries/Preferences/src/Preferences.h|Preferences.h|bool getBool(const char *key, bool defaultValue = false);
core|libraries/Preferences/src/Preferences.h|Preferences.h|String getString(const char *key, String defaultValue = String());
core|libraries/Preferences/src/Preferences.h|Preferences.h|size_t getBytes(const char *key, void *buf, size_t maxLen);
core|cores/esp32/USB.h|USB.h|bool productName(const char *name);
core|cores/esp32/USB.h|USB.h|bool manufacturerName(const char *name);
core|cores/esp32/USB.h|USB.h|bool begin();
core|cores/esp32/USB.h|USB.h|extern ESPUSB USB;
core|libraries/USB/src/USBHIDMouse.h|USBHIDMouse.h|#define MOUSE_LEFT 0x01
core|libraries/USB/src/USBHIDMouse.h|USBHIDMouse.h|void begin(void);
core|libraries/USB/src/USBHIDMouse.h|USBHIDMouse.h|void press(uint8_t b = MOUSE_LEFT);
core|libraries/USB/src/USBHIDMouse.h|USBHIDMouse.h|void release(uint8_t b = MOUSE_LEFT);
core|libraries/USB/src/USBHIDMouse.h|USBHIDMouse.h|void move(int8_t x, int8_t y, int8_t wheel = 0, int8_t pan = 0);
core|libraries/USB/src/USBHIDMouse.h|USBHIDMouse.h|typedef USBHIDRelativeMouse USBHIDMouse;
core|libraries/USB/src/USBHIDKeyboard.h|USBHIDKeyboard.h|void begin(const uint8_t *layout = KeyboardLayout_en_US);
core|libraries/USB/src/USBHIDKeyboard.h|USBHIDKeyboard.h|size_t write(uint8_t k);
core|libraries/USB/src/USBHIDKeyboard.h|USBHIDKeyboard.h|size_t press(uint8_t k);
core|libraries/USB/src/USBHIDKeyboard.h|USBHIDKeyboard.h|size_t release(uint8_t k);
core|libraries/USB/src/USBHIDKeyboard.h|USBHIDKeyboard.h|void releaseAll(void);
core|libraries/USB/src/USBHIDKeyboard.h|USBHIDKeyboard.h|#define KEY_LEFT_CTRL 0x80
core|libraries/USB/src/USBHIDKeyboard.h|USBHIDKeyboard.h|#define KEY_LEFT_SHIFT 0x81
core|libraries/USB/src/USBHIDKeyboard.h|USBHIDKeyboard.h|#define KEY_LEFT_ALT 0x82
core|libraries/USB/src/USBHIDKeyboard.h|USBHIDKeyboard.h|#define KEY_LEFT_GUI 0x83
core|libraries/USB/src/USBHIDKeyboard.h|USBHIDKeyboard.h|#define KEY_UP_ARROW 0xDA
core|libraries/USB/src/USBHIDKeyboard.h|USBHIDKeyboard.h|#define KEY_DOWN_ARROW 0xD9
core|libraries/USB/src/USBHIDKeyboard.h|USBHIDKeyboard.h|#define KEY_LEFT_ARROW 0xD8
core|libraries/USB/src/USBHIDKeyboard.h|USBHIDKeyboard.h|#define KEY_RIGHT_ARROW 0xD7
core|libraries/USB/src/USBHIDKeyboard.h|USBHIDKeyboard.h|#define KEY_BACKSPACE 0xB2
core|libraries/USB/src/USBHIDKeyboard.h|USBHIDKeyboard.h|#define KEY_TAB 0xB3
core|libraries/USB/src/USBHIDKeyboard.h|USBHIDKeyboard.h|#define KEY_RETURN 0xB0
core|libraries/USB/src/USBHIDKeyboard.h|USBHIDKeyboard.h|#define KEY_ESC 0xB1
core|libraries/SPI/src/SPI.h|SPI.h|SPISettings(uint32_t clock, uint8_t bitOrder, uint8_t dataMode)
core|libraries/SPI/src/SPI.h|SPI.h|bool begin(int8_t sck = -1, int8_t miso = -1, int8_t mosi = -1, int8_t ss = -1)
core|libraries/SPI/src/SPI.h|SPI.h|void beginTransaction(SPISettings settings)
core|libraries/SPI/src/SPI.h|SPI.h|void endTransaction(void)
core|libraries/SPI/src/SPI.h|SPI.h|uint8_t transfer(uint8_t data)
core|cores/esp32/esp32-hal-spi.h|SPI.h|#define SPI_MODE0 0
libs|arduino_tinyusb/tinyusb/src/device/usbd.h|tusb.h|bool tud_mounted(void);
libs|arduino_tinyusb/tinyusb/src/device/usbd.h|tusb.h|bool tud_suspended(void);
libs|arduino_tinyusb/tinyusb/src/device/usbd.h|tusb.h|bool tud_remote_wakeup(void);
libs|esp_system/include/esp_task_wdt.h|esp_task_wdt.h|uint32_t timeout_ms;
libs|esp_system/include/esp_task_wdt.h|esp_task_wdt.h|uint32_t idle_core_mask;
libs|esp_system/include/esp_task_wdt.h|esp_task_wdt.h|bool trigger_panic;
libs|esp_system/include/esp_task_wdt.h|esp_task_wdt.h|esp_err_t esp_task_wdt_reconfigure(const esp_task_wdt_config_t *config);
libs|esp_system/include/esp_task_wdt.h|esp_task_wdt.h|esp_err_t esp_task_wdt_add(TaskHandle_t task_handle);
libs|esp_system/include/esp_task_wdt.h|esp_task_wdt.h|esp_err_t esp_task_wdt_reset(void);
libs|esp_system/include/esp_system.h|esp_system.h|esp_reset_reason_t esp_reset_reason(void);
libs|esp_system/include/esp_system.h|esp_system.h|ESP_RST_UNKNOWN, ESP_RST_POWERON, ESP_RST_EXT, ESP_RST_SW, ESP_RST_PANIC, ESP_RST_INT_WDT, ESP_RST_TASK_WDT, ESP_RST_WDT, ESP_RST_DEEPSLEEP, ESP_RST_BROWNOUT, ESP_RST_SDIO, ESP_RST_USB,
libs|esp_hw_support/include/esp_mac.h|esp_mac.h|esp_err_t esp_read_mac(uint8_t *mac, esp_mac_type_t type);
libs|esp_hw_support/include/esp_mac.h|esp_mac.h|ESP_MAC_WIFI_STA,
libs|esp_hw_support/include/esp_random.h|esp_random.h|void esp_fill_random(void *buf, size_t len);
libs|esp_driver_gpio/include/driver/gpio.h|driver/gpio.h|int gpio_get_level(gpio_num_t gpio_num);
libs|mbedtls/mbedtls/include/mbedtls/md.h|mbedtls/md.h|MBEDTLS_MD_SHA256=0x09,
libs|mbedtls/mbedtls/include/mbedtls/md.h|mbedtls/md.h|#define MBEDTLS_ERR_MD_BAD_INPUT_DATA -0x5100
libs|mbedtls/mbedtls/include/mbedtls/md.h|mbedtls/md.h|typedef struct mbedtls_md_info_t mbedtls_md_info_t;
libs|mbedtls/mbedtls/include/mbedtls/md.h|mbedtls/md.h|const mbedtls_md_info_t *mbedtls_md_info_from_type(mbedtls_md_type_t md_type);
libs|mbedtls/mbedtls/include/mbedtls/md.h|mbedtls/md.h|void mbedtls_md_init(mbedtls_md_context_t *ctx);
libs|mbedtls/mbedtls/include/mbedtls/md.h|mbedtls/md.h|void mbedtls_md_free(mbedtls_md_context_t *ctx);
libs|mbedtls/mbedtls/include/mbedtls/md.h|mbedtls/md.h|int mbedtls_md_setup(mbedtls_md_context_t *ctx, const mbedtls_md_info_t *md_info, int hmac);
libs|mbedtls/mbedtls/include/mbedtls/md.h|mbedtls/md.h|int mbedtls_md_hmac_starts(mbedtls_md_context_t *ctx, const unsigned char *key, size_t keylen);
libs|mbedtls/mbedtls/include/mbedtls/md.h|mbedtls/md.h|int mbedtls_md_hmac_update(mbedtls_md_context_t *ctx, const unsigned char *input, size_t ilen);
libs|mbedtls/mbedtls/include/mbedtls/md.h|mbedtls/md.h|int mbedtls_md_hmac_finish(mbedtls_md_context_t *ctx, unsigned char *output);
libs|mbedtls/mbedtls/include/mbedtls/cipher.h|mbedtls/gcm.h|MBEDTLS_CIPHER_ID_NONE = 0, MBEDTLS_CIPHER_ID_NULL, MBEDTLS_CIPHER_ID_AES,
libs|mbedtls/mbedtls/include/mbedtls/gcm.h|mbedtls/gcm.h|#define MBEDTLS_ERR_GCM_AUTH_FAILED -0x0012
libs|mbedtls/mbedtls/include/mbedtls/gcm.h|mbedtls/gcm.h|void mbedtls_gcm_init(mbedtls_gcm_context *ctx);
libs|mbedtls/mbedtls/include/mbedtls/gcm.h|mbedtls/gcm.h|int mbedtls_gcm_setkey(mbedtls_gcm_context *ctx, mbedtls_cipher_id_t cipher, const unsigned char *key, unsigned int keybits);
libs|mbedtls/mbedtls/include/mbedtls/gcm.h|mbedtls/gcm.h|int mbedtls_gcm_auth_decrypt(mbedtls_gcm_context *ctx, size_t length, const unsigned char *iv, size_t iv_len, const unsigned char *add, size_t add_len, const unsigned char *tag, size_t tag_len, const unsigned char *input, unsigned char *output);
libs|mbedtls/mbedtls/include/mbedtls/gcm.h|mbedtls/gcm.h|void mbedtls_gcm_free(mbedtls_gcm_context *ctx);
libs|mbedtls/mbedtls/include/mbedtls/base64.h|mbedtls/base64.h|#define MBEDTLS_ERR_BASE64_BUFFER_TOO_SMALL -0x002A
libs|mbedtls/mbedtls/include/mbedtls/base64.h|mbedtls/base64.h|int mbedtls_base64_encode(unsigned char *dst, size_t dlen, size_t *olen, const unsigned char *src, size_t slen);
libs|mbedtls/mbedtls/include/mbedtls/sha1.h|mbedtls/sha1.h|int mbedtls_sha1(const unsigned char *input, size_t ilen, unsigned char output[20]);
EOF
)

checked=0
failed=0
declare_normalized() { printf '%s' "$1" | normalize; }
cache_dir=$(mktemp -d "${TMPDIR:-/tmp}/link-native-sig.XXXXXX")
trap 'rm -rf "$cache_dir"' EXIT
normalized_file() {
  local f=$1 key
  key=$(printf '%s' "$f" | tr '/ ' '__')
  [ -f "$cache_dir/$key" ] || normalize "$f" >"$cache_dir/$key"
  echo "$cache_dir/$key"
}

while IFS='|' read -r root core_rel shim_rel decl; do
  [ -n "$root" ] || continue
  case "$root" in
    core) core_file="$CORE/$core_rel" ;;
    libs) core_file="$LIBS/$core_rel" ;;
    *) echo "bad table row: $root" >&2; exit 2 ;;
  esac
  shim_file="$SHIM/$shim_rel"
  core_decl=$decl
  shim_decl=$decl
  case "$decl" in
    *' => '*) core_decl=${decl%% => *}; shim_decl=${decl#* => } ;;
  esac
  checked=$((checked + 1))
  if [ ! -f "$core_file" ]; then
    echo "MISSING core header: $core_rel"
    failed=$((failed + 1))
    continue
  fi
  if [ ! -f "$shim_file" ]; then
    echo "MISSING shim header: shim/$shim_rel"
    failed=$((failed + 1))
    continue
  fi
  want_core=$(declare_normalized "$core_decl")
  want_shim=$(declare_normalized "$shim_decl")
  if ! grep -qF -- "$want_core" "$(normalized_file "$core_file")"; then
    echo "MISMATCH core $core_rel lacks: $core_decl"
    failed=$((failed + 1))
  fi
  if ! grep -qF -- "$want_shim" "$(normalized_file "$shim_file")"; then
    echo "MISMATCH shim/$shim_rel lacks: $shim_decl"
    failed=$((failed + 1))
  fi
done <<<"$TABLE"

if [ "$failed" -ne 0 ]; then
  echo "signatures FAILED: $failed problem(s) in $checked declarations (core $VERSION at $CORE)"
  exit 1
fi
echo "signatures ok"
