// The configuration as the firmware keeps it in RAM, the rules that build it
// from the EEPROM blocks (at start-up and after a failed read), and the rules
// that merge it with the changes made in RAM meanwhile. Hardware-free.
#pragma once

#include <stdint.h>

#include "vdm/calibration.h"
#include "vdm/config_blocks.h"
#include "vdm/eeprom_layout.h"
#include "vdm/legacy_layout.h"

namespace vdm {

// The 1.x layout plus blocks A (escalation, learn time, lease timeout), B
// (failsafe positions) and C (calibration records).
struct ConfigImage : LegacyLayout {
  EscalationConfig escalation;
  uint32_t learnTimeS;               // 0 = time trigger off
  uint16_t leaseTimeoutMin;          // 0 = off, 5..1440
  uint8_t failsafePct[kValveCount];  // 0..100, kFailsafeHold
  CalibRecord calib[kValveCount];    // flags 0: no record
};

// cfgFlags (gstax field 18): what the last load found
constexpr uint8_t kCfgLayoutCrc = 0x01;        // the 1.x layout failed its CRC: motor fields and learn movements from block B
constexpr uint8_t kCfgShadowMissing = 0x02;    // ... and block B was unusable: those fields are defaults
constexpr uint8_t kCfgSettingsCorrupt = 0x04;  // block A damaged: escalation, learn time and lease timeout are defaults
constexpr uint8_t kCfgSafetyCorrupt = 0x08;    // block B damaged or lost: failsafe positions are 50
constexpr uint8_t kCfgSensorSlot = 0x10;       // a sensor slot failed its ROM CRC and was cleared
constexpr uint8_t kCfgCalib = 0x20;            // a calibration record was damaged: that valve calibrates again
constexpr uint8_t kCfgUnverified = 0x40;       // no layout CRC yet (first start after 1.x or 2.0.0): a migration write follows
constexpr uint8_t kCfgReadFailed = 0x80;       // the EEPROM could not be read

// Fields changed in RAM (include/eeprom.h EEP_CHANGED_*): they select the
// blocks to write (blocksFor) and survive a re-read (mergeChanges).
constexpr uint16_t kChangedSensors = 0x0001;     // owsensors1/2, per slot in ChangeSet::slots
constexpr uint16_t kChangedMovements = 0x0002;   // numberOfMovements
constexpr uint16_t kChangedMotor = 0x0004;       // factors, startOnPower, noOfMinCounts, maxCalibRetries
constexpr uint16_t kChangedEscalation = 0x0008;  // escalation
constexpr uint16_t kChangedLearnTime = 0x0010;   // learnTimeS
constexpr uint16_t kChangedLease = 0x0020;       // leaseTimeoutMin
constexpr uint16_t kChangedFailsafe = 0x0040;    // failsafePct
constexpr uint16_t kChangedCalib = 0x0080;       // calib, per valve in ChangeSet::calib
constexpr uint16_t kChangedAll = 0x00FF;

// The bytes read from the EEPROM; a block that could not be read is 0xFF.
struct RawImages {
  uint8_t layout[kLegacyImageSize];
  uint8_t settings[kExtensionBlockSize];
  uint8_t safety[kSafetyBlockSize];
  uint8_t calib[kValveCount][kCalibBlockSize];
  bool readFailed;  // the read gave up (too many failed transfers)
};

// LoadResult::leaseSource: where the lease timeout came from
constexpr uint8_t kLeaseSourceSettings = 0;  // block A
constexpr uint8_t kLeaseSourceSafety = 1;    // the copy in block B
constexpr uint8_t kLeaseSourceDefault = 2;   // neither: kLeaseTimeoutDefaultMin (after a warm reset
                                             // the glue takes the copy kept in RAM instead)

struct LoadResult {
  ConfigImage image;
  uint8_t cfgFlags;
  uint16_t rewrite;  // kChanged* fields to write back (0 after a failed read)
  uint8_t leaseSource;
};

// Builds the configuration from the blocks read at start-up or at a re-read:
// - block A with a layout CRC: a 1.x layout that does not match it takes the
//   motor fields and learn movements from the shadow in block B (defaults if
//   B is unusable). Without a layout CRC (1.x or 2.0.0 image, or A damaged)
//   the layout is used as it is and gets one;
// - sensor slots owsensors1/2 that fail their ROM CRC are cleared;
// - failsafe positions from block B, else 50; lease timeout from block A,
//   else from the copy in B, else kLeaseTimeoutDefaultMin; learn time and
//   escalation from A, else their defaults; calibration records from C;
// - every repair or migration is written back (`rewrite`);
// - after a failed read the layout and A are used as read, the lease timeout
//   and failsafe positions are the defaults (60, 50; the glue prefers the copy
//   kept in RAM after a warm reset), cfgFlags is kCfgReadFailed and nothing is
//   written back.
void resolveConfig(const RawImages& raw, LoadResult& out);

// True for the flags of a load that repaired or defaulted a checked block
// (gstax cfgEvents counts such loads).
bool repairsConfig(uint8_t cfgFlags);

struct ChangeSet {
  uint16_t fields;  // kChanged*
  uint32_t slots;   // bit s: sensor slot s changed (0..11 owsensors1, 12..23 owsensors2)
  uint16_t calib;   // bit v: calibration record of valve v changed
};

// Copies the changes made in RAM into the configuration read again from the
// EEPROM: sensor slots and calibration records one by one, the other fields
// by group.
void mergeChanges(ConfigImage& stored, const ConfigImage& ram, const ChangeSet& changes);

enum StoreBlock : uint8_t {
  kBlockLayout = 0x01,
  kBlockSettings = 0x02,  // block A
  kBlockSafety = 0x04,    // block B
  kBlockCalib = 0x08,     // the blocks C of the valves in ChangeSet::calib
};

// The blocks that hold the fields: the 1.x fields are written with block B
// (shadow) and block A (layout CRC); the lease timeout with A and B (copy).
uint8_t blocksFor(uint16_t fields);

}  // namespace vdm
