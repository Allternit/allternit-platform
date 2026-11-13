// zoom-sidecar: the Zoom Meeting SDK for Linux behind a framed socket protocol.
// Spawned by ../../index.ts with the protocol socket on fd 3 (ZOOM_IPC_FD).
// stdout/stderr carry only logs (the SDK and Qt write there too). Tokens are
// never logged: they arrive inside the `join` command and go straight to the SDK.

#include <csignal>
#include <cstdlib>
#include <fstream>
#include <string>
#include <sys/stat.h>

#include <glib.h>

#include "bot.h"
#include "ipc.h"

namespace {

GMainLoop* g_loop = nullptr;

void onSignal(int) {
  if (g_loop) g_main_loop_quit(g_loop);
}

// Raw audio needs PulseAudio plus ~/.config/zoomus.conf with system.audio.type=default
// (https://developers.zoom.us/docs/meeting-sdk/linux/custom-ui/advanced-features/raw-data/).
void ensureZoomConf() {
  const char* home = std::getenv("HOME");
  if (!home || !*home) return;
  const std::string dir = std::string(home) + "/.config";
  const std::string path = dir + "/zoomus.conf";
  struct stat st {};
  if (stat(path.c_str(), &st) == 0) return;
  mkdir(dir.c_str(), 0700);
  std::ofstream f(path);
  f << "[General]\nsystem.audio.type=default\n";
}

}  // namespace

int main() {
  const char* fdEnv = std::getenv("ZOOM_IPC_FD");
  const int fd = fdEnv ? std::atoi(fdEnv) : 3;
  struct stat st {};
  if (fstat(fd, &st) != 0) {
    std::fprintf(stderr, "zoom-sidecar: fd %d is not open; run me from the Node adapter\n", fd);
    return 2;
  }
  std::signal(SIGPIPE, SIG_IGN);
  std::signal(SIGINT, onSignal);
  std::signal(SIGTERM, onSignal);
  ensureZoomConf();

  g_loop = g_main_loop_new(nullptr, FALSE);
  int exitCode = 0;
  {
    allternit::Ipc ipc(fd);
    allternit::ZoomBot bot(ipc);
    bot.setLoop(g_loop);

    ipc.onJson([&](const nlohmann::json& cmd) { bot.handle(cmd); });
    ipc.onAudio([&](allternit::AudioIn&& a) { bot.pushMic(std::move(a)); });
    ipc.onVideo([&](allternit::VideoIn&& v) { bot.pushCamera(std::move(v)); });
    // Node went away (or sent a corrupt frame): leave the meeting and exit.
    ipc.onClose([&] { bot.shutdown(); });
    ipc.start();

    if (!bot.init()) {
      exitCode = 3;
    } else {
      ipc.sendJson({{"type", "ready"}, {"sdkVersion", allternit::ZoomBot::sdkVersion()}});
      g_main_loop_run(g_loop);
    }
    bot.leaveIfActive();
    bot.cleanup();
    if (ipc.droppedAudioFrames() > 0) {
      std::fprintf(stderr, "zoom-sidecar: dropped %llu audio frames (Node fell behind)\n",
                   static_cast<unsigned long long>(ipc.droppedAudioFrames()));
    }
  }  // Ipc destructor drains queued frames
  g_main_loop_unref(g_loop);
  return exitCode;
}
