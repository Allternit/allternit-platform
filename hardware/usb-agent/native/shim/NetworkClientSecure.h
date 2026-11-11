// Native board: NetworkClientSecure on OpenSSL 3 (plan step 3, decision 4).
//
// Signatures follow arduino-esp32 3.3.12 NetworkClientSecure.h; the behaviour
// the firmware can observe follows the real class (see runtime/tls_openssl.cpp):
//   - trust comes ONLY from the file named by LINK_NATIVE_CA_FILE. With no
//     file, or one that cannot be read, every connect fails. The built-in
//     CA bundle request (useBuiltinCACertBundle) maps to that same file;
//   - the server certificate chain and the host name are always verified.
//     There is no insecure mode;
//   - the address comes from native::resolveHost() (LINK_NATIVE_HOSTS);
//   - connect's timeout bounds the TCP connect, setHandshakeTimeout() (seconds,
//     default 120) bounds the TLS handshake;
//   - available()/read() never block; write() retries like NetworkClient.
#pragma once

#include <WiFi.h>

#include <string>

class NetworkClientSecure : public NetworkClient {
 public:
  NetworkClientSecure();
  ~NetworkClientSecure() override;

  int connect(IPAddress ip, uint16_t port) override;
  int connect(IPAddress ip, uint16_t port, int32_t timeout) override;
  int connect(const char *host, uint16_t port) override;
  int connect(const char *host, uint16_t port, int32_t timeout) override;
  size_t write(uint8_t data) override;
  size_t write(const uint8_t *buf, size_t size) override;
  using Print::write;
  int available() override;
  int read() override;
  void stop() override;
  uint8_t connected() override;
  void setHandshakeTimeout(unsigned long handshake_timeout);
  void useBuiltinCACertBundle() { useBundle_ = true; }

 private:
  void *ssl_ = nullptr;  // SSL*, kept opaque so the firmware never sees OpenSSL
  std::string plain_;    // decrypted bytes not yet read
  size_t plainPos_ = 0;
  unsigned long handshakeTimeout_ = 120;  // seconds, as in the core
  bool useBundle_ = false;

  int tlsConnect(const char *host, uint16_t port, int32_t timeout);
  bool pump();  // one non-blocking SSL_read into plain_; false once the link is gone
};
