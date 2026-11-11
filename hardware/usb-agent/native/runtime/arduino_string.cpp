// Native board: String, IPAddress and Print, written for this project
// (decision 3). No core source was copied; behaviour is matched where the
// firmware can see it (see the notes in shim/WString.h and shim/IPAddress.h).
#include <Arduino.h>
#include <arpa/inet.h>

#include <algorithm>

// ── String ──────────────────────────────────────────────────────────────────

namespace {

// Digits of `v` in `base` (2 to 36), lowercase, as the core's utoa/ultoa.
std::string digits(unsigned long long v, unsigned base) {
  if (base < 2 || base > 36) base = 10;
  char buf[72];
  int i = sizeof buf;
  buf[--i] = '\0';
  do {
    unsigned d = (unsigned)(v % base);
    buf[--i] = (char)(d < 10 ? '0' + d : 'a' + d - 10);
    v /= base;
  } while (v);
  return std::string(buf + i);
}

// The core's itoa/ltoa: a minus sign only in base 10; other bases print the
// two's complement bits of the same width.
template <typename S, typename U>
std::string signedDigits(S v, unsigned base) {
  if (base == 10 && v < 0) return "-" + digits((unsigned long long)(-(long long)v), 10);
  return digits((unsigned long long)(U)v, base);
}

std::string fixed(double v, unsigned places) {
  char buf[64];
  snprintf(buf, sizeof buf, "%*.*f", (int)(places + 2), (int)places, v);
  return buf;
}

}  // namespace

String::String(const char *cstr) {
  if (cstr) s_ = cstr;
}
String::String(const char *cstr, unsigned int length) {
  if (cstr) s_.assign(cstr, length);
}
String::String(const String &str) : s_(str.s_) {}
String::String(String &&rval) : s_(std::move(rval.s_)) {}
String::String(char c) : s_(1, c) {}
String::String(unsigned char v, unsigned char base) : s_(digits(v, base)) {}
String::String(int v, unsigned char base) : s_(signedDigits<int, unsigned int>(v, base)) {}
String::String(unsigned int v, unsigned char base) : s_(digits(v, base)) {}
String::String(long v, unsigned char base) : s_(signedDigits<long, unsigned long>(v, base)) {}
String::String(unsigned long v, unsigned char base) : s_(digits(v, base)) {}
String::String(float v, unsigned int decimalPlaces) : s_(fixed(v, decimalPlaces)) {}
String::String(double v, unsigned int decimalPlaces) : s_(fixed(v, decimalPlaces)) {}
String::String(long long v, unsigned char base) : s_(signedDigits<long long, unsigned long long>(v, base)) {}
String::String(unsigned long long v, unsigned char base) : s_(digits(v, base)) {}

bool String::reserve(unsigned int size) {
  s_.reserve(size);
  return true;
}

String &String::operator=(const String &rhs) {
  if (this != &rhs) s_ = rhs.s_;
  return *this;
}
String &String::operator=(const char *cstr) {
  if (cstr) s_ = cstr;
  else s_.clear();
  return *this;
}
String &String::operator=(String &&rval) {
  if (this != &rval) s_ = std::move(rval.s_);
  return *this;
}

bool String::concat(const String &str) {
  s_.append(str.s_);
  return true;
}
bool String::concat(const char *cstr) {
  if (!cstr) return false;
  s_.append(cstr);
  return true;
}
bool String::concat(const char *cstr, unsigned int length) {
  if (!cstr) return false;
  s_.append(cstr, length);
  return true;
}
bool String::concat(char c) {
  s_.push_back(c);
  return true;
}
bool String::concat(unsigned char num) { s_ += digits(num, 10); return true; }
bool String::concat(int num) { s_ += signedDigits<int, unsigned int>(num, 10); return true; }
bool String::concat(unsigned int num) { s_ += digits(num, 10); return true; }
bool String::concat(long num) { s_ += signedDigits<long, unsigned long>(num, 10); return true; }
bool String::concat(unsigned long num) { s_ += digits(num, 10); return true; }
bool String::concat(long long num) { s_ += signedDigits<long long, unsigned long long>(num, 10); return true; }
bool String::concat(unsigned long long num) { s_ += digits(num, 10); return true; }

// The core compares with strcmp: a NUL byte ends the comparison.
int String::compareTo(const String &s) const {
  return strcmp(s_.c_str(), s.s_.c_str());
}
bool String::equals(const String &s) const {
  return len() == s.len() && compareTo(s) == 0;
}
bool String::equals(const char *cstr) const {
  if (len() == 0) return cstr == nullptr || *cstr == 0;
  if (cstr == nullptr) return s_.c_str()[0] == 0;
  return strcmp(s_.c_str(), cstr) == 0;
}

bool String::startsWith(const String &prefix) const {
  if (len() < prefix.len()) return false;
  return startsWith(prefix, 0);
}
bool String::startsWith(const String &prefix, unsigned int offset) const {
  if (offset > len() - prefix.len()) return false;  // unsigned, as in the core
  return strncmp(s_.c_str() + offset, prefix.s_.c_str(), prefix.len()) == 0;
}
bool String::endsWith(const String &suffix) const {
  if (len() < suffix.len()) return false;
  return strcmp(s_.c_str() + len() - suffix.len(), suffix.s_.c_str()) == 0;
}

char String::charAt(unsigned int index) const {
  return operator[](index);
}
void String::setCharAt(unsigned int index, char c) {
  if (index < len()) s_[index] = c;
}
char String::operator[](unsigned int index) const {
  if (index >= len()) return 0;
  return s_[index];
}
char &String::operator[](unsigned int index) {
  static char dummy;
  if (index >= len()) {
    dummy = 0;
    return dummy;
  }
  return s_[index];
}

int String::indexOf(char ch) const {
  return indexOf(ch, 0);
}
int String::indexOf(char ch, unsigned int fromIndex) const {
  if (fromIndex >= len()) return -1;
  const char *p = strchr(s_.c_str() + fromIndex, ch);
  return p ? (int)(p - s_.c_str()) : -1;
}
int String::indexOf(const String &str) const {
  return indexOf(str, 0);
}
int String::indexOf(const String &str, unsigned int fromIndex) const {
  if (fromIndex >= len()) return -1;
  const char *p = strstr(s_.c_str() + fromIndex, str.s_.c_str());
  return p ? (int)(p - s_.c_str()) : -1;
}
int String::lastIndexOf(char ch) const {
  return lastIndexOf(ch, len() - 1);
}
int String::lastIndexOf(char ch, unsigned int fromIndex) const {
  if (fromIndex >= len()) return -1;
  // The core cuts the text after fromIndex and uses strrchr.
  std::string head = s_.substr(0, fromIndex + 1);
  const char *p = strrchr(head.c_str(), ch);
  return p ? (int)(p - head.c_str()) : -1;
}

String String::substring(unsigned int left, unsigned int right) const {
  if (left > right) std::swap(left, right);
  String out;
  if (left >= len()) return out;
  if (right > len()) right = len();
  out.s_ = s_.substr(left, right - left);
  return out;
}

void String::replace(char find, char replace) {
  for (auto &c : s_)
    if (c == find) c = replace;
}
void String::replace(const String &find, const String &replace) {
  if (len() == 0 || find.len() == 0) return;
  std::string out;
  size_t at = 0;
  while (true) {
    // strstr semantics: the search stops at a NUL in this string.
    const char *base = s_.c_str();
    const char *p = at <= s_.size() ? strstr(base + at, find.s_.c_str()) : nullptr;
    if (!p) break;
    size_t i = p - base;
    out.append(s_, at, i - at);
    out.append(replace.s_);
    at = i + find.len();
  }
  if (at == 0) return;
  out.append(s_, at, std::string::npos);
  s_ = out;
}
void String::remove(unsigned int index) {
  remove(index, (unsigned int)-1);
}
void String::remove(unsigned int index, unsigned int count) {
  if (index >= len()) return;
  if (count <= 0) return;
  if (count > len() - index) count = len() - index;
  s_.erase(index, count);
}
void String::toLowerCase(void) {
  for (auto &c : s_) c = (char)tolower((unsigned char)c);
}
void String::toUpperCase(void) {
  for (auto &c : s_) c = (char)toupper((unsigned char)c);
}
void String::trim(void) {
  if (s_.empty()) return;
  // The core trims at the C string (a NUL ends it) with isspace().
  const char *b = s_.c_str();
  size_t n = strlen(b);
  size_t begin = 0;
  while (begin < n && isspace((unsigned char)b[begin])) begin++;
  size_t end = n;
  while (end > begin && isspace((unsigned char)b[end - 1])) end--;
  s_ = std::string(b + begin, end - begin);
}
long String::toInt(void) const {
  return atol(s_.c_str());
}

String operator+(const String &lhs, const String &rhs) { String s(lhs); s.concat(rhs); return s; }
String operator+(const String &lhs, const char *rhs) { String s(lhs); s.concat(rhs); return s; }
String operator+(const char *lhs, const String &rhs) { String s(lhs); s.concat(rhs); return s; }
String operator+(const String &lhs, char c) { String s(lhs); s.concat(c); return s; }
String operator+(const String &lhs, unsigned char num) { String s(lhs); s.concat(num); return s; }
String operator+(const String &lhs, int num) { String s(lhs); s.concat(num); return s; }
String operator+(const String &lhs, unsigned int num) { String s(lhs); s.concat(num); return s; }
String operator+(const String &lhs, long num) { String s(lhs); s.concat(num); return s; }
String operator+(const String &lhs, unsigned long num) { String s(lhs); s.concat(num); return s; }

// ── IPAddress ───────────────────────────────────────────────────────────────

IPAddress::IPAddress() {}
IPAddress::IPAddress(uint8_t a, uint8_t b, uint8_t c, uint8_t d) {
  _bytes[12] = a;
  _bytes[13] = b;
  _bytes[14] = c;
  _bytes[15] = d;
}
IPAddress::IPAddress(uint32_t address) {
  memcpy(_bytes + 12, &address, 4);  // network order in memory, as lwIP keeps it
}
IPAddress::IPAddress(const char *address) {
  fromString(address);
}
IPAddress::IPAddress(const IPAddress &address) {
  *this = address;
}
IPAddress &IPAddress::operator=(const IPAddress &address) {
  memcpy(_bytes, address._bytes, sizeof _bytes);
  _type = address._type;
  return *this;
}
bool IPAddress::operator==(const IPAddress &addr) const {
  return _type == addr._type && memcmp(_bytes, addr._bytes, sizeof _bytes) == 0;
}

bool IPAddress::fromString(const char *address) {
  if (!fromString4(address)) return fromString6(address);
  return true;
}

// Exactly three dots, each part 1+ digits worth 0 to 255; nothing else.
bool IPAddress::fromString4(const char *address) {
  int acc = -1;
  int dots = 0;
  uint8_t out[4] = {0};
  for (const char *p = address; *p; p++) {
    char c = *p;
    if (c >= '0' && c <= '9') {
      acc = acc < 0 ? c - '0' : acc * 10 + (c - '0');
      if (acc > 255) return false;
    } else if (c == '.') {
      if (dots == 3 || acc < 0) return false;
      out[dots++] = (uint8_t)acc;
      acc = -1;
    } else {
      return false;
    }
  }
  if (dots != 3 || acc < 0) return false;
  out[3] = (uint8_t)acc;
  memset(_bytes, 0, sizeof _bytes);
  memcpy(_bytes + 12, out, 4);
  _type = IPv4;
  return true;
}

bool IPAddress::fromString6(const char *address) {
  uint8_t out[16];
  if (inet_pton(AF_INET6, address, out) != 1) return false;
  memcpy(_bytes, out, 16);
  _type = IPv6;
  return true;
}

String IPAddress::toString(bool) const {
  char buf[INET6_ADDRSTRLEN];
  if (_type == IPv4) {
    snprintf(buf, sizeof buf, "%u.%u.%u.%u", _bytes[12], _bytes[13], _bytes[14], _bytes[15]);
  } else {
    inet_ntop(AF_INET6, _bytes, buf, sizeof buf);
  }
  return String(buf);
}

// ── Print and Stream ────────────────────────────────────────────────────────

size_t Print::write(const uint8_t *buffer, size_t size) {
  size_t n = 0;
  while (size--) {
    if (!write(*buffer++)) break;
    n++;
  }
  return n;
}
size_t Print::printf(const char *format, ...) {
  va_list ap;
  va_start(ap, format);
  va_list copy;
  va_copy(copy, ap);
  int len = vsnprintf(nullptr, 0, format, copy);
  va_end(copy);
  if (len < 0) {
    va_end(ap);
    return 0;
  }
  std::string buf((size_t)len + 1, '\0');
  vsnprintf(&buf[0], buf.size(), format, ap);
  va_end(ap);
  return write((const uint8_t *)buf.data(), (size_t)len);
}
size_t Print::print(const String &s) {
  return write((const uint8_t *)s.c_str(), s.length());
}
size_t Print::print(const char str[]) {
  return write((const uint8_t *)str, strlen(str));
}
size_t Print::print(char c) {
  return write((uint8_t)c);
}
size_t Print::println(void) {
  return print("\r\n");
}
size_t Print::println(const String &s) {
  return print(s) + println();
}
size_t Print::println(const char c[]) {
  return print(c) + println();
}

void Stream::setTimeout(unsigned long timeout) {
  _timeout = timeout;
}
