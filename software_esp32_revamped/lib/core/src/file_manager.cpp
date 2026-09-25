#include "vdm/file_manager.h"

#include <string.h>

#include "vdm/common.h"
#include "vdm/json_writer.h"

namespace vdm {

namespace {

bool startsWith(const char* p, size_t len, const char* prefix) {
  const size_t n = strlen(prefix);
  return len >= n && memcmp(p, prefix, n) == 0;
}

bool endsWith(const char* p, size_t len, const char* suffix) {
  const size_t n = strlen(suffix);
  return len >= n && memcmp(p + len - n, suffix, n) == 0;
}

char lower(char c) { return c >= 'A' && c <= 'Z' ? static_cast<char>(c - 'A' + 'a') : c; }

// A file directly below `dir` ("/stm/"), no deeper level.
bool directlyIn(const char* p, size_t len, const char* dir) {
  const size_t n = strlen(dir);
  return startsWith(p, len, dir) && memchr(p + n, '/', len - n) == nullptr;
}

}  // namespace

bool fsPathValid(const char* p, size_t len) {
  if (p == nullptr || len < 2 || len > kFsPathMax || p[0] != '/' || !isPrintableText(p, len)) {
    return false;
  }
  for (size_t pos = 1; pos <= len;) {
    const char* seg = p + pos;
    const void* slash = memchr(seg, '/', len - pos);
    const size_t n = slash ? static_cast<size_t>(static_cast<const char*>(slash) - seg) : len - pos;
    if (n == 0) return false;  // "//" or a trailing '/'
    if (seg[0] == '.' && (n == 1 || (n == 2 && seg[1] == '.'))) return false;
    pos += n + 1;
  }
  return true;
}

FileKind classifyFsPath(const char* p, size_t len) {
  if (directlyIn(p, len, "/stm/")) {
    if (endsWith(p, len, ".part")) return FileKind::UploadPart;
    if (endsWith(p, len, ".bin")) return FileKind::StmImage;
  }
  if (startsWith(p, len, "/log/")) return FileKind::Log;
  if (startsWith(p, len, "/sys/")) return FileKind::Internal;
  if (len == strlen("/HADiscovery.cfg") && startsWith(p, len, "/HADiscovery.cfg")) {
    return FileKind::LegacyHaList;
  }
  if (isLegacyImageFile(p, len)) return FileKind::LegacyImage;
  return FileKind::Other;
}

bool fileDeletable(FileKind k) {
  return k == FileKind::UploadPart || k == FileKind::LegacyImage || k == FileKind::Other;
}

const char* fileKindName(FileKind k) {
  switch (k) {
    case FileKind::StmImage: return "stm_image";
    case FileKind::UploadPart: return "upload_part";
    case FileKind::Log: return "log";
    case FileKind::Internal: return "internal";
    case FileKind::LegacyHaList: return "legacy_ha_list";
    case FileKind::LegacyImage: return "legacy_image";
    case FileKind::Other: break;
  }
  return "other";
}

const char* fileProtectReason(FileKind k) {
  switch (k) {
    case FileKind::StmImage: return "use DELETE /api/stm/images/<name>";
    case FileKind::Log: return "event log";
    case FileKind::Internal: return "internal file";
    case FileKind::LegacyHaList: return "kept for a rollback to the legacy firmware";
    case FileKind::UploadPart:
    case FileKind::LegacyImage:
    case FileKind::Other: break;
  }
  return "";
}

bool isLegacyImageFile(const char* p, size_t len) {
  static const char kSuffix[] = ".bin";
  const size_t n = sizeof kSuffix - 1;
  if (p == nullptr || len < n + 2 || !directlyIn(p, len, "/")) return false;
  for (size_t i = 0; i < n; ++i) {
    if (lower(p[len - n + i]) != kSuffix[i]) return false;
  }
  return true;
}

bool writeFilesJson(JsonWriter& jw, const FileEntry* files, size_t count, uint32_t total,
                    uint32_t used, bool truncated) {
  jw.beginObject();
  jw.kv("total", total);
  jw.kv("used", used);
  jw.kv("truncated", truncated);
  jw.key("files");
  jw.beginArray();
  for (size_t i = 0; i < count; ++i) {
    const FileEntry& f = files[i];
    const size_t len = boundedLength(f.path, kFsPathMax);
    const FileKind k = classifyFsPath(f.path, len);
    jw.beginObject();
    jw.kv("path", f.path);
    jw.kv("size", f.size);
    jw.kv("kind", fileKindName(k));
    jw.kv("deletable", fileDeletable(k));
    jw.endObject();
  }
  jw.endArray();
  jw.endObject();
  return jw.ok();
}

}  // namespace vdm
