#include "stm_link.h"

#include <Arduino.h>
#include <LittleFS.h>
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
#include "board.h"
#include "logger.h"
#include "storage.h"

namespace stm_link {

namespace {

HardwareSerial& gUart = Serial2;

void openUart(uint32_t baud, bool evenParity) {
  gUart.end();
  gUart.setRxBufferSize(board::kStmRxBufferSize);
  gUart.setTxBufferSize(board::kStmTxBufferSize);
  gUart.begin(baud, evenParity ? SERIAL_8E1 : SERIAL_8N1, board::kStmRxPin, board::kStmTxPin);
}

void setReset(bool asserted) {
  digitalWrite(board::kStmResetPin,
               asserted == board::kStmResetAssertedLevel ? HIGH : LOW);
}

// ---------------------------------------------------------------- flasher I/O

class UartTransport : public vdm::FlashTransport {
 public:
  void configure(uint32_t baud, bool evenParity) override { openUart(baud, evenParity); }
  size_t write(const uint8_t* data, size_t len) override { return gUart.write(data, len); }
  size_t read(uint8_t* out, size_t cap) override {
    const int avail = gUart.available();
    if (avail <= 0) return 0;
    return gUart.read(out, static_cast<size_t>(avail) < cap ? static_cast<size_t>(avail) : cap);
  }
  void discardInput() override {
    while (gUart.available() > 0) gUart.read();
  }
  void setReset(bool asserted) override { stm_link::setReset(asserted); }
};

class FileImage : public vdm::FlashImage {
 public:
  bool open(const char* name) {
    close();
    char path[48];
    snprintf(path, sizeof path, "/stm/%s", name);
    file_ = LittleFS.open(path, FILE_READ);
    return static_cast<bool>(file_);
  }
  void close() {
    if (file_) file_.close();
  }
  uint32_t size() const override { return file_ ? static_cast<uint32_t>(file_.size()) : 0; }
  bool read(uint32_t offset, uint8_t* out, size_t len) override {
    return file_ && file_.seek(offset) && file_.read(out, len) == len;
  }

 private:
  mutable File file_;
};

// ---------------------------------------------------------------- state

vdm::StaticLineAssembler<vdm::kStmMaxLineLen + 1> gLines;
vdm::LinkPolicy gLink;
vdm::PollPlanner gPlanner;
vdm::ValveModel gModel;
vdm::SensorModel gSensors;
vdm::RebootDetector gReboot;
vdm::HealthMonitor gHealth;
vdm::Reply gReply;
UartTransport gTransport;
vdm::StmFlasher gFlasher(gTransport);
FileImage gImage;

vdm::Config gCfg;
uint32_t gCfgRevision = 0;
vdm::OneWireId gSlotIds[vdm::kTempSlotCount];

app::StmSnapshot gSnap;
vdm::ValveState gPrev[vdm::kValveCount];
vdm::LinkState gPrevLink = vdm::LinkState::Unknown;
bool gDirty = true;
uint32_t gLastPublishMs = 0;
vdm::Version gMinVersion;

void logEvents(const vdm::Event* ev, size_t n) {
  for (size_t i = 0; i < n; ++i) logger::log(ev[i]);
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
  for (uint8_t i = 0; i < vdm::kTempSlotCount; ++i) gSlotIds[i] = gCfg.temps[i].id;
}

void afterStmReset(uint32_t now, bool byPolicy) {
  gLink.onStmReset(now, byPolicy);
  gLines.reset();
  gModel.onStmRebooted(now);
  gReboot.reset();
  gPlanner.requestResync();
  gDirty = true;
}

void enqueue(const vdm::RequestLine& r, vdm::Priority p) {
  if (r.len == 0) return;
  if (gLink.enqueue(r, p) == vdm::EnqueueResult::Full) {
    logger::log(vdm::EventCode::StmQueueFull, vdm::kNoValve, static_cast<int32_t>(r.cmd));
  }
}

// ---------------------------------------------------------------- commands

void startFlash(const app::Command& c, uint32_t now) {
  if (gFlasher.active() || !gImage.open(c.image)) {
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
    return;
  }
  vdm::copyString(gSnap.flashImage, sizeof gSnap.flashImage, c.image);
  logger::log(vdm::EventCode::StmFlashStarted, vdm::kNoValve,
              static_cast<int32_t>(gImage.size()), 0, c.image);
}

void handleCommand(const app::Command& c, uint32_t now) {
  vdm::RequestLine r;
  switch (c.type) {
    case app::CommandType::SetTarget:
      gModel.setDesiredTarget(c.valve, c.pos, c.source, now);
      gDirty = true;
      break;
    case app::CommandType::Calibrate:
      if (vdm::buildCalibrate(c.valve, r)) enqueue(r, vdm::Priority::User);
      break;
    case app::CommandType::Assembly:
      if (vdm::buildAssembly(c.valve, r)) enqueue(r, vdm::Priority::User);
      break;
    case app::CommandType::Detect:
      if (vdm::buildDetect(r)) enqueue(r, vdm::Priority::User);
      break;
    case app::CommandType::ScanSensors:
      if (vdm::buildScanOneWire(r)) enqueue(r, vdm::Priority::User);
      gSensors.clear();
      gPlanner.requestTempList();
      gPlanner.requestVoltList();
      break;
    case app::CommandType::SetValveSensors:
      if (vdm::buildSetValveSensors(c.valve, c.ids[0], c.ids[1], r)) {
        enqueue(r, vdm::Priority::User);
        if (vdm::buildMatchSensors(r)) enqueue(r, vdm::Priority::User);
        gPlanner.requestValveSensors();
      }
      break;
    case app::CommandType::SetMotorChars:
      if (vdm::buildSetMotorChars(c.motor, r)) enqueue(r, vdm::Priority::User);
      gPlanner.requestMotorParams();
      break;
    case app::CommandType::SetLearnMovements:
      if (vdm::buildSetLearnMovements(c.learnMovements, r)) enqueue(r, vdm::Priority::User);
      gPlanner.requestMotorParams();
      break;
    case app::CommandType::SetBreakaway:
      if (gPlanner.protocol() >= 2 && vdm::buildSetBreakaway(c.breakaway, r)) {
        enqueue(r, vdm::Priority::User);
        gPlanner.requestMotorParams();
      }
      break;
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
      pulseReset();
      logger::log(vdm::EventCode::StmResetByUser);
      afterStmReset(now, false);
      break;
    case app::CommandType::StartFlash:
      startFlash(c, now);
      break;
    case app::CommandType::AbortFlash:
      gFlasher.abort();
      break;
    case app::CommandType::ConfigChanged:
      reloadConfig();
      break;
  }
}

// ---------------------------------------------------------------- replies

// Applies reply data. `req` is the matched request or nullptr for a stray
// line (then only self-identifying replies are applied).
void applyReply(const vdm::Reply& rep, const vdm::RequestLine* req, uint32_t now) {
  gDirty = true;
  switch (rep.cmd) {
    case vdm::Cmd::Gvlvd:
      gModel.applyValveData(rep.valveData, now);
      if (gReboot.onValveData(rep.valveData)) {
        logger::log(vdm::EventCode::StmRebootDetected, vdm::kNoValve, 3);
        gModel.onStmRebooted(now);
        gPlanner.requestResync();
      }
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
      if (req) gSensors.applyTempData(static_cast<uint8_t>(req->arg), rep.tempData, now);
      break;
    case vdm::Cmd::Gowvd:
      if (req) gSensors.applyVoltData(static_cast<uint8_t>(req->arg), rep.voltData, now);
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
      gSnap.version = rep.version;
      gSnap.build = rep.build;
      gSnap.compatible = vdm::compareVersion(rep.version, gMinVersion) >= 0;
      break;
    case vdm::Cmd::Ghwin:
      gSnap.hwId = rep.hwId;
      break;
    case vdm::Cmd::Gproto:
      gPlanner.setProtocol(rep.proto);
      gSnap.proto = rep.proto;
      break;
    case vdm::Cmd::Gstat:
      gSnap.status = rep.status;
      gSnap.haveStatus = true;
      if (gReboot.onStatus(rep.status)) {
        logger::log(vdm::EventCode::StmRebootDetected, vdm::kNoValve, 1);
        gModel.onStmRebooted(now);
        gPlanner.requestResync();
      }
      break;
    case vdm::Cmd::Gprof:
      if (rep.profile.valve < vdm::kValveCount) gSnap.profiles[rep.profile.valve] = rep.profile;
      break;
    default:
      break;
  }
}

void onCompletion(const vdm::Completion& c, uint32_t now) {
  const bool ok = c.outcome == vdm::Outcome::Ok;
  if (c.request.cmd == vdm::Cmd::Stgtp) {
    if (ok) {
      gModel.onTargetAck(c.request.valve, now);
    } else {
      gModel.onTargetTimeout(c.request.valve, now);
    }
  } else {
    gPlanner.onResult(c.request, ok, now);
  }
  if (c.request.cmd == vdm::Cmd::Gproto && !ok) gSnap.proto = gPlanner.protocol();
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
    onCompletion(done, now);
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
    const size_t n = gUart.readBytes(buf, static_cast<size_t>(avail) < sizeof buf
                                              ? static_cast<size_t>(avail)
                                              : sizeof buf);
    size_t off = 0;
    while (off < n) {
      off += gLines.feed(buf + off, n - off);
      if (gLines.hasLine()) processLine(now);
      else if (off < n) break;  // defensive: assembler refused bytes without a line
    }
  }
}

void scheduleRequests(uint32_t now) {
  vdm::RequestLine r;
  uint8_t valve = 0, pos = 0;
  if (gLink.queued(vdm::Priority::Config) == 0) {
    if (gModel.nextTargetPush(now, valve, pos) && vdm::buildSetTarget(valve, pos, r)) {
      enqueue(r, vdm::Priority::Config);
    } else if (gModel.nextVerify(valve)) {
      gPlanner.requestTarget(valve);
    }
  }
  for (uint8_t i = 0; i < vdm::kValveCount; ++i) gPlanner.setValveBusy(i, gModel.isBusy(i));
  if (gLink.queued(vdm::Priority::Poll) == 0 && gPlanner.next(now, r)) {
    enqueue(r, gPlanner.resyncActive() ? vdm::Priority::Config : vdm::Priority::Poll);
  }
}

void serviceFlasher(uint32_t now) {
  const vdm::FlashPhase phase = gFlasher.step(now);
  gSnap.flash = gFlasher.status();
  gDirty = true;
  if (phase != vdm::FlashPhase::Done && phase != vdm::FlashPhase::Failed) return;
  gImage.close();
  openUart(board::kStmBaud, false);
  const vdm::FlashStatus& st = gFlasher.status();
  if (phase == vdm::FlashPhase::Done) {
    char ver[32];
    vdm::formatVersion(st.appVersion, ver, sizeof ver);
    logger::log(vdm::EventCode::StmFlashDone, vdm::kNoValve,
                static_cast<int32_t>(st.finishedMs - st.startedMs), 0, ver);
  } else {
    logger::log(vdm::EventCode::StmFlashFailed, vdm::kNoValve, static_cast<int32_t>(st.error),
                static_cast<int32_t>(st.errorAddress), vdm::flashPhaseName(st.errorPhase));
  }
  gLink.resume(now);
  afterStmReset(now, false);
}

void publish(uint32_t now) {
  vdm::Event ev[vdm::kMaxEventsPerUpdate];
  for (uint8_t i = 0; i < vdm::kValveCount; ++i) {
    const vdm::ValveState& cur = gModel.valve(i);
    if (cur.revision == gPrev[i].revision) continue;
    const bool active = (gModel.activeMask() >> i) & 1u;
    logEvents(ev, gHealth.onValve(i, gPrev[i], cur, active, ev, vdm::kMaxEventsPerUpdate));
    gPrev[i] = cur;
    gDirty = true;
  }
  const vdm::LinkState ls = gLink.state(now);
  if (ls != gPrevLink) {
    logEvents(ev, gHealth.onLink(gPrevLink, ls, gLink.stats().consecutiveTimeouts, ev,
                                 vdm::kMaxEventsPerUpdate));
    if (gReboot.onLinkState(gPrevLink, ls)) {
      logger::log(vdm::EventCode::StmRebootDetected, vdm::kNoValve, 4);
      gModel.onStmRebooted(now);
      gPlanner.requestResync();
    }
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

void begin() {
  // R6: never reset the STM on ESP boot. Drive NRST released right away (the
  // IO15 strap pull-up may hold the STM in reset until this point).
  pinMode(board::kStmResetPin, OUTPUT);
  setReset(false);
  pinMode(board::kStmBoot0Pin, OUTPUT);
  digitalWrite(board::kStmBoot0Pin, LOW);
  openUart(board::kStmBaud, false);
  vdm::parseVersion(vdm::minStmVersion(), strlen(vdm::minStmVersion()), gMinVersion);
}

void task(void*) {
  esp_task_wdt_add(nullptr);
  reloadConfig();
  gPlanner.requestResync();
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
      if (gLink.poll(now, done)) onCompletion(done, now);
      if (gLink.shouldResetStm(now)) {
        logger::log(vdm::EventCode::StmResetByPolicy, vdm::kNoValve,
                    gLink.stats().consecutiveTimeouts);
        pulseReset();
        afterStmReset(now, true);
      }
      gModel.tick(now);
      scheduleRequests(now);
      if (const vdm::RequestLine* line = gLink.nextToSend(now)) {
        gUart.write(reinterpret_cast<const uint8_t*>(line->text), line->len);
        gLink.onSent(now);
      }
    }
    publish(now);
    vTaskDelay(pdMS_TO_TICKS(2));
  }
}

}  // namespace stm_link
