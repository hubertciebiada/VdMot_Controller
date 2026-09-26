// Tests of src/eeprom.cpp (glue_eeprom) on the fake 24LC64: the blocks C(v), B, 1.x layout, A and
// their write order, the load through vdm::resolveConfig (flags, events, repairs written back, lease
// source), the write schedule of eepromloop() (vdm::StoreScheduler: debounce, attempts, backoff),
// the I2C bus restart before every retry (S5), the re-read after a failed read that keeps the slots
// and records changed meanwhile (W11), calibration records and the counters of gstax.
#include "eeprom.h"
#include "glue_test.h"
#include "hardware.h"
#include "stub_app.h"
#include "vdm/config_blocks.h"
#include "vdm/config_store.h"
#include "vdm/eeprom_layout.h"
#include "vdm/legacy_layout.h"
#include "vdm/replies_v2.h"

namespace {

// a DS18B20 address with a valid CRC: family 0x28, serial n
void validSlot(ds1820_eeprom_layout& s, uint8_t n) {
  uint8_t a[8] = {0x28, n, 0, 0, 0, 0, 0, 0};
  a[7] = OneWire::crc8(a, 7);
  s.familycode = a[0];
  for (unsigned i = 0; i < 6; i++) s.romcode[i] = a[1 + i];
  s.crc = a[7];
}

void fillLayout(eeprom_layout& lay) {
  lay = eeprom_layout();
  lay.b_slave = 1;
  strncpy(lay.descr, "VdMot Controller", sizeof lay.descr);
  lay.currentbound_low_fac = 18;
  lay.currentbound_high_fac = 19;
  lay.numberOfMovements = 0x1234;
  for (unsigned v = 0; v < ACTUATOR_COUNT; v++) {
    validSlot(lay.owsensors1[v], static_cast<uint8_t>(v));
    validSlot(lay.owsensors2[v], static_cast<uint8_t>(100 + v));
  }
  lay.startOnPower = 40;
  lay.noOfMinCounts = 0xABCD;
  lay.maxCalibRetries = 2;
  lay.escalation = {1, 30, 40};
  lay.learnTimeS = 3600;
  lay.leaseTimeoutMin = 90;
  for (unsigned v = 0; v < ACTUATOR_COUNT; v++) lay.failsafePct[v] = static_cast<uint8_t>(10 + v);
}

std::vector<uint8_t> stored(uint16_t address, size_t length) {
  return std::vector<uint8_t>(fake::eeprom.bytes + address, fake::eeprom.bytes + address + length);
}

// a consistent configuration in the EEPROM (1.x layout, A, B, no calibration records)
void storeConsistent() {
  eeprom_layout lay;
  fillLayout(lay);
  REQUIRE(eeprom_write_layout(&lay) == 0);
  fake::eeprom.ops.clear();
  fake::takeTx(Serial6);
}

// start-up read into eep_content
int16_t boot() {
  eepromsetup();
  const int16_t r = eeprom_read_layout(&eep_content);
  fake::eeprom.ops.clear();
  fake::takeTx(Serial6);
  stub::calls.clear();
  return r;
}

// eepromloop() runs until the EEPROM is written or n runs
unsigned runsUntilWrite(unsigned n) {
  const unsigned writes = fake::eeprom.writes;
  for (unsigned i = 1; i <= n; i++) {
    eepromloop();
    if (fake::eeprom.writes != writes) return i;
  }
  return 0;
}

vdm::CalibRecord record(uint16_t oc, uint16_t cc, uint16_t mA, uint8_t flags) {
  vdm::CalibRecord r;
  r.openingCount = oc;
  r.closingCount = cc;
  r.meanCurrent = mA;
  r.flags = flags;
  return r;
}

}  // namespace

TEST_CASE("eepromsetup: the RAM mirror starts as not loaded, nothing pending") {
  glue::begin();
  eep_content.status = EEP_CHANGED;
  CHECK(eepromsetup() == 0);
  CHECK(eep_content.status == EEP_INIT);
  CHECK(eeprom_free());
  CHECK(eeprom_state() == vdm::kEepStateOk);
  CHECK(eeprom_writes() == 0);
  CHECK(eeprom_cfg_flags() == 0);
  CHECK(eeprom_cfg_events() == 0);
  CHECK(eeprom_lease_source() == vdm::kLeaseSourceDefault);
}

TEST_CASE("eeprom_write_layout: block B, the 1.x layout in one transfer, then block A with the layout CRC") {
  glue::begin();
  eeprom_layout lay;
  fillLayout(lay);
  lay.calib[0] = record(3600, 3650, 20, vdm::kCalibValid);
  CHECK(eeprom_write_layout(&lay) == 0);
  REQUIRE(fake::eeprom.ops.size() == 3);
  CHECK(fake::eeprom.ops[0].address == vdm::kSafetyBlockAddress);
  CHECK(fake::eeprom.ops[0].length == 25);
  CHECK(fake::eeprom.ops[1].address == vdm::kLegacyLayoutAddress);
  CHECK(fake::eeprom.ops[1].length == vdm::kLegacyImageSize);
  CHECK(fake::eeprom.ops[2].address == vdm::kExtensionAddress);
  CHECK(fake::eeprom.ops[2].length == 14);
  uint8_t image[vdm::kLegacyImageSize];
  vdm::encodeLegacyLayout(lay, image);
  CHECK(stored(vdm::kLegacyLayoutAddress, sizeof image) == std::vector<uint8_t>(image, image + sizeof image));
  uint8_t ext[vdm::kExtensionBlockSize];
  memcpy(ext, fake::eeprom.bytes + vdm::kExtensionAddress, sizeof ext);
  vdm::StoredExtension back;
  CHECK(vdm::decodeExtension(ext, back) == vdm::ExtensionState::Valid);
  CHECK(back.layoutCrc == vdm::crc16Ccitt(image, sizeof image));
  CHECK(back.learnTimeS == 3600);
  CHECK(back.leaseTimeoutMin == 90);
  CHECK(back.escalation.maxmA == 40);
  uint8_t safety[vdm::kSafetyBlockSize];
  memcpy(safety, fake::eeprom.bytes + vdm::kSafetyBlockAddress, sizeof safety);
  vdm::SafetyBlock b;
  CHECK(vdm::decodeSafety(safety, b) == vdm::BlockState::Valid);
  CHECK(b.failsafePct[0] == 10);
  CHECK(b.failsafePct[11] == 21);
  CHECK(b.shadow.lowFac == 18);
  CHECK(b.shadow.highFac == 19);
  CHECK(b.shadow.movements == 0x1234);
  CHECK(b.shadow.startOnPower == 40);
  CHECK(b.shadow.minCounts == 0xABCD);
  CHECK(b.shadow.maxRetries == 2);
  CHECK(b.leaseValid);
  CHECK(b.leaseTimeoutMin == 90);
  // no calibration record: they are written by eeprom_store_calib() only
  CHECK(stored(vdm::kCalibBlockAddress, 16) == std::vector<uint8_t>(16, 0xFF));
  CHECK(fake::takeTx(Serial6) == "write eeprom layout to eeprom...\r\nfinished\r\n");
}

TEST_CASE("eeprom_write_layout: stops at the first failed transfer, for every one of the 3") {
  for (unsigned k = 1; k <= 3; k++) {
    CAPTURE(k);
    glue::begin();
    eeprom_layout lay;
    fillLayout(lay);
    fake::eeprom.failWritesFrom = k;
    CHECK(eeprom_write_layout(&lay) == -1);
    CHECK(fake::eeprom.ops.size() == k);
    CHECK(fake::takeTx(Serial6) == "write eeprom layout to eeprom...\r\nwrite error, aborted\r\n");
  }
}

TEST_CASE("eeprom_read_layout: a consistent configuration loads without flags and without a write") {
  glue::begin();
  storeConsistent();
  eeprom_layout back;
  memset(&back, 0, sizeof back);
  CHECK(eeprom_read_layout(&back) == 0);
  REQUIRE(fake::eeprom.ops.size() == 4);
  CHECK(fake::eeprom.ops[0].address == vdm::kLegacyLayoutAddress);
  CHECK(fake::eeprom.ops[0].length == 309);
  CHECK(fake::eeprom.ops[1].address == vdm::kExtensionAddress);
  CHECK(fake::eeprom.ops[1].length == 16);
  CHECK(fake::eeprom.ops[2].address == vdm::kSafetyBlockAddress);
  CHECK(fake::eeprom.ops[2].length == 32);
  CHECK(fake::eeprom.ops[3].address == vdm::kCalibBlockAddress);
  CHECK(fake::eeprom.ops[3].length == 192);
  CHECK(back.status == EEP_VALID);
  CHECK(back.b_slave == 1);
  CHECK(std::string(back.descr) == "VdMot Controller");
  CHECK(back.numberOfMovements == 0x1234);
  CHECK(back.owsensors1[11].romcode[0] == 11);
  CHECK(back.owsensors2[11].romcode[0] == 111);
  CHECK(back.noOfMinCounts == 0xABCD);
  CHECK(back.maxCalibRetries == 2);
  CHECK(back.escalation.stepPct == 30);
  CHECK(back.learnTimeS == 3600);
  CHECK(back.leaseTimeoutMin == 90);
  CHECK(back.failsafePct[5] == 15);
  CHECK(back.calib[0].flags == 0);
  CHECK(eeprom_state() == vdm::kEepStateOk);
  CHECK(eeprom_cfg_flags() == 0);
  CHECK(eeprom_cfg_events() == 0);
  CHECK(eeprom_lease_source() == vdm::kLeaseSourceSettings);
  CHECK(fake::takeTx(Serial6) == "Read eeprom layout from eeprom...finished, cfgFlags 0\r\n");
  for (int i = 0; i < 40; i++) eepromloop();
  CHECK(fake::eeprom.writes == 3);
  CHECK(eeprom_writes() == 0);
}

TEST_CASE("first start on a new chip: defaults, unverified, one write of B, layout and A") {
  glue::begin();
  CHECK(boot() == 0);
  CHECK(eeprom_cfg_flags() == vdm::kCfgUnverified);
  CHECK(eeprom_cfg_events() == 0);
  CHECK(eeprom_lease_source() == vdm::kLeaseSourceDefault);
  CHECK(eep_content.leaseTimeoutMin == 60);
  CHECK(eep_content.failsafePct[0] == 50);
  CHECK(eep_content.learnTimeS == 604800);
  CHECK(eep_content.status == EEP_CHANGED);
  CHECK(eeprom_state() == vdm::kEepStatePending);
  CHECK_FALSE(eeprom_free());
  CHECK(runsUntilWrite(10) == 3);
  CHECK(fake::eeprom.ops.size() == 3);
  CHECK(eeprom_writes() == 1);
  CHECK(eeprom_state() == vdm::kEepStateOk);
  CHECK(eep_content.status == EEP_VALID);
  CHECK(stub::calls.empty());  // a first write is no retry: no bus restart
  CHECK(boot() == 0);
  CHECK(eeprom_cfg_flags() == 0);
  CHECK(eeprom_state() == vdm::kEepStateOk);
}

TEST_CASE("a damaged byte of the 1.x motor fields: taken from block B, flagged, repaired by one write") {
  glue::begin();
  storeConsistent();
  fake::eeprom.bytes[0x0139] ^= 0x8B;  // low byte of noOfMinCounts
  CHECK(boot() == 0);
  CHECK(eep_content.noOfMinCounts == 0xABCD);
  CHECK(eeprom_cfg_flags() == vdm::kCfgLayoutCrc);
  CHECK(eeprom_cfg_events() == 1);
  CHECK(eeprom_state() == vdm::kEepStatePending);
  CHECK(runsUntilWrite(10) == 3);
  CHECK(boot() == 0);
  CHECK(eeprom_cfg_flags() == 0);
  CHECK(eeprom_cfg_events() == 1);
  CHECK(eep_content.noOfMinCounts == 0xABCD);
}

TEST_CASE("lease source: block A, else the copy in B, else the default") {
  glue::begin();
  storeConsistent();
  boot();
  CHECK(eeprom_lease_source() == vdm::kLeaseSourceSettings);
  fake::eeprom.bytes[vdm::kExtensionAddress + 5] ^= 1;  // block A damaged
  boot();
  CHECK(eeprom_lease_source() == vdm::kLeaseSourceSafety);
  CHECK(eep_content.leaseTimeoutMin == 90);
  CHECK(eeprom_cfg_flags() == vdm::kCfgSettingsCorrupt);
  CHECK(eeprom_cfg_events() == 1);
  fake::eeprom.bytes[vdm::kSafetyBlockAddress + 3] ^= 1;  // and block B
  boot();
  CHECK(eeprom_lease_source() == vdm::kLeaseSourceDefault);
  CHECK(eep_content.leaseTimeoutMin == 60);
  CHECK(eeprom_cfg_flags() == (vdm::kCfgSettingsCorrupt | vdm::kCfgSafetyCorrupt));
  CHECK(eeprom_cfg_events() == 2);
}

TEST_CASE("eepromloop: a change is written 3 s after the last change, at most 30 s after the first") {
  glue::begin();
  storeConsistent();
  boot();
  eeprom_changed(EEP_CHANGED_LEARNTIME);
  CHECK(eeprom_state() == vdm::kEepStatePending);
  CHECK(eep_content.status == EEP_CHANGED);
  CHECK_FALSE(eeprom_free());
  eepromloop();
  eepromloop();
  eeprom_changed(EEP_CHANGED_LEARNTIME);
  CHECK(runsUntilWrite(10) == 3);
  REQUIRE(fake::eeprom.ops.size() == 1);  // the learn time is in block A only
  CHECK(fake::eeprom.ops[0].address == vdm::kExtensionAddress);
  CHECK(eep_content.status == EEP_VALID);
  CHECK(eeprom_state() == vdm::kEepStateOk);
  CHECK(eeprom_free());
  CHECK(eeprom_writes() == 1);
  // a change every second: written after 30
  unsigned n = 0;
  const unsigned writes = fake::eeprom.writes;
  while (fake::eeprom.writes == writes && n < 40) {
    eeprom_changed(EEP_CHANGED_FAILSAFE);
    eepromloop();
    n++;
  }
  CHECK(n == 30);
  CHECK(fake::eeprom.ops.back().address == vdm::kSafetyBlockAddress);
  CHECK(eeprom_writes() == 2);
  CHECK(stub::calls.empty());
}

TEST_CASE("eepromloop: blocks per field, calibration records first, block A last") {
  glue::begin();
  storeConsistent();
  boot();
  eeprom_store_calib(3, record(3600, 3650, 20, vdm::kCalibValid));
  eeprom_store_calib(1, record(1000, 1100, 30, vdm::kCalibValid | vdm::kCalibFailed));
  eeprom_changed(EEP_CHANGED_ESCALATION | EEP_CHANGED_LEASE);
  CHECK(runsUntilWrite(10) == 3);
  REQUIRE(fake::eeprom.ops.size() == 4);
  CHECK(fake::eeprom.ops[0].address == vdm::kCalibBlockAddress + 16);
  CHECK(fake::eeprom.ops[0].length == 12);
  CHECK(fake::eeprom.ops[1].address == vdm::kCalibBlockAddress + 48);
  CHECK(fake::eeprom.ops[2].address == vdm::kSafetyBlockAddress);
  CHECK(fake::eeprom.ops[3].address == vdm::kExtensionAddress);
  uint8_t raw[vdm::kCalibBlockSize];
  memcpy(raw, fake::eeprom.bytes + vdm::kCalibBlockAddress + 48, sizeof raw);
  vdm::CalibRecord back;
  CHECK(vdm::decodeCalib(raw, 3, back) == vdm::BlockState::Valid);
  CHECK(back.openingCount == 3600);
  CHECK(back.closingCount == 3650);
  CHECK(back.meanCurrent == 20);
  CHECK(back.flags == vdm::kCalibValid);
  // the next write step has no record left
  fake::eeprom.ops.clear();
  eeprom_changed(EEP_CHANGED_FAILSAFE);
  CHECK(runsUntilWrite(10) == 3);
  REQUIRE(fake::eeprom.ops.size() == 1);
  CHECK(fake::eeprom.ops[0].address == vdm::kSafetyBlockAddress);
  // the records load at the next start
  boot();
  CHECK(eep_content.calib[3].openingCount == 3600);
  CHECK(eep_content.calib[1].flags == (vdm::kCalibValid | vdm::kCalibFailed));
  CHECK(eep_content.calib[0].flags == 0);
}

TEST_CASE("eeprom_store_calib: an unchanged record or an invalid valve is not written") {
  glue::begin();
  storeConsistent();
  boot();
  eeprom_store_calib(3, record(0, 0, 0, 0));
  eeprom_store_calib(12, record(3600, 3650, 20, vdm::kCalibValid));
  CHECK(eeprom_state() == vdm::kEepStateOk);
  eeprom_store_calib(3, record(3600, 3650, 20, vdm::kCalibValid));
  CHECK(eeprom_state() == vdm::kEepStatePending);
  CHECK(eep_content.status == EEP_CHANGED);
  CHECK(eep_content.calib[3].closingCount == 3650);
  runsUntilWrite(10);
  CHECK(eeprom_state() == vdm::kEepStateOk);
  for (const vdm::CalibRecord& same : {record(3600, 3650, 20, vdm::kCalibValid)}) {
    eeprom_store_calib(3, same);
    CHECK(eeprom_state() == vdm::kEepStateOk);
  }
  const vdm::CalibRecord changed[] = {record(3601, 3650, 20, 1), record(3600, 3651, 20, 1), record(3600, 3650, 21, 1),
                                      record(3600, 3650, 20, 3)};
  for (const vdm::CalibRecord& r : changed) {
    fake::eeprom.ops.clear();
    eeprom_store_calib(3, r);
    CHECK(eeprom_state() == vdm::kEepStatePending);
    CHECK(runsUntilWrite(10) == 3);
    REQUIRE(fake::eeprom.ops.size() == 1);
    CHECK(fake::eeprom.ops[0].address == vdm::kCalibBlockAddress + 48);
  }
}

TEST_CASE("eepromloop: a failed write is repeated with a bus restart, after 3 attempts eepState 2 and backoff") {
  glue::begin();
  storeConsistent();
  boot();
  fake::eeprom.failWritesFrom = fake::eeprom.writes + 1;
  eeprom_changed(EEP_CHANGED_LEARNTIME);
  for (int i = 0; i < 3; i++) eepromloop();
  CHECK(fake::eeprom.writes == 4);
  CHECK(stub::calls.empty());
  eepromloop();
  CHECK(stub::calls == stub::Calls{"i2c_bus_restart()"});
  eepromloop();
  CHECK(fake::eeprom.writes == 6);
  CHECK(eeprom_state() == vdm::kEepStateWriteFailed);
  CHECK(eeprom_free());
  CHECK(eeprom_writes() == 0);
  CHECK(stub::calls == stub::Calls{"i2c_bus_restart()", "i2c_bus_restart()"});
  // retried after 30 s with a bus restart
  fake::eeprom.failWritesFrom = 0;
  stub::calls.clear();
  unsigned n = runsUntilWrite(40);
  CHECK(n == 30);
  CHECK(stub::calls == stub::Calls{"i2c_bus_restart()"});
  CHECK(eeprom_state() == vdm::kEepStateOk);
  CHECK(eeprom_writes() == 1);
  CHECK(fake::takeTx(Serial6).find("write error, aborted") != std::string::npos);
}

TEST_CASE("a failed start-up read: defaults, eepState 3, nothing written, the re-read after a bus restart") {
  glue::begin();
  storeConsistent();
  fake::eeprom.failReadsFrom = 2;
  fake::eeprom.failReadsCount = 3;
  CHECK(boot() == -1);
  CHECK(eep_content.b_slave == 1);                     // the layout was read
  CHECK(eep_content.escalation.stepPct != 30);         // block A was not
  CHECK(eeprom_state() == vdm::kEepStateReadFailed);
  CHECK(eeprom_cfg_flags() == vdm::kCfgReadFailed);
  CHECK(eeprom_cfg_events() == 0);
  CHECK(eep_content.status == EEP_VALID);
  eeprom_changed(EEP_CHANGED_MOTOR);
  CHECK(eeprom_state() == vdm::kEepStateReadFailed);
  CHECK(eeprom_free());
  const unsigned writes = fake::eeprom.writes;
  for (int i = 0; i < 29; i++) eepromloop();
  CHECK(fake::eeprom.writes == writes);
  CHECK(stub::calls.empty());
  eepromloop();  // 30 s: re-read, then the change is written
  CHECK(stub::calls == stub::Calls{"i2c_bus_restart()", "app_load_config()"});
  CHECK(eeprom_state() == vdm::kEepStatePending);
  CHECK(eep_content.escalation.stepPct == 30);
  CHECK(eeprom_cfg_flags() == 0);
  CHECK(fake::takeTx(Serial6).find("eeprom read after failure\r\n") != std::string::npos);
  CHECK(runsUntilWrite(10) == 3);  // the debounce restarts with the re-read
  CHECK(eeprom_state() == vdm::kEepStateOk);
}

TEST_CASE("a failed re-read keeps the backoff: the next one after 60 s") {
  glue::begin();
  storeConsistent();
  fake::eeprom.failReadsFrom = 1;
  CHECK(boot() == -1);
  for (int i = 0; i < 30; i++) eepromloop();
  CHECK(stub::calls == stub::Calls{"i2c_bus_restart()"});
  CHECK(eeprom_state() == vdm::kEepStateReadFailed);
  fake::eeprom.failReadsFrom = 0;
  stub::calls.clear();
  for (int i = 0; i < 59; i++) eepromloop();
  CHECK(stub::calls.empty());
  eepromloop();
  CHECK(stub::calls == stub::Calls{"i2c_bus_restart()", "app_load_config()"});
  CHECK(eeprom_state() == vdm::kEepStateOk);
}

TEST_CASE("re-read after a failed read: the lease source of the blocks read, or of a timeout set meanwhile") {
  glue::begin();
  storeConsistent();
  fake::eeprom.failReadsFrom = 1;
  boot();
  CHECK(eeprom_lease_source() == vdm::kLeaseSourceDefault);
  CHECK(eep_content.leaseTimeoutMin == 60);
  fake::eeprom.failReadsFrom = 0;
  for (int i = 0; i < 30; i++) eepromloop();
  CHECK(eeprom_state() == vdm::kEepStateOk);
  CHECK(eeprom_lease_source() == vdm::kLeaseSourceSettings);
  CHECK(eep_content.leaseTimeoutMin == 90);
  // block A damaged: the copy in B, also after a change of another field
  fake::eeprom.bytes[vdm::kExtensionAddress + 5] ^= 1;
  fake::eeprom.failReadsFrom = 1;
  boot();
  eeprom_changed(EEP_CHANGED_MOTOR);
  fake::eeprom.failReadsFrom = 0;
  for (int i = 0; i < 120; i++) eepromloop();
  CHECK(eeprom_lease_source() == vdm::kLeaseSourceSafety);
  CHECK(eep_content.leaseTimeoutMin == 90);
  // slcfg while the EEPROM could not be read: its timeout counts as configured
  fake::eeprom.failReadsFrom = 1;
  boot();
  eep_content.leaseTimeoutMin = 0;
  eeprom_changed(EEP_CHANGED_LEASE);
  fake::eeprom.failReadsFrom = 0;
  for (int i = 0; i < 120; i++) eepromloop();
  CHECK(eeprom_lease_source() == vdm::kLeaseSourceSettings);
  CHECK(eep_content.leaseTimeoutMin == 0);
}

TEST_CASE("re-read after a failed read: only the changed sensor slot and record are taken from RAM") {
  glue::begin();
  storeConsistent();
  fake::eeprom.failReadsFrom = 1;
  boot();
  // RAM holds 0xFF fallbacks; stsnx 2 (slot 2) and a calibration record of valve 4 meanwhile
  validSlot(eep_content.owsensors1[2], 77);
  eeprom_changed_slot(2);
  validSlot(eep_content.owsensors2[5], 78);
  eeprom_changed_slot(17);
  eeprom_store_calib(4, record(2000, 2100, 25, vdm::kCalibValid));
  fake::eeprom.failReadsFrom = 0;
  for (int i = 0; i < 30; i++) eepromloop();
  CHECK(eeprom_state() == vdm::kEepStatePending);
  CHECK(eep_content.owsensors1[2].romcode[0] == 77);
  CHECK(eep_content.owsensors2[5].romcode[0] == 78);
  CHECK(eep_content.owsensors1[3].romcode[0] == 3);
  CHECK(eep_content.owsensors1[1].romcode[0] == 1);
  CHECK(eep_content.owsensors2[4].romcode[0] == 104);
  CHECK(eep_content.owsensors2[6].romcode[0] == 106);
  CHECK(eep_content.calib[4].openingCount == 2000);
  CHECK(eep_content.noOfMinCounts == 0xABCD);
  fake::eeprom.ops.clear();
  CHECK(runsUntilWrite(10) == 3);
  REQUIRE(fake::eeprom.ops.size() == 4);
  CHECK(fake::eeprom.ops[0].address == vdm::kCalibBlockAddress + 64);
  // stored: the other slots as before, the new ones
  boot();
  CHECK(eep_content.owsensors1[2].romcode[0] == 77);
  CHECK(eep_content.owsensors1[3].romcode[0] == 3);
  CHECK(eep_content.owsensors2[5].romcode[0] == 78);
  CHECK(eep_content.calib[4].closingCount == 2100);
  CHECK(eeprom_cfg_flags() == 0);
}

TEST_CASE("re-read: eeprom_changed(EEP_CHANGED_SENSORS) takes every slot, EEP_CHANGED_CALIB every record") {
  glue::begin();
  storeConsistent();
  fake::eeprom.failReadsFrom = 1;
  boot();
  memset(eep_content.owsensors1, 0, sizeof eep_content.owsensors1);
  memset(eep_content.owsensors2, 0, sizeof eep_content.owsensors2);
  eep_content.calib[11] = record(1500, 1500, 20, vdm::kCalibValid);
  eeprom_changed(EEP_CHANGED_SENSORS | EEP_CHANGED_CALIB);
  fake::eeprom.failReadsFrom = 0;
  for (int i = 0; i < 30; i++) eepromloop();
  CHECK(eep_content.owsensors1[0].familycode == 0);
  CHECK(eep_content.owsensors1[11].familycode == 0);
  CHECK(eep_content.owsensors2[0].familycode == 0);
  CHECK(eep_content.owsensors2[11].familycode == 0);
  CHECK(eep_content.calib[11].openingCount == 1500);
  CHECK(eep_content.numberOfMovements == 0x1234);
}

TEST_CASE("a sensor slot with a flipped bit is cleared, flagged and written back") {
  glue::begin();
  storeConsistent();
  fake::eeprom.bytes[vdm::kLegacyLayoutAddress + 33 + 8 * 4 + 1] ^= 0x10;  // owsensors1[4]
  boot();
  CHECK(eeprom_cfg_flags() == (vdm::kCfgLayoutCrc | vdm::kCfgSensorSlot));
  CHECK(eep_content.owsensors1[4].familycode == 0);
  CHECK(eep_content.owsensors1[5].familycode == 0x28);
  CHECK(eeprom_state() == vdm::kEepStatePending);
}
