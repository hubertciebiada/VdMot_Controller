#include "stm_link.h"

#include <Arduino.h>
#include <esp_task_wdt.h>
#include <string.h>

#include <vdm/config.h>
#include <vdm/health_monitor.h>
#include <vdm/line_assembler.h>
#include <vdm/link_policy.h>
#include <vdm/poll_planner.h>
#include <vdm/stm_codec.h>
#include <vdm/stm_flasher.h>
#include <vdm/valve_model.h>

#include "app.h"
#include "boot_alloc.h"
#include "board.h"
#include "logger.h"
#include "ota.h"
#include "storage.h"

namespace stm_link {

namespace {

constexpr uint32_t kSensorStaleMs = 60000;       // DESIGN.md: temps stale after 60 s
constexpr uint32_t kSensorGraceMs = 30000;       // after a bus scan / STM reset: lists re-read
constexpr uint32_t kServiceMoveWaitMs = 300000;  // svmov accepted -> lastMove expected
constexpr uint32_t kScheduledCalibWindowMs =
    4UL * 3600UL * 1000UL;  // STM calibrates one valve at a time

HardwareSerial& gUart = Serial2;

void openUart(uint32_t baud, bool evenParity) {
  gUart.end();
  gUart.setRxBufferSize(board::kStmRxBufferSize);
  gUart.setTxBufferSize(board::kStmTxBufferSize);
  gUart.begin(baud, evenParity ? SERIAL_8E1 : SERIAL_8N1, board::kStmRxPin, board::kStmTxPin);
}

void setReset(bool asserted) {
  digitalWrite(board::kStmResetPin, asserted == board::kStmResetAssertedLevel ? HIGH : LOW);
}

// ---------------------------------------------------------------- flasher I/O

class UartTransport : public vdm::FlashTransport {
 public:
  void configure(uint32_t baud, bool evenParity) override { openUart(baud, evenParity); }
  // The flasher writes at most one frame (<= 260 B) per step after the
  // previous one was ACKed, so the 512 B TX ring buffer never blocks here.
  size_t write(const uint8_t* data, size_t len) override { return gUart.write(data, len); }
  size_t read(uint8_t* out, size_t cap) override {
    const int avail = gUart.available();
    if (avail <= 0 || cap == 0) return 0;
    return gUart.read(out, static_cast<size_t>(avail) < cap ? static_cast<size_t>(avail) : cap);
  }
  void discardInput() override {
    uint8_t buf[64];
    for (int i = 0; i < 64 && gUart.available() > 0; ++i) gUart.read(buf, sizeof buf);
  }
  void setReset(bool asserted) override { stm_link::setReset(asserted); }
};

// ---------------------------------------------------------------- state

vdm::StaticLineAssembler<vdm::kStmMaxLineLen + 1> gLines;
vdm::LinkPolicy gLink;
vdm::PollPlanner gPlanner;
vdm::ValveModel& gModel = bootAlloc<vdm::ValveModel>();
vdm::SensorModel& gSensors = bootAlloc<vdm::SensorModel>();
vdm::RebootDetector gReboot;
vdm::HealthMonitor gHealth;
vdm::Reply& gReply = bootAlloc<vdm::Reply>();
UartTransport gTransport;
vdm::StmFlasher gFlasher(gTransport);
storage::FileImage gImage;

vdm::Config& gCfg = bootAlloc<vdm::Config>();
uint32_t gCfgRevision = 0;
vdm::OneWireId gSlotIds[vdm::kTempSlotCount];

app::StmSnapshot& gSnap = bootAlloc<app::StmSnapshot>();
using ValveStates = ObjArray<vdm::ValveState, vdm::kValveCount>;
ValveStates& gPrev = bootAlloc<ValveStates>();
vdm::LinkState gPrevLink = vdm::LinkState::Unknown;
bool gDirty = true;
uint32_t gLastPublishMs = 0;
uint32_t gLastSecondMs = 0;
vdm::Version gMinVersion;
char gLoggedVersion[32] = {0};   // last StmVersion event text
bool gIncompatibleLogged = false;

// Scheduled calibration: CalibStarted of these valves carries arg1 = 1.
uint16_t gScheduledMask = 0;
uint32_t gScheduledAtMs = 0;

// Service moves accepted by the STM, waiting for their lastMove.
struct PendingMove {
  bool active = false;
  uint32_t moveSeq = 0;
  uint32_t sinceMs = 0;
};
PendingMove gServiceMoves[vdm::kValveCount];

// Sensor failure edges per config slot.
struct SlotTrack {
  bool known = false;
  bool valid = false;
};
SlotTrack gTempTrack[vdm::kTempSlotCount];
SlotTrack gVoltTrack[vdm::kVoltSlotCount];
uint32_t gSensorGraceFromMs = 0;
bool gSensorGrace = true;
uint8_t gLastTempCount = 0;
uint8_t gLastVoltCount = 0;
bool gCountsKnown = false;

void logEvents(const vdm::Event* ev, size_t n) {
  for (size_t i = 0; i < n; ++i) logger::log(ev[i]);
}

void startSensorGrace(uint32_t now) {
  gSensorGrace = true;
  gSensorGraceFromMs = now;
}

void reloadConfig() {
  gCfgRevision = storage::configRevision();
  storage::getConfig(gCfg);
  uint16_t mask = 0;
  for (uint8_t i = 0; i < vdm::kValveCount; ++i) {
    if (gCfg.valves[i].active) mask |= static_cast<uint16_t>(1u << i);
  }
  gModel.setActiveMask(mask);
  gPlanner.setActiveMask(mask);
  bool idsChanged = false;
  for (uint8_t i = 0; i < vdm::kTempSlotCount; ++i) {
    if (gSlotIds[i] != gCfg.temps[i].id) idsChanged = true;
    gSlotIds[i] = gCfg.temps[i].id;
  }
  // Slot ids changed: re-resolve the valve sensor assignment.
  if (idsChanged) gPlanner.requestValveSensors();
  gDirty = true;
}

// STM rebooted/was reset/re-flashed: forget link-level history and re-sync.
void resync(uint32_t now) {
  gLines.reset();
  gModel.onStmRebooted(now);
  gReboot.reset();
  gPlanner.requestResync();
  gSnap.proto = gPlanner.protocol();  // unknown until gproto answers again
  startSensorGrace(now);
  for (PendingMove& m : gServiceMoves) m.active = false;
  gDirty = true;
}

void afterStmReset(uint32_t now, bool byPolicy) {
  gLink.onStmReset(now, byPolicy);
  resync(now);
}

void onRebootDetected(uint32_t now, int32_t cause) {
  logger::log(vdm::EventCode::StmRebootDetected, vdm::kNoValve, cause);
  gModel.onStmRebooted(now);
  gPlanner.requestResync();
  gSnap.proto = gPlanner.protocol();
  startSensorGrace(now);
  gDirty = true;
}

bool enqueue(const vdm::RequestLine& r, vdm::Priority p) {
  if (r.len == 0) return false;
  if (gLink.enqueue(r, p) == vdm::EnqueueResult::Full) {
    logger::log(vdm::EventCode::StmQueueFull, vdm::kNoValve, static_cast<int32_t>(r.cmd));
    return false;
  }
  return true;
}

// ---------------------------------------------------------------- commands

void startFlash(const app::Command& c, uint32_t now) {
  if (gFlasher.active()) {
    logger::log(vdm::EventCode::StmFlashFailed, vdm::kNoValve, 0, 0, "busy");
    return;
  }
  // An ESP restart would cut the flash run (STM half-erased, in reset or in
  // 8E1). A restart is due at the earliest 1 s after it was requested, and
  // the flag is raised right after this check, before anything here can
  // block, so ota::serviceRestart() either made us refuse or sees the flag.
  if (ota::restartPending()) {
    logger::log(vdm::EventCode::StmFlashFailed, vdm::kNoValve, 0, 0, "restart pending");
    return;
  }
  app::markStmFlashActive();  // not only with the next snapshot (<= 100 ms)
  gDirty = true;              // a refused start below publishes "idle" again
  if (!gImage.open(c.image)) {
    logger::log(vdm::EventCode::StmFlashFailed, vdm::kNoValve,
                static_cast<int32_t>(vdm::FlashError::ImageRead), 0, c.image);
    return;
  }
  vdm::FlashOptions opt;
  opt.blank = c.blank;
  opt.force = c.force;
  gLink.suspend();
  if (!gFlasher.begin(gImage, opt, now)) {
    gImage.close();
    gLink.resume(now);
    logger::log(vdm::EventCode::StmFlashFailed, vdm::kNoValve, 0, 0, "start refused");
    return;
  }
  gLines.reset();
  vdm::copyString(gSnap.flashImage, sizeof gSnap.flashImage, c.image);
  gSnap.flash = gFlasher.status();
  logger::log(vdm::EventCode::StmFlashStarted, vdm::kNoValve,
              static_cast<int32_t>(gImage.size()), 0, c.image);
}

void handleCommand(const app::Command& c, uint32_t now) {
  vdm::RequestLine r;
  const bool flashing = gFlasher.active();
  switch (c.type) {
    case app::CommandType::SetTarget:
      if (!gModel.setDesiredTarget(c.valve, c.pos, c.source, now)) {
        logger::log(vdm::EventCode::MqttCommandRejected, vdm::kNoValve,
                    c.valve < vdm::kValveCount ? c.valve + 1 : 0, 0, "rejected by model");
      }
      gDirty = true;
      break;
    case app::CommandType::Calibrate:
      if (vdm::buildCalibrate(c.valve, r) && enqueue(r, vdm::Priority::User)) {
        if (c.scheduled) {
          gScheduledMask = gModel.activeMask();
          gScheduledAtMs = now;
        } else {
          gScheduledMask = 0;
        }
      }
      break;
    case app::CommandType::Assembly:
      if (vdm::buildAssembly(c.valve, r)) enqueue(r, vdm::Priority::User);
      break;
    case app::CommandType::Detect:
      if (vdm::buildDetect(r)) enqueue(r, vdm::Priority::User);
      break;
    case app::CommandType::ScanSensors:
      if (vdm::buildScanOneWire(r) && enqueue(r, vdm::Priority::User)) {
        gSensors.clear();
        gPlanner.requestTempList();
        gPlanner.requestVoltList();
        gPlanner.requestValveSensors();
        startSensorGrace(now);
      }
      break;
    case app::CommandType::SetValveSensors:
      if (vdm::buildSetValveSensors(c.valve, c.ids[0], c.ids[1], r) &&
          enqueue(r, vdm::Priority::User)) {
        if (vdm::buildMatchSensors(r)) enqueue(r, vdm::Priority::User);
        gPlanner.requestValveSensors();
      }
      break;
    case app::CommandType::SetMotorSettings: {
      // All or nothing on the link queue too: Poll entries make room, so
      // only queued User/Config requests count.
      const size_t need = (c.hasMotor ? 1u : 0u) + (c.hasLearnMovements ? 1u : 0u) +
                          (c.hasBreakaway && gPlanner.protocol() >= 2 ? 1u : 0u);
      const size_t held = gLink.queued(vdm::Priority::User) + gLink.queued(vdm::Priority::Config);
      if (held + need > vdm::LinkPolicy::kQueueCapacity) {
        logger::log(vdm::EventCode::StmQueueFull, vdm::kNoValve,
                    static_cast<int32_t>(vdm::Cmd::Smotc));
        break;
      }
      bool any = false;
      if (c.hasMotor && vdm::buildSetMotorChars(c.motor, r)) any |= enqueue(r, vdm::Priority::User);
      if (c.hasLearnMovements && vdm::buildSetLearnMovements(c.learnMovements, r)) {
        any |= enqueue(r, vdm::Priority::User);
      }
      // v2 only: a v1 STM would ignore it (the web answers 409 before).
      if (c.hasBreakaway && gPlanner.protocol() >= 2 && vdm::buildSetBreakaway(c.breakaway, r)) {
        any |= enqueue(r, vdm::Priority::User);
      }
      if (any) gPlanner.requestMotorParams();
      break;
    }
    case app::CommandType::ServiceMove:
      if (gPlanner.protocol() >= 2 &&
          vdm::buildServiceMove(c.valve, c.dir, c.counts, c.maxmA, r)) {
        enqueue(r, vdm::Priority::User);
      }
      break;
    case app::CommandType::RequestProfile:
      gPlanner.requestProfile(c.valve);
      break;
    case app::CommandType::ResetStm:
      if (flashing) {
        // The flasher owns NRST; a pulse now would corrupt the flash run.
        logger::log(vdm::EventCode::StmFlashFailed, vdm::kNoValve, 0, 0, "reset refused");
        break;
      }
      pulseReset();
      logger::log(vdm::EventCode::StmResetByUser);
      afterStmReset(now, false);
      break;
    case app::CommandType::StartFlash:
      startFlash(c, now);
      break;
    case app::CommandType::AbortFlash:
      if (flashing) gFlasher.abort();
      break;
    case app::CommandType::ConfigChanged:
      reloadConfig();
      break;
  }
}

// ---------------------------------------------------------------- replies

void onVersion(const vdm::Reply& rep) {
  gSnap.version = rep.version;
  gSnap.build = rep.build;
  gSnap.compatible = vdm::compareVersion(rep.version, gMinVersion) >= 0;
  char ver[32];
  if (vdm::formatVersion(rep.version, ver, sizeof ver) == 0) return;
  if (strcmp(ver, gLoggedVersion) != 0) {
    vdm::copyString(gLoggedVersion, sizeof gLoggedVersion, ver);
    gIncompatibleLogged = false;
    logger::log(vdm::EventCode::StmVersion, vdm::kNoValve, gPlanner.protocol(), gSnap.hwId, ver);
  }
  if (!gSnap.compatible && !gIncompatibleLogged) {
    gIncompatibleLogged = true;
    logger::log(vdm::EventCode::StmIncompatible, vdm::kNoValve, 0, 0, ver);
  }
  gPlanner.onVersion(vdm::isRevamped(rep.version));
  gSnap.proto = gPlanner.protocol();
}

// Applies reply data. `req` is the matched request or nullptr for a stray
// line (then only self-identifying replies are applied).
void applyReply(const vdm::Reply& rep, const vdm::RequestLine* req, uint32_t now) {
  gDirty = true;
  switch (rep.cmd) {
    case vdm::Cmd::Gvlvd:
      gModel.applyValveData(rep.valveData, now);
      if (gReboot.onValveData(rep.valveData)) onRebootDetected(now, 3);
      break;
    case vdm::Cmd::Gvlvx:
      gModel.applyValveEx(rep.valveEx, now);
      break;
    case vdm::Cmd::Gvlst:
      if (req) gModel.applyValveStates(rep.valveStates, now);
      break;
    case vdm::Cmd::Gtgtp:
      gModel.applyTarget(rep.target, now);
      break;
    case vdm::Cmd::Gonec:
      if (req && gSensors.applyTempList(rep.oneWireList, now) && !rep.oneWireList.hasList) {
        gPlanner.requestTempList();
      }
      gPlanner.setSensorCounts(gSensors.tempCount(), gSensors.voltCount());
      break;
    case vdm::Cmd::Gowvc:
      if (req && gSensors.applyVoltList(rep.oneWireList, now) && !rep.oneWireList.hasList) {
        gPlanner.requestVoltList();
      }
      gPlanner.setSensorCounts(gSensors.tempCount(), gSensors.voltCount());
      break;
    case vdm::Cmd::Goned:
      // The reply carries no bus index: only the requested one is known.
      if (req && req->cmd == vdm::Cmd::Goned) {
        gSensors.applyTempData(static_cast<uint8_t>(req->arg), rep.tempData, now);
      }
      break;
    case vdm::Cmd::Gowvd:
      if (req && req->cmd == vdm::Cmd::Gowvd) {
        gSensors.applyVoltData(static_cast<uint8_t>(req->arg), rep.voltData, now);
      }
      break;
    case vdm::Cmd::Gvlon:
      if (req && !rep.gvlonError) {
        gModel.applyValveSensors(rep.valveSensors, gSlotIds, vdm::kTempSlotCount);
      }
      break;
    case vdm::Cmd::Gmotc:
      gSnap.motor = rep.motorChars;
      gSnap.haveMotor = true;
      break;
    case vdm::Cmd::Gtlnm:
      gSnap.learnMovements = rep.learnMovements;
      break;
    case vdm::Cmd::Gcalx:
      gSnap.breakaway = rep.breakaway;
      gSnap.haveBreakaway = true;
      break;
    case vdm::Cmd::Gvers:
      onVersion(rep);
      break;
    case vdm::Cmd::Ghwin:
      gSnap.hwId = rep.hwId;
      break;
    case vdm::Cmd::Gproto:
      gPlanner.setProtocol(rep.proto);
      gSnap.proto = gPlanner.protocol();
      break;
    case vdm::Cmd::Gstat:
      gSnap.status = rep.status;
      gSnap.haveStatus = true;
      if (gReboot.onStatus(rep.status)) onRebootDetected(now, 1);
      break;
    case vdm::Cmd::Gprof:
      if (rep.profile.valve < vdm::kValveCount) gSnap.profiles[rep.profile.valve] = rep.profile;
      break;
    default:
      break;
  }
}

void onServiceMoveResult(const vdm::Completion& c, const vdm::Reply* rep, uint32_t now) {
  const uint8_t v = c.request.valve;
  if (v >= vdm::kValveCount) return;
  if (c.outcome == vdm::Outcome::Ok) {
    gServiceMoves[v].active = true;
    gServiceMoves[v].moveSeq = gModel.valve(v).moveSeq;
    gServiceMoves[v].sinceMs = now;
    gPlanner.requestTarget(v);  // fast read-back of gvlvx
    return;
  }
  // Rejected ("svmov v err n") or no answer: reported as a failed service move.
  const int32_t code = (c.outcome == vdm::Outcome::Rejected && rep != nullptr)
                           ? static_cast<int32_t>(rep->serviceMove.errorCode)
                           : -1;
  logger::logSev(vdm::EventCode::ServiceMoveDone, vdm::Severity::Warning, v, -1, code,
                 c.outcome == vdm::Outcome::Rejected ? "rejected" : "no reply");
}

void onCompletion(const vdm::Completion& c, const vdm::Reply* rep, uint32_t now) {
  const bool ok = c.outcome == vdm::Outcome::Ok;
  switch (c.request.cmd) {
    case vdm::Cmd::Stgtp:
      if (ok) {
        gModel.onTargetAck(c.request.valve, now);
      } else {
        gModel.onTargetTimeout(c.request.valve, now);
      }
      break;
    case vdm::Cmd::Svmov:
      onServiceMoveResult(c, rep, now);
      break;
    default:
      gPlanner.onResult(c.request, ok, now);
      break;
  }
  if (c.request.cmd == vdm::Cmd::Gproto) gSnap.proto = gPlanner.protocol();
  gDirty = true;
}

void processLine(uint32_t now) {
  const vdm::ParseStatus ps = vdm::parseReply(gLines.line(), gLines.length(), gReply);
  gLines.release();
  if (ps != vdm::ParseStatus::Ok) {
    gLink.onParseError(now);
    return;
  }
  vdm::Completion done;
  if (gLink.onReply(gReply, now, done)) {
    applyReply(gReply, &done.request, now);
    onCompletion(done, &gReply, now);
  } else {
    applyReply(gReply, nullptr, now);
  }
}

void readUart(uint32_t now) {
  char buf[128];
  // Bounded per iteration: at most 4 chunks (512 B) before yielding.
  for (int chunk = 0; chunk < 4; ++chunk) {
    const int avail = gUart.available();
    if (avail <= 0) return;
    const size_t want =
        static_cast<size_t>(avail) < sizeof buf ? static_cast<size_t>(avail) : sizeof buf;
    const size_t n = gUart.read(reinterpret_cast<uint8_t*>(buf), want);
    size_t off = 0;
    while (off < n) {
      off += gLines.feed(buf + off, n - off);
      if (gLines.hasLine()) {
        processLine(now);
      } else if (off < n) {
        break;  // defensive: the assembler refused bytes without a line
      }
    }
  }
}

void scheduleRequests(uint32_t now) {
  vdm::RequestLine r;
  uint8_t valve = 0, pos = 0;
  // Unknown protocol (re-sync) is treated like 1.x.
  gModel.setHoldTargetsWhileCalibrating(gPlanner.protocol() < 2);
  if (gLink.queued(vdm::Priority::Config) == 0) {
    if (gModel.nextTargetPush(now, valve, pos)) {
      // A push that cannot be queued is retried after pushRetryMs; it is not
      // logged (enqueue()) because that would repeat every 2 s while the
      // queue stays full, the link counts it in queueFull.
      vdm::EnqueueResult res = vdm::EnqueueResult::Invalid;
      if (vdm::buildSetTarget(valve, pos, r)) res = gLink.enqueue(r, vdm::Priority::Config);
      if (res != vdm::EnqueueResult::Queued && res != vdm::EnqueueResult::Coalesced) {
        gModel.onTargetPushDropped(valve, now);
      }
    } else if (gModel.nextVerify(valve)) {
      gPlanner.requestTarget(valve);
    }
  }
  for (uint8_t i = 0; i < vdm::kValveCount; ++i) gPlanner.setValveBusy(i, gModel.isBusy(i));
  if (gLink.queued(vdm::Priority::Poll) == 0 && gPlanner.next(now, r)) {
    enqueue(r, gPlanner.lastWasResync() ? vdm::Priority::Config : vdm::Priority::Poll);
  }
}

void serviceFlasher(uint32_t now) {
  const vdm::FlashPhase phase = gFlasher.step(now);
  gSnap.flash = gFlasher.status();
  gDirty = true;
  if (phase != vdm::FlashPhase::Done && phase != vdm::FlashPhase::Failed) return;
  gImage.close();
  openUart(board::kStmBaud, false);  // the flasher restored 8N1 already; make sure
  const vdm::FlashStatus& st = gFlasher.status();
  if (phase == vdm::FlashPhase::Done) {
    char ver[32] = {0};
    vdm::formatVersion(st.appVersion, ver, sizeof ver);
    logger::log(vdm::EventCode::StmFlashDone, vdm::kNoValve,
                static_cast<int32_t>(vdm::elapsedMs(st.finishedMs, st.startedMs)), 0, ver);
    storage::requestLastGoodCopy(gSnap.flashImage);
  } else {
    logger::log(vdm::EventCode::StmFlashFailed, vdm::kNoValve, static_cast<int32_t>(st.error),
                static_cast<int32_t>(st.errorAddress), vdm::flashPhaseName(st.errorPhase));
  }
  // The flasher reset the STM: 5 s hold-off, full re-sync, targets re-pushed.
  gLink.resume(now);
  resync(now);
}

// Sensor slot failure/recovery edges (once per second).
void checkSensors(uint32_t now) {
  if (gSensorGrace) {
    if (vdm::elapsedMs(now, gSensorGraceFromMs) < kSensorGraceMs) return;
    gSensorGrace = false;
  }
  vdm::Event ev[vdm::kMaxEventsPerUpdate];
  for (uint8_t i = 0; i < vdm::kTempSlotCount; ++i) {
    SlotTrack& t = gTempTrack[i];
    const vdm::TempSlotConfig& slot = gCfg.temps[i];
    if (!slot.active || vdm::isZero(slot.id)) {
      t = SlotTrack{};
      continue;
    }
    const int bus = gSensors.findTemp(slot.id);
    const vdm::TempReading& rd = gSensors.temp(bus >= 0 ? static_cast<uint8_t>(bus) : 0xFF);
    const bool valid = bus >= 0 && vdm::tempRawValid(rd.raw) &&
                       gSensors.tempFresh(static_cast<uint8_t>(bus), now, kSensorStaleMs);
    const size_t n = gHealth.onTempSensor(static_cast<uint8_t>(i + 1), t.known, t.valid, valid,
                                          rd.raw, ev, vdm::kMaxEventsPerUpdate);
    for (size_t k = 0; k < n; ++k) {
      if (ev[k].code == vdm::EventCode::TempSensorFailed) {
        vdm::formatOneWireId(slot.id, ev[k].text, sizeof ev[k].text);
      }
      logger::log(ev[k]);
    }
    t.known = true;
    t.valid = valid;
  }
  for (uint8_t i = 0; i < vdm::kVoltSlotCount; ++i) {
    SlotTrack& t = gVoltTrack[i];
    const vdm::VoltSlotConfig& slot = gCfg.volts[i];
    if (!slot.active || vdm::isZero(slot.id)) {
      t = SlotTrack{};
      continue;
    }
    const int bus = gSensors.findVolt(slot.id);
    const vdm::VoltReading& rd = gSensors.volt(bus >= 0 ? static_cast<uint8_t>(bus) : 0xFF);
    const bool valid = bus >= 0 && rd.seen && vdm::vadValid(rd.vad) &&
                       vdm::elapsedMs(now, rd.lastSeenMs) <= kSensorStaleMs;
    if (t.known && t.valid && !valid) {
      logger::log(vdm::EventCode::VoltSensorFailed, vdm::kNoValve, i + 1, rd.vad);
    }
    t.known = true;
    t.valid = valid;
  }
  const uint8_t tc = gSensors.tempCount();
  const uint8_t vc = gSensors.voltCount();
  if (gCountsKnown && tc != gLastTempCount) {
    logger::log(vdm::EventCode::SensorCountChanged, vdm::kNoValve, tc, 0);
  }
  if (gCountsKnown && vc != gLastVoltCount) {
    logger::log(vdm::EventCode::SensorCountChanged, vdm::kNoValve, vc, 1);
  }
  gCountsKnown = true;
  gLastTempCount = tc;
  gLastVoltCount = vc;
}

// Once per second: model staleness, counters, sensor edges, service moves.
void everySecond(uint32_t now) {
  gModel.tick(now);
  vdm::Event ev[vdm::kMaxEventsPerUpdate];
  const vdm::LinkStats& ls = gLink.stats();
  logEvents(ev, gHealth.onStmCounters(gLines.overflowCount(),
                                      ls.parseErrors + gLines.malformedCount(), 0, now, ev,
                                      vdm::kMaxEventsPerUpdate));
  if (gSnap.haveStatus) {
    logEvents(ev, gHealth.onStmCounters(gSnap.status.rxOverflow, gSnap.status.parseErrors, 1, now,
                                        ev, vdm::kMaxEventsPerUpdate));
  }
  if (!gFlasher.active()) checkSensors(now);
  // v2 has no gvlvd: valve temperatures from gvlon + goned (v1: from gvlvd).
  if (gPlanner.protocol() >= 2) gModel.applySensorTemps(gSensors, now, kSensorStaleMs);
  const bool settled =
      gLink.state(now) == vdm::LinkState::Up && !gPlanner.resyncActive() && !gSensorGrace;
  if (settled != gSnap.sensorsSettled) {
    gSnap.sensorsSettled = settled;
    gDirty = true;
  }
  for (uint8_t v = 0; v < vdm::kValveCount; ++v) {
    PendingMove& m = gServiceMoves[v];
    if (!m.active) continue;
    const vdm::ValveState& s = gModel.valve(v);
    if (s.moveSeq != m.moveSeq) {
      m.active = false;
      logger::log(vdm::EventCode::ServiceMoveDone, v,
                  static_cast<int32_t>(s.lastMove.countedCounts),
                  static_cast<int32_t>(s.lastMove.stop));
    } else if (vdm::elapsedMs(now, m.sinceMs) >= kServiceMoveWaitMs) {
      m.active = false;
    }
  }
  if (gScheduledMask != 0 && vdm::elapsedMs(now, gScheduledAtMs) >= kScheduledCalibWindowMs) {
    gScheduledMask = 0;
  }
}

void publish(uint32_t now) {
  vdm::Event ev[vdm::kMaxEventsPerUpdate];
  for (uint8_t i = 0; i < vdm::kValveCount; ++i) {
    const vdm::ValveState& cur = gModel.valve(i);
    if (cur.revision == gPrev[i].revision) continue;
    const bool active = (gModel.activeMask() >> i) & 1u;
    const size_t n = gHealth.onValve(i, gPrev[i], cur, active, ev, vdm::kMaxEventsPerUpdate);
    for (size_t k = 0; k < n; ++k) {
      if (ev[k].code == vdm::EventCode::CalibStarted && ((gScheduledMask >> i) & 1u)) {
        ev[k].arg1 = 1;
        gScheduledMask = static_cast<uint16_t>(gScheduledMask & ~(1u << i));
      }
      logger::log(ev[k]);
    }
    gPrev[i] = cur;
    gDirty = true;
  }
  const vdm::LinkState ls = gLink.state(now);
  if (ls != gPrevLink) {
    logEvents(ev, gHealth.onLink(gPrevLink, ls, gLink.stats().consecutiveTimeouts, ev,
                                 vdm::kMaxEventsPerUpdate));
    if (gReboot.onLinkState(gPrevLink, ls)) onRebootDetected(now, 4);
    gPrevLink = ls;
    gDirty = true;
  }
  if (!gDirty || vdm::elapsedMs(now, gLastPublishMs) < 100) return;
  for (uint8_t i = 0; i < vdm::kValveCount; ++i) gSnap.valves[i] = gModel.valve(i);
  gSnap.tempCount = gSensors.tempCount();
  for (uint8_t i = 0; i < vdm::kTempSlotCount; ++i) gSnap.temps[i] = gSensors.temp(i);
  gSnap.voltCount = gSensors.voltCount();
  for (uint8_t i = 0; i < vdm::kVoltSlotCount; ++i) gSnap.volts[i] = gSensors.volt(i);
  gSnap.link = ls;
  gSnap.linkStats = gLink.stats();
  gSnap.lineOverflows = gLines.overflowCount();
  gSnap.lineMalformed = gLines.malformedCount();
  gSnap.takenMs = now;
  ++gSnap.revision;
  app::publishStmSnapshot(gSnap);
  gLastPublishMs = now;
  gDirty = false;
}

}  // namespace

void pulseReset() {
  setReset(true);
  vTaskDelay(pdMS_TO_TICKS(100));
  setReset(false);
}

void releaseReset() {
  // BOOT0 LOW before NRST is released (specs/06 §5.2), so a wired BOOT0
  // never starts the ROM bootloader.
  digitalWrite(board::kStmBoot0Pin, LOW);
  pinMode(board::kStmBoot0Pin, OUTPUT);
  setReset(false);
  pinMode(board::kStmResetPin, OUTPUT);
}

void begin() {
  // R6: never reset the STM on ESP boot. NRST was released in initVariant
  // already; doing it again is harmless and covers a missing hook.
  releaseReset();
  openUart(board::kStmBaud, false);
  vdm::parseVersion(vdm::minStmVersion(), strlen(vdm::minStmVersion()), gMinVersion);
}

void task(void*) {
  esp_task_wdt_add(nullptr);
  const uint32_t start = app::nowMs();
  reloadConfig();
  startSensorGrace(start);
  gPlanner.requestResync();
  // R6: no reset here, but the IO15 strap most likely reset the STM while
  // the ESP booted; give it the same start-up hold-off as after a pulse.
  gLink.holdAfterEspBoot(start);
  gLastSecondMs = start;
  for (;;) {
    esp_task_wdt_reset();
    const uint32_t now = app::nowMs();
    if (storage::configRevision() != gCfgRevision) reloadConfig();

    app::Command cmd;
    for (int i = 0; i < 4 && app::receive(cmd); ++i) handleCommand(cmd, now);

    if (gFlasher.active()) {
      serviceFlasher(now);
    } else {
      readUart(now);
      vdm::Completion done;
      if (gLink.poll(now, done)) onCompletion(done, nullptr, now);
      if (gLink.shouldResetStm(now)) {
        const vdm::LinkStats& st = gLink.stats();
        logger::log(vdm::EventCode::StmResetByPolicy, vdm::kNoValve, st.consecutiveTimeouts,
                    static_cast<int32_t>(vdm::elapsedMs(now, st.lastReplyMs) / 1000));
        pulseReset();
        afterStmReset(app::nowMs(), true);
      }
      scheduleRequests(now);
      if (const vdm::RequestLine* line = gLink.nextToSend(now)) {
        gUart.write(reinterpret_cast<const uint8_t*>(line->text), line->len);
        gLink.onSent(app::nowMs());
      }
    }
    if (vdm::elapsedMs(now, gLastSecondMs) >= 1000) {
      gLastSecondMs = now;
      everySecond(now);
    }
    publish(now);
    vTaskDelay(pdMS_TO_TICKS(2));
  }
}

}  // namespace stm_link
