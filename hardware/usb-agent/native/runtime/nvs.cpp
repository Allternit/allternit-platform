// Native board: Preferences over a file (LINK_NATIVE_STATE), with the NVS
// rules a board enforces (see shim/Preferences.h).
//
// File format, one entry per line, written whole and renamed on every change:
//   ns <namespace>
//   kv <namespace> <key> <type> <hex value>
// Types: u8, u16, str, blob.
#include <Arduino.h>
#include <Preferences.h>
#include <native_runtime.h>
#include <unistd.h>

#include <fstream>
#include <map>
#include <mutex>
#include <set>
#include <sstream>
#include <string>

namespace {

const size_t NVS_NAME_MAX = 15;  // NVS_KEY_NAME_MAX_SIZE (16) minus the NUL

struct Item {
  std::string type;
  std::string value;  // raw bytes
};

std::mutex g_nvs;
std::set<std::string> g_namespaces;
std::map<std::string, std::map<std::string, Item>> g_items;
std::string g_path;

std::string unhex(const std::string &h) {
  std::string out;
  for (size_t i = 0; i + 1 < h.size(); i += 2) out += (char)strtol(h.substr(i, 2).c_str(), nullptr, 16);
  return out;
}

void save() {
  if (g_path.empty()) return;
  std::ostringstream o;
  for (const auto &ns : g_namespaces) o << "ns " << ns << "\n";
  for (const auto &ns : g_items) {
    for (const auto &kv : ns.second) {
      o << "kv " << ns.first << " " << kv.first << " " << kv.second.type << " "
        << native::hex((const uint8_t *)kv.second.value.data(), kv.second.value.size()) << "\n";
    }
  }
  std::string tmp = g_path + ".tmp." + std::to_string(getpid());
  {
    std::ofstream f(tmp, std::ios::binary | std::ios::trunc);
    f << o.str();
  }
  rename(tmp.c_str(), g_path.c_str());
}

// A name the NVS API would refuse. Reports it and returns true.
bool violates(const char *op, const std::string &ns, const char *key) {
  bool bad = false;
  std::string fields = "\"op\":" + native::jsonString(op) + ",\"ns\":" + native::jsonString(ns);
  if (ns.size() > NVS_NAME_MAX) bad = true;
  if (key) {
    fields += ",\"key\":" + native::jsonString(key);
    if (strlen(key) > NVS_NAME_MAX) bad = true;
  }
  if (bad) native::emit("nvs-violation", fields);
  return bad;
}

}  // namespace

namespace native {
void nvsInit() {
  std::lock_guard<std::mutex> lock(g_nvs);
  const char *p = getenv("LINK_NATIVE_STATE");
  g_path = p ? p : "";
  std::ifstream f(g_path);
  std::string line;
  while (std::getline(f, line)) {
    std::istringstream in(line);
    std::string tag, ns, key, type, value;
    in >> tag >> ns;
    if (tag == "ns") {
      g_namespaces.insert(ns);
    } else if (tag == "kv") {
      in >> key >> type >> value;
      g_items[ns][key] = {type, unhex(value)};
    }
  }
}
}  // namespace native

bool Preferences::begin(const char *name, bool readOnly, const char *) {
  if (started_ || !name) return false;
  if (violates("begin", name, nullptr)) return false;
  std::lock_guard<std::mutex> lock(g_nvs);
  if (readOnly && !g_namespaces.count(name)) return false;  // nvs_open(NVS_READONLY): ESP_ERR_NVS_NOT_FOUND
  if (!readOnly && g_namespaces.insert(name).second) save();
  ns_ = name;
  readOnly_ = readOnly;
  started_ = true;
  return true;
}

void Preferences::end() {
  started_ = false;
}

bool Preferences::clear() {
  if (!started_ || readOnly_) return false;
  std::lock_guard<std::mutex> lock(g_nvs);
  g_items.erase(ns_);
  save();
  return true;
}

bool Preferences::remove(const char *key) {
  if (!started_ || !key || readOnly_) return false;
  if (violates("remove", ns_, key)) return false;
  std::lock_guard<std::mutex> lock(g_nvs);
  auto ns = g_items.find(ns_);
  if (ns == g_items.end() || !ns->second.erase(key)) return false;  // ESP_ERR_NVS_NOT_FOUND
  save();
  return true;
}

static bool put(bool started, bool readOnly, const std::string &ns, const char *op, const char *key, const char *type,
                const std::string &value) {
  if (!started || !key || readOnly) return false;
  if (violates(op, ns, key)) return false;
  std::lock_guard<std::mutex> lock(g_nvs);
  g_items[ns][key] = {type, value};  // a new type replaces the old entry
  save();
  return true;
}

// Returns false when absent or stored with another type.
static bool get(bool started, const std::string &ns, const char *op, const char *key, const char *type, std::string *out) {
  if (!started || !key) return false;
  if (violates(op, ns, key)) return false;
  std::lock_guard<std::mutex> lock(g_nvs);
  auto n = g_items.find(ns);
  if (n == g_items.end()) return false;
  auto it = n->second.find(key);
  if (it == n->second.end() || it->second.type != type) return false;
  *out = it->second.value;
  return true;
}

size_t Preferences::putUShort(const char *key, uint16_t value) {
  std::string v((const char *)&value, 2);
  return put(started_, readOnly_, ns_, "putUShort", key, "u16", v) ? 2 : 0;
}

size_t Preferences::putBool(const char *key, bool value) {
  std::string v(1, (char)(value ? 1 : 0));
  return put(started_, readOnly_, ns_, "putBool", key, "u8", v) ? 1 : 0;
}

size_t Preferences::putString(const char *key, const char *value) {
  if (!value) return 0;
  return put(started_, readOnly_, ns_, "putString", key, "str", value) ? strlen(value) : 0;
}

size_t Preferences::putString(const char *key, String value) {
  return putString(key, value.c_str());
}

size_t Preferences::putBytes(const char *key, const void *value, size_t len) {
  if (!value || !len) return 0;
  return put(started_, readOnly_, ns_, "putBytes", key, "blob", std::string((const char *)value, len)) ? len : 0;
}

uint16_t Preferences::getUShort(const char *key, uint16_t defaultValue) {
  std::string v;
  if (!get(started_, ns_, "getUShort", key, "u16", &v) || v.size() != 2) return defaultValue;
  uint16_t out;
  memcpy(&out, v.data(), 2);
  return out;
}

bool Preferences::getBool(const char *key, bool defaultValue) {
  std::string v;
  uint8_t u = defaultValue ? 1 : 0;
  if (get(started_, ns_, "getBool", key, "u8", &v) && v.size() == 1) u = (uint8_t)v[0];
  return u == 1;  // the core: getUChar(key, default ? 1 : 0) == 1
}

String Preferences::getString(const char *key, String defaultValue) {
  std::string v;
  if (!get(started_, ns_, "getString", key, "str", &v)) return defaultValue;
  return String(v.c_str());
}

size_t Preferences::getBytes(const char *key, void *buf, size_t maxLen) {
  std::string v;
  if (!get(started_, ns_, "getBytes", key, "blob", &v)) return 0;
  if (!buf || !maxLen) return v.size();
  if (v.size() > maxLen) return 0;  // the core: "not enough space in buffer"
  memcpy(buf, v.data(), v.size());
  return v.size();
}
