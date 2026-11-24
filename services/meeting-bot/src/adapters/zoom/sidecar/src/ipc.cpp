#include "ipc.h"

#include <cerrno>
#include <cstring>
#include <fcntl.h>
#include <unistd.h>

namespace allternit {

namespace {

std::string frameBytes(uint8_t type, const void* body, size_t len) {
  const uint32_t n = static_cast<uint32_t>(len + 1);
  std::string f;
  f.resize(5 + len);
  f[0] = static_cast<char>((n >> 24) & 0xff);
  f[1] = static_cast<char>((n >> 16) & 0xff);
  f[2] = static_cast<char>((n >> 8) & 0xff);
  f[3] = static_cast<char>(n & 0xff);
  f[4] = static_cast<char>(type);
  if (len) std::memcpy(&f[5], body, len);
  return f;
}

uint32_t be32(const uint8_t* p) {
  return (uint32_t(p[0]) << 24) | (uint32_t(p[1]) << 16) | (uint32_t(p[2]) << 8) | uint32_t(p[3]);
}
uint16_t be16(const uint8_t* p) { return uint16_t((uint16_t(p[0]) << 8) | uint16_t(p[1])); }

}  // namespace

Ipc::Ipc(int fd) : fd_(fd) {
  const int flags = fcntl(fd_, F_GETFL, 0);
  if (flags >= 0) fcntl(fd_, F_SETFL, flags | O_NONBLOCK);
  writer_ = std::thread([this] { writerLoop(); });
}

Ipc::~Ipc() {
  {
    std::lock_guard<std::mutex> lk(mu_);
    stopping_ = true;
  }
  cv_.notify_all();
  if (writer_.joinable()) writer_.join();
  if (watch_) g_source_remove(watch_);
  if (channel_) g_io_channel_unref(channel_);
}

void Ipc::start() {
  channel_ = g_io_channel_unix_new(fd_);
  watch_ = g_io_add_watch(channel_, static_cast<GIOCondition>(G_IO_IN | G_IO_HUP | G_IO_ERR), &Ipc::onReadable, this);
}

gboolean Ipc::onReadable(GIOChannel*, GIOCondition cond, gpointer self) {
  auto* ipc = static_cast<Ipc*>(self);
  bool alive = true;
  if (cond & G_IO_IN) alive = ipc->readSome();
  if (alive && (cond & (G_IO_HUP | G_IO_ERR)) && !(cond & G_IO_IN)) alive = false;
  if (!alive) {
    ipc->watch_ = 0;
    if (!ipc->closed_) {
      ipc->closed_ = true;
      if (ipc->onClose_) ipc->onClose_();
    }
    return FALSE;
  }
  return TRUE;
}

bool Ipc::readSome() {
  uint8_t buf[65536];
  for (;;) {
    const ssize_t n = ::read(fd_, buf, sizeof buf);
    if (n > 0) {
      in_.insert(in_.end(), buf, buf + n);
      continue;
    }
    if (n == 0) {
      parseFrames();
      return false;  // peer closed
    }
    if (errno == EINTR) continue;
    if (errno == EAGAIN || errno == EWOULDBLOCK) break;
    return false;
  }
  parseFrames();
  return !closed_;
}

void Ipc::parseFrames() {
  size_t off = 0;
  while (in_.size() - off >= 4) {
    const uint32_t n = be32(&in_[off]);
    if (n < 1 || n > kMaxFrameBytes) {
      // Corrupt stream: stop reading; main.cpp leaves the meeting and exits.
      in_.clear();
      closed_ = true;
      if (onClose_) onClose_();
      return;
    }
    if (in_.size() - off < 4 + size_t(n)) break;
    const uint8_t type = in_[off + 4];
    const uint8_t* body = &in_[off + 5];
    const size_t len = n - 1;
    switch (type) {
      case kFrameJson: {
        auto msg = nlohmann::json::parse(body, body + len, nullptr, /*allow_exceptions=*/false);
        if (msg.is_object() && onJson_) onJson_(msg);
        else if (!msg.is_object()) log("warn", "ignored a JSON frame that is not an object");
        break;
      }
      case kFrameAudioIn: {
        if (len >= 5 && onAudio_) {
          AudioIn a{be32(body), body[4], std::vector<uint8_t>(body + 5, body + len)};
          onAudio_(std::move(a));
        }
        break;
      }
      case kFrameVideoIn: {
        if (len >= 4 && onVideo_) {
          VideoIn v{be16(body), be16(body + 2), std::vector<uint8_t>(body + 4, body + len)};
          onVideo_(std::move(v));
        }
        break;
      }
      default:
        break;  // unknown frame types are skipped, for forward compatibility
    }
    off += 4 + n;
  }
  if (off) in_.erase(in_.begin(), in_.begin() + static_cast<std::ptrdiff_t>(off));
}

void Ipc::sendJson(const nlohmann::json& msg) {
  // dump(-1, ' ', false, replace): invalid UTF-8 from the SDK (names, chat) must not throw.
  const std::string s = msg.dump(-1, ' ', false, nlohmann::json::error_handler_t::replace);
  enqueue(frameBytes(kFrameJson, s.data(), s.size()), false);
}

void Ipc::sendAudioOut(uint32_t sampleRate, uint8_t channels, const char* pcm, size_t len) {
  std::string body;
  body.resize(5 + len);
  body[0] = static_cast<char>((sampleRate >> 24) & 0xff);
  body[1] = static_cast<char>((sampleRate >> 16) & 0xff);
  body[2] = static_cast<char>((sampleRate >> 8) & 0xff);
  body[3] = static_cast<char>(sampleRate & 0xff);
  body[4] = static_cast<char>(channels);
  if (len) std::memcpy(&body[5], pcm, len);
  enqueue(frameBytes(kFrameAudioOut, body.data(), body.size()), true);
}

void Ipc::log(const char* level, const std::string& msg) {
  sendJson({{"type", "log"}, {"level", level}, {"msg", msg}});
}

void Ipc::enqueue(std::string&& frame, bool isAudio) {
  {
    std::lock_guard<std::mutex> lk(mu_);
    if (stopping_) return;
    if (isAudio) {
      if (queuedAudio_ + frame.size() > kMaxQueuedAudioBytes) {
        dropped_.fetch_add(1);
        return;
      }
      queuedAudio_ += frame.size();
    }
    out_.emplace_back(std::move(frame), isAudio);
  }
  cv_.notify_one();
}

void Ipc::writerLoop() {
  for (;;) {
    std::pair<std::string, bool> item;
    {
      std::unique_lock<std::mutex> lk(mu_);
      cv_.wait(lk, [this] { return stopping_ || !out_.empty(); });
      if (out_.empty()) return;  // stopping and drained
      item = std::move(out_.front());
      out_.pop_front();
      if (item.second) queuedAudio_ -= item.first.size();
    }
    const char* p = item.first.data();
    size_t left = item.first.size();
    while (left > 0) {
      const ssize_t n = ::write(fd_, p, left);
      if (n > 0) {
        p += n;
        left -= size_t(n);
      } else if (n < 0 && (errno == EAGAIN || errno == EWOULDBLOCK || errno == EINTR)) {
        usleep(1000);  // the fd is non-blocking for the reader; back off briefly
      } else {
        return;  // peer gone
      }
    }
  }
}

}  // namespace allternit
