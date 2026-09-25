// Types shared between the STM task and the other tasks: the commands into
// the STM task and the STM snapshot it publishes (DESIGN.md "Task model").
// Hardware-free.
#pragma once

#include <stddef.h>
#include <stdint.h>

#include "vdm/common.h"
#include "vdm/link_policy.h"
#include "vdm/stm_codec.h"
#include "vdm/stm_flasher.h"
#include "vdm/valve_model.h"
#include "vdm/version.h"

namespace vdm {

// ---------------------------------------------------------------- commands

// Commands into the STM task (web and MQTT never touch the UART or the
// model directly). Validation happens before submit(): the STM task only
// re-checks invariants the core builders enforce anyway.
enum class StmCommandType : uint8_t {
  SetTarget,          // valve, pos, source
  Calibrate,          // valve or kAllValves
  Assembly,           // valve or kAllValves
  Detect,             // stdet 255
  ScanSensors,        // stons, then lists
  SetValveSensors,    // valve, ids[2]; followed by masns + gvlon 255
  SetMotorSettings,   // motor / learnMovements / breakaway (v2), each if its has* flag
  ServiceMove,        // valve, dir, counts, maxmA (v2)
  RequestProfile,     // valve (v2)
  ResetStm,           // NRST pulse by the user
  StartFlash,         // image, blank, force
  AbortFlash,
  ConfigChanged,      // active mask / sensor slots changed: re-read config
};

struct StmCommand {
  StmCommandType type = StmCommandType::ConfigChanged;
  uint8_t valve = kNoValve;
  uint8_t pos = 0;
  TargetSource source = TargetSource::None;
  OneWireId ids[2];
  // SetMotorSettings: one command, so a request is queued completely or not
  // at all even while other tasks submit concurrently.
  bool hasMotor = false;
  MotorChars motor;
  bool hasLearnMovements = false;
  uint16_t learnMovements = 0;
  bool hasBreakaway = false;
  Breakaway breakaway;
  MoveDir dir = MoveDir::Open;
  uint16_t counts = 0;
  uint8_t maxmA = 0;
  char image[32] = {0};   // LittleFS name below /stm/
  bool blank = false;
  bool force = false;
  bool scheduled = false;  // Calibrate: fired by the calibration schedule
};

// ---------------------------------------------------------------- snapshot

// Everything other tasks may know about the STM, published by the STM task
// after every change (copy under a mutex, never references into the model).
struct StmSnapshot {
  uint32_t revision = 0;
  uint32_t takenMs = 0;
  ValveState valves[kValveCount];
  TempReading temps[kTempSlotCount];
  uint8_t tempCount = 0;
  VoltReading volts[kVoltSlotCount];
  uint8_t voltCount = 0;
  LinkState link = LinkState::Unknown;
  LinkStats linkStats;
  uint8_t proto = 0;
  Version version;
  uint32_t build = 0;
  uint16_t hwId = 0;
  bool compatible = true;
  bool haveMotor = false;
  MotorChars motor;
  uint16_t learnMovements = 0;
  bool haveBreakaway = false;
  Breakaway breakaway;
  bool haveStatus = false;
  StmStatus status;
  // Link Up, re-sync finished and the 30 s sensor grace over: valve sensor
  // assignments and temperatures reflect the STM (HA first-run cleanup).
  bool sensorsSettled = false;
  uint32_t lineOverflows = 0;
  uint32_t lineMalformed = 0;
  FlashStatus flash;
  char flashImage[32] = {0};
  Profile profiles[kValveCount];  // last gprof per valve (count 0 = none)
};

}  // namespace vdm
