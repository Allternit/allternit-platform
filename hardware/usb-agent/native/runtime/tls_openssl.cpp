// Native board: the TLS client behind shim/NetworkClientSecure.h (plan step 3).
//
// OpenSSL 3 is linked, not copied. Trust: only the file named by
// LINK_NATIVE_CA_FILE; no file means every connect fails. The chain and the
// host name are verified on every connection (there is no insecure switch).
// The TCP part is NetworkClient::connectAddr, so sockets are tracked for a
// reboot like every other board socket.
#include <Arduino.h>
#include <NetworkClientSecure.h>
#include <arpa/inet.h>
#include <errno.h>
#include <native_runtime.h>
#include <openssl/err.h>
#include <openssl/ssl.h>
#include <openssl/x509v3.h>
#include <poll.h>
#include <signal.h>

#include <algorithm>
#include <chrono>
#include <cstdlib>
#include <cstring>

static const int WRITE_RETRIES = 10;     // as NetworkClient::write
static const int WRITE_SELECT_MS = 1000;
static const size_t READ_CHUNK = 1436;

static long monoMs() {
  using namespace std::chrono;
  return (long)duration_cast<milliseconds>(steady_clock::now().time_since_epoch()).count();
}

NetworkClientSecure::NetworkClientSecure() {}

NetworkClientSecure::~NetworkClientSecure() {
  stop();
}

void NetworkClientSecure::setHandshakeTimeout(unsigned long handshake_timeout) {
  handshakeTimeout_ = handshake_timeout;
}

int NetworkClientSecure::connect(IPAddress ip, uint16_t port) {
  return connect(ip.toString().c_str(), port, 3000);
}
int NetworkClientSecure::connect(IPAddress ip, uint16_t port, int32_t timeout) {
  return connect(ip.toString().c_str(), port, timeout);
}
int NetworkClientSecure::connect(const char *host, uint16_t port) {
  return connect(host, port, 3000);
}
int NetworkClientSecure::connect(const char *host, uint16_t port, int32_t timeout) {
  stop();
  int r = tlsConnect(host, port, timeout);
  if (r != 1) stop();
  return r;
}

int NetworkClientSecure::tlsConnect(const char *host, uint16_t port, int32_t timeout) {
  const char *caFile = getenv("LINK_NATIVE_CA_FILE");
  if (!host || !*host || !caFile || !*caFile) return 0;
#ifndef SO_NOSIGPIPE
  signal(SIGPIPE, SIG_IGN);  // OpenSSL writes with write(); the host build has no per-send flag
#endif
  sockaddr_storage addr;
  socklen_t len = 0;
  if (!native::resolveHost(host, port, &addr, &len)) return 0;

  SSL_CTX *ctx = SSL_CTX_new(TLS_client_method());
  if (!ctx) return 0;
  SSL_CTX_set_min_proto_version(ctx, TLS1_2_VERSION);
  SSL_CTX_set_verify(ctx, SSL_VERIFY_PEER, nullptr);
  if (SSL_CTX_load_verify_file(ctx, caFile) != 1) {
    SSL_CTX_free(ctx);
    return 0;
  }
  SSL *ssl = SSL_new(ctx);
  SSL_CTX_free(ctx);  // the SSL keeps its own reference
  if (!ssl) return 0;

  // Host name: a DNS name is checked against the certificate's names (and sent
  // as SNI); an IP literal against its IP addresses.
  unsigned char probe[16];
  bool literal = inet_pton(AF_INET, host, probe) == 1 || inet_pton(AF_INET6, host, probe) == 1;
  bool named = literal ? SSL_set1_host(ssl, nullptr) == 1 && X509_VERIFY_PARAM_set1_ip_asc(SSL_get0_param(ssl), host) == 1
                       : SSL_set1_host(ssl, host) == 1 && SSL_set_tlsext_host_name(ssl, host) == 1;
  if (!named) {
    SSL_free(ssl);
    return 0;
  }

  if (!connectAddr((const sockaddr *)&addr, (unsigned)len, timeout)) {
    SSL_free(ssl);
    return 0;
  }
  if (SSL_set_fd(ssl, fd_) != 1) {
    SSL_free(ssl);
    return 0;
  }
  ssl_ = ssl;

  long deadline = monoMs() + (long)handshakeTimeout_ * 1000;
  for (;;) {
    int r = SSL_connect(ssl);
    if (r == 1) break;
    int err = SSL_get_error(ssl, r);
    if (err != SSL_ERROR_WANT_READ && err != SSL_ERROR_WANT_WRITE) return 0;
    long left = deadline - monoMs();
    if (left <= 0) return 0;
    pollfd p = {fd_, (short)(err == SSL_ERROR_WANT_READ ? POLLIN : POLLOUT), 0};
    int pr = poll(&p, 1, (int)std::min<long>(left, 1000));
    if (pr < 0 && errno != EINTR) return 0;
  }
  if (SSL_get_verify_result(ssl) != X509_V_OK) return 0;
  return 1;
}

// One non-blocking read. Data goes to plain_. A close_notify, a reset or any
// other failure ends the connection, as the core's data_to_read() does.
bool NetworkClientSecure::pump() {
  if (!ssl_) return false;
  SSL *ssl = (SSL *)ssl_;
  char buf[READ_CHUNK];
  ERR_clear_error();
  int n = SSL_read(ssl, buf, (int)sizeof buf);
  if (n > 0) {
    if (plainPos_ == plain_.size()) {
      plain_.clear();
      plainPos_ = 0;
    }
    plain_.append(buf, (size_t)n);
    return true;
  }
  int err = SSL_get_error(ssl, n);
  if (err == SSL_ERROR_WANT_READ || err == SSL_ERROR_WANT_WRITE) return true;
  stop();
  return false;
}

int NetworkClientSecure::available() {
  if (!ssl_) return 0;
  if (plainPos_ == plain_.size()) pump();
  if (!ssl_) return 0;
  return (int)(plain_.size() - plainPos_) + SSL_pending((SSL *)ssl_);
}

int NetworkClientSecure::read() {
  if (!ssl_) return -1;
  if (plainPos_ == plain_.size() && !pump()) return -1;
  if (plainPos_ == plain_.size()) return -1;
  return (uint8_t)plain_[plainPos_++];
}

uint8_t NetworkClientSecure::connected() {
  if (!ssl_ || !connected_) return 0;
  if (plainPos_ == plain_.size() && SSL_pending((SSL *)ssl_) == 0) pump();  // notices a close
  return ssl_ != nullptr && connected_;
}

size_t NetworkClientSecure::write(uint8_t data) {
  return write(&data, 1);
}

size_t NetworkClientSecure::write(const uint8_t *buf, size_t size) {
  if (!ssl_ || !connected_ || fd_ < 0) return 0;
  SSL *ssl = (SSL *)ssl_;
  size_t sent = 0;
  int retry = WRITE_RETRIES;
  while (retry && sent < size) {
    ERR_clear_error();
    int n = SSL_write(ssl, buf + sent, (int)(size - sent));
    if (n > 0) {
      sent += (size_t)n;
      retry = WRITE_RETRIES;
      continue;
    }
    int err = SSL_get_error(ssl, n);
    if (err != SSL_ERROR_WANT_READ && err != SSL_ERROR_WANT_WRITE) {
      stop();
      break;
    }
    retry--;
    pollfd p = {fd_, (short)(err == SSL_ERROR_WANT_READ ? POLLIN : POLLOUT), 0};
    if (poll(&p, 1, WRITE_SELECT_MS) < 0 && errno != EINTR) break;
  }
  return sent;
}

void NetworkClientSecure::stop() {
  if (ssl_) {
    SSL *ssl = (SSL *)ssl_;
    ssl_ = nullptr;
    if (connected_) SSL_shutdown(ssl);  // best effort close_notify; the socket is non-blocking
    SSL_free(ssl);
  }
  plain_.clear();
  plainPos_ = 0;
  NetworkClient::stop();
}
