// LittleFS file management rules (GET/DELETE /api/files): path syntax, the
// kind of a file, whether the dashboard may delete it, and the /api/files
// document. Hardware-free; the walk and the removal are storage glue.
#pragma once

#include <stddef.h>
#include <stdint.h>

namespace vdm {

class JsonWriter;

constexpr size_t kFsPathMax = 95;

enum class FileKind : uint8_t {
  StmImage,      // /stm/<name>.bin: deleted through DELETE /api/stm/images/<name>
  UploadPart,    // /stm/<name>.part: an aborted upload
  Log,           // /log/*: the event log
  Internal,      // /sys/*: config backups, import report, discovery list
  LegacyHaList,  // /HADiscovery.cfg: kept for a rollback to the legacy firmware
  LegacyImage,   // /<name>.bin (any case): an STM image the legacy firmware uploaded
  Other,
};

// '/' first, 2..kFsPathMax bytes, printable text (UTF-8 allowed), segments
// separated by single '/', none of them "." or "..", no trailing '/'.
bool fsPathValid(const char* p, size_t len);
// Kind of a valid path (table above; the first matching row wins).
FileKind classifyFsPath(const char* p, size_t len);
bool fileDeletable(FileKind k);
// "stm_image", "upload_part", "log", "internal", "legacy_ha_list",
// "legacy_image", "other".
const char* fileKindName(FileKind k);
// Detail of the 403 answer for a protected kind; "" when deletable.
const char* fileProtectReason(FileKind k);
// A root-level file whose name ends in ".bin" (case-insensitive), with at
// least one character before the suffix.
bool isLegacyImageFile(const char* p, size_t len);

struct FileEntry {
  char path[kFsPathMax + 1];
  uint32_t size;
};

// {"total":<bytes>,"used":<bytes>,"truncated":false,"files":[{"path":"/stm/x.bin",
//  "size":98304,"kind":"stm_image","deletable":false},...]}. Returns jw.ok().
bool writeFilesJson(JsonWriter& jw, const FileEntry* files, size_t count, uint32_t total,
                    uint32_t used, bool truncated);

}  // namespace vdm
