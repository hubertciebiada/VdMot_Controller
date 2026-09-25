// Smoke tests of src/storage.cpp: file system mount, config load/apply, NVS values, STM images.
#include <LittleFS.h>

#include <string>
#include <vector>

#include "glue_test.h"
#include "storage.h"

namespace {

std::vector<uint8_t> blobOf(const vdm::Config& c) {
  std::vector<uint8_t> b(vdm::kConfigBlobMax);
  b.resize(vdm::encodeConfig(c, b.data(), b.size()));
  return b;
}

vdm::Config named(const char* station) {
  vdm::Config c;
  vdm::setDefaults(c);
  vdm::copyString(c.station, sizeof c.station, station);
  return c;
}

void mount() {
  bool formatted = false;
  REQUIRE(storage::beginFs(formatted));
}

}  // namespace

TEST_CASE("storage beginFs: an unformatted partition is formatted, /stm and /log created") {
  glue::begin();
  fakes::fs().formatted = false;
  bool formatted = false;
  CHECK(storage::beginFs(formatted));
  CHECK(formatted);
  CHECK(storage::fsReady());
  CHECK(fakes::fs().formats == 1);
  CHECK(fakes::fs().nodes.at("/stm").dir);
  CHECK(fakes::fs().nodes.at("/log").dir);
}

TEST_CASE("storage beginFs: a format that fails leaves the firmware without files") {
  glue::begin();
  fakes::fs().formatted = false;
  fakes::fs().formatOk = false;
  bool formatted = true;
  CHECK_FALSE(storage::beginFs(formatted));
  CHECK_FALSE(formatted);
  CHECK_FALSE(storage::fsReady());
  CHECK(storage::fsTotal() == 0);
}

TEST_CASE("storage beginFs: upload leftovers are removed, images are indexed") {
  glue::begin();
  fakes::fs().put("/stm/a.bin", std::string(100, 'a'));
  fakes::fs().put("/stm/b.bin.part", "x");
  fakes::fs().put("/stm/notes.txt", "n");
  mount();
  CHECK_FALSE(fakes::fs().exists("/stm/b.bin.part"));
  storage::ImageEntry e;
  REQUIRE(storage::findImage("a", e));
  CHECK(e.size == 100);
  CHECK_FALSE(e.scanned);
  storage::ImageEntry list[storage::kImageSlots];
  CHECK(storage::listImages(list, storage::kImageSlots) == 1);
}

TEST_CASE("storage loadConfig: a stored blob is loaded") {
  glue::begin();
  fakes::nvs().setBlob("vdmrev", "cfg", blobOf(named("Stored")));
  vdm::Config c;
  vdm::ImportReport report;
  storage::LoadDetails details;
  CHECK(storage::loadConfig(c, report, details) == storage::LoadSource::Stored);
  CHECK(std::string(c.station) == "Stored");
  CHECK(storage::bootLoadSource() == storage::LoadSource::Stored);
  CHECK(sib::logger().events.empty());
}

TEST_CASE("storage loadConfig: an unreadable blob gives defaults and a ConfigDefaults event") {
  glue::begin();
  std::vector<uint8_t> bad = blobOf(named("Bad"));
  bad[bad.size() / 2] ^= 0xFF;
  fakes::nvs().setBlob("vdmrev", "cfg", bad);
  vdm::Config c;
  vdm::ImportReport report;
  storage::LoadDetails details;
  CHECK(storage::loadConfig(c, report, details) == storage::LoadSource::DefaultsAfterError);
  vdm::Config defaults;
  vdm::setDefaults(defaults);
  CHECK(std::string(c.station) == defaults.station);
  const vdm::Event e = sib::logger().withCode(vdm::EventCode::ConfigDefaults).at(0);
  CHECK(e.arg1 == details.errorCode);
  CHECK(details.errorCode != 0);
  CHECK(fakes::nvs().getBlob("vdmrev", "cfg") == bad);  // never overwritten automatically
}

TEST_CASE("storage loadConfig: nothing stored and no legacy data gives defaults, import done") {
  glue::begin();
  vdm::Config c;
  vdm::ImportReport report;
  storage::LoadDetails details;
  CHECK(storage::loadConfig(c, report, details) == storage::LoadSource::Defaults);
  CHECK(fakes::nvs().getU("vdmrev", "imported") == 1);
  CHECK_FALSE(fakes::nvs().getBlob("vdmrev", "cfg").empty());
  CHECK(storage::loadConfig(c, report, details) == storage::LoadSource::Stored);
}

TEST_CASE("storage applyConfig: persisted, published, revision + 1; a failed write names nvs") {
  glue::begin();
  storage::setActiveConfig(named("A"));
  const uint32_t rev = storage::configRevision();
  char path[32] = "";
  CHECK(storage::applyConfig(named("B"), path, sizeof path));
  CHECK(storage::configRevision() == rev + 1);
  CHECK(storage::configSavedSinceBoot());
  vdm::Config out;
  storage::getConfig(out);
  CHECK(std::string(out.station) == "B");
  vdm::Config decoded;
  const std::vector<uint8_t> blob = fakes::nvs().getBlob("vdmrev", "cfg");
  REQUIRE(vdm::decodeConfig(blob.data(), blob.size(), decoded) == vdm::DecodeResult::Ok);
  CHECK(std::string(decoded.station) == "B");
  fakes::nvs().failSet.insert("cfg");
  CHECK_FALSE(storage::applyConfig(named("C"), path, sizeof path));
  CHECK(std::string(path) == "nvs");
  CHECK(storage::configRevision() == rev + 1);
}

TEST_CASE("storage factoryReset: vdmrev is erased, the latch is kept, imported is set") {
  glue::begin();
  fakes::nvs().setBlob("vdmrev", "cfg", blobOf(named("X")));
  fakes::nvs().setU8("vdmrev", "frLatch", 1);
  fakes::nvs().setU32("vdmrev", "boots", 9);
  CHECK(storage::factoryReset());
  CHECK_FALSE(fakes::nvs().has("vdmrev", "cfg"));
  CHECK_FALSE(fakes::nvs().has("vdmrev", "boots"));
  CHECK(fakes::nvs().getU("vdmrev", "frLatch") == 1);
  CHECK(fakes::nvs().getU("vdmrev", "imported") == 1);
}

TEST_CASE("storage: boot count, calibration slot and time, flags") {
  glue::begin();
  fakes::nvs().setU32("vdmrev", "boots", 4);
  CHECK(storage::incrementBootCount() == 5);
  CHECK(storage::bootCount() == 5);
  CHECK(fakes::nvs().getU("vdmrev", "boots") == 5);
  storage::saveCalibSlot(20260923);
  CHECK(storage::loadCalibSlot() == 20260923);
  storage::saveLastCalib(0);  // ignored
  CHECK(storage::loadLastCalib() == 0);
  storage::saveLastCalib(1790136000);
  CHECK(storage::loadLastCalib() == 1790136000);
  CHECK_FALSE(storage::haCleanupDone());
  storage::setHaCleanupDone();
  CHECK(storage::haCleanupDone());
  storage::setOtaStmRequired(true);
  CHECK(storage::otaStmRequired());
  storage::clearOtaStmRequired();
  CHECK_FALSE(fakes::nvs().has("vdmrev", "otaStm"));
  storage::setHaLayout(2);
  CHECK(storage::haLayout() == 2);
  const uint8_t t[3] = {1, 2, 3};
  CHECK(storage::saveTargets(t, 3));
  uint8_t out[3];
  CHECK(storage::loadTargets(out, 2) == 0);
  CHECK(storage::loadTargets(out, 3) == 3);
}

TEST_CASE("storage image upload: the part file becomes the image and is indexed") {
  glue::begin();
  mount();
  REQUIRE(storage::imageUploadBegin("fw.bin", 1000) == storage::ImageResult::Ok);
  CHECK(storage::imageUploadActive());
  CHECK(fakes::fs().exists("/stm/fw.bin.part"));
  const std::string data(300, 'd');
  CHECK(storage::imageUploadWrite(reinterpret_cast<const uint8_t*>(data.data()), data.size()) ==
        storage::ImageResult::Ok);
  storage::ImageEntry info;
  CHECK(storage::imageUploadEnd(info) == storage::ImageResult::Ok);
  CHECK(std::string(info.name) == "fw");
  CHECK(info.size == 300);
  CHECK(info.crc == vdm::crc32(reinterpret_cast<const uint8_t*>(data.data()), data.size()));
  CHECK(fakes::fs().read("/stm/fw.bin") == data);
  CHECK_FALSE(fakes::fs().exists("/stm/fw.bin.part"));
  CHECK_FALSE(storage::imageUploadActive());
}

TEST_CASE("storage image upload: bad names, last_good, busy, too many images") {
  glue::begin();
  mount();
  CHECK(storage::imageUploadBegin(".hidden", 10) == storage::ImageResult::BadName);
  CHECK(storage::imageUploadBegin("last_good.bin", 10) == storage::ImageResult::BadName);
  for (const char* n : {"a", "b", "c"}) fakes::fs().put(std::string("/stm/") + n + ".bin", "x");
  fakes::fs().put("/stm/last_good.bin", "x");
  bool formatted = false;
  storage::beginFs(formatted);  // indexes the four images
  CHECK(storage::imageUploadBegin("d", 10) == storage::ImageResult::TooMany);
  REQUIRE(storage::imageUploadBegin("a", 10) == storage::ImageResult::Ok);  // replaces a
  CHECK(storage::imageUploadBegin("b", 10) == storage::ImageResult::Busy);
  storage::imageUploadAbort();
  CHECK_FALSE(fakes::fs().exists("/stm/a.bin.part"));
}

TEST_CASE("storage image upload: not enough free space for the image and the reserve") {
  glue::begin();
  mount();
  fakes::fs().totalBytes = fakes::fs().usedBytes() + 1000 + storage::kFsReserve;
  CHECK(storage::imageUploadBegin("x", 1001) == storage::ImageResult::NoSpace);
  CHECK(storage::imageUploadBegin("x", 1000) == storage::ImageResult::Ok);
}

TEST_CASE("storage images: delete, and the last_good copy after a flash") {
  glue::begin();
  mount();
  const std::string img(3000, 'i');
  fakes::fs().put("/stm/fw.bin", img);
  bool formatted = false;
  storage::beginFs(formatted);
  storage::requestLastGoodCopy("fw");
  storage::service();
  CHECK(fakes::fs().read("/stm/last_good.bin") == img);
  storage::ImageEntry e;
  REQUIRE(storage::findImage("last_good", e));
  CHECK(e.size == 3000);
  CHECK(storage::deleteImage("fw") == storage::ImageResult::Ok);
  CHECK_FALSE(fakes::fs().exists("/stm/fw.bin"));
  CHECK(storage::deleteImage("fw") == storage::ImageResult::NotFound);
}

TEST_CASE("storage FileImage: reads at offsets, refuses reads past the end") {
  glue::begin();
  mount();
  fakes::fs().put("/stm/fw.bin", "0123456789");
  storage::FileImage img;
  REQUIRE(img.open("fw"));
  CHECK(img.size() == 10);
  uint8_t out[4] = {};
  CHECK(img.read(6, out, 4));
  CHECK(std::string(reinterpret_cast<char*>(out), 4) == "6789");
  CHECK(img.read(0, out, 2));
  CHECK(out[0] == '0');
  CHECK_FALSE(img.read(8, out, 3));
  img.close();
  CHECK(img.size() == 0);
  CHECK_FALSE(img.open("missing"));
}

TEST_CASE("storage names: image name rules and paths") {
  glue::begin();
  char out[40];
  CHECK(storage::normalizeImageName("fw-1_2.bin", 10, out, sizeof out));
  CHECK(std::string(out) == "fw-1_2");
  CHECK_FALSE(storage::normalizeImageName("a b", 3, out, sizeof out));
  CHECK_FALSE(storage::normalizeImageName(".bin", 4, out, sizeof out));
  CHECK(storage::imagePath("fw", true, out, sizeof out));
  CHECK(std::string(out) == "/stm/fw.bin.part");
  CHECK_FALSE(storage::imagePath("fw", true, out, 16));
  CHECK(std::string(storage::imageResultName(storage::ImageResult::TooMany)) == "too_many_images");
}
