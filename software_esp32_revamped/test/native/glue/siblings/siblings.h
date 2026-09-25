// Sibling fakes of the ESP glue modules (namespace sib): every function of src/<module>.h with
// scripted results and recorded calls. The executable of glue file X links all sibling fakes
// except siblings/fake_X.cpp, so X runs against fakes of the modules it calls. Calls that
// matter for ordering are journaled ("stm_link.begin", "logger.service 0", ...).
#pragma once

#include <stddef.h>
#include <stdint.h>

#include <deque>
#include <functional>
#include <string>
#include <vector>

#include <vdm/config.h>
#include <vdm/event_log.h>
#include <vdm/failsafe.h>
#include <vdm/json_api.h>
#include <vdm/legacy_import.h>
#include <vdm/stm_types.h>

#include "app.h"
#include "logger.h"
#include "mqtt_client.h"
#include "net.h"
#include "ota.h"
#include "stm_link.h"
#include "stm_service.h"
#include "storage.h"
#include "web_server.h"

namespace sib {

// Every sibling back to its defaults (glue::begin()).
void reset();

// ---------------------------------------------------------------- app (src/app.h)

struct App {
  // scripted
  int64_t uptimeS = -1;               // -1: esp_timer_get_time() / 1 s
  bool submitResult = true;           // false: the queue is full
  std::deque<app::Command> toReceive;  // receive() hands these out in order
  vdm::StmSnapshot snapshot;          // readStmSnapshot()
  uint32_t snapshotRevision = 0;
  vdm::LinkState link = vdm::LinkState::Unknown;
  bool flashActive = false;
  uint8_t proto = 0;
  vdm::StmSupport support = vdm::StmSupport::Unknown;
  vdm::StmSaveState saveState = vdm::StmSaveState::Idle;
  app::CalibInfo calib;
  vdm::HealthSnapshot health;
  // recorded
  std::vector<app::Command> submitted;
  vdm::StmSnapshot published;         // the last publishStmSnapshot()
  int publishes = 0;
  int setupCalls = 0;
  int saveRequests = 0;
  std::vector<vdm::StmSaveState> saveStates;
  int flashMarks = 0;
  std::vector<app::CalibInfo> calibInfos;
  int healthReads = 0;
};
App& app();

// ---------------------------------------------------------------- logger (src/logger.h)

struct Configure {
  uint8_t syslogLevel;
  uint32_t syslogServer;
  uint16_t syslogPort;
  bool persist;
  std::string hostname;
};
struct Logger {
  // recorded, and what read()/readSince()/lastSeq() answer (a real vdm::EventLog of 512)
  std::vector<vdm::Event> events;  // every log()/logSev(), seq assigned from 1
  std::vector<Configure> configures;
  int begins = 0;
  std::vector<bool> services;  // service(netUp)
  int flushes = 0;
  int flushRequests = 0;
  std::vector<std::string> debugLines;
  // scripted
  vdm::LogHealthInfo stats;
  std::vector<vdm::Event> withCode(vdm::EventCode code) const;
  bool has(vdm::EventCode code) const { return !withCode(code).empty(); }
};
Logger& logger();

// ---------------------------------------------------------------- storage (src/storage.h)

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

// ---------------------------------------------------------------- net (src/net.h)

struct NetService {
  uint32_t nowMs;
  bool mqttConnected;
};
struct Net {
  // scripted
  bool up = false;
  net::Info info;
  vdm::NetHealthInfo health;
  bool otaNetOk = false;
  std::string hostname = "VdMot";
  vdm::LocalTime localTime;
  bool timeValid = false;
  uint32_t lastSyncEpoch = 0;
  net::TrialInfo trial;
  bool trialConfirmResult = false;
  bool trialRevertResult = false;
  std::function<void(vdm::Config&)> onBegin;  // begin() may change the config it is given
  // recorded
  int begins = 0;
  vdm::Config begunWith;
  std::vector<NetService> services;
  std::vector<vdm::Config> reconfigures;
  std::vector<uint32_t> inboundHttp;
  int trialConfirms = 0;
  int trialReverts = 0;
};
Net& net();

// ---------------------------------------------------------------- ota (src/ota.h)

struct RestartRequest {
  uint8_t reason;
  uint32_t delayMs;
  int32_t detail;
};
struct OtaService {
  uint32_t nowMs;
  bool netOk;
  bool linkUp;
  bool webStarted;
};
struct OtaRestartService {
  uint32_t nowMs;
  bool netUp;
  bool linkUp;
};
struct Ota {
  // scripted
  bool restartPending = false;  // set by requestRestart()
  bool uploadActive = false;
  bool uploadBeginResult = true;
  bool uploadWriteResult = true;
  bool uploadEndResult = true;
  std::string uploadError = "unknown";
  vdm::OtaHealthInfo health;
  // recorded
  int begins = 0;
  std::vector<RestartRequest> restartRequests;
  std::vector<std::pair<size_t, std::string>> uploadBegins;  // announced bytes, md5
  std::string uploadData;
  std::vector<bool> uploadEnds;  // commit flags
  std::vector<OtaService> services;
  std::vector<OtaRestartService> serviceRestarts;
};
Ota& ota();

// ---------------------------------------------------------------- mqtt (src/mqtt_client.h)

struct Mqtt {
  // scripted
  mqtt::Status status;
  vdm::RegulatorInput regulator;
  bool calibEnded[vdm::kValveCount] = {};
  vdm::LocalTime calibEnd[vdm::kValveCount];
  // recorded
  int begins = 0;
  int tasks = 0;
  int reconnectRequests = 0;
  std::vector<mqtt::DiscoveryAction> discoveryRequests;
};
Mqtt& mqtt();

// ---------------------------------------------------------------- stm_link, stm_service, web

struct StmLink {
  int releaseResets = 0;
  int begins = 0;
  int pulseResets = 0;
  int tasks = 0;
};
StmLink& stmLink();

struct StmService {
  int begins = 0;
  std::vector<uint32_t> services;
  int restartFlushes = 0;
};
StmService& stmService();

struct Web {
  bool started = false;  // set by begin()
  int begins = 0;
};
Web& web();

}  // namespace sib
