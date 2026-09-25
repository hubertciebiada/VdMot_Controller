// Sibling fake of src/storage.cpp (see siblings.h). The name and path helpers follow the same
// rules as storage.cpp; FileImage reads /stm/<name>.bin from the fake LittleFS, so a flash run
// of the STM link reads a file a test put there.
#include <stdio.h>
#include <string.h>

#include <LittleFS.h>

#include "fakes/fakes.h"
#include "siblings.h"
#include "storage.h"

namespace storage {

bool beginFs(bool& formatted) {
  sib::Storage& s = sib::storage();
  ++s.beginFsCalls;
  fakes::note("storage.beginFs");
  formatted = s.formatted;
  return s.beginFsResult;
}

bool fsReady() { return sib::storage().fsReady; }

LoadSource loadConfig(vdm::Config& out, vdm::ImportReport& report, LoadDetails& details) {
  sib::Storage& s = sib::storage();
  ++s.loadConfigCalls;
  fakes::note("storage.loadConfig");
  out = s.loadedConfig;
  report = s.importReport;
  details = s.loadDetails;
  return s.loadSource;
}

LoadSource bootLoadSource() { return sib::storage().loadSource; }

const LoadDetails& bootLoadDetails() { return sib::storage().loadDetails; }

bool configSavedSinceBoot() { return sib::storage().configSaved; }

void setActiveConfig(const vdm::Config& c) {
  sib::Storage& s = sib::storage();
  s.activeSets.push_back(c);
  s.active = c;
  ++s.revision;
  fakes::note("storage.setActiveConfig");
}

void getConfig(vdm::Config& out) { out = sib::storage().active; }

uint32_t configRevision() { return sib::storage().revision; }

bool applyConfig(const vdm::Config& c, char* path, size_t pathCap) {
  sib::Storage& s = sib::storage();
  s.applied.push_back(c);
  fakes::note("storage.applyConfig");
  if (!s.applyResult) {
    if (path != nullptr && pathCap > 0) snprintf(path, pathCap, "%s", s.applyPath.c_str());
    return false;
  }
  s.active = c;
  ++s.revision;
  s.configSaved = true;
  return true;
}

bool factoryReset() {
  ++sib::storage().factoryResets;
  fakes::note("storage.factoryReset");
  return sib::storage().factoryResetResult;
}

uint32_t incrementBootCount() { return ++sib::storage().bootCount; }

uint32_t bootCount() { return sib::storage().bootCount; }

uint32_t loadCalibSlot() { return sib::storage().calibSlot; }

void saveCalibSlot(uint32_t slot) {
  sib::storage().savedCalibSlots.push_back(slot);
  sib::storage().calibSlot = slot;
}

int64_t loadLastCalib() { return sib::storage().lastCalib; }

void saveLastCalib(int64_t epoch) {
  sib::storage().savedLastCalibs.push_back(epoch);
  sib::storage().lastCalib = epoch;
}

bool haCleanupDone() { return sib::storage().haCleanupDone; }

void setHaCleanupDone() {
  ++sib::storage().haCleanupMarks;
  sib::storage().haCleanupDone = true;
}

bool saveTargets(const uint8_t* data, size_t len) {
  sib::Storage& s = sib::storage();
  ++s.targetSaves;
  if (!s.saveTargetsResult || data == nullptr || len == 0) return false;
  s.targets.assign(data, data + len);
  return true;
}

size_t loadTargets(uint8_t* out, size_t cap) {
  const std::vector<uint8_t>& t = sib::storage().targets;
  if (out == nullptr || t.empty() || t.size() > cap) return 0;
  memcpy(out, t.data(), t.size());
  return t.size();
}

size_t loadNetTrialBlob(uint8_t* out, size_t cap) {
  const std::vector<uint8_t>& t = sib::storage().netTrial;
  if (out == nullptr || t.empty() || t.size() > cap) return 0;
  memcpy(out, t.data(), t.size());
  return t.size();
}

bool saveNetTrialBlob(const uint8_t* data, size_t len) {
  sib::Storage& s = sib::storage();
  ++s.netTrialSaves;
  if (!s.saveNetTrialResult || data == nullptr || len == 0) return false;
  s.netTrial.assign(data, data + len);
  return true;
}

void clearNetTrial() {
  ++sib::storage().netTrialClears;
  sib::storage().netTrial.clear();
}

bool factoryLatched() { return sib::storage().factoryLatched; }

void setFactoryLatched(bool on) {
  sib::storage().factoryLatchSets.push_back(on);
  sib::storage().factoryLatched = on;
}

bool otaStmRequired() { return sib::storage().otaStmRequired; }

void setOtaStmRequired(bool on) {
  sib::storage().otaStmSets.push_back(on);
  sib::storage().otaStmRequired = on;
}

void clearOtaStmRequired() { setOtaStmRequired(false); }

uint8_t haLayout() { return sib::storage().haLayout; }

void setHaLayout(uint8_t layout) {
  sib::storage().haLayoutSets.push_back(layout);
  sib::storage().haLayout = layout;
}

bool writeImportReport(const vdm::ImportReport&) {
  ++sib::storage().importReportWrites;
  return sib::storage().writeImportReportResult;
}

bool hasImportReport() { return sib::storage().hasImportReport; }

bool dismissImportReport() {
  sib::Storage& s = sib::storage();
  ++s.importReportDismissals;
  const bool had = s.hasImportReport;
  s.hasImportReport = false;
  return had;
}

size_t listFiles(vdm::FileEntry*, size_t, bool& truncated) {
  truncated = false;
  return 0;
}

FileResult deleteFile(const char* path) {
  sib::storage().deletedFiles.push_back(path != nullptr ? path : "");
  return sib::storage().deleteFileResult;
}

uint32_t removeLegacyImages(uint32_t& kib) {
  kib = sib::storage().legacyImagesKib;
  return sib::storage().legacyImagesRemoved;
}

uint32_t fsTotal() { return sib::storage().fsTotal; }

uint32_t fsUsed() { return sib::storage().fsUsed; }

bool normalizeImageName(const char* in, size_t len, char* out, size_t cap) {
  if (cap > 0 && out != nullptr) out[0] = '\0';
  if (in == nullptr || out == nullptr) return false;
  if (len >= 4 && memcmp(in + len - 4, ".bin", 4) == 0) len -= 4;
  if (len == 0 || len > kImageNameMax || len >= cap || in[0] == '.') return false;
  for (size_t i = 0; i < len; ++i) {
    const char c = in[i];
    const bool ok = (c >= 'A' && c <= 'Z') || (c >= 'a' && c <= 'z') || (c >= '0' && c <= '9') ||
                    c == '.' || c == '_' || c == '-';
    if (!ok) return false;
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
  size_t n = 0;
  for (const ImageEntry& e : sib::storage().images) {
    if (n < maxOut) out[n++] = e;
  }
  return n;
}

bool findImage(const char* name, ImageEntry& out) {
  for (const ImageEntry& e : sib::storage().images) {
    if (name != nullptr && strcmp(e.name, name) == 0) {
      out = e;
      return true;
    }
  }
  return false;
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
  sib::Storage& s = sib::storage();
  s.uploadBegins.push_back({name != nullptr ? name : "", announcedBytes});
  s.uploadData.clear();
  if (s.uploadBeginResult == ImageResult::Ok) s.imageUploadActive = true;
  return s.uploadBeginResult;
}

ImageResult imageUploadWrite(const uint8_t* data, size_t len) {
  sib::Storage& s = sib::storage();
  if (s.uploadWriteResult != ImageResult::Ok) {
    s.imageUploadActive = false;  // storage removes the part file
    return s.uploadWriteResult;
  }
  if (len > 0) s.uploadData.append(reinterpret_cast<const char*>(data), len);
  return ImageResult::Ok;
}

ImageResult imageUploadEnd(ImageEntry& info) {
  sib::Storage& s = sib::storage();
  ++s.uploadEnds;
  s.imageUploadActive = false;
  if (s.uploadEndResult == ImageResult::Ok) info = s.uploadEndInfo;
  return s.uploadEndResult;
}

void imageUploadAbort() {
  ++sib::storage().uploadAborts;
  sib::storage().imageUploadActive = false;
}

bool imageUploadActive() { return sib::storage().imageUploadActive; }

ImageResult deleteImage(const char* name) {
  sib::storage().deletedImages.push_back(name != nullptr ? name : "");
  return sib::storage().deleteImageResult;
}

void requestLastGoodCopy(const char* name) {
  sib::storage().lastGoodCopies.push_back(name != nullptr ? name : "");
  fakes::note(std::string("storage.requestLastGoodCopy ") + (name != nullptr ? name : ""));
}

void service() {
  ++sib::storage().services;
  fakes::note("storage.service");
}

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
    pos_ = UINT32_MAX;
    return false;
  }
  pos_ += static_cast<uint32_t>(len);
  return true;
}

}  // namespace storage
