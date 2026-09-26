// src/storage.cpp: the small NVS values at boot and without NVS, flags, targets and the network
// trial record, config revision and apply refusals, the load table edges (blob sizes, backup
// files, sameFile), the legacy NVS reader (integer widths, strings, blobs) and the import events.
#include <LittleFS.h>
#include <string.h>

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

std::vector<uint8_t> extOf(const vdm::Config& c, const std::vector<uint8_t>& keep = {}) {
  std::vector<uint8_t> b(vdm::kConfigExtBlobMax);
  b.resize(vdm::encodeConfigExt(c, b.data(), b.size(), keep.empty() ? nullptr : keep.data(),
                                keep.size()));
  return b;
}

std::string str(const std::vector<uint8_t>& b) { return std::string(b.begin(), b.end()); }

vdm::Config named(const char* station) {
  vdm::Config c;
  vdm::copyString(c.station, sizeof c.station, station);
  return c;
}

vdm::Config withExt(const char* station) {
  vdm::Config c = named(station);
  c.failsafe.timeoutMin = 90;
  return c;
}

void mount() {
  bool formatted = false;
  REQUIRE(storage::beginFs(formatted));
}

std::vector<uint8_t> corrupt(std::vector<uint8_t> b) {
  b[b.size() / 2] ^= 0xFF;
  return b;
}

struct Load {
  storage::LoadSource src;
  vdm::Config cfg;
  vdm::ImportReport report;
  storage::LoadDetails details;
};

Load load() {
  Load l;
  l.src = storage::loadConfig(l.cfg, l.report, l.details);
  return l;
}

// A damaged cfg in NVS and a usable cfg.bak: the backup is loaded with this cfgx.bak.
Load restoreWithExtBackup(const std::string& extBak) {
  mount();
  fakes::fs().put("/sys/cfg.bak", str(blobOf(withExt("Bak"))));
  fakes::fs().put("/sys/cfgx.bak", extBak);
  fakes::nvs().setBlob("vdmrev", "cfg", corrupt(blobOf(named("Bad"))));
  fakes::nvs().setBlob("vdmrev", "cfgx", extOf(named("Bad")));
  return load();
}

}  // namespace

// ---------------------------------------------------------------- NVS values

TEST_CASE("storage values: a fresh device") {
  glue::begin();
  CHECK(storage::configRevision() == 0);
  CHECK(storage::bootCount() == 0);
  CHECK_FALSE(storage::otaStmRequired());
  CHECK_FALSE(storage::factoryLatched());
  CHECK(storage::haLayout() == 0);
  CHECK(storage::loadCalibSlot() == 0);
  CHECK(storage::loadLastCalib() == 0);
  CHECK(storage::incrementBootCount() == 1);
  CHECK(fakes::nvs().getU("vdmrev", "boots") == 1);
}

TEST_CASE("storage values: NVS that cannot be opened reads as zero and refuses the reset") {
  glue::begin();
  fakes::nvs().setU32("vdmrev", "boots", 7);
  fakes::nvs().setU32("vdmrev", "calSlot", 20260101);
  fakes::nvs().setI64("vdmrev", "lastCal", 1790136000);
  fakes::nvs().setU8("vdmrev", "haLayout", 2);
  fakes::nvs().setBlob("vdmrev", "targets", {1, 2, 3});
  fakes::nvs().failOpen.insert("vdmrev");
  CHECK(storage::incrementBootCount() == 0);
  CHECK(storage::loadCalibSlot() == 0);
  CHECK(storage::loadLastCalib() == 0);
  CHECK(storage::haLayout() == 0);
  uint8_t out[8];
  CHECK(storage::loadTargets(out, sizeof out) == 0);
  CHECK_FALSE(storage::factoryReset());
  CHECK(fakes::nvs().getU("vdmrev", "boots") == 7);
}

TEST_CASE("storage values: flags, targets, the network trial record, the calibration time") {
  glue::begin();
  storage::setFactoryLatched(true);
  CHECK(storage::factoryLatched());
  storage::setFactoryLatched(false);
  CHECK_FALSE(storage::factoryLatched());
  CHECK_FALSE(fakes::nvs().has("vdmrev", "frLatch"));
  const uint8_t t[3] = {7, 8, 9};
  CHECK_FALSE(storage::saveTargets(t, 0));
  CHECK_FALSE(fakes::nvs().has("vdmrev", "targets"));
  CHECK(storage::saveTargets(t, 1));
  uint8_t out[8] = {};
  CHECK(storage::loadTargets(out, sizeof out) == 1);
  CHECK(out[0] == 7);
  CHECK(storage::loadTargets(nullptr, sizeof out) == 0);
  CHECK(storage::saveNetTrialBlob(t, 3));
  CHECK(fakes::nvs().getBlob("vdmrev", "netTrial") == std::vector<uint8_t>{7, 8, 9});
  memset(out, 0, sizeof out);
  CHECK(storage::loadNetTrialBlob(out, sizeof out) == 3);
  CHECK(out[2] == 9);
  storage::clearNetTrial();
  CHECK_FALSE(fakes::nvs().has("vdmrev", "netTrial"));
  CHECK(storage::loadNetTrialBlob(out, sizeof out) == 0);
  storage::saveLastCalib(0);
  CHECK_FALSE(fakes::nvs().has("vdmrev", "lastCal"));
  storage::saveLastCalib(1);
  CHECK(storage::loadLastCalib() == 1);
}

TEST_CASE("storage setActiveConfig: every publish adds exactly one to the revision") {
  glue::begin();
  storage::setActiveConfig(named("A"));
  CHECK(storage::configRevision() == 1);
  storage::setActiveConfig(named("B"));
  CHECK(storage::configRevision() == 2);
}

TEST_CASE("storage applyConfig: an invalid config is refused with its key, nothing saved") {
  glue::begin();
  vdm::Config c = named("Inv");
  c.calib.hour = 24;
  char path[32] = "";
  CHECK_FALSE(storage::applyConfig(c, path, sizeof path));
  CHECK(std::string(path) == "calib.hour");
  CHECK_FALSE(fakes::nvs().has("vdmrev", "cfg"));
  CHECK(storage::configRevision() == 0);
  CHECK_FALSE(storage::configSavedSinceBoot());
}

TEST_CASE("storage applyConfig: a failed write without a path buffer") {
  glue::begin();
  fakes::nvs().failSet.insert("cfg");
  CHECK_FALSE(storage::applyConfig(named("N"), nullptr, 16));
  CHECK(storage::configRevision() == 0);
}

TEST_CASE("storage factoryReset: a failed erase fails the reset") {
  glue::begin();
  fakes::nvs().setU32("vdmrev", "boots", 3);
  fakes::nvs().failErase = true;
  CHECK_FALSE(storage::factoryReset());
  CHECK(fakes::nvs().getU("vdmrev", "imported") == 1);
}

TEST_CASE("storage factoryReset: the kept unknown cfgx records are forgotten") {
  glue::begin();
  const std::vector<uint8_t> unknown = {200, 0, 2, 7, 9};
  fakes::nvs().setBlob("vdmrev", "cfg", blobOf(named("K")));
  fakes::nvs().setBlob("vdmrev", "cfgx", extOf(named("K"), unknown));
  REQUIRE(load().src == storage::LoadSource::Stored);
  CHECK(storage::factoryReset());
  char path[16] = "";
  REQUIRE(storage::applyConfig(named("After"), path, sizeof path));
  CHECK(fakes::nvs().getBlob("vdmrev", "cfgx") == extOf(named("After")));
}

// ---------------------------------------------------------------- load table edges

TEST_CASE("storage load: defaults keep no unknown cfgx records for the next save") {
  glue::begin();
  fakes::nvs().setU8("vdmrev", "imported", 1);
  REQUIRE(load().src == storage::LoadSource::Defaults);
  char path[16] = "";
  REQUIRE(storage::applyConfig(named("K"), path, sizeof path));
  CHECK(fakes::nvs().getBlob("vdmrev", "cfgx") == extOf(named("K")));
}

TEST_CASE("storage load: a one-byte cfg is a damaged blob") {
  glue::begin();
  fakes::nvs().setBlob("vdmrev", "cfg", {1});
  const Load l = load();
  CHECK(l.src == storage::LoadSource::DefaultsAfterError);
  CHECK(l.details.errorCode == static_cast<uint8_t>(vdm::DecodeResult::TooShort));
}

TEST_CASE("storage load: a damaged cfg of the maximum size is decoded, not unreadable") {
  glue::begin();
  fakes::nvs().setBlob("vdmrev", "cfg", std::vector<uint8_t>(vdm::kConfigBlobMax, 0x55));
  const Load l = load();
  CHECK(l.src == storage::LoadSource::DefaultsAfterError);
  CHECK(l.details.errorCode == static_cast<uint8_t>(vdm::DecodeResult::BadMagic));
}

TEST_CASE("storage load: without cfgx the backup follows, with a damaged cfgx it waits") {
  glue::begin();
  mount();
  fakes::nvs().setBlob("vdmrev", "cfg", blobOf(named("NoExt")));
  Load l = load();
  REQUIRE(l.src == storage::LoadSource::Stored);
  storage::setActiveConfig(l.cfg);
  storage::service();
  CHECK(fakes::fs().read("/sys/cfg.bak") == str(blobOf(named("NoExt"))));
  fakes::fs().nodes.erase("/sys/cfg.bak");
  fakes::fs().nodes.erase("/sys/cfgx.bak");
  fakes::nvs().setBlob("vdmrev", "cfgx", corrupt(extOf(withExt("NoExt"))));
  l = load();
  REQUIRE(l.src == storage::LoadSource::Stored);
  CHECK(l.details.info.ext == vdm::ExtResult::BadCrc);
  storage::service();
  CHECK_FALSE(fakes::fs().exists("/sys/cfg.bak"));
}

TEST_CASE("storage load: a backup that differs after its first 64 bytes is rewritten") {
  glue::begin();
  mount();
  const vdm::Config c = withExt("Cmp");
  const std::vector<uint8_t> base = blobOf(c);
  REQUIRE(base.size() > 128);
  std::vector<uint8_t> bak = base;
  bak[100] ^= 0x01;
  fakes::nvs().setBlob("vdmrev", "cfg", base);
  fakes::nvs().setBlob("vdmrev", "cfgx", extOf(c));
  fakes::fs().put("/sys/cfg.bak", str(bak));
  fakes::fs().put("/sys/cfgx.bak", str(extOf(c)));
  const Load l = load();
  REQUIRE(l.src == storage::LoadSource::Stored);
  storage::setActiveConfig(l.cfg);
  storage::service();
  CHECK(fakes::fs().read("/sys/cfg.bak") == str(base));
}

TEST_CASE("storage load: a backup file that cannot be opened is not taken as equal") {
  glue::begin();
  mount();
  const vdm::Config c = withExt("Open");
  fakes::nvs().setBlob("vdmrev", "cfg", blobOf(c));
  fakes::nvs().setBlob("vdmrev", "cfgx", extOf(c));
  fakes::fs().put("/sys/cfg.bak", str(blobOf(c)));
  fakes::fs().put("/sys/cfgx.bak", str(extOf(c)));
  fakes::fs().fail("open", "/sys/cfg.bak");
  const Load l = load();
  REQUIRE(l.src == storage::LoadSource::Stored);
  storage::setActiveConfig(l.cfg);
  storage::service();
  CHECK(fakes::journalOf("fs.rename").size() == 2);
}

TEST_CASE("storage load: a cfgx.bak that cannot be opened restores without cfgx") {
  glue::begin();
  fakes::fs().fail("open", "/sys/cfgx.bak");
  const Load l = restoreWithExtBackup(str(extOf(withExt("Bak"))));
  CHECK(l.src == storage::LoadSource::Backup);
  CHECK(l.cfg.failsafe.timeoutMin == 60);
  CHECK_FALSE(fakes::nvs().has("vdmrev", "cfgx"));
}

TEST_CASE("storage load: a cfgx.bak larger than a cfgx blob restores without cfgx") {
  glue::begin();
  std::string ext = str(extOf(withExt("Bak")));
  ext.resize(vdm::kConfigExtBlobMax + 1, '\0');
  const Load l = restoreWithExtBackup(ext);
  CHECK(l.src == storage::LoadSource::Backup);
  CHECK(l.cfg.failsafe.timeoutMin == 60);
  CHECK_FALSE(fakes::nvs().has("vdmrev", "cfgx"));
}

TEST_CASE("storage load: a cfgx.bak of exactly the blob maximum is used") {
  glue::begin();
  std::string ext = str(extOf(withExt("Bak")));
  ext.resize(vdm::kConfigExtBlobMax, '\0');  // bytes after the CRC are ignored
  const Load l = restoreWithExtBackup(ext);
  CHECK(l.src == storage::LoadSource::Backup);
  CHECK(l.cfg.failsafe.timeoutMin == 90);
  CHECK(str(fakes::nvs().getBlob("vdmrev", "cfgx")) == ext);
}

TEST_CASE("storage load: a one-byte cfgx.bak goes back to NVS as it is") {
  glue::begin();
  const Load l = restoreWithExtBackup("V");
  CHECK(l.src == storage::LoadSource::Backup);
  CHECK(l.cfg.failsafe.timeoutMin == 60);
  CHECK(fakes::nvs().getBlob("vdmrev", "cfgx") == std::vector<uint8_t>{'V'});
}

TEST_CASE("storage load: a single unknown cfgx record is logged as a newer firmware") {
  glue::begin();
  const vdm::Config c = named("One");
  fakes::nvs().setBlob("vdmrev", "cfg", blobOf(c));
  fakes::nvs().setBlob("vdmrev", "cfgx", extOf(c, {201, 0, 0}));
  REQUIRE(load().src == storage::LoadSource::Stored);
  const std::vector<vdm::Event> ev = sib::logger().withCode(vdm::EventCode::ConfigNewerSchema);
  REQUIRE(ev.size() == 1);
  CHECK(ev[0].arg2 == 1);
}

TEST_CASE("storage beginFs: a partition that mounts is not reported as formatted") {
  glue::begin();
  bool formatted = true;
  CHECK(storage::beginFs(formatted));
  CHECK_FALSE(formatted);
  CHECK(fakes::fs().formats == 0);
  CHECK(storage::fsTotal() == fakes::fs().totalBytes);
  CHECK(storage::fsUsed() == fakes::fs().usedBytes());
}

TEST_CASE("storage fsUsed: zero without a file system") {
  glue::begin();
  fakes::fs().formatted = false;
  fakes::fs().formatOk = false;
  bool formatted = false;
  REQUIRE_FALSE(storage::beginFs(formatted));
  CHECK(storage::fsUsed() == 0);
}

// ---------------------------------------------------------------- legacy reader and import

TEST_CASE("storage import: legacy integers of every width are read") {
  glue::begin();
  struct Width {
    fakes::NvsType type;
    int64_t value;
  };
  const Width widths[] = {
      {fakes::NvsType::U8, 200},     {fakes::NvsType::I8, -5},
      {fakes::NvsType::U16, 40000},  {fakes::NvsType::I16, -300},
      {fakes::NvsType::U32, 100000}, {fakes::NvsType::I32, -70000},
      {fakes::NvsType::I64, 123456},
  };
  for (const Width& w : widths) {
    CAPTURE(static_cast<int>(w.type));
    fakes::nvs().ns.erase("vdmrev");
    fakes::nvs().ns.erase("protCfg");
    switch (w.type) {
      case fakes::NvsType::U8: fakes::nvs().setU8("protCfg", "brokerMQTO", 200); break;
      case fakes::NvsType::I8: fakes::nvs().setI8("protCfg", "brokerMQTO", -5); break;
      case fakes::NvsType::U16: fakes::nvs().setU16("protCfg", "brokerMQTO", 40000); break;
      case fakes::NvsType::I16: fakes::nvs().setI16("protCfg", "brokerMQTO", -300); break;
      case fakes::NvsType::U32: fakes::nvs().setU32("protCfg", "brokerMQTO", 100000); break;
      case fakes::NvsType::I32: fakes::nvs().setI32("protCfg", "brokerMQTO", -70000); break;
      default: fakes::nvs().setI64("protCfg", "brokerMQTO", 123456); break;
    }
    const Load l = load();
    CHECK(l.src == storage::LoadSource::Imported);
    CHECK(l.report.legacyFailsafeValid);
    CHECK(l.report.legacyFailsafeTimeoutMin == w.value);
    CHECK(l.report.legacyFailsafePct == 0);
    CHECK(l.report.ignored == 1);  // brokerMQTO; brokerMQToPos is absent
  }
}

TEST_CASE("storage import: legacy strings are read whole, an over-long one is rejected") {
  glue::begin();
  fakes::nvs().setStr("sysCfg", "stName", "Legacy");
  fakes::nvs().setStr("netCfg", "userName", std::string(65, 'u'));
  const Load l = load();
  CHECK(l.src == storage::LoadSource::Imported);
  CHECK(std::string(l.cfg.station) == "Legacy");
  CHECK(l.cfg.web.user[0] == '\0');
  CHECK(l.report.imported == 1);
  CHECK(std::string(l.report.firstRejected) == "netCfg/userName");
}

TEST_CASE("storage import: a legacy blob of exactly its size is read") {
  glue::begin();
  std::vector<uint8_t> valves(12 * 12, 0);
  memcpy(valves.data(), "Bad", 3);
  valves[11] = 1;
  fakes::nvs().setBlob("valvesCfg", "valves", valves);
  const Load l = load();
  CHECK(l.src == storage::LoadSource::Imported);
  CHECK(std::string(l.cfg.valves[0].name) == "Bad");
}

TEST_CASE("storage import: the last calibration time is stored only when the import has one") {
  glue::begin();
  fakes::nvs().setStr("sysCfg", "stName", "Legacy");
  REQUIRE(load().src == storage::LoadSource::Imported);
  CHECK_FALSE(fakes::nvs().has("vdmrev", "lastCal"));
  fakes::nvs().ns.erase("vdmrev");
  fakes::nvs().setI64("Misc", "MiscLC", 1700000000);
  const Load l = load();
  REQUIRE(l.src == storage::LoadSource::Imported);
  CHECK(l.report.lastCalibEpoch == 1700000000);
  CHECK(storage::loadLastCalib() == 1700000000);
}

TEST_CASE("storage import: dropped messenger settings without PI valves are logged") {
  glue::begin();
  fakes::nvs().setStr("sysCfg", "stName", "Legacy");
  fakes::nvs().setU8("msgCfg", "msgFlags", 1);
  REQUIRE(load().src == storage::LoadSource::Imported);
  const std::vector<vdm::Event> ev = sib::logger().withCode(vdm::EventCode::ImportDropped);
  REQUIRE(ev.size() == 1);
  CHECK(ev[0].arg1 == 0);
  CHECK(ev[0].arg2 == vdm::kDroppedMessenger);
}

TEST_CASE("storage import: a single removed legacy image is logged") {
  glue::begin();
  mount();
  fakes::nvs().setStr("sysCfg", "stName", "Legacy");
  fakes::fs().put("/x.bin", std::string(100, 'x'));
  REQUIRE(load().src == storage::LoadSource::Imported);
  const std::vector<vdm::Event> ev = sib::logger().withCode(vdm::EventCode::FilesRemoved);
  REQUIRE(ev.size() == 1);
  CHECK(ev[0].arg1 == 1);
  CHECK(ev[0].arg2 == 1);
}

TEST_CASE("storage removeLegacyImages: a legacy image with the longest path is removed") {
  glue::begin();
  mount();
  const std::string path = "/" + std::string(vdm::kFsPathMax - 5, 'L') + ".bin";
  REQUIRE(path.size() == vdm::kFsPathMax);
  fakes::fs().put(path, "x");
  uint32_t kib = 0;
  CHECK(storage::removeLegacyImages(kib) == 1);
  CHECK(kib == 1);
  CHECK_FALSE(fakes::fs().exists(path));
}

TEST_CASE("storage removeLegacyImages: a file that cannot be removed ends the run after its batch") {
  glue::begin();
  mount();
  for (int i = 0; i < 10; ++i) fakes::fs().put("/i" + std::to_string(i) + ".bin", "x");
  fakes::fs().fail("remove", "/i0.bin", 100);
  uint32_t kib = 0;
  CHECK(storage::removeLegacyImages(kib) == 7);  // the batch of eight, less the one that stays
  CHECK(fakes::fs().exists("/i0.bin"));
  CHECK(fakes::fs().exists("/i8.bin"));
  CHECK(fakes::fs().exists("/i9.bin"));
}
