// Native board: Arduino `String`, written for this project and backed by
// std::string (design decision 3). The core's WString.cpp (LGPL-2.1) is not
// used or copied. Public signatures follow the arduino-esp32 3.3.12 header
// (check-signatures.sh compares them); behaviour follows the core, including
// the corners the firmware can reach:
//   - length() is `unsigned int`, indexOf() returns int and -1 when not found;
//   - substring(a, b) swaps a > b and clamps b to the length;
//   - searches and comparisons stop at a NUL byte (the core uses strstr/strcmp);
//   - toInt() is atol();
//   - `+= (unsigned char)x` appends the number, `+= (char)x` the character.
#pragma once

#include <stddef.h>
#include <stdint.h>

#include <string>

class String {
 public:
  String(const char *cstr = "");
  String(const char *cstr, unsigned int length);
  String(const uint8_t *cstr, unsigned int length) : String(reinterpret_cast<const char *>(cstr), length) {}
  String(const String &str);
  String(String &&rval);
  explicit String(char c);
  explicit String(unsigned char, unsigned char base = 10);
  explicit String(int, unsigned char base = 10);
  explicit String(unsigned int, unsigned char base = 10);
  explicit String(long, unsigned char base = 10);
  explicit String(unsigned long, unsigned char base = 10);
  explicit String(float, unsigned int decimalPlaces = 2);
  explicit String(double, unsigned int decimalPlaces = 2);
  explicit String(long long, unsigned char base = 10);
  explicit String(unsigned long long, unsigned char base = 10);
  ~String() = default;

  bool reserve(unsigned int size);
  inline unsigned int length(void) const {
    return (unsigned int)s_.size();
  }
  inline void clear(void) {
    s_.clear();
  }
  inline bool isEmpty(void) const {
    return s_.empty();
  }

  String &operator=(const String &rhs);
  String &operator=(const char *cstr);
  String &operator=(String &&rval);

  bool concat(const String &str);
  bool concat(const char *cstr);
  bool concat(const char *cstr, unsigned int length);
  bool concat(char c);
  bool concat(unsigned char num);
  bool concat(int num);
  bool concat(unsigned int num);
  bool concat(long num);
  bool concat(unsigned long num);
  bool concat(long long num);
  bool concat(unsigned long long num);

  String &operator+=(const String &rhs) { concat(rhs); return *this; }
  String &operator+=(const char *cstr) { concat(cstr); return *this; }
  String &operator+=(char c) { concat(c); return *this; }
  String &operator+=(unsigned char num) { concat(num); return *this; }
  String &operator+=(int num) { concat(num); return *this; }
  String &operator+=(unsigned int num) { concat(num); return *this; }
  String &operator+=(long num) { concat(num); return *this; }
  String &operator+=(unsigned long num) { concat(num); return *this; }
  String &operator+=(long long num) { concat(num); return *this; }
  String &operator+=(unsigned long long num) { concat(num); return *this; }

  int compareTo(const String &s) const;
  bool equals(const String &s) const;
  bool equals(const char *cstr) const;
  bool operator==(const String &rhs) const { return equals(rhs); }
  bool operator==(const char *cstr) const { return equals(cstr); }
  bool operator!=(const String &rhs) const { return !equals(rhs); }
  bool operator!=(const char *cstr) const { return !equals(cstr); }

  bool startsWith(const String &prefix) const;
  bool startsWith(const char *prefix) const {
    return this->startsWith(String(prefix));
  }
  bool startsWith(const String &prefix, unsigned int offset) const;
  bool endsWith(const String &suffix) const;
  bool endsWith(const char *suffix) const {
    return this->endsWith(String(suffix));
  }

  char charAt(unsigned int index) const;
  void setCharAt(unsigned int index, char c);
  char operator[](unsigned int index) const;
  char &operator[](unsigned int index);
  const char *c_str() const {
    return s_.c_str();
  }

  int indexOf(char ch) const;
  int indexOf(char ch, unsigned int fromIndex) const;
  int indexOf(const String &str) const;
  int indexOf(const String &str, unsigned int fromIndex) const;
  int lastIndexOf(char ch) const;
  int lastIndexOf(char ch, unsigned int fromIndex) const;
  String substring(unsigned int beginIndex) const {
    return substring(beginIndex, len());
  };
  String substring(unsigned int beginIndex, unsigned int endIndex) const;

  void replace(char find, char replace);
  void replace(const String &find, const String &replace);
  void replace(const char *find, const String &replace) {
    this->replace(String(find), replace);
  }
  void replace(const char *find, const char *replace) {
    this->replace(String(find), String(replace));
  }
  void remove(unsigned int index);
  void remove(unsigned int index, unsigned int count);
  void toLowerCase(void);
  void toUpperCase(void);
  void trim(void);

  long toInt(void) const;

 private:
  unsigned int len() const {
    return (unsigned int)s_.size();
  }
  std::string s_;
};

String operator+(const String &lhs, const String &rhs);
String operator+(const String &lhs, const char *rhs);
String operator+(const char *lhs, const String &rhs);
String operator+(const String &lhs, char c);
String operator+(const String &lhs, unsigned char num);
String operator+(const String &lhs, int num);
String operator+(const String &lhs, unsigned int num);
String operator+(const String &lhs, long num);
String operator+(const String &lhs, unsigned long num);
inline bool operator==(const char *lhs, const String &rhs) { return rhs.equals(lhs); }
inline bool operator!=(const char *lhs, const String &rhs) { return !rhs.equals(lhs); }
