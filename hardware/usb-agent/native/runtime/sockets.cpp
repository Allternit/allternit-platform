// Native board: NetworkClient on a real TCP socket, with the core 3.3.12
// behaviour the firmware can observe (see shim/WiFi.h):
//   - connect() needs Wi-Fi; a host is an IP literal or a LINK_NATIVE_HOSTS
//     name (no DNS); the timeout bounds the TCP connect;
//   - available() = buffered bytes + FIONREAD; a FIONREAD error stops it;
//   - read() returns -1 when nothing is buffered or readable;
//   - connected() peeks one byte: 0 bytes (the peer's FIN) still counts as
//     connected, ECONNRESET/EPIPE/ENOTCONN/ECONNREFUSED/ECONNABORTED do not;
//   - write() retries for room (1 s select, 10 tries) and stops the client on
//     an error other than EAGAIN.
#include <Arduino.h>
#include <WiFi.h>
#include <arpa/inet.h>
#include <errno.h>
#include <fcntl.h>
#include <native_runtime.h>
#include <netinet/in.h>
#include <netinet/tcp.h>
#include <poll.h>
#include <sys/ioctl.h>
#include <sys/socket.h>
#include <unistd.h>

#include <mutex>
#include <set>
#include <string>
#include <thread>
#include <vector>

#ifndef MSG_NOSIGNAL
#define MSG_NOSIGNAL 0
#endif

static const int32_t DEFAULT_CONNECT_TIMEOUT_MS = 3000;  // WIFI_CLIENT_DEF_CONN_TIMEOUT_MS
static const int WRITE_RETRIES = 10;                     // WIFI_CLIENT_MAX_WRITE_RETRY
static const int WRITE_SELECT_MS = 1000;                 // WIFI_CLIENT_SELECT_TIMEOUT_US

// ── sockets across a reboot ─────────────────────────────────────────────────
// A board that resets sends nothing: its peer keeps a half-dead connection
// and only learns of it when it sends and the new stack answers with a reset.
// So the board's open sockets survive the exec, and the next boot watches
// them: the first thing that arrives on one gets an RST (SO_LINGER 0, close).

namespace native {

static std::mutex g_sockMutex;
static std::set<int> g_sockets;

void trackSocket(int fd) {
  std::lock_guard<std::mutex> lock(g_sockMutex);
  g_sockets.insert(fd);
}

void untrackSocket(int fd) {
  std::lock_guard<std::mutex> lock(g_sockMutex);
  g_sockets.erase(fd);
}

std::string socketsForNextBoot() {
  std::lock_guard<std::mutex> lock(g_sockMutex);
  std::string out;
  for (int fd : g_sockets) {
    int flags = fcntl(fd, F_GETFD);
    if (flags < 0) continue;
    fcntl(fd, F_SETFD, flags & ~FD_CLOEXEC);
    if (!out.empty()) out += ",";
    out += std::to_string(fd);
  }
  return out;
}

static void resetWhenTouched(std::vector<int> fds) {
  while (!fds.empty()) {
    std::vector<pollfd> ps;
    for (int fd : fds) ps.push_back({fd, POLLIN, 0});
    if (poll(ps.data(), (nfds_t)ps.size(), 1000) < 0 && errno != EINTR) return;
    std::vector<int> left;
    for (auto &p : ps) {
      if (p.revents & (POLLIN | POLLHUP | POLLERR | POLLNVAL)) {
        linger l = {1, 0};
        setsockopt(p.fd, SOL_SOCKET, SO_LINGER, &l, sizeof l);
        ::close(p.fd);
      } else {
        left.push_back(p.fd);
      }
    }
    fds = left;
  }
}

void adoptSocketsFromLastBoot(const char *list) {
  if (!list || !*list) return;
  std::vector<int> fds;
  std::string all = list;
  size_t at = 0;
  while (at < all.size()) {
    size_t end = all.find(',', at);
    if (end == std::string::npos) end = all.size();
    int fd = atoi(all.substr(at, end - at).c_str());
    if (fd > 2) {
      fcntl(fd, F_SETFD, FD_CLOEXEC);  // one boot only
      fds.push_back(fd);
    }
    at = end + 1;
  }
  if (!fds.empty()) std::thread(resetWhenTouched, fds).detach();
}

}  // namespace native

NetworkClient::NetworkClient() {}

NetworkClient::~NetworkClient() {
  stop();
}

int NetworkClient::connect(IPAddress ip, uint16_t port) {
  return connect(ip, port, DEFAULT_CONNECT_TIMEOUT_MS);
}

int NetworkClient::connect(IPAddress ip, uint16_t port, int32_t timeout_ms) {
  return connect(ip.toString().c_str(), port, timeout_ms);
}

int NetworkClient::connect(const char *host, uint16_t port) {
  return connect(host, port, DEFAULT_CONNECT_TIMEOUT_MS);
}

int NetworkClient::connect(const char *host, uint16_t port, int32_t timeout_ms) {
  sockaddr_storage addr;
  socklen_t len = 0;
  if (!native::resolveHost(host, port, &addr, &len)) return 0;
  return connectAddr((const sockaddr *)&addr, len, timeout_ms);
}

int NetworkClient::connectAddr(const sockaddr *addr, unsigned len, int32_t timeout_ms) {
  stop();
  int fd = ::socket(addr->sa_family, SOCK_STREAM, 0);
  if (fd < 0) return 0;
  fcntl(fd, F_SETFD, FD_CLOEXEC);  // see "sockets across a reboot"
#ifdef SO_NOSIGPIPE
  int one = 1;
  setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &one, sizeof one);
#endif
  fcntl(fd, F_SETFL, fcntl(fd, F_GETFL) | O_NONBLOCK);
  int r = ::connect(fd, addr, (socklen_t)len);
  if (r != 0 && errno != EINPROGRESS) {
    ::close(fd);
    return 0;
  }
  if (r != 0) {
    pollfd p = {fd, POLLOUT, 0};
    int pr;
    do {
      pr = poll(&p, 1, timeout_ms < 0 ? 0 : timeout_ms);
    } while (pr < 0 && errno == EINTR);
    int err = 0;
    socklen_t elen = sizeof err;
    if (pr <= 0 || getsockopt(fd, SOL_SOCKET, SO_ERROR, &err, &elen) != 0 || err != 0) {
      ::close(fd);
      return 0;
    }
  }
  fd_ = fd;
  native::trackSocket(fd);
  connected_ = true;
  rxPos_ = rxFill_ = 0;
  rxFailed_ = false;
  return 1;
}

size_t NetworkClient::rxAvailable() {
  if (fd_ < 0) return 0;
  int count = -1;
  if (ioctl(fd_, FIONREAD, &count) < 0 || count < 0) {
    rxFailed_ = true;
    return 0;
  }
  return (size_t)count;
}

size_t NetworkClient::rxFillBuffer() {
  if (rxFill_ && rxPos_ == rxFill_) rxFill_ = rxPos_ = 0;
  if (sizeof rx_ <= rxFill_ || !rxAvailable()) return 0;
  ssize_t res = ::recv(fd_, rx_ + rxFill_, sizeof rx_ - rxFill_, MSG_DONTWAIT);
  if (res < 0) {
    if (errno != EWOULDBLOCK && errno != EAGAIN) rxFailed_ = true;
    return 0;
  }
  rxFill_ += (size_t)res;
  return (size_t)res;
}

int NetworkClient::available() {
  if (fd_ < 0) return 0;
  size_t res = (rxFill_ - rxPos_) + rxAvailable();
  if (rxFailed_) stop();
  return (int)res;
}

int NetworkClient::read() {
  if (fd_ < 0) return -1;
  if (rxPos_ == rxFill_ && !rxFillBuffer()) {
    if (rxFailed_) {
      stop();
      return -1;
    }
    return -1;
  }
  return rx_[rxPos_++];
}

uint8_t NetworkClient::connected() {
  if (fd_ == -1 && connected_) stop();
  if (connected_) {
    uint8_t dummy;
    ssize_t res = ::recv(fd_, &dummy, 1, MSG_DONTWAIT | MSG_PEEK);
    if (res < 0) {
      switch (errno) {
        case EWOULDBLOCK:
        case ENOENT:
          connected_ = true;
          break;
        case ENOTCONN:
        case EPIPE:
        case ECONNRESET:
        case ECONNREFUSED:
        case ECONNABORTED:
          connected_ = false;
          break;
        default:
          connected_ = true;
          break;
      }
    } else {
      connected_ = true;  // includes res == 0, the peer's FIN, as in the core
    }
  }
  return connected_;
}

size_t NetworkClient::write(uint8_t data) {
  return write(&data, 1);
}

size_t NetworkClient::write(const uint8_t *buf, size_t size) {
  if (!connected_ || fd_ < 0) return 0;
  size_t sent = 0;
  int retry = WRITE_RETRIES;
  while (retry) {
    retry--;
    pollfd p = {fd_, POLLOUT, 0};
    if (poll(&p, 1, WRITE_SELECT_MS) < 0) return 0;
    if (p.revents & (POLLOUT | POLLERR | POLLHUP)) {
      ssize_t res = ::send(fd_, buf + sent, size - sent, MSG_DONTWAIT | MSG_NOSIGNAL);
      if (res > 0) {
        sent += (size_t)res;
        if (sent >= size) retry = 0;
        else retry = WRITE_RETRIES;
      } else if (res < 0 && errno != EAGAIN) {
        stop();
        retry = 0;
      }
    }
  }
  return sent;
}

void NetworkClient::stop() {
  if (fd_ >= 0) {
    native::untrackSocket(fd_);
    ::close(fd_);
  }
  fd_ = -1;
  connected_ = false;
  rxPos_ = rxFill_ = 0;
  rxFailed_ = false;
}

int NetworkClient::setNoDelay(bool nodelay) {
  if (fd_ < 0) return -1;
  int flag = nodelay;
  return setsockopt(fd_, IPPROTO_TCP, TCP_NODELAY, &flag, sizeof flag);
}
