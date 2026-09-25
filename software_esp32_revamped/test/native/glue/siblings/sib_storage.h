// Scripted results and recorded calls of the sibling fake of src/storage.cpp
// (siblings/fake_storage.cpp); both files go with src/storage.cpp. See siblings.h.
#pragma once

#include <stddef.h>
#include <stdint.h>

#include <string>
#include <vector>

#include <vdm/config.h>
#include <vdm/file_manager.h>
#include <vdm/legacy_import.h>

#include "storage.h"

namespace sib {

struct ImageUploadBegin {
  std::string name;
  size_t announcedBytes;
};
struct Storage {
  // scripted
  bool beginFsResult = true;
  bool formatted = false;
  bool fsReady = true;
  storage::LoadSource loadSource = storage::LoadSource::Stored;
  storage::LoadDetails loadDetails;
  vdm::Config loadedConfig;      // loadConfig() output (defaults unless a test sets it)
  vdm::ImportReport importReport;
  bool configSaved = false;
  vdm::Config active;            // getConfig()
  uint32_t revision = 0;
  bool applyResult = true;
  std::string applyPath;         // written into `path` when applyResult is false
  bool factoryResetResult = true;
  uint32_t bootCount = 0;        // incrementBootCount() returns ++bootCount
  uint32_t calibSlot = 0;
  int64_t lastCalib = 0;
  bool haCleanupDone = false;
  std::vector<uint8_t> targets;
  bool saveTargetsResult = true;
  std::vector<uint8_t> netTrial;
  bool saveNetTrialResult = true;
  bool factoryLatched = false;
  bool otaStmRequired = false;
  uint8_t haLayout = 0;
  bool writeImportReportResult = true;
  bool hasImportReport = false;
  std::vector<vdm::FileEntry> files;  // listFiles(), cut to `max` (then truncated)
  bool filesTruncated = false;        // listFiles() truncated even when everything fits
  storage::FileResult deleteFileResult = storage::FileResult::NotFound;
  uint32_t legacyImagesRemoved = 0;
  uint32_t legacyImagesKib = 0;
  uint32_t fsTotal = 0x170000;
  uint32_t fsUsed = 0x10000;
  std::vector<storage::ImageEntry> images;  // listImages(), findImage()
  bool imageUploadActive = false;
  storage::ImageResult uploadBeginResult = storage::ImageResult::Ok;
  storage::ImageResult uploadWriteResult = storage::ImageResult::Ok;
  storage::ImageResult uploadEndResult = storage::ImageResult::Ok;
  storage::ImageEntry uploadEndInfo;
  storage::ImageResult deleteImageResult = storage::ImageResult::Ok;
  // recorded
  int beginFsCalls = 0;
  int loadConfigCalls = 0;
  std::vector<vdm::Config> activeSets;   // setActiveConfig()
  std::vector<vdm::Config> applied;      // applyConfig() attempts
  int factoryResets = 0;
  std::vector<uint32_t> savedCalibSlots;
  std::vector<int64_t> savedLastCalibs;
  int haCleanupMarks = 0;
  int targetSaves = 0;
  int netTrialSaves = 0;
  int netTrialClears = 0;
  std::vector<bool> factoryLatchSets;
  std::vector<bool> otaStmSets;
  std::vector<uint8_t> haLayoutSets;
  int importReportWrites = 0;
  int importReportDismissals = 0;
  std::vector<std::string> deletedFiles;
  std::vector<ImageUploadBegin> uploadBegins;
  std::string uploadData;
  int uploadEnds = 0;
  int uploadAborts = 0;
  std::vector<std::string> deletedImages;
  std::vector<std::string> lastGoodCopies;
  int services = 0;
};
Storage& storage();

}  // namespace sib
