// Framed IPC with the Node adapter over one socket fd (fd 3 by default).
// Protocol: ../../protocol.ts and ../README.md "Protocol". Keep them in step.
//
// Reading happens on the GLib main loop (the SDK's callback thread), so every
// command runs on the same thread as SDK callbacks. Writing goes through one
// writer thread, because raw-audio callbacks arrive on SDK worker threads.
// JSON frames are never dropped; audio frames are dropped when the queue holds
// more than kMaxQueuedAudioBytes (Node fell behind), and the drop is counted.
#pragma once

#include <atomic>
#include <condition_variable>
#include <cstdint>
#include <deque>
#include <functional>
#include <mutex>
#include <string>
#include <thread>
#include <vector>

#include <glib.h>
#include <nlohmann/json.hpp>

namespace allternit {

constexpr uint8_t kFrameJson = 0x01;
constexpr uint8_t kFrameAudioOut = 0x02;
constexpr uint8_t kFrameAudioIn = 0x03;
constexpr uint8_t kFrameVideoIn = 0x04;
constexpr uint32_t kMaxFrameBytes = 16u * 1024u * 1024u;
constexpr size_t kMaxQueuedAudioBytes = 4u * 1024u * 1024u;

struct AudioIn {
  uint32_t sampleRate;
  uint8_t channels;
  std::vector<uint8_t> pcm;  // s16le
};

struct VideoIn {
  uint16_t width;
  uint16_t height;
  std::vector<uint8_t> i420;
};

class Ipc {
 public:
  using JsonHandler = std::function<void(const nlohmann::json&)>;
  using AudioHandler = std::function<void(AudioIn&&)>;
  using VideoHandler = std::function<void(VideoIn&&)>;
  using CloseHandler = std::function<void()>;

  explicit Ipc(int fd);
  ~Ipc();

  void onJson(JsonHandler h) { onJson_ = std::move(h); }
  void onAudio(AudioHandler h) { onAudio_ = std::move(h); }
  void onVideo(VideoHandler h) { onVideo_ = std::move(h); }
  void onClose(CloseHandler h) { onClose_ = std::move(h); }

  /** Attach the read watch to the default GLib main context. */
  void start();

  /** Thread-safe. */
  void sendJson(const nlohmann::json& msg);
  /** Thread-safe. s16le PCM of mixed call audio. */
  void sendAudioOut(uint32_t sampleRate, uint8_t channels, const char* pcm, size_t len);
  /** Thread-safe. */
  void log(const char* level, const std::string& msg);

  uint64_t droppedAudioFrames() const { return dropped_.load(); }

 private:
  static gboolean onReadable(GIOChannel* ch, GIOCondition cond, gpointer self);
  bool readSome();
  void parseFrames();
  void enqueue(std::string&& frame, bool isAudio);
  void writerLoop();

  int fd_;
  GIOChannel* channel_ = nullptr;
  guint watch_ = 0;
  std::vector<uint8_t> in_;

  std::mutex mu_;
  std::condition_variable cv_;
  std::deque<std::pair<std::string, bool>> out_;  // frame bytes, isAudio
  size_t queuedAudio_ = 0;
  bool stopping_ = false;
  std::atomic<uint64_t> dropped_{0};
  std::thread writer_;

  JsonHandler onJson_;
  AudioHandler onAudio_;
  VideoHandler onVideo_;
  CloseHandler onClose_;
  bool closed_ = false;
};

}  // namespace allternit
