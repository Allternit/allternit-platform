// Native board: NetworkClientSecure STUB (step 2).
//
// connect() always fails, so the relay path behaves like a board that cannot
// reach the relay. Step 3 replaces this one header with the real interface
// and adds runtime/tls_openssl.cpp (OpenSSL TLS with hostname and CA checks,
// trust from LINK_NATIVE_CA_FILE, address from native::resolveHost()).
// Signatures copy arduino-esp32 3.3.12 NetworkClientSecure.h.
#pragma once

#include <WiFi.h>

class NetworkClientSecure : public NetworkClient {
 public:
  int connect(IPAddress ip, uint16_t port) override { return 0; }
  int connect(IPAddress ip, uint16_t port, int32_t timeout) override { return 0; }
  int connect(const char *host, uint16_t port) override { return 0; }
  int connect(const char *host, uint16_t port, int32_t timeout) override { return 0; }
  void setHandshakeTimeout(unsigned long handshake_timeout) { handshakeTimeout_ = handshake_timeout; }
  void useBuiltinCACertBundle() {}

 private:
  unsigned long handshakeTimeout_ = 120;
};
