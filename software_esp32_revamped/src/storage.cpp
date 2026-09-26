#include "storage.h"

#include <Arduino.h>
#include <LittleFS.h>
#include <Preferences.h>
#include <freertos/FreeRTOS.h>
#include <freertos/semphr.h>
#include <nvs.h>
#include <nvs_flash.h>
#include <string.h>

#include <vdm/event_log.h>
#include <vdm/file_manager.h>
#include <vdm/image_store.h>
#include <vdm/json_writer.h>

#include "boot_alloc.h"
#include "logger.h"
#include "net.h"

namespace storage {

namespace {

bool gFsReady = false;
Preferences gPrefs;
bool gPrefsOpen = false;
LoadSource gBootSource = LoadSource::Defaults;
// Written once in setup(), before the other tasks start.
LoadDetails& gBootDetails = bootAlloc<LoadDetails>();
volatile bool gSavedSinceBoot = false;

StaticSemaphore_t gCfgMutexStorage;
SemaphoreHandle_t gCfgMutex = nullptr;
vdm::Config& gActive = bootAlloc<vdm::Config>();
volatile uint32_t gRevision = 0;
uint32_t gBootCount = 0;
using Blob = ObjArray<uint8_t, vdm::kConfigBlobMax>;
Blob& gBlob = bootAlloc<Blob>();  // encode/decode scratch, guarded by gCfgMutex
using ExtBlob = ObjArray<uint8_t, vdm::kConfigExtBlobMax>;
ExtBlob& gExt = bootAlloc<ExtBlob>();  // the same for cfgx
// Unknown cfgx records (a newer firmware's keys) read at boot and written back
// by every save. Guarded by gCfgMutex.
using Keep = ObjArray<uint8_t, vdm::kConfigExtKeepMax>;
Keep& gKeep = bootAlloc<Keep>();
size_t gKeepLen = 0;
// The blobs in NVS differ from the backup files (app task writes them).
volatile bool gBackupPending = false;

// Image index, upload and last_good copy state (guarded by gFsMutex; file
// I/O happens outside the lock except for the short open/rename steps).
StaticSemaphore_t gFsMutexStorage;
SemaphoreHandle_t gFsMutex = nullptr;
ImageEntry gImages[kImageSlots];

struct Upload {
  bool active = false;
  fs::File file;
  char name[kImageNameMax + 1] = {0};
  uint32_t written = 0;
  uint32_t crc = 0;
} gUpload;

struct Copy {
  bool pending = false;
  bool running = false;
  char name[kImageNameMax + 1] = {0};
  fs::File src;
  fs::File dst;
  uint32_t done = 0;
} gCopy;
constexpr size_t kCopyChunk = 1024;
constexpr size_t kCopyChunksPerCall = 8;  // 8 KiB per app task pass

void ensureMutexes() {
  if (gCfgMutex == nullptr) gCfgMutex = xSemaphoreCreateMutexStatic(&gCfgMutexStorage);
  if (gFsMutex == nullptr) gFsMutex = xSemaphoreCreateMutexStatic(&gFsMutexStorage);
}

bool openPrefs() {
  if (!gPrefsOpen) gPrefsOpen = gPrefs.begin(kNamespace, false);
  return gPrefsOpen;
}

// Read-only view of the legacy namespaces through the raw NVS API (types are
// checked by trying the getters of every integer width).
class NvsLegacyReader : public vdm::LegacyNvsReader {
 public:
  bool readInt(const char* ns, const char* key, int64_t& out) override {
    nvs_handle_t h;
    if (nvs_open(ns, NVS_READONLY, &h) != ESP_OK) return false;
    bool ok = true;
    uint8_t u8;
    int8_t i8;
    uint16_t u16;
    int16_t i16;
    uint32_t u32;
    int32_t i32;
    int64_t i64;
    if (nvs_get_u8(h, key, &u8) == ESP_OK) out = u8;
    else if (nvs_get_i8(h, key, &i8) == ESP_OK) out = i8;
    else if (nvs_get_u16(h, key, &u16) == ESP_OK) out = u16;
    else if (nvs_get_i16(h, key, &i16) == ESP_OK) out = i16;
    else if (nvs_get_u32(h, key, &u32) == ESP_OK) out = u32;
    else if (nvs_get_i32(h, key, &i32) == ESP_OK) out = i32;
    else if (nvs_get_i64(h, key, &i64) == ESP_OK) out = i64;
    else ok = false;
    nvs_close(h);
    return ok;
  }

  bool readString(const char* ns, const char* key, char* out, size_t cap,
                  bool& truncated) override {
    truncated = false;
    if (out == nullptr || cap == 0) return false;
    nvs_handle_t h;
    if (nvs_open(ns, NVS_READONLY, &h) != ESP_OK) return false;
    size_t len = 0;  // stored length including the NUL
    bool ok = nvs_get_str(h, key, nullptr, &len) == ESP_OK && len > 0;
    if (ok) {
      truncated = len > cap;
      if (truncated) {
        out[0] = '\0';
      } else {
        ok = nvs_get_str(h, key, out, &len) == ESP_OK;
        out[cap - 1] = '\0';
      }
    }
    nvs_close(h);
    return ok;
  }

  bool readBlob(const char* ns, const char* key, uint8_t* out, size_t cap,
                size_t& storedLen) override {
    nvs_handle_t h;
    if (nvs_open(ns, NVS_READONLY, &h) != ESP_OK) return false;
    size_t len = 0;
    bool ok = nvs_get_blob(h, key, nullptr, &len) == ESP_OK;
    if (ok) {
      storedLen = len;
      if (len <= cap) {
        ok = nvs_get_blob(h, key, out, &len) == ESP_OK;
      } else if (out != nullptr) {
        // NVS cannot read a prefix; the importer rejects the size anyway.
        memset(out, 0, cap);
      }
    }
    nvs_close(h);
    return ok;
  }
};

class Lock {
 public:
  explicit Lock(SemaphoreHandle_t& m) : m_(m) {
    ensureMutexes();
    xSemaphoreTake(m_, portMAX_DELAY);
  }
  ~Lock() { xSemaphoreGive(m_); }
  Lock(const Lock&) = delete;
  Lock& operator=(const Lock&) = delete;

 private:
  SemaphoreHandle_t& m_;
};

// Serialises every Preferences access (it is not thread-safe) and the
// active config.
struct CfgLock : Lock {
  CfgLock() : Lock(gCfgMutex) {}
};
struct FsLock : Lock {
  FsLock() : Lock(gFsMutex) {}
};

// cfgx first: a cfg without its cfgx loads with the new keys at their
// defaults, a cfgx without its cfg is never read.
bool saveBlobLocked(const vdm::Config& c) {
  const size_t x = vdm::encodeConfigExt(c, gExt.data(), sizeof gExt.items, gKeep.data(), gKeepLen);
  const size_t n = vdm::encodeConfig(c, gBlob.data(), sizeof gBlob.items);
  const bool ok = x > 0 && n > 0 && openPrefs() &&
                  gPrefs.putBytes(kKeyConfigExt, gExt.data(), x) == x &&
                  gPrefs.putBytes(kKeyConfig, gBlob.data(), n) == n;
  if (ok) gBackupPending = true;
  return ok;
}

// A stored blob of at most `cap` bytes; 0 when it is absent or larger.
size_t loadBytesLocked(const char* key, uint8_t* out, size_t cap) {
  if (out == nullptr || !openPrefs()) return 0;
  const size_t len = gPrefs.getBytesLength(key);
  if (len == 0 || len > cap) return 0;
  return gPrefs.getBytes(key, out, len) == len ? len : 0;
}

bool saveBytesLocked(const char* key, const uint8_t* data, size_t len) {
  return data != nullptr && len > 0 && openPrefs() && gPrefs.putBytes(key, data, len) == len;
}

// u8 flag: 1 = set, absent = clear.
bool flagLocked(const char* key) { return openPrefs() && gPrefs.getUChar(key, 0) == 1; }

void setFlagLocked(const char* key, bool on) {
  if (!openPrefs()) return;
  if (on) {
    gPrefs.putUChar(key, 1);
  } else {
    gPrefs.remove(key);  // false when it was not there: nothing to do
  }
}

// ---------------------------------------------------------------- images

bool endsWith(const char* s, size_t len, const char* suffix) {
  const size_t n = strlen(suffix);
  return len >= n && memcmp(s + len - n, suffix, n) == 0;
}

// Index slot of `name`, -1 if absent. Caller holds gFsMutex.
int findLocked(const char* name) {
  for (size_t i = 0; i < kImageSlots; ++i) {
    if (gImages[i].name[0] != '\0' && strcmp(gImages[i].name, name) == 0) {
      return static_cast<int>(i);
    }
  }
  return -1;
}

// Adds or refreshes an index entry (unscanned). Caller holds gFsMutex.
bool putLocked(const char* name, uint32_t size) {
  int i = findLocked(name);
  if (i < 0) {
    for (size_t k = 0; k < kImageSlots && i < 0; ++k) {
      if (gImages[k].name[0] == '\0') i = static_cast<int>(k);
    }
  }
  if (i < 0) return false;
  ImageEntry& e = gImages[i];
  e = ImageEntry{};
  vdm::copyString(e.name, sizeof e.name, name);
  e.size = size;
  return true;
}

size_t uploadedCountLocked(const char* except) {
  size_t n = 0;
  for (const ImageEntry& e : gImages) {
    if (e.name[0] == '\0' || strcmp(e.name, kLastGoodName) == 0) continue;
    if (except != nullptr && strcmp(e.name, except) == 0) continue;
    ++n;
  }
  return n;
}

size_t fsFree() {
  const size_t total = LittleFS.totalBytes();
  const size_t used = LittleFS.usedBytes();
  return total > used ? total - used : 0;
}

// Builds the index from /stm and removes upload leftovers. Boot only.
void indexImages() {
  constexpr size_t kMaxLeftovers = 8;
  char leftovers[kMaxLeftovers][48];
  size_t nLeft = 0;
  fs::File dir = LittleFS.open("/stm");
  if (!dir || !dir.isDirectory()) return;
  for (fs::File f = dir.openNextFile(); f; f = dir.openNextFile()) {
    if (f.isDirectory()) continue;
    const char* base = f.name();
    const size_t len = strlen(base);
    if (endsWith(base, len, ".part")) {
      if (nLeft < kMaxLeftovers) snprintf(leftovers[nLeft++], sizeof leftovers[0], "/stm/%s", base);
      continue;
    }
    char name[kImageNameMax + 1];
    if (!endsWith(base, len, ".bin") || !normalizeImageName(base, len, name, sizeof name)) continue;
    FsLock lock;
    putLocked(name, static_cast<uint32_t>(f.size()));
  }
  dir.close();
  for (size_t i = 0; i < nLeft; ++i) LittleFS.remove(leftovers[i]);
}

void closeUploadLocked(bool removePart) {
  if (gUpload.file) gUpload.file.close();
  if (removePart) {
    char path[48];
    if (imagePath(gUpload.name, true, path, sizeof path)) LittleFS.remove(path);
  }
  gUpload.active = false;
}

void stopCopy(bool ok) {
  if (gCopy.src) gCopy.src.close();
  if (gCopy.dst) gCopy.dst.close();
  char part[48];
  char final[48];
  imagePath(kLastGoodName, true, part, sizeof part);
  imagePath(kLastGoodName, false, final, sizeof final);
  if (ok) {
    LittleFS.remove(final);
    ok = LittleFS.rename(part, final);
  }
  if (!ok) LittleFS.remove(part);
  FsLock lock;
  if (ok) {
    putLocked(kLastGoodName, gCopy.done);
  } else {
    // The old last_good was removed only on success; keep the index honest.
    const int i = findLocked(kLastGoodName);
    if (i >= 0 && !LittleFS.exists(final)) gImages[i] = ImageEntry{};
  }
  gCopy.running = false;
}

// One bounded step of the last_good copy.
void serviceCopy() {
  if (!gCopy.running) {
    char name[kImageNameMax + 1];
    {
      FsLock lock;
      if (!gCopy.pending) return;
      gCopy.pending = false;
      vdm::copyString(name, sizeof name, gCopy.name);
    }
    if (strcmp(name, kLastGoodName) == 0) return;  // flashed last_good itself
    char src[48];
    char part[48];
    if (!imagePath(name, false, src, sizeof src) ||
        !imagePath(kLastGoodName, true, part, sizeof part)) {
      return;
    }
    gCopy.src = LittleFS.open(src, FILE_READ);
    if (!gCopy.src) return;
    if (fsFree() < gCopy.src.size() + 16 * 1024) {
      gCopy.src.close();
      logger::log(vdm::EventCode::StmFlashFailed, vdm::kNoValve, 0, 0, "last_good: no space");
      return;
    }
    {
      // deleteFile() refuses the .part of a running copy: running is set
      // with the file created
      FsLock lock;
      gCopy.dst = LittleFS.open(part, FILE_WRITE);
      gCopy.running = static_cast<bool>(gCopy.dst);
    }
    if (!gCopy.running) {
      gCopy.src.close();
      return;
    }
    gCopy.done = 0;
  }
  static uint8_t buf[kCopyChunk];
  for (size_t i = 0; i < kCopyChunksPerCall; ++i) {
    const size_t n = gCopy.src.read(buf, sizeof buf);
    if (n == 0) {
      stopCopy(gCopy.done == gCopy.src.size() && gCopy.done > 0);
      return;
    }
    if (gCopy.dst.write(buf, n) != n) {
      stopCopy(false);
      return;
    }
    gCopy.done += static_cast<uint32_t>(n);
  }
}

// Validates one unscanned image (reads the whole file; runs in the app task).
void serviceScan() {
  ImageEntry e;
  {
    FsLock lock;
    int idx = -1;
    for (size_t i = 0; i < kImageSlots && idx < 0; ++i) {
      if (gImages[i].name[0] != '\0' && !gImages[i].scanned) idx = static_cast<int>(i);
    }
    if (idx < 0 || (gUpload.active && strcmp(gUpload.name, gImages[idx].name) == 0)) return;
    e = gImages[idx];
  }
  FileImage img;
  vdm::ImageInfo info;
  vdm::FlashError err = vdm::FlashError::ImageRead;
  if (img.open(e.name)) err = vdm::validateImage(img, 0, true, info);
  const uint32_t size = img.size();
  img.close();
  FsLock lock;
  const int i = findLocked(e.name);
  if (i < 0 || gImages[i].size != size) return;  // replaced meanwhile: scan again later
  gImages[i].scanned = true;
  gImages[i].check = err;
  gImages[i].crc = info.crc;
  vdm::copyString(gImages[i].version, sizeof gImages[i].version, info.version);
  vdm::copyString(gImages[i].hwTag, sizeof gImages[i].hwTag, info.hwTag);
}

// ---------------------------------------------------------------- config files

constexpr const char* kBackupBaseTmp = "/sys/cfg.bak.tmp";
constexpr const char* kBackupExtTmp = "/sys/cfgx.bak.tmp";

uint32_t kibOf(uint32_t bytes) { return (bytes + 1023u) / 1024u; }

// A file of at most `cap` bytes; 0 when it is missing, empty, larger or
// unreadable.
size_t readFile(const char* path, uint8_t* out, size_t cap) {
  if (!LittleFS.exists(path)) return 0;
  fs::File f = LittleFS.open(path, FILE_READ);
  if (!f) return 0;
  const size_t len = f.size();
  const bool ok = len <= cap && f.read(out, len) == len;
  f.close();
  return ok ? len : 0;
}

bool writeFile(const char* path, const uint8_t* data, size_t len) {
  fs::File f = LittleFS.open(path, FILE_WRITE);
  if (!f) return false;
  const bool ok = f.write(data, len) == len;
  f.close();
  return ok;
}

// The file holds exactly these bytes.
bool sameFile(const char* path, const uint8_t* data, size_t len) {
  if (!LittleFS.exists(path)) return false;
  fs::File f = LittleFS.open(path, FILE_READ);
  if (!f) return false;
  bool same = f.size() == len;
  uint8_t chunk[64];
  for (size_t pos = 0; same && pos < len;) {
    const size_t n = len - pos < sizeof chunk ? len - pos : sizeof chunk;
    same = f.read(chunk, n) == n && memcmp(chunk, data + pos, n) == 0;
    pos += n;
  }
  f.close();
  return same;
}

// The blobs of `c` (with the kept unknown records) into gBlob/gExt; false
// when they do not fit. Caller holds gCfgMutex.
bool encodeLocked(const vdm::Config& c, size_t& n, size_t& x) {
  x = vdm::encodeConfigExt(c, gExt.data(), sizeof gExt.items, gKeep.data(), gKeepLen);
  n = vdm::encodeConfig(c, gBlob.data(), sizeof gBlob.items);
  return n > 0 && x > 0;
}

// A backup write cut off between its two renames: /sys/cfg.bak is new and
// its ext blob is still in cfgx.bak.tmp (see writeBackup()).
bool backupCut() { return !LittleFS.exists(kBackupBaseTmp) && LittleFS.exists(kBackupExtTmp); }

// /sys/cfg.bak and /sys/cfgx.bak from the active config, a pair that a load
// takes together. Both are written to .tmp files first (base, then ext) and
// renamed over the old ones in the same order, so a power cut never leaves
// a pair of two saves: cfgx.bak.tmp without cfg.bak.tmp is the ext blob of
// the renamed base (backupCut()), any other .tmp file belongs to a write
// that renamed nothing and left the old pair.
void writeBackup() {
  CfgLock lock;
  gBackupPending = false;
  // New .tmp files must not meet the ext blob of a cut write: it goes first.
  if (backupCut() && !LittleFS.rename(kBackupExtTmp, kBackupExt)) return;
  size_t n = 0;
  size_t x = 0;
  const bool ok = encodeLocked(gActive, n, x) && writeFile(kBackupBaseTmp, gBlob.data(), n) &&
                  writeFile(kBackupExtTmp, gExt.data(), x) &&
                  LittleFS.rename(kBackupBaseTmp, kBackupBase);
  if (!ok) {
    // The ext first: a cut in between leaves cfg.bak.tmp, still a write
    // that renamed nothing.
    LittleFS.remove(kBackupExtTmp);
    LittleFS.remove(kBackupBaseTmp);
    return;
  }
  // A failure keeps cfgx.bak.tmp as the pair of the new base.
  LittleFS.rename(kBackupExtTmp, kBackupExt);
}

}  // namespace

// ---------------------------------------------------------------- FS / config

bool beginFs(bool& formatted) {
  ensureMutexes();
  formatted = false;
  gFsReady = LittleFS.begin(false);
  if (!gFsReady) {
    formatted = LittleFS.format();
    gFsReady = formatted && LittleFS.begin(false);
  }
  if (gFsReady) {
    if (!LittleFS.exists("/stm")) LittleFS.mkdir("/stm");
    if (!LittleFS.exists("/log")) LittleFS.mkdir("/log");
    if (!LittleFS.exists("/sys")) LittleFS.mkdir("/sys");
    indexImages();
  }
  return gFsReady;
}

bool fsReady() { return gFsReady; }

namespace {

// The backup files as the config; on success NVS gets their bytes back.
// Caller holds gCfgMutex.
bool loadBackupLocked(vdm::Config& out, vdm::LoadInfo& info) {
  if (!gFsReady) return false;
  const size_t n = readFile(kBackupBase, gBlob.data(), sizeof gBlob.items);
  const size_t x =
      readFile(backupCut() ? kBackupExtTmp : kBackupExt, gExt.data(), sizeof gExt.items);
  vdm::StoredBlobs b;
  b.base = gBlob.data();
  b.baseLen = n;
  b.ext = gExt.data();
  b.extLen = x;
  if (!vdm::loadConfigBlobs(b, out, info, gKeep.data(), sizeof gKeep.items)) return false;
  gKeepLen = info.extInfo.keepLen;
  if (x > 0) {
    gPrefs.putBytes(kKeyConfigExt, gExt.data(), x);
  } else {
    gPrefs.remove(kKeyConfigExt);
  }
  gPrefs.putBytes(kKeyConfig, gBlob.data(), n);
  return true;
}

// The load order of DESIGN.md "Persistence": NVS, the backup files, the
// legacy import, the defaults.
LoadSource loadStored(vdm::Config& out, vdm::ImportReport& report, LoadDetails& details) {
  CfgLock lock;
  vdm::setDefaults(out);
  gKeepLen = 0;
  if (!openPrefs()) {
    details.errorCode = 100;
    return LoadSource::DefaultsAfterError;
  }

  const size_t len = gPrefs.getBytesLength(kKeyConfig);
  if (len > 0) {
    details.errorCode = 101;
    if (len <= sizeof gBlob.items && gPrefs.getBytes(kKeyConfig, gBlob.data(), len) == len) {
      vdm::StoredBlobs b;
      b.base = gBlob.data();
      b.baseLen = len;
      b.ext = gExt.data();
      b.extLen = loadBytesLocked(kKeyConfigExt, gExt.data(), sizeof gExt.items);
      if (vdm::loadConfigBlobs(b, out, details.info, gKeep.data(), sizeof gKeep.items)) {
        details.errorCode = 0;
        gKeepLen = details.info.extInfo.keepLen;
        // First boot after the upgrade (or a lost file): the backup follows.
        // A damaged cfgx left the new keys at their defaults; the backup may
        // still hold them, so only the next explicit save replaces it.
        const bool extDamaged = details.info.ext != vdm::ExtResult::Absent &&
                                details.info.ext != vdm::ExtResult::Ok;
        size_t n = 0;
        size_t x = 0;
        gBackupPending = !extDamaged && encodeLocked(out, n, x) && gFsReady &&
                         !(sameFile(kBackupBase, gBlob.data(), n) &&
                           sameFile(kBackupExt, gExt.data(), x));
        return LoadSource::Stored;
      }
      details.errorCode = static_cast<uint8_t>(details.info.base);
    }
    // Never overwritten automatically without a usable backup: the next
    // explicit save replaces it.
    if (loadBackupLocked(out, details.info)) return LoadSource::Backup;
    vdm::setDefaults(out);
    return LoadSource::DefaultsAfterError;
  }
  if (gPrefs.getUChar(kKeyImported, 0) == 1) return LoadSource::Defaults;
  if (loadBackupLocked(out, details.info)) return LoadSource::Backup;  // NVS was erased
  vdm::setDefaults(out);

  NvsLegacyReader reader;
  report = vdm::importLegacyConfig(reader, out);
  if (report.lastCalibEpoch > 0) gPrefs.putLong64(kKeyLastCalib, report.lastCalibEpoch);
  // "imported" only after the blob is safely stored: a failed save retries
  // the (idempotent) import on the next boot.
  if (saveBlobLocked(out)) gPrefs.putUChar(kKeyImported, 1);
  return report.anyLegacy ? LoadSource::Imported : LoadSource::Defaults;
}

// /sys/import.json from the report and the imported config. Caller holds
// gCfgMutex (gBlob is the text buffer).
bool writeReportLocked(const vdm::ImportReport& report, const vdm::Config& c) {
  if (!gFsReady) return false;
  char* buf = reinterpret_cast<char*>(gBlob.data());
  vdm::JsonWriter jw(buf, sizeof gBlob.items);
  return vdm::writeImportReportJson(jw, report, c) &&
         writeFile(kImportReportFile, gBlob.data(), jw.length());
}

void logStored(const LoadDetails& d) {
  const vdm::Repairs& a = d.info.decode.repairs;
  const vdm::Repairs& b = d.info.repairs;
  if ((a.mask | b.mask) != 0) {
    logger::log(vdm::EventCode::ConfigRepaired, vdm::kNoValve, static_cast<int32_t>(a.mask | b.mask),
                a.count + b.count, a.first[0] != '\0' ? a.first : b.first);
  }
  if (d.info.decode.newerSchema || d.info.extInfo.unknown > 0) {
    logger::log(vdm::EventCode::ConfigNewerSchema, vdm::kNoValve, d.info.decode.schema,
                d.info.extInfo.unknown);
  }
}

void logImported(const vdm::ImportReport& report, const vdm::Config& c) {
  logger::log(vdm::EventCode::ConfigImported, vdm::kNoValve, report.imported, report.rejected,
              report.firstRejected);
  if (report.dropped != 0 || report.piValves != 0) {
    char text[24];
    snprintf(text, sizeof text, "ignored %u keys", static_cast<unsigned>(report.ignored));
    logger::log(vdm::EventCode::ImportDropped, vdm::kNoValve, report.piValves, report.dropped,
                text);
  }
  {
    CfgLock lock;
    writeReportLocked(report, c);
  }
  uint32_t kib = 0;
  const uint32_t files = removeLegacyImages(kib);
  if (files > 0) {
    logger::log(vdm::EventCode::FilesRemoved, vdm::kNoValve, static_cast<int32_t>(files),
                static_cast<int32_t>(kib), "legacy images");
  }
}

}  // namespace

LoadSource loadConfig(vdm::Config& out, vdm::ImportReport& report, LoadDetails& details) {
  details = LoadDetails{};
  const LoadSource src = loadStored(out, report, details);
  switch (src) {
    case LoadSource::Stored:
      logStored(details);
      break;
    case LoadSource::Backup:
      logger::log(vdm::EventCode::ConfigRestored, vdm::kNoValve, details.errorCode);
      break;
    case LoadSource::Imported:
      logImported(report, out);
      break;
    case LoadSource::DefaultsAfterError:
      logger::log(vdm::EventCode::ConfigDefaults, vdm::kNoValve, details.errorCode);
      break;
    case LoadSource::Defaults:
      break;
  }
  gBootSource = src;
  gBootDetails = details;
  return src;
}

LoadSource bootLoadSource() { return gBootSource; }

const LoadDetails& bootLoadDetails() { return gBootDetails; }

bool configSavedSinceBoot() { return gSavedSinceBoot; }

void setActiveConfig(const vdm::Config& c) {
  CfgLock lock;
  gActive = c;
  gRevision = gRevision + 1;
}

void getConfig(vdm::Config& out) {
  CfgLock lock;
  out = gActive;
}

uint32_t configRevision() { return gRevision; }

bool applyConfig(const vdm::Config& c, char* path, size_t pathCap) {
  if (!vdm::validateConfig(c, path, pathCap)) return false;
  CfgLock lock;
  const bool ok = saveBlobLocked(c);  // sets gBackupPending
  if (ok) {
    gActive = c;
    gRevision = gRevision + 1;
    gSavedSinceBoot = true;
  } else if (path && pathCap) {
    vdm::copyString(path, pathCap, "nvs");
  }
  return ok;
}

bool factoryReset() {
  CfgLock lock;
  if (gFsReady) {
    for (const char* f : {kBackupBase, kBackupExt, kImportReportFile}) LittleFS.remove(f);
  }
  gBackupPending = false;
  gKeepLen = 0;
  if (!openPrefs()) return false;
  // The latch must survive: the pin is still set and must not reset again.
  const bool latched = flagLocked(kKeyFactoryLatch);
  const bool ok = gPrefs.clear();
  setFlagLocked(kKeyFactoryLatch, latched);
  return gPrefs.putUChar(kKeyImported, 1) == 1 && ok;
}

uint32_t incrementBootCount() {
  CfgLock lock;
  if (!openPrefs()) return 0;
  gBootCount = gPrefs.getULong(kKeyBootCount, 0) + 1;
  gPrefs.putULong(kKeyBootCount, gBootCount);
  return gBootCount;
}

uint32_t bootCount() { return gBootCount; }

uint32_t loadCalibSlot() {
  CfgLock lock;
  return openPrefs() ? gPrefs.getULong(kKeyCalibSlot, 0) : 0;
}

void saveCalibSlot(uint32_t slot) {
  CfgLock lock;
  if (openPrefs()) gPrefs.putULong(kKeyCalibSlot, slot);
}

int64_t loadLastCalib() {
  CfgLock lock;
  return openPrefs() ? gPrefs.getLong64(kKeyLastCalib, 0) : 0;
}

void saveLastCalib(int64_t epoch) {
  CfgLock lock;
  if (openPrefs() && epoch > 0) gPrefs.putLong64(kKeyLastCalib, epoch);
}

bool haCleanupDone() {
  CfgLock lock;
  return openPrefs() && gPrefs.getUChar(kKeyHaCleanup, 0) == 1;
}

void setHaCleanupDone() {
  CfgLock lock;
  if (openPrefs()) gPrefs.putUChar(kKeyHaCleanup, 1);
}

bool saveTargets(const uint8_t* data, size_t len) {
  CfgLock lock;
  return saveBytesLocked(kKeyTargets, data, len);
}

size_t loadTargets(uint8_t* out, size_t cap) {
  CfgLock lock;
  return loadBytesLocked(kKeyTargets, out, cap);
}

size_t loadNetTrialBlob(uint8_t* out, size_t cap) {
  CfgLock lock;
  return loadBytesLocked(kKeyNetTrial, out, cap);
}

bool saveNetTrialBlob(const uint8_t* data, size_t len) {
  CfgLock lock;
  return saveBytesLocked(kKeyNetTrial, data, len);
}

void clearNetTrial() {
  CfgLock lock;
  if (openPrefs()) gPrefs.remove(kKeyNetTrial);
}

bool factoryLatched() {
  CfgLock lock;
  return flagLocked(kKeyFactoryLatch);
}

void setFactoryLatched(bool on) {
  CfgLock lock;
  setFlagLocked(kKeyFactoryLatch, on);
}

bool otaStmRequired() {
  CfgLock lock;
  return flagLocked(kKeyOtaStm);
}

void setOtaStmRequired(bool on) {
  CfgLock lock;
  setFlagLocked(kKeyOtaStm, on);
}

void clearOtaStmRequired() { setOtaStmRequired(false); }

uint8_t haLayout() {
  CfgLock lock;
  return openPrefs() ? gPrefs.getUChar(kKeyHaLayout, 0) : 0;
}

void setHaLayout(uint8_t layout) {
  CfgLock lock;
  if (openPrefs()) gPrefs.putUChar(kKeyHaLayout, layout);
}

// ---------------------------------------------------------------- files

bool writeImportReport(const vdm::ImportReport& report) {
  CfgLock lock;
  return writeReportLocked(report, gActive);
}

bool hasImportReport() { return gFsReady && LittleFS.exists(kImportReportFile); }

bool dismissImportReport() { return gFsReady && LittleFS.remove(kImportReportFile); }

namespace {

// One listed file; false when the list is full (truncated).
bool addFile(fs::File& f, vdm::FileEntry* out, size_t max, size_t& n) {
  if (n == max) return false;
  if (vdm::copyString(out[n].path, sizeof out[n].path, f.path())) {
    out[n].size = static_cast<uint32_t>(f.size());
    ++n;
  }
  return true;
}

}  // namespace

size_t listFiles(vdm::FileEntry* out, size_t max, bool& truncated) {
  truncated = false;
  size_t n = 0;
  if (!gFsReady) return 0;
  fs::File root = LittleFS.open("/");
  for (fs::File f = root.openNextFile(); f && !truncated; f = root.openNextFile()) {
    if (!f.isDirectory()) {
      truncated = !addFile(f, out, max, n);
      continue;
    }
    // One level below the root.
    for (fs::File g = f.openNextFile(); g && !truncated; g = f.openNextFile()) {
      if (!g.isDirectory()) truncated = !addFile(g, out, max, n);
    }
  }
  return n;
}

FileResult deleteFile(const char* path) {
  const size_t len = path != nullptr ? strlen(path) : 0;
  if (!vdm::fsPathValid(path, len)) return FileResult::BadPath;
  if (!vdm::fileDeletable(vdm::classifyFsPath(path, len))) return FileResult::Protected;
  if (!gFsReady) return FileResult::Io;
  uint32_t size = 0;
  {
    // The running upload writes its .part under gFsMutex; the last_good copy
    // writes its own outside it and sets running with the file under it.
    FsLock lock;
    char part[48];
    if (gUpload.active && imagePath(gUpload.name, true, part, sizeof part) &&
        strcmp(part, path) == 0) {
      return FileResult::Protected;
    }
    if (gCopy.running && imagePath(kLastGoodName, true, part, sizeof part) &&
        strcmp(part, path) == 0) {
      return FileResult::Protected;
    }
    if (!LittleFS.exists(path)) return FileResult::NotFound;
    fs::File f = LittleFS.open(path, FILE_READ);
    if (!f) return FileResult::Io;
    const bool dir = f.isDirectory();
    size = static_cast<uint32_t>(f.size());
    f.close();
    if (dir) return FileResult::BadPath;
    if (!LittleFS.remove(path)) return FileResult::Io;
  }
  logger::log(vdm::EventCode::FilesRemoved, vdm::kNoValve, 1, static_cast<int32_t>(kibOf(size)),
              path);
  return FileResult::Ok;
}

uint32_t removeLegacyImages(uint32_t& kib) {
  kib = 0;
  if (!gFsReady) return 0;
  constexpr size_t kBatch = 8;  // collected first: no removal while the root is listed
  uint32_t files = 0;
  uint32_t bytes = 0;
  for (bool more = true; more;) {
    char paths[kBatch][vdm::kFsPathMax + 1];
    uint32_t sizes[kBatch];
    size_t n = 0;
    fs::File root = LittleFS.open("/");
    for (fs::File f = root.openNextFile(); f && n < kBatch; f = root.openNextFile()) {
      const char* p = f.path();
      if (f.isDirectory() || !vdm::isLegacyImageFile(p, strlen(p))) continue;
      vdm::copyString(paths[n], sizeof paths[n], p);  // root-level: always fits
      sizes[n++] = static_cast<uint32_t>(f.size());
    }
    root.close();
    more = n == kBatch;
    for (size_t i = 0; i < n; ++i) {
      if (LittleFS.remove(paths[i])) {
        ++files;
        bytes += sizes[i];
      } else {
        more = false;  // it would be found again
      }
    }
  }
  kib = kibOf(bytes);
  return files;
}

uint32_t fsTotal() { return gFsReady ? static_cast<uint32_t>(LittleFS.totalBytes()) : 0; }

uint32_t fsUsed() { return gFsReady ? static_cast<uint32_t>(LittleFS.usedBytes()) : 0; }

// ---------------------------------------------------------------- images

bool normalizeImageName(const char* in, size_t len, char* out, size_t cap) {
  return vdm::normalizeImageName(in, len, out, cap);
}

bool imagePath(const char* name, bool part, char* out, size_t cap) {
  return vdm::imagePath(name, part, out, cap);
}

size_t listImages(ImageEntry* out, size_t maxOut) {
  FsLock lock;
  size_t n = 0;
  for (const ImageEntry& e : gImages) {
    if (e.name[0] != '\0' && n < maxOut) out[n++] = e;
  }
  return n;
}

bool findImage(const char* name, ImageEntry& out) {
  FsLock lock;
  const int i = findLocked(name);
  if (i < 0) return false;
  out = gImages[i];
  return true;
}

const char* imageResultName(ImageResult r) {
  switch (r) {
    case ImageResult::Ok: return "ok";
    case ImageResult::BadName: return "bad_name";
    case ImageResult::TooLarge: return "too_large";
    case ImageResult::NoSpace: return "no_space";
    case ImageResult::TooMany: return "too_many_images";
    case ImageResult::Busy: return "busy";
    case ImageResult::Io: return "io_error";
    case ImageResult::NotFound: return "not_found";
    case ImageResult::Empty: return "empty";
  }
  return "unknown";
}

ImageResult imageUploadBegin(const char* name, size_t announcedBytes) {
  if (!gFsReady) return ImageResult::Io;
  char clean[kImageNameMax + 1];
  if (name == nullptr || !normalizeImageName(name, strlen(name), clean, sizeof clean) ||
      strcmp(clean, kLastGoodName) == 0) {
    return ImageResult::BadName;
  }
  char path[48];
  if (!imagePath(clean, true, path, sizeof path)) return ImageResult::BadName;
  FsLock lock;
  if (gUpload.active) return ImageResult::Busy;
  // At most 3 uploaded images besides the one being replaced; with
  // last_good that never exceeds kImageSlots index entries.
  if (uploadedCountLocked(clean) >= kMaxUploadedImages) return ImageResult::TooMany;
  if (fsFree() < announcedBytes + kFsReserve) return ImageResult::NoSpace;
  gUpload.file = LittleFS.open(path, FILE_WRITE);
  if (!gUpload.file) return ImageResult::Io;
  vdm::copyString(gUpload.name, sizeof gUpload.name, clean);
  gUpload.written = 0;
  gUpload.crc = 0;
  gUpload.active = true;
  return ImageResult::Ok;
}

ImageResult imageUploadWrite(const uint8_t* data, size_t len) {
  FsLock lock;
  if (!gUpload.active) return ImageResult::Io;
  if (len > kMaxImageSize - gUpload.written) {
    closeUploadLocked(true);
    return ImageResult::TooLarge;
  }
  if (len > 0 && gUpload.file.write(data, len) != len) {
    closeUploadLocked(true);
    return ImageResult::Io;
  }
  gUpload.crc = vdm::crc32(data, len, gUpload.crc);
  gUpload.written += static_cast<uint32_t>(len);
  return ImageResult::Ok;
}

ImageResult imageUploadEnd(ImageEntry& info) {
  FsLock lock;
  if (!gUpload.active) return ImageResult::Io;
  if (gUpload.written == 0) {
    closeUploadLocked(true);
    return ImageResult::Empty;
  }
  gUpload.file.close();
  char part[48];
  char final[48];
  imagePath(gUpload.name, true, part, sizeof part);
  imagePath(gUpload.name, false, final, sizeof final);
  LittleFS.remove(final);
  if (!LittleFS.rename(part, final)) {
    const int i = findLocked(gUpload.name);
    if (i >= 0) gImages[i] = ImageEntry{};  // the old file is gone as well
    closeUploadLocked(true);
    return ImageResult::Io;
  }
  if (!putLocked(gUpload.name, gUpload.written)) {
    // Cannot happen (slots were checked at begin); never keep an unindexed file.
    LittleFS.remove(final);
    gUpload.active = false;
    return ImageResult::TooMany;
  }
  info = ImageEntry{};
  vdm::copyString(info.name, sizeof info.name, gUpload.name);
  info.size = gUpload.written;
  info.crc = gUpload.crc;
  gUpload.active = false;
  return ImageResult::Ok;
}

void imageUploadAbort() {
  FsLock lock;
  if (gUpload.active) closeUploadLocked(true);
}

bool imageUploadActive() {
  FsLock lock;
  return gUpload.active;
}

ImageResult deleteImage(const char* name) {
  char path[48];
  if (name == nullptr || !imagePath(name, false, path, sizeof path)) return ImageResult::BadName;
  FsLock lock;
  const int i = findLocked(name);
  if (i < 0) return ImageResult::NotFound;
  if (gCopy.running && strcmp(gCopy.name, name) == 0) return ImageResult::Busy;
  if (!LittleFS.remove(path) && LittleFS.exists(path)) return ImageResult::Io;
  gImages[i] = ImageEntry{};
  return ImageResult::Ok;
}

void requestLastGoodCopy(const char* name) {
  FsLock lock;
  vdm::copyString(gCopy.name, sizeof gCopy.name, name);
  gCopy.pending = true;
}

void service() {
  if (!gFsReady) return;
  // Not while a network trial runs: a revert must not find the trial
  // settings in the backup.
  if (gBackupPending && !net::trialInfo().active) writeBackup();
  if (gCopy.running || gCopy.pending) {
    serviceCopy();
    return;
  }
  serviceScan();
}

// ---------------------------------------------------------------- FileImage

bool FileImage::open(const char* name) {
  close();
  char path[48];
  if (name == nullptr || !imagePath(name, false, path, sizeof path)) return false;
  file_ = LittleFS.open(path, FILE_READ);
  if (!file_) return false;
  size_ = static_cast<uint32_t>(file_.size());
  pos_ = 0;
  return true;
}

void FileImage::close() {
  if (file_) file_.close();
  size_ = 0;
  pos_ = 0;
}

bool FileImage::read(uint32_t offset, uint8_t* out, size_t len) {
  if (!file_ || offset > size_ || len > size_ - offset) return false;
  if (offset != pos_) {
    if (!file_.seek(offset)) return false;
    pos_ = offset;
  }
  if (file_.read(out, len) != len) {
    pos_ = UINT32_MAX;  // position unknown: seek next time
    return false;
  }
  pos_ += static_cast<uint32_t>(len);
  return true;
}

}  // namespace storage
