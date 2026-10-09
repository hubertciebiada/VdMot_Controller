// Reference of json_body: ArduinoJson 6.21.6 as the VdMot ESP32 firmware 2.1.7 built and called
// it, run over corpus.txt. It prints one outcome line per body; that output is golden.txt, and
// src/json_body/tests_golden.rs formats json_body's outcome the same way.
//
// Build and run in the tools/rust image (the test live_reference_matches_golden does both), from
// the repository root, after tools/rust/cpp217.sh extracted the C++ 2.1.7 tree:
//   g++ -m32 -msse2 -mfpmath=sse -std=gnu++17 -O2 -Wall -Wextra -Werror
//       -I .cache/cpp-2.1.7/test/native/third_party
//       -I software_esp32_rust/glue/tests/json_body/shim
//       software_esp32_rust/glue/tests/json_body/reference.cpp -o /tmp/json_body_reference
//   /tmp/json_body_reference software_esp32_rust/glue/tests/json_body/corpus.txt
//
// -m32: 32-bit pointers as on the ESP32, so a VariantSlot has 16 bytes and the pool of
// StaticJsonDocument<512> holds 32 of them. -msse2 -mfpmath=sse: IEEE double arithmetic as the
// ESP32 computes it in software; x87 code would keep the products of make_float() in 80 bits.
//
// Switches: the firmware's build flags (platformio.ini) set USE_LONG_LONG=1, ENABLE_STD_STRING=0
// and ENABLE_STD_STREAM=0. ARDUINO is defined on the device, which turns on ENABLE_PROGMEM (the
// power-of-ten tables of the number parser are read through pgm_read_*; shim/avr/pgmspace.h has
// the ESP32's plain loads) and the adapters for Arduino String, Stream and Print, which a char*
// input never reaches (left out: they need Arduino.h). Everything else keeps its default on both:
// DECODE_UNICODE=1, COMMENTS=0, NAN=0, INFINITY=0, USE_DOUBLE=1, nesting limit 10, 2-byte slot
// offsets, little endian.
//
// Corpus: one body per line as "name body", the first space ends the name (a line without one
// is an empty body); lines that are empty or start with '#' are skipped, a CR before the LF is
// dropped. In a body, "%HH" is the byte 0xHH and "%{N}" repeats the next unit N times: a byte
// (literal or %HH) or a group "(...)" of them up to the next ')'. Every other character stands
// for itself, so a literal '%' is "%25" (and a space at the end of a line is "%20").
//
// Outcome: "name Error mem=B" or "name Ok mem=B value[ P=probes]", B = memoryUsage().
//   value   null | true | false | u<decimal> | i<decimal> | f<bits> | s<size>"<bytes>"
//           | {"key"@<index of o[key]>=value,...} | [value,...], each followed by
//           <n<isNull> i<is<long long>>:<as<long long>> d<is<double>>:<bits of as<double>>
//            b<is<bool>>:<as<bool>> s<is<const char*>>:<"as<const char*>" or ->
//            o<is<JsonObjectConst>> a<is<JsonArrayConst>> z<size()>>
//   probes  for a root object: the index of o[key] for each key of kProbes, '-' when unbound.
//   Bytes outside [0-9A-Za-z_.-] are written as \xhh; bits are the 16 hex digits of a double.

#define ARDUINOJSON_USE_LONG_LONG 1
#define ARDUINOJSON_ENABLE_STD_STRING 0
#define ARDUINOJSON_ENABLE_STD_STREAM 0
#define ARDUINOJSON_ENABLE_PROGMEM 1
#include <ArduinoJson-v6.21.6.h>

#include <cinttypes>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <string>
#include <vector>

using ArduinoJson::DeserializationError;
using ArduinoJson::JsonArrayConst;
using ArduinoJson::JsonObjectConst;
using ArduinoJson::JsonPairConst;
using ArduinoJson::JsonString;
using ArduinoJson::JsonVariantConst;
using ArduinoJson::StaticJsonDocument;
namespace detail = ArduinoJson::detail;

static_assert(sizeof(void*) == 4, "build with -m32: the ESP32 has 32-bit pointers");
static_assert(sizeof(detail::VariantSlot) == 16, "the ESP32's VariantSlot has 16 bytes");

// The keys the glue looks up (web_server.cpp), the empty key and two of the corpus.
static const char* const kProbes[] = {
    "target", "dir",   "counts", "maxmA",  "slot1", "slot2",  "motor",        "learnMovements",
    "breakaway", "lowC", "highC", "startOnPower", "noOfMinCount", "maxCalReps", "enable",
    "stepPct", "confirm", "image", "mode", "force", "board", "action", "valve", "value", "",
    "a", "k1"};

static int hexValue(char c) {
  if (c >= '0' && c <= '9') return c - '0';
  if (c >= 'a' && c <= 'f') return c - 'a' + 10;
  if (c >= 'A' && c <= 'F') return c - 'A' + 10;
  return -1;
}

// One unit at `i`: a literal byte or %HH. False on a malformed escape.
static bool unit(const std::string& in, size_t& i, std::string& out) {
  if (in[i] != '%') {
    out += in[i++];
    return true;
  }
  if (i + 2 >= in.size() || hexValue(in[i + 1]) < 0 || hexValue(in[i + 2]) < 0) return false;
  out += static_cast<char>(hexValue(in[i + 1]) * 16 + hexValue(in[i + 2]));
  i += 3;
  return true;
}

static bool decodeBody(const std::string& in, std::string& out) {
  size_t i = 0;
  while (i < in.size()) {
    if (in.compare(i, 2, "%{") != 0) {
      if (!unit(in, i, out)) return false;
      continue;
    }
    const size_t close = in.find('}', i);
    if (close == std::string::npos || close == i + 2) return false;
    size_t count = 0;
    for (size_t k = i + 2; k < close; ++k) {
      if (in[k] < '0' || in[k] > '9') return false;
      count = count * 10 + static_cast<size_t>(in[k] - '0');
    }
    i = close + 1;
    if (i >= in.size()) return false;
    std::string group;
    if (in[i] == '(') {
      const size_t end = in.find(')', i);
      if (end == std::string::npos) return false;
      size_t k = i + 1;
      while (k < end) {
        if (!unit(in, k, group) || k > end) return false;
      }
      i = end + 1;
    } else if (!unit(in, i, group)) {
      return false;
    }
    for (size_t n = 0; n < count; ++n) out += group;
  }
  return true;
}

static void putEscaped(std::string& out, const char* p, size_t n) {
  static const char hex[] = "0123456789abcdef";
  for (size_t i = 0; i < n; ++i) {
    const unsigned char c = static_cast<unsigned char>(p[i]);
    if ((c >= '0' && c <= '9') || (c >= 'A' && c <= 'Z') || (c >= 'a' && c <= 'z') || c == '_' ||
        c == '.' || c == '-') {
      out += static_cast<char>(c);
    } else {
      out += "\\x";
      out += hex[c >> 4];
      out += hex[c & 15];
    }
  }
}

static void putBits(std::string& out, double d) {
  uint64_t bits;
  memcpy(&bits, &d, sizeof bits);
  char b[24];
  snprintf(b, sizeof b, "%016" PRIx64, bits);
  out += b;
}

static const detail::VariantData* dataOf(JsonVariantConst v) {
  return detail::VariantAttorney::getData(v);
}

// Index of the member whose value `found` is, -1 when unbound.
static int indexOf(JsonObjectConst o, JsonVariantConst found) {
  int i = 0;
  for (JsonPairConst kv : o) {
    if (dataOf(kv.value()) == dataOf(found) && dataOf(found) != nullptr) return i;
    ++i;
  }
  return -1;
}

static void putIndex(std::string& out, int index) {
  if (index < 0) {
    out += '-';
  } else {
    out += std::to_string(index);
  }
}

static void putQueries(std::string& out, JsonVariantConst v) {
  out += "<n";
  out += v.isNull() ? '1' : '0';
  out += " i";
  out += v.is<long long>() ? '1' : '0';
  out += ':';
  out += std::to_string(v.as<long long>());
  out += " d";
  out += v.is<double>() ? '1' : '0';
  out += ':';
  putBits(out, v.as<double>());
  out += " b";
  out += v.is<bool>() ? '1' : '0';
  out += ':';
  out += v.as<bool>() ? '1' : '0';
  out += " s";
  out += v.is<const char*>() ? '1' : '0';
  out += ':';
  const char* s = v.as<const char*>();
  if (s == nullptr) {
    out += '-';
  } else {
    out += '"';
    putEscaped(out, s, strlen(s));
    out += '"';
  }
  out += " o";
  out += v.is<JsonObjectConst>() ? '1' : '0';
  out += " a";
  out += v.is<JsonArrayConst>() ? '1' : '0';
  out += " z";
  out += std::to_string(v.size());
  out += '>';
}

static void putValue(std::string& out, JsonVariantConst v) {
  const detail::VariantData* data = dataOf(v);
  const unsigned type = data ? data->type() : unsigned(detail::VALUE_IS_NULL);
  switch (type) {
    case detail::VALUE_IS_NULL:
      out += "null";
      break;
    case detail::VALUE_IS_BOOLEAN:
      out += v.as<bool>() ? "true" : "false";
      break;
    case detail::VALUE_IS_UNSIGNED_INTEGER:
      out += 'u';
      out += std::to_string(v.as<unsigned long long>());
      break;
    case detail::VALUE_IS_SIGNED_INTEGER:
      out += 'i';
      out += std::to_string(v.as<long long>());
      break;
    case detail::VALUE_IS_FLOAT:
      out += 'f';
      putBits(out, v.as<double>());
      break;
    case detail::VALUE_IS_LINKED_STRING:
    case detail::VALUE_IS_OWNED_STRING: {
      const JsonString js = v.as<JsonString>();
      out += type == detail::VALUE_IS_LINKED_STRING ? 's' : 'S';  // zero-copy: always linked
      out += std::to_string(js.size());
      out += '"';
      putEscaped(out, js.c_str(), js.size());
      out += '"';
      break;
    }
    case detail::VALUE_IS_OBJECT: {
      const JsonObjectConst o = v.as<JsonObjectConst>();
      out += '{';
      bool first = true;
      for (JsonPairConst kv : o) {
        if (!first) out += ',';
        first = false;
        const char* key = kv.key().c_str();
        out += '"';
        putEscaped(out, key, strlen(key));
        out += "\"@";
        putIndex(out, indexOf(o, o[key]));
        out += '=';
        putValue(out, kv.value());
      }
      out += '}';
      break;
    }
    case detail::VALUE_IS_ARRAY: {
      out += '[';
      bool first = true;
      for (JsonVariantConst e : v.as<JsonArrayConst>()) {
        if (!first) out += ',';
        first = false;
        putValue(out, e);
      }
      out += ']';
      break;
    }
    default:
      out += '?';
      out += std::to_string(type);
      break;
  }
  putQueries(out, v);
}

// parseBody() of web_server.cpp: the body in a buffer of its length plus a terminator, the
// document on the heap, cleared, then deserializeJson(doc, char*, size_t).
static void outcome(const std::string& body, std::string& out) {
  std::vector<char> buf(body.begin(), body.end());
  buf.push_back('\0');
  StaticJsonDocument<512>* doc = new StaticJsonDocument<512>();
  doc->clear();
  const DeserializationError e = deserializeJson(*doc, buf.data(), body.size());
  out += e.c_str();
  out += " mem=";
  out += std::to_string(doc->memoryUsage());
  if (!e) {
    out += ' ';
    const JsonVariantConst root = doc->as<JsonVariantConst>();
    putValue(out, root);
    if (root.is<JsonObjectConst>()) {
      const JsonObjectConst o = root.as<JsonObjectConst>();
      out += " P=";
      for (size_t k = 0; k < sizeof kProbes / sizeof kProbes[0]; ++k) {
        if (k) out += ',';
        putIndex(out, indexOf(o, o[kProbes[k]]));
      }
    }
  }
  delete doc;
}

int main(int argc, char** argv) {
  if (argc != 2) {
    fprintf(stderr, "usage: %s corpus.txt\n", argv[0]);
    return 2;
  }
  FILE* f = fopen(argv[1], "rb");
  if (f == nullptr) {
    perror(argv[1]);
    return 2;
  }
  char* line = nullptr;
  size_t cap = 0;
  ssize_t n;
  unsigned lineNo = 0;
  int rc = 0;
  while ((n = getline(&line, &cap, f)) >= 0) {
    ++lineNo;
    std::string l(line, static_cast<size_t>(n));
    if (!l.empty() && l.back() == '\n') l.pop_back();
    if (!l.empty() && l.back() == '\r') l.pop_back();
    if (l.empty() || l[0] == '#') continue;
    const size_t space = l.find(' ');
    std::string body;
    if (space != std::string::npos && !decodeBody(l.substr(space + 1), body)) {
      fprintf(stderr, "%s:%u: malformed corpus line\n", argv[1], lineNo);
      rc = 2;
      break;
    }
    std::string out = l.substr(0, space);
    out += ' ';
    outcome(body, out);
    out += '\n';
    fputs(out.c_str(), stdout);
  }
  free(line);
  fclose(f);
  return rc;
}
