// image_store: the ".bin" suffix check reads exactly the last four characters of the
// given length, and a one-byte output still gets its NUL.
#include <string.h>

#include <string>

#include "doctest.h"
#include "vdm/image_store.h"

using namespace vdm;

namespace {

std::string normN(const char* in, size_t len) {
  char out[64];
  memset(out, 'x', sizeof out);
  return normalizeImageName(in, len, out, sizeof out) ? std::string(out) : "!" + std::string(out);
}

}  // namespace

TEST_CASE("image_store: the suffix is the last four characters of the given length") {
  // A name not terminated after its length: the byte past it is not part of the suffix.
  const char unterminated[] = "x.binz";
  CHECK(normN(unterminated, 5) == "x");
  // Only the full ".bin" is a suffix, ".bi" followed by something else is not.
  CHECK(normN("x.bix", 5) == "x.bix");
  // Names shorter than the suffix never look before their first character.
  const char before[] = ".bin";
  CHECK(normN(before + 1, 3) == "bin");
  CHECK(normN(before + 3, 1) == "n");
}

TEST_CASE("image_store: a one-byte output is cleared") {
  char out[1] = {'z'};
  CHECK_FALSE(normalizeImageName("a", 1, out, sizeof out));
  CHECK(out[0] == '\0');
}
