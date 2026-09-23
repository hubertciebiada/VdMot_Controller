// Persistent storage: NVS namespace "vdmrev" (new config + small runtime
// state), read-only access to the legacy namespaces for the one-shot import,
// and the LittleFS mount (partition "spiffs", shared with the legacy build).
#pragma once

#include <stddef.h>
#include <stdint.h>

#include <vdm/config.h>
#include <vdm/legacy_import.h>

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
// caller). Creates /stm and /log. Returns false when even the format failed
// (the firmware then runs without files: no STM images, no log files).
bool beginFs(bool& formatted);
bool fsReady();

// Opens NVS. Loads the config: valid blob -> use it; missing blob and
// !imported -> legacy import (vdm::importLegacyConfig) then save; unreadable
// blob -> defaults (never overwrites the bad blob automatically; the next
// explicit save replaces it). `report` is filled when an import ran.
enum class LoadSource : uint8_t { Stored, Imported, Defaults, DefaultsAfterError };
LoadSource loadConfig(vdm::Config& out, vdm::ImportReport& report);

// Thread-safe config holder (all tasks). Copies in/out under a mutex.
void setActiveConfig(const vdm::Config& c);
void getConfig(vdm::Config& out);   // ~3.5 KB: callers use static storage
uint32_t configRevision();          // +1 on every successful apply
// Validates, persists (NVS blob) and publishes a new config. On failure
// nothing changes and `path` names the offending key.
bool applyConfig(const vdm::Config& c, char* path, size_t pathCap);
// Factory reset: erases "vdmrev" (legacy namespaces are left untouched but
// "imported" is set so they are not imported again).
bool factoryReset();

// Small persistent runtime values (written rarely; NVS wear is not a concern
// at these rates).
uint32_t incrementBootCount();
uint32_t loadCalibSlot();
void saveCalibSlot(uint32_t slot);
int64_t loadLastCalib();
void saveLastCalib(int64_t epoch);
bool haCleanupDone();
void setHaCleanupDone();

}  // namespace storage
