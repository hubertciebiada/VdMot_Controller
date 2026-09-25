#include <stdint.h>
#include <string.h>

#include "doctest.h"
#include "vdm/config_store.h"
#include "vdm/onewire_check.h"

using vdm::ConfigImage;
using vdm::LoadResult;
using vdm::RawImages;
using vdm::SensorSlot;

namespace {

// offsets in the 1.x image
constexpr size_t kLowFacOffset = 29;
constexpr size_t kSlotsOffset = 33;  // owsensors1[0]

// a DS18B20-like address with a valid CRC, different for every n
SensorSlot validSlot(uint8_t n) {
  uint8_t address[8] = {0x28, static_cast<uint8_t>(0x10 + n), 0x4C, static_cast<uint8_t>(n * 7), 0x61, 0x16, 0x04, 0};
  address[7] = vdm::crc8(address, 7);
  SensorSlot s;
  memcpy(&s, address, sizeof(s));
  return s;
}

SensorSlot erasedSlot() {
  SensorSlot s;
  memset(&s, 0xFF, sizeof(s));
  return s;
}

// what this firmware writes: a consistent set of blocks
struct Stored {
  vdm::LegacyLayout layout;
  vdm::StoredExtension a;
  vdm::SafetyBlock b;
  vdm::CalibRecord calib[vdm::kValveCount];  // flags 0: block never written
};

Stored sampleStored() {
  Stored s{};
  vdm::LegacyLayout& l = s.layout;
  l.b_slave = 0;
  strcpy(l.descr, "VdMot");
  l.OneWireCfg[0] = 20;
  l.currentbound_low_fac = 21;
  l.currentbound_high_fac = 27;
  l.numberOfMovements = 1500;
  for (uint8_t v = 0; v < vdm::kValveCount; ++v) {
    l.owsensors1[v] = v < 8 ? validSlot(v) : erasedSlot();
    l.owsensors2[v] = v < 4 ? validSlot(static_cast<uint8_t>(20 + v)) : erasedSlot();
  }
  memset(&l.owsensors2[4], 0, sizeof(SensorSlot));  // a cleared assignment
  for (SensorSlot& x : l.owsensors) x = erasedSlot();
  l.startOnPower = 40;
  l.noOfMinCounts = 2500;
  l.maxCalibRetries = 1;

  s.a.escalation = vdm::EscalationConfig{1, 30, 55};
  s.a.learnTimeS = 86400;
  s.a.leaseTimeoutMin = 30;
  s.a.hasV3 = true;

  const uint8_t fs[12] = {0, 10, 20, 30, 40, 50, 60, 70, 80, 90, 100, 255};
  memcpy(s.b.failsafePct, fs, sizeof(fs));
  s.b.shadow = vdm::MotorShadow{21, 27, 1500, 40, 2500, 1};
  s.b.leaseTimeoutMin = 30;

  s.calib[0] = vdm::CalibRecord{3567, 3610, 17, vdm::kCalibValid};
  s.calib[3] = vdm::CalibRecord{2800, 2900, 25, static_cast<uint8_t>(vdm::kCalibValid | vdm::kCalibFailed)};
  s.calib[11] = vdm::CalibRecord{0, 0, 20, vdm::kCalibFailed};
  return s;
}

// the blocks as read from the EEPROM; `layoutCrc` of block A is the CRC of the layout
// as `s` has it, the layout bytes may be changed afterwards
void writeRaw(const Stored& s, RawImages& raw) {
  memset(&raw, 0xFF, sizeof(raw));
  raw.readFailed = false;
  vdm::encodeLegacyLayout(s.layout, raw.layout);
  vdm::StoredExtension a = s.a;
  a.layoutCrc = vdm::crc16Ccitt(raw.layout, sizeof(raw.layout));
  vdm::encodeExtension(a, raw.settings);
  vdm::encodeSafety(s.b, raw.safety);
  for (uint8_t v = 0; v < vdm::kValveCount; ++v) {
    if (s.calib[v].flags != 0) vdm::encodeCalib(s.calib[v], v, raw.calib[v]);
  }
}

// block A as firmware 2.0.0 wrote it (version 2: escalation only)
void writeSettingsV2(RawImages& raw, uint8_t enable, uint8_t stepPct, uint8_t maxmA) {
  memset(raw.settings, 0xFF, sizeof(raw.settings));
  raw.settings[0] = 2;
  raw.settings[1] = 3;
  raw.settings[2] = enable;
  raw.settings[3] = stepPct;
  raw.settings[4] = maxmA;
  raw.settings[5] = vdm::crc8(raw.settings, 5);
}

bool sameSlot(const SensorSlot& x, const SensorSlot& y) { return memcmp(&x, &y, sizeof(x)) == 0; }

bool isZeroSlot(const SensorSlot& x) {
  SensorSlot zero;
  memset(&zero, 0, sizeof(zero));
  return sameSlot(x, zero);
}

bool sameCalib(const vdm::CalibRecord& x, const vdm::CalibRecord& y) {
  return x.openingCount == y.openingCount && x.closingCount == y.closingCount && x.meanCurrent == y.meanCurrent &&
         x.flags == y.flags;
}

bool noRecord(const vdm::CalibRecord& x) { return sameCalib(x, vdm::CalibRecord{0, 0, 0, 0}); }

bool sameMotorFields(const ConfigImage& img, uint8_t low, uint8_t high, uint16_t movements, uint8_t startOnPower,
                     uint16_t minCounts, uint8_t maxRetries) {
  return img.currentbound_low_fac == low && img.currentbound_high_fac == high && img.numberOfMovements == movements &&
         img.startOnPower == startOnPower && img.noOfMinCounts == minCounts && img.maxCalibRetries == maxRetries;
}

bool allFailsafe(const ConfigImage& img, uint8_t pct) {
  for (uint8_t p : img.failsafePct) {
    if (p != pct) return false;
  }
  return true;
}

// The whole image as `s` describes it (the fields a load keeps unchanged).
bool matchesStored(const ConfigImage& img, const Stored& s) {
  const vdm::LegacyLayout& l = s.layout;
  if (img.b_slave != l.b_slave || memcmp(img.descr, l.descr, sizeof(l.descr)) != 0 ||
      memcmp(img.OneWireCfg, l.OneWireCfg, sizeof(l.OneWireCfg)) != 0 ||
      !sameMotorFields(img, l.currentbound_low_fac, l.currentbound_high_fac, l.numberOfMovements, l.startOnPower,
                       l.noOfMinCounts, l.maxCalibRetries)) {
    return false;
  }
  for (uint8_t v = 0; v < vdm::kValveCount; ++v) {
    if (!sameSlot(img.owsensors1[v], l.owsensors1[v]) || !sameSlot(img.owsensors2[v], l.owsensors2[v])) return false;
    if (!sameCalib(img.calib[v], s.calib[v])) return false;
  }
  for (uint8_t i = 0; i < vdm::kExtraSensorSlots; ++i) {
    if (!sameSlot(img.owsensors[i], l.owsensors[i])) return false;
  }
  return img.escalation.enable == s.a.escalation.enable && img.escalation.stepPct == s.a.escalation.stepPct &&
         img.escalation.maxmA == s.a.escalation.maxmA && img.learnTimeS == s.a.learnTimeS &&
         img.leaseTimeoutMin == s.a.leaseTimeoutMin && memcmp(img.failsafePct, s.b.failsafePct, 12) == 0;
}

constexpr uint8_t kAllBlocks = vdm::kBlockLayout | vdm::kBlockSettings | vdm::kBlockSafety;

LoadResult resolve(const RawImages& raw) {
  LoadResult r;
  memset(&r, 0xAA, sizeof(r));
  vdm::resolveConfig(raw, r);
  return r;
}

}  // namespace

TEST_CASE("config_store: flag, mask and source values") {
  CHECK(vdm::kCfgLayoutCrc == 0x01);
  CHECK(vdm::kCfgShadowMissing == 0x02);
  CHECK(vdm::kCfgSettingsCorrupt == 0x04);
  CHECK(vdm::kCfgSafetyCorrupt == 0x08);
  CHECK(vdm::kCfgSensorSlot == 0x10);
  CHECK(vdm::kCfgCalib == 0x20);
  CHECK(vdm::kCfgUnverified == 0x40);
  CHECK(vdm::kCfgReadFailed == 0x80);
  CHECK(vdm::kChangedSensors == 0x0001);
  CHECK(vdm::kChangedMovements == 0x0002);
  CHECK(vdm::kChangedMotor == 0x0004);
  CHECK(vdm::kChangedEscalation == 0x0008);
  CHECK(vdm::kChangedLearnTime == 0x0010);
  CHECK(vdm::kChangedLease == 0x0020);
  CHECK(vdm::kChangedFailsafe == 0x0040);
  CHECK(vdm::kChangedCalib == 0x0080);
  CHECK(vdm::kChangedAll == 0x00FF);
  CHECK(vdm::kLeaseSourceSettings == 0);
  CHECK(vdm::kLeaseSourceSafety == 1);
  CHECK(vdm::kLeaseSourceDefault == 2);
}

TEST_CASE("resolveConfig (a): consistent blocks load unchanged, nothing to write") {
  const Stored s = sampleStored();
  RawImages raw;
  writeRaw(s, raw);
  const LoadResult r = resolve(raw);
  CHECK(r.cfgFlags == 0);
  CHECK(r.rewrite == 0);
  CHECK(r.leaseSource == vdm::kLeaseSourceSettings);
  CHECK(matchesStored(r.image, s));
  CHECK(r.image.calib[3].flags == (vdm::kCalibValid | vdm::kCalibFailed));
  CHECK(noRecord(r.image.calib[5]));
}

TEST_CASE("resolveConfig (b): a changed motor field fails the layout CRC, block B has the values") {
  const Stored s = sampleStored();
  RawImages raw;
  writeRaw(s, raw);
  raw.layout[kLowFacOffset] = 35;  // currentbound_low_fac, torn write
  const LoadResult r = resolve(raw);
  CHECK(r.cfgFlags == vdm::kCfgLayoutCrc);
  CHECK(vdm::blocksFor(r.rewrite) == kAllBlocks);
  CHECK(r.leaseSource == vdm::kLeaseSourceSettings);
  CHECK(matchesStored(r.image, s));  // low factor 21 from the shadow, the rest as stored

  // every motor field and the learn movements come from the shadow
  Stored other = s;
  other.b.shadow = vdm::MotorShadow{11, 12, 60000, 13, 14, 2};
  writeRaw(other, raw);
  raw.layout[kLowFacOffset] = 35;
  const LoadResult r2 = resolve(raw);
  CHECK(r2.cfgFlags == vdm::kCfgLayoutCrc);
  CHECK(sameMotorFields(r2.image, 11, 12, 60000, 13, 14, 2));
  CHECK(sameSlot(r2.image.owsensors1[2], s.layout.owsensors1[2]));
  CHECK(memcmp(r2.image.descr, s.layout.descr, sizeof(s.layout.descr)) == 0);
}

TEST_CASE("resolveConfig (c): layout CRC fails and block B is damaged: motor defaults") {
  const Stored s = sampleStored();
  RawImages raw;
  writeRaw(s, raw);
  raw.layout[kLowFacOffset] = 35;
  raw.safety[5] ^= 0x10;
  const LoadResult r = resolve(raw);
  CHECK(r.cfgFlags == (vdm::kCfgLayoutCrc | vdm::kCfgShadowMissing | vdm::kCfgSafetyCorrupt));
  CHECK(sameMotorFields(r.image, 17, 17, 2000, 30, 3000, 2));
  CHECK(allFailsafe(r.image, 50));
  CHECK(vdm::blocksFor(r.rewrite) == kAllBlocks);
  CHECK(r.leaseSource == vdm::kLeaseSourceSettings);
  CHECK(r.image.leaseTimeoutMin == 30);

  // block B never written (A has a layout CRC, so B was lost): the same
  writeRaw(s, raw);
  raw.layout[kLowFacOffset] = 35;
  memset(raw.safety, 0xFF, sizeof(raw.safety));
  const LoadResult r2 = resolve(raw);
  CHECK(r2.cfgFlags == (vdm::kCfgLayoutCrc | vdm::kCfgShadowMissing | vdm::kCfgSafetyCorrupt));
  CHECK(sameMotorFields(r2.image, 17, 17, 2000, 30, 3000, 2));
}

TEST_CASE("resolveConfig (d): a sensor slot with a flipped bit is cleared, the others stay") {
  const Stored s = sampleStored();
  Stored damaged = s;
  damaged.layout.owsensors1[5].romcode[2] ^= 0x04;  // stored like that: the layout CRC matches
  RawImages raw;
  writeRaw(damaged, raw);
  const LoadResult r = resolve(raw);
  CHECK(r.cfgFlags == vdm::kCfgSensorSlot);
  CHECK(vdm::blocksFor(r.rewrite) == kAllBlocks);
  CHECK(isZeroSlot(r.image.owsensors1[5]));
  for (uint8_t v = 0; v < vdm::kValveCount; ++v) {
    if (v != 5) CHECK(sameSlot(r.image.owsensors1[v], s.layout.owsensors1[v]));
    CHECK(sameSlot(r.image.owsensors2[v], s.layout.owsensors2[v]));
  }

  // the same in a second slot of a valve
  damaged = s;
  damaged.layout.owsensors2[1].crc ^= 0x80;
  writeRaw(damaged, raw);
  const LoadResult r2 = resolve(raw);
  CHECK(r2.cfgFlags == vdm::kCfgSensorSlot);
  CHECK(isZeroSlot(r2.image.owsensors2[1]));
  CHECK(sameSlot(r2.image.owsensors1[1], s.layout.owsensors1[1]));

  // a bit flipped after the write fails the layout CRC as well
  writeRaw(s, raw);
  raw.layout[kSlotsOffset + 8 * 3 + 2] ^= 0x01;  // owsensors1[3]
  const LoadResult r3 = resolve(raw);
  CHECK(r3.cfgFlags == (vdm::kCfgLayoutCrc | vdm::kCfgSensorSlot));
  CHECK(isZeroSlot(r3.image.owsensors1[3]));

  // the additional slots are not used and not checked
  damaged = s;
  damaged.layout.owsensors[0] = validSlot(9);
  damaged.layout.owsensors[0].crc ^= 0x01;
  writeRaw(damaged, raw);
  const LoadResult r4 = resolve(raw);
  CHECK(r4.cfgFlags == 0);
  CHECK(sameSlot(r4.image.owsensors[0], damaged.layout.owsensors[0]));
}

TEST_CASE("resolveConfig (e): first start after 2.0.0 keeps the escalation, migrates the layout") {
  Stored s = sampleStored();
  RawImages raw;
  writeRaw(s, raw);
  writeSettingsV2(raw, 1, 10, 20);
  memset(raw.safety, 0xFF, sizeof(raw.safety));
  for (auto& c : raw.calib) memset(c, 0xFF, sizeof(c));
  raw.layout[kLowFacOffset] = 35;  // no layout CRC yet: used as read
  const LoadResult r = resolve(raw);
  CHECK(r.cfgFlags == vdm::kCfgUnverified);
  CHECK(vdm::blocksFor(r.rewrite) == kAllBlocks);
  CHECK(r.image.escalation.enable == 1);
  CHECK(r.image.escalation.stepPct == 10);
  CHECK(r.image.escalation.maxmA == 20);
  CHECK(r.image.learnTimeS == 604800);
  CHECK(r.image.leaseTimeoutMin == 60);
  CHECK(r.leaseSource == vdm::kLeaseSourceDefault);
  CHECK(allFailsafe(r.image, 50));
  CHECK(r.image.currentbound_low_fac == 35);
  CHECK(r.image.numberOfMovements == 1500);
  for (const vdm::CalibRecord& c : r.image.calib) CHECK(noRecord(c));
}

TEST_CASE("resolveConfig (f): a new chip (all 0xFF) loads defaults and gets written once") {
  RawImages raw;
  memset(&raw, 0xFF, sizeof(raw));
  raw.readFailed = false;
  const LoadResult r = resolve(raw);
  CHECK(r.cfgFlags == vdm::kCfgUnverified);
  CHECK(vdm::blocksFor(r.rewrite) == kAllBlocks);
  CHECK(r.image.escalation.enable == vdm::kEscalationDefault.enable);
  CHECK(r.image.escalation.stepPct == vdm::kEscalationDefault.stepPct);
  CHECK(r.image.escalation.maxmA == vdm::kEscalationDefault.maxmA);
  CHECK(r.image.learnTimeS == 604800);
  CHECK(r.image.leaseTimeoutMin == 60);
  CHECK(r.leaseSource == vdm::kLeaseSourceDefault);
  CHECK(allFailsafe(r.image, 50));
  for (const vdm::CalibRecord& c : r.image.calib) CHECK(noRecord(c));
  // the 1.x fields as read: app_load_config() replaces what is out of range
  CHECK(r.image.currentbound_low_fac == 0xFF);
  CHECK(r.image.noOfMinCounts == 0xFFFF);
  CHECK(sameSlot(r.image.owsensors1[0], erasedSlot()));
}

TEST_CASE("resolveConfig (g): after a failed read nothing is repaired or written") {
  const Stored s = sampleStored();
  RawImages raw;
  writeRaw(s, raw);
  raw.readFailed = true;
  LoadResult r = resolve(raw);
  CHECK(r.cfgFlags == vdm::kCfgReadFailed);
  CHECK(r.rewrite == 0);
  // lease timeout and failsafe positions: defaults (the glue prefers the copies in RAM after a warm reset)
  CHECK(r.leaseSource == vdm::kLeaseSourceDefault);
  CHECK(r.image.leaseTimeoutMin == 60);
  CHECK(allFailsafe(r.image, 50));
  // what was read is used
  CHECK(r.image.escalation.stepPct == 30);
  CHECK(r.image.learnTimeS == 86400);
  CHECK(r.image.currentbound_low_fac == 21);
  CHECK(sameCalib(r.image.calib[0], s.calib[0]));

  // unread blocks are 0xFF: no layout check, no slot check, no flags but the read failure
  memset(raw.settings, 0xFF, sizeof(raw.settings));
  memset(raw.safety, 0xFF, sizeof(raw.safety));
  raw.layout[kLowFacOffset] = 35;
  raw.layout[kSlotsOffset + 1] ^= 0x01;
  raw.calib[2][4] ^= 0x01;
  r = resolve(raw);
  CHECK(r.cfgFlags == vdm::kCfgReadFailed);
  CHECK(r.rewrite == 0);
  CHECK(r.image.currentbound_low_fac == 35);
  CHECK_FALSE(isZeroSlot(r.image.owsensors1[0]));
  CHECK(r.image.escalation.stepPct == vdm::kEscalationDefault.stepPct);
  CHECK(r.image.learnTimeS == 604800);
}

TEST_CASE("resolveConfig (h): block B missing next to a block A of this firmware is flagged") {
  const Stored s = sampleStored();
  RawImages raw;
  writeRaw(s, raw);
  memset(raw.safety, 0xFF, sizeof(raw.safety));
  const LoadResult r = resolve(raw);
  CHECK(r.cfgFlags == vdm::kCfgSafetyCorrupt);
  CHECK(r.rewrite == vdm::kChangedFailsafe);
  CHECK(vdm::blocksFor(r.rewrite) == vdm::kBlockSafety);
  CHECK(allFailsafe(r.image, 50));
  CHECK(r.image.leaseTimeoutMin == 30);  // from block A
  CHECK(r.image.currentbound_low_fac == 21);
}

TEST_CASE("resolveConfig (i): a damaged calibration record is dropped and flagged") {
  const Stored s = sampleStored();
  RawImages raw;
  writeRaw(s, raw);
  raw.calib[3][6] ^= 0x20;
  const LoadResult r = resolve(raw);
  CHECK(r.cfgFlags == vdm::kCfgCalib);
  CHECK(r.rewrite == 0);  // written again after the next calibration of the valve
  CHECK(noRecord(r.image.calib[3]));
  CHECK(sameCalib(r.image.calib[0], s.calib[0]));
  CHECK(sameCalib(r.image.calib[11], s.calib[11]));

  // the record of valve 11 in the block of valve 10 (index check)
  writeRaw(s, raw);
  memcpy(raw.calib[10], raw.calib[11], sizeof(raw.calib[10]));
  const LoadResult r2 = resolve(raw);
  CHECK(r2.cfgFlags == vdm::kCfgCalib);
  CHECK(noRecord(r2.image.calib[10]));
  CHECK(sameCalib(r2.image.calib[11], s.calib[11]));
}

TEST_CASE("resolveConfig (j): after a 1.x downgrade, new sensor slots stay and motor fields come from B") {
  const Stored s = sampleStored();
  RawImages raw;
  writeRaw(s, raw);
  // 1.x rewrote the layout: a sensor for valve 9 and another low factor; A and B untouched
  Stored by1x = s;
  by1x.layout.owsensors1[9] = validSlot(42);
  by1x.layout.currentbound_low_fac = 33;
  vdm::encodeLegacyLayout(by1x.layout, raw.layout);
  const LoadResult r = resolve(raw);
  CHECK(r.cfgFlags == vdm::kCfgLayoutCrc);
  CHECK(vdm::blocksFor(r.rewrite) == kAllBlocks);
  CHECK(sameSlot(r.image.owsensors1[9], validSlot(42)));
  CHECK(r.image.currentbound_low_fac == 21);
  CHECK(matchesStored(r.image, by1x) == false);
  Stored expected = s;
  expected.layout.owsensors1[9] = validSlot(42);
  CHECK(matchesStored(r.image, expected));
}

TEST_CASE("resolveConfig: lease timeout from block A, else the copy in B, else 60") {
  const Stored s = sampleStored();
  RawImages raw;

  // A damaged, B valid: the copy in B
  writeRaw(s, raw);
  raw.settings[3] ^= 0x01;
  LoadResult r = resolve(raw);
  CHECK(r.cfgFlags == vdm::kCfgSettingsCorrupt);
  CHECK(r.leaseSource == vdm::kLeaseSourceSafety);
  CHECK(r.image.leaseTimeoutMin == 30);
  CHECK(r.image.learnTimeS == 604800);
  CHECK(r.image.escalation.stepPct == vdm::kEscalationDefault.stepPct);
  CHECK(vdm::blocksFor(r.rewrite) == kAllBlocks);
  CHECK(r.image.failsafePct[1] == 10);

  // 2.1 -> 2.0.0 -> 2.1: A is version 2 again, B and C survived
  writeRaw(s, raw);
  writeSettingsV2(raw, 1, 30, 55);
  r = resolve(raw);
  CHECK(r.cfgFlags == vdm::kCfgUnverified);
  CHECK(r.leaseSource == vdm::kLeaseSourceSafety);
  CHECK(r.image.leaseTimeoutMin == 30);
  CHECK(r.image.failsafePct[10] == 100);
  CHECK(sameCalib(r.image.calib[3], s.calib[3]));

  // A of a 1.x image (erased) and no B: default
  writeRaw(s, raw);
  memset(raw.settings, 0xFF, sizeof(raw.settings));
  memset(raw.safety, 0xFF, sizeof(raw.safety));
  r = resolve(raw);
  CHECK(r.cfgFlags == vdm::kCfgUnverified);
  CHECK(r.leaseSource == vdm::kLeaseSourceDefault);
  CHECK(r.image.leaseTimeoutMin == 60);

  // B valid with an invalid copy: default
  Stored badCopy = s;
  badCopy.b.leaseTimeoutMin = 2000;
  writeRaw(badCopy, raw);
  memset(raw.settings, 0xFF, sizeof(raw.settings));
  r = resolve(raw);
  CHECK(r.leaseSource == vdm::kLeaseSourceDefault);
  CHECK(r.image.leaseTimeoutMin == 60);
  CHECK(r.image.failsafePct[2] == 20);

  // A of this firmware wins over the copy, also "off"
  Stored off = s;
  off.a.leaseTimeoutMin = 0;
  writeRaw(off, raw);
  r = resolve(raw);
  CHECK(r.leaseSource == vdm::kLeaseSourceSettings);
  CHECK(r.image.leaseTimeoutMin == 0);

  // both damaged
  writeRaw(s, raw);
  raw.settings[3] ^= 0x01;
  raw.safety[3] ^= 0x01;
  r = resolve(raw);
  CHECK(r.cfgFlags == (vdm::kCfgSettingsCorrupt | vdm::kCfgSafetyCorrupt));
  CHECK(r.leaseSource == vdm::kLeaseSourceDefault);
  CHECK(r.image.leaseTimeoutMin == 60);
  CHECK(allFailsafe(r.image, 50));
}

TEST_CASE("repairsConfig: repaired or defaulted blocks count, a migration and a failed read do not") {
  CHECK(vdm::repairsConfig(vdm::kCfgLayoutCrc));
  CHECK(vdm::repairsConfig(vdm::kCfgShadowMissing));
  CHECK(vdm::repairsConfig(vdm::kCfgSettingsCorrupt));
  CHECK(vdm::repairsConfig(vdm::kCfgSafetyCorrupt));
  CHECK(vdm::repairsConfig(vdm::kCfgSensorSlot));
  CHECK(vdm::repairsConfig(vdm::kCfgCalib));
  CHECK_FALSE(vdm::repairsConfig(0));
  CHECK_FALSE(vdm::repairsConfig(vdm::kCfgUnverified));
  CHECK_FALSE(vdm::repairsConfig(vdm::kCfgReadFailed));
  CHECK_FALSE(vdm::repairsConfig(vdm::kCfgUnverified | vdm::kCfgReadFailed));
  CHECK(vdm::repairsConfig(vdm::kCfgUnverified | vdm::kCfgCalib));
  CHECK(vdm::repairsConfig(0xFF));
}

namespace {

ConfigImage imageA() {
  const Stored s = sampleStored();
  RawImages raw;
  writeRaw(s, raw);
  LoadResult r;
  vdm::resolveConfig(raw, r);
  return r.image;
}

// every field different from imageA()
ConfigImage imageB() {
  ConfigImage b = imageA();
  for (uint8_t v = 0; v < vdm::kValveCount; ++v) {
    b.owsensors1[v] = validSlot(static_cast<uint8_t>(100 + v));
    b.owsensors2[v] = validSlot(static_cast<uint8_t>(120 + v));
    b.calib[v] = vdm::CalibRecord{static_cast<uint16_t>(1000 + v), static_cast<uint16_t>(2000 + v), 40, vdm::kCalibValid};
    b.failsafePct[v] = static_cast<uint8_t>(v + 1);
  }
  for (uint8_t i = 0; i < vdm::kExtraSensorSlots; ++i) b.owsensors[i] = validSlot(static_cast<uint8_t>(140 + i));
  b.b_slave = 7;
  b.descr[0] = 'x';
  b.OneWireCfg[1] = 9;
  b.currentbound_low_fac = 31;
  b.currentbound_high_fac = 32;
  b.numberOfMovements = 333;
  b.startOnPower = 77;
  b.noOfMinCounts = 1234;
  b.maxCalibRetries = 0;
  b.escalation = vdm::EscalationConfig{0, 5, 44};
  b.learnTimeS = 12345;
  b.leaseTimeoutMin = 720;
  return b;
}

// fields of `img` that differ from imageA(), as kChanged* groups; slot and record
// differences as bits in `slots` / `calib`
uint16_t changedGroups(const ConfigImage& img, uint32_t& slots, uint16_t& calib) {
  const ConfigImage a = imageA();
  uint16_t groups = 0;
  slots = 0;
  calib = 0;
  for (uint8_t v = 0; v < vdm::kValveCount; ++v) {
    if (!sameSlot(img.owsensors1[v], a.owsensors1[v])) slots |= 1ul << v;
    if (!sameSlot(img.owsensors2[v], a.owsensors2[v])) slots |= 1ul << (v + 12);
    if (!sameCalib(img.calib[v], a.calib[v])) calib = static_cast<uint16_t>(calib | (1u << v));
  }
  if (img.numberOfMovements != a.numberOfMovements) groups |= vdm::kChangedMovements;
  if (img.currentbound_low_fac != a.currentbound_low_fac || img.currentbound_high_fac != a.currentbound_high_fac ||
      img.startOnPower != a.startOnPower || img.noOfMinCounts != a.noOfMinCounts ||
      img.maxCalibRetries != a.maxCalibRetries) {
    groups |= vdm::kChangedMotor;
  }
  // all five motor fields change together
  if (groups & vdm::kChangedMotor) {
    CHECK(img.currentbound_low_fac != a.currentbound_low_fac);
    CHECK(img.currentbound_high_fac != a.currentbound_high_fac);
    CHECK(img.startOnPower != a.startOnPower);
    CHECK(img.noOfMinCounts != a.noOfMinCounts);
    CHECK(img.maxCalibRetries != a.maxCalibRetries);
  }
  if (img.escalation.enable != a.escalation.enable || img.escalation.stepPct != a.escalation.stepPct ||
      img.escalation.maxmA != a.escalation.maxmA) {
    groups |= vdm::kChangedEscalation;
  }
  if (img.learnTimeS != a.learnTimeS) groups |= vdm::kChangedLearnTime;
  if (img.leaseTimeoutMin != a.leaseTimeoutMin) groups |= vdm::kChangedLease;
  if (memcmp(img.failsafePct, a.failsafePct, sizeof(a.failsafePct)) != 0) groups |= vdm::kChangedFailsafe;
  // never merged
  CHECK(img.b_slave == a.b_slave);
  CHECK(memcmp(img.descr, a.descr, sizeof(a.descr)) == 0);
  CHECK(memcmp(img.OneWireCfg, a.OneWireCfg, sizeof(a.OneWireCfg)) == 0);
  for (uint8_t i = 0; i < vdm::kExtraSensorSlots; ++i) CHECK(sameSlot(img.owsensors[i], a.owsensors[i]));
  return groups;
}

uint16_t mergeAndDiff(const vdm::ChangeSet& changes, uint32_t& slots, uint16_t& calib) {
  ConfigImage stored = imageA();
  const ConfigImage ram = imageB();
  vdm::mergeChanges(stored, ram, changes);
  return changedGroups(stored, slots, calib);
}

}  // namespace

TEST_CASE("mergeChanges: sensor slots one by one") {
  uint32_t slots;
  uint16_t calib;
  CHECK(mergeAndDiff(vdm::ChangeSet{vdm::kChangedSensors, 1ul << 5, 0}, slots, calib) == 0);
  CHECK(slots == 1ul << 5);  // owsensors1[5] only
  CHECK(calib == 0);
  CHECK(mergeAndDiff(vdm::ChangeSet{vdm::kChangedSensors, 1ul << 17, 0}, slots, calib) == 0);
  CHECK(slots == 1ul << 17);  // owsensors2[5] only
  CHECK(mergeAndDiff(vdm::ChangeSet{vdm::kChangedSensors, (1ul << 0) | (1ul << 11) | (1ul << 12) | (1ul << 23), 0},
                     slots, calib) == 0);
  CHECK(slots == ((1ul << 0) | (1ul << 11) | (1ul << 12) | (1ul << 23)));
  // the group bit alone copies no slot
  CHECK(mergeAndDiff(vdm::ChangeSet{vdm::kChangedSensors, 0, 0}, slots, calib) == 0);
  CHECK(slots == 0);
}

TEST_CASE("mergeChanges: calibration records one by one") {
  uint32_t slots;
  uint16_t calib;
  CHECK(mergeAndDiff(vdm::ChangeSet{vdm::kChangedCalib, 0, 1u << 3}, slots, calib) == 0);
  CHECK(calib == 1u << 3);
  CHECK(slots == 0);
  CHECK(mergeAndDiff(vdm::ChangeSet{vdm::kChangedCalib, 0, (1u << 0) | (1u << 11)}, slots, calib) == 0);
  CHECK(calib == ((1u << 0) | (1u << 11)));
  CHECK(mergeAndDiff(vdm::ChangeSet{vdm::kChangedCalib, 0, 0}, slots, calib) == 0);
  CHECK(calib == 0);
}

TEST_CASE("mergeChanges: the other fields by group") {
  uint32_t slots;
  uint16_t calib;
  const uint16_t groups[] = {vdm::kChangedMovements, vdm::kChangedMotor, vdm::kChangedEscalation,
                             vdm::kChangedLearnTime, vdm::kChangedLease, vdm::kChangedFailsafe};
  for (uint16_t g : groups) {
    INFO("group ", g);
    CHECK(mergeAndDiff(vdm::ChangeSet{g, 0, 0}, slots, calib) == g);
    CHECK(slots == 0);
    CHECK(calib == 0);
  }
  CHECK(mergeAndDiff(vdm::ChangeSet{vdm::kChangedAll, 0, 0}, slots, calib) ==
        (vdm::kChangedMovements | vdm::kChangedMotor | vdm::kChangedEscalation | vdm::kChangedLearnTime |
         vdm::kChangedLease | vdm::kChangedFailsafe));
}

TEST_CASE("mergeChanges: an empty set and bits beyond the valves copy nothing") {
  uint32_t slots;
  uint16_t calib;
  CHECK(mergeAndDiff(vdm::ChangeSet{0, 0, 0}, slots, calib) == 0);
  CHECK(slots == 0);
  CHECK(calib == 0);
  CHECK(mergeAndDiff(vdm::ChangeSet{0, 0xFF000000ul, 0xF000u}, slots, calib) == 0);
  CHECK(slots == 0);
  CHECK(calib == 0);
}

TEST_CASE("blocksFor: the blocks that hold the fields") {
  constexpr uint8_t layoutBA = vdm::kBlockLayout | vdm::kBlockSafety | vdm::kBlockSettings;
  CHECK(vdm::blocksFor(0) == 0);
  CHECK(vdm::blocksFor(vdm::kChangedSensors) == layoutBA);
  CHECK(vdm::blocksFor(vdm::kChangedMovements) == layoutBA);
  CHECK(vdm::blocksFor(vdm::kChangedMotor) == layoutBA);
  CHECK(vdm::blocksFor(vdm::kChangedEscalation) == vdm::kBlockSettings);
  CHECK(vdm::blocksFor(vdm::kChangedLearnTime) == vdm::kBlockSettings);
  CHECK(vdm::blocksFor(vdm::kChangedLease) == (vdm::kBlockSettings | vdm::kBlockSafety));
  CHECK(vdm::blocksFor(vdm::kChangedFailsafe) == vdm::kBlockSafety);
  CHECK(vdm::blocksFor(vdm::kChangedCalib) == vdm::kBlockCalib);
  CHECK(vdm::blocksFor(vdm::kChangedEscalation | vdm::kChangedCalib) == (vdm::kBlockSettings | vdm::kBlockCalib));
  CHECK(vdm::blocksFor(vdm::kChangedAll) == (layoutBA | vdm::kBlockCalib));
  CHECK(vdm::blocksFor(0xFF00) == 0);
}
