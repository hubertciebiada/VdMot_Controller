#include "vdm/config_store.h"

#include <string.h>

#include "vdm/failsafe.h"
#include "vdm/lease.h"

namespace vdm {

namespace {

// the 1.x layout is written together with block B (shadow) and block A (layout CRC)
constexpr uint16_t kRewriteLayout = kChangedSensors | kChangedMovements | kChangedMotor;

void applyShadow(ConfigImage& img, const MotorShadow& s) {
  img.currentbound_low_fac = s.lowFac;
  img.currentbound_high_fac = s.highFac;
  img.numberOfMovements = s.movements;
  img.startOnPower = s.startOnPower;
  img.noOfMinCounts = s.minCounts;
  img.maxCalibRetries = s.maxRetries;
}

// true if the slot failed its ROM CRC and was cleared
bool clearIfInvalid(SensorSlot& s) {
  if (sensorSlotValid(s)) return false;
  memset(&s, 0, sizeof(s));
  return true;
}

}  // namespace

void resolveConfig(const RawImages& raw, LoadResult& out) {
  ConfigImage& img = out.image;
  decodeLegacyLayout(raw.layout, img);

  StoredExtension a;
  const ExtensionState aState = decodeExtension(raw.settings, a);
  img.escalation = a.escalation;
  img.learnTimeS = a.learnTimeS;

  SafetyBlock b;
  const BlockState bState = decodeSafety(raw.safety, b);
  memcpy(img.failsafePct, b.failsafePct, sizeof(img.failsafePct));

  if (a.hasV3) {
    img.leaseTimeoutMin = a.leaseTimeoutMin;
    out.leaseSource = kLeaseSourceSettings;
  } else if (b.leaseValid) {
    img.leaseTimeoutMin = b.leaseTimeoutMin;
    out.leaseSource = kLeaseSourceSafety;
  } else {
    img.leaseTimeoutMin = kLeaseTimeoutDefaultMin;
    out.leaseSource = kLeaseSourceDefault;
  }

  bool calibCorrupt = false;
  for (uint8_t v = 0; v < kValveCount; ++v) {
    if (decodeCalib(raw.calib[v], v, img.calib[v]) == BlockState::Corrupt) calibCorrupt = true;
  }

  out.rewrite = 0;
  if (raw.readFailed) {
    // what could be read is used as it is and nothing is written back; the
    // lease timeout and the failsafe positions are the defaults
    img.leaseTimeoutMin = kLeaseTimeoutDefaultMin;
    out.leaseSource = kLeaseSourceDefault;
    memset(img.failsafePct, kFailsafeDefaultPct, sizeof(img.failsafePct));
    out.cfgFlags = kCfgReadFailed;
    return;
  }

  uint8_t flags = 0;
  if (a.hasV3) {
    if (crc16Ccitt(raw.layout, sizeof(raw.layout)) != a.layoutCrc) {
      // torn write or a 1.x downgrade: the motor fields as this firmware last wrote them
      // (decodeSafety() leaves the defaults in an unusable block B)
      applyShadow(img, b.shadow);
      flags |= bState == BlockState::Valid ? kCfgLayoutCrc : (kCfgLayoutCrc | kCfgShadowMissing);
      out.rewrite |= kRewriteLayout;
    }
  } else {
    // 1.x or 2.0.0 image, or block A damaged: the layout as read, then stored with a CRC
    flags |= aState == ExtensionState::Corrupt ? kCfgSettingsCorrupt : kCfgUnverified;
    out.rewrite |= kRewriteLayout;
  }

  bool slotCleared = false;
  for (SensorSlot& s : img.owsensors1) {
    if (clearIfInvalid(s)) slotCleared = true;
  }
  for (SensorSlot& s : img.owsensors2) {
    if (clearIfInvalid(s)) slotCleared = true;
  }
  if (slotCleared) {
    flags |= kCfgSensorSlot;
    out.rewrite |= kRewriteLayout;
  }

  // a missing block B is expected on the first start after 1.x or 2.0.0 only
  if (bState == BlockState::Corrupt || (bState == BlockState::Absent && a.hasV3)) flags |= kCfgSafetyCorrupt;
  if (bState != BlockState::Valid) out.rewrite |= kChangedFailsafe;

  if (calibCorrupt) flags |= kCfgCalib;
  out.cfgFlags = flags;
}

bool repairsConfig(uint8_t cfgFlags) {
  return (cfgFlags & (kCfgLayoutCrc | kCfgShadowMissing | kCfgSettingsCorrupt | kCfgSafetyCorrupt |
                      kCfgSensorSlot | kCfgCalib)) != 0;
}

void mergeChanges(ConfigImage& stored, const ConfigImage& ram, const ChangeSet& changes) {
  for (uint8_t v = 0; v < kValveCount; ++v) {
    if (changes.slots & (1ul << v)) stored.owsensors1[v] = ram.owsensors1[v];
    if (changes.slots & (1ul << (v + kValveCount))) stored.owsensors2[v] = ram.owsensors2[v];
    if (changes.calib & (1u << v)) stored.calib[v] = ram.calib[v];
  }
  if (changes.fields & kChangedMovements) stored.numberOfMovements = ram.numberOfMovements;
  if (changes.fields & kChangedMotor) {
    stored.currentbound_low_fac = ram.currentbound_low_fac;
    stored.currentbound_high_fac = ram.currentbound_high_fac;
    stored.startOnPower = ram.startOnPower;
    stored.noOfMinCounts = ram.noOfMinCounts;
    stored.maxCalibRetries = ram.maxCalibRetries;
  }
  if (changes.fields & kChangedEscalation) stored.escalation = ram.escalation;
  if (changes.fields & kChangedLearnTime) stored.learnTimeS = ram.learnTimeS;
  if (changes.fields & kChangedLease) stored.leaseTimeoutMin = ram.leaseTimeoutMin;
  if (changes.fields & kChangedFailsafe) memcpy(stored.failsafePct, ram.failsafePct, sizeof(stored.failsafePct));
}

uint8_t blocksFor(uint16_t fields) {
  uint8_t blocks = 0;
  if (fields & (kChangedSensors | kChangedMovements | kChangedMotor)) blocks |= kBlockLayout | kBlockSafety | kBlockSettings;
  if (fields & (kChangedEscalation | kChangedLearnTime)) blocks |= kBlockSettings;
  if (fields & kChangedLease) blocks |= kBlockSettings | kBlockSafety;
  if (fields & kChangedFailsafe) blocks |= kBlockSafety;
  if (fields & kChangedCalib) blocks |= kBlockCalib;
  return blocks;
}

}  // namespace vdm
