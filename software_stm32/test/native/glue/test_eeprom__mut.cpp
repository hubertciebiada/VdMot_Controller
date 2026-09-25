// More tests of src/eeprom.cpp (glue_eeprom): a failed calibration record write, the read failure
// budget of one read, the slot bookkeeping after a successful write and the loop return value.
#include "eeprom.h"
#include "glue_test.h"
#include "hardware.h"
#include "stub_app.h"
#include "vdm/config_blocks.h"
#include "vdm/config_store.h"
#include "vdm/legacy_layout.h"
#include "vdm/replies_v2.h"

namespace {

void validSlot(ds1820_eeprom_layout& s, uint8_t n) {
  uint8_t a[8] = {0x28, n, 0, 0, 0, 0, 0, 0};
  a[7] = OneWire::crc8(a, 7);
  s.familycode = a[0];
  for (unsigned i = 0; i < 6; i++) s.romcode[i] = a[1 + i];
  s.crc = a[7];
}

void storeConsistent() {
  eeprom_layout lay = eeprom_layout();
  lay.b_slave = 1;
  lay.currentbound_low_fac = 18;
  lay.currentbound_high_fac = 19;
  for (unsigned v = 0; v < ACTUATOR_COUNT; v++) {
    validSlot(lay.owsensors1[v], static_cast<uint8_t>(v));
    validSlot(lay.owsensors2[v], static_cast<uint8_t>(100 + v));
  }
  lay.leaseTimeoutMin = 90;
  REQUIRE(eeprom_write_layout(&lay) == 0);
  fake::eeprom.ops.clear();
  fake::takeTx(Serial6);
}

int16_t boot() {
  eepromsetup();
  const int16_t r = eeprom_read_layout(&eep_content);
  fake::eeprom.ops.clear();
  fake::takeTx(Serial6);
  stub::calls.clear();
  return r;
}

// eepromloop() until the storage state is ok (at most n runs)
void settle(int n) {
  for (int i = 0; i < n && eeprom_state() != vdm::kEepStateOk; i++) eepromloop();
}

}  // namespace

TEST_CASE("eepromloop: returns 0, a failed calibration record write is not a successful step") {
  glue::begin();
  storeConsistent();
  boot();
  settle(40);
  const uint32_t steps = eeprom_writes();
  fake::eeprom.failWritesFrom = fake::eeprom.writes + 1;
  vdm::CalibRecord r;
  r.openingCount = 3000;
  r.closingCount = 3100;
  r.meanCurrent = 20;
  r.flags = vdm::kCalibValid;
  eeprom_store_calib(2, r);
  for (int i = 0; i < 3; i++) CHECK(eepromloop() == 0);
  CHECK(fake::eeprom.ops.size() == 1);
  CHECK(eeprom_writes() == steps);
  CHECK(eeprom_state() != vdm::kEepStateOk);
  CHECK(fake::takeTx(Serial6).find("write error, aborted") != std::string::npos);
  fake::eeprom.failWritesFrom = 0;
  settle(40);
  CHECK(eeprom_writes() == steps + 1);
}

TEST_CASE("eeprom_read_layout: two failed transfers in one read are retried, the third ends the read") {
  glue::begin();
  storeConsistent();
  fake::eeprom.failReadsFrom = fake::eeprom.reads + 1;
  fake::eeprom.failReadsCount = 2;
  CHECK(boot() == 0);
  CHECK(eeprom_cfg_flags() == 0);
  fake::eeprom.failReadsFrom = fake::eeprom.reads + 1;
  fake::eeprom.failReadsCount = 3;
  CHECK(boot() == -1);
  CHECK(eeprom_cfg_flags() == vdm::kCfgReadFailed);
  fake::eeprom.failReadsFrom = 0;
  fake::eeprom.failReadsCount = 0;
  settle(40);
}

TEST_CASE("a written sensor slot is no longer taken from RAM at a later re-read") {
  glue::begin();
  storeConsistent();
  boot();
  settle(40);
  // slot 0 changed and written
  validSlot(eep_content.owsensors1[0], 50);
  eeprom_changed_slot(0);
  settle(10);
  REQUIRE(eeprom_state() == vdm::kEepStateOk);
  // a failed read; the re-read takes slot 0 from the EEPROM, not from RAM
  fake::eeprom.failReadsFrom = fake::eeprom.reads + 1;
  fake::eeprom.failReadsCount = 0;
  CHECK(boot() == -1);
  eep_content.owsensors1[0].familycode = 0;
  fake::eeprom.failReadsFrom = 0;
  settle(40);
  CHECK(eep_content.owsensors1[0].familycode == 0x28);
  CHECK(eep_content.owsensors1[0].romcode[0] == 50);
}
