// ESP side of the STM image check (docs/rust/GLUE-DESIGN-STM.md §5.8, C1-C3): runs the C++
// validateImage() and checkBoard() of software_esp32_revamped/lib/core/src/stm_flasher.cpp,
// the code an ESP 2.1.7 runs before it flashes an STM image, on one Rust image. Until the Rust
// port vdm_esp_core::stm_flasher exists, this harness is the reference (risk R9).
//
//   esp_validate <image.bin> <f401|f411> <C1|C2> <version> [<gvers reply line>]
//
// Built and run by tools/rust/stm/image_check.sh. Exit code 0 when every check passes; one
// line per check on stdout.
#include <cstdio>
#include <cstring>
#include <string>
#include <vector>

#include "vdm/stm_flasher.h"
#include "vdm/version.h"

namespace {

class FileImage : public vdm::FlashImage {
 public:
  explicit FileImage(std::vector<uint8_t> data) : data_(std::move(data)) {}
  uint32_t size() const override { return static_cast<uint32_t>(data_.size()); }
  bool read(uint32_t offset, uint8_t* out, size_t len) override {
    if (offset > data_.size() || len > data_.size() - offset) return false;
    memcpy(out, data_.data() + offset, len);
    return true;
  }

 private:
  std::vector<uint8_t> data_;
};

int failures = 0;

void check(bool ok, const std::string& what) {
  printf("  %s %s\n", ok ? "ok  " : "FAIL", what.c_str());
  if (!ok) failures++;
}

std::string err(vdm::FlashError e) { return vdm::flashErrorName(e); }

// The acceptance of StmFlasher::onAppLine (WaitingApp, after a flash): "gvers " + a version
// with the image's numbers and suffix (sameVersion) and, when both carry one, the same board
// tag. Replicated here because onAppLine is private.
bool gversAccepted(const char* line, const vdm::ImageInfo& info, std::string& why) {
  const size_t n = strlen(line);
  if (n < 6 || memcmp(line, "gvers ", 6) != 0) {
    why = "no gvers prefix";
    return false;
  }
  size_t p = 6;
  while (p < n && line[p] == ' ') ++p;
  size_t q = p;
  while (q < n && line[q] != ' ') ++q;
  vdm::Version app, image;
  if (!vdm::parseVersion(line + p, q - p, app)) {
    why = "version does not parse";
    return false;
  }
  vdm::parseVersion(info.version, strlen(info.version), image);
  const bool same = image.major == app.major && image.minor == app.minor &&
                    image.patch == app.patch && strcmp(image.suffix, app.suffix) == 0 &&
                    (image.hw[0] == '\0' || strcmp(image.hw, app.hw) == 0);
  if (!same) {
    why = "AppVersionMismatch (version)";
    return false;
  }
  if (info.hwTag[0] != '\0' && app.hw[0] != '\0' && strcmp(info.hwTag, app.hw) != 0) {
    why = "AppVersionMismatch (board tag)";
    return false;
  }
  return true;
}

}  // namespace

int main(int argc, char** argv) {
  if (argc < 5) {
    fprintf(stderr, "usage: %s <image.bin> <f401|f411> <C1|C2> <version> [<gvers line>]\n", argv[0]);
    return 2;
  }
  const char* path = argv[1];
  const std::string chip = argv[2];
  const std::string tag = argv[3];
  const std::string version = argv[4];
  FILE* f = fopen(path, "rb");
  if (f == nullptr) {
    fprintf(stderr, "%s: cannot open\n", path);
    return 2;
  }
  std::vector<uint8_t> data;
  uint8_t buf[4096];
  size_t got;
  while ((got = fread(buf, 1, sizeof buf, f)) > 0) data.insert(data.end(), buf, buf + got);
  fclose(f);
  FileImage img(data);
  printf("%s (%zu bytes), ESP validateImage/checkBoard of stm_flasher.cpp:\n", path, data.size());

  // C1: validateImage with requireHandshake (no force), per detected chip
  struct Case {
    uint16_t pid;
    vdm::FlashError expect;
  };
  const bool f401 = chip == "f401";
  const std::vector<Case> cases =
      f401 ? std::vector<Case>{{0, vdm::FlashError::None},
                               {0x423, vdm::FlashError::None},
                               {0x433, vdm::FlashError::None},
                               {0x431, vdm::FlashError::None}}
           : std::vector<Case>{{0, vdm::FlashError::None},
                               {0x431, vdm::FlashError::None},
                               {0x423, vdm::FlashError::ImageChipMismatch},
                               {0x433, vdm::FlashError::ImageChipMismatch}};
  vdm::ImageInfo info;
  for (const Case& c : cases) {
    vdm::ImageInfo i;
    const vdm::FlashError e = vdm::validateImage(img, c.pid, true, i);
    char what[96];
    snprintf(what, sizeof what, "C1 validateImage(pid 0x%03x, requireHandshake) = %s (expected %s)", c.pid,
             err(e).c_str(), err(c.expect).c_str());
    check(e == c.expect, what);
    if (c.pid == 0) info = i;
  }

  // C2: what the scan found
  check(std::string(info.version) == version,
        "C2 version \"" + std::string(info.version) + "\" (expected \"" + version + "\")");
  check(std::string(info.hwTag) == tag, "C2 hwTag \"" + std::string(info.hwTag) + "\" (expected \"" + tag + "\")");
  check(!info.hwConflict, std::string("C2 hwConflict ") + (info.hwConflict ? "true" : "false"));
  check(info.hasHandshake, std::string("C2 hasHandshake ") + (info.hasHandshake ? "true" : "false"));
  char vec[96];
  snprintf(vec, sizeof vec, "C2 initial SP 0x%08x, reset vector 0x%08x, CRC32 0x%08x", info.initialSp,
           info.resetVector, info.crc);
  check(info.initialSp == (f401 ? 0x20010000u : 0x20020000u), vec);

  // C3: the board check before flashing
  const std::string other = tag == "C1" ? "C2" : "C1";
  check(vdm::checkBoard(info.hwTag, tag.c_str()) == vdm::BoardCheck::Ok, "C3 checkBoard(image, " + tag + ") = ok");
  check(vdm::checkBoard(info.hwTag, other.c_str()) == vdm::BoardCheck::Mismatch,
        "C3 checkBoard(image, " + other + ") = mismatch");
  check(vdm::checkBoard(info.hwTag, "") == vdm::BoardCheck::BoardRequired, "C3 checkBoard(image, \"\") = board_required");

  // C5 on the ESP side: the erase set
  const uint8_t sectors = vdm::sectorsForImage(info.paddedSize);
  check(sectors >= 1 && sectors <= 5,
        "C5 sectorsForImage(" + std::to_string(info.paddedSize) + ") = " + std::to_string(sectors) + " (at most 5)");

  // after the flash: the new application's gvers must match the image
  if (argc >= 6) {
    std::string why;
    const bool ok = gversAccepted(argv[5], info, why);
    check(ok, std::string("WaitingApp accepts \"") + argv[5] + "\"" + (ok ? "" : ": " + why));
  }

  printf("  %s\n", failures == 0 ? "PASS" : "FAILED");
  return failures == 0 ? 0 : 1;
}
