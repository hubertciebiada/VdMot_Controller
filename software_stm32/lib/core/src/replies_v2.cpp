#include "vdm/replies_v2.h"

#include "vdm/motor_params.h"

namespace vdm {

ReplyLine::ReplyLine(BufWriter& out, const char* cmd) : out_(out), start_(out.length()), ok_(out.append(cmd)) {}

ReplyLine& ReplyLine::u(uint32_t v) {
  ok_ = ok_ && out_.append(' ') && out_.appendUnsigned(v);
  return *this;
}

ReplyLine& ReplyLine::s(int32_t v) {
  ok_ = ok_ && out_.append(' ') && out_.appendSigned(v);
  return *this;
}

ReplyLine& ReplyLine::text(const char* t) {
  ok_ = ok_ && out_.append(' ') && out_.append(t);
  return *this;
}

bool ReplyLine::done() {
  if (!ok_) out_.truncate(start_);
  return ok_;
}

uint8_t composeCalState(bool running, bool requested, bool earlyWarn, bool lastFailed) {
  uint8_t v = running ? kCalStateRunning : (requested ? kCalStateRequested : kCalStateIdle);
  if (earlyWarn) v |= kCalFlagEarlyStop;
  if (lastFailed) v |= kCalFlagLastFailed;
  return v;
}

uint8_t encodeValveStatus(uint8_t status, bool calibration) {
  return calibration ? static_cast<uint8_t>(status | kStatusCalibrationBit) : status;
}

uint8_t eepstSaved(uint8_t eepState) { return eepState == kEepStateOk ? 1 : 0; }

ReplyLine& appendValveExtFields(ReplyLine& line, const ValveExtReply& r) {
  return line.u(r.index)
      .u(r.status)
      .u(r.position)
      .u(r.target)
      .u(r.meanCurrent)
      .u(r.openingCount)
      .u(r.closingCount)
      .s(r.deadzoneCount)
      .u(r.calibRetries)
      .u(r.movements)
      .u(r.calState)
      .u(r.earlyStops)
      .u(r.cmdRejected)
      .u(r.last.dir)
      .u(r.last.requestedCounts)
      .u(r.last.countedCounts)
      .u(r.last.stopReason)
      .u(r.last.peakCurrent)
      .u(r.last.durationMs);
}

bool formatValveExt(BufWriter& out, const ValveExtReply& r) {
  ReplyLine line(out, "gvlvx");
  return appendValveExtFields(line, r).done();
}

bool formatProfile(BufWriter& out, uint8_t index, const ProfileRecorder& profile) {
  const size_t start = out.length();
  bool ok = ReplyLine(out, "gprof").u(index).u(profile.size()).done();
  for (uint8_t i = 0; ok && i < profile.size(); ++i) {
    const ProfileSample p = profile.at(i);
    ok = out.append(' ') && out.appendUnsigned(p.count) && out.append(':') && out.appendUnsigned(p.current);
  }
  if (!ok) out.truncate(start);
  return ok;
}

ReplyLine& appendStatFields(ReplyLine& line, const StatReply& r) {
  return line.u(r.uptimeSeconds).u(r.resets).u(r.bootReason).u(r.rxOverflow).u(r.parseErrors).u(r.eepromState);
}

bool formatStat(BufWriter& out, const StatReply& r) {
  ReplyLine line(out, "gstat");
  return appendStatFields(line, r).done();
}

bool formatEscalation(BufWriter& out, const EscalationConfig& c) {
  return ReplyLine(out, "gcalx").u(c.enable).u(c.stepPct).u(c.maxmA).done();
}

bool formatMotorLimits(BufWriter& out) {
  return ReplyLine(out, "gmotx")
      .u(kLowFacRange.min)
      .u(kLowFacRange.max)
      .u(kHighFacRange.min)
      .u(kHighFacRange.max)
      .u(kStartOnPowerRange.min)
      .u(kStartOnPowerRange.max)
      .u(kMinCountsRange.min)
      .u(kMinCountsRange.max)
      .u(kMaxRetriesRange.min)
      .u(kMaxRetriesRange.max)
      .done();
}

bool formatProtocolVersion(BufWriter& out) { return ReplyLine(out, "gproto").u(kProtocolVersion).done(); }

bool formatResult(BufWriter& out, const char* cmd, bool ok) {
  return ReplyLine(out, cmd).text(ok ? "ok" : "err").done();
}

bool formatIndexedResult(BufWriter& out, const char* cmd, int32_t index, uint8_t errorCode) {
  ReplyLine line(out, cmd);
  line.s(index);
  if (errorCode == 0) return line.text("ok").done();
  return line.text("err").u(errorCode).done();
}

}  // namespace vdm
