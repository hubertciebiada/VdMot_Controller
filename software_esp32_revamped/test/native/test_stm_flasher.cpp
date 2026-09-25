// StmFlasher / validateImage: AN3155 over a simulated STM (v1 boot window,
// ROM bootloader with flash model, application answering gvers), image
// checks, failure paths, cleanup guarantees and fixed-seed fuzz.
#include <stdint.h>
#include <string.h>

#include <algorithm>
#include <deque>
#include <functional>
#include <memory>
#include <set>
#include <string>
#include <utility>
#include <vector>

#include "doctest.h"
#include "support/sim_stm.h"
#include "vdm/stm_flasher.h"

using namespace vdm;
using vdm_test::ACK;
using vdm_test::kBase;
using vdm_test::kKiB;
using vdm_test::Lcg;
using vdm_test::NACK;
using vdm_test::SimStm;

namespace {

// ---------------------------------------------------------------- helpers

uint32_t refCrc32(const uint8_t* d, size_t n) {
  uint32_t c = 0xFFFFFFFFu;
  for (size_t i = 0; i < n; ++i) {
    c ^= d[i];
    for (int k = 0; k < 8; ++k) c = (c & 1u) ? (c >> 1) ^ 0xEDB88320u : c >> 1;
  }
  return ~c;
}

void put32(std::vector<uint8_t>& v, size_t off, uint32_t x) {
  v[off] = static_cast<uint8_t>(x);
  v[off + 1] = static_cast<uint8_t>(x >> 8);
  v[off + 2] = static_cast<uint8_t>(x >> 16);
  v[off + 3] = static_cast<uint8_t>(x >> 24);
}

void putStr(std::vector<uint8_t>& v, size_t off, const std::string& s) {
  memcpy(v.data() + off, s.data(), s.size());
}

// Random body without printable runs (bytes 0x80..0xFF), vectors at 0,
// optional "\x01DEADBEEF\0" "\x01BEEFIT\0" and "\x01<version>\0" near 3/4.
std::vector<uint8_t> makeImage(size_t size, uint32_t sp = 0x20020000u, uint32_t pc = 0,
                               bool handshake = true, const std::string& version = "1.4.9_Dev",
                               uint32_t seed = 7) {
  std::vector<uint8_t> v(size);
  Lcg r(seed);
  for (auto& b : v) b = static_cast<uint8_t>(0x80u | r.next());
  if (size >= 8) {
    put32(v, 0, sp);
    put32(v, 4, pc != 0 ? pc : kBase + 0x1C5u);
  }
  size_t at = size * 3 / 4;
  if (handshake) {
    REQUIRE(at + 20 <= size);
    putStr(v, at, std::string("\x01" "DEADBEEF", 9) + std::string("\0\x01" "BEEFIT\0", 9));
    at += 18;
  }
  if (!version.empty()) {
    REQUIRE(at + version.size() + 2 <= size);
    v[at] = 0x01;
    putStr(v, at + 1, version);
    v[at + 1 + version.size()] = 0;
  }
  return v;
}

class MemImage : public FlashImage {
 public:
  std::vector<uint8_t> data;
  int64_t failAt = -1;       // a read covering this offset fails
  uint32_t sizeOverride = 0;
  size_t bytesRead = 0;
  size_t maxRead = 0;
  uint32_t size() const override {
    return sizeOverride ? sizeOverride : static_cast<uint32_t>(data.size());
  }
  bool read(uint32_t offset, uint8_t* out, size_t len) override {
    bytesRead += len;
    if (len > maxRead) maxRead = len;
    if (failAt >= 0 && failAt >= static_cast<int64_t>(offset) &&
        failAt < static_cast<int64_t>(offset + len)) {
      return false;
    }
    if (offset > data.size() || len > data.size() - offset) return false;
    memcpy(out, data.data() + offset, len);
    return true;
  }
};

// ---------------------------------------------------------------- rig

struct Rig {
  uint32_t now = 1000;
  SimStm sim{now};
  MemImage img;
  StmFlasher f{sim};
  FlashOptions opt;
  std::vector<uint8_t> percents;
  std::vector<FlashPhase> phases;
  size_t steps = 0;

  explicit Rig(std::vector<uint8_t> image = makeImage(53760)) { img.data = std::move(image); }

  bool begin() { return f.begin(img, opt, now); }

  template <typename Hook>
  FlashPhase run(Hook hook, uint32_t maxMs = 30u * 60u * 1000u, uint32_t stepMs = 2) {
    const uint32_t start = now;
    while (f.active() && now - start < maxMs) {
      now += stepMs;
      f.step(now);
      ++steps;
      percents.push_back(f.status().percent);
      if (phases.empty() || phases.back() != f.status().phase) phases.push_back(f.status().phase);
      hook(*this);
    }
    return f.status().phase;
  }
  FlashPhase run() {
    return run([](Rig&) {});
  }
  FlashPhase beginAndRun() {
    REQUIRE(begin());
    return run();
  }

  bool flashMatchesImage() const {
    const auto& d = img.data;
    const size_t padded = (d.size() + 3) & ~size_t(3);
    if (memcmp(sim.flash.data(), d.data(), d.size()) != 0) return false;
    for (size_t i = d.size(); i < padded; ++i) {
      if (sim.flash[i] != 0xFF) return false;
    }
    return true;
  }
  // STM released from reset, running (or about to run) its application, UART 8N1.
  bool leftClean() const {
    return !sim.resets.empty() && !sim.resets.back().second && !sim.configs.empty() &&
           sim.configs.back() == std::make_pair(uint32_t{115200}, false);
  }
  bool percentMonotonic() const {
    for (size_t i = 1; i < percents.size(); ++i) {
      if (percents[i] < percents[i - 1]) return false;
    }
    return true;
  }
  // Only the handshake/app bytes: STM never touched.
  bool untouched() const { return sim.resets.empty() && sim.configs.empty() && sim.writes.empty(); }
};

// Reference image rules (independent re-statement of the header contract).
FlashError refValidate(const std::vector<uint8_t>& d, uint16_t pid, bool req) {
  const size_t size = d.size();
  if (size == 0) return FlashError::ImageEmpty;
  if (size > 512 * kKiB) return FlashError::ImageTooLarge;
  if (size < 8) return FlashError::ImageBadVectors;
  const uint32_t sp = d[0] | d[1] << 8 | d[2] << 16 | static_cast<uint32_t>(d[3]) << 24;
  const uint32_t pc = d[4] | d[5] << 8 | d[6] << 16 | static_cast<uint32_t>(d[7]) << 24;
  if (!(sp > 0x20000000u && sp <= 0x20020000u && sp % 4 == 0)) return FlashError::ImageBadVectors;
  if (!(pc % 2 == 1 && pc >= kBase && pc < kBase + size)) return FlashError::ImageBadVectors;
  if (pid != 0) {
    uint32_t fl = 0, top = 0;
    if (pid == 0x423) fl = 256 * kKiB, top = 0x20010000u;
    if (pid == 0x431) fl = 512 * kKiB, top = 0x20020000u;
    if (pid == 0x433) fl = 512 * kKiB, top = 0x20018000u;
    if (fl == 0) return FlashError::UnknownChip;
    if (size > fl) return FlashError::ImageTooLarge;
    if (sp > top) return FlashError::ImageChipMismatch;
  }
  const std::string s(d.begin(), d.end());
  const bool hs = s.find("DEADBEEF") != std::string::npos && s.find("BEEFIT") != std::string::npos;
  if (req && !hs) return FlashError::ImageNoHandshake;
  return FlashError::None;
}

FlashError validate(const std::vector<uint8_t>& d, uint16_t pid, bool req, ImageInfo& info) {
  MemImage m;
  m.data = d;
  return validateImage(m, pid, req, info);
}

std::string versionOf(const std::vector<uint8_t>& d) {
  ImageInfo info;
  validate(d, 0, false, info);
  return info.version;
}

std::vector<uint8_t> bytesOf(const std::string& s) { return std::vector<uint8_t>(s.begin(), s.end()); }

}  // namespace

// ================================================================ names

TEST_CASE("flasher: phase names, legacy status codes and error names") {
  const struct {
    FlashPhase p;
    const char* name;
    uint8_t legacy;
  } phases[] = {
      {FlashPhase::Idle, "idle", 0},           {FlashPhase::Validating, "validating", 1},
      {FlashPhase::Resetting, "resetting", 1}, {FlashPhase::Handshake, "handshake", 1},
      {FlashPhase::Sync, "sync", 1},           {FlashPhase::GetId, "getid", 1},
      {FlashPhase::Erasing, "erasing", 3},     {FlashPhase::Writing, "writing", 4},
      {FlashPhase::Verifying, "verifying", 5}, {FlashPhase::Starting, "starting", 5},
      {FlashPhase::WaitingApp, "waiting_app", 5}, {FlashPhase::Done, "done", 6},
      {FlashPhase::Failed, "failed", 8},
  };
  for (const auto& p : phases) {
    CHECK(std::string(flashPhaseName(p.p)) == p.name);
    CHECK(legacyFlashStatus(p.p) == p.legacy);
  }
  CHECK(std::string(flashPhaseName(static_cast<FlashPhase>(200))) == "unknown");
  CHECK(legacyFlashStatus(static_cast<FlashPhase>(200)) == 8);

  const struct {
    FlashError e;
    const char* name;
  } errors[] = {
      {FlashError::None, "none"},
      {FlashError::ImageEmpty, "image_empty"},
      {FlashError::ImageTooLarge, "image_too_large"},
      {FlashError::ImageBadVectors, "image_bad_vectors"},
      {FlashError::ImageNoHandshake, "image_no_handshake"},
      {FlashError::ImageChipMismatch, "image_chip_mismatch"},
      {FlashError::ImageRead, "image_read"},
      {FlashError::HandshakeTimeout, "handshake_timeout"},
      {FlashError::SyncFailed, "sync_failed"},
      {FlashError::UnknownChip, "unknown_chip"},
      {FlashError::Nack, "nack"},
      {FlashError::Timeout, "timeout"},
      {FlashError::VerifyMismatch, "verify_mismatch"},
      {FlashError::TransportWrite, "transport_write"},
      {FlashError::AppNotResponding, "app_not_responding"},
      {FlashError::AppVersionMismatch, "app_version_mismatch"},
      {FlashError::Aborted, "aborted"},
      {FlashError::BoardMismatch, "board_mismatch"},
      {FlashError::BoardRequired, "board_required"},
  };
  CHECK(sizeof errors / sizeof errors[0] == static_cast<size_t>(FlashError::BoardRequired) + 1);
  for (const auto& e : errors) CHECK(std::string(flashErrorName(e.e)) == e.name);
  CHECK(std::string(flashErrorName(static_cast<FlashError>(200))) == "unknown");
}

// ================================================================ board check

TEST_CASE("flasher: board check table and tag rule") {
  CHECK(checkBoard("", "C2") == BoardCheck::Untagged);
  CHECK(checkBoard("", "") == BoardCheck::Untagged);
  CHECK(checkBoard(nullptr, "C2") == BoardCheck::Untagged);
  CHECK(checkBoard("C2", "") == BoardCheck::BoardRequired);
  CHECK(checkBoard("C2", nullptr) == BoardCheck::BoardRequired);
  CHECK(checkBoard("C2", "C2") == BoardCheck::Ok);
  CHECK(checkBoard("C12", "C12") == BoardCheck::Ok);
  CHECK(checkBoard("C1", "C2") == BoardCheck::Mismatch);
  CHECK(checkBoard("C1", "C12") == BoardCheck::Mismatch);
  CHECK(checkBoard("C12", "C1") == BoardCheck::Mismatch);
  // Tag arrays without a NUL: at most their 4 bytes are read.
  const char full[4] = {'C', '1', '2', '3'};
  const char fullToo[4] = {'C', '1', '2', '3'};
  CHECK(checkBoard(full, fullToo) == BoardCheck::Ok);
  CHECK(checkBoard(full, "C12") == BoardCheck::Mismatch);

  CHECK(boardTagValid("C1"));
  CHECK(boardTagValid("C0"));
  CHECK(boardTagValid("C9"));
  CHECK(boardTagValid("C12"));
  CHECK(boardTagValid("C99"));
  CHECK_FALSE(boardTagValid("C"));
  CHECK_FALSE(boardTagValid("C123"));
  CHECK_FALSE(boardTagValid("c1"));
  CHECK_FALSE(boardTagValid("X1"));
  CHECK_FALSE(boardTagValid("C1x"));
  CHECK_FALSE(boardTagValid("Cx"));
  CHECK_FALSE(boardTagValid("C/"));
  CHECK_FALSE(boardTagValid("C:"));
  CHECK_FALSE(boardTagValid(""));
  CHECK_FALSE(boardTagValid(nullptr));
  CHECK_FALSE(boardTagValid(full));  // 3 digits, no NUL inside the 4 bytes

  CHECK(std::string(boardCheckName(BoardCheck::Ok)) == "ok");
  CHECK(std::string(boardCheckName(BoardCheck::Untagged)) == "untagged");
  CHECK(std::string(boardCheckName(BoardCheck::Mismatch)) == "mismatch");
  CHECK(std::string(boardCheckName(BoardCheck::BoardRequired)) == "board_required");
  CHECK(std::string(boardCheckName(static_cast<BoardCheck>(4))) == "unknown");
}

TEST_CASE("flasher: option and status defaults of the board check") {
  const FlashOptions opt;
  CHECK(opt.appTimeoutMs == 60000);
  CHECK(opt.fallbackBaud == 57600);
  CHECK(opt.boardHw[0] == '\0');
  const FlashStatus st;
  CHECK(st.board == BoardCheck::Ok);
  CHECK(st.boardHw[0] == '\0');
  CHECK_FALSE(st.manualReset);
  CHECK(st.baud == 0);
  const ImageInfo info;
  CHECK(info.hwTag[0] == '\0');
  CHECK_FALSE(info.hwConflict);
}

// ================================================================ sectors

TEST_CASE("flasher: sectorsForImage boundaries (16,16,16,16,64,128,128,128 KiB)") {
  CHECK(sectorsForImage(0) == 0);
  CHECK(sectorsForImage(1) == 1);
  const uint32_t ends[] = {16, 32, 48, 64, 128, 256, 384, 512};
  for (uint8_t i = 0; i < 8; ++i) {
    CAPTURE(i);
    CHECK(sectorsForImage(ends[i] * kKiB) == i + 1);
    CHECK(sectorsForImage(ends[i] * kKiB - 1) == i + 1);
    if (i < 7) CHECK(sectorsForImage(ends[i] * kKiB + 1) == i + 2);
  }
  CHECK(sectorsForImage(512 * kKiB + 1) == 0);
  CHECK(sectorsForImage(0xFFFFFFFFu) == 0);
}

// ================================================================ validateImage

TEST_CASE("validate: CRC reference and a real-release-like image") {
  const char* golden = "123456789";
  CHECK(refCrc32(reinterpret_cast<const uint8_t*>(golden), 9) == 0xCBF43926u);

  // Shape of releases/STM32/1.4.9_Dev/STM32F411_C1_firmware.bin: 53 760 B,
  // SP 0x20020000, reset vector odd inside the image, handshake strings.
  const auto d = makeImage(53760, 0x20020000u, 0x0800B4D1u);
  ImageInfo info;
  CHECK(validate(d, 0, true, info) == FlashError::None);
  CHECK(info.size == 53760);
  CHECK(info.paddedSize == 53760);
  CHECK(info.initialSp == 0x20020000u);
  CHECK(info.resetVector == 0x0800B4D1u);
  CHECK(info.hasHandshake);
  CHECK(info.crc == refCrc32(d.data(), d.size()));
  CHECK(std::string(info.version) == "1.4.9_Dev");
  CHECK(validate(d, 0x431, true, info) == FlashError::None);
  CHECK(validate(d, 0x433, true, info) == FlashError::ImageChipMismatch);  // 96 KiB RAM
  CHECK(validate(d, 0x423, true, info) == FlashError::ImageChipMismatch);

  const auto f401 = makeImage(53472, 0x20010000u, 0x0800B4B1u);
  CHECK(validate(f401, 0x423, true, info) == FlashError::None);
  CHECK(validate(f401, 0x433, true, info) == FlashError::None);
  CHECK(validate(f401, 0x431, true, info) == FlashError::None);
}

TEST_CASE("validate: size limits and padding") {
  ImageInfo info;
  CHECK(validate({}, 0, false, info) == FlashError::ImageEmpty);
  CHECK(info.size == 0);
  CHECK(info.paddedSize == 0);

  auto big = makeImage(512 * kKiB + 1, 0x20020000u, kBase + 1, false, "");
  CHECK(validate(big, 0, false, info) == FlashError::ImageTooLarge);
  CHECK(info.size == 512 * kKiB + 1);
  big.pop_back();
  CHECK(validate(big, 0, false, info) == FlashError::None);
  CHECK(info.paddedSize == 512 * kKiB);

  for (size_t n = 1; n < 8; ++n) {
    CAPTURE(n);
    CHECK(validate(std::vector<uint8_t>(n, 0), 0, false, info) == FlashError::ImageBadVectors);
    CHECK(info.size == n);
    CHECK(info.paddedSize == ((n + 3) & ~size_t(3)));
  }
  const struct {
    size_t size;
    uint32_t padded;
  } pads[] = {{8, 8}, {9, 12}, {10, 12}, {11, 12}, {12, 12}, {13, 16}, {1001, 1004}};
  for (const auto& p : pads) {
    CAPTURE(p.size);
    auto d = makeImage(p.size, 0x20001000u, kBase + 1, false, "");
    CHECK(validate(d, 0, false, info) == FlashError::None);
    CHECK(info.paddedSize == p.padded);
  }
}

TEST_CASE("validate: initial SP and reset vector boundaries") {
  ImageInfo info;
  const struct {
    uint32_t sp;
    FlashError e;
  } sps[] = {
      {0x20000000u, FlashError::ImageBadVectors}, {0x20000004u, FlashError::None},
      {0x2001FFFCu, FlashError::None},            {0x20020000u, FlashError::None},
      {0x20020004u, FlashError::ImageBadVectors}, {0x2001FFFEu, FlashError::ImageBadVectors},
      {0x20010001u, FlashError::ImageBadVectors}, {0x1FFFFFFCu, FlashError::ImageBadVectors},
      {0x00000000u, FlashError::ImageBadVectors}, {0xFFFFFFFCu, FlashError::ImageBadVectors},
  };
  for (const auto& s : sps) {
    CAPTURE(s.sp);
    CHECK(validate(makeImage(1024, s.sp, kBase + 0x101), 0, true, info) == s.e);
    CHECK(info.initialSp == s.sp);
  }
  const uint32_t size = 1023;  // odd: kBase + size is odd too
  const struct {
    uint32_t pc;
    FlashError e;
  } pcs[] = {
      {kBase + 1, FlashError::None},
      {kBase + size - 2, FlashError::None},
      {kBase + size, FlashError::ImageBadVectors},
      {kBase + size + 2, FlashError::ImageBadVectors},
      {kBase + 0x100, FlashError::ImageBadVectors},  // even: not Thumb
      {kBase - 1, FlashError::ImageBadVectors},
      {0x1FFF0001u, FlashError::ImageBadVectors},
      {0xFFFFFFFFu, FlashError::ImageBadVectors},
  };
  for (const auto& p : pcs) {
    CAPTURE(p.pc);
    CHECK(validate(makeImage(size, 0x20020000u, p.pc), 0, true, info) == p.e);
    CHECK(info.resetVector == p.pc);
  }
  // Even size: last odd address inside is kBase + size - 1.
  CHECK(validate(makeImage(1024, 0x20020000u, kBase + 1023), 0, true, info) == FlashError::None);
  CHECK(validate(makeImage(1024, 0x20020000u, kBase + 1025), 0, true, info) ==
        FlashError::ImageBadVectors);
}

TEST_CASE("validate: chip-specific limits") {
  ImageInfo info;
  const auto at = [&](size_t size, uint32_t sp, uint16_t pid) {
    return validate(makeImage(size, sp, kBase + 1, false, ""), pid, false, info);
  };
  CHECK(at(256 * kKiB, 0x20010000u, 0x423) == FlashError::None);
  CHECK(at(256 * kKiB + 1, 0x20010000u, 0x423) == FlashError::ImageTooLarge);
  CHECK(at(256 * kKiB, 0x20010004u, 0x423) == FlashError::ImageChipMismatch);
  CHECK(at(512 * kKiB, 0x20018000u, 0x433) == FlashError::None);
  CHECK(at(512 * kKiB, 0x2001800Cu, 0x433) == FlashError::ImageChipMismatch);
  CHECK(at(512 * kKiB, 0x20020000u, 0x431) == FlashError::None);
  // Size is checked before SP.
  CHECK(at(300 * kKiB, 0x20020000u, 0x423) == FlashError::ImageTooLarge);
  // Unknown / non-F401/F411 PIDs.
  CHECK(at(1024, 0x20010000u, 0x413) == FlashError::UnknownChip);
  CHECK(at(1024, 0x20010000u, 0x001) == FlashError::UnknownChip);
  CHECK(at(1024, 0x20010000u, 0xFFFF) == FlashError::UnknownChip);
  // Generic errors win over chip errors.
  CHECK(at(1024, 0x20000000u, 0x413) == FlashError::ImageBadVectors);
  CHECK(validate({}, 0x413, false, info) == FlashError::ImageEmpty);
}

TEST_CASE("validate: handshake strings, also across chunk boundaries") {
  ImageInfo info;
  auto d = makeImage(4096, 0x20020000u, kBase + 1, false, "");
  CHECK(validate(d, 0, true, info) == FlashError::ImageNoHandshake);
  CHECK_FALSE(info.hasHandshake);
  CHECK(info.crc == refCrc32(d.data(), d.size()));  // filled before the handshake verdict
  CHECK(validate(d, 0, false, info) == FlashError::None);
  CHECK_FALSE(info.hasHandshake);

  auto onlyDead = d;
  putStr(onlyDead, 1000, "DEADBEEF");
  CHECK(validate(onlyDead, 0, true, info) == FlashError::ImageNoHandshake);
  auto onlyBeef = d;
  putStr(onlyBeef, 1000, "BEEFIT");
  CHECK(validate(onlyBeef, 0, true, info) == FlashError::ImageNoHandshake);
  auto nearMiss = d;
  putStr(nearMiss, 1000, "DEADBEEX");
  putStr(nearMiss, 2000, "BEEFIX");
  CHECK(validate(nearMiss, 0, true, info) == FlashError::ImageNoHandshake);

  // Every split position of both strings across the 256-byte read chunks.
  for (size_t k = 1; k < 8; ++k) {
    CAPTURE(k);
    auto s = d;
    putStr(s, 512 - k, "DEADBEEF");
    putStr(s, 1024 - (k < 6 ? k : 5), "BEEFIT");
    CHECK(validate(s, 0, true, info) == FlashError::None);
    CHECK(info.hasHandshake);
  }
  // At the very end of the image.
  auto end = d;
  putStr(end, 4096 - 14, "DEADBEEFBEEFIT");
  CHECK(validate(end, 0, true, info) == FlashError::None);
  // Overlapping "DEADBEEFIT" contains both.
  auto both = d;
  putStr(both, 3000, "DEADBEEFIT");
  CHECK(validate(both, 0, true, info) == FlashError::None);
}

TEST_CASE("validate: version string extraction") {
  const auto withBlob = [](const std::string& blob, size_t at = 2000) {
    auto d = makeImage(4096, 0x20020000u, kBase + 1, false, "");
    putStr(d, at, blob);
    return d;
  };
  using S = std::string;
  CHECK(versionOf(withBlob(S("\x01" "1.4.9_Dev\0", 11))) == "1.4.9_Dev");
  CHECK(versionOf(withBlob(S("\0" "2.0.0-revamped\0", 16))) == "2.0.0-revamped");
  CHECK(versionOf(withBlob(S("\x01" "1.4.9_C1\0", 10))) == "1.4.9_C1");
  CHECK(versionOf(withBlob(S("\x01" "1.4\0", 5))) == "");
  CHECK(versionOf(withBlob(S("\x01" "abc 1.4.9\0", 11))) == "");  // run has a space
  CHECK(versionOf(withBlob(S("\x01" "1.4.9 \0", 8))) == "");
  CHECK(versionOf(withBlob(S("\x01" "x1.4.9\0", 8))) == "");
  CHECK(versionOf(withBlob(S("\x01" "1.4.9\x01", 7))) == "");  // not NUL-terminated
  CHECK(versionOf(withBlob(S("xyz\x02" "1.4.9\0", 10))) == "1.4.9");  // run restarts
  // Printable is 0x20..0x7E: '~' and ' ' belong to the run, 0x1F and 0x7F end it.
  CHECK(versionOf(withBlob(S("\x01" "~1.4.9\0", 8))) == "");
  CHECK(versionOf(withBlob(S("\x01" " 1.4.9\0", 8))) == "");
  CHECK(versionOf(withBlob(S("\x1F" "1.4.9\0", 7))) == "1.4.9");
  CHECK(versionOf(withBlob(S("~~\x7F" "1.4.9\0", 9))) == "1.4.9");
  // First match wins; invalid candidates before it are skipped.
  CHECK(versionOf(withBlob(S("\x01" "9.9\0" "1.2.3\0" "4.5.6\0", 17))) == "1.2.3");
  // 31 characters is the longest version; a 32-char run is not a version,
  // not even its 31-char tail.
  const S v31 = "1.4.9_" + S(25, 'a');
  REQUIRE(v31.size() == 31);
  CHECK(versionOf(withBlob("\x01" + v31 + S("\0", 1))) == v31);
  CHECK(versionOf(withBlob("\x01" "x" + v31 + S("\0", 1))) == "");
  CHECK(versionOf(withBlob("\x01" + S(100, 'q') + "1.2.3" + S("\0", 1))) == "");
  // Image ending in a run without NUL.
  auto tail = makeImage(4096, 0x20020000u, kBase + 1, false, "");
  putStr(tail, 4096 - 6, "\x01" "1.2.3");
  CHECK(versionOf(tail) == "");
  // Version as the first bytes after the vectors: vector bytes are not printable here.
  auto early = makeImage(4096, 0x20020000u, kBase + 0x101, false, "");
  putStr(early, 8, S("3.2.1\0", 6));
  CHECK(versionOf(early) == "3.2.1");
}

TEST_CASE("validate: read errors") {
  MemImage m;
  m.data = makeImage(4096);
  ImageInfo info;
  m.failAt = 3;
  CHECK(validateImage(m, 0, true, info) == FlashError::ImageRead);
  m.failAt = 3000;
  CHECK(validateImage(m, 0, true, info) == FlashError::ImageRead);
  CHECK(info.initialSp == 0x20020000u);  // header facts kept
  m.failAt = -1;
  m.sizeOverride = 5000;  // size() larger than the readable data
  CHECK(validateImage(m, 0, true, info) == FlashError::ImageRead);
}

TEST_CASE("validate: fixed-seed fuzz against the reference rules") {
  Lcg r(0xC0FFEEu);
  const uint16_t pids[] = {0, 0x423, 0x431, 0x433, 0x413};
  for (int iter = 0; iter < 3000; ++iter) {
    size_t size = r.below(4) == 0 ? r.below(16) : 8 + r.below(2048);
    std::vector<uint8_t> d(size);
    for (auto& b : d) b = static_cast<uint8_t>(r.next());
    if (size >= 8) {
      const uint32_t sps[] = {0x20020000u, 0x20010000u, 0x20018000u, 0x20000000u,
                              0x20000004u, 0x20020004u, 0x20018002u, r.next()};
      put32(d, 0, sps[r.below(8)]);
      const uint32_t pcs[] = {kBase + 1, static_cast<uint32_t>(kBase + size - 1),
                              static_cast<uint32_t>(kBase + size), kBase + 2 * r.below(1200) + 1,
                              kBase + 2 * r.below(1200), r.next()};
      put32(d, 4, pcs[r.below(6)]);
    }
    if (size > 40 && r.below(2)) putStr(d, 10 + r.below(static_cast<uint32_t>(size - 30)), "DEADBEEF");
    if (size > 40 && r.below(2)) putStr(d, 10 + r.below(static_cast<uint32_t>(size - 30)), "BEEFIT");
    const uint16_t pid = pids[r.below(5)];
    const bool req = r.below(2) != 0;
    ImageInfo info;
    CAPTURE(iter);
    CHECK(validate(d, pid, req, info) == refValidate(d, pid, req));
    CHECK(info.size == size);
  }
}

// ================================================================ begin/idle

TEST_CASE("flasher: idle object, begin() preconditions") {
  Rig rig;
  CHECK_FALSE(rig.f.active());
  CHECK(rig.f.status().phase == FlashPhase::Idle);
  CHECK(rig.f.step(rig.now) == FlashPhase::Idle);
  rig.f.abort();
  CHECK(rig.f.step(rig.now) == FlashPhase::Idle);
  CHECK(rig.untouched());

  rig.opt.baud = 1199;
  CHECK_FALSE(rig.begin());
  rig.opt.baud = 115201;
  CHECK_FALSE(rig.begin());
  rig.opt.baud = 0;
  CHECK_FALSE(rig.begin());
  CHECK(rig.f.status().phase == FlashPhase::Idle);

  rig.opt.baud = 1200;
  CHECK(rig.begin());
  CHECK(rig.f.active());
  CHECK(rig.f.status().phase == FlashPhase::Validating);
  CHECK(rig.f.status().startedMs == 1000);
  CHECK(rig.f.status().percent == 0);
  CHECK(rig.f.status().error == FlashError::None);
  // Busy: a second begin is refused and changes nothing.
  rig.now += 50;
  CHECK_FALSE(rig.f.begin(rig.img, rig.opt, rig.now));
  CHECK(rig.f.status().startedMs == 1000);
  CHECK(rig.untouched());
}

TEST_CASE("flasher: begin accepts the upper baud limit") {
  Rig rig;
  rig.opt.baud = 115200;
  CHECK(rig.begin());
}

// ================================================================ happy path

TEST_CASE("flasher: normal mode end to end on a 1.4.9-sized F411 image") {
  Rig rig;
  REQUIRE(rig.begin());
  CHECK(rig.run() == FlashPhase::Done);
  const FlashStatus& st = rig.f.status();
  CHECK(st.error == FlashError::None);
  CHECK(st.percent == 100);
  CHECK(st.chipPid == 0x431);
  CHECK(st.bootloaderVersion == 0x31);
  CHECK(st.attempt == 0);
  CHECK(st.bytesTotal == 53760);
  CHECK(st.bytesDone == 53760);
  CHECK(st.image.size == 53760);
  CHECK(std::string(st.image.version) == "1.4.9_Dev");
  CHECK(st.image.crc == refCrc32(rig.img.data.data(), rig.img.data.size()));
  CHECK(st.appVersion.valid);
  CHECK(st.appVersion.major == 1);
  CHECK(st.appVersion.patch == 9);
  CHECK(std::string(st.appVersion.hw) == "C1");
  CHECK(st.finishedMs == rig.now);
  CHECK(st.startedMs == 1000);
  CHECK_FALSE(rig.f.active());
  CHECK(rig.percentMonotonic());
  CHECK(rig.flashMatchesImage());
  // Sectors 0..3 (64 KiB) erased; the rest of the chip untouched.
  CHECK(memcmp(rig.sim.flash.data() + 64 * kKiB, rig.sim.original.data() + 64 * kKiB,
               448 * kKiB) == 0);
  for (size_t i = 53760; i < 64 * kKiB; ++i) REQUIRE(rig.sim.flash[i] == 0xFF);

  // Phase order.
  const std::vector<FlashPhase> order = {
      FlashPhase::Validating, FlashPhase::Resetting, FlashPhase::Handshake, FlashPhase::Sync,
      FlashPhase::GetId,      FlashPhase::Erasing,   FlashPhase::Writing,   FlashPhase::Verifying,
      FlashPhase::Starting,   FlashPhase::WaitingApp, FlashPhase::Done};
  std::vector<FlashPhase> seen = {FlashPhase::Validating};
  for (FlashPhase p : rig.phases) {
    if (p != seen.back()) seen.push_back(p);
  }
  CHECK(seen == order);

  // UART: 8E1 at the bootloader baud, then 8N1 for the application.
  REQUIRE(rig.sim.configs.size() == 2);
  CHECK(rig.sim.configs[0] == std::make_pair(uint32_t{115200}, true));
  CHECK(rig.sim.configs[1] == std::make_pair(uint32_t{115200}, false));
  // Two NRST pulses of 100 ms.
  REQUIRE(rig.sim.resets.size() == 4);
  CHECK(rig.sim.resets[0].second);
  CHECK_FALSE(rig.sim.resets[1].second);
  CHECK(rig.sim.resets[1].first - rig.sim.resets[0].first >= 100);
  CHECK(rig.sim.resets[1].first - rig.sim.resets[0].first <= 102);
  CHECK(rig.sim.resets[2].second);
  CHECK_FALSE(rig.sim.resets[3].second);
  CHECK(rig.sim.resets[3].first - rig.sim.resets[2].first >= 100);
  CHECK(rig.sim.resets[3].first - rig.sim.resets[2].first <= 102);

  // Handshake: first "DEADBEEF\n" 20 ms after release.
  const uint32_t release = rig.sim.resets[1].first;
  const auto& w0 = rig.sim.writes[0];
  CHECK(w0.second == bytesOf("DEADBEEF\n"));
  CHECK(w0.first - release >= 20);
  CHECK(w0.first - release <= 22);

  // Sync 0x7F >= 250 ms after BEEFIT, then GET, GET ID, erase list.
  REQUIRE(rig.sim.syncTimes.size() == 1);
  CHECK(rig.sim.syncTimes[0] - rig.sim.beefitAt >= 250);
  CHECK(rig.sim.syncTimes[0] - rig.sim.beefitAt <= 254);
  REQUIRE(rig.sim.commands.size() >= 3);
  CHECK(rig.sim.commands[0] == 0x00);
  CHECK(rig.sim.commands[1] == 0x02);
  CHECK(rig.sim.commands[2] == 0x44);
  REQUIRE(rig.sim.eraseFrames.size() == 1);
  CHECK(rig.sim.eraseFrames[0] ==
        std::vector<uint8_t>{0x00, 0x03, 0x00, 0x00, 0x00, 0x01, 0x00, 0x02, 0x00, 0x03, 0x03});

  // 210 blocks written once each: 1..209, then 0.
  REQUIRE(rig.sim.programmed.size() == 210);
  for (uint32_t i = 0; i < 209; ++i) {
    CHECK(rig.sim.programmed[i].addr == kBase + (i + 1) * 256);
    CHECK(rig.sim.programmed[i].len == 256);
  }
  CHECK(rig.sim.programmed[209].addr == kBase);
  // Verify reads 0..209 in order.
  REQUIRE(rig.sim.readAddrs.size() == 210);
  for (uint32_t i = 0; i < 210; ++i) CHECK(rig.sim.readAddrs[i] == kBase + i * 256);

  // App: gvers 4 s after the final release, exact request text.
  REQUIRE(rig.sim.gversTimes.size() == 1);
  CHECK(rig.sim.gversTimes[0] - rig.sim.resets[3].first >= 4000);
  CHECK(rig.sim.gversTimes[0] - rig.sim.resets[3].first <= 4002);
  CHECK(rig.sim.writes.back().second == bytesOf("gvers \r\n"));
  CHECK(rig.leftClean());
  CHECK(rig.sim.inApp());
}

TEST_CASE("flasher: exact AN3155 frames for write and read of one block") {
  Rig rig(makeImage(600));  // blocks: 0 (256), 1 (256), 2 (88)
  CHECK(rig.beginAndRun() == FlashPhase::Done);
  std::vector<std::vector<uint8_t>> w;
  for (auto& x : rig.sim.writes) w.push_back(x.second);
  // Locate the first write command.
  size_t i = 0;
  while (i < w.size() && w[i] != std::vector<uint8_t>{0x31, 0xCE}) ++i;
  REQUIRE(i + 2 < w.size());
  CHECK(w[i + 1] == std::vector<uint8_t>{0x08, 0x00, 0x01, 0x00, 0x09});
  REQUIRE(w[i + 2].size() == 258);
  CHECK(w[i + 2][0] == 0xFF);
  uint8_t cs = 0xFF;
  for (size_t k = 0; k < 256; ++k) {
    CHECK(w[i + 2][1 + k] == rig.img.data[256 + k]);
    cs ^= rig.img.data[256 + k];
  }
  CHECK(w[i + 2][257] == cs);
  // Second block: the 88-byte tail at 0x08000200.
  CHECK(w[i + 3] == std::vector<uint8_t>{0x31, 0xCE});
  CHECK(w[i + 4] == std::vector<uint8_t>{0x08, 0x00, 0x02, 0x00, 0x0A});
  CHECK(w[i + 5].size() == 90);
  CHECK(w[i + 5][0] == 87);
  // Then block 0 last.
  CHECK(w[i + 7] == std::vector<uint8_t>{0x08, 0x00, 0x00, 0x00, 0x08});
  // First read: 11 EE, address, N ~N.
  while (i < w.size() && w[i] != std::vector<uint8_t>{0x11, 0xEE}) ++i;
  REQUIRE(i + 2 < w.size());
  CHECK(w[i + 1] == std::vector<uint8_t>{0x08, 0x00, 0x00, 0x00, 0x08});
  CHECK(w[i + 2] == std::vector<uint8_t>{0xFF, 0x00});
  // Tail read: N = 87.
  CHECK(w[i + 8] == std::vector<uint8_t>{87, static_cast<uint8_t>(87 ^ 0xFF)});
  // Command framing of GET / GET ID / erase.
  CHECK(std::count(w.begin(), w.end(), std::vector<uint8_t>{0x00, 0xFF}) == 1);
  CHECK(std::count(w.begin(), w.end(), std::vector<uint8_t>{0x02, 0xFD}) == 1);
  CHECK(std::count(w.begin(), w.end(), std::vector<uint8_t>{0x44, 0xBB}) == 1);
  CHECK(std::count(w.begin(), w.end(), std::vector<uint8_t>{0x7F}) == 1);
  CHECK(rig.sim.eraseFrames[0] == std::vector<uint8_t>{0x00, 0x00, 0x00, 0x00, 0x00});
}

TEST_CASE("flasher: image sizes around block and word boundaries") {
  const size_t sizes[] = {8, 9, 255, 256, 257, 511, 512, 1001, 16 * 1024 + 4};
  for (size_t size : sizes) {
    CAPTURE(size);
    const bool hs = size >= 64;
    Rig rig(makeImage(size, 0x20020000u, kBase + 1, hs, hs ? "1.4.9_Dev" : ""));
    rig.opt.force = !hs;
    CHECK(rig.beginAndRun() == FlashPhase::Done);
    CHECK(rig.f.status().error == FlashError::None);
    CHECK(rig.flashMatchesImage());
    const uint32_t padded = static_cast<uint32_t>((size + 3) & ~size_t(3));
    CHECK(rig.f.status().bytesTotal == padded);
    uint32_t total = 0;
    for (auto& p : rig.sim.programmed) {
      CHECK(p.len % 4 == 0);
      total += p.len;
    }
    CHECK(total == padded);
    CHECK(rig.sim.programmed.back().addr == kBase);
    CHECK(rig.sim.programmed.size() == (padded + 255) / 256);
    CHECK(rig.sim.eraseFrames[0][1] == sectorsForImage(padded) - 1);
  }
}

TEST_CASE("flasher: full 512 KiB image erases all 8 sectors") {
  Rig rig(makeImage(512 * kKiB));
  CHECK(rig.beginAndRun() == FlashPhase::Done);
  CHECK(rig.flashMatchesImage());
  REQUIRE(rig.sim.eraseFrames.size() == 1);
  std::vector<uint8_t> exp = {0x00, 0x07};
  uint8_t cs = 0x07;
  for (uint8_t s = 0; s < 8; ++s) {
    exp.push_back(0);
    exp.push_back(s);
    cs ^= s;
  }
  exp.push_back(cs);
  CHECK(rig.sim.eraseFrames[0] == exp);
}

TEST_CASE("flasher: one data frame per step, chained transitions") {
  Rig rig(makeImage(16 * kKiB));
  REQUIRE(rig.begin());
  size_t seen = 0;
  size_t maxFrames = 0;
  uint32_t writeStart = 0, verifyStart = 0, startingAt = 0;
  rig.run([&](Rig& r) {
    size_t frames = 0;
    for (; seen < r.sim.writes.size(); ++seen) frames += r.sim.writes[seen].second.size() > 16;
    if (frames > maxFrames) maxFrames = frames;
    const FlashPhase p = r.f.status().phase;
    if (p == FlashPhase::Writing && writeStart == 0) writeStart = r.now;
    if (p == FlashPhase::Verifying && verifyStart == 0) verifyStart = r.now;
    if (p == FlashPhase::Starting && startingAt == 0) startingAt = r.now;
  });
  CHECK(rig.f.status().phase == FlashPhase::Done);
  CHECK(maxFrames == 1);
  // 64 blocks; the simulator answers after 1-2 ms, so a chained write is
  // 2-3 steps (4-6 ms) per block, a read-back about the same.
  CHECK(verifyStart - writeStart <= 64 * 6 + 4);
  CHECK(startingAt - verifyStart <= 64 * 6 + 4);
}

TEST_CASE("flasher: validation reads at most 1 KiB per step") {
  Rig rig(makeImage(512 * kKiB));
  REQUIRE(rig.begin());
  size_t prev = 0;
  size_t maxPerStep = 0;
  size_t validatingSteps = 0;
  rig.run([&](Rig& r) {
    if (r.f.status().phase == FlashPhase::Validating) ++validatingSteps;
    if (validatingSteps > 0 && r.sim.resets.empty()) {
      const size_t d = r.img.bytesRead - prev;
      if (d > maxPerStep) maxPerStep = d;
    }
    prev = r.img.bytesRead;
  });
  CHECK(maxPerStep <= 1024 + 8);
  // Step 1 reads the vectors and the first 1 KiB, steps 2..512 the rest; the
  // last one already moves on to Resetting.
  CHECK(validatingSteps == 511);
  CHECK(rig.f.status().phase == FlashPhase::Done);
}

TEST_CASE("flasher: validation progress is reported 0..2 percent") {
  Rig rig(makeImage(64 * kKiB));
  REQUIRE(rig.begin());
  uint8_t maxValidating = 0;
  bool sawOne = false;
  rig.run([&](Rig& r) {
    if (r.f.status().phase == FlashPhase::Validating) {
      if (r.f.status().percent > maxValidating) maxValidating = r.f.status().percent;
      if (r.f.status().percent == 1) sawOne = true;
    }
  });
  CHECK(sawOne);
  CHECK(maxValidating <= 2);
}

TEST_CASE("flasher: percent milestones per phase") {
  Rig rig(makeImage(8 * kKiB));
  REQUIRE(rig.begin());
  struct Range {
    uint8_t lo = 255, hi = 0;
  };
  Range r[13];
  rig.run([&](Rig& g) {
    const auto p = static_cast<size_t>(g.f.status().phase);
    const uint8_t v = g.f.status().percent;
    if (v < r[p].lo) r[p].lo = v;
    if (v > r[p].hi) r[p].hi = v;
  });
  const auto R = [&](FlashPhase p) { return r[static_cast<size_t>(p)]; };
  CHECK(R(FlashPhase::Resetting).lo == 2);
  CHECK(R(FlashPhase::Handshake).lo == 3);
  CHECK(R(FlashPhase::Handshake).hi == 3);
  CHECK(R(FlashPhase::Sync).lo == 4);
  CHECK(R(FlashPhase::GetId).lo == 5);
  CHECK(R(FlashPhase::Erasing).hi == 5);
  CHECK(R(FlashPhase::Writing).lo >= 15);
  CHECK(R(FlashPhase::Writing).hi <= 75);
  CHECK(R(FlashPhase::Verifying).lo >= 75);
  CHECK(R(FlashPhase::Verifying).hi <= 95);
  CHECK(R(FlashPhase::Starting).lo == 95);
  CHECK(R(FlashPhase::WaitingApp).lo == 97);
  CHECK(R(FlashPhase::WaitingApp).hi == 98);
  CHECK(R(FlashPhase::Done).lo == 100);
}

TEST_CASE("flasher: exact percent and byte counters while writing and verifying") {
  // 8 KiB = 32 blocks. The last block's 75 % is set together with the switch
  // to Verifying, so Writing itself tops out at 15 + 60 * 31/32 = 73.
  Rig rig(makeImage(8 * kKiB));
  REQUIRE(rig.begin());
  uint8_t writeLo = 255, writeHi = 0, verifyLo = 255, verifyHi = 0;
  uint32_t writeBytesMax = 0, verifyBytesMax = 0;
  rig.run([&](Rig& g) {
    const FlashStatus& st = g.f.status();
    if (st.phase == FlashPhase::Writing) {
      writeLo = std::min(writeLo, st.percent);
      writeHi = std::max(writeHi, st.percent);
      writeBytesMax = std::max(writeBytesMax, st.bytesDone);
    }
    if (st.phase == FlashPhase::Verifying) {
      verifyLo = std::min(verifyLo, st.percent);
      verifyHi = std::max(verifyHi, st.percent);
      verifyBytesMax = std::max(verifyBytesMax, st.bytesDone);
    }
  });
  CHECK(rig.f.status().phase == FlashPhase::Done);
  CHECK(writeLo == 15);
  CHECK(writeHi == 73);
  CHECK(writeBytesMax == 31 * 256);
  CHECK(verifyLo == 75);
  CHECK(verifyHi == 94);  // 75 + 20 * 31/32
  CHECK(verifyBytesMax == 31 * 256);
}

TEST_CASE("flasher: blank mode reports 4 percent in Sync") {
  Rig rig(makeImage(2048));
  rig.opt.blank = true;
  rig.sim.hasBootLoop = false;
  rig.sim.bootPinResets = 1;
  REQUIRE(rig.begin());
  uint8_t syncLo = 255, syncHi = 0;
  rig.run([&](Rig& g) {
    if (g.f.status().phase == FlashPhase::Sync) {
      syncLo = std::min(syncLo, g.f.status().percent);
      syncHi = std::max(syncHi, g.f.status().percent);
    }
  });
  CHECK(rig.f.status().phase == FlashPhase::Done);
  CHECK(syncLo == 4);
  CHECK(syncHi == 4);
}

TEST_CASE("flasher: runs across the millis() wrap") {
  Rig rig(makeImage(4096));
  rig.now = 0xFFFFF000u;
  CHECK(rig.beginAndRun() == FlashPhase::Done);
  CHECK(rig.flashMatchesImage());
  CHECK(rig.f.status().startedMs == 0xFFFFF000u);
}

TEST_CASE("flasher: other bootloader baud") {
  Rig rig(makeImage(2048));
  rig.opt.baud = 57600;
  rig.sim.hasBootLoop = false;
  rig.sim.bootPinResets = 1;  // v1 window needs 115200: use blank mode
  rig.opt.blank = true;
  CHECK(rig.beginAndRun() == FlashPhase::Done);
  CHECK(rig.sim.configs[0] == std::make_pair(uint32_t{57600}, true));
  CHECK(rig.sim.configs[1] == std::make_pair(uint32_t{115200}, false));
}

TEST_CASE("flasher: normal mode handshakes at 115200 and then switches to the bootloader baud") {
  Rig rig(makeImage(2048));
  rig.opt.baud = 57600;
  REQUIRE(rig.begin());
  uint32_t switchedAt = 0;
  rig.run([&](Rig& r) {
    if (switchedAt == 0 && r.sim.configs.size() == 2) switchedAt = r.now;
  });
  CHECK(rig.f.status().phase == FlashPhase::Done);
  CHECK(rig.flashMatchesImage());
  REQUIRE(rig.sim.configs.size() == 3);
  CHECK(rig.sim.configs[0] == std::make_pair(uint32_t{115200}, true));
  CHECK(rig.sim.configs[1] == std::make_pair(uint32_t{57600}, true));
  CHECK(rig.sim.configs[2] == std::make_pair(uint32_t{115200}, false));
  // Switched once BEEFIT was seen, before the first 0x7F.
  REQUIRE(!rig.sim.syncTimes.empty());
  CHECK(switchedAt >= rig.sim.beefitAt);
  CHECK(switchedAt <= rig.sim.syncTimes[0]);
}

TEST_CASE("flasher: normal mode at 115200 configures the UART once for the bootloader") {
  Rig rig(makeImage(2048));
  CHECK(rig.beginAndRun() == FlashPhase::Done);
  REQUIRE(rig.sim.configs.size() == 2);
  CHECK(rig.sim.configs[0] == std::make_pair(uint32_t{115200}, true));
  CHECK(rig.sim.configs[1] == std::make_pair(uint32_t{115200}, false));
}

TEST_CASE("flasher: a second run after Done and after Failed") {
  Rig rig(makeImage(2048));
  CHECK(rig.beginAndRun() == FlashPhase::Done);
  rig.sim.programmed.clear();
  CHECK(rig.beginAndRun() == FlashPhase::Done);
  CHECK(rig.f.status().startedMs > 1000);
  CHECK(rig.sim.programmed.size() == 8);
  rig.img.data = {};
  CHECK(rig.beginAndRun() == FlashPhase::Failed);
  CHECK(rig.f.status().error == FlashError::ImageEmpty);
  rig.img.data = makeImage(2048);
  CHECK(rig.beginAndRun() == FlashPhase::Done);
  CHECK(rig.f.status().error == FlashError::None);
  CHECK(rig.f.status().chipPid == 0x431);
}

// ================================================================ validation failures

TEST_CASE("flasher: invalid images fail before the STM is touched") {
  const struct {
    std::vector<uint8_t> image;
    FlashError e;
  } cases[] = {
      {{}, FlashError::ImageEmpty},
      {std::vector<uint8_t>(7, 0), FlashError::ImageBadVectors},
      {makeImage(2048, 0x20000000u), FlashError::ImageBadVectors},
      {makeImage(2048, 0x20020000u, kBase + 2048 + 1), FlashError::ImageBadVectors},
      {makeImage(2048, 0x20020000u, kBase + 1, false), FlashError::ImageNoHandshake},
      {makeImage(512 * kKiB + 4, 0x20020000u, kBase + 1, false, ""), FlashError::ImageTooLarge},
  };
  for (const auto& c : cases) {
    Rig rig(c.image);
    REQUIRE(rig.begin());
    CHECK(rig.run() == FlashPhase::Failed);
    CHECK(rig.f.status().error == c.e);
    CHECK(rig.f.status().errorPhase == FlashPhase::Validating);
    CHECK(rig.f.status().finishedMs == rig.now);
    CHECK(rig.untouched());
    CHECK(rig.steps <= 3);
  }
}

TEST_CASE("flasher: image read error during validation reports the offset") {
  Rig rig(makeImage(4096));
  rig.img.failAt = 1500;
  REQUIRE(rig.begin());
  CHECK(rig.run() == FlashPhase::Failed);
  CHECK(rig.f.status().error == FlashError::ImageRead);
  CHECK(rig.f.status().errorAddress == 1280);
  CHECK(rig.untouched());
}

TEST_CASE("flasher: force flashes an image without handshake strings") {
  Rig rig(makeImage(2048, 0x20020000u, kBase + 1, false, ""));
  rig.opt.force = true;
  CHECK(rig.beginAndRun() == FlashPhase::Done);
  CHECK_FALSE(rig.f.status().image.hasHandshake);
  CHECK(rig.flashMatchesImage());
}

// ================================================================ handshake

TEST_CASE("flasher: handshake resyncs the v1 STM's fixed 8-byte matcher") {
  for (int stray = 0; stray < 8; ++stray) {
    CAPTURE(stray);
    Rig rig(makeImage(1024));
    rig.sim.stray = stray;
    CHECK(rig.beginAndRun() == FlashPhase::Done);
    // At most 8 sends are needed to realign.
    size_t sends = 0;
    for (auto& w : rig.sim.writes) sends += w.second == bytesOf("DEADBEEF\n") ? 1 : 0;
    CHECK(sends >= 1);
    CHECK(sends <= 8);
  }
}

TEST_CASE("flasher: bytes received before the first DEADBEEF are discarded") {
  Rig rig(makeImage(1024));
  rig.sim.bootNoise = "BEEFIT\r\n";  // stale bytes / reset glitch
  CHECK(rig.beginAndRun() == FlashPhase::Done);
  CHECK(rig.f.status().error == FlashError::None);
  REQUIRE_FALSE(rig.sim.syncTimes.empty());
  CHECK(rig.sim.syncTimes[0] - rig.sim.beefitAt >= 250);
}

TEST_CASE("flasher: BEEFIT behind noise and split over many reads") {
  Rig rig(makeImage(1024));
  rig.sim.beefitPrefix = "\xFF\x00zzBEEF";
  rig.sim.replySpacingMs = 3;
  CHECK(rig.beginAndRun() == FlashPhase::Done);
}

TEST_CASE("flasher: handshake timeout resets the STM back to its application") {
  Rig rig(makeImage(1024));
  rig.sim.hasBootLoop = false;  // running app has no BootLoop
  REQUIRE(rig.begin());
  CHECK(rig.run() == FlashPhase::Failed);
  const FlashStatus& st = rig.f.status();
  CHECK(st.error == FlashError::HandshakeTimeout);
  CHECK(st.errorPhase == FlashPhase::Handshake);
  const uint32_t release = rig.sim.resets[1].first;
  // Sends at +20, +120, ... +2420: 25 of them, none at/after 2500.
  std::vector<uint32_t> sends;
  for (auto& w : rig.sim.writes) {
    if (w.second == bytesOf("DEADBEEF\n")) sends.push_back(w.first - release);
  }
  REQUIRE(sends.size() == 25);
  CHECK(sends.front() >= 20);
  CHECK(sends.front() <= 22);
  for (size_t i = 1; i < sends.size(); ++i) {
    CHECK(sends[i] - sends[i - 1] >= 100);
    CHECK(sends[i] - sends[i - 1] <= 102);
  }
  CHECK(sends.back() < 2500);
  // Cleanup: another 100 ms pulse, UART 8N1.
  REQUIRE(rig.sim.resets.size() == 4);
  CHECK(rig.sim.resets[2].first - release >= 2500);
  CHECK(rig.sim.resets[2].first - release <= 2502);
  CHECK(rig.sim.resets[3].first - rig.sim.resets[2].first >= 100);
  CHECK(st.finishedMs == rig.sim.resets[3].first);
  CHECK(rig.leftClean());
  CHECK(rig.sim.programmed.empty());
  CHECK(rig.sim.eraseFrames.empty());
}

TEST_CASE("flasher: custom handshake timing options are honoured") {
  Rig rig(makeImage(1024));
  rig.sim.hasBootLoop = false;
  rig.opt.handshakeFirstMs = 50;
  rig.opt.handshakeRepeatMs = 300;
  rig.opt.handshakeWindowMs = 1000;
  rig.opt.resetPulseMs = 60;
  REQUIRE(rig.begin());
  CHECK(rig.run() == FlashPhase::Failed);
  const uint32_t release = rig.sim.resets[1].first;
  CHECK(release - rig.sim.resets[0].first >= 60);
  CHECK(release - rig.sim.resets[0].first <= 62);
  std::vector<uint32_t> sends;
  for (auto& w : rig.sim.writes) sends.push_back(w.first - release);
  REQUIRE(sends.size() == 4);  // 50, 350, 650, 950
  CHECK(sends[0] >= 50);
  CHECK(sends[0] <= 52);
  CHECK(sends[3] < 1000);
  CHECK(rig.sim.resets[2].first - release >= 1000);
  CHECK(rig.sim.resets[2].first - release <= 1002);
}

// ================================================================ blank mode

TEST_CASE("flasher: a first handshake due at the window end is never sent") {
  Rig rig(makeImage(1024));
  rig.opt.handshakeFirstMs = 2500;  // == handshakeWindowMs
  REQUIRE(rig.begin());
  CHECK(rig.run([](Rig&) {}, 60000, 1) == FlashPhase::Failed);
  CHECK(rig.f.status().error == FlashError::HandshakeTimeout);
  CHECK(rig.f.status().errorAddress == 0);
  CHECK(rig.sim.writes.empty());
  REQUIRE(rig.sim.resets.size() == 4);
  CHECK(rig.sim.resets[2].first - rig.sim.resets[1].first == 2500);
  CHECK(rig.leftClean());
}

TEST_CASE("flasher: blank mode skips the handshake") {
  Rig rig(makeImage(4096));
  rig.opt.blank = true;
  rig.sim.hasBootLoop = false;
  rig.sim.bootPinResets = 1;  // user holds BOOT0 for the first reset only
  CHECK(rig.beginAndRun() == FlashPhase::Done);
  for (auto& w : rig.sim.writes) CHECK(w.second != bytesOf("DEADBEEF\n"));
  REQUIRE(rig.sim.syncTimes.size() == 1);
  CHECK(rig.sim.syncTimes[0] - rig.sim.resets[1].first >= 250);
  CHECK(rig.sim.syncTimes[0] - rig.sim.resets[1].first <= 252);
  CHECK(rig.flashMatchesImage());
  CHECK(rig.f.status().percent == 100);
}

// ================================================================ sync

TEST_CASE("flasher: sync retries, NACK counts as synced, gives up after the attempts") {
  SUBCASE("second 0x7F answered") {
    Rig rig(makeImage(1024));
    rig.sim.syncSilent = 1;
    CHECK(rig.beginAndRun() == FlashPhase::Done);
    REQUIRE(rig.sim.syncTimes.size() == 2);
    CHECK(rig.sim.syncTimes[1] - rig.sim.syncTimes[0] >= 1000);
    CHECK(rig.sim.syncTimes[1] - rig.sim.syncTimes[0] <= 1002);
  }
  SUBCASE("third 0x7F answered") {
    Rig rig(makeImage(1024));
    rig.sim.syncSilent = 2;
    CHECK(rig.beginAndRun() == FlashPhase::Done);
  }
  SUBCASE("never answered") {
    Rig rig(makeImage(1024));
    rig.sim.syncSilent = 100;
    CHECK(rig.beginAndRun() == FlashPhase::Failed);
    CHECK(rig.f.status().error == FlashError::SyncFailed);
    CHECK(rig.f.status().errorPhase == FlashPhase::Sync);
    CHECK(rig.sim.syncTimes.size() == 3);
    CHECK(rig.leftClean());
  }
  SUBCASE("syncAttempts 0 behaves like 1") {
    Rig rig(makeImage(1024));
    rig.opt.syncAttempts = 0;
    rig.sim.syncSilent = 100;
    CHECK(rig.beginAndRun() == FlashPhase::Failed);
    CHECK(rig.f.status().error == FlashError::SyncFailed);
    CHECK(rig.sim.syncTimes.size() == 1);
  }
  SUBCASE("syncAttempts 5") {
    Rig rig(makeImage(1024));
    rig.opt.syncAttempts = 5;
    rig.sim.syncSilent = 4;
    CHECK(rig.beginAndRun() == FlashPhase::Done);
    CHECK(rig.sim.syncTimes.size() == 5);
  }
  SUBCASE("already synced bootloader NACKs 0x7F") {
    Rig rig(makeImage(1024));
    rig.sim.syncNack = true;
    CHECK(rig.beginAndRun() == FlashPhase::Done);
  }
  SUBCASE("noise before every ACK") {
    Rig rig(makeImage(1024));
    rig.sim.noiseReplies = 1000;
    CHECK(rig.beginAndRun() == FlashPhase::Done);
    CHECK(rig.flashMatchesImage());
  }
}

// ================================================================ GET / GET ID

TEST_CASE("flasher: GET is optional, GET ID is required and checked") {
  SUBCASE("silent GET") {
    Rig rig(makeImage(1024));
    rig.sim.getSilent = true;
    CHECK(rig.beginAndRun() == FlashPhase::Done);
    CHECK(rig.f.status().bootloaderVersion == 0);
  }
  SUBCASE("GET ID retried") {
    Rig rig(makeImage(1024));
    rig.sim.getIdSilent = 3;
    CHECK(rig.beginAndRun() == FlashPhase::Done);
    CHECK(rig.f.status().chipPid == 0x431);
  }
  SUBCASE("GET ID never answers") {
    Rig rig(makeImage(1024));
    rig.sim.getIdSilent = 4;
    CHECK(rig.beginAndRun() == FlashPhase::Failed);
    CHECK(rig.f.status().error == FlashError::Timeout);
    CHECK(rig.f.status().errorPhase == FlashPhase::GetId);
    CHECK(rig.sim.eraseFrames.empty());
    CHECK(rig.leftClean());
  }
  SUBCASE("GET ID reply without closing ACK") {
    Rig rig(makeImage(1024));
    rig.sim.getIdBadEnd = 4;
    CHECK(rig.beginAndRun() == FlashPhase::Failed);
    CHECK(rig.f.status().error == FlashError::Nack);
    CHECK(rig.f.status().errorPhase == FlashPhase::GetId);
  }
  SUBCASE("GET ID bad end once") {
    Rig rig(makeImage(1024));
    rig.sim.getIdBadEnd = 1;
    CHECK(rig.beginAndRun() == FlashPhase::Done);
  }
  SUBCASE("blockRetries 0: no GET ID retry") {
    Rig rig(makeImage(1024));
    rig.opt.blockRetries = 0;
    rig.sim.getIdSilent = 1;
    CHECK(rig.beginAndRun() == FlashPhase::Failed);
    CHECK(rig.f.status().error == FlashError::Timeout);
  }
  SUBCASE("unknown chip") {
    Rig rig(makeImage(1024));
    rig.sim.pid = 0x413;
    CHECK(rig.beginAndRun() == FlashPhase::Failed);
    CHECK(rig.f.status().error == FlashError::UnknownChip);
    CHECK(rig.f.status().chipPid == 0x413);
    CHECK(rig.sim.eraseFrames.empty());
    CHECK(rig.leftClean());
    CHECK(rig.sim.flash == rig.sim.original);
  }
  SUBCASE("one-byte PID reply") {
    Rig rig(makeImage(1024));
    rig.sim.pidN = 0;
    rig.sim.pid = 0x31;
    CHECK(rig.beginAndRun() == FlashPhase::Failed);
    CHECK(rig.f.status().error == FlashError::UnknownChip);
    CHECK(rig.f.status().chipPid == 0x31);
  }
  SUBCASE("longer PID reply uses the first two bytes") {
    Rig rig(makeImage(1024));
    rig.sim.pidN = 2;
    CHECK(rig.beginAndRun() == FlashPhase::Done);
    CHECK(rig.f.status().chipPid == 0x431);
  }
  SUBCASE("F411 image on an F401CC") {
    Rig rig(makeImage(1024, 0x20020000u));
    rig.sim.pid = 0x423;
    CHECK(rig.beginAndRun() == FlashPhase::Failed);
    CHECK(rig.f.status().error == FlashError::ImageChipMismatch);
    CHECK(rig.f.status().errorPhase == FlashPhase::GetId);
    CHECK(rig.sim.flash == rig.sim.original);
  }
  SUBCASE("image larger than the F401CC flash") {
    Rig rig(makeImage(300 * kKiB, 0x20010000u));
    rig.sim.pid = 0x423;
    CHECK(rig.beginAndRun() == FlashPhase::Failed);
    CHECK(rig.f.status().error == FlashError::ImageTooLarge);
  }
  SUBCASE("F401 image on F401xE and F411") {
    for (uint16_t pid : {0x423, 0x433, 0x431}) {
      Rig rig(makeImage(1024, 0x20010000u));
      rig.sim.pid = pid;
      CHECK(rig.beginAndRun() == FlashPhase::Done);
      CHECK(rig.f.status().chipPid == pid);
    }
  }
}

// ================================================================ erase

TEST_CASE("flasher: erase failures repeat the session and then fail") {
  SUBCASE("NACK once") {
    Rig rig(makeImage(1024));
    rig.sim.nackErase = 1;
    CHECK(rig.beginAndRun() == FlashPhase::Done);
    CHECK(rig.f.status().attempt == 1);
  }
  SUBCASE("NACK always (write protection)") {
    Rig rig(makeImage(1024));
    rig.sim.nackErase = 100;
    CHECK(rig.beginAndRun() == FlashPhase::Failed);
    const FlashStatus& st = rig.f.status();
    CHECK(st.error == FlashError::Nack);
    CHECK(st.errorPhase == FlashPhase::Erasing);
    CHECK(st.errorAddress == kBase);
    CHECK(st.attempt == 2);
    CHECK(rig.sim.writesEqual({0x44, 0xBB}) == 3);
    CHECK(rig.leftClean());
  }
  SUBCASE("sessionRetries 0") {
    Rig rig(makeImage(1024));
    rig.opt.sessionRetries = 0;
    rig.sim.nackErase = 1;
    CHECK(rig.beginAndRun() == FlashPhase::Failed);
    CHECK(rig.sim.writesEqual({0x44, 0xBB}) == 1);
  }
  SUBCASE("erase done ACK lost") {
    Rig rig(makeImage(1024));
    rig.opt.eraseTimeoutMs = 5000;
    rig.sim.dropEraseAck = 1;
    uint32_t firstErase = 0, secondErase = 0;
    REQUIRE(rig.begin());
    rig.run([&](Rig& r) {
      if (r.sim.eraseFrames.size() == 1 && firstErase == 0) firstErase = r.now;
      if (r.sim.eraseFrames.size() == 2 && secondErase == 0) secondErase = r.now;
    });
    CHECK(rig.f.status().phase == FlashPhase::Done);
    CHECK(secondErase - firstErase >= 5000);
    CHECK(secondErase - firstErase <= 5010);
  }
  SUBCASE("erase waits past the ACK timeout") {
    Rig rig(makeImage(1024));
    rig.sim.eraseDelayMs = 20000;  // > ackTimeoutMs, < eraseTimeoutMs
    CHECK(rig.beginAndRun() == FlashPhase::Done);
    CHECK(rig.f.status().attempt == 0);
  }
}

// ================================================================ write

TEST_CASE("flasher: write retries per block and per session") {
  SUBCASE("NACK once") {
    Rig rig(makeImage(2048));
    rig.sim.nackWriteData = 1;
    CHECK(rig.beginAndRun() == FlashPhase::Done);
    CHECK(rig.f.status().attempt == 0);
    CHECK(rig.sim.writesEqual({0x31, 0xCE}) == 9);
  }
  SUBCASE("NACK three times: still the same session") {
    Rig rig(makeImage(2048));
    rig.sim.nackWriteData = 3;
    CHECK(rig.beginAndRun() == FlashPhase::Done);
    CHECK(rig.f.status().attempt == 0);
    CHECK(rig.sim.eraseFrames.size() == 1);
  }
  SUBCASE("NACK four times: re-erase in the same ROM session") {
    Rig rig(makeImage(2048));
    rig.sim.nackWriteData = 4;
    CHECK(rig.beginAndRun() == FlashPhase::Done);
    CHECK(rig.f.status().attempt == 1);
    CHECK(rig.sim.eraseFrames.size() == 2);
    CHECK(rig.sim.resets.size() == 4);  // no extra reset between sessions
    CHECK(rig.flashMatchesImage());
    CHECK(rig.percentMonotonic());
  }
  SUBCASE("lost data ACK") {
    Rig rig(makeImage(2048));
    rig.sim.dropWriteAck = 2;
    CHECK(rig.beginAndRun() == FlashPhase::Done);
    CHECK(rig.flashMatchesImage());
  }
  SUBCASE("a block that never programs") {
    Rig rig(makeImage(2048));
    rig.sim.nackWriteAddr.insert(kBase + 0x300);
    CHECK(rig.beginAndRun() == FlashPhase::Failed);
    const FlashStatus& st = rig.f.status();
    CHECK(st.error == FlashError::Nack);
    CHECK(st.errorPhase == FlashPhase::Writing);
    CHECK(st.errorAddress == kBase + 0x300);
    CHECK(st.attempt == 2);
    CHECK(rig.sim.eraseFrames.size() == 3);
    // 3 sessions x (1 + 3 retries) attempts at that block.
    size_t tries = 0;
    for (auto& w : rig.sim.writes) {
      tries += w.second == std::vector<uint8_t>{0x08, 0x00, 0x03, 0x00, 0x0B} ? 1 : 0;
    }
    CHECK(tries == 12);
    // Block 0 (vector table) was never written: the chip stays "blank".
    for (auto& p : rig.sim.programmed) CHECK(p.addr != kBase);
    CHECK(rig.leftClean());
  }
  SUBCASE("readout protection") {
    Rig rig(makeImage(2048));
    rig.sim.rdp = true;
    CHECK(rig.beginAndRun() == FlashPhase::Failed);
    CHECK(rig.f.status().error == FlashError::Nack);
    CHECK(rig.f.status().errorPhase == FlashPhase::Erasing);
  }
  SUBCASE("data ACK timeout is ackTimeoutMs plus the frame's wire time") {
    Rig rig(makeImage(1024));
    rig.opt.ackTimeoutMs = 300;
    rig.sim.dropWriteAck = 1;
    std::vector<uint32_t> cmdTimes;
    REQUIRE(rig.begin());
    size_t seen = 0;
    rig.run([&](Rig& r) {
      for (; seen < r.sim.writes.size(); ++seen) {
        if (r.sim.writes[seen].second == std::vector<uint8_t>{0x31, 0xCE}) {
          cmdTimes.push_back(r.sim.writes[seen].first);
        }
      }
    });
    CHECK(rig.f.status().phase == FlashPhase::Done);
    REQUIRE(cmdTimes.size() >= 2);
    // 258 bytes x 11 bits at 115200 = 24.6 -> 25 ms.
    CHECK(cmdTimes[1] - cmdTimes[0] >= 325);
    CHECK(cmdTimes[1] - cmdTimes[0] <= 345);
  }
  SUBCASE("slow baud: a data frame still on the wire is not timed out") {
    // 258 bytes x 11 bits at 1200 baud = 2365 ms, more than ackTimeoutMs.
    Rig rig(makeImage(1024));
    rig.opt.baud = 1200;
    rig.opt.blank = true;
    rig.opt.ackTimeoutMs = 300;
    rig.sim.hasBootLoop = false;
    rig.sim.bootPinResets = 1;
    rig.sim.dropWriteAck = 1;
    std::vector<uint32_t> cmdTimes;
    REQUIRE(rig.begin());
    size_t seen = 0;
    rig.run([&](Rig& r) {
      for (; seen < r.sim.writes.size(); ++seen) {
        if (r.sim.writes[seen].second == std::vector<uint8_t>{0x31, 0xCE}) {
          cmdTimes.push_back(r.sim.writes[seen].first);
        }
      }
    });
    CHECK(rig.f.status().phase == FlashPhase::Done);
    CHECK(rig.flashMatchesImage());
    REQUIRE(cmdTimes.size() >= 2);
    // 300 ms + 2365 ms wire time of the data frame.
    CHECK(cmdTimes[1] - cmdTimes[0] >= 2665);
    CHECK(cmdTimes[1] - cmdTimes[0] <= 2700);
  }
}

// ================================================================ verify

TEST_CASE("flasher: verify compares bytes and reports the first bad address") {
  SUBCASE("transient read corruption is retried") {
    Rig rig(makeImage(2048));
    rig.sim.corruptReads = 1;
    CHECK(rig.beginAndRun() == FlashPhase::Done);
    CHECK(rig.sim.readAddrs.size() == 9);
    CHECK(rig.sim.readAddrs[0] == kBase);
    CHECK(rig.sim.readAddrs[1] == kBase);
  }
  SUBCASE("stuck flash bit") {
    Rig rig(makeImage(2048));
    rig.sim.stuck.insert(kBase + 0x345);
    CHECK(rig.beginAndRun() == FlashPhase::Failed);
    const FlashStatus& st = rig.f.status();
    CHECK(st.error == FlashError::VerifyMismatch);
    CHECK(st.errorPhase == FlashPhase::Verifying);
    CHECK(st.errorAddress == kBase + 0x345);
    CHECK(st.attempt == 2);
    CHECK(rig.leftClean());
  }
  SUBCASE("stuck bit in the first and last byte of a block") {
    for (uint32_t a : {kBase + 0x100u, kBase + 0x1FFu, kBase + 0x7FFu}) {
      Rig rig(makeImage(2048));
      rig.opt.sessionRetries = 0;
      rig.opt.blockRetries = 0;
      rig.sim.stuck.insert(a);
      CHECK(rig.beginAndRun() == FlashPhase::Failed);
      CHECK(rig.f.status().errorAddress == a);
    }
  }
  SUBCASE("image changed on disk after writing") {
    Rig rig(makeImage(2048));
    REQUIRE(rig.begin());
    bool changed = false;
    rig.run([&](Rig& r) {
      if (!changed && r.f.status().phase == FlashPhase::Verifying) {
        // Same change on disk and in flash: bytes compare equal, CRC does not.
        r.img.data[1500] ^= 0x10;
        r.sim.flash[1500] = r.img.data[1500];
        changed = true;
      }
    });
    CHECK(rig.f.status().phase == FlashPhase::Failed);
    CHECK(rig.f.status().error == FlashError::ImageRead);
    CHECK(rig.f.status().errorPhase == FlashPhase::Verifying);
    CHECK(rig.leftClean());
  }
  SUBCASE("read data slower than the ACK timeout still fits the data budget") {
    // 257 reply bytes 1 ms apart: 50 ms ACK + 25 ms wire + 200 ms margin.
    Rig rig(makeImage(1024));
    rig.opt.ackTimeoutMs = 50;
    rig.sim.replySpacingMs = 1;
    CHECK(rig.beginAndRun() == FlashPhase::Done);
    CHECK(rig.sim.readAddrs.size() == 4);
  }
  SUBCASE("read data beyond the data budget times out") {
    Rig rig(makeImage(1024));
    rig.opt.ackTimeoutMs = 50;
    rig.opt.blockRetries = 0;
    rig.opt.sessionRetries = 0;
    rig.sim.replySpacingMs = 2;
    CHECK(rig.beginAndRun() == FlashPhase::Failed);
    CHECK(rig.f.status().error == FlashError::Timeout);
    CHECK(rig.f.status().errorPhase == FlashPhase::Verifying);
    CHECK(rig.f.status().errorAddress == kBase);
  }
}

TEST_CASE("flasher: image read failure during writing") {
  Rig rig(makeImage(2048));
  REQUIRE(rig.begin());
  rig.run([&](Rig& r) {
    if (r.f.status().phase == FlashPhase::Erasing) r.img.failAt = 1030;
  });
  CHECK(rig.f.status().phase == FlashPhase::Failed);
  CHECK(rig.f.status().error == FlashError::ImageRead);
  CHECK(rig.f.status().errorPhase == FlashPhase::Writing);
  CHECK(rig.f.status().errorAddress == kBase + 0x400);
  CHECK(rig.leftClean());
}

// ================================================================ transport

TEST_CASE("flasher: short transport writes fail and still clean up") {
  SUBCASE("during the handshake") {
    Rig rig(makeImage(1024));
    rig.sim.writeLimit = 4;
    CHECK(rig.beginAndRun() == FlashPhase::Failed);
    CHECK(rig.f.status().error == FlashError::TransportWrite);
    CHECK(rig.f.status().errorPhase == FlashPhase::Handshake);
    CHECK(rig.sim.writes.size() == 1);
    CHECK(rig.leftClean());
  }
  SUBCASE("on the first data frame") {
    Rig rig(makeImage(1024));
    rig.sim.writeLimit = 200;
    CHECK(rig.beginAndRun() == FlashPhase::Failed);
    CHECK(rig.f.status().error == FlashError::TransportWrite);
    CHECK(rig.f.status().errorPhase == FlashPhase::Writing);
    CHECK(rig.leftClean());
  }
  SUBCASE("while polling the application") {
    Rig rig(makeImage(1024));
    REQUIRE(rig.begin());
    rig.run([](Rig& r) {
      if (r.f.status().phase == FlashPhase::WaitingApp) r.sim.writeLimit = 3;
    });
    CHECK(rig.f.status().phase == FlashPhase::Failed);
    CHECK(rig.f.status().error == FlashError::TransportWrite);
    CHECK(rig.f.status().errorPhase == FlashPhase::WaitingApp);
    CHECK(rig.sim.resets.size() == 4);  // no extra pulse: the app already runs
  }
}

// ================================================================ abort

TEST_CASE("flasher: abort") {
  SUBCASE("while validating: nothing touched") {
    Rig rig(makeImage(64 * kKiB));
    REQUIRE(rig.begin());
    rig.now += 2;
    rig.f.step(rig.now);
    rig.f.abort();
    CHECK(rig.f.active());
    rig.now += 2;
    CHECK(rig.f.step(rig.now) == FlashPhase::Failed);
    CHECK(rig.f.status().error == FlashError::Aborted);
    CHECK(rig.f.status().errorPhase == FlashPhase::Validating);
    CHECK(rig.untouched());
  }
  SUBCASE("while writing: pulse, 8N1, then Failed") {
    Rig rig(makeImage(8 * kKiB));
    REQUIRE(rig.begin());
    bool aborted = false;
    rig.run([&](Rig& r) {
      if (!aborted && r.f.status().phase == FlashPhase::Writing && r.sim.programmed.size() == 3) {
        r.f.abort();
        aborted = true;
      }
    });
    const FlashStatus& st = rig.f.status();
    CHECK(st.phase == FlashPhase::Failed);
    CHECK(st.error == FlashError::Aborted);
    CHECK(st.errorPhase == FlashPhase::Writing);
    CHECK(rig.sim.programmed.size() <= 4);
    CHECK(rig.leftClean());
    REQUIRE(rig.sim.resets.size() == 4);
    CHECK(rig.sim.resets[3].first - rig.sim.resets[2].first >= 100);
  }
  SUBCASE("during the cleanup pulse it is ignored") {
    Rig rig(makeImage(1024));
    rig.sim.hasBootLoop = false;
    REQUIRE(rig.begin());
    bool once = false;
    rig.run([&](Rig& r) {
      if (!once && r.f.status().error == FlashError::HandshakeTimeout) {
        r.f.abort();
        once = true;
      }
    });
    CHECK(rig.f.status().error == FlashError::HandshakeTimeout);
    CHECK(rig.sim.resets.size() == 4);
  }
  SUBCASE("while waiting for the application: no extra pulse") {
    Rig rig(makeImage(1024));
    REQUIRE(rig.begin());
    rig.run([](Rig& r) {
      if (r.f.status().phase == FlashPhase::WaitingApp) r.f.abort();
    });
    CHECK(rig.f.status().phase == FlashPhase::Failed);
    CHECK(rig.f.status().error == FlashError::Aborted);
    CHECK(rig.f.status().errorPhase == FlashPhase::WaitingApp);
    CHECK(rig.sim.resets.size() == 4);
    CHECK(rig.leftClean());
  }
  SUBCASE("abort before begin does not leak into the next run") {
    Rig rig(makeImage(1024));
    rig.f.abort();
    CHECK(rig.beginAndRun() == FlashPhase::Done);
  }
  SUBCASE("abort after Done does nothing") {
    Rig rig(makeImage(1024));
    CHECK(rig.beginAndRun() == FlashPhase::Done);
    rig.f.abort();
    CHECK(rig.f.step(rig.now + 2) == FlashPhase::Done);
    CHECK(rig.f.status().error == FlashError::None);
  }
}

// ================================================================ application

TEST_CASE("flasher: waiting for the new application") {
  SUBCASE("no answer: gvers at 4 s then every 1 s until 15 s") {
    Rig rig(makeImage(1024));
    rig.sim.appAnswers = false;
    rig.opt.appTimeoutMs = 15000;
    CHECK(rig.beginAndRun() == FlashPhase::Failed);
    CHECK(rig.f.status().error == FlashError::AppNotResponding);
    CHECK(rig.f.status().errorPhase == FlashPhase::WaitingApp);
    const uint32_t release = rig.sim.resets.back().first;
    REQUIRE(rig.sim.gversTimes.size() == 11);
    for (size_t i = 0; i < 11; ++i) {
      CHECK(rig.sim.gversTimes[i] - release >= 4000 + 1000 * i);
      CHECK(rig.sim.gversTimes[i] - release <= 4000 + 1000 * i + 2 * (i + 1));
    }
    CHECK(rig.f.status().finishedMs - release >= 15000);
    CHECK(rig.f.status().finishedMs - release <= 15002);
    CHECK(rig.sim.resets.size() == 4);
    CHECK(rig.leftClean());
    CHECK(rig.flashMatchesImage());
  }
  SUBCASE("late answer within the budget") {
    Rig rig(makeImage(1024));
    rig.sim.appAnswers = false;
    REQUIRE(rig.begin());
    rig.run([](Rig& r) {
      if (r.sim.gversTimes.size() == 5) r.sim.appAnswers = true;
    });
    CHECK(rig.f.status().phase == FlashPhase::Done);
    CHECK(rig.sim.gversTimes.size() == 6);
  }
  SUBCASE("version mismatch") {
    Rig rig(makeImage(1024));
    rig.sim.appReply = "gvers 1.4.8_Dev_C1 1 ";
    CHECK(rig.beginAndRun() == FlashPhase::Failed);
    CHECK(rig.f.status().error == FlashError::AppVersionMismatch);
    CHECK(rig.f.status().appVersion.patch == 8);
    CHECK(rig.sim.resets.size() == 4);
  }
  SUBCASE("suffix mismatch") {
    Rig rig(makeImage(1024));
    rig.sim.appReply = "gvers 1.4.9_C1 1 ";
    CHECK(rig.beginAndRun() == FlashPhase::Failed);
    CHECK(rig.f.status().error == FlashError::AppVersionMismatch);
  }
  SUBCASE("force skips the version check") {
    Rig rig(makeImage(1024));
    rig.opt.force = true;
    rig.sim.appReply = "gvers 1.4.8_Dev_C1 1 ";
    CHECK(rig.beginAndRun() == FlashPhase::Done);
    CHECK(rig.f.status().appVersion.patch == 8);
  }
  SUBCASE("image without a version string accepts any version") {
    Rig rig(makeImage(1024, 0x20020000u, 0, true, ""));
    rig.sim.appReply = "gvers 7.0.1-revamped_C2 ";
    CHECK(rig.beginAndRun() == FlashPhase::Done);
    CHECK(rig.f.status().appVersion.major == 7);
  }
  SUBCASE("image version with a hw tag must match it") {
    Rig a(makeImage(1024, 0x20020000u, 0, true, "1.4.9_C1"));
    a.sim.appReply = "gvers 1.4.9_C2 1 ";
    CHECK(a.beginAndRun() == FlashPhase::Failed);
    CHECK(a.f.status().error == FlashError::AppVersionMismatch);
    Rig b(makeImage(1024, 0x20020000u, 0, true, "1.4.9_C1"));
    b.sim.appReply = "gvers 1.4.9_C1 1 ";
    CHECK(b.beginAndRun() == FlashPhase::Done);
  }
  SUBCASE("revamped version without build") {
    Rig rig(makeImage(1024, 0x20020000u, 0, true, "2.0.0-revamped"));
    rig.sim.appReply = "gvers 2.0.0-revamped_C2 ";
    CHECK(rig.beginAndRun() == FlashPhase::Done);
  }
  SUBCASE("noise and other lines before the reply are ignored") {
    Rig rig(makeImage(1024));
    rig.sim.appNoise = std::string("\xFF\xFE garbage\r\n", 12) + "gvlst 12 1,1,\r\n" +
                       "gversX 9.9.9\r\n" + "gvers\r\n" + "gvers   \r\n" +
                       std::string(400, 'z') + "\r\n" + "gvers 1.4\r\n" +
                       std::string("gvers 1.4.9_Dev_C1\x01\r\n", 21);
    CHECK(rig.beginAndRun() == FlashPhase::Done);
    CHECK(std::string(rig.f.status().appVersion.suffix) == "_Dev");
  }
  SUBCASE("custom app timing") {
    Rig rig(makeImage(1024));
    rig.sim.appAnswers = false;
    rig.opt.appBootMs = 500;
    rig.opt.appPollMs = 250;
    rig.opt.appTimeoutMs = 1500;
    CHECK(rig.beginAndRun() == FlashPhase::Failed);
    CHECK(rig.sim.writesEqual(bytesOf("gvers \r\n")) == 4);  // 500, 750, 1000, 1250
    CHECK(rig.f.status().finishedMs - rig.sim.resets.back().first == 1500);
  }
}

// ================================================================ exact timing

TEST_CASE("flasher: exact timing with 1 ms steps") {
  SUBCASE("successful run") {
    Rig rig(makeImage(1024));
    REQUIRE(rig.begin());
    CHECK(rig.run([](Rig&) {}, 60000, 1) == FlashPhase::Done);
    REQUIRE(rig.sim.resets.size() == 4);
    const uint32_t release = rig.sim.resets[1].first;
    CHECK(release - rig.sim.resets[0].first == 100);
    uint32_t firstHandshake = 0;
    for (auto& w : rig.sim.writes) {
      if (w.second == bytesOf("DEADBEEF\n")) {
        firstHandshake = w.first;
        break;
      }
    }
    CHECK(firstHandshake - release == 20);
    REQUIRE_FALSE(rig.sim.syncTimes.empty());
    CHECK(rig.sim.syncTimes[0] - rig.sim.beefitAt == 250);
    CHECK(rig.sim.resets[3].first - rig.sim.resets[2].first == 100);
    REQUIRE(rig.sim.gversTimes.size() == 1);
    CHECK(rig.sim.gversTimes[0] - rig.sim.resets[3].first == 4000);
  }
  SUBCASE("handshake window") {
    Rig rig(makeImage(1024));
    rig.sim.hasBootLoop = false;
    REQUIRE(rig.begin());
    CHECK(rig.run([](Rig&) {}, 60000, 1) == FlashPhase::Failed);
    CHECK(rig.f.status().error == FlashError::HandshakeTimeout);
    REQUIRE(rig.sim.resets.size() == 4);
    const uint32_t release = rig.sim.resets[1].first;
    std::vector<uint32_t> sends;
    for (auto& w : rig.sim.writes) {
      if (w.second == bytesOf("DEADBEEF\n")) sends.push_back(w.first - release);
    }
    REQUIRE(sends.size() == 25);
    for (size_t i = 0; i < sends.size(); ++i) CHECK(sends[i] == 20 + 100 * i);
    CHECK(rig.sim.resets[2].first - release == 2500);
    CHECK(rig.sim.resets[3].first - rig.sim.resets[2].first == 100);
    CHECK(rig.f.status().finishedMs == rig.sim.resets[3].first);
  }
  SUBCASE("sync retry after ackTimeoutMs plus 1 ms wire time") {
    Rig rig(makeImage(1024));
    rig.sim.syncSilent = 1;
    REQUIRE(rig.begin());
    CHECK(rig.run([](Rig&) {}, 60000, 1) == FlashPhase::Done);
    REQUIRE(rig.sim.syncTimes.size() == 2);
    CHECK(rig.sim.syncTimes[1] - rig.sim.syncTimes[0] == 1001);
  }
  SUBCASE("application polling") {
    Rig rig(makeImage(1024));
    rig.sim.appAnswers = false;
    rig.opt.appTimeoutMs = 15000;
    REQUIRE(rig.begin());
    CHECK(rig.run([](Rig&) {}, 60000, 1) == FlashPhase::Failed);
    const uint32_t release = rig.sim.resets.back().first;
    REQUIRE(rig.sim.gversTimes.size() == 11);
    for (size_t i = 0; i < 11; ++i) CHECK(rig.sim.gversTimes[i] - release == 4000 + 1000 * i);
    CHECK(rig.f.status().finishedMs - release == 15000);
  }
  SUBCASE("read-back data budget at 1200 baud, both sides of the limit") {
    // Per read: ACK at +2 ms, then 256 more bytes 11 ms apart: last at +2818.
    // Budget: ackTimeoutMs + wire(2 B) 19 + wire(257 B) 2356 + 200.
    for (const uint16_t ack : {uint16_t{243}, uint16_t{242}}) {
      CAPTURE(ack);
      Rig rig(makeImage(1024));
      rig.opt.baud = 1200;
      rig.opt.ackTimeoutMs = ack;
      rig.opt.blockRetries = 0;
      rig.opt.sessionRetries = 0;
      rig.sim.replySpacingMs = 11;
      REQUIRE(rig.begin());
      rig.run([](Rig&) {}, 120000, 1);
      if (ack == 243) {
        CHECK(rig.f.status().phase == FlashPhase::Done);
        CHECK(rig.sim.readAddrs.size() == 4);
      } else {
        CHECK(rig.f.status().phase == FlashPhase::Failed);
        CHECK(rig.f.status().error == FlashError::Timeout);
        CHECK(rig.f.status().errorPhase == FlashPhase::Verifying);
        CHECK(rig.f.status().errorAddress == kBase);
      }
    }
  }
}

TEST_CASE("flasher: instant replies chain exactly one block per step") {
  Rig rig(makeImage(16 * kKiB));  // 64 blocks
  rig.sim.instantReplies = true;
  REQUIRE(rig.begin());
  size_t seen = 0, maxFrames = 0;
  std::vector<uint32_t> dataTimes, readTimes;
  rig.run(
      [&](Rig& r) {
        size_t frames = 0;
        for (; seen < r.sim.writes.size(); ++seen) {
          const auto& w = r.sim.writes[seen];
          if (w.second.size() > 16) {
            ++frames;
            dataTimes.push_back(w.first);
          }
          if (w.second == std::vector<uint8_t>{0xFF, 0x00}) readTimes.push_back(w.first);
        }
        maxFrames = std::max(maxFrames, frames);
      },
      60000, 1);
  CHECK(rig.f.status().phase == FlashPhase::Done);
  CHECK(maxFrames == 1);
  REQUIRE(dataTimes.size() == 64);
  REQUIRE(readTimes.size() == 64);
  CHECK(dataTimes.back() - dataTimes.front() == 63);
  CHECK(readTimes.back() - readTimes.front() == 63);
}

// ================================================================ more failure detail

TEST_CASE("flasher: failures without a flash address report address 0") {
  struct Case {
    const char* name;
    FlashError error;
    std::function<void(Rig&)> setup;
    std::function<void(Rig&)> hook;
  };
  const auto none = [](Rig&) {};
  const std::vector<Case> cases = {
      {"empty image", FlashError::ImageEmpty, [](Rig& r) { r.img.data.clear(); }, none},
      {"no handshake strings", FlashError::ImageNoHandshake,
       [](Rig& r) { r.img.data = makeImage(1024, 0x20020000u, 0, false); }, none},
      {"handshake timeout", FlashError::HandshakeTimeout,
       [](Rig& r) { r.sim.hasBootLoop = false; }, none},
      {"sync failed", FlashError::SyncFailed, [](Rig& r) { r.sim.syncSilent = 100; }, none},
      {"GET ID timeout", FlashError::Timeout, [](Rig& r) { r.sim.getIdSilent = 100; }, none},
      {"GET ID NACK", FlashError::Nack, [](Rig& r) { r.sim.getIdBadEnd = 100; }, none},
      {"unknown chip", FlashError::UnknownChip, [](Rig& r) { r.sim.pid = 0x413; }, none},
      {"short write", FlashError::TransportWrite, [](Rig& r) { r.sim.writeLimit = 4; }, none},
      {"abort while writing", FlashError::Aborted, none,
       [](Rig& r) {
         if (r.f.status().phase == FlashPhase::Writing) r.f.abort();
       }},
      {"image changed during the run", FlashError::ImageRead, none,
       [changed = std::make_shared<bool>(false)](Rig& r) {
         // Same change on disk and in flash: bytes compare equal, CRC does not.
         if (!*changed && r.f.status().phase == FlashPhase::Verifying) {
           r.img.data[700] ^= 0x10;
           r.sim.flash[700] = r.img.data[700];
           *changed = true;
         }
       }},
      {"application silent", FlashError::AppNotResponding,
       [](Rig& r) { r.sim.appAnswers = false; }, none},
      {"application version", FlashError::AppVersionMismatch,
       [](Rig& r) { r.sim.appReply = "gvers 1.4.8_Dev_C1 1 "; }, none},
  };
  for (const Case& c : cases) {
    const std::string name = c.name;
    CAPTURE(name);
    Rig rig(makeImage(1024));
    c.setup(rig);
    REQUIRE(rig.begin());
    CHECK(rig.run(c.hook) == FlashPhase::Failed);
    CHECK(rig.f.status().error == c.error);
    CHECK(rig.f.status().errorAddress == 0);
    CHECK(rig.leftClean() == !rig.sim.resets.empty());
  }
}

TEST_CASE("flasher: verify retries are counted per block") {
  // One corrupted read-back on two different blocks with blockRetries 1:
  // each block has its own retry, so no session retry is needed.
  Rig rig(makeImage(2048));
  rig.opt.blockRetries = 1;
  rig.sim.corruptReadAt = {kBase + 0x100, kBase + 0x300};
  CHECK(rig.beginAndRun() == FlashPhase::Done);
  CHECK(rig.f.status().attempt == 0);
  CHECK(rig.sim.eraseFrames.size() == 1);
  CHECK(rig.sim.readAddrs.size() == 10);
}

TEST_CASE("flasher: BEEFIT detection") {
  SUBCASE("behind noise in the same read") {
    Rig rig(makeImage(1024));
    rig.sim.beefitPrefix = "zzBEEF";
    CHECK(rig.beginAndRun() == FlashPhase::Done);
  }
  SUBCASE("split across two answers") {
    Rig rig(makeImage(1024));
    rig.sim.hasBootLoop = false;
    rig.sim.bootPinResets = 1;  // already in the ROM bootloader; the echo plays BEEFIT
    rig.sim.echoes = {"xxBEE", "FIT\r\n"};
    CHECK(rig.beginAndRun() == FlashPhase::Done);
    CHECK(rig.sim.writesEqual(bytesOf("DEADBEEF\n")) == 2);
  }
  SUBCASE("near misses never match") {
    Rig rig(makeImage(1024));
    rig.sim.hasBootLoop = false;
    rig.sim.echoRepeat = "BEEFIxBEEFI\r\nBEEFT beefit";
    CHECK(rig.beginAndRun() == FlashPhase::Failed);
    CHECK(rig.f.status().error == FlashError::HandshakeTimeout);
    CHECK(rig.sim.syncTimes.empty());
  }
}

TEST_CASE("flasher: application reply parsing details") {
  SUBCASE("a wrong version on a line with a control character is dropped") {
    Rig rig(makeImage(1024));
    rig.sim.appNoise = "\x01gvers 1.4.8_Dev_C1 1 \r\ngvers 1.4.8\x02_Dev_C1 1 \r\n";
    CHECK(rig.beginAndRun() == FlashPhase::Done);
    CHECK(rig.f.status().appVersion.patch == 9);
  }
  SUBCASE("'~' is printable") {
    Rig rig(makeImage(1024));
    rig.sim.appReply = "gvers 1.4.9_Dev_C1 1~ ";
    CHECK(rig.beginAndRun() == FlashPhase::Done);
  }
  SUBCASE("extra spaces before the version") {
    Rig rig(makeImage(1024));
    rig.sim.appReply = "gvers   1.4.9_Dev_C1 1 ";
    CHECK(rig.beginAndRun() == FlashPhase::Done);
  }
  SUBCASE("boot noise without a line end is discarded before the first gvers") {
    Rig rig(makeImage(1024));
    rig.sim.bootNoise = "zz";
    CHECK(rig.beginAndRun() == FlashPhase::Done);
    CHECK(rig.sim.gversTimes.size() == 1);
  }
}

// ================================================================ fuzz

TEST_CASE("flasher: fixed-seed fault fuzz always ends clean") {
  Lcg r(0xF1A5u);
  int done = 0, failed = 0;
  for (int iter = 0; iter < 120; ++iter) {
    CAPTURE(iter);
    const size_t size = 128 + 4 * r.below(700);
    Rig rig(makeImage(size, 0x20020000u, kBase + 1, true, "1.4.9_Dev", iter + 1));
    rig.opt.ackTimeoutMs = 200;
    rig.opt.eraseTimeoutMs = 2000;
    rig.sim.stray = static_cast<int>(r.below(8));
    rig.sim.syncSilent = static_cast<int>(r.below(4));
    rig.sim.getIdSilent = static_cast<int>(r.below(3));
    rig.sim.nackErase = static_cast<int>(r.below(3));
    rig.sim.dropEraseAck = static_cast<int>(r.below(2));
    rig.sim.nackWriteData = static_cast<int>(r.below(6));
    rig.sim.dropWriteAck = static_cast<int>(r.below(3));
    rig.sim.corruptReads = static_cast<int>(r.below(3));
    rig.sim.corruptOffset = 0;
    rig.sim.noiseReplies = static_cast<int>(r.below(20));
    rig.sim.replySpacingMs = r.below(2);
    rig.sim.eraseDelayMs = 50 + r.below(500);
    if (r.below(10) == 0) rig.sim.stuck.insert(kBase + r.below(static_cast<uint32_t>(size)));
    if (r.below(10) == 0) rig.sim.appAnswers = false;
    const FlashPhase end = rig.beginAndRun();
    REQUIRE((end == FlashPhase::Done || end == FlashPhase::Failed));
    const std::string why = std::string(flashErrorName(rig.f.status().error)) + " in " +
                            flashPhaseName(rig.f.status().errorPhase);
    INFO(why);
    CHECK(rig.leftClean());
    CHECK(rig.percentMonotonic());
    CHECK(rig.f.status().finishedMs == rig.now);
    if (end == FlashPhase::Done) {
      ++done;
      CHECK(rig.flashMatchesImage());
      CHECK(rig.f.status().error == FlashError::None);
    } else {
      ++failed;
      CHECK(rig.f.status().error != FlashError::None);
    }
    // Block 0 is written only after every other block of that session.
    for (size_t i = 0; i + 1 < rig.sim.programmed.size(); ++i) {
      if (rig.sim.programmed[i].addr == kBase && size > 256) {
        CHECK(rig.sim.programmed[i + 1].addr != kBase);
      }
    }
  }
  CHECK(done > 0);
  CHECK(failed > 0);
}
