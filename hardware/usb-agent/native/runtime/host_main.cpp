// Native board: process entry, events, clock, reboot, watchdog and the
// control thread (decisions 5, 6 and 8).
//
// One process is one boot. ESP.restart(), the watchdog and `power-cycle`
// execv() this same binary, so every global and static local starts fresh, as
// on a board. Kept across a boot: the pty (its fds are inherited), stdin and
// stdout, the NVS file, and the world the test set up (pins held, USB host,
// Wi-Fi in the air, mDNS table), passed in the environment.
//
// Environment (public): LINK_NATIVE_MAC, LINK_NATIVE_STATE, LINK_NATIVE_HOSTS,
// LINK_NATIVE_CA_FILE (step 3). Internal (set by this file for the next
// boot): LINK_NATIVE_RESET, LINK_NATIVE_PTY, LINK_NATIVE_PTY_FD,
// LINK_NATIVE_PTY_HOLD_FD, LINK_NATIVE_WORLD, LINK_NATIVE_SOCKETS,
// LINK_NATIVE_STATE_TEMP.
#include <Arduino.h>
#include <esp_task_wdt.h>
#include <fcntl.h>
#include <native_runtime.h>
#include <signal.h>
#include <time.h>
#include <unistd.h>

#include <atomic>
#include <chrono>
#include <mutex>
#include <string>
#include <thread>

#ifdef __APPLE__
#include <mach-o/dyld.h>
#endif

namespace native {

// ── events ──────────────────────────────────────────────────────────────────

static std::mutex g_emitMutex;

std::string hex(const uint8_t *b, size_t n) {
  static const char *H = "0123456789abcdef";
  std::string s;
  s.reserve(n * 2);
  for (size_t i = 0; i < n; i++) {
    s += H[b[i] >> 4];
    s += H[b[i] & 15];
  }
  return s;
}

std::string jsonString(const std::string &in) {
  std::string s = "\"";
  for (unsigned char c : in) {
    if (c == '"' || c == '\\') {
      s += '\\';
      s += (char)c;
    } else if (c < 0x20 || c >= 0x7f) {
      char buf[8];
      snprintf(buf, sizeof buf, "\\u%04x", c);
      s += buf;
    } else {
      s += (char)c;
    }
  }
  return s + "\"";
}

static void writeAll(int fd, const char *p, size_t n) {
  while (n) {
    ssize_t w = ::write(fd, p, n);
    if (w < 0) {
      if (errno == EINTR) continue;
      _exit(0);  // stdout is gone: whoever ran the board has stopped
    }
    p += w;
    n -= (size_t)w;
  }
}

void emit(const char *ev, const std::string &fields) {
  std::string line = "{\"ev\":\"";
  line += ev;
  line += "\",\"t\":" + std::to_string(boardMillis());
  if (!fields.empty()) line += "," + fields;
  line += "}\n";
  std::lock_guard<std::mutex> lock(g_emitMutex);
  writeAll(1, line.data(), line.size());
}

// ── clock ───────────────────────────────────────────────────────────────────

static uint64_t g_bootMono = 0;
static std::atomic<uint64_t> g_skew{0};

uint64_t realMonoMs() {
  timespec ts;
  clock_gettime(CLOCK_MONOTONIC, &ts);
  return (uint64_t)ts.tv_sec * 1000 + (uint64_t)ts.tv_nsec / 1000000;
}
uint32_t boardMillis() {
  return (uint32_t)(realMonoMs() - g_bootMono + g_skew.load());
}
void addSkew(uint64_t ms) {
  g_skew += ms;
}
void sleepRealMs(uint32_t ms) {
  timespec ts = {(time_t)(ms / 1000), (long)(ms % 1000) * 1000000L};
  while (nanosleep(&ts, &ts) != 0 && errno == EINTR) {}
}

// ── reboot ──────────────────────────────────────────────────────────────────

static int g_bootReason = ESP_RST_POWERON;
static std::string g_ptyPath;
static int g_ptyFd = -1, g_ptyHoldFd = -1;
static std::string g_self;
static char **g_argv = nullptr;
// Held by the control thread while it reads and runs one line, so a reboot
// never cuts a command in half (a half-read line would reach the next boot
// as garbage).
static std::timed_mutex g_lineMutex;
static std::atomic<bool> g_rebooting{false};

int bootReason() {
  return g_bootReason;
}
const char *ptyPath() {
  return g_ptyPath.c_str();
}

static void rememberForNextBoot(int reason) {
  setenv("LINK_NATIVE_RESET", std::to_string(reason).c_str(), 1);
  setenv("LINK_NATIVE_PTY", g_ptyPath.c_str(), 1);
  setenv("LINK_NATIVE_PTY_FD", std::to_string(g_ptyFd).c_str(), 1);
  setenv("LINK_NATIVE_PTY_HOLD_FD", std::to_string(g_ptyHoldFd).c_str(), 1);
  setenv("LINK_NATIVE_WORLD", worldCommands().c_str(), 1);
  setenv("LINK_NATIVE_SOCKETS", socketsForNextBoot().c_str(), 1);
}

void reboot(int reason) {
  if (g_rebooting.exchange(true)) {
    // Another thread is already rebooting; wait for exec to replace us.
    for (;;) pause();
  }
  bool locked = g_lineMutex.try_lock_for(std::chrono::milliseconds(300));
  (void)locked;
  rememberForNextBoot(reason);
  g_emitMutex.lock();  // never unlocked: no half-written event line before exec
  execv(g_self.c_str(), g_argv);
  static const char msg[] = "native board: execv failed\n";
  ::write(2, msg, sizeof msg - 1);
  _exit(70);
}

static void quit() {
  if (getenv("LINK_NATIVE_STATE_TEMP") && getenv("LINK_NATIVE_STATE")) unlink(getenv("LINK_NATIVE_STATE"));
  std::lock_guard<std::mutex> lock(g_emitMutex);
  _exit(0);
}

// ── watchdog (real time, decision 6) ────────────────────────────────────────

static std::atomic<uint32_t> g_wdtTimeout{5000};  // the core's default before reconfigure
static std::atomic<bool> g_wdtSubscribed{false};
static std::atomic<uint64_t> g_wdtFedAt{0};

void wdtConfigure(uint32_t timeoutMs) {
  g_wdtTimeout = timeoutMs;
}
void wdtSubscribe() {
  g_wdtFedAt = realMonoMs();
  g_wdtSubscribed = true;
}
bool wdtFeed() {
  if (!g_wdtSubscribed) return false;
  g_wdtFedAt = realMonoMs();
  return true;
}

static void watchdogThread() {
  for (;;) {
    sleepRealMs(20);
    if (!g_wdtSubscribed) continue;
    if (realMonoMs() - g_wdtFedAt.load() > g_wdtTimeout.load()) {
      emit("wdt");
      native::reboot(ESP_RST_TASK_WDT);
    }
  }
}

// ── control thread (stdin) ──────────────────────────────────────────────────

static void controlThread() {
  std::string line;
  for (;;) {
    char c;
    ssize_t n = ::read(0, &c, 1);  // one byte at a time: nothing is read ahead and lost at exec
    if (n == 0) quit();             // stdin closed: the test runner is gone
    if (n < 0) {
      if (errno == EINTR) continue;
      quit();
    }
    if (line.empty()) g_lineMutex.lock();
    if (c != '\n') {
      if (c != '\r') line += c;
      if (line.size() > 4096) line.clear();
      continue;
    }
    std::string cmd = line;
    line.clear();
    bool powerCycle = false;
    if (cmd == "quit") quit();
    if (cmd == "power-cycle") {
      powerCycle = true;
    } else if (!cmd.empty()) {
      std::string err = control(cmd);
      if (!err.empty()) emit("ctl-error", "\"line\":" + jsonString(cmd) + ",\"error\":" + jsonString(err));
    }
    g_lineMutex.unlock();
    if (powerCycle) native::reboot(ESP_RST_POWERON);
  }
}

// ── pty ─────────────────────────────────────────────────────────────────────

static void clearCloexec(int fd) {
  int flags = fcntl(fd, F_GETFD);
  if (flags >= 0) fcntl(fd, F_SETFD, flags & ~FD_CLOEXEC);
}

static void openPty() {
  const char *fdEnv = getenv("LINK_NATIVE_PTY_FD");
  if (fdEnv && getenv("LINK_NATIVE_PTY")) {
    g_ptyFd = atoi(fdEnv);
    g_ptyPath = getenv("LINK_NATIVE_PTY");
    const char *hold = getenv("LINK_NATIVE_PTY_HOLD_FD");
    g_ptyHoldFd = hold ? atoi(hold) : -1;
    return;
  }
  int m = posix_openpt(O_RDWR | O_NOCTTY);
  if (m < 0 || grantpt(m) != 0 || unlockpt(m) != 0) {
    perror("native board: pty");
    _exit(71);
  }
  const char *name = ptsname(m);
  if (!name) {
    perror("native board: ptsname");
    _exit(71);
  }
  g_ptyPath = name;
  // The board keeps one slave fd open for its whole life, so the pty stays
  // usable when the host closes and reopens the port (Linux returns EIO on
  // the master while no slave is open). It never reads from it.
  g_ptyHoldFd = open(name, O_RDWR | O_NOCTTY);
  fcntl(m, F_SETFL, fcntl(m, F_GETFL) | O_NONBLOCK);
  g_ptyFd = m;
}

static const char *reasonName(int r) {
  switch (r) {
    case ESP_RST_POWERON: return "power-on";
    case ESP_RST_BROWNOUT: return "brownout";
    case ESP_RST_TASK_WDT: case ESP_RST_INT_WDT: case ESP_RST_WDT: return "watchdog";
    case ESP_RST_PANIC: return "crash";
    case ESP_RST_SW: return "software";
    case ESP_RST_USB: return "usb";
    default: return "other";
  }
}

std::string boardIdFromMac();  // esp_glue.cpp

}  // namespace native

int main(int argc, char **argv) {
  using namespace native;
  signal(SIGPIPE, SIG_IGN);
  g_bootMono = realMonoMs();
  g_argv = argv;
#ifdef __APPLE__
  char path[4096];
  uint32_t size = sizeof path;
  g_self = _NSGetExecutablePath(path, &size) == 0 ? path : argv[0];
#else
  char path[4096];
  ssize_t n = readlink("/proc/self/exe", path, sizeof path - 1);
  g_self = n > 0 ? std::string(path, (size_t)n) : argv[0];
#endif
  if (const char *r = getenv("LINK_NATIVE_RESET")) g_bootReason = atoi(r);

  if (!getenv("LINK_NATIVE_STATE")) {
    const char *tmp = getenv("TMPDIR");
    std::string tpl = std::string(tmp && *tmp ? tmp : "/tmp") + "/link-native-nvs-XXXXXX";
    std::string buf = tpl;
    int fd = mkstemp(&buf[0]);
    if (fd < 0) {
      perror("native board: state file");
      return 71;
    }
    close(fd);
    setenv("LINK_NATIVE_STATE", buf.c_str(), 1);
    setenv("LINK_NATIVE_STATE_TEMP", "1", 1);
  }

  openPty();
  clearCloexec(g_ptyFd);
  if (g_ptyHoldFd >= 0) clearCloexec(g_ptyHoldFd);
  nvsInit();
  serialInit(g_ptyFd);

  // The world the test set up before this boot (nothing is attached yet, so
  // no interrupt fires).
  if (const char *w = getenv("LINK_NATIVE_WORLD")) {
    std::string all = w, cmd;
    for (size_t i = 0; i <= all.size(); i++) {
      if (i == all.size() || all[i] == '\n') {
        if (!cmd.empty()) control(cmd);
        cmd.clear();
      } else {
        cmd += all[i];
      }
    }
  }

  adoptSocketsFromLastBoot(getenv("LINK_NATIVE_SOCKETS"));
  unsetenv("LINK_NATIVE_SOCKETS");

  emit("boot", "\"reason\":" + jsonString(reasonName(g_bootReason)) + ",\"id\":" + jsonString(boardIdFromMac()) +
                   ",\"pty\":" + jsonString(g_ptyPath));

  std::thread(controlThread).detach();
  std::thread(watchdogThread).detach();

  setup();
  for (;;) loop();
}
