// Reply formatters of the protocol 3 commands (see PROTOCOL_V2.md). Like the v2
// replies: one line of space separated integers without a trailing space, the
// caller appends CR LF, each formatter writes all or nothing. The ok/err
// replies (slcfg, sfspo, sstop, ssafe, slhbt err) use formatResult() and
// formatIndexedResult() of vdm/replies_v2.h. A later firmware may append
// values to these replies; gvlvx and gstat never change.
#pragma once

#include <stddef.h>
#include <stdint.h>

#include "vdm/buf_writer.h"
#include "vdm/legacy_layout.h"
#include "vdm/replies_v2.h"

namespace vdm {

struct ValveExtV3Reply {
  ValveExtReply base;   // values 1..19, the same as gvlvx
  uint16_t flags;       // kVlvFlag* (vdm/valve_codes.h)
  uint8_t fault;        // ValveFault
  uint8_t failsafePct;  // 0..100, kFailsafeHold
  uint8_t drive;        // drive target 0..100
  uint32_t retryS;      // seconds to the next automatic retry, 0 = none
  uint8_t retries;      // automatic retries since the fault began
};

// "gvlvy" + 25 numbers of up to 11 characters, separated by spaces.
constexpr size_t kValveExtV3ReplyMaxLen = 5 + 25 * 12;
bool formatValveExtV3(BufWriter& out, const ValveExtV3Reply& r);

struct StatV3Reply {
  StatReply base;            // values 1..6, the same as gstat
  uint8_t lease;             // LeaseState
  uint32_t leaseRemainS;     // while running, else 0
  uint8_t leaseClient;       // 1 while a lease command arrived within kLeaseClientIdleS
  uint16_t leaseTimeoutMin;  // 0, 5..1440
  uint16_t failsafeMask;     // bit v: valve v is at its failsafe position because the lease expired
  uint8_t safeMode;
  uint8_t wdgResets;         // watchdog resets in the current window
  uint32_t uartOre;          // USART errors and dropped bytes since start-up
  uint32_t uartFe;
  uint32_t uartNe;
  uint32_t rxDropped;
  uint8_t cfgFlags;          // kCfg* of the last EEPROM load (vdm/config_store.h)
  uint32_t cfgEvents;        // EEPROM loads since start-up that repaired or defaulted a block
  uint32_t eepWrites;        // successful EEPROM write steps since start-up
  uint32_t tempAgeS;         // seconds since the last complete temperature cycle
  uint32_t owScanAgeS;       // seconds since the last 1-Wire enumeration
  uint8_t sysFlags;          // kSysFlag* (vdm/valve_codes.h)
};

// "gstax" + 23 numbers of up to 11 characters, separated by spaces.
constexpr size_t kStatV3ReplyMaxLen = 5 + 23 * 12;
bool formatStatV3(BufWriter& out, const StatV3Reply& r);

// "slhbt <lease> <remainS>"
bool formatHeartbeat(BufWriter& out, uint8_t lease, uint32_t remainS);

// "glcfg <timeoutMin> <fs0> ... <fs11>"
bool formatLeaseConfig(BufWriter& out, uint16_t timeoutMin, const uint8_t (&failsafePct)[kValveCount]);

// "gtlnt <seconds>"
bool formatLearnTime(BufWriter& out, uint32_t seconds);

}  // namespace vdm
