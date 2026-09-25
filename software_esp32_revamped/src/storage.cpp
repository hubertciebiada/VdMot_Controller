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

#include "boot_alloc.h"
#include "logger.h"

namespace storage {

namespace {

bool gFsReady = false;
Preferences gPrefs;
bool gPrefsOpen = false;

StaticSemaphore_t gCfgMutexStorage;
SemaphoreHandle_t gCfgMutex = nullptr;
vdm::Config& gActive = bootAlloc<vdm::Config>();
volatile uint32_t gRevision = 0;
uint32_t gBootCount = 0;
using Blob = ObjArray<uint8_t, vdm::kConfigBlobMax>;
Blob& gBlob = bootAlloc<Blob>();  // encode/decode scratch, guarded by gCfgMutex

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
    nvs_handle_t h;
    if (out == nullptr || cap == 0 || nvs_open(ns, NVS_READONLY, &h) != ESP_OK) return false;
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

bool saveBlobLocked(const vdm::Config& c) {
  const size_t n = vdm::encodeConfig(c, gBlob.data(), sizeof gBlob.items);
  return n > 0 && openPrefs() && gPrefs.putBytes(kKeyConfig, gBlob.data(), n) == n;
}

// ---------------------------------------------------------------- images

bool validNameChar(char c) {
  return (c >= 'A' && c <= 'Z') || (c >= 'a' && c <= 'z') || (c >= '0' && c <= '9') || c == '.' ||
         c == '_' || c == '-';
}

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
    gCopy.dst = LittleFS.open(part, FILE_WRITE);
    if (!gCopy.dst) {
      gCopy.src.close();
      return;
    }
    gCopy.done = 0;
    gCopy.running = true;
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
    indexImages();
  }
  return gFsReady;
}

bool fsReady() { return gFsReady; }

namespace {

LoadSource loadStored(vdm::Config& out, vdm::ImportReport& report, uint8_t& errorCode) {
  CfgLock lock;
  errorCode = 0;
  vdm::setDefaults(out);
  if (!openPrefs()) {
    errorCode = 100;
    return LoadSource::DefaultsAfterError;
  }

  const size_t len = gPrefs.getBytesLength(kKeyConfig);
  if (len > 0) {
    if (len > sizeof gBlob.items || gPrefs.getBytes(kKeyConfig, gBlob.data(), len) != len) {
      errorCode = 101;
      return LoadSource::DefaultsAfterError;
    }
    const vdm::DecodeResult r = vdm::decodeConfig(gBlob.data(), len, out);
    if (r == vdm::DecodeResult::Ok) return LoadSource::Stored;
    vdm::setDefaults(out);
    errorCode = static_cast<uint8_t>(r);
    return LoadSource::DefaultsAfterError;
  }
  if (gPrefs.getUChar(kKeyImported, 0) == 1) return LoadSource::Defaults;

  NvsLegacyReader reader;
  report = vdm::importLegacyConfig(reader, out);
  if (report.lastCalibEpoch > 0) gPrefs.putLong64(kKeyLastCalib, report.lastCalibEpoch);
  // "imported" only after the blob is safely stored: a failed save retries
  // the (idempotent) import on the next boot.
  if (saveBlobLocked(out)) gPrefs.putUChar(kKeyImported, 1);
  return report.anyLegacy ? LoadSource::Imported : LoadSource::Defaults;
}

}  // namespace

LoadSource loadConfig(vdm::Config& out, vdm::ImportReport& report, uint8_t& errorCode) {
  const LoadSource src = loadStored(out, report, errorCode);
  if (src == LoadSource::Imported) {
    logger::log(vdm::EventCode::ConfigImported, vdm::kNoValve, report.imported, report.rejected,
                report.firstRejected);
  } else if (src == LoadSource::DefaultsAfterError) {
    logger::log(vdm::EventCode::ConfigDefaults, vdm::kNoValve, errorCode);
  }
  return src;
}

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
  const bool ok = saveBlobLocked(c);
  if (ok) {
    gActive = c;
    gRevision = gRevision + 1;
  } else if (path && pathCap) {
    vdm::copyString(path, pathCap, "nvs");
  }
  return ok;
}

bool factoryReset() {
  CfgLock lock;
  if (!openPrefs()) return false;
  const bool ok = gPrefs.clear();
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

// ---------------------------------------------------------------- images

bool normalizeImageName(const char* in, size_t len, char* out, size_t cap) {
  if (cap > 0) out[0] = '\0';
  if (in == nullptr || out == nullptr) return false;
  if (endsWith(in, len, ".bin")) len -= 4;
  if (len == 0 || len > kImageNameMax || len >= cap || in[0] == '.') return false;
  for (size_t i = 0; i < len; ++i) {
    if (!validNameChar(in[i])) return false;
  }
  memcpy(out, in, len);
  out[len] = '\0';
  return true;
}

bool imagePath(const char* name, bool part, char* out, size_t cap) {
  const int n = snprintf(out, cap, "/stm/%s.bin%s", name, part ? ".part" : "");
  return n > 0 && static_cast<size_t>(n) < cap;
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
