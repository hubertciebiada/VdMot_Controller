// StmSession test rig: a fake port, the scripted line-level STM (LineStm), the
// flash transport simulator and a loop that drives the session like the
// stm_link task. Header-only, shared by the test_stm_session*.cpp files.
#pragma once

#include <stdint.h>
#include <string.h>

#include <algorithm>
#include <deque>
#include <string>
#include <utility>
#include <vector>

#include "line_stm.h"
#include "sim_stm.h"
#include "vdm/stm_session.h"

using namespace vdm;
using vdm_test::LineStm;
using vdm_test::SimStm;

namespace {

class MemImage : public FlashImage {
 public:
  std::vector<uint8_t> data;
  uint32_t size() const override { return static_cast<uint32_t>(data.size()); }
  bool read(uint32_t offset, uint8_t* out, size_t len) override {
    if (offset > data.size() || len > data.size() - offset) return false;
    memcpy(out, data.data() + offset, len);
    return true;
  }
};

struct TestPort : StmSessionPort {
  LineStm* stm = nullptr;
  std::vector<Event> events;
  StmSnapshot last;
  int publishes = 0;
  std::vector<uint32_t> pulses;
  std::vector<std::string> lastGood;
  bool restart = false;
  int flashMarks = 0;
  std::vector<PersistedTargets> targets;
  struct Calib {
    uint16_t attempt;
    bool ok;
    CalibFailure reason;
  };
  std::vector<Calib> calibs;
  std::vector<StmSaveState> saves;
  LeaseClient::Snapshot lease;
  int leaseRecords = 0;
  MemImage image;
  bool imageOk = true;
  std::vector<std::string> opened;
  int closed = 0;

  void logEvent(const Event& e) override { events.push_back(e); }
  uint32_t pulseReset(uint32_t nowMs) override {
    pulses.push_back(nowMs);
    if (stm) stm->reset(nowMs + 100);
    return nowMs + 100;
  }
  void publish(const StmSnapshot& s) override {
    last = s;
    ++publishes;
    publishTimes.push_back(s.takenMs);
  }
  std::vector<uint32_t> publishTimes;
  void requestLastGoodCopy(const char* image) override { lastGood.push_back(image); }
  bool restartPending() override { return restart; }
  void markFlashActive() override { ++flashMarks; }
  void storeDesiredTargets(const PersistedTargets& t) override { targets.push_back(t); }
  void postScheduledCalibResult(uint16_t attempt, bool ok, CalibFailure reason) override {
    calibs.push_back({attempt, ok, reason});
  }
  void setStmSaveState(StmSaveState s) override { saves.push_back(s); }
  void storeLeaseRecord(const LeaseClient::Snapshot& s) override {
    lease = s;
    ++leaseRecords;
  }
  FlashImage* openImage(const char* name) override {
    opened.push_back(name);
    return imageOk ? &image : nullptr;
  }
  void closeImage() override { ++closed; }

  std::vector<Event> withCode(EventCode c) const {
    std::vector<Event> out;
    for (const Event& e : events) {
      if (e.code == c) out.push_back(e);
    }
    return out;
  }
  bool has(EventCode c) const { return !withCode(c).empty(); }
};

struct Rig {
  uint32_t now = 1000;
  SimStm sim{now};
  TestPort port;
  LineStm stm;
  StmSnapshot snapshot;
  StmSession s{port, sim, snapshot};
  Config cfg;
  bool trusted = true;
  RegulatorInput reg;
  StmSaveState save = StmSaveState::Idle;
  std::deque<std::pair<uint32_t, std::string>> replies;
  uint32_t lastSecond = 0;
  uint32_t replyDelayMs = 3;
  bool viaRx = false;  // replies as UART bytes (onRx, CR LF) instead of onLine()

  explicit Rig(uint8_t protocol = 3, uint16_t active = 0x00F) {
    port.stm = &stm;
    stm.protocol = protocol;
    setDefaults(cfg);
    for (uint8_t v = 0; v < kValveCount; ++v) cfg.valves[v].active = ((active >> v) & 1u) != 0;
  }
  void start(const PersistedTargets& t = PersistedTargets{},
             RestoreSource src = RestoreSource::None, const LeaseClient::Snapshot* lease = nullptr) {
    s.applyConfig(cfg, trusted);
    s.begin(now, t, src, lease);
    lastSecond = now;
  }
  void command(const StmCommand& c) { s.handleCommand(c, now); }
  void step() {
    now += 2;
    while (!replies.empty() && replies.front().first <= now) {
      const std::string r = replies.front().second;
      replies.pop_front();
      if (viaRx) {
        const std::string bytes = r + "\r\n";
        s.onRx(bytes.c_str(), bytes.size(), now);
      } else {
        s.onLine(r.c_str(), r.size(), now);
      }
    }
    if (s.flashing()) {
      s.flashStep(now);
    } else {
      s.poll(now);
      if (const RequestLine* line = s.nextToSend(now)) {
        std::string text(line->text, line->len);
        while (!text.empty() && (text.back() == '\n' || text.back() == '\r' || text.back() == ' ')) {
          text.pop_back();
        }
        s.onSent(now);
        const std::string r = stm.answer(text, now);
        if (!r.empty()) replies.emplace_back(now + replyDelayMs, r);
      }
    }
    if (now - lastSecond >= 1000) {
      lastSecond = now;
      s.everySecond(now, reg, save);
    }
    s.publishIfDue(now);
  }
  void run(uint32_t ms) {
    const uint32_t end = now + ms;
    while (static_cast<int32_t>(end - now) > 0) step();
  }
  // Runs until `cond` or `ms` passed; returns cond().
  template <typename F>
  bool runUntil(F cond, uint32_t ms) {
    const uint32_t end = now + ms;
    while (!cond() && static_cast<int32_t>(end - now) > 0) step();
    return cond();
  }
  size_t count(const std::string& cmd) const { return stm.linesOf(cmd).size(); }
  StmCommand target(uint8_t v, uint8_t pos, TargetSource src = TargetSource::Web) const {
    StmCommand c;
    c.type = StmCommandType::SetTarget;
    c.valve = v;
    c.pos = pos;
    c.source = src;
    return c;
  }
};

inline StmCommand cmd(StmCommandType t, uint8_t valve = kNoValve) {
  StmCommand c;
  c.type = t;
  c.valve = valve;
  return c;
}

}  // namespace
