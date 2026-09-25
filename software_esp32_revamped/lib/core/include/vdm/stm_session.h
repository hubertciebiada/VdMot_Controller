// The STM link as one hardware-free object: runs LinkPolicy, PollPlanner,
// ValveModel, SensorModel, RebootDetector, HealthMonitor, LeaseClient,
// LearnTimeSync, ResetGate and the StmFlasher, executes the commands of the
// other tasks and builds the STM snapshot. The stm_link glue owns the UART,
// the NRST pin and the task loop and implements StmSessionPort.
#pragma once

#include <stddef.h>
#include <stdint.h>

#include "vdm/calib_schedule.h"
#include "vdm/config.h"
#include "vdm/event_log.h"
#include "vdm/failsafe.h"
#include "vdm/health_monitor.h"
#include "vdm/lease_client.h"
#include "vdm/line_assembler.h"
#include "vdm/link_policy.h"
#include "vdm/poll_planner.h"
#include "vdm/reset_gate.h"
#include "vdm/stm_codec.h"
#include "vdm/stm_flasher.h"
#include "vdm/stm_types.h"
#include "vdm/target_store.h"
#include "vdm/valve_model.h"

namespace vdm {

// What the session needs from the firmware around it.
struct StmSessionPort {
  virtual ~StmSessionPort() = default;
  virtual void logEvent(const Event& e) = 0;
  // Pulses NRST (100 ms); returns the time after the pulse.
  virtual uint32_t pulseReset(uint32_t nowMs) = 0;
  virtual void publish(const StmSnapshot& s) = 0;
  // The flashed image becomes the last good one.
  virtual void requestLastGoodCopy(const char* image) = 0;
  // An ESP restart is due (a flash would be cut).
  virtual bool restartPending() = 0;
  // The flash is starting: other tasks see it before the next snapshot.
  virtual void markFlashActive() = 0;
  // Desired targets changed: RTC copy and the hand-over to the NVS saver.
  virtual void storeDesiredTargets(const PersistedTargets& t) = 0;
  virtual void postScheduledCalibResult(uint16_t attempt, bool ok, CalibFailure reason) = 0;
  virtual void setStmSaveState(StmSaveState s) = 0;
  // Once per second: the lease emulation for the RTC record.
  virtual void storeLeaseRecord(const LeaseClient::Snapshot& s) = 0;
  // The image of a flash run (nullptr when it cannot be opened); closed
  // when the run ends or does not start.
  virtual FlashImage* openImage(const char* name) = 0;
  virtual void closeImage() = 0;
};

class StmSession {
 public:
  static constexpr uint32_t kSensorStaleMs = 60000;   // DESIGN.md: temps stale after 60 s
  static constexpr uint32_t kSensorGraceMs = 30000;   // after a bus scan / STM reset: lists re-read
  static constexpr uint32_t kServiceMoveWaitMs = 300000;  // svmov accepted -> lastMove expected
  static constexpr uint32_t kScheduledCalibWindowMs = 4UL * 3600UL * 1000UL;
  static constexpr uint32_t kPublishMinMs = 100;
  static constexpr uint16_t kTagScheduledCalib = 1;
  static constexpr uint32_t kStmBaud = 115200;

  StmSession(StmSessionPort& port, FlashTransport& transport);
  StmSession(const StmSession&) = delete;
  StmSession& operator=(const StmSession&) = delete;

  // Config (initially and whenever it changed). trusted false: the ESP runs
  // on defaults nobody saved (no lease config push).
  void applyConfig(const Config& cfg, bool trusted);
  // ESP boot, after the first applyConfig(): restores the desired targets
  // (TargetsRestored) and the lease record, starts the re-sync behind the
  // STM start-up hold-off (the IO15 strap may have reset it).
  void begin(uint32_t nowMs, const PersistedTargets& targets, RestoreSource src,
             const LeaseClient::Snapshot* lease);
  void handleCommand(const StmCommand& c, uint32_t nowMs);
  // Bytes read from the UART (not while flashing).
  void onRx(const char* data, size_t len, uint32_t nowMs);
  // One complete reply line.
  void onLine(const char* line, size_t len, uint32_t nowMs);
  // Timeouts, the policy reset, the reset gate and request scheduling.
  void poll(uint32_t nowMs);
  // The next request line for the UART; onSent() right after writing it.
  const RequestLine* nextToSend(uint32_t nowMs) { return link_.nextToSend(nowMs); }
  void onSent(uint32_t nowMs) { link_.onSent(nowMs); }
  // Once per second: regulator, lease, emulation, counters, sensors,
  // service moves, the STM save before an ESP restart.
  void everySecond(uint32_t nowMs, const RegulatorInput& regulator, StmSaveState save);
  // Valve/link events, desired-target hand-over, snapshot (at most every
  // kPublishMinMs, only after a change).
  void publishIfDue(uint32_t nowMs);
  bool flashing() const { return flasher_.active(); }
  // One flasher step while flashing() (instead of onRx/poll/nextToSend).
  void flashStep(uint32_t nowMs);

  // For tests.
  const ValveModel& model() const { return model_; }
  const SensorModel& sensors() const { return sensors_; }
  const LinkPolicy& link() const { return link_; }
  const PollPlanner& planner() const { return planner_; }
  const LeaseClient& lease() const { return lease_; }
  const StmSnapshot& snapshot() const { return snap_; }
  bool flashPending() const { return flashPending_; }

 private:
  enum class GateAction : uint8_t { None = 0, StmReset = 1, Flash = 2, EspRestart = 3 };
  struct PendingMove {
    bool active = false;
    uint32_t moveSeq = 0;
    uint32_t sinceMs = 0;
  };
  struct SlotTrack {
    bool known = false;
    bool valid = false;
  };

  void log(EventCode code, uint8_t valve = kNoValve, int32_t a1 = 0, int32_t a2 = 0,
           const char* text = nullptr);
  void logEvents(const Event* ev, size_t n);
  bool enqueue(const RequestLine& r, Priority p, uint16_t tag = 0);
  bool stmAnswers(uint32_t nowMs) const;
  bool tooOld() const { return planner_.support() == StmSupport::TooOld; }
  void startSensorGrace(uint32_t nowMs);
  void newStmSession(uint32_t nowMs);
  void resync(uint32_t nowMs);
  void afterStmReset(uint32_t nowMs, bool byPolicy);
  void onRebootDetected(uint32_t nowMs, int32_t cause);
  void calibrate(const StmCommand& c, uint32_t nowMs);
  void setMotorSettings(const StmCommand& c);
  void requestFlash(const StmCommand& c, uint32_t nowMs);
  void beginFlash(const StmCommand& c, uint32_t nowMs);
  void serviceGate(uint32_t nowMs);
  void onVersion(const Reply& rep);
  void onStatus(const StmStatus& s, uint32_t nowMs);
  void applyReply(const Reply& rep, const RequestLine* req, uint32_t nowMs);
  void onServiceMoveResult(const Completion& c, const Reply* rep, uint32_t nowMs);
  void onCompletion(const Completion& c, const Reply* rep, uint32_t nowMs);
  void scheduleRequests(uint32_t nowMs);
  void checkSensors(uint32_t nowMs);

  StmSessionPort& port_;
  FlashTransport& transport_;
  StaticLineAssembler<kStmMaxLineLen + 1> lines_;
  LinkPolicy link_;
  PollPlanner planner_;
  ValveModel model_;
  SensorModel sensors_;
  RebootDetector reboot_;
  HealthMonitor health_;
  LeaseClient lease_;
  LearnTimeSync learn_;
  ResetGate gate_;
  StmFlasher flasher_;
  Reply reply_;

  Config cfg_;
  OneWireId slotIds_[kTempSlotCount];
  StmSnapshot snap_;
  ValveState prev_[kValveCount];
  LinkState prevLink_ = LinkState::Unknown;
  bool dirty_ = true;
  bool publishedOnce_ = false;
  uint32_t lastPublishMs_ = 0;
  Version minVersion_;
  char loggedVersion_[32] = {0};
  bool incompatibleLogged_ = false;
  StmSupport prevSupport_ = StmSupport::Unknown;
  bool havePrevStatus_ = false;
  StmStatus prevStatus_;
  uint32_t lastDesiredRev_ = 0;
  LeaseStatus lastLease_;

  uint16_t scheduledMask_ = 0;   // CalibStarted of these valves carries arg1 = 1
  uint32_t scheduledAtMs_ = 0;
  uint16_t schedAttempt_ = 0;    // attempt of the scheduled staln in the queue
  PendingMove moves_[kValveCount];

  SlotTrack tempTrack_[kTempSlotCount];
  SlotTrack voltTrack_[kVoltSlotCount];
  uint32_t sensorGraceFromMs_ = 0;
  bool sensorGrace_ = true;
  uint8_t lastTempCount_ = 0;
  uint8_t lastVoltCount_ = 0;
  bool countsKnown_ = false;

  GateAction gateAction_ = GateAction::None;
  bool gateAnswered_ = false;    // the gate began with an answering STM
  StmCommand pendingFlash_;
  bool flashPending_ = false;
};

}  // namespace vdm
