// Application wiring: task layout, the command queue into the STM task and
// the shared STM snapshot. See DESIGN.md "Task model" (binding).
#pragma once

#include <stddef.h>
#include <stdint.h>

#include <vdm/common.h>
#include <vdm/link_policy.h>
#include <vdm/stm_codec.h>
#include <vdm/stm_flasher.h>
#include <vdm/valve_model.h>
#include <vdm/version.h>

namespace app {

// ---------------------------------------------------------------- tasks

struct TaskSpec {
  const char* name;
  uint32_t stackBytes;
  unsigned priority;
  int core;
};
// Binding task table (DESIGN.md). AsyncTCP's own task runs on core 0.
constexpr TaskSpec kStmTask{"stm", 6144, 5, 1};
constexpr TaskSpec kAppTask{"app", 6144, 3, 1};
constexpr TaskSpec kMqttTask{"mqtt", 8192, 2, 1};
// Task watchdog: every task above subscribes and must feed it at least this
// often; the ESP panics (and reboots, reason TASK_WDT) otherwise.
constexpr uint32_t kTaskWdtTimeoutS = 30;

// Called once from setup(): factory-reset pin, storage, config, logger,
// network, tasks, web server. Never returns early: a failing subsystem is
// logged and the rest keeps running.
void setup();

// Uptime helpers used by all glue modules.
uint32_t nowMs();       // millis()
uint32_t uptimeS();

// ---------------------------------------------------------------- commands

// Commands into the STM task (web and MQTT never touch the UART or the
// model directly). Validation happens before submit(): the STM task only
// re-checks invariants the core builders enforce anyway.
enum class CommandType : uint8_t {
  SetTarget,          // valve, pos, source
  Calibrate,          // valve or kAllValves
  Assembly,           // valve or kAllValves
  Detect,             // stdet 255
  ScanSensors,        // stons, then lists
  SetValveSensors,    // valve, ids[2]; followed by masns + gvlon 255
  SetMotorChars,      // motor
  SetLearnMovements,  // learnMovements
  SetBreakaway,       // breakaway (v2)
  ServiceMove,        // valve, dir, counts, maxmA (v2)
  RequestProfile,     // valve (v2)
  ResetStm,           // NRST pulse by the user
  StartFlash,         // image, blank, force
  AbortFlash,
  ConfigChanged,      // active mask / sensor slots changed: re-read config
};

struct Command {
  CommandType type = CommandType::ConfigChanged;
  uint8_t valve = vdm::kNoValve;
  uint8_t pos = 0;
  vdm::TargetSource source = vdm::TargetSource::None;
  vdm::OneWireId ids[2];
  vdm::MotorChars motor;
  uint16_t learnMovements = 0;
  vdm::Breakaway breakaway;
  vdm::MoveDir dir = vdm::MoveDir::Open;
  uint16_t counts = 0;
  uint8_t maxmA = 0;
  char image[32] = {0};   // LittleFS name below /stm/
  bool blank = false;
  bool force = false;
  bool scheduled = false;  // Calibrate: fired by the calibration schedule
};

// Depth of the FreeRTOS queue into the STM task.
constexpr size_t kCommandQueueDepth = 16;
// Non-blocking; false when the queue is full (caller answers 503 / logs).
bool submit(const Command& cmd);
// Free entries in the queue (a handler that submits several commands checks
// this first so a request is applied completely or not at all).
size_t queueSpace();
// STM task side: next command, false when none.
bool receive(Command& out);

// ---------------------------------------------------------------- snapshot

// Everything other tasks may know about the STM, published by the STM task
// after every change (copy under a mutex, never references into the model).
struct StmSnapshot {
  uint32_t revision = 0;
  uint32_t takenMs = 0;
  vdm::ValveState valves[vdm::kValveCount];
  vdm::TempReading temps[vdm::kTempSlotCount];
  uint8_t tempCount = 0;
  vdm::VoltReading volts[vdm::kVoltSlotCount];
  uint8_t voltCount = 0;
  vdm::LinkState link = vdm::LinkState::Unknown;
  vdm::LinkStats linkStats;
  uint8_t proto = 0;
  vdm::Version version;
  uint32_t build = 0;
  uint16_t hwId = 0;
  bool compatible = true;
  bool haveMotor = false;
  vdm::MotorChars motor;
  uint16_t learnMovements = 0;
  bool haveBreakaway = false;
  vdm::Breakaway breakaway;
  bool haveStatus = false;
  vdm::StmStatus status;
  uint32_t lineOverflows = 0;
  uint32_t lineMalformed = 0;
  vdm::FlashStatus flash;
  char flashImage[32] = {0};
  vdm::Profile profiles[vdm::kValveCount];  // last gprof per valve (count 0 = none)
};

// Copies the latest snapshot (~7 KB: callers keep `out` in static storage,
// never on a task stack).
void readStmSnapshot(StmSnapshot& out);
// Cheap accessors (no snapshot copy).
vdm::LinkState stmLinkState();
bool stmFlashActive();   // flasher running (UART owned by it)
uint32_t stmSnapshotRevision();  // changes whenever a new snapshot is published
uint8_t stmProtocol();   // 0 unknown, 1, 2
// STM task only.
void publishStmSnapshot(const StmSnapshot& in);

// ---------------------------------------------------------------- calibration

// Scheduled calibration state for /api/status (written by the app task).
struct CalibInfo {
  int64_t lastScheduledEpoch = 0;  // vdmrev/lastCal
  uint32_t nextSlot = 0;           // yyyymmdd, 0 = none / no valid time
};
CalibInfo calibInfo();

}  // namespace app
