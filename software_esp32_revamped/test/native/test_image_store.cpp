// image_store: image name rules (every character class, lengths, the
// ".bin" suffix, leading '.') and image paths (fit and overflow).
#include <string.h>

#include <string>

#include "doctest.h"
#include "vdm/image_store.h"

using namespace vdm;

namespace {

std::string norm(const std::string& in, size_t cap = 40) {
  char out[64];
  memset(out, 'x', sizeof out);
  return normalizeImageName(in.data(), in.size(), out, cap) ? std::string(out) : "!" + std::string(out);
}

}  // namespace

TEST_CASE("image_store: name rules") {
  CHECK(norm("fw-1_2.bin") == "fw-1_2");
  CHECK(norm("fw-1_2") == "fw-1_2");
  CHECK(norm("AZaz09._-") == "AZaz09._-");
  CHECK(norm("a.bin.bin") == "a.bin");
  CHECK(norm("x.BIN") == "x.BIN");
  CHECK(norm("a.bi") == "a.bi");
  CHECK(norm(std::string(31, 'a')) == std::string(31, 'a'));
  CHECK(norm(std::string(31, 'a') + ".bin") == std::string(31, 'a'));
  CHECK(norm(std::string(32, 'a')) == "!");
  CHECK(norm("") == "!");
  CHECK(norm(".bin") == "!");
  CHECK(norm("bin") == "bin");
  CHECK(norm(".x") == "!");
  for (char c : std::string(" /+#\"\\:@~,!\x7f")) {
    CAPTURE(c);
    CHECK(norm(std::string("a") + c) == "!");
  }
  for (char c : std::string("@[`{")) {  // just outside the letter and digit ranges
    CHECK(norm(std::string("a") + c) == "!");
  }
  CHECK(norm(std::string("/0")) == "!");
  CHECK(norm(std::string(":9")) == "!");
  // The output needs room for the name and the NUL.
  CHECK(norm("abc", 4) == "abc");
  CHECK(norm("abc", 3) == "!");
  char out[4] = "zz";
  CHECK_FALSE(normalizeImageName(nullptr, 3, out, sizeof out));
  CHECK(out[0] == '\0');
  CHECK_FALSE(normalizeImageName("a", 1, nullptr, 4));
  CHECK_FALSE(normalizeImageName("a", 1, out, 0));
  CHECK(out[0] == '\0');
}

TEST_CASE("image_store: paths") {
  char out[40];
  CHECK(imagePath("fw", false, out, sizeof out));
  CHECK(std::string(out) == "/stm/fw.bin");
  CHECK(imagePath("fw", true, out, sizeof out));
  CHECK(std::string(out) == "/stm/fw.bin.part");
  CHECK(imagePath("fw", true, out, 17));
  CHECK_FALSE(imagePath("fw", true, out, 16));
  CHECK(imagePath("fw", false, out, 12));
  CHECK_FALSE(imagePath("fw", false, out, 11));
}
