// Persistent storage: NVS namespace "vdmrev" (new config + small runtime
// state), read-only access to the legacy namespaces for the one-shot import,
// the LittleFS mount (partition "spiffs", shared with the legacy build) and
// the STM image store below /stm.
#pragma once

#include <FS.h>
#include <stddef.h>
#include <stdint.h>

#include <vdm/config.h>
#include <vdm/legacy_import.h>
#include <vdm/stm_flasher.h>

namespace vdm {
struct FileEntry;  // vdm/file_manager.h
}  // namespace vdm

namespace storage {

// NVS namespace and keys (binding, DESIGN.md "Persistence").
constexpr const char* kNamespace = "vdmrev";
constexpr const char* kKeyConfig = "cfg";        // blob, vdm::encodeConfig()
constexpr const char* kKeyConfigExt = "cfgx";    // blob, vdm::encodeConfigExt()
constexpr const char* kKeyImported = "imported"; // u8 1 = legacy import done (or not needed)
constexpr const char* kKeyBootCount = "boots";   // u32
constexpr const char* kKeyCalibSlot = "calSlot"; // u32 yyyymmdd of the last scheduled calibration
constexpr const char* kKeyLastCalib = "lastCal"; // i64 epoch of the last calibration command
constexpr const char* kKeyHaCleanup = "haDrop";  // u8 1 = legacy DROP entities deleted
constexpr const char* kKeyTargets = "targets";   // blob, the desired targets (stm_service)
constexpr const char* kKeyNetTrial = "netTrial"; // blob, the network trial record (net)
constexpr const char* kKeyFactoryLatch = "frLatch";  // u8 1 = factory reset done, pin still set
constexpr const char* kKeyOtaStm = "otaStm";     // u8 1 = STM link up at the ESP OTA upload
constexpr const char* kKeyHaLayout = "haLayout"; // u8 2 = the 2.1 discovery layout was published

// LittleFS files of the config backup and the legacy import report.
constexpr const char* kBackupBase = "/sys/cfg.bak";
constexpr const char* kBackupExt = "/sys/cfgx.bak";
constexpr const char* kImportReportFile = "/sys/import.json";

// Mounts LittleFS on the "spiffs" partition. Formats only when mounting
// fails (then `formatted` is true and an FsFormatted event is logged by the
// caller). Creates /stm, /log and /sys, removes upload leftovers (*.part) and
// indexes the STM images. Returns false when even the format failed (the
// firmware then runs without files: no STM images, no log files).
bool beginFs(bool& formatted);
bool fsReady();

// Opens NVS. Loads the config (vdm::loadConfigBlobs: cfg + cfgx, repaired):
// usable blobs -> Stored (unknown cfgx records are kept for the next save;
// the backup files are rewritten when they differ); unusable cfg -> the
// backup files (Backup, NVS rewritten from them), else defaults
// (DefaultsAfterError; the bad blob is never overwritten automatically, the
// next explicit save replaces it); missing cfg: imported set -> Defaults,
// else the backup files, else the legacy import (vdm::importLegacyConfig)
// then save, the import report file and the removal of the legacy images.
// `report` is filled when an import ran. `details.errorCode` receives the
// reason for DefaultsAfterError and Backup: the vdm::DecodeResult value,
// 100 = NVS not usable, 101 = blob unreadable, 0 = no cfg in NVS.
// Logs the result (ConfigRepaired, ConfigNewerSchema, ConfigRestored,
// ConfigImported, ImportDropped, FilesRemoved, ConfigDefaults).
enum class LoadSource : uint8_t { Stored, Imported, Defaults, DefaultsAfterError, Backup };
struct LoadDetails {
  uint8_t errorCode = 0;
  vdm::LoadInfo info;
};
LoadSource loadConfig(vdm::Config& out, vdm::ImportReport& report, LoadDetails& details);
// What loadConfig() did at boot (/api/status).
LoadSource bootLoadSource();
const LoadDetails& bootLoadDetails();
// True after the first successful applyConfig() since boot.
bool configSavedSinceBoot();

// Thread-safe config holder (all tasks). Copies in/out under a mutex.
void setActiveConfig(const vdm::Config& c);
void getConfig(vdm::Config& out);   // ~2 KB: callers use static storage
uint32_t configRevision();          // +1 on every successful apply
// Validates, persists (NVS cfgx, then cfg) and publishes a new config. On
// failure nothing changes and `path` names the offending key ("nvs" when a
// write failed). service() then copies the blobs to the backup files, not
// while a network trial runs.
bool applyConfig(const vdm::Config& c, char* path, size_t pathCap);
// Factory reset: erases "vdmrev" except frLatch and removes the backup and
// import report files (legacy namespaces are left untouched but "imported"
// is set so they are not imported again).
bool factoryReset();

// Small persistent runtime values (written rarely; NVS wear is not a concern
// at these rates).
uint32_t incrementBootCount();
uint32_t bootCount();
uint32_t loadCalibSlot();
void saveCalibSlot(uint32_t slot);
int64_t loadLastCalib();
void saveLastCalib(int64_t epoch);
bool haCleanupDone();
void setHaCleanupDone();
// Desired targets as encoded by their owner; 0 = none stored.
bool saveTargets(const uint8_t* data, size_t len);
size_t loadTargets(uint8_t* out, size_t cap);
// Network trial record (raw bytes; the record format belongs to net); 0 =
// none stored.
size_t loadNetTrialBlob(uint8_t* out, size_t cap);
bool saveNetTrialBlob(const uint8_t* data, size_t len);
void clearNetTrial();
// Factory reset done while the pin was still set (kept by factoryReset()).
bool factoryLatched();
void setFactoryLatched(bool on);
// STM link was up when an ESP OTA was uploaded (read and cleared at the next
// boot).
bool otaStmRequired();
void setOtaStmRequired(bool on);
void clearOtaStmRequired();
// Home Assistant discovery layout last published (0 = none).
uint8_t haLayout();
void setHaLayout(uint8_t layout);

// ---------------------------------------------------------------- files

// The legacy import report (kImportReportFile).
bool writeImportReport(const vdm::ImportReport& report);
bool hasImportReport();
bool dismissImportReport();  // false when there was none

enum class FileResult : uint8_t { Ok, BadPath, Protected, NotFound, Io };
// The root and one directory level below it, at most `max` entries.
size_t listFiles(vdm::FileEntry* out, size_t max, bool& truncated);
FileResult deleteFile(const char* path);
// Removes the images the legacy firmware left in the root; returns the
// number of files, `kib` the space freed.
uint32_t removeLegacyImages(uint32_t& kib);
uint32_t fsTotal();
uint32_t fsUsed();

// ---------------------------------------------------------------- STM images

// Images live in /stm/<name>.bin. Names follow vdm::matchApiRoute's {name}
// rule, [A-Za-z0-9._-]{1,31} without a leading '.', and exclude the ".bin"
// suffix. "last_good" holds a copy of the last successfully flashed image
// and cannot be uploaded.
constexpr size_t kImageNameMax = 31;
constexpr size_t kMaxImageSize = 512 * 1024;
constexpr size_t kMaxUploadedImages = 3;  // plus last_good
constexpr size_t kImageSlots = kMaxUploadedImages + 1;
constexpr const char* kLastGoodName = "last_good";
// LittleFS space that must stay free after an upload (log rotation,
// metadata, the last_good copy).
constexpr size_t kFsReserve = 2 * 64 * 1024 + 16 * 1024;

struct ImageEntry {
  char name[kImageNameMax + 1] = {0};
  uint32_t size = 0;
  bool scanned = false;                            // validateImage() ran
  vdm::FlashError check = vdm::FlashError::None;   // chip-independent checks incl. handshake
  uint32_t crc = 0;                                // valid when scanned
  char version[32] = {0};                          // "" = none found
  char hwTag[4] = {0};                             // board of the image ("C2"), "" untagged
};

// Accepts "x" or "x.bin" (`len` bytes) and writes the bare name.
bool normalizeImageName(const char* in, size_t len, char* out, size_t cap);
// "/stm/<name>.bin" (+ ".part"); false when it does not fit.
bool imagePath(const char* name, bool part, char* out, size_t cap);

// Copies of the index (no file I/O, safe from HTTP handlers).
size_t listImages(ImageEntry* out, size_t maxOut);
bool findImage(const char* name, ImageEntry& out);

enum class ImageResult : uint8_t {
  Ok,
  BadName,
  TooLarge,
  NoSpace,
  TooMany,
  Busy,
  Io,
  NotFound,
  Empty
};
const char* imageResultName(ImageResult r);

// Upload into /stm/<name>.bin.part, renamed over /stm/<name>.bin on
// success. One upload at a time; every write is checked; the size limit is
// enforced on the running total.
ImageResult imageUploadBegin(const char* name, size_t announcedBytes);
ImageResult imageUploadWrite(const uint8_t* data, size_t len);
ImageResult imageUploadEnd(ImageEntry& info);
void imageUploadAbort();
bool imageUploadActive();
ImageResult deleteImage(const char* name);

// After a successful flash: copy the image to last_good (done
// incrementally by service()).
void requestLastGoodCopy(const char* name);

// App task: writes the pending config backup (not during a network trial),
// validates one unscanned image per call and advances the last_good copy by
// a bounded number of bytes.
void service();

// LittleFS-backed image for the flasher and the validator.
class FileImage : public vdm::FlashImage {
 public:
  bool open(const char* name);  // bare name
  void close();
  uint32_t size() const override { return size_; }
  bool read(uint32_t offset, uint8_t* out, size_t len) override;

 private:
  fs::File file_;
  uint32_t size_ = 0;
  uint32_t pos_ = 0;
};

}  // namespace storage
