// json_writer: every value kind, separators, nesting, misuse, escaping,
// number formats, overflow at every capacity (never writes past it),
// reset, jsonEscape.
#include <math.h>
#include <stdint.h>
#include <string.h>

#include <random>
#include <string>
#include <vector>

#include "doctest.h"
#include "vdm/json_writer.h"

using namespace vdm;

namespace {

std::string doc(const JsonWriter& jw) { return std::string(jw.c_str(), jw.length()); }

// Writes a fixed document exercising every call.
void writeSample(JsonWriter& jw) {
  jw.beginObject();
  jw.kv("s", "a\"b");
  jw.kv("b", true);
  jw.kv("i", static_cast<int32_t>(-12));
  jw.kv("u", static_cast<uint32_t>(4000000000u));
  jw.kv("l", static_cast<int64_t>(-9000000000ll));
  jw.key("n");
  jw.nullValue();
  jw.key("f");
  jw.fixed(215, 1);
  jw.key("d");
  jw.number(1.5, 2);
  jw.key("a");
  jw.beginArray();
  jw.value(1);
  jw.beginObject();
  jw.endObject();
  jw.beginArray();
  jw.endArray();
  jw.raw("{\"r\":1}");
  jw.value("x", 1);
  jw.value("\x01z");  // a long escape followed by a short char
  jw.endArray();
  jw.endObject();
}

const char* const kSample =
    "{\"s\":\"a\\\"b\",\"b\":true,\"i\":-12,\"u\":4000000000,\"l\":-9000000000,\"n\":null,"
    "\"f\":21.5,\"d\":1.50,\"a\":[1,{},[],{\"r\":1},\"x\",\"\\u0001z\"]}";

}  // namespace

TEST_CASE("json writer: full sample document") {
  char buf[256];
  JsonWriter jw(buf, sizeof buf);
  CHECK(jw.ok());
  CHECK_FALSE(jw.complete());
  CHECK(jw.length() == 0);
  CHECK(std::string(jw.c_str()).empty());
  writeSample(jw);
  CHECK(jw.ok());
  CHECK(jw.complete());
  CHECK(doc(jw) == kSample);
  CHECK(strlen(buf) == jw.length());
}

TEST_CASE("json writer: root scalars and single root") {
  char buf[32];
  {
    JsonWriter jw(buf, sizeof buf);
    jw.value(static_cast<int32_t>(5));
    CHECK(jw.complete());
    CHECK(doc(jw) == "5");
    jw.value(static_cast<int32_t>(6));  // second root value
    CHECK_FALSE(jw.ok());
    CHECK(doc(jw) == "5");
  }
  {
    JsonWriter jw(buf, sizeof buf);
    jw.value("x");
    CHECK(doc(jw) == "\"x\"");
    CHECK(jw.complete());
  }
  {
    JsonWriter jw(buf, sizeof buf);
    jw.beginArray();
    jw.endArray();
    CHECK(jw.complete());
    jw.beginArray();
    CHECK_FALSE(jw.ok());
    CHECK(doc(jw) == "[]");
  }
}

TEST_CASE("json writer: misuse poisons the writer") {
  char buf[64];
  auto poisoned = [&](JsonWriter& jw) {
    CHECK_FALSE(jw.ok());
    CHECK_FALSE(jw.complete());
    const std::string before = doc(jw);
    jw.beginObject();
    jw.key("k");
    jw.value(1);
    jw.endObject();
    CHECK(doc(jw) == before);  // every later call is a no-op
  };
  {
    JsonWriter jw(buf, sizeof buf);
    jw.key("k");  // key outside an object
    poisoned(jw);
    CHECK(doc(jw).empty());
  }
  {
    JsonWriter jw(buf, sizeof buf);
    jw.beginArray();
    jw.key("k");  // key in an array
    poisoned(jw);
    CHECK(doc(jw) == "[");
  }
  {
    JsonWriter jw(buf, sizeof buf);
    jw.beginObject();
    jw.key("a");
    jw.key("b");  // two keys
    poisoned(jw);
    CHECK(doc(jw) == "{\"a\":");
  }
  {
    JsonWriter jw(buf, sizeof buf);
    jw.beginObject();
    jw.value(true);  // value without key
    poisoned(jw);
  }
  {
    JsonWriter jw(buf, sizeof buf);
    jw.beginObject();
    jw.key("a");
    jw.endObject();  // pending key
    poisoned(jw);
  }
  {
    JsonWriter jw(buf, sizeof buf);
    jw.beginObject();
    jw.endArray();
    poisoned(jw);
  }
  {
    JsonWriter jw(buf, sizeof buf);
    jw.beginArray();
    jw.endObject();
    poisoned(jw);
  }
  {
    JsonWriter jw(buf, sizeof buf);
    jw.endObject();
    poisoned(jw);
  }
  {
    JsonWriter jw(buf, sizeof buf);
    jw.endArray();
    poisoned(jw);
  }
  {
    JsonWriter jw(buf, sizeof buf);
    jw.beginObject();
    jw.key(nullptr);
    poisoned(jw);
  }
  {
    JsonWriter jw(buf, sizeof buf);
    jw.beginArray();
    jw.value(nullptr, 3);
    poisoned(jw);
  }
  {
    JsonWriter jw(buf, sizeof buf);
    jw.beginArray();
    jw.raw(nullptr);
    poisoned(jw);
  }
  {
    JsonWriter jw(buf, sizeof buf);
    jw.beginArray();
    jw.raw("");
    poisoned(jw);
  }
}

TEST_CASE("json writer: nesting depth") {
  char buf[64];
  JsonWriter jw(buf, sizeof buf);
  for (int i = 0; i < JsonWriter::kMaxDepth; ++i) jw.beginArray();
  CHECK(jw.ok());
  jw.beginArray();
  CHECK_FALSE(jw.ok());
  CHECK(doc(jw) == "[[[[[[[[");

  JsonWriter ok(buf, sizeof buf);
  for (int i = 0; i < JsonWriter::kMaxDepth; ++i) {
    ok.beginObject();
    ok.key("k");
  }
  ok.value(1);
  for (int i = 0; i < JsonWriter::kMaxDepth; ++i) ok.endObject();
  CHECK(ok.complete());
  CHECK(doc(ok) == "{\"k\":{\"k\":{\"k\":{\"k\":{\"k\":{\"k\":{\"k\":{\"k\":1}}}}}}}}");
  JsonWriter deep(buf, sizeof buf);
  for (int i = 0; i < JsonWriter::kMaxDepth; ++i) {
    deep.beginObject();
    deep.key("k");
  }
  deep.beginObject();
  CHECK_FALSE(deep.ok());
}

TEST_CASE("json writer: string escaping") {
  char buf[128];
  JsonWriter jw(buf, sizeof buf);
  jw.beginArray();
  jw.value("\"\\/\b\f\n\r\t");
  jw.value("\x01\x1f\x7f \x10");
  jw.value("\xc3\xa4");
  jw.value("a\0b", 3);
  jw.value("", 0);
  jw.value(static_cast<const char*>(nullptr));
  jw.endArray();
  CHECK(jw.complete());
  CHECK(doc(jw) ==
        "[\"\\\"\\\\/\\b\\f\\n\\r\\t\",\"\\u0001\\u001f\x7f \\u0010\",\"\xc3\xa4\",\"a\\u0000b\",\"\",null]");

  JsonWriter k(buf, sizeof buf);
  k.beginObject();
  k.key("a\"\n");
  k.value(false);
  k.endObject();
  CHECK(doc(k) == "{\"a\\\"\\n\":false}");
}

TEST_CASE("json writer: integers") {
  char buf[128];
  JsonWriter jw(buf, sizeof buf);
  jw.beginArray();
  jw.value(static_cast<int32_t>(INT32_MIN));
  jw.value(static_cast<int32_t>(INT32_MAX));
  jw.value(static_cast<uint32_t>(0));
  jw.value(static_cast<uint32_t>(UINT32_MAX));
  jw.value(static_cast<int64_t>(INT64_MIN));
  jw.value(static_cast<int64_t>(INT64_MAX));
  jw.endArray();
  CHECK(doc(jw) ==
        "[-2147483648,2147483647,0,4294967295,-9223372036854775808,9223372036854775807]");
}

TEST_CASE("json writer: fixed-point") {
  struct Case {
    int32_t v;
    uint8_t d;
    const char* text;
  };
  const Case cases[] = {{215, 1, "21.5"},        {-5, 1, "-0.5"},       {12345, 3, "12.345"},
                        {0, 1, "0.0"},           {-1, 3, "-0.001"},     {7, 0, "7"},
                        {-7, 0, "-7"},           {100, 2, "1.00"},      {-100, 1, "-10.0"},
                        {INT32_MIN, 6, "-2147.483648"}, {INT32_MAX, 6, "2147.483647"},
                        {INT32_MIN, 0, "-2147483648"},  {5, 6, "0.000005"}, {0, 0, "0"}};
  for (const Case& k : cases) {
    CAPTURE(k.text);
    char buf[32];
    JsonWriter jw(buf, sizeof buf);
    jw.fixed(k.v, k.d);
    CHECK(jw.complete());
    CHECK(doc(jw) == k.text);
  }
  char buf[32];
  JsonWriter jw(buf, sizeof buf);
  jw.fixed(1, 7);
  CHECK_FALSE(jw.ok());
  CHECK(doc(jw).empty());
}

TEST_CASE("json writer: doubles") {
  struct Case {
    double v;
    uint8_t d;
    const char* text;
  };
  const Case cases[] = {{0.0, 0, "0"}, {7.0, 0, "7"}, {21.456, 2, "21.46"}, {-3.0, 0, "-3"},      {0.0, 3, "0.000"},
                        {1e15, 0, "1000000000000000"}, {-1e15, 1, "-1000000000000000.0"},
                        {0.1234567, 6, "0.123457"}, {NAN, 2, "null"}, {INFINITY, 2, "null"},
                        {-INFINITY, 0, "null"}};
  for (const Case& k : cases) {
    CAPTURE(k.text);
    char buf[40];
    JsonWriter jw(buf, sizeof buf);
    jw.number(k.v, k.d);
    CHECK(jw.complete());
    CHECK(doc(jw) == k.text);
  }
  char buf[40];
  JsonWriter a(buf, sizeof buf);
  a.number(1.0, 7);
  CHECK_FALSE(a.ok());
  JsonWriter b(buf, sizeof buf);
  b.number(1.0000001e15, 0);
  CHECK_FALSE(b.ok());
  JsonWriter c(buf, sizeof buf);
  c.number(-1.0000001e15, 0);
  CHECK_FALSE(c.ok());
  JsonWriter n(buf, sizeof buf);
  n.beginObject();
  n.number(NAN, 1);  // null without key: misuse
  CHECK_FALSE(n.ok());
}

TEST_CASE("json writer: overflow at every capacity, never past the buffer") {
  char ref[256];
  JsonWriter full(ref, sizeof ref);
  writeSample(full);
  const std::string whole = doc(full);
  REQUIRE(whole == kSample);
  for (size_t cap = 0; cap <= whole.size() + 1; ++cap) {
    CAPTURE(cap);
    std::vector<char> buf(cap + 8, '#');
    JsonWriter jw(buf.data(), cap);
    writeSample(jw);
    const bool fits = cap > whole.size();
    CHECK(jw.ok() == fits);
    CHECK(jw.complete() == fits);
    for (size_t i = cap; i < buf.size(); ++i) CHECK(buf[i] == '#');
    if (cap == 0) continue;
    const std::string got(buf.data());
    CHECK(got.size() == jw.length());
    CHECK(got.size() < cap);
    CHECK(whole.compare(0, got.size(), got) == 0);  // a prefix at a token boundary
  }
}

TEST_CASE("json writer: null buffer, reset and c_str") {
  JsonWriter none(nullptr, 100);
  CHECK_FALSE(none.ok());
  none.value(1);
  CHECK(std::string(none.c_str()).empty());
  CHECK(none.length() == 0);
  none.reset();
  CHECK_FALSE(none.ok());

  char one[1] = {'x'};
  JsonWriter tiny(one, 1);
  CHECK(tiny.ok());
  CHECK(one[0] == '\0');
  tiny.value(1);
  CHECK_FALSE(tiny.ok());
  CHECK(one[0] == '\0');

  char buf[32];
  JsonWriter jw(buf, sizeof buf);
  jw.beginObject();
  jw.key("a");
  jw.reset();
  CHECK(jw.ok());
  CHECK(jw.length() == 0);
  CHECK(std::string(buf).empty());
  jw.beginArray();
  jw.value(true);
  jw.endArray();
  CHECK(jw.complete());
  CHECK(doc(jw) == "[true]");
  jw.value(1);  // poisoned
  jw.reset();   // and recovered
  jw.value(2);
  CHECK(doc(jw) == "2");
}

TEST_CASE("json writer: kv covers every value overload") {
  char buf[128];
  JsonWriter jw(buf, sizeof buf);
  jw.beginObject();
  jw.kv("a", static_cast<const char*>("s"));
  jw.kv("b", false);
  jw.kv("c", static_cast<int32_t>(-1));
  jw.kv("d", static_cast<uint32_t>(1));
  jw.kv("e", static_cast<int64_t>(2));
  jw.kv("f", static_cast<const char*>(nullptr));
  jw.endObject();
  CHECK(doc(jw) == "{\"a\":\"s\",\"b\":false,\"c\":-1,\"d\":1,\"e\":2,\"f\":null}");
}

TEST_CASE("json escape helper") {
  char out[16];
  CHECK(jsonEscape("a\"b", out, sizeof out) == 4);
  CHECK(std::string(out) == "a\\\"b");
  CHECK(jsonEscape("", out, sizeof out) == 0);
  CHECK(std::string(out).empty());
  CHECK(jsonEscape("\n", out, sizeof out) == 2);
  CHECK(std::string(out) == "\\n");
  memset(out, 'z', sizeof out);
  CHECK(jsonEscape(nullptr, out, sizeof out) == 0);
  CHECK(out[0] == '\0');
  // Exact fit: the escaped text plus its NUL.
  CHECK(jsonEscape("abcd", out, 5) == 4);
  CHECK(std::string(out) == "abcd");
  memset(out, 'z', sizeof out);
  CHECK(jsonEscape("abcd", out, 4) == 0);
  CHECK(out[0] == '\0');
  CHECK(out[4] == 'z');
  // Escape sequences are never split: "a\n" needs 3 + 1.
  CHECK(jsonEscape("a\n", out, 4) == 3);
  CHECK(std::string(out) == "a\\n");
  memset(out, 'z', sizeof out);
  CHECK(jsonEscape("a\n", out, 3) == 0);
  CHECK(out[0] == '\0');
  CHECK(out[3] == 'z');
  // Control bytes use \u00XX; the six chars plus NUL need 7.
  CHECK(jsonEscape("\x1f", out, 7) == 6);
  CHECK(std::string(out) == "\\u001f");
  CHECK(jsonEscape("\x1f", out, 6) == 0);
  CHECK(out[0] == '\0');
  CHECK(jsonEscape("\"\\/\t\b\f\r", out, sizeof out) == 13);
  CHECK(std::string(out) == "\\\"\\\\/\\t\\b\\f\\r");
  out[0] = 'q';
  CHECK(jsonEscape("a", out, 0) == 0);
  CHECK(out[0] == 'q');
  CHECK(jsonEscape("a", out, 1) == 0);  // too small, but terminated
  CHECK(out[0] == '\0');
}

TEST_CASE("json writer: random call sequences stay bounded") {
  std::mt19937 rng(1234);
  for (int iter = 0; iter < 2000; ++iter) {
    const size_t cap = rng() % 80;
    std::vector<char> buf(cap + 4, '#');
    JsonWriter jw(buf.data(), cap);
    for (int step = 0; step < 30; ++step) {
      switch (rng() % 10) {
        case 0: jw.beginObject(); break;
        case 1: jw.endObject(); break;
        case 2: jw.beginArray(); break;
        case 3: jw.endArray(); break;
        case 4: jw.key("k\"y"); break;
        case 5: jw.value("v\x01"); break;
        case 6: jw.value(static_cast<int32_t>(rng())); break;
        case 7: jw.fixed(static_cast<int32_t>(rng()), static_cast<uint8_t>(rng() % 8)); break;
        case 8: jw.number(static_cast<double>(rng()) / 7.0, static_cast<uint8_t>(rng() % 8)); break;
        default: jw.nullValue(); break;
      }
    }
    for (size_t i = cap; i < buf.size(); ++i) CHECK(buf[i] == '#');
    if (cap) CHECK(strlen(buf.data()) == jw.length());
    CHECK(jw.length() < (cap ? cap : 1));
  }
}

TEST_CASE("JsonWriter: a key whose ':' does not fit fails at once") {
  char buf[5];
  JsonWriter jw(buf, sizeof buf);
  jw.beginObject();
  REQUIRE(jw.ok());
  jw.key("a");  // "{\"a\"" fits, the ':' does not
  CHECK_FALSE(jw.ok());
  CHECK(std::string(buf) == "{");
  char buf6[6];
  JsonWriter ok6(buf6, sizeof buf6);
  ok6.beginObject();
  ok6.key("a");
  CHECK(ok6.ok());
  CHECK(std::string(buf6) == "{\"a\":");
}
