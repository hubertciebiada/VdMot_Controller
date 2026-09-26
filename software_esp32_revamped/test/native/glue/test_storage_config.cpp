// src/storage.cpp: the config load order (every row of the load table), the
// cfgx blob, the backup files, the legacy import report and image cleanup,
// factory reset, and the file list and delete of the file manager.
#include <LittleFS.h>
#include <string.h>

#include <string>
#include <vector>

#include <vdm/file_manager.h>
#include <vdm/json_writer.h>

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

// A config with an ext key away from its default.
vdm::Config withExt(const char* station) {
  vdm::Config c = named(station);
  c.failsafe.timeoutMin = 90;
  vdm::copyString(c.valves[1].topic, sizeof c.valves[1].topic, "Bad/WC");
  return c;
}

void mount() {
  bool formatted = false;
  REQUIRE(storage::beginFs(formatted));
}

void storeNvs(const vdm::Config& c) {
  fakes::nvs().setBlob("vdmrev", "cfg", blobOf(c));
  fakes::nvs().setBlob("vdmrev", "cfgx", extOf(c));
}

void storeBackup(const vdm::Config& c) {
  fakes::fs().put("/sys/cfg.bak", str(blobOf(c)));
  fakes::fs().put("/sys/cfgx.bak", str(extOf(c)));
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

}  // namespace

// ---------------------------------------------------------------- load table

TEST_CASE("storage load: cfg and cfgx are used, no event, the backup follows once") {
  glue::begin();
  mount();
  const vdm::Config c = withExt("Stored");
  storeNvs(c);
  const Load l = load();
  CHECK(l.src == storage::LoadSource::Stored);
  CHECK(std::string(l.cfg.station) == "Stored");
  CHECK(l.cfg.failsafe.timeoutMin == 90);
  CHECK(std::string(l.cfg.valves[1].topic) == "Bad/WC");
  CHECK(l.details.errorCode == 0);
  CHECK(l.details.info.ext == vdm::ExtResult::Ok);
  CHECK(sib::logger().events.empty());
  CHECK_FALSE(fakes::fs().exists("/sys/cfg.bak"));
  storage::setActiveConfig(l.cfg);
  storage::service();
  CHECK(fakes::fs().read("/sys/cfg.bak") == str(blobOf(c)));
  CHECK(fakes::fs().read("/sys/cfgx.bak") == str(extOf(c)));
  CHECK_FALSE(fakes::fs().exists("/sys/cfg.bak.tmp"));
  CHECK_FALSE(fakes::fs().exists("/sys/cfgx.bak.tmp"));
  CHECK(fakes::journalOf("fs.rename") ==
        std::vector<std::string>{"fs.rename /sys/cfg.bak.tmp /sys/cfg.bak",
                                 "fs.rename /sys/cfgx.bak.tmp /sys/cfgx.bak"});
  // Nothing pending any more.
  const int writes = fakes::fs().writeOpens;
  storage::service();
  CHECK(fakes::fs().writeOpens == writes);
}

TEST_CASE("storage load: backup files equal to NVS are not written again") {
  glue::begin();
  mount();
  const vdm::Config c = withExt("Same");
  storeNvs(c);
  storeBackup(c);
  const Load l = load();
  CHECK(l.src == storage::LoadSource::Stored);
  storage::setActiveConfig(l.cfg);
  const int writes = fakes::fs().writeOpens;
  storage::service();
  CHECK(fakes::fs().writeOpens == writes);
}

TEST_CASE("storage load: a cfgx backup that differs is rewritten, also a missing one") {
  glue::begin();
  mount();
  const vdm::Config c = withExt("Diff");
  storeNvs(c);
  fakes::fs().put("/sys/cfg.bak", str(blobOf(c)));
  fakes::fs().put("/sys/cfgx.bak", str(extOf(named("Diff"))));
  const Load l = load();
  storage::setActiveConfig(l.cfg);
  storage::service();
  CHECK(fakes::fs().read("/sys/cfgx.bak") == str(extOf(c)));
}

TEST_CASE("storage load: a cfg without cfgx (2.0.0) loads with the new keys at defaults") {
  glue::begin();
  mount();
  fakes::nvs().setBlob("vdmrev", "cfg", blobOf(withExt("Old")));
  const Load l = load();
  CHECK(l.src == storage::LoadSource::Stored);
  CHECK(l.details.info.ext == vdm::ExtResult::Absent);
  CHECK(l.cfg.failsafe.timeoutMin == 60);
  CHECK(l.cfg.valves[1].topic[0] == '\0');
  CHECK(sib::logger().events.empty());
}

TEST_CASE("storage load: a damaged cfgx with a good cfg loads the new keys at defaults, the backup kept") {
  glue::begin();
  mount();
  const vdm::Config c = withExt("Ext");
  storeBackup(c);
  fakes::nvs().setBlob("vdmrev", "cfg", blobOf(c));
  fakes::nvs().setBlob("vdmrev", "cfgx", corrupt(extOf(c)));
  const Load l = load();
  CHECK(l.src == storage::LoadSource::Stored);
  CHECK(l.details.errorCode == 0);
  CHECK(l.details.info.ext == vdm::ExtResult::BadCrc);
  CHECK(std::string(l.cfg.station) == "Ext");
  CHECK(l.cfg.failsafe.timeoutMin == 60);
  CHECK(l.cfg.valves[1].topic[0] == '\0');
  CHECK(sib::logger().events.empty());
  // the backup still holds the new keys: only an explicit save replaces it
  storage::setActiveConfig(l.cfg);
  const int writes = fakes::fs().writeOpens;
  storage::service();
  CHECK(fakes::fs().writeOpens == writes);
  CHECK(fakes::fs().read("/sys/cfgx.bak") == str(extOf(c)));
}

TEST_CASE("storage load: repairs are logged once with the first key path") {
  glue::begin();
  mount();
  vdm::Config e = named("Rep");
  vdm::copyString(e.valves[0].name, sizeof e.valves[0].name, "Bad");
  vdm::Config broken = e;
  broken.calib.hour = 24;  // base blob: a field repair
  const std::vector<uint8_t> base = blobOf(broken);
  vdm::Config ext = e;
  vdm::copyString(ext.valves[3].topic, sizeof ext.valves[3].topic, "Bad");  // V2 after cfgx
  fakes::nvs().setBlob("vdmrev", "cfg", base);
  fakes::nvs().setBlob("vdmrev", "cfgx", extOf(ext));
  const Load l = load();
  CHECK(l.src == storage::LoadSource::Stored);
  CHECK(l.cfg.calib.hour == 0);
  CHECK(l.cfg.valves[3].topic[0] == '\0');
  const std::vector<vdm::Event> ev = sib::logger().withCode(vdm::EventCode::ConfigRepaired);
  REQUIRE(ev.size() == 1);
  CHECK(ev[0].arg1 == static_cast<int32_t>(vdm::kRepairField | vdm::kRepairTopics));
  CHECK(ev[0].arg2 == 2);
  CHECK(std::string(ev[0].text) == "calib.hour");
  CHECK(sib::logger().events.size() == 1);
}

TEST_CASE("storage load: a repair only after the ext records names that key") {
  glue::begin();
  mount();
  vdm::Config e = named("Rep");
  vdm::copyString(e.valves[0].name, sizeof e.valves[0].name, "Bad");
  vdm::Config ext = e;
  vdm::copyString(ext.valves[3].topic, sizeof ext.valves[3].topic, "Bad");
  fakes::nvs().setBlob("vdmrev", "cfg", blobOf(e));
  fakes::nvs().setBlob("vdmrev", "cfgx", extOf(ext));
  load();
  const std::vector<vdm::Event> ev = sib::logger().withCode(vdm::EventCode::ConfigRepaired);
  REQUIRE(ev.size() == 1);
  CHECK(ev[0].arg1 == static_cast<int32_t>(vdm::kRepairTopics));
  CHECK(ev[0].arg2 == 1);
  CHECK(std::string(ev[0].text) == "valves.4.topic");
}

TEST_CASE("storage load: a newer firmware's config is logged, its unknown keys survive a save") {
  glue::begin();
  mount();
  const vdm::Config c = named("Newer");
  std::vector<uint8_t> base = blobOf(c);
  base[4] = 2;  // base schema 2: read by its schema-1 prefix
  const uint32_t crc = vdm::crc32(base.data(), base.size() - 4);
  for (int i = 0; i < 4; ++i) base[base.size() - 4 + i] = static_cast<uint8_t>(crc >> (8 * i));
  const std::vector<uint8_t> unknown = {200, 0, 2, 7, 9};
  fakes::nvs().setBlob("vdmrev", "cfg", base);
  fakes::nvs().setBlob("vdmrev", "cfgx", extOf(c, unknown));
  const Load l = load();
  CHECK(l.src == storage::LoadSource::Stored);
  const std::vector<vdm::Event> ev = sib::logger().withCode(vdm::EventCode::ConfigNewerSchema);
  REQUIRE(ev.size() == 1);
  CHECK(ev[0].arg1 == 2);
  CHECK(ev[0].arg2 == 1);
  CHECK_FALSE(sib::logger().has(vdm::EventCode::ConfigRepaired));
  // The next save writes the unknown record back.
  char path[16] = "";
  REQUIRE(storage::applyConfig(named("Saved"), path, sizeof path));
  CHECK(fakes::nvs().getBlob("vdmrev", "cfgx") == extOf(named("Saved"), unknown));
}

TEST_CASE("storage load: only unknown cfgx records are logged as a newer firmware") {
  glue::begin();
  mount();
  const vdm::Config c = named("Ext");
  fakes::nvs().setBlob("vdmrev", "cfg", blobOf(c));
  fakes::nvs().setBlob("vdmrev", "cfgx", extOf(c, {201, 0, 0, 202, 1, 1, 5}));
  load();
  const std::vector<vdm::Event> ev = sib::logger().withCode(vdm::EventCode::ConfigNewerSchema);
  REQUIRE(ev.size() == 1);
  CHECK(ev[0].arg1 == 1);
  CHECK(ev[0].arg2 == 2);
}

TEST_CASE("storage load: an unusable cfg is replaced by the backup, NVS rewritten") {
  glue::begin();
  mount();
  const vdm::Config good = withExt("Backup");
  storeBackup(good);
  const std::vector<uint8_t> bad = corrupt(blobOf(named("Bad")));
  fakes::nvs().setBlob("vdmrev", "cfg", bad);
  fakes::nvs().setBlob("vdmrev", "cfgx", extOf(named("Bad")));
  const Load l = load();
  CHECK(l.src == storage::LoadSource::Backup);
  CHECK(storage::bootLoadSource() == storage::LoadSource::Backup);
  CHECK(std::string(l.cfg.station) == "Backup");
  CHECK(l.cfg.failsafe.timeoutMin == 90);
  CHECK(l.details.errorCode == static_cast<uint8_t>(vdm::DecodeResult::BadCrc));
  CHECK(fakes::nvs().getBlob("vdmrev", "cfg") == blobOf(good));
  CHECK(fakes::nvs().getBlob("vdmrev", "cfgx") == extOf(good));
  const std::vector<vdm::Event> ev = sib::logger().withCode(vdm::EventCode::ConfigRestored);
  REQUIRE(ev.size() == 1);
  CHECK(ev[0].arg1 == static_cast<int32_t>(vdm::DecodeResult::BadCrc));
  CHECK(sib::logger().events.size() == 1);
}

TEST_CASE("storage load: a backup without cfgx.bak restores the base and removes cfgx") {
  glue::begin();
  mount();
  fakes::fs().put("/sys/cfg.bak", str(blobOf(named("Base"))));
  fakes::nvs().setBlob("vdmrev", "cfg", corrupt(blobOf(named("Bad"))));
  fakes::nvs().setBlob("vdmrev", "cfgx", extOf(withExt("Bad")));
  const Load l = load();
  CHECK(l.src == storage::LoadSource::Backup);
  CHECK(std::string(l.cfg.station) == "Base");
  CHECK(l.cfg.failsafe.timeoutMin == 60);
  CHECK_FALSE(fakes::nvs().has("vdmrev", "cfgx"));
}

TEST_CASE("storage load: an unreadable cfg (too large) is replaced by the backup, reason 101") {
  glue::begin();
  mount();
  storeBackup(named("Backup"));
  fakes::nvs().setBlob("vdmrev", "cfg", std::vector<uint8_t>(vdm::kConfigBlobMax + 1, 1));
  const Load l = load();
  CHECK(l.src == storage::LoadSource::Backup);
  CHECK(l.details.errorCode == 101);
  CHECK(sib::logger().withCode(vdm::EventCode::ConfigRestored).at(0).arg1 == 101);
}

TEST_CASE("storage load: an unusable cfg and an unusable backup give defaults") {
  glue::begin();
  mount();
  fakes::fs().put("/sys/cfg.bak", str(corrupt(blobOf(named("B")))));
  const std::vector<uint8_t> bad = corrupt(blobOf(named("Bad")));
  fakes::nvs().setBlob("vdmrev", "cfg", bad);
  const Load l = load();
  CHECK(l.src == storage::LoadSource::DefaultsAfterError);
  CHECK(std::string(l.cfg.station) == "VdMot");
  CHECK(fakes::nvs().getBlob("vdmrev", "cfg") == bad);
  CHECK(sib::logger().withCode(vdm::EventCode::ConfigDefaults).at(0).arg1 ==
        static_cast<int32_t>(vdm::DecodeResult::BadCrc));
  CHECK_FALSE(sib::logger().has(vdm::EventCode::ConfigRestored));
}

TEST_CASE("storage load: without a mounted file system the backup is not tried") {
  glue::begin();
  storeBackup(named("Backup"));
  fakes::nvs().setBlob("vdmrev", "cfg", corrupt(blobOf(named("Bad"))));
  const Load l = load();
  CHECK(l.src == storage::LoadSource::DefaultsAfterError);
}

TEST_CASE("storage load: NVS erased after a factory reset gives defaults, not the backup") {
  glue::begin();
  mount();
  storeBackup(named("Backup"));
  fakes::nvs().setU8("vdmrev", "imported", 1);
  const Load l = load();
  CHECK(l.src == storage::LoadSource::Defaults);
  CHECK(std::string(l.cfg.station) == "VdMot");
  CHECK(sib::logger().events.empty());
}

TEST_CASE("storage load: an empty NVS without the import flag takes the backup, reason 0") {
  glue::begin();
  mount();
  storeBackup(withExt("Backup"));
  fakes::nvs().setStr("sysCfg", "stName", "Legacy");  // not imported: the backup wins
  const Load l = load();
  CHECK(l.src == storage::LoadSource::Backup);
  CHECK(std::string(l.cfg.station) == "Backup");
  CHECK(fakes::nvs().getBlob("vdmrev", "cfg") == blobOf(withExt("Backup")));
  CHECK(sib::logger().withCode(vdm::EventCode::ConfigRestored).at(0).arg1 == 0);
  CHECK_FALSE(sib::logger().has(vdm::EventCode::ConfigImported));
}

TEST_CASE("storage load: the legacy import writes the report, logs dropped features, removes images") {
  glue::begin();
  mount();
  fakes::nvs().setStr("sysCfg", "stName", "");
  fakes::nvs().setU8("protCfg", "brokerMQF", 0x02);
  fakes::nvs().setU8("msgCfg", "msgFlags", 1);
  std::vector<uint8_t> ctrl(12 * 20, 0);
  ctrl[0] = 0x01;
  ctrl[40] = 0x01;
  fakes::nvs().setBlob("valvesCtrlCfg", "valvesCtrl", ctrl);
  fakes::fs().put("/x.bin", std::string(1500, 'x'));
  fakes::fs().put("/Y.BIN", std::string(700, 'y'));
  fakes::fs().put("/HADiscovery.cfg", "list");
  fakes::fs().put("/stm/fw.bin", "img");
  const Load l = load();
  CHECK(l.src == storage::LoadSource::Imported);
  CHECK(std::string(l.cfg.mqtt.rootTopic) == "VdMotFBH");
  CHECK(fakes::nvs().getU("vdmrev", "imported") == 1);
  CHECK(fakes::nvs().has("vdmrev", "cfgx"));
  const std::vector<vdm::Event>& ev = sib::logger().events;
  REQUIRE(ev.size() == 3);
  CHECK(ev[0].code == vdm::EventCode::ConfigImported);
  CHECK(ev[1].code == vdm::EventCode::ImportDropped);
  CHECK(ev[1].arg1 == 2);
  CHECK(ev[1].arg2 == (vdm::kDroppedPi | vdm::kDroppedMessenger | vdm::kDroppedDs18Timeout));
  CHECK(std::string(ev[1].text) == "ignored " + std::to_string(l.report.ignored) + " keys");
  CHECK(ev[2].code == vdm::EventCode::FilesRemoved);
  CHECK(ev[2].arg1 == 2);
  CHECK(ev[2].arg2 == 3);  // 2200 bytes
  CHECK(std::string(ev[2].text) == "legacy images");
  CHECK_FALSE(fakes::fs().exists("/x.bin"));
  CHECK_FALSE(fakes::fs().exists("/Y.BIN"));
  CHECK(fakes::fs().exists("/HADiscovery.cfg"));
  CHECK(fakes::fs().exists("/stm/fw.bin"));
  static char expect[2048];
  vdm::JsonWriter jw(expect, sizeof expect);
  REQUIRE(vdm::writeImportReportJson(jw, l.report, l.cfg));
  CHECK(fakes::fs().read("/sys/import.json") == std::string(expect, jw.length()));
  CHECK(storage::hasImportReport());
  // The import save sets the backup flag.
  storage::setActiveConfig(l.cfg);
  storage::service();
  CHECK(fakes::fs().read("/sys/cfg.bak") == str(blobOf(l.cfg)));
}

TEST_CASE("storage load: an import without dropped features or images logs only the import") {
  glue::begin();
  mount();
  fakes::nvs().setStr("sysCfg", "stName", "Legacy");
  const Load l = load();
  CHECK(l.src == storage::LoadSource::Imported);
  REQUIRE(sib::logger().events.size() == 1);
  CHECK(sib::logger().events[0].code == vdm::EventCode::ConfigImported);
  CHECK(fakes::fs().exists("/sys/import.json"));
}

TEST_CASE("storage load: only the PI count also logs the dropped features") {
  glue::begin();
  mount();
  std::vector<uint8_t> ctrl(12 * 20, 0);
  ctrl[20] = 0x01;
  fakes::nvs().setBlob("valvesCtrlCfg", "valvesCtrl", ctrl);
  fakes::nvs().setStr("sysCfg", "stName", "Legacy");
  load();
  const std::vector<vdm::Event> ev = sib::logger().withCode(vdm::EventCode::ImportDropped);
  REQUIRE(ev.size() == 1);
  CHECK(ev[0].arg1 == 1);
  CHECK(ev[0].arg2 == vdm::kDroppedPi);
}

TEST_CASE("storage load: NVS not usable gives defaults with reason 100") {
  glue::begin();
  fakes::nvs().failOpen.insert("vdmrev");
  const Load l = load();
  CHECK(l.src == storage::LoadSource::DefaultsAfterError);
  CHECK(l.details.errorCode == 100);
  CHECK(storage::bootLoadDetails().errorCode == 100);
}

// ---------------------------------------------------------------- save and backup

TEST_CASE("storage save: cfgx is written before cfg; a failing cfgx write names nvs") {
  glue::begin();
  mount();
  storage::setActiveConfig(named("A"));
  const uint32_t rev = storage::configRevision();
  fakes::nvs().failSet.insert("cfgx");
  char path[16] = "";
  CHECK_FALSE(storage::applyConfig(withExt("B"), path, sizeof path));
  CHECK(std::string(path) == "nvs");
  CHECK_FALSE(fakes::nvs().has("vdmrev", "cfg"));
  CHECK(storage::configRevision() == rev);
  CHECK_FALSE(storage::configSavedSinceBoot());
  storage::service();
  CHECK_FALSE(fakes::fs().exists("/sys/cfg.bak"));  // nothing saved, nothing to back up
}

TEST_CASE("storage save: a failing cfg write after cfgx fails the save") {
  glue::begin();
  mount();
  fakes::nvs().failSet.insert("cfg");
  char path[16] = "";
  CHECK_FALSE(storage::applyConfig(withExt("B"), path, sizeof path));
  CHECK(std::string(path) == "nvs");
  CHECK(fakes::nvs().getBlob("vdmrev", "cfgx") == extOf(withExt("B")));
  CHECK_FALSE(fakes::nvs().has("vdmrev", "cfg"));
}

TEST_CASE("storage save: both blobs saved, the backup written by service()") {
  glue::begin();
  mount();
  char path[16] = "";
  REQUIRE(storage::applyConfig(withExt("S"), path, sizeof path));
  CHECK(fakes::nvs().getBlob("vdmrev", "cfg") == blobOf(withExt("S")));
  CHECK(fakes::nvs().getBlob("vdmrev", "cfgx") == extOf(withExt("S")));
  CHECK_FALSE(fakes::fs().exists("/sys/cfg.bak"));
  storage::service();
  CHECK(fakes::fs().read("/sys/cfg.bak") == str(blobOf(withExt("S"))));
  CHECK(fakes::fs().read("/sys/cfgx.bak") == str(extOf(withExt("S"))));
}

TEST_CASE("storage save: no backup while a network trial runs, written after it") {
  glue::begin();
  mount();
  storeBackup(named("Old"));
  char path[16] = "";
  REQUIRE(storage::applyConfig(named("Trial"), path, sizeof path));
  sib::net().trial.active = true;
  storage::service();
  CHECK(fakes::fs().read("/sys/cfg.bak") == str(blobOf(named("Old"))));
  sib::net().trial.active = false;
  storage::service();
  CHECK(fakes::fs().read("/sys/cfg.bak") == str(blobOf(named("Trial"))));
}

TEST_CASE("storage save: a failed backup rename keeps the old backup, removes the tmp files") {
  glue::begin();
  mount();
  storeBackup(named("Old"));
  char path[16] = "";
  REQUIRE(storage::applyConfig(named("New"), path, sizeof path));
  fakes::fs().fail("rename", "/sys/cfg.bak.tmp");
  storage::service();
  CHECK(fakes::fs().read("/sys/cfg.bak") == str(blobOf(named("Old"))));
  CHECK_FALSE(fakes::fs().exists("/sys/cfg.bak.tmp"));
  CHECK_FALSE(fakes::fs().exists("/sys/cfgx.bak.tmp"));
}

TEST_CASE("storage save: a failed tmp write renames nothing") {
  glue::begin();
  mount();
  storeBackup(named("Old"));
  char path[16] = "";
  REQUIRE(storage::applyConfig(named("New"), path, sizeof path));
  fakes::fs().fail("write", "/sys/cfg.bak.tmp");
  storage::service();
  CHECK(fakes::journalOf("fs.rename").empty());
  CHECK(fakes::fs().read("/sys/cfgx.bak") == str(extOf(named("Old"))));
  CHECK_FALSE(fakes::fs().exists("/sys/cfgx.bak.tmp"));
}

TEST_CASE("storage save: a failed rename of the ext keeps its tmp file, a load pairs it with the new base") {
  glue::begin();
  mount();
  storeBackup(withExt("Old"));  // failsafe timeout 90
  vdm::Config n = named("New");
  n.failsafe.timeoutMin = 30;
  char path[16] = "";
  REQUIRE(storage::applyConfig(n, path, sizeof path));
  fakes::fs().fail("rename", "/sys/cfgx.bak.tmp");
  storage::service();
  CHECK(fakes::fs().read("/sys/cfg.bak") == str(blobOf(n)));
  CHECK(fakes::fs().read("/sys/cfgx.bak") == str(extOf(withExt("Old"))));
  CHECK(fakes::fs().read("/sys/cfgx.bak.tmp") == str(extOf(n)));
  CHECK_FALSE(fakes::fs().exists("/sys/cfg.bak.tmp"));
  // NVS lost: the backup gives the pair of the last save
  fakes::nvs().setBlob("vdmrev", "cfg", corrupt(blobOf(n)));
  const Load l = load();
  CHECK(l.src == storage::LoadSource::Backup);
  CHECK(std::string(l.cfg.station) == "New");
  CHECK(l.cfg.failsafe.timeoutMin == 30);
  CHECK(fakes::nvs().getBlob("vdmrev", "cfgx") == extOf(n));
}

TEST_CASE("storage load: a backup cut between its renames uses cfgx.bak.tmp, one cut before them the old pair") {
  glue::begin();
  mount();
  vdm::Config n = named("New");
  n.failsafe.timeoutMin = 30;
  // cut after the rename of the base
  fakes::fs().put("/sys/cfg.bak", str(blobOf(n)));
  fakes::fs().put("/sys/cfgx.bak", str(extOf(withExt("Old"))));
  fakes::fs().put("/sys/cfgx.bak.tmp", str(extOf(n)));
  fakes::nvs().setBlob("vdmrev", "cfg", corrupt(blobOf(n)));
  Load l = load();
  CHECK(l.src == storage::LoadSource::Backup);
  CHECK(std::string(l.cfg.station) == "New");
  CHECK(l.cfg.failsafe.timeoutMin == 30);
  // cut before it: both tmp files next to the old pair
  storeBackup(withExt("Old"));
  fakes::fs().put("/sys/cfg.bak.tmp", str(blobOf(n)));
  fakes::nvs().setBlob("vdmrev", "cfg", corrupt(blobOf(n)));
  l = load();
  CHECK(l.src == storage::LoadSource::Backup);
  CHECK(std::string(l.cfg.station) == "Old");
  CHECK(l.cfg.failsafe.timeoutMin == 90);
}

TEST_CASE("storage save: the next backup first finishes one cut between its renames") {
  glue::begin();
  mount();
  vdm::Config first = named("First");
  first.failsafe.timeoutMin = 30;
  fakes::fs().put("/sys/cfg.bak", str(blobOf(first)));
  fakes::fs().put("/sys/cfgx.bak", str(extOf(withExt("Old"))));
  fakes::fs().put("/sys/cfgx.bak.tmp", str(extOf(first)));
  vdm::Config second = named("Second");
  second.failsafe.timeoutMin = 40;
  char path[16] = "";
  REQUIRE(storage::applyConfig(second, path, sizeof path));
  // the rename that finishes the cut backup fails: nothing else is written
  fakes::fs().fail("rename", "/sys/cfgx.bak.tmp");
  storage::service();
  CHECK(fakes::fs().read("/sys/cfg.bak") == str(blobOf(first)));
  CHECK(fakes::fs().read("/sys/cfgx.bak.tmp") == str(extOf(first)));
  CHECK_FALSE(fakes::fs().exists("/sys/cfg.bak.tmp"));
  // the next save finishes it, then writes its own pair
  REQUIRE(storage::applyConfig(second, path, sizeof path));
  fakes::journal().clear();
  storage::service();
  CHECK(fakes::journalOf("fs.rename") == std::vector<std::string>{
                                            "fs.rename /sys/cfgx.bak.tmp /sys/cfgx.bak",
                                            "fs.rename /sys/cfg.bak.tmp /sys/cfg.bak",
                                            "fs.rename /sys/cfgx.bak.tmp /sys/cfgx.bak"});
  CHECK(fakes::fs().read("/sys/cfg.bak") == str(blobOf(second)));
  CHECK(fakes::fs().read("/sys/cfgx.bak") == str(extOf(second)));
  CHECK_FALSE(fakes::fs().exists("/sys/cfgx.bak.tmp"));
}

TEST_CASE("storage factoryReset: the backup and report files go, the latch stays") {
  glue::begin();
  mount();
  storeBackup(named("B"));
  fakes::fs().put("/sys/import.json", "{}");
  fakes::fs().put("/sys/other", "x");
  fakes::nvs().setU8("vdmrev", "frLatch", 1);
  char path[16] = "";
  REQUIRE(storage::applyConfig(named("Pending"), path, sizeof path));
  CHECK(storage::factoryReset());
  CHECK_FALSE(fakes::fs().exists("/sys/cfg.bak"));
  CHECK_FALSE(fakes::fs().exists("/sys/cfgx.bak"));
  CHECK_FALSE(fakes::fs().exists("/sys/import.json"));
  CHECK(fakes::fs().exists("/sys/other"));
  CHECK(fakes::nvs().getU("vdmrev", "frLatch") == 1);
  // The save before the reset is not backed up afterwards.
  storage::service();
  CHECK_FALSE(fakes::fs().exists("/sys/cfg.bak"));
}

TEST_CASE("storage beginFs: /sys is created") {
  glue::begin();
  mount();
  CHECK(fakes::fs().nodes.at("/sys").dir);
}

// ---------------------------------------------------------------- import report

TEST_CASE("storage import report: written from the active config, dismissed once") {
  glue::begin();
  mount();
  vdm::Config c = named("R");
  vdm::copyString(c.mqtt.rootTopic, sizeof c.mqtt.rootTopic, "Root");
  storage::setActiveConfig(c);
  CHECK_FALSE(storage::hasImportReport());
  CHECK_FALSE(storage::dismissImportReport());
  vdm::ImportReport r;
  r.imported = 4;
  CHECK(storage::writeImportReport(r));
  char expect[512];
  vdm::JsonWriter jw(expect, sizeof expect);
  REQUIRE(vdm::writeImportReportJson(jw, r, c));
  CHECK(fakes::fs().read("/sys/import.json") == std::string(expect, jw.length()));
  CHECK(storage::hasImportReport());
  CHECK(storage::dismissImportReport());
  CHECK_FALSE(storage::hasImportReport());
  CHECK_FALSE(storage::dismissImportReport());
}

TEST_CASE("storage import report: nothing without a file system") {
  glue::begin();
  fakes::fs().put("/sys/import.json", "{}");
  CHECK_FALSE(storage::writeImportReport(vdm::ImportReport{}));
  CHECK_FALSE(storage::hasImportReport());
  CHECK_FALSE(storage::dismissImportReport());
}

TEST_CASE("storage import report: a failed write returns false") {
  glue::begin();
  mount();
  fakes::fs().fail("open", "/sys/import.json");
  CHECK_FALSE(storage::writeImportReport(vdm::ImportReport{}));
}

// ---------------------------------------------------------------- files

TEST_CASE("storage listFiles: the root and one level below, sizes and paths") {
  glue::begin();
  mount();
  fakes::fs().put("/a.bin", "12345");
  fakes::fs().put("/stm/fw.bin", "123");
  fakes::fs().put("/sys/deep/x", "1");  // two levels down: not listed
  fakes::fs().put("/zz", "");
  vdm::FileEntry out[8];
  bool truncated = true;
  const size_t n = storage::listFiles(out, 8, truncated);
  CHECK_FALSE(truncated);
  std::vector<std::string> paths;
  for (size_t i = 0; i < n; ++i) paths.push_back(out[i].path);
  CHECK(paths == std::vector<std::string>{"/a.bin", "/stm/fw.bin", "/zz"});
  CHECK(out[0].size == 5);
  CHECK(out[1].size == 3);
  CHECK(out[2].size == 0);
}

TEST_CASE("storage listFiles: 70 files give 64 and truncated") {
  glue::begin();
  mount();
  for (int i = 0; i < 35; ++i) {
    fakes::fs().put("/f" + std::to_string(100 + i), "x");
    fakes::fs().put("/stm/g" + std::to_string(100 + i) + ".part", "y");
  }
  static vdm::FileEntry out[64];
  bool truncated = false;
  CHECK(storage::listFiles(out, 64, truncated) == 64);
  CHECK(truncated);
  // Exactly full: not truncated.
  static vdm::FileEntry all[70];
  CHECK(storage::listFiles(all, 70, truncated) == 70);
  CHECK_FALSE(truncated);
}

TEST_CASE("storage listFiles: a full list in the subdirectory is truncated too") {
  glue::begin();
  mount();
  fakes::fs().put("/stm/a.part", "1");
  fakes::fs().put("/stm/b.part", "1");
  vdm::FileEntry out[1];
  bool truncated = false;
  CHECK(storage::listFiles(out, 1, truncated) == 1);
  CHECK(truncated);
  CHECK(std::string(out[0].path) == "/stm/a.part");
}

TEST_CASE("storage listFiles: nothing without a file system") {
  glue::begin();
  vdm::FileEntry out[4];
  bool truncated = true;
  CHECK(storage::listFiles(out, 4, truncated) == 0);
  CHECK_FALSE(truncated);
}

TEST_CASE("storage deleteFile: a deletable file is removed and logged") {
  glue::begin();
  mount();
  fakes::fs().put("/x.bin", std::string(2049, 'x'));
  CHECK(storage::deleteFile("/x.bin") == storage::FileResult::Ok);
  CHECK_FALSE(fakes::fs().exists("/x.bin"));
  const std::vector<vdm::Event> ev = sib::logger().withCode(vdm::EventCode::FilesRemoved);
  REQUIRE(ev.size() == 1);
  CHECK(ev[0].arg1 == 1);
  CHECK(ev[0].arg2 == 3);
  CHECK(std::string(ev[0].text) == "/x.bin");
}

TEST_CASE("storage deleteFile: every refusal") {
  glue::begin();
  mount();
  fakes::fs().put("/sys/cfg.bak", "b");
  fakes::fs().put("/stm/fw.bin", "i");
  fakes::fs().put("/y.txt", "y");
  CHECK(storage::deleteFile(nullptr) == storage::FileResult::BadPath);
  CHECK(storage::deleteFile("//x") == storage::FileResult::BadPath);
  CHECK(storage::deleteFile("/sys/cfg.bak") == storage::FileResult::Protected);
  CHECK(storage::deleteFile("/stm/fw.bin") == storage::FileResult::Protected);
  CHECK(storage::deleteFile("/missing.bin") == storage::FileResult::NotFound);
  CHECK(storage::deleteFile("/stm") == storage::FileResult::BadPath);  // a directory
  fakes::fs().fail("remove", "/y.txt");
  CHECK(storage::deleteFile("/y.txt") == storage::FileResult::Io);
  fakes::fs().fail("open", "/y.txt");
  CHECK(storage::deleteFile("/y.txt") == storage::FileResult::Io);
  CHECK(fakes::fs().exists("/sys/cfg.bak"));
  CHECK(fakes::fs().exists("/stm/fw.bin"));
  CHECK(fakes::fs().exists("/stm"));
  CHECK(fakes::fs().exists("/y.txt"));
  CHECK_FALSE(sib::logger().has(vdm::EventCode::FilesRemoved));
}

TEST_CASE("storage deleteFile: without a file system Io") {
  glue::begin();
  fakes::fs().put("/y.txt", "y");
  CHECK(storage::deleteFile("/y.txt") == storage::FileResult::Io);
}

TEST_CASE("storage removeLegacyImages: more images than one batch, directories and others kept") {
  glue::begin();
  mount();
  for (int i = 0; i < 10; ++i) fakes::fs().put("/img" + std::to_string(i) + ".Bin", std::string(1024, 'i'));
  fakes::fs().put("/keep.txt", "k");
  fakes::fs().mkdirs("/dir.bin");
  uint32_t kib = 99;
  CHECK(storage::removeLegacyImages(kib) == 10);
  CHECK(kib == 10);
  CHECK(fakes::fs().exists("/keep.txt"));
  CHECK(fakes::fs().exists("/dir.bin"));
  for (int i = 0; i < 10; ++i) CHECK_FALSE(fakes::fs().exists("/img" + std::to_string(i) + ".Bin"));
}

TEST_CASE("storage removeLegacyImages: a file that cannot be removed ends the run") {
  glue::begin();
  mount();
  for (int i = 0; i < 8; ++i) fakes::fs().put("/i" + std::to_string(i) + ".bin", "x");
  fakes::fs().fail("remove", "/i0.bin", 100);
  uint32_t kib = 0;
  CHECK(storage::removeLegacyImages(kib) == 7);
  CHECK(kib == 1);
  CHECK(fakes::fs().exists("/i0.bin"));
}

TEST_CASE("storage removeLegacyImages: nothing without a file system") {
  glue::begin();
  fakes::fs().put("/x.bin", "x");
  uint32_t kib = 5;
  CHECK(storage::removeLegacyImages(kib) == 0);
  CHECK(kib == 0);
}
