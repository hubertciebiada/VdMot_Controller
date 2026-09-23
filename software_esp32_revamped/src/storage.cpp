#include "storage.h"

#include <Arduino.h>
#include <LittleFS.h>
#include <Preferences.h>
#include <freertos/FreeRTOS.h>
#include <freertos/semphr.h>
#include <nvs.h>
#include <nvs_flash.h>
#include <string.h>

namespace storage {

namespace {

bool gFsReady = false;
Preferences gPrefs;
bool gPrefsOpen = false;

StaticSemaphore_t gCfgMutexStorage;
SemaphoreHandle_t gCfgMutex = nullptr;
vdm::Config gActive;
uint32_t gRevision = 0;
uint8_t gBlob[vdm::kConfigBlobMax];  // encode/decode scratch, guarded by gCfgMutex

void ensureMutex() {
  if (gCfgMutex == nullptr) gCfgMutex = xSemaphoreCreateMutexStatic(&gCfgMutexStorage);
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
    nvs_handle_t h;
    if (cap == 0 || nvs_open(ns, NVS_READONLY, &h) != ESP_OK) return false;
    size_t len = 0;
    bool ok = nvs_get_str(h, key, nullptr, &len) == ESP_OK && len > 0;
    if (ok) {
      truncated = len > cap;
      if (truncated) {
        out[0] = '\0';
      } else {
        ok = nvs_get_str(h, key, out, &len) == ESP_OK;
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
      } else {
        memset(out, 0, cap);
      }
    }
    nvs_close(h);
    return ok;
  }
};

// Serialises every Preferences access (it is not thread-safe) and the
// active config.
class Lock {
 public:
  Lock() {
    ensureMutex();
    xSemaphoreTake(gCfgMutex, portMAX_DELAY);
  }
  ~Lock() { xSemaphoreGive(gCfgMutex); }
  Lock(const Lock&) = delete;
  Lock& operator=(const Lock&) = delete;
};

bool saveBlobLocked(const vdm::Config& c) {
  const size_t n = vdm::encodeConfig(c, gBlob, sizeof gBlob);
  return n > 0 && openPrefs() && gPrefs.putBytes(kKeyConfig, gBlob, n) == n;
}

}  // namespace

bool beginFs(bool& formatted) {
  formatted = false;
  gFsReady = LittleFS.begin(false);
  if (!gFsReady) {
    formatted = LittleFS.format();
    gFsReady = formatted && LittleFS.begin(false);
  }
  if (gFsReady) {
    if (!LittleFS.exists("/stm")) LittleFS.mkdir("/stm");
    if (!LittleFS.exists("/log")) LittleFS.mkdir("/log");
  }
  return gFsReady;
}

bool fsReady() { return gFsReady; }

LoadSource loadConfig(vdm::Config& out, vdm::ImportReport& report) {
  Lock lock;
  vdm::setDefaults(out);
  if (!openPrefs()) return LoadSource::DefaultsAfterError;

  const size_t len = gPrefs.getBytesLength(kKeyConfig);
  LoadSource src = LoadSource::Defaults;
  if (len > 0 && len <= sizeof gBlob && gPrefs.getBytes(kKeyConfig, gBlob, len) == len) {
    src = vdm::decodeConfig(gBlob, len, out) == vdm::DecodeResult::Ok
              ? LoadSource::Stored
              : LoadSource::DefaultsAfterError;
    if (src != LoadSource::Stored) vdm::setDefaults(out);
  } else if (len == 0 && gPrefs.getUChar(kKeyImported, 0) == 0) {
    NvsLegacyReader reader;
    report = vdm::importLegacyConfig(reader, out);
    if (saveBlobLocked(out)) gPrefs.putUChar(kKeyImported, 1);
    src = report.anyLegacy ? LoadSource::Imported : LoadSource::Defaults;
    if (report.lastCalibEpoch > 0) gPrefs.putLong64(kKeyLastCalib, report.lastCalibEpoch);
  } else if (len > 0) {
    src = LoadSource::DefaultsAfterError;
  }
  return src;
}

void setActiveConfig(const vdm::Config& c) {
  Lock lock;
  gActive = c;
  ++gRevision;
}

void getConfig(vdm::Config& out) {
  Lock lock;
  out = gActive;
}

uint32_t configRevision() { return gRevision; }

bool applyConfig(const vdm::Config& c, char* path, size_t pathCap) {
  if (!vdm::validateConfig(c, path, pathCap)) return false;
  Lock lock;
  const bool ok = saveBlobLocked(c);
  if (ok) {
    gActive = c;
    ++gRevision;
  } else if (path && pathCap) {
    vdm::copyString(path, pathCap, "nvs");
  }
  return ok;
}

bool factoryReset() {
  Lock lock;
  if (!openPrefs()) return false;
  const bool ok = gPrefs.clear();
  gPrefs.putUChar(kKeyImported, 1);
  return ok;
}

uint32_t incrementBootCount() {
  Lock lock;
  if (!openPrefs()) return 0;
  const uint32_t n = gPrefs.getULong(kKeyBootCount, 0) + 1;
  gPrefs.putULong(kKeyBootCount, n);
  return n;
}

uint32_t loadCalibSlot() {
  Lock lock;
  return openPrefs() ? gPrefs.getULong(kKeyCalibSlot, 0) : 0;
}

void saveCalibSlot(uint32_t slot) {
  Lock lock;
  if (openPrefs()) gPrefs.putULong(kKeyCalibSlot, slot);
}

int64_t loadLastCalib() {
  Lock lock;
  return openPrefs() ? gPrefs.getLong64(kKeyLastCalib, 0) : 0;
}

void saveLastCalib(int64_t epoch) {
  Lock lock;
  if (openPrefs() && epoch > 0) gPrefs.putLong64(kKeyLastCalib, epoch);
}

bool haCleanupDone() {
  Lock lock;
  return openPrefs() && gPrefs.getUChar(kKeyHaCleanup, 0) == 1;
}

void setHaCleanupDone() {
  Lock lock;
  if (openPrefs()) gPrefs.putUChar(kKeyHaCleanup, 1);
}

}  // namespace storage
