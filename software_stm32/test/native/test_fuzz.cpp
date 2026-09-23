// Random-input tests with fixed seeds: every run exercises the same inputs,
// so failures are reproducible. ASan/UBSan catch out-of-bounds accesses.
#include <errno.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#include <random>
#include <string>
#include <vector>

#include "doctest.h"
#include "vdm/arg_parser.h"
#include "vdm/buf_writer.h"
#include "vdm/line_assembler.h"
#include "vdm/replies.h"
#include "vdm/tokenizer.h"

namespace {

std::string randomBytes(std::mt19937& rng, size_t maxLen) {
  std::uniform_int_distribution<size_t> lenDist(0, maxLen);
  std::uniform_int_distribution<int> byteDist(0, 255);
  std::string s(lenDist(rng), '\0');
  for (char& c : s) c = static_cast<char>(byteDist(rng));
  return s;
}

// Mostly protocol-like text with occasional junk.
std::string randomProtocolText(std::mt19937& rng, size_t maxLen) {
  static const char kAlphabet[] = "stgvlpxdn0123456789 -\t\r\n\x01\xff";
  std::uniform_int_distribution<size_t> lenDist(0, maxLen);
  std::uniform_int_distribution<size_t> pick(0, sizeof(kAlphabet) - 2);
  std::string s(lenDist(rng), '\0');
  for (char& c : s) c = kAlphabet[pick(rng)];
  return s;
}

bool isLineChar(char c) {
  const unsigned char u = static_cast<unsigned char>(c);
  return c == '\t' || (u >= 0x20 && u < 0x7F);
}

// Reference line splitter used to cross-check the assembler.
struct Reference {
  std::vector<std::string> lines;
  uint32_t overflows = 0;
  uint32_t malformed = 0;
};

Reference referenceSplit(const std::string& input, size_t capacity) {
  Reference ref;
  std::string cur;
  bool bad = false;
  bool over = false;
  for (char c : input) {
    if (c == '\r' || c == '\n') {
      if (!bad && !over && !cur.empty()) ref.lines.push_back(cur);
      cur.clear();
      bad = over = false;
      continue;
    }
    if (bad || over) continue;
    if (!isLineChar(c)) {
      bad = true;
      ++ref.malformed;
    } else if (cur.size() + 1 >= capacity) {
      over = true;
      ++ref.overflows;
    } else {
      cur += c;
    }
  }
  return ref;
}

}  // namespace

TEST_CASE("fuzz: LineAssembler matches a reference splitter on random bytes") {
  std::mt19937 rng(0x5eed1234u);
  for (int round = 0; round < 2000; ++round) {
    const size_t capacity = 2 + round % 40;
    const std::string input = (round % 2) ? randomBytes(rng, 300) : randomProtocolText(rng, 300);
    std::vector<char> storage(capacity + 1, '#');  // sentinel after the buffer
    vdm::LineAssembler la(storage.data(), capacity);

    std::vector<std::string> lines;
    size_t pos = 0;
    std::uniform_int_distribution<size_t> chunk(1, 17);
    while (pos < input.size()) {
      const size_t n = std::min(chunk(rng), input.size() - pos);
      const size_t used = la.feed(input.data() + pos, n);
      REQUIRE(used <= n);
      pos += used;
      REQUIRE(la.length() < capacity);
      REQUIRE(strlen(la.line()) == la.length());
      if (la.hasLine()) {
        REQUIRE(la.length() > 0);
        lines.emplace_back(la.line());
        la.release();
      }
    }
    REQUIRE(storage[capacity] == '#');

    const Reference ref = referenceSplit(input, capacity);
    CHECK(lines == ref.lines);
    CHECK(la.overflowCount() == ref.overflows);
    CHECK(la.malformedCount() == ref.malformed);
    for (const std::string& l : lines) {
      for (char c : l) REQUIRE(isLineChar(c));
    }
  }
}

TEST_CASE("fuzz: Tokenizer invariants on random lines") {
  std::mt19937 rng(0xC0FFEEu);
  for (int round = 0; round < 5000; ++round) {
    std::string text = randomProtocolText(rng, 80);
    for (char& c : text) {
      if (c == '\0') c = ' ';
    }
    std::vector<char> line(text.begin(), text.end());
    line.push_back('\0');
    const char* begin = line.data();
    const char* end = line.data() + line.size();
    const uint8_t maxArgs = static_cast<uint8_t>(round % 10);

    vdm::Tokenizer t;
    const bool ok = t.parse(line.data(), maxArgs);
    const uint8_t limit = maxArgs < vdm::Tokenizer::kMaxArgs ? maxArgs : vdm::Tokenizer::kMaxArgs;
    REQUIRE(t.argc() <= limit);
    CHECK(ok == (!t.tooManyArgs() && t.command()[0] != '\0'));

    // Count tokens independently.
    size_t tokens = 0;
    bool inToken = false;
    for (char c : text) {
      const bool sep = (c == ' ' || c == '\t');
      if (!sep && !inToken) ++tokens;
      inToken = !sep;
    }
    if (tokens == 0) {
      CHECK_FALSE(ok);
      CHECK(t.argc() == 0);
      continue;
    }
    CHECK(t.tooManyArgs() == (tokens - 1 > limit));
    CHECK(t.argc() == std::min<size_t>(tokens - 1, limit));

    const char* all[1 + vdm::Tokenizer::kMaxArgs];
    all[0] = t.command();
    for (uint8_t i = 0; i < t.argc(); ++i) all[i + 1] = t.arg(i);
    for (uint8_t i = 0; i <= t.argc(); ++i) {
      REQUIRE(all[i] >= begin);
      REQUIRE(all[i] < end);
      const size_t len = strlen(all[i]);
      REQUIRE(len > 0);
      REQUIRE(all[i] + len < end);
      CHECK(strchr(all[i], ' ') == nullptr);
      CHECK(strchr(all[i], '\t') == nullptr);
    }
  }
}

TEST_CASE("fuzz: parseU32/parseI32 agree with strtoll on random text") {
  std::mt19937 rng(0xDEADBEEFu);
  static const char kChars[] = "0123456789+- x";
  std::uniform_int_distribution<size_t> lenDist(0, 12);
  std::uniform_int_distribution<size_t> pick(0, sizeof(kChars) - 2);
  std::uniform_int_distribution<uint32_t> bound;
  for (int round = 0; round < 20000; ++round) {
    std::string s(lenDist(rng), '0');
    for (char& c : s) c = (round % 3 == 0) ? kChars[pick(rng)] : static_cast<char>('0' + pick(rng) % 10);
    uint32_t a = bound(rng) % 100000u, b = bound(rng);
    if (round % 4 == 0) b = a + bound(rng) % 1000u;
    const uint32_t lo = std::min(a, b), hi = std::max(a, b);

    // Reference: digits only, fits into [lo, hi].
    bool refOk = !s.empty() && s.find_first_not_of("0123456789") == std::string::npos;
    unsigned long long refVal = 0;
    if (refOk) {
      errno = 0;
      refVal = strtoull(s.c_str(), nullptr, 10);
      refOk = errno == 0 && refVal >= lo && refVal <= hi;
    }
    uint32_t out = 0xA5A5A5A5u;
    CAPTURE(s);
    REQUIRE(vdm::parseU32(s.c_str(), lo, hi, out) == refOk);
    if (refOk) {
      CHECK(out == refVal);
    } else {
      CHECK(out == 0xA5A5A5A5u);
    }

    const int32_t slo = static_cast<int32_t>(lo) - 50000, shi = static_cast<int32_t>(hi / 2);
    bool srefOk = false;
    long long srefVal = 0;
    if (!s.empty()) {
      const size_t digitsFrom = (s[0] == '+' || s[0] == '-') ? 1 : 0;
      const std::string digits = s.substr(digitsFrom);
      if (!digits.empty() && digits.find_first_not_of("0123456789") == std::string::npos) {
        errno = 0;
        srefVal = strtoll(s.c_str(), nullptr, 10);
        srefOk = errno == 0 && srefVal >= INT32_MIN && srefVal <= INT32_MAX &&
                 slo <= shi && srefVal >= slo && srefVal <= shi;
      }
    }
    int32_t sout = 12345;
    REQUIRE(vdm::parseI32(s.c_str(), slo, shi, sout) == srefOk);
    CHECK(sout == (srefOk ? srefVal : 12345));
  }
}

TEST_CASE("fuzz: parseOneWireAddress round-trips and rejects random text") {
  std::mt19937 rng(0x1111u);
  std::uniform_int_distribution<int> byteDist(0, 255);
  for (int round = 0; round < 5000; ++round) {
    uint8_t addr[8];
    for (uint8_t& b : addr) b = static_cast<uint8_t>(byteDist(rng));
    vdm::StaticBufWriter<24> w;
    REQUIRE(w.appendOneWireAddress(addr));
    uint8_t back[8] = {};
    REQUIRE(vdm::parseOneWireAddress(w.c_str(), back));
    CHECK(memcmp(addr, back, 8) == 0);

    // Corrupt one character: only another hex digit in a digit slot keeps it valid.
    std::string text = w.c_str();
    std::uniform_int_distribution<size_t> posDist(0, text.size() - 1);
    const size_t pos = posDist(rng);
    const char replacement = static_cast<char>(byteDist(rng));
    text[pos] = replacement;
    const bool isSepSlot = (pos % 3) == 2;
    const bool isHex = strchr("0123456789abcdefABCDEF", replacement) != nullptr && replacement != '\0';
    const bool expectOk = isSepSlot ? replacement == '-' : isHex;
    uint8_t scratch[8] = {};
    CHECK(vdm::parseOneWireAddress(text.c_str(), scratch) == expectOk);
  }
  for (int round = 0; round < 5000; ++round) {
    const std::string junk = randomBytes(rng, 40);
    const std::string s(junk.c_str());  // stop at the first NUL
    uint8_t out[8] = {1, 1, 1, 1, 1, 1, 1, 1};
    if (!vdm::parseOneWireAddress(s.c_str(), out)) {
      for (uint8_t b : out) CHECK(b == 1);
    } else {
      CHECK(s.size() == vdm::kOneWireAddressTextLen);
    }
  }
}

TEST_CASE("fuzz: BufWriter never exceeds its capacity") {
  std::mt19937 rng(0xBADC0DEu);
  std::uniform_int_distribution<int> op(0, 5);
  std::uniform_int_distribution<uint32_t> num;
  for (int round = 0; round < 3000; ++round) {
    const size_t capacity = static_cast<size_t>(round % 50);
    std::vector<char> storage(capacity + 1, '#');
    vdm::BufWriter w(capacity ? storage.data() : nullptr, capacity);
    std::string model;
    bool modelOk = true;
    for (int step = 0; step < 30; ++step) {
      std::string piece;
      bool r = false;
      const uint32_t v = num(rng);
      switch (op(rng)) {
        case 0: piece = std::to_string(v); r = w.appendUnsigned(v); break;
        case 1: piece = std::to_string(static_cast<int32_t>(v)); r = w.appendSigned(static_cast<int32_t>(v)); break;
        case 2: {
          char hex[3];
          snprintf(hex, sizeof(hex), "%02x", v & 0xFF);
          piece = hex;
          r = w.appendHex2(static_cast<uint8_t>(v));
          break;
        }
        case 3: piece = std::string(v % 7, 'a'); r = w.append(piece.c_str()); break;
        case 4: piece = "x"; r = w.append('x'); break;
        default: {
          const size_t to = v % 20;
          w.truncate(to);
          if (to < model.size()) model.resize(to);
          continue;
        }
      }
      const bool fits = capacity > 0 && model.size() + piece.size() < capacity;
      REQUIRE(r == fits);
      if (fits) {
        model += piece;
      } else {
        modelOk = false;
      }
      REQUIRE(std::string(w.c_str()) == model);
      REQUIRE(w.ok() == modelOk);
    }
    REQUIRE(storage[capacity] == '#');
  }
}

TEST_CASE("fuzz: formatValveData matches the v1 itoa format") {
  std::mt19937 rng(0x7777u);
  std::uniform_int_distribution<int32_t> any(INT32_MIN, INT32_MAX);
  std::uniform_int_distribution<int32_t> small(-2000, 70000);
  for (int round = 0; round < 3000; ++round) {
    auto gen = [&]() { return (round % 2) ? any(rng) : small(rng); };
    vdm::ValveDataReply r{static_cast<uint32_t>(round % 12), gen(), gen(), gen(), gen(), gen(),
                          gen(), gen(), gen(), gen(), gen()};
    vdm::StaticBufWriter<vdm::kValveDataReplyMaxLen + 1> w;
    REQUIRE(vdm::formatValveData(w, "gvlvd", r));
    char expected[256];
    snprintf(expected, sizeof(expected), "gvlvd %u %d %d %d %d %d %d %d %d %d %d ",
             static_cast<unsigned>(r.index), r.actualPosition, r.meanCurrent, r.status,
             r.temperature1, r.temperature2, r.movements, r.openingCount, r.closingCount,
             r.deadzoneCount, r.calibRetries);
    CHECK(std::string(w.c_str()) == expected);
  }
}
