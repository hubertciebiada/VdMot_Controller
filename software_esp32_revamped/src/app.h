// Application wiring: task layout, the command queue into the STM task and
// the shared STM snapshot. See DESIGN.md "Task model" (binding).
#pragma once

#include <stddef.h>
#include <stdint.h>

#include <vdm/common.h>
#include <vdm/link_policy.h>
#include <vdm/stm_codec.h>
#include <vdm/stm_flasher.h>
#include <vdm/stm_types.h>
#include <vdm/valve_model.h>
#include <vdm/version.h>

namespace vdm {
struct HealthSnapshot;  // vdm/json_api.h
}  // namespace vdm

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
constexpr TaskSpec kAppTask{"app", 8192, 3, 1};
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

// Commands into the STM task (vdm/stm_types.h).
using CommandType = vdm::StmCommandType;
using Command = vdm::StmCommand;

// Depth of the FreeRTOS queue into the STM task.
constexpr size_t kCommandQueueDepth = 16;
// Non-blocking; false when the queue is full (caller answers 503 / logs).
// A request that needs several STM commands is one Command (see
// SetMotorSettings): web, mqtt and app submit concurrently, so free space
// checked before several submits would not stay free.
bool submit(const Command& cmd);
// STM task side: next command, false when none.
bool receive(Command& out);

// ---------------------------------------------------------------- snapshot

// The STM snapshot published by the STM task (vdm/stm_types.h).
using StmSnapshot = vdm::StmSnapshot;

// Copies the latest snapshot (~7 KB: callers keep `out` in static storage,
// never on a task stack).
void readStmSnapshot(StmSnapshot& out);
// Cheap accessors (no snapshot copy).
vdm::LinkState stmLinkState();
bool stmFlashActive();   // flasher running (UART owned by it)
// STM task, right after the flasher started (the snapshot follows later).
void markStmFlashActive();
uint32_t stmSnapshotRevision();  // changes whenever a new snapshot is published
uint8_t stmProtocol();   // 0 unknown, 1..3
// Support level of the running STM firmware (StmSnapshot::support).
vdm::StmSupport stmSupport();
// STM task only.
void publishStmSnapshot(const StmSnapshot& in);

// ---------------------------------------------------------------- restart

// The STM EEPROM save before an ESP restart: a restart asks for it
// (Waiting), the STM task reports the outcome.
void requestStmSave();
vdm::StmSaveState stmSaveState();
void setStmSaveState(vdm::StmSaveState s);

// ---------------------------------------------------------------- calibration

// Scheduled calibration state for /api/status (written by the app task,
// stm_service).
struct CalibInfo {
  int64_t lastScheduledEpoch = 0;  // vdmrev/lastCal
  uint32_t nextSlot = 0;           // yyyymmdd, 0 = none / no valid time
  int64_t nextEpoch = 0;           // the next slot at calib.hour:calib.minute local time, 0 = none
};
CalibInfo calibInfo();
void setCalibInfo(const CalibInfo& c);

// ---------------------------------------------------------------- health

// Everything GET /api/health shows (version, uptime, heap, tasks, network,
// OTA, log).
void readHealth(vdm::HealthSnapshot& out);

}  // namespace app
