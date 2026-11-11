// Native board: WiFi.h and NetworkClient (decision 11: Wi-Fi is a table,
// sockets are real).
//
// WiFi: the networks "in the air" come from `wifi air <ssidhex> <passhex|->`.
// begin() joins after WIFI_JOIN_MS of board time when the SSID is in the air
// and the passphrase matches exactly (an open network needs an empty one, as
// the core's WPA2 threshold requires). With auto-reconnect it rejoins when the
// network comes back. Events: wifi {state: joining|connected|disconnected}.
//
// NetworkClient: a real TCP socket with the core's 3.3.12 semantics, kept on
// purpose: available() counts what FIONREAD reports, connected() peeks one
// byte and treats a 0-byte peek (the peer's FIN) as still connected, and a
// failed send() stops the client. connect() fails without Wi-Fi.
#pragma once

#include <Arduino.h>

typedef enum {
  WL_NO_SHIELD = 255,
  WL_STOPPED = 254,
  WL_IDLE_STATUS = 0,
  WL_NO_SSID_AVAIL = 1,
  WL_SCAN_COMPLETED = 2,
  WL_CONNECTED = 3,
  WL_CONNECT_FAILED = 4,
  WL_CONNECTION_LOST = 5,
  WL_DISCONNECTED = 6
} wl_status_t;

class NetworkClient : public Stream {
 public:
  NetworkClient();
  ~NetworkClient() override;
  NetworkClient(const NetworkClient &) = delete;
  NetworkClient &operator=(const NetworkClient &) = delete;

  virtual int connect(IPAddress ip, uint16_t port);
  virtual int connect(IPAddress ip, uint16_t port, int32_t timeout_ms);
  virtual int connect(const char *host, uint16_t port);
  virtual int connect(const char *host, uint16_t port, int32_t timeout_ms);
  size_t write(uint8_t data) override;
  size_t write(const uint8_t *buf, size_t size) override;
  using Print::write;
  int available() override;
  int read() override;
  virtual void stop();
  virtual uint8_t connected();
  int setNoDelay(bool nodelay);

 protected:
  int fd_ = -1;
  bool connected_ = false;
  // Receive buffer, 1436 bytes as in the core.
  uint8_t rx_[1436];
  size_t rxPos_ = 0, rxFill_ = 0;
  bool rxFailed_ = false;
  int connectAddr(const struct sockaddr *addr, unsigned len, int32_t timeout_ms);
  size_t rxAvailable();
  size_t rxFillBuffer();
};
typedef NetworkClient WiFiClient;

class WiFiClass {
 public:
  wl_status_t begin(const char *ssid, const char *passphrase = NULL, int32_t channel = 0, const uint8_t *bssid = NULL, bool tryConnect = true);
  wl_status_t status();
  bool disconnect(bool wifioff = false, bool eraseap = false, unsigned long timeoutLength = 100);
  bool setAutoReconnect(bool autoReconnect);
  static bool setHostname(const char *hostname);
  bool setSleep(bool enabled);
};
extern WiFiClass WiFi;
