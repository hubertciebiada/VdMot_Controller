// Fake Arduino-ESP32 2.0.7 WString.h: String on top of std::string, with the members the glue,
// the fake libraries and the tests use. Comparisons with a null C string treat it as "".
#pragma once

#include <ctype.h>
#include <stdlib.h>
#include <string.h>

#include <string>

class __FlashStringHelper;

class String {
 public:
  String(const char* cstr = "") : s_(cstr != nullptr ? cstr : "") {}
  String(const char* cstr, unsigned int length)
      : s_(cstr != nullptr ? std::string(cstr, length) : "") {}
  String(const std::string& s) : s_(s) {}
  String(const __FlashStringHelper* str) : String(reinterpret_cast<const char*>(str)) {}
  explicit String(char c) : s_(1, c) {}
  explicit String(unsigned char value, unsigned char base = 10) : s_(number(value, base)) {}
  explicit String(int value, unsigned char base = 10) : s_(number(value, base)) {}
  explicit String(unsigned int value, unsigned char base = 10) : s_(number(value, base)) {}
  explicit String(long value, unsigned char base = 10) : s_(number(value, base)) {}
  explicit String(unsigned long value, unsigned char base = 10) : s_(number(value, base)) {}
  explicit String(long long value, unsigned char base = 10) : s_(number(value, base)) {}
  explicit String(unsigned long long value, unsigned char base = 10) : s_(number(value, base)) {}
  explicit String(double value, unsigned int decimalPlaces = 2) : s_(fixed(value, decimalPlaces)) {}

  String& operator=(const char* cstr) {
    s_ = cstr != nullptr ? cstr : "";
    return *this;
  }

  unsigned int length() const { return static_cast<unsigned int>(s_.size()); }
  bool isEmpty() const { return s_.empty(); }
  const char* c_str() const { return s_.c_str(); }
  const std::string& str() const { return s_; }
  bool reserve(unsigned int size) {
    s_.reserve(size);
    return true;
  }
  char charAt(unsigned int index) const { return index < s_.size() ? s_[index] : '\0'; }
  void setCharAt(unsigned int index, char c) {
    if (index < s_.size()) s_[index] = c;
  }
  char operator[](unsigned int index) const { return charAt(index); }
  char& operator[](unsigned int index) {
    static char dummy;
    if (index >= s_.size()) {
      dummy = '\0';
      return dummy;
    }
    return s_[index];
  }

  bool concat(const String& s) {
    s_ += s.s_;
    return true;
  }
  bool concat(const char* cstr) {
    if (cstr == nullptr) return false;
    s_ += cstr;
    return true;
  }
  bool concat(const char* cstr, unsigned int length) {
    if (cstr == nullptr) return false;
    s_.append(cstr, length);
    return true;
  }
  bool concat(char c) {
    s_ += c;
    return true;
  }
  bool concat(int value) { return concat(String(value)); }
  bool concat(unsigned int value) { return concat(String(value)); }
  bool concat(long value) { return concat(String(value)); }
  bool concat(unsigned long value) { return concat(String(value)); }
  String& operator+=(const String& s) {
    concat(s);
    return *this;
  }
  String& operator+=(const char* cstr) {
    concat(cstr);
    return *this;
  }
  String& operator+=(char c) {
    concat(c);
    return *this;
  }
  String& operator+=(int value) {
    concat(value);
    return *this;
  }
  String& operator+=(unsigned int value) {
    concat(value);
    return *this;
  }
  String& operator+=(long value) {
    concat(value);
    return *this;
  }
  String& operator+=(unsigned long value) {
    concat(value);
    return *this;
  }

  int compareTo(const String& s) const { return s_.compare(s.s_); }
  bool equals(const String& s) const { return s_ == s.s_; }
  bool equals(const char* cstr) const { return s_ == (cstr != nullptr ? cstr : ""); }
  bool equalsIgnoreCase(const String& s) const {
    if (s_.size() != s.s_.size()) return false;
    for (size_t i = 0; i < s_.size(); ++i) {
      const int a = tolower(static_cast<unsigned char>(s_[i]));
      if (a != tolower(static_cast<unsigned char>(s.s_[i]))) return false;
    }
    return true;
  }
  bool operator==(const String& s) const { return equals(s); }
  bool operator==(const char* cstr) const { return equals(cstr); }
  bool operator!=(const String& s) const { return !equals(s); }
  bool operator!=(const char* cstr) const { return !equals(cstr); }
  bool operator<(const String& s) const { return s_ < s.s_; }
  bool operator>(const String& s) const { return s_ > s.s_; }
  bool operator<=(const String& s) const { return s_ <= s.s_; }
  bool operator>=(const String& s) const { return s_ >= s.s_; }

  bool startsWith(const String& prefix) const { return startsWith(prefix, 0); }
  bool startsWith(const String& prefix, unsigned int offset) const {
    return offset <= s_.size() && s_.compare(offset, prefix.s_.size(), prefix.s_) == 0 &&
           s_.size() - offset >= prefix.s_.size();
  }
  bool endsWith(const String& suffix) const {
    return s_.size() >= suffix.s_.size() &&
           s_.compare(s_.size() - suffix.s_.size(), suffix.s_.size(), suffix.s_) == 0;
  }

  int indexOf(char ch, unsigned int fromIndex = 0) const { return pos(s_.find(ch, fromIndex)); }
  int indexOf(const String& s, unsigned int fromIndex = 0) const {
    return pos(s_.find(s.s_, fromIndex));
  }
  int lastIndexOf(char ch) const { return pos(s_.rfind(ch)); }
  int lastIndexOf(const String& s) const { return pos(s_.rfind(s.s_)); }
  String substring(unsigned int beginIndex) const {
    return beginIndex < s_.size() ? String(s_.substr(beginIndex)) : String();
  }
  String substring(unsigned int beginIndex, unsigned int endIndex) const {
    if (beginIndex > endIndex) {
      const unsigned int t = beginIndex;
      beginIndex = endIndex;
      endIndex = t;
    }
    if (beginIndex >= s_.size()) return String();
    if (endIndex > s_.size()) endIndex = static_cast<unsigned int>(s_.size());
    return String(s_.substr(beginIndex, endIndex - beginIndex));
  }

  void replace(char find, char replace) {
    for (char& c : s_) {
      if (c == find) c = replace;
    }
  }
  void replace(const String& find, const String& replace) {
    if (find.s_.empty()) return;
    size_t at = 0;
    while ((at = s_.find(find.s_, at)) != std::string::npos) {
      s_.replace(at, find.s_.size(), replace.s_);
      at += replace.s_.size();
    }
  }
  void remove(unsigned int index) {
    if (index < s_.size()) s_.erase(index);
  }
  void remove(unsigned int index, unsigned int count) {
    if (index < s_.size()) s_.erase(index, count);
  }
  void toLowerCase() {
    for (char& c : s_) c = static_cast<char>(tolower(static_cast<unsigned char>(c)));
  }
  void toUpperCase() {
    for (char& c : s_) c = static_cast<char>(toupper(static_cast<unsigned char>(c)));
  }
  void trim() {
    const size_t first = s_.find_first_not_of(" \t\r\n\f\v");
    if (first == std::string::npos) {
      s_.clear();
      return;
    }
    const size_t last = s_.find_last_not_of(" \t\r\n\f\v");
    s_ = s_.substr(first, last - first + 1);
  }

  long toInt() const { return atol(s_.c_str()); }
  float toFloat() const { return static_cast<float>(atof(s_.c_str())); }
  double toDouble() const { return atof(s_.c_str()); }

 private:
  static int pos(size_t p) { return p == std::string::npos ? -1 : static_cast<int>(p); }
  template <typename T>
  static std::string number(T value, unsigned char base) {
    const bool negative = value < 0;
    unsigned long long v = negative ? 0ull - static_cast<unsigned long long>(value)
                                    : static_cast<unsigned long long>(value);
    if (base < 2 || base > 36) base = 10;
    std::string out;
    do {
      const unsigned d = static_cast<unsigned>(v % base);
      out.insert(out.begin(), static_cast<char>(d < 10 ? '0' + d : 'a' + d - 10));
      v /= base;
    } while (v != 0);
    if (negative) out.insert(out.begin(), '-');
    return out;
  }
  static std::string fixed(double value, unsigned int decimals);

  std::string s_;
};

inline String operator+(const String& a, const String& b) {
  String s(a);
  s += b;
  return s;
}
inline String operator+(const String& a, const char* b) {
  String s(a);
  s += b;
  return s;
}
inline String operator+(const char* a, const String& b) {
  String s(a);
  s += b;
  return s;
}
inline String operator+(const String& a, char b) {
  String s(a);
  s += b;
  return s;
}
inline bool operator==(const char* a, const String& b) { return b == a; }
inline bool operator!=(const char* a, const String& b) { return b != a; }
