// src/storage.cpp, STM image store: the index and its slots, upload leftovers, the upload
// limits and failures, the last_good copy (steps, space, failures), the image scan, the
// protected part of a running upload, deleteImage and FileImage.
#include <LittleFS.h>
#include <string.h>

#include <string>
#include <vector>

#include "glue_test.h"
#include "storage.h"

namespace {

void mount() {
  bool formatted = false;
  REQUIRE(storage::beginFs(formatted));
}

std::string pattern(size_t n) {
  std::string s(n, '\0');
  for (size_t i = 0; i < n; ++i) s[i] = static_cast<char>('a' + i % 26);
  return s;
}

const uint8_t* bytes(const std::string& s) { return reinterpret_cast<const uint8_t*>(s.data()); }

void put32(std::string& s, size_t at, uint32_t v) {
  for (int i = 0; i < 4; ++i) s[at + i] = static_cast<char>(v >> (8 * i));
}

// A 4 KiB STM image that passes the chip-independent checks: SP and reset vector, optional
// handshake strings, the version string.
std::string stmImage(bool handshake) {
  std::string s(4096, '\x80');
  put32(s, 0, 0x20020000u);
  put32(s, 4, 0x08000101u);
  size_t at = 3000;
  if (handshake) {
    s.replace(at, 18, std::string("\x01" "DEADBEEF\0\x01" "BEEFIT\0", 18));
    at += 18;
  }
  s.replace(at, 11, std::string("\x01" "1.4.9_Dev\0", 11));
  return s;
}

std::vector<std::string> names() {
  storage::ImageEntry list[storage::kImageSlots];
  const size_t n = storage::listImages(list, storage::kImageSlots);
  std::vector<std::string> out;
  for (size_t i = 0; i < n; ++i) out.push_back(list[i].name);
  return out;
}

bool scanned(const char* name) {
  storage::ImageEntry e;
  REQUIRE(storage::findImage(name, e));
  return e.scanned;
}

size_t partSize(const std::string& path) {
  auto it = fakes::fs().nodes.find(path);
  return it == fakes::fs().nodes.end() ? SIZE_MAX : it->second.data.size();
}

}  // namespace

// ---------------------------------------------------------------- index

TEST_CASE("storage images: the index takes the free slots in order, a fifth image stays out") {
  glue::begin();
  fakes::fs().put("/stm/a.bin", "1");
  fakes::fs().put("/stm/b.bin", "22");
  fakes::fs().put("/stm/c.bin", "333");
  fakes::fs().put("/stm/last_good.bin", "4444");
  fakes::fs().put("/stm/z.bin", "55555");
  mount();
  CHECK(names() == std::vector<std::string>{"a", "b", "c", "last_good"});
  storage::ImageEntry e;
  REQUIRE(storage::findImage("c", e));
  CHECK(std::string(e.name) == "c");
  CHECK(e.size == 3);
  CHECK_FALSE(storage::findImage("z", e));
  storage::ImageEntry two[2];
  CHECK(storage::listImages(two, 2) == 2);
  CHECK(std::string(two[1].name) == "b");
}

TEST_CASE("storage images: a 31-character name is indexed, uploaded and copied to last_good") {
  glue::begin();
  const std::string disk(31, 'd');
  fakes::fs().put("/stm/" + disk + ".bin", "on disk");
  mount();
  storage::ImageEntry e;
  REQUIRE(storage::findImage(disk.c_str(), e));
  CHECK(e.size == 7);
  const std::string up(31, 'u');
  REQUIRE(storage::imageUploadBegin((up + ".bin").c_str(), 10) == storage::ImageResult::Ok);
  const std::string data = pattern(10);
  REQUIRE(storage::imageUploadWrite(bytes(data), data.size()) == storage::ImageResult::Ok);
  storage::ImageEntry info;
  REQUIRE(storage::imageUploadEnd(info) == storage::ImageResult::Ok);
  CHECK(std::string(info.name) == up);
  CHECK(fakes::fs().read("/stm/" + up + ".bin") == data);
  storage::requestLastGoodCopy(up.c_str());
  storage::service();
  CHECK(fakes::fs().read("/stm/last_good.bin") == data);
}

TEST_CASE("storage images: at most eight upload leftovers are removed per boot") {
  glue::begin();
  for (int i = 0; i < 9; ++i) fakes::fs().put("/stm/p" + std::to_string(i) + ".part", "x");
  mount();
  for (int i = 0; i < 8; ++i) CHECK_FALSE(fakes::fs().exists("/stm/p" + std::to_string(i) + ".part"));
  CHECK(fakes::fs().exists("/stm/p8.part"));
  CHECK(fakes::journalOf("fs.remove").size() == 8);
}

TEST_CASE("storage images: a bare .part and a 42-character leftover are removed") {
  glue::begin();
  const std::string longPart = std::string(37, 'q') + ".part";
  fakes::fs().put("/stm/.part", "x");
  fakes::fs().put("/stm/" + longPart, "y");
  mount();
  CHECK_FALSE(fakes::fs().exists("/stm/.part"));
  CHECK_FALSE(fakes::fs().exists("/stm/" + longPart));
}

// ---------------------------------------------------------------- last_good copy

TEST_CASE("storage last_good copy: 8 KiB per call, the copied image busy until it ends") {
  glue::begin();
  const std::string img = pattern(20000);
  fakes::fs().put("/stm/fw.bin", img);
  fakes::fs().put("/stm/a.bin", "a");
  mount();
  storage::requestLastGoodCopy("fw");
  storage::service();
  CHECK(partSize("/stm/last_good.bin.part") == 8192);
  CHECK_FALSE(fakes::fs().exists("/stm/last_good.bin"));
  CHECK(storage::deleteImage("fw") == storage::ImageResult::Busy);
  CHECK(storage::deleteImage("a") == storage::ImageResult::Ok);
  storage::service();
  CHECK(partSize("/stm/last_good.bin.part") == 16384);
  storage::service();
  CHECK(fakes::fs().read("/stm/last_good.bin") == img);
  CHECK_FALSE(fakes::fs().exists("/stm/last_good.bin.part"));
  CHECK(fakes::fs().openHandles == 0);
  storage::ImageEntry e;
  REQUIRE(storage::findImage("last_good", e));
  CHECK(e.size == 20000);
  // Done: the next call scans instead of copying again.
  const int writes = fakes::fs().writeOpens;
  storage::service();
  CHECK(fakes::fs().writeOpens == writes);
  CHECK(scanned("last_good"));  // in the slot "a" left
  CHECK(storage::deleteImage("fw") == storage::ImageResult::Ok);
}

TEST_CASE("storage last_good copy: the image plus 16 KiB must be free") {
  glue::begin();
  fakes::fs().put("/stm/fw.bin", pattern(3000));
  mount();
  fakes::fs().totalBytes = fakes::fs().usedBytes() + 3000 + 16 * 1024;
  storage::requestLastGoodCopy("fw");
  storage::service();
  CHECK(fakes::fs().read("/stm/last_good.bin") == pattern(3000));
  CHECK_FALSE(sib::logger().has(vdm::EventCode::StmFlashFailed));
}

TEST_CASE("storage last_good copy: one byte short of the space logs and copies nothing") {
  glue::begin();
  fakes::fs().put("/stm/fw.bin", pattern(3000));
  mount();
  fakes::fs().totalBytes = fakes::fs().usedBytes() + 3000 + 16 * 1024 - 1;
  storage::requestLastGoodCopy("fw");
  storage::service();
  CHECK_FALSE(fakes::fs().exists("/stm/last_good.bin.part"));
  CHECK_FALSE(fakes::fs().exists("/stm/last_good.bin"));
  CHECK(fakes::fs().openHandles == 0);
  const std::vector<vdm::Event> ev = sib::logger().withCode(vdm::EventCode::StmFlashFailed);
  REQUIRE(ev.size() == 1);
  CHECK(ev[0].valve == vdm::kNoValve);
  CHECK(ev[0].arg1 == 0);
  CHECK(ev[0].arg2 == 0);
  CHECK(std::string(ev[0].text) == "last_good: no space");
}

TEST_CASE("storage last_good copy: an empty image is not copied, a one-byte image is") {
  glue::begin();
  fakes::fs().put("/stm/e.bin", "");
  fakes::fs().put("/stm/o.bin", "x");
  mount();
  storage::requestLastGoodCopy("e");
  storage::service();
  CHECK_FALSE(fakes::fs().exists("/stm/last_good.bin"));
  CHECK_FALSE(fakes::fs().exists("/stm/last_good.bin.part"));
  storage::ImageEntry e;
  CHECK_FALSE(storage::findImage("last_good", e));
  storage::requestLastGoodCopy("o");
  storage::service();
  CHECK(fakes::fs().read("/stm/last_good.bin") == "x");
  REQUIRE(storage::findImage("last_good", e));
  CHECK(e.size == 1);
}

TEST_CASE("storage last_good copy: a failed write keeps the old copy, a failed rename drops it") {
  glue::begin();
  fakes::fs().put("/stm/last_good.bin", "old");
  fakes::fs().put("/stm/z.bin", pattern(3000));
  mount();
  fakes::fs().fail("write", "/stm/last_good.bin.part");
  storage::requestLastGoodCopy("z");
  storage::service();
  CHECK(fakes::fs().read("/stm/last_good.bin") == "old");
  CHECK_FALSE(fakes::fs().exists("/stm/last_good.bin.part"));
  CHECK(fakes::fs().openHandles == 0);
  storage::ImageEntry e;
  REQUIRE(storage::findImage("last_good", e));
  CHECK(e.size == 3);
  // The rename runs after the old copy was removed: nothing is left to index.
  fakes::fs().fail("rename", "/stm/last_good.bin.part");
  storage::requestLastGoodCopy("z");
  storage::service();
  CHECK_FALSE(fakes::fs().exists("/stm/last_good.bin"));
  CHECK_FALSE(fakes::fs().exists("/stm/last_good.bin.part"));
  CHECK_FALSE(storage::findImage("last_good", e));
}

TEST_CASE("storage last_good copy: last_good itself is never copied") {
  glue::begin();
  fakes::fs().put("/stm/last_good.bin", "lg");
  mount();
  storage::requestLastGoodCopy("last_good");
  storage::service();
  CHECK(fakes::find("fs.open /stm/last_good.bin.part w") == -1);
  CHECK(fakes::fs().read("/stm/last_good.bin") == "lg");
}

// ---------------------------------------------------------------- scan

TEST_CASE("storage scan: one image per call in index order, the results recorded") {
  glue::begin();
  const std::string good = stmImage(true);
  fakes::fs().put("/stm/a.bin", good);
  fakes::fs().put("/stm/b.bin", stmImage(false));
  mount();
  storage::service();
  storage::ImageEntry e;
  REQUIRE(storage::findImage("a", e));
  CHECK(e.scanned);
  CHECK(e.check == vdm::FlashError::None);
  CHECK(e.crc == vdm::crc32(bytes(good), good.size()));
  CHECK(std::string(e.version) == "1.4.9_Dev");
  CHECK_FALSE(scanned("b"));
  storage::service();
  REQUIRE(storage::findImage("b", e));
  CHECK(e.scanned);
  CHECK(e.check == vdm::FlashError::ImageNoHandshake);
  storage::service();
  CHECK(fakes::fs().openHandles == 0);
}

TEST_CASE("storage scan: an image whose size changed after indexing is not scanned") {
  glue::begin();
  fakes::fs().put("/stm/a.bin", std::string(100, 'a'));
  mount();
  fakes::fs().put("/stm/a.bin", std::string(200, 'a'));
  storage::service();
  storage::service();
  CHECK_FALSE(scanned("a"));
}

TEST_CASE("storage scan: an upload holds back only the scan of its own image") {
  glue::begin();
  fakes::fs().put("/stm/a.bin", stmImage(true));
  mount();
  REQUIRE(storage::imageUploadBegin("a", 10) == storage::ImageResult::Ok);
  storage::service();
  CHECK_FALSE(scanned("a"));
  storage::imageUploadAbort();
  REQUIRE(storage::imageUploadBegin("b", 10) == storage::ImageResult::Ok);
  storage::service();
  CHECK(scanned("a"));
}

// ---------------------------------------------------------------- upload

TEST_CASE("storage upload: exactly 512 KiB is accepted, one byte more removes the part") {
  glue::begin();
  mount();
  REQUIRE(storage::imageUploadBegin("big", 0) == storage::ImageResult::Ok);
  const std::string chunk = pattern(64 * 1024);
  for (int i = 0; i < 8; ++i) {
    REQUIRE(storage::imageUploadWrite(bytes(chunk), chunk.size()) == storage::ImageResult::Ok);
  }
  CHECK(partSize("/stm/big.bin.part") == storage::kMaxImageSize);
  CHECK(storage::imageUploadWrite(bytes(chunk), 1) == storage::ImageResult::TooLarge);
  CHECK_FALSE(fakes::fs().exists("/stm/big.bin.part"));
  CHECK_FALSE(storage::imageUploadActive());
  CHECK(fakes::fs().openHandles == 0);
}

TEST_CASE("storage upload: a one-byte image is written, an empty one is refused") {
  glue::begin();
  mount();
  REQUIRE(storage::imageUploadBegin("one", 1) == storage::ImageResult::Ok);
  REQUIRE(storage::imageUploadWrite(bytes("z"), 1) == storage::ImageResult::Ok);
  storage::ImageEntry info;
  REQUIRE(storage::imageUploadEnd(info) == storage::ImageResult::Ok);
  CHECK(info.size == 1);
  CHECK(fakes::fs().read("/stm/one.bin") == "z");
  REQUIRE(storage::imageUploadBegin("none", 1) == storage::ImageResult::Ok);
  CHECK(storage::imageUploadWrite(bytes("z"), 0) == storage::ImageResult::Ok);
  CHECK(storage::imageUploadEnd(info) == storage::ImageResult::Empty);
  CHECK_FALSE(fakes::fs().exists("/stm/none.bin.part"));
  CHECK_FALSE(fakes::fs().exists("/stm/none.bin"));
  CHECK_FALSE(storage::imageUploadActive());
}

TEST_CASE("storage upload: a failed write removes the part") {
  glue::begin();
  mount();
  REQUIRE(storage::imageUploadBegin("w", 10) == storage::ImageResult::Ok);
  fakes::fs().fail("write", "/stm/w.bin.part");
  CHECK(storage::imageUploadWrite(bytes("abc"), 3) == storage::ImageResult::Io);
  CHECK_FALSE(fakes::fs().exists("/stm/w.bin.part"));
  CHECK_FALSE(storage::imageUploadActive());
}

TEST_CASE("storage upload: a failed rename drops the replaced image and the part") {
  glue::begin();
  fakes::fs().put("/stm/a.bin", "old");
  fakes::fs().put("/stm/b.bin", "bb");
  mount();
  REQUIRE(storage::imageUploadBegin("a", 10) == storage::ImageResult::Ok);
  REQUIRE(storage::imageUploadWrite(bytes("new"), 3) == storage::ImageResult::Ok);
  fakes::fs().fail("rename", "/stm/a.bin.part");
  storage::ImageEntry info;
  CHECK(storage::imageUploadEnd(info) == storage::ImageResult::Io);
  CHECK_FALSE(fakes::fs().exists("/stm/a.bin"));
  CHECK_FALSE(fakes::fs().exists("/stm/a.bin.part"));
  storage::ImageEntry e;
  CHECK_FALSE(storage::findImage("a", e));
  CHECK(storage::findImage("b", e));
  CHECK_FALSE(storage::imageUploadActive());
}

TEST_CASE("storage upload: a replacement counts only the other images") {
  glue::begin();
  for (const char* n : {"a", "m", "z"}) fakes::fs().put(std::string("/stm/") + n + ".bin", "x");
  mount();
  REQUIRE(storage::imageUploadBegin("a", 10) == storage::ImageResult::Ok);
  storage::imageUploadAbort();
  CHECK(storage::imageUploadBegin("q", 10) == storage::ImageResult::TooMany);
}

TEST_CASE("storage deleteFile: the part of the running upload is protected, others are not") {
  glue::begin();
  mount();
  fakes::fs().put("/stm/x.bin.part", "x");
  CHECK(storage::deleteFile("/stm/x.bin.part") == storage::FileResult::Ok);
  REQUIRE(storage::imageUploadBegin("up", 10) == storage::ImageResult::Ok);
  CHECK(storage::deleteFile("/stm/up.bin.part") == storage::FileResult::Protected);
  CHECK(fakes::fs().exists("/stm/up.bin.part"));
  fakes::fs().put("/stm/old.bin.part", "o");
  CHECK(storage::deleteFile("/stm/old.bin.part") == storage::FileResult::Ok);
  CHECK_FALSE(fakes::fs().exists("/stm/old.bin.part"));
}

TEST_CASE("storage deleteFile: the part of the running last_good copy is protected") {
  glue::begin();
  const std::string img = pattern(20000);
  fakes::fs().put("/stm/fw.bin", img);
  mount();
  // the part of a copy that does not run is a leftover
  fakes::fs().put("/stm/last_good.bin.part", "x");
  CHECK(storage::deleteFile("/stm/last_good.bin.part") == storage::FileResult::Ok);
  storage::requestLastGoodCopy("fw");
  storage::service();
  REQUIRE(partSize("/stm/last_good.bin.part") == 8192);
  CHECK(storage::deleteFile("/stm/last_good.bin.part") == storage::FileResult::Protected);
  fakes::fs().put("/stm/x.bin.part", "x");
  CHECK(storage::deleteFile("/stm/x.bin.part") == storage::FileResult::Ok);
  storage::service();
  storage::service();
  CHECK(fakes::fs().read("/stm/last_good.bin") == img);
  CHECK(storage::deleteFile("/stm/last_good.bin.part") == storage::FileResult::NotFound);
}

TEST_CASE("storage last_good copy: a part that cannot be created starts no copy") {
  glue::begin();
  fakes::fs().put("/stm/fw.bin", pattern(20000));
  mount();
  fakes::fs().fail("open", "/stm/last_good.bin.part");
  storage::requestLastGoodCopy("fw");
  storage::service();
  CHECK_FALSE(fakes::fs().exists("/stm/last_good.bin.part"));
  CHECK(fakes::fs().openHandles == 0);
  CHECK(storage::deleteImage("fw") == storage::ImageResult::Ok);  // no copy holds it
}

// ---------------------------------------------------------------- deleteImage

TEST_CASE("storage deleteImage: a vanished file leaves the index, long names") {
  glue::begin();
  fakes::fs().put("/stm/a.bin", "a");
  mount();
  fakes::fs().nodes.erase("/stm/a.bin");
  CHECK(storage::deleteImage("a") == storage::ImageResult::Ok);
  storage::ImageEntry e;
  CHECK_FALSE(storage::findImage("a", e));
  CHECK(storage::deleteImage(nullptr) == storage::ImageResult::BadName);
  // "/stm/<name>.bin" must fit 48 bytes.
  CHECK(storage::deleteImage(std::string(38, 'n').c_str()) == storage::ImageResult::NotFound);
  CHECK(storage::deleteImage(std::string(39, 'n').c_str()) == storage::ImageResult::BadName);
}

// ---------------------------------------------------------------- FileImage

TEST_CASE("storage FileImage: names, the end of the file, seeks only when needed") {
  glue::begin();
  mount();
  const std::string n38(38, 'f');
  const std::string n39(39, 'f');
  fakes::fs().put("/stm/" + n38 + ".bin", "0123456789");
  fakes::fs().put("/stm/" + n39 + ".bin", "0123456789");
  storage::FileImage img;
  CHECK_FALSE(img.open(nullptr));
  CHECK_FALSE(img.open(n39.c_str()));
  REQUIRE(img.open(n38.c_str()));
  uint8_t out[8] = {};
  fakes::fs().fail("seek");    // armed until the first seek
  CHECK(img.read(0, out, 4));  // at the start after open: no seek
  CHECK(std::string(reinterpret_cast<char*>(out), 4) == "0123");
  CHECK_FALSE(img.read(6, out, 5));  // past the end: refused before any seek
  CHECK(img.read(4, out, 2));        // sequential: no seek
  CHECK(std::string(reinterpret_cast<char*>(out), 2) == "45");
  CHECK_FALSE(img.read(2, out, 2));  // the seek fails
  CHECK(img.read(10, out, 0));
  CHECK_FALSE(img.read(11, out, 0));
  fakes::fs().fail("read");
  CHECK_FALSE(img.read(2, out, 2));
  CHECK(img.read(2, out, 2));
  CHECK(std::string(reinterpret_cast<char*>(out), 2) == "23");
  img.close();
  CHECK(fakes::fs().openHandles == 0);
  CHECK_FALSE(img.read(0, out, 0));
}
