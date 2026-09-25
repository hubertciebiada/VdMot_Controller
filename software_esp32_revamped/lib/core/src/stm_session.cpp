#include "vdm/stm_session.h"

#include <string.h>

#include "vdm/version.h"

namespace vdm {

StmSession::StmSession(StmSessionPort& port, FlashTransport& transport)
    : port_(port), transport_(transport), flasher_(transport) {
  parseVersion(minStmVersion(), strlen(minStmVersion()), minVersion_);
}

// ---------------------------------------------------------------- helpers

void StmSession::log(EventCode code, uint8_t valve, int32_t a1, int32_t a2, const char* text) {
  port_.logEvent(makeEvent(code, eventDefaultSeverity(code), valve, a1, a2, text));
}

void StmSession::logEvents(const Event* ev, size_t n) {
  for (size_t i = 0; i < n; ++i) port_.logEvent(ev[i]);
}

bool StmSession::enqueue(const RequestLine& r, Priority p, uint16_t tag) {
  if (r.len == 0) return false;
  if (link_.enqueue(r, p, tag) == EnqueueResult::Full) {
    log(EventCode::StmQueueFull, kNoValve, static_cast<int32_t>(r.cmd));
    return false;
  }
  return true;
}

bool StmSession::stmAnswers(uint32_t nowMs) const {
  const LinkState s = link_.state(nowMs);
  return s == LinkState::Up || s == LinkState::Degraded;
}

void StmSession::startSensorGrace(uint32_t nowMs) {
  sensorGrace_ = true;
  sensorGraceFromMs_ = nowMs;
}

// ---------------------------------------------------------------- config and start

void StmSession::applyConfig(const Config& cfg, bool trusted) {
  cfg_ = cfg;
  uint16_t mask = 0;
  for (uint8_t i = 0; i < kValveCount; ++i) {
    if (cfg_.valves[i].active) mask = static_cast<uint16_t>(mask | (1u << i));
  }
  model_.setActiveMask(mask);
  planner_.setActiveMask(mask);
  bool idsChanged = false;
  for (uint8_t i = 0; i < kTempSlotCount; ++i) {
    if (slotIds_[i] != cfg_.temps[i].id) idsChanged = true;
    slotIds_[i] = cfg_.temps[i].id;
  }
  // Slot ids changed: re-resolve the valve sensor assignment.
  if (idsChanged) planner_.requestValveSensors();
  LeaseConfig lc;
  effectiveLeaseConfig(cfg_, lc);
  lease_.setConfig(lc);
  lease_.setConfigTrusted(trusted);
  learn_.setDesired(stmLearnTime(cfg_.calib));
  dirty_ = true;
}

void StmSession::begin(uint32_t nowMs, const PersistedTargets& targets, RestoreSource src,
                       const LeaseClient::Snapshot* lease) {
  startSensorGrace(nowMs);
  planner_.requestResync();
  // R6: no reset here, but the IO15 strap most likely reset the STM while
  // the ESP booted; give it the same start-up hold-off as after a pulse.
  link_.holdAfterEspBoot(nowMs);
  if (lease != nullptr) lease_.restore(*lease, nowMs);
  const uint8_t n = restoreTargets(model_, targets);
  if (n > 0) log(EventCode::TargetsRestored, kNoValve, n, static_cast<int32_t>(src));
  lastDesiredRev_ = model_.desiredRevision();
  dirty_ = true;
}

// Everything the STM knew about this session is gone.
void StmSession::newStmSession(uint32_t nowMs) {
  model_.onStmRebooted(nowMs);
  planner_.requestResync();
  lease_.onStmReboot();
  learn_.onStmReboot();
  havePrevStatus_ = false;
  snap_.proto = planner_.protocol();  // unknown until gproto answers again
  snap_.support = planner_.support();
  startSensorGrace(nowMs);
  dirty_ = true;
}

// STM rebooted/was reset/re-flashed by us: forget link-level history too.
void StmSession::resync(uint32_t nowMs) {
  lines_.reset();
  reboot_.reset();
  newStmSession(nowMs);
  for (PendingMove& m : moves_) m.active = false;
}

void StmSession::afterStmReset(uint32_t nowMs, bool byPolicy) {
  link_.onStmReset(nowMs, byPolicy);
  resync(nowMs);
}

void StmSession::onRebootDetected(uint32_t nowMs, int32_t cause) {
  log(EventCode::StmRebootDetected, kNoValve, cause);
  newStmSession(nowMs);
}

// ---------------------------------------------------------------- commands

void StmSession::calibrate(const StmCommand& c, uint32_t nowMs) {
  RequestLine r;
  if (c.scheduled && flashing()) {
    port_.postScheduledCalibResult(c.attempt, false, CalibFailure::NotSent);
    return;
  }
  const uint16_t tag = c.scheduled ? kTagScheduledCalib : 0;
  if (!buildCalibrate(c.valve, r) || !enqueue(r, Priority::User, tag)) {
    if (c.scheduled) port_.postScheduledCalibResult(c.attempt, false, CalibFailure::NotSent);
    return;
  }
  if (c.scheduled) {
    schedAttempt_ = c.attempt;
    scheduledMask_ = model_.activeMask();
    scheduledAtMs_ = nowMs;
  } else {
    scheduledMask_ = 0;
  }
}

void StmSession::setMotorSettings(const StmCommand& c) {
  RequestLine r;
  // All or nothing on the link queue too: Poll entries make room, so only
  // queued User/Config requests count.
  const bool v2 = planner_.protocol() >= 2;
  const size_t need = (c.hasMotor ? 1u : 0u) + (c.hasLearnMovements ? 1u : 0u) +
                      (c.hasBreakaway && v2 ? 1u : 0u);
  const size_t held = link_.queued(Priority::User) + link_.queued(Priority::Config);
  if (held + need > LinkPolicy::kQueueCapacity) {
    log(EventCode::StmQueueFull, kNoValve, static_cast<int32_t>(Cmd::Smotc));
    return;
  }
  bool any = false;
  if (c.hasMotor && buildSetMotorChars(c.motor, r)) any |= enqueue(r, Priority::User);
  if (c.hasLearnMovements && buildSetLearnMovements(c.learnMovements, r)) {
    any |= enqueue(r, Priority::User);
  }
  // v2 only: a v1 STM would ignore it (the web answers 409 before).
  if (c.hasBreakaway && v2 && buildSetBreakaway(c.breakaway, r)) any |= enqueue(r, Priority::User);
  if (any) planner_.requestMotorParams();
}

void StmSession::handleCommand(const StmCommand& c, uint32_t nowMs) {
  RequestLine r;
  const uint8_t proto = planner_.protocol();
  // An STM below the minimum version gets targets only; other commands would
  // go unanswered (the web answers 409 before).
  if (tooOld()) {
    switch (c.type) {
      case StmCommandType::SetTarget:
      case StmCommandType::ResetStm:
      case StmCommandType::StartFlash:
      case StmCommandType::AbortFlash:
      case StmCommandType::ConfigChanged:
        break;
      default:
        if (c.type == StmCommandType::Calibrate && c.scheduled) {
          port_.postScheduledCalibResult(c.attempt, false, CalibFailure::Unsupported);
        }
        return;
    }
  }
  switch (c.type) {
    case StmCommandType::SetTarget:
      if (!model_.setDesiredTarget(c.valve, c.pos, c.source, nowMs)) {
        log(EventCode::MqttCommandRejected, kNoValve, c.valve < kValveCount ? c.valve + 1 : 0, 0,
            "rejected by model");
      }
      dirty_ = true;
      break;
    case StmCommandType::Calibrate:
      calibrate(c, nowMs);
      break;
    case StmCommandType::Assembly:
      // No stgtp afterwards: it would end the STM's assembly hold.
      if (buildAssembly(c.valve, r) && enqueue(r, Priority::User)) model_.setAssembly(c.valve, nowMs);
      break;
    case StmCommandType::Detect:
      if (buildDetect(r)) enqueue(r, Priority::User);
      break;
    case StmCommandType::ScanSensors:
      if (buildScanOneWire(r) && enqueue(r, Priority::User)) {
        sensors_.clear();
        planner_.requestTempList();
        planner_.requestVoltList();
        planner_.requestValveSensors();
        startSensorGrace(nowMs);
      }
      break;
    case StmCommandType::SetValveSensors:
      if (buildSetValveSensors(c.valve, c.ids[0], c.ids[1], r) && enqueue(r, Priority::User)) {
        if (buildMatchSensors(r)) enqueue(r, Priority::User);
        planner_.requestValveSensors();
      }
      break;
    case StmCommandType::SetMotorSettings:
      setMotorSettings(c);
      break;
    case StmCommandType::ServiceMove:
      if (proto >= 2 && buildServiceMove(c.valve, c.dir, c.counts, c.maxmA, r)) {
        enqueue(r, Priority::User);
      }
      break;
    case StmCommandType::RequestProfile:
      planner_.requestProfile(c.valve);
      break;
    case StmCommandType::ResetStm:
      if (flashing()) {
        // The flasher owns NRST; a pulse now would corrupt the flash run.
        log(EventCode::StmFlashFailed, kNoValve, 0, 0, "reset refused");
        break;
      }
      if (gateAction_ != GateAction::None) break;  // a reset or flash waits already
      gateAnswered_ = stmAnswers(nowMs);
      gate_.begin(nowMs, gateAnswered_);
      gateAction_ = GateAction::StmReset;
      break;
    case StmCommandType::StartFlash:
      requestFlash(c, nowMs);
      break;
    case StmCommandType::AbortFlash:
      if (flashing()) {
        flasher_.abort();
      } else if (gateAction_ == GateAction::Flash) {
        gate_.reset();
        gateAction_ = GateAction::None;
        flashPending_ = false;
        dirty_ = true;
      }
      break;
    case StmCommandType::ConfigChanged:
      break;  // the glue re-reads the config on every revision change
    case StmCommandType::StopValve:
      if (proto >= 3 && buildStop(c.valve, r)) enqueue(r, Priority::User);
      break;
    case StmCommandType::LeaveSafeMode:
      if (proto >= 3 && buildLeaveSafeMode(r)) enqueue(r, Priority::User);
      break;
  }
}

// Early checks at once; the flash itself waits for the STM EEPROM (normal
// mode) or starts at once (blank mode: the STM is in the ROM bootloader).
void StmSession::requestFlash(const StmCommand& c, uint32_t nowMs) {
  if (flashing() || gateAction_ != GateAction::None) {
    log(EventCode::StmFlashFailed, kNoValve, 0, 0, "busy");
    return;
  }
  // An ESP restart would cut the flash run (STM half-erased, in reset or in
  // 8E1). A restart is due at the earliest 1 s after it was requested, and
  // the flag is raised right after this check, so the restart either made
  // us refuse or sees the flag.
  if (port_.restartPending()) {
    log(EventCode::StmFlashFailed, kNoValve, 0, 0, "restart pending");
    return;
  }
  port_.markFlashActive();  // not only with the next snapshot (<= 100 ms)
  flashPending_ = true;
  dirty_ = true;
  if (c.blank) {
    beginFlash(c, nowMs);
    return;
  }
  pendingFlash_ = c;
  gateAnswered_ = stmAnswers(nowMs);
  gate_.begin(nowMs, gateAnswered_);
  gateAction_ = GateAction::Flash;
}

void StmSession::beginFlash(const StmCommand& c, uint32_t nowMs) {
  flashPending_ = false;
  dirty_ = true;  // a refused start below publishes "idle" again
  FlashImage* img = port_.openImage(c.image);
  if (img == nullptr) {
    log(EventCode::StmFlashFailed, kNoValve, static_cast<int32_t>(FlashError::ImageRead), 0,
        c.image);
    return;
  }
  FlashOptions opt;
  opt.blank = c.blank;
  opt.force = c.force;
  // The running STM's tag wins over the user's choice.
  if (snap_.version.hw[0] != '\0') {
    memcpy(opt.boardHw, snap_.version.hw, sizeof opt.boardHw);
  } else if (boardTagValid(c.board)) {
    memcpy(opt.boardHw, c.board, sizeof opt.boardHw);
  }
  opt.boardHw[sizeof opt.boardHw - 1] = '\0';
  link_.suspend();
  if (!flasher_.begin(*img, opt, nowMs)) {
    port_.closeImage();
    link_.resume(nowMs);
    log(EventCode::StmFlashFailed, kNoValve, 0, 0, "start refused");
    return;
  }
  lines_.reset();
  copyString(snap_.flashImage, sizeof snap_.flashImage, c.image);
  snap_.flash = flasher_.status();
  log(EventCode::StmFlashStarted, kNoValve, static_cast<int32_t>(img->size()), 0, c.image);
}

void StmSession::serviceGate(uint32_t nowMs) {
  if (gateAction_ == GateAction::None) return;
  const ResetGate::State st = gate_.update(nowMs);
  if (st == ResetGate::State::Waiting) return;
  const GateAction action = gateAction_;
  const uint32_t waited = gate_.waitedMs(nowMs);
  gateAction_ = GateAction::None;
  gate_.reset();
  if (st == ResetGate::State::TimedOut) {
    log(EventCode::StmEepromWaitTimeout, kNoValve, static_cast<int32_t>(waited),
        static_cast<int32_t>(action));
  }
  switch (action) {
    case GateAction::StmReset: {
      const uint32_t after = port_.pulseReset(nowMs);
      log(EventCode::StmResetByUser);
      afterStmReset(after, false);
      break;
    }
    case GateAction::Flash:
      beginFlash(pendingFlash_, nowMs);
      break;
    case GateAction::EspRestart:
      port_.setStmSaveState(st == ResetGate::State::TimedOut
                                ? StmSaveState::TimedOut
                                : (gateAnswered_ ? StmSaveState::Saved : StmSaveState::Unavailable));
      break;
    case GateAction::None:
      break;
  }
}

// ---------------------------------------------------------------- replies

void StmSession::onVersion(const Reply& rep) {
  snap_.version = rep.version;
  snap_.build = rep.build;
  snap_.compatible = compareVersion(rep.version, minVersion_) >= 0;
  planner_.onVersion(rep.version);
  snap_.support = planner_.support();
  snap_.proto = planner_.protocol();
  // Unsupported: the STM data shown so far is not trustworthy.
  if (snap_.support == StmSupport::TooOld && prevSupport_ != StmSupport::TooOld) {
    model_.forgetStmData();
    // Targets are still delivered (stgtp/gtgtp exist on every 1.x): read them back.
    for (uint8_t v = 0; v < kValveCount; ++v) {
      if ((model_.activeMask() >> v) & 1u) planner_.requestTarget(v);
    }
  }
  prevSupport_ = snap_.support;
  char ver[32];
  if (formatVersion(rep.version, ver, sizeof ver) == 0) return;
  if (strcmp(ver, loggedVersion_) != 0) {
    copyString(loggedVersion_, sizeof loggedVersion_, ver);
    incompatibleLogged_ = false;
    log(EventCode::StmVersion, kNoValve, planner_.protocol(), snap_.hwId, ver);
  }
  if (!snap_.compatible && !incompatibleLogged_) {
    incompatibleLogged_ = true;
    log(EventCode::StmIncompatible, kNoValve, 0, 0, ver);
  }
}

void StmSession::onStatus(const StmStatus& s, uint32_t nowMs) {
  snap_.status = s;
  snap_.haveStatus = true;
  // A reboot also clears the kept previous status.
  const uint8_t cause = reboot_.onStatus(s, nowMs);
  if (cause != 0) onRebootDetected(nowMs, cause);
  if (!s.v3) return;
  Event ev[kMaxEventsPerUpdate];
  logEvents(ev, health_.onStmStatus(havePrevStatus_ ? &prevStatus_ : nullptr, s, nowMs, ev,
                                    kMaxEventsPerUpdate));
  lease_.onStatus(s, nowMs);
  sensors_.setStmTempAge(s.tempAgeS, nowMs);
  prevStatus_ = s;
  havePrevStatus_ = true;
}

// Applies reply data. `req` is the matched request or nullptr for a stray
// line (then only self-identifying replies are applied).
void StmSession::applyReply(const Reply& rep, const RequestLine* req, uint32_t nowMs) {
  dirty_ = true;
  switch (rep.cmd) {
    case Cmd::Gvlvd:
      model_.applyValveData(rep.valveData, nowMs);
      if (reboot_.onValveData(rep.valveData)) onRebootDetected(nowMs, 3);
      break;
    case Cmd::Gvlvx:
    case Cmd::Gvlvy:
      model_.applyValveEx(rep.valveEx, nowMs);
      break;
    case Cmd::Gvlst:
      if (req) model_.applyValveStates(rep.valveStates, nowMs);
      break;
    case Cmd::Gtgtp:
      model_.applyTarget(rep.target, nowMs);
      break;
    case Cmd::Gonec:
      if (req && sensors_.applyTempList(rep.oneWireList, nowMs) && !rep.oneWireList.hasList) {
        planner_.requestTempList();
      }
      planner_.setSensorCounts(sensors_.tempCount(), sensors_.voltCount());
      break;
    case Cmd::Gowvc:
      if (req && sensors_.applyVoltList(rep.oneWireList, nowMs) && !rep.oneWireList.hasList) {
        planner_.requestVoltList();
      }
      planner_.setSensorCounts(sensors_.tempCount(), sensors_.voltCount());
      break;
    case Cmd::Goned:
      // The reply carries no bus index: the matched request names it; a
      // late reading lands on the index of its id.
      if (req) {
        sensors_.applyTempData(static_cast<uint8_t>(req->arg), rep.tempData, nowMs);
      } else if (!sensors_.applyStrayTempData(rep.tempData, nowMs) && rep.tempData.valid) {
        planner_.requestTempList();
      }
      break;
    case Cmd::Gowvd:
      if (req) {
        sensors_.applyVoltData(static_cast<uint8_t>(req->arg), rep.voltData, nowMs);
      } else if (!sensors_.applyStrayVoltData(rep.voltData, nowMs) && rep.voltData.valid) {
        planner_.requestVoltList();
      }
      break;
    case Cmd::Gvlon:
      if (req && !rep.gvlonError) {
        model_.applyValveSensors(rep.valveSensors, slotIds_, kTempSlotCount);
      }
      break;
    case Cmd::Gmotc:
      snap_.motor = rep.motorChars;
      snap_.haveMotor = true;
      model_.setMinCounts(rep.motorChars.minCounts);
      health_.setMinCounts(rep.motorChars.minCounts);
      break;
    case Cmd::Gtlnm:
      snap_.learnMovements = rep.learnMovements;
      break;
    case Cmd::Gcalx:
      snap_.breakaway = rep.breakaway;
      snap_.haveBreakaway = true;
      break;
    case Cmd::Gvers:
      onVersion(rep);
      break;
    case Cmd::Ghwin:
      snap_.hwId = rep.hwId;
      break;
    case Cmd::Gproto:
      planner_.setProtocol(rep.proto);
      snap_.proto = planner_.protocol();
      break;
    case Cmd::Gstat:
    case Cmd::Gstax:
      onStatus(rep.status, nowMs);
      break;
    case Cmd::Gprof:
      if (rep.profile.valve < kValveCount) snap_.profiles[rep.profile.valve] = rep.profile;
      break;
    default:
      break;
  }
}

void StmSession::onServiceMoveResult(const Completion& c, const Reply* rep, uint32_t nowMs) {
  const uint8_t v = c.request.valve;
  if (v >= kValveCount) return;
  if (c.outcome == Outcome::Ok) {
    moves_[v].active = true;
    moves_[v].moveSeq = model_.valve(v).moveSeq;
    moves_[v].sinceMs = nowMs;
    planner_.requestTarget(v);  // fast read-back of gvlvx
    return;
  }
  // Rejected ("svmov v err n", "svmov -1 err n") or no answer.
  const int32_t code = (c.outcome == Outcome::Rejected && rep != nullptr)
                           ? static_cast<int32_t>(rep->serviceMove.errorCode)
                           : -1;
  port_.logEvent(makeEvent(EventCode::ServiceMoveDone, Severity::Warning, v, -1, code,
                           c.outcome == Outcome::Rejected ? "rejected" : "no reply"));
}

void StmSession::onCompletion(const Completion& c, const Reply* rep, uint32_t nowMs) {
  const bool ok = c.outcome == Outcome::Ok;
  const RequestLine& req = c.request;
  switch (req.cmd) {
    case Cmd::Stgtp:
      if (ok) {
        model_.onTargetAck(req.valve, nowMs);
      } else {
        model_.onTargetTimeout(req.valve, nowMs);
      }
      break;
    case Cmd::Svmov:
      onServiceMoveResult(c, rep, nowMs);
      break;
    case Cmd::Staln:
      if (c.tag == kTagScheduledCalib) {
        port_.postScheduledCalibResult(schedAttempt_, ok, ok ? CalibFailure::None
                                                             : CalibFailure::NoReply);
      }
      break;
    case Cmd::Staop:
      if (ok) {
        model_.onAssemblyAck(req.valve, nowMs);
      } else {
        model_.onAssemblyFailed(req.valve, nowMs);
      }
      break;
    case Cmd::Stons:
      // A legacy STM does not re-match the sensors after its search.
      if (ok && planner_.protocol() < 2) planner_.requestMatchSensors(nowMs);
      break;
    case Cmd::Masns:
      if (ok) planner_.requestValveSensors();
      break;
    case Cmd::Slhbt:
    case Cmd::Slcfg:
    case Cmd::Sfspo:
    case Cmd::Glcfg:
      lease_.onCompletion(req, c.outcome, rep, nowMs);
      break;
    case Cmd::Gtlnt:
    case Cmd::Stlnt:
      learn_.onCompletion(req, c.outcome, rep, nowMs);
      break;
    case Cmd::Eepst:
      gate_.onEepst(c.outcome != Outcome::Timeout, rep != nullptr && rep->eepromIdle, nowMs);
      break;
    case Cmd::Gstat:
    case Cmd::Gstax:
      if (c.outcome == Outcome::Timeout && reboot_.onStatusFailed()) onRebootDetected(nowMs, 4);
      break;
    case Cmd::Sstop:
      if (!ok) break;
      if (req.valve == kAllValves) {
        for (uint8_t v = 0; v < kValveCount; ++v) {
          if ((model_.activeMask() >> v) & 1u) planner_.requestTarget(v);
        }
      } else {
        planner_.requestTarget(req.valve);
      }
      break;
    default:
      break;
  }
  planner_.onResult(req, ok, nowMs);
  snap_.proto = planner_.protocol();
  dirty_ = true;
}

void StmSession::onLine(const char* line, size_t len, uint32_t nowMs) {
  const ParseStatus ps = parseReply(line, len, reply_);
  if (ps != ParseStatus::Ok) {
    link_.onParseError(nowMs);
    return;
  }
  Completion done;
  if (link_.onReply(reply_, nowMs, done)) {
    applyReply(reply_, &done.request, nowMs);
    onCompletion(done, &reply_, nowMs);
  } else {
    applyReply(reply_, nullptr, nowMs);
  }
}

void StmSession::onRx(const char* data, size_t len, uint32_t nowMs) {
  size_t off = 0;
  while (off < len) {
    const size_t used = lines_.feed(data + off, len - off);
    off += used;
    if (lines_.hasLine()) {
      onLine(lines_.line(), lines_.length(), nowMs);
      lines_.release();
    } else if (used == 0) {
      break;  // defensive: the assembler refused bytes without a line
    }
  }
}

// ---------------------------------------------------------------- loop

void StmSession::scheduleRequests(uint32_t nowMs) {
  RequestLine r;
  uint8_t valve = 0, pos = 0;
  if (gate_.state() == ResetGate::State::Waiting) {
    const bool held = link_.queued(Priority::User) + link_.queued(Priority::Config) > 0 ||
                      link_.busyWith(Priority::User) || link_.busyWith(Priority::Config);
    if (gate_.pollDue(nowMs, held) && buildEepromState(r) && enqueue(r, Priority::User)) {
      gate_.onPollSent(nowMs);
    }
  }
  const uint8_t proto = planner_.protocol();
  const bool unsupported = tooOld();
  // Unknown protocol (re-sync) is treated like 1.x.
  model_.setHoldTargetsWhileCalibrating(proto < 2);
  model_.setAssemblyViaStaop(proto >= 2 && !unsupported);
  lease_.setProtocol(proto, nowMs);
  learn_.setProtocol(unsupported ? 0 : proto);
  // Nothing but gproto and gvers before the version is known.
  const ResyncStep step = planner_.resyncStep();
  const bool versionKnown = step != ResyncStep::Proto && step != ResyncStep::Version;
  if (versionKnown && link_.queued(Priority::Config) == 0) {
    if (!unsupported && lease_.next(nowMs, r)) {
      enqueue(r, Priority::Config);
    } else if (!unsupported && learn_.next(nowMs, r)) {
      enqueue(r, Priority::Config);
    } else if (model_.nextAssemblyPush(nowMs, valve)) {
      if (!buildAssembly(valve, r) || !enqueue(r, Priority::User)) {
        model_.onTargetPushDropped(valve, nowMs);
      }
    } else if (model_.nextTargetPush(nowMs, valve, pos)) {
      // A push that cannot be queued is retried after pushRetryMs; it is not
      // logged (enqueue()) because that would repeat every 2 s while the
      // queue stays full, the link counts it in queueFull.
      EnqueueResult res = EnqueueResult::Invalid;
      if (buildSetTarget(valve, pos, r)) res = link_.enqueue(r, Priority::Config);
      if (res != EnqueueResult::Queued && res != EnqueueResult::Coalesced) {
        model_.onTargetPushDropped(valve, nowMs);
      }
    } else if (model_.nextVerify(valve)) {
      planner_.requestTarget(valve);
    }
  }
  for (uint8_t i = 0; i < kValveCount; ++i) planner_.setValveBusy(i, model_.isBusy(i));
  if (link_.queued(Priority::Poll) == 0 && planner_.next(nowMs, r)) {
    expectSensor(r, sensors_);
    enqueue(r, planner_.lastWasResync() ? Priority::Config : Priority::Poll);
  }
}

void StmSession::poll(uint32_t nowMs) {
  Completion done;
  if (link_.poll(nowMs, done)) onCompletion(done, nullptr, nowMs);
  if (link_.shouldResetStm(nowMs)) {
    // No EEPROM wait: the STM has not answered for a minute.
    const LinkStats& st = link_.stats();
    log(EventCode::StmResetByPolicy, kNoValve, st.consecutiveTimeouts,
        static_cast<int32_t>(elapsedMs(nowMs, st.lastReplyMs) / 1000));
    afterStmReset(port_.pulseReset(nowMs), true);
  }
  serviceGate(nowMs);
  if (!flashing()) scheduleRequests(nowMs);
}

void StmSession::flashStep(uint32_t nowMs) {
  const FlashPhase phase = flasher_.step(nowMs);
  snap_.flash = flasher_.status();
  dirty_ = true;
  if (phase != FlashPhase::Done && phase != FlashPhase::Failed) return;
  port_.closeImage();
  transport_.configure(kStmBaud, false);  // the flasher restored 8N1 already; make sure
  const FlashStatus& st = flasher_.status();
  if (phase == FlashPhase::Done) {
    char ver[32] = {0};
    formatVersion(st.appVersion, ver, sizeof ver);
    log(EventCode::StmFlashDone, kNoValve,
        static_cast<int32_t>(elapsedMs(st.finishedMs, st.startedMs)), 0, ver);
    port_.requestLastGoodCopy(snap_.flashImage);
  } else {
    log(EventCode::StmFlashFailed, kNoValve, static_cast<int32_t>(st.error),
        static_cast<int32_t>(st.errorAddress), flashPhaseName(st.errorPhase));
  }
  // The flasher reset the STM: 5 s hold-off, full re-sync, targets re-pushed.
  link_.resume(nowMs);
  resync(nowMs);
}

// Sensor slot failure/recovery edges (once per second).
void StmSession::checkSensors(uint32_t nowMs) {
  if (sensorGrace_) {
    if (elapsedMs(nowMs, sensorGraceFromMs_) < kSensorGraceMs) return;
    sensorGrace_ = false;
  }
  Event ev[kMaxEventsPerUpdate];
  for (uint8_t i = 0; i < kTempSlotCount; ++i) {
    SlotTrack& t = tempTrack_[i];
    const TempSlotConfig& slot = cfg_.temps[i];
    if (!slot.active || isZero(slot.id)) {
      t = SlotTrack{};
      continue;
    }
    const int bus = sensors_.findTemp(slot.id);
    const TempReading& rd = sensors_.temp(bus >= 0 ? static_cast<uint8_t>(bus) : 0xFF);
    const bool valid = bus >= 0 && tempRawValid(rd.raw) &&
                       sensors_.tempFresh(static_cast<uint8_t>(bus), nowMs, kSensorStaleMs);
    const size_t n = health_.onTempSensor(static_cast<uint8_t>(i + 1), t.known, t.valid, valid,
                                          rd.raw, ev, kMaxEventsPerUpdate);
    for (size_t k = 0; k < n; ++k) {
      if (ev[k].code == EventCode::TempSensorFailed) {
        formatOneWireId(slot.id, ev[k].text, sizeof ev[k].text);
      }
      port_.logEvent(ev[k]);
    }
    t.known = true;
    t.valid = valid;
  }
  for (uint8_t i = 0; i < kVoltSlotCount; ++i) {
    SlotTrack& t = voltTrack_[i];
    const VoltSlotConfig& slot = cfg_.volts[i];
    if (!slot.active || isZero(slot.id)) {
      t = SlotTrack{};
      continue;
    }
    const int bus = sensors_.findVolt(slot.id);
    const VoltReading& rd = sensors_.volt(bus >= 0 ? static_cast<uint8_t>(bus) : 0xFF);
    const bool valid = bus >= 0 && rd.seen && vadValid(rd.vad) &&
                       elapsedMs(nowMs, rd.lastSeenMs) <= kSensorStaleMs;
    if (t.known && t.valid && !valid) log(EventCode::VoltSensorFailed, kNoValve, i + 1, rd.vad);
    t.known = true;
    t.valid = valid;
  }
  const uint8_t tc = sensors_.tempCount();
  const uint8_t vc = sensors_.voltCount();
  if (countsKnown_ && tc != lastTempCount_) log(EventCode::SensorCountChanged, kNoValve, tc, 0);
  if (countsKnown_ && vc != lastVoltCount_) log(EventCode::SensorCountChanged, kNoValve, vc, 1);
  countsKnown_ = true;
  lastTempCount_ = tc;
  lastVoltCount_ = vc;
}

void StmSession::everySecond(uint32_t nowMs, const RegulatorInput& regulator, StmSaveState save) {
  Event ev[kMaxEventsPerUpdate];
  // K1: regulator, lease events, the ESP emulation.
  lease_.setRegulator(regulatorCause(regulator), regulator.commandSeq, nowMs);
  logEvents(ev, lease_.tick(nowMs, ev, kMaxEventsPerUpdate));
  model_.setFailsafeDrive(lease_.emulatedMask(nowMs), lease_.config().failsafePct);
  port_.storeLeaseRecord(lease_.snapshot(nowMs));
  const LeaseStatus ls = lease_.status(nowMs);
  if (ls.mode != lastLease_.mode || ls.state != lastLease_.state ||
      ls.failsafeMask != lastLease_.failsafeMask || ls.regulator != lastLease_.regulator ||
      ls.configSynced != lastLease_.configSynced || ls.configFailed != lastLease_.configFailed ||
      ls.configTrusted != lastLease_.configTrusted || ls.timeoutMin != lastLease_.timeoutMin) {
    dirty_ = true;
  }
  lastLease_ = ls;
  // E4: the STM save before an ESP restart.
  if (save == StmSaveState::Waiting && gateAction_ == GateAction::None) {
    gateAnswered_ = stmAnswers(nowMs) && !flashing();
    gate_.begin(nowMs, gateAnswered_);
    gateAction_ = GateAction::EspRestart;
    serviceGate(nowMs);
  }
  // An unsupported STM sends no valve data: no stale flags.
  if (!tooOld()) model_.tick(nowMs);
  const LinkStats& st = link_.stats();
  logEvents(ev, health_.onStmCounters(lines_.overflowCount(), st.parseErrors + lines_.malformedCount(),
                                      0, nowMs, ev, kMaxEventsPerUpdate));
  if (snap_.haveStatus) {
    logEvents(ev, health_.onStmCounters(snap_.status.rxOverflow, snap_.status.parseErrors, 1, nowMs,
                                        ev, kMaxEventsPerUpdate));
  }
  if (!flashing()) checkSensors(nowMs);
  const bool settled =
      link_.state(nowMs) == LinkState::Up && !planner_.resyncActive() && !sensorGrace_;
  // v2+ has no gvlvd: valve temperatures from gvlon + goned (v1: from gvlvd).
  if (planner_.protocol() >= 2) model_.applySensorTemps(sensors_, nowMs, kSensorStaleMs, settled);
  if (settled != snap_.sensorsSettled) {
    snap_.sensorsSettled = settled;
    dirty_ = true;
  }
  for (uint8_t v = 0; v < kValveCount; ++v) {
    PendingMove& m = moves_[v];
    if (!m.active) continue;
    const ValveState& s = model_.valve(v);
    if (s.moveSeq != m.moveSeq) {
      m.active = false;
      log(EventCode::ServiceMoveDone, v, static_cast<int32_t>(s.lastMove.countedCounts),
          static_cast<int32_t>(s.lastMove.stop));
    } else if (elapsedMs(nowMs, m.sinceMs) >= kServiceMoveWaitMs) {
      m.active = false;
    }
  }
  if (scheduledMask_ != 0 && elapsedMs(nowMs, scheduledAtMs_) >= kScheduledCalibWindowMs) {
    scheduledMask_ = 0;
  }
}

void StmSession::publishIfDue(uint32_t nowMs) {
  Event ev[kMaxEventsPerUpdate];
  for (uint8_t i = 0; i < kValveCount; ++i) {
    const ValveState& cur = model_.valve(i);
    if (cur.revision == prev_[i].revision) continue;
    const bool active = (model_.activeMask() >> i) & 1u;
    const size_t n = health_.onValve(i, prev_[i], cur, active, ev, kMaxEventsPerUpdate);
    for (size_t k = 0; k < n; ++k) {
      if (ev[k].code == EventCode::CalibStarted && ((scheduledMask_ >> i) & 1u)) {
        ev[k].arg1 = 1;
        scheduledMask_ = static_cast<uint16_t>(scheduledMask_ & ~(1u << i));
      }
      port_.logEvent(ev[k]);
    }
    prev_[i] = cur;
    dirty_ = true;
  }
  const LinkState ls = link_.state(nowMs);
  if (ls != prevLink_) {
    logEvents(ev, health_.onLink(prevLink_, ls, link_.stats().consecutiveTimeouts, ev,
                                 kMaxEventsPerUpdate));
    // A link recovery is a reboot only when the STM status says so (2/3).
    const RebootDetector::Recovery r = reboot_.onLinkState(prevLink_, ls, planner_.protocol());
    if (r == RebootDetector::Recovery::Reboot) onRebootDetected(nowMs, 4);
    if (r == RebootDetector::Recovery::CheckStatus) planner_.requestStatus();
    prevLink_ = ls;
    dirty_ = true;
  }
  if (model_.desiredRevision() != lastDesiredRev_) {
    lastDesiredRev_ = model_.desiredRevision();
    PersistedTargets t;
    captureTargets(model_, t);
    port_.storeDesiredTargets(t);
  }
  if (!dirty_ || (publishedOnce_ && elapsedMs(nowMs, lastPublishMs_) < kPublishMinMs)) return;
  for (uint8_t i = 0; i < kValveCount; ++i) snap_.valves[i] = model_.valve(i);
  snap_.tempCount = sensors_.tempCount();
  for (uint8_t i = 0; i < kTempSlotCount; ++i) snap_.temps[i] = sensors_.temp(i);
  snap_.voltCount = sensors_.voltCount();
  for (uint8_t i = 0; i < kVoltSlotCount; ++i) snap_.volts[i] = sensors_.volt(i);
  snap_.link = ls;
  snap_.linkStats = link_.stats();
  snap_.lineOverflows = lines_.overflowCount();
  snap_.lineMalformed = lines_.malformedCount();
  snap_.support = planner_.support();
  snap_.lease = lease_.status(nowMs);
  snap_.haveLearnTime = learn_.haveStmValue();
  snap_.learnTimeS = learn_.stmValue();
  snap_.flashPending = flashPending_;
  snap_.takenMs = nowMs;
  ++snap_.revision;
  port_.publish(snap_);
  lastPublishMs_ = nowMs;
  publishedOnce_ = true;
  dirty_ = false;
}

}  // namespace vdm
