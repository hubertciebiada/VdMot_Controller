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

namespace storage {

// NVS namespace and keys (binding, DESIGN.md "Persistence").
constexpr const char* kNamespace = "vdmrev";
constexpr const char* kKeyConfig = "cfg";        // blob, vdm::encodeConfig()
constexpr const char* kKeyImported = "imported"; // u8 1 = legacy import done (or not needed)
constexpr const char* kKeyBootCount = "boots";   // u32
constexpr const char* kKeyCalibSlot = "calSlot"; // u32 yyyymmdd of the last scheduled calibration
constexpr const char* kKeyLastCalib = "lastCal"; // i64 epoch of the last calibration command
constexpr const char* kKeyHaCleanup = "haDrop";  // u8 1 = legacy DROP entities deleted

// Mounts LittleFS on the "spiffs" partition. Formats only when mounting
// fails (then `formatted` is true and an FsFormatted event is logged by the
// caller). Creates /stm and /log, removes upload leftovers (*.part) and
// indexes the STM images. Returns false when even the format failed (the
// firmware then runs without files: no STM images, no log files).
bool beginFs(bool& formatted);
bool fsReady();

// Opens NVS. Loads the config: valid blob -> use it; missing blob and
// !imported -> legacy import (vdm::importLegacyConfig) then save; unreadable
// blob -> defaults (never overwrites the bad blob automatically; the next
// explicit save replaces it). `report` is filled when an import ran.
// For DefaultsAfterError `errorCode` receives the reason: the
// vdm::DecodeResult value, 100 = NVS not usable, 101 = blob unreadable.
// Logs the result (ConfigImported, ConfigDefaults).
enum class LoadSource : uint8_t { Stored, Imported, Defaults, DefaultsAfterError };
LoadSource loadConfig(vdm::Config& out, vdm::ImportReport& report, uint8_t& errorCode);

// Thread-safe config holder (all tasks). Copies in/out under a mutex.
void setActiveConfig(const vdm::Config& c);
void getConfig(vdm::Config& out);   // ~2 KB: callers use static storage
uint32_t configRevision();          // +1 on every successful apply
// Validates, persists (NVS blob) and publishes a new config. On failure
// nothing changes and `path` names the offending key ("nvs" when the write
// failed).
bool applyConfig(const vdm::Config& c, char* path, size_t pathCap);
// Factory reset: erases "vdmrev" (legacy namespaces are left untouched but
// "imported" is set so they are not imported again).
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

// App task: validates one unscanned image per call and advances the
// last_good copy by a bounded number of bytes.
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
