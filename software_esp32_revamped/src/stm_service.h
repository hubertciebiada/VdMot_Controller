// App-task side of the STM link: the scheduled calibration (DESIGN.md
// "Calibration schedule", confirmed by the STM), the NVS copy of the desired
// targets and the RTC records that survive ESP software restarts. Talks to
// the STM task through app::submit() and the hand-over functions below.
#pragma once

#include <stdint.h>

#include <vdm/calib_schedule.h>
#include <vdm/lease_client.h>
#include <vdm/target_store.h>

namespace stm_service {

// app::setup, after the config was loaded: restores the scheduled-calibration
// state (last booked slot, last calibration time), chooses the boot copy of
// the desired targets (RTC, else NVS) and reads the RTC lease record.
void begin();

// App task, every second: scheduled calibration (evaluated every 10 s, its
// STM results at once) and the desired-target NVS saver.
void service(uint32_t nowMs);

// App task, before an ESP restart (not for a factory reset): writes what
// must survive the restart to NVS now.
void flushForRestart();

// ---- hand-over from the STM task (any task may call them)

// Desired targets changed: the RTC copy at once, the NVS copy by service().
void storeDesiredTargets(const vdm::PersistedTargets& t);
// The ESP failsafe emulation for the next software restart (RTC).
void storeLeaseRecord(const vdm::LeaseClient::Snapshot& s);
// Result of the scheduled staln of `attempt` (StmCommand::attempt).
void postScheduledCalibResult(uint16_t attempt, bool ok, vdm::CalibFailure reason);

// ---- boot state chosen by begin() (STM task start)

const vdm::PersistedTargets& bootTargets(vdm::RestoreSource& src);
bool bootLease(vdm::LeaseClient::Snapshot& out);  // false: no valid RTC record

}  // namespace stm_service
