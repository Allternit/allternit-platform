// Native board: Preferences.h over a file (LINK_NATIVE_STATE).
//
// Real NVS limits are enforced, not relaxed: a key or namespace longer than
// 15 characters fails as on the board (begin() false, get returns the
// default, put returns 0) and emits an `nvs-violation` event. A read-only
// begin() on a namespace that was never opened read-write fails, as
// nvs_open(NVS_READONLY) does. Keys are typed: a get with another type finds
// nothing. Every put and remove is written to the file at once.
#pragma once

#include <Arduino.h>

class Preferences {
 public:
  bool begin(const char *name, bool readOnly = false, const char *partition_label = NULL);
  void end();
  bool clear();
  bool remove(const char *key);
  size_t putUShort(const char *key, uint16_t value);
  size_t putBool(const char *key, bool value);
  size_t putString(const char *key, const char *value);
  size_t putString(const char *key, String value);
  size_t putBytes(const char *key, const void *value, size_t len);
  uint16_t getUShort(const char *key, uint16_t defaultValue = 0);
  bool getBool(const char *key, bool defaultValue = false);
  String getString(const char *key, String defaultValue = String());
  size_t getBytes(const char *key, void *buf, size_t maxLen);

 private:
  bool started_ = false;
  bool readOnly_ = false;
  std::string ns_;
};
