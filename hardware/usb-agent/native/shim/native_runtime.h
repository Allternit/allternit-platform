// Native board: the runtime's internal interface.
//
// Not an Arduino or ESP-IDF header. The shim headers and the files in
// ../runtime/ use it; the firmware never includes it. Later steps (the TLS
// client, the SSD1681 panel model) build on these hooks instead of editing
// the files that define them.
//
// Control (stdin) and events (stdout) are JSON lines; see
// docs/research-allternit-link-native-board-PLAN.md, design decision 8.
#pragma once

#include <stdint.h>
#include <stddef.h>
#include <sys/socket.h>

#include <string>

// The firmware's entry points (usb_agent.ino).
void setup();
void loop();

namespace native {

// ── events ──────────────────────────────────────────────────────────────────
// Writes one line: {"ev":"<ev>","t":<millis>,<fields>}. `fields` is either
// empty or a comma-separated list of JSON members without braces.
void emit(const char *ev, const std::string &fields = std::string());
std::string jsonString(const std::string &s);  // a quoted, escaped JSON string
std::string hex(const uint8_t *b, size_t n);   // lowercase hex

// ── clock ───────────────────────────────────────────────────────────────────
uint32_t boardMillis();       // what millis() returns: real time since boot plus the skew
uint64_t realMonoMs();        // real monotonic milliseconds (the watchdog's clock)
void addSkew(uint64_t ms);    // `clock +<ms>`
void sleepRealMs(uint32_t ms);

// ── reboot ──────────────────────────────────────────────────────────────────
// Replaces this process with a fresh boot of the same binary (decision 5).
// `reason` is an esp_reset_reason_t value. Never returns.
[[noreturn]] void reboot(int reason);
int bootReason();
const char *ptyPath();

// ── world (things outside the board that the test controls) ────────────────
// Each command returns "" on success or an error text.
std::string control(const std::string &line);
// Serialised world, replayed after a reboot (pins held, USB host, Wi-Fi in
// the air, mDNS table, panel busy time). The clock skew is board state and
// resets.
std::string worldCommands();

// GPIO.
int gpioLevel(int pin);                       // what digitalRead()/gpio_get_level() see
void gpioDrive(int pin, int level, bool fireInterrupt);  // an outside level on an input
void gpioQuiet(int pin);                      // a peripheral model owns this pin: no `pin` events
using GpioWriteHook = void (*)(int pin, int level);
void gpioOnWrite(GpioWriteHook hook);         // called for every digitalWrite (models decode SPI etc.)

// USB host state.
enum UsbState { USB_MOUNTED, USB_ASLEEP, USB_NONE };
UsbState usbState();
bool usbDelivers();  // a HID report reaches the phone now (mounted and not suspended)

// Wi-Fi.
bool wifiConnected();

// Host names and addresses for sockets. Returns false when the board could
// not reach `host` (no Wi-Fi, not an IP literal and not in LINK_NATIVE_HOSTS).
// LINK_NATIVE_HOSTS ("relay.test=127.0.0.1:8443,…") also rewrites the port,
// because the firmware hard-codes 443 for the relay.
bool resolveHost(const char *host, uint16_t port, sockaddr_storage *out, socklen_t *outLen);

// Sockets: every board socket is tracked so a reboot can leave it half dead
// (no FIN; the next boot answers the peer's next segment with an RST). The
// TLS client (step 3) tracks its sockets the same way.
void trackSocket(int fd);
void untrackSocket(int fd);
std::string socketsForNextBoot();
void adoptSocketsFromLastBoot(const char *list);

// Panel model knob (`epd busy <ms>`); read by the panel model (step 4).
uint32_t epdBusyMs();

// Watchdog.
void wdtConfigure(uint32_t timeoutMs);
void wdtSubscribe();
bool wdtFeed();  // false when the task is not subscribed

// NVS file and serial port setup, called once per boot by main().
void nvsInit();
void serialInit(int masterFd);

}  // namespace native
