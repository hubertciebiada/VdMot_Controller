// Smoke tests of src/eeprom.cpp (glue_eeprom) on the fake 24LC64: layout write and read, transfer
// failures, the write delay of eepromloop() and the storage state reported by gstat.
#include "eeprom.h"
#include "glue_test.h"
#include "hardware.h"
#include "stub_app.h"
#include "vdm/eeprom_layout.h"
#include "vdm/legacy_layout.h"
#include "vdm/replies_v2.h"

namespace {

void fillLayout(eeprom_layout& lay) {
  lay = eeprom_layout();
  lay.b_slave = 1;
  strncpy(lay.descr, "VdMot Controller", sizeof lay.descr);
  lay.currentbound_low_fac = 18;
  lay.currentbound_high_fac = 19;
  lay.numberOfMovements = 0x1234;
  for (unsigned v = 0; v < ACTUATOR_COUNT; v++) {
    lay.owsensors1[v].familycode = 0x28;
    lay.owsensors1[v].romcode[0] = static_cast<uint8_t>(v);
    lay.owsensors1[v].crc = static_cast<uint8_t>(0x80 + v);
  }
  lay.startOnPower = 40;
  lay.noOfMinCounts = 0xABCD;
  lay.maxCalibRetries = 2;
  lay.escalation = {1, 30, 40};
  lay.learnTimeS = 3600;
  lay.leaseTimeoutMin = 60;
}

std::vector<uint8_t> stored(uint16_t address, size_t length) {
  return std::vector<uint8_t>(fake::eeprom.bytes + address, fake::eeprom.bytes + address + length);
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

}  // namespace

TEST_CASE("eepromsetup: the RAM mirror starts as not loaded") {
  glue::begin();
  eep_content.status = EEP_CHANGED;
  CHECK(eepromsetup() == 0);
  CHECK(eep_content.status == EEP_INIT);
  CHECK(eeprom_free());
}

TEST_CASE("eeprom_write_layout: the 1.x image in 37 transfers, then block A with the layout CRC") {
  glue::begin();
  eeprom_layout lay;
  fillLayout(lay);
  CHECK(eeprom_write_layout(&lay) == 0);
  REQUIRE(fake::eeprom.ops.size() == 1 + 34 + 1 + 1);
  CHECK(fake::eeprom.ops[0].address == EE_GENERALDATA_ADR);
  CHECK(fake::eeprom.ops[0].length == 33);
  CHECK(fake::eeprom.ops[1].address == EE_GENERALDATA_ADR + 33);
  CHECK(fake::eeprom.ops[34].address == EE_GENERALDATA_ADR + 33 + 33 * 8);
  CHECK(fake::eeprom.ops[35].length == 4);
  CHECK(fake::eeprom.ops[36].address == vdm::kExtensionAddress);
  uint8_t image[vdm::kLegacyImageSize];
  vdm::encodeLegacyLayout(lay, image);
  CHECK(stored(EE_GENERALDATA_ADR, sizeof image) == std::vector<uint8_t>(image, image + sizeof image));
  uint8_t ext[vdm::kExtensionBlockSize];
  memcpy(ext, fake::eeprom.bytes + vdm::kExtensionAddress, sizeof ext);
  vdm::StoredExtension back;
  CHECK(vdm::decodeExtension(ext, back) == vdm::ExtensionState::Valid);
  CHECK(back.layoutCrc == vdm::crc16Ccitt(image, sizeof image));
  CHECK(back.learnTimeS == 3600);
  CHECK(back.leaseTimeoutMin == 60);
  CHECK(fake::takeTx(Serial6) == "write eeprom layout to eeprom...\r\nfinished\r\n");
}

TEST_CASE("eeprom_write_layout: stops at the first failed transfer, for every one of the 37") {
  for (unsigned k = 1; k <= 37; k++) {
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

TEST_CASE("eeprom_read_layout: what eeprom_write_layout wrote reads back") {
  glue::begin();
  eeprom_layout lay;
  fillLayout(lay);
  REQUIRE(eeprom_write_layout(&lay) == 0);
  eeprom_layout back;
  memset(&back, 0, sizeof back);
  CHECK(eeprom_read_layout(&back) == 0);
  CHECK(back.status == EEP_VALID);
  CHECK(back.b_slave == 1);
  CHECK(std::string(back.descr) == "VdMot Controller");
  CHECK(back.numberOfMovements == 0x1234);
  CHECK(back.owsensors1[11].crc == 0x8B);
  CHECK(back.owsensors1[11].romcode[0] == 11);
  CHECK(back.noOfMinCounts == 0xABCD);
  CHECK(back.maxCalibRetries == 2);
  CHECK(back.escalation.stepPct == 30);
  CHECK(back.learnTimeS == 3600);
  CHECK(eeprom_state() == vdm::kEepStateOk);
}

TEST_CASE("eeprom_read_layout: after 3 failed transfers the rest is 0xFF and writing stays off") {
  glue::begin();
  eeprom_layout lay;
  fillLayout(lay);
  REQUIRE(eeprom_write_layout(&lay) == 0);
  fake::takeTx(Serial6);
  fake::eeprom.failReadsFrom = 2;
  fake::eeprom.failReadsCount = 3;
  CHECK(eeprom_read_layout(&eep_content) == -1);
  CHECK(eep_content.b_slave == 1);                       // block 1 was read
  CHECK(eep_content.owsensors1[0].familycode == 0xFF);   // blocks after the third failure: erased value
  CHECK(eeprom_state() == vdm::kEepStateReadFailed);
  // a change is kept in RAM, not written over the stored layout
  eepromloop();
  eeprom_changed(EEP_CHANGED_MOTOR);
  CHECK(eeprom_state() == vdm::kEepStateReadFailed);
  const unsigned writes = fake::eeprom.writes;
  for (int i = 0; i < 6; i++) eepromloop();
  CHECK(fake::eeprom.writes == writes);
}

TEST_CASE("eepromloop: a change is written on the 4th run after it, then the storage is ok") {
  glue::begin();
  eeprom_read_layout(&eep_content);
  eepromloop();  // E_INIT
  eeprom_changed(EEP_CHANGED_MOVEMENTS);
  CHECK(eeprom_state() == vdm::kEepStatePending);
  CHECK_FALSE(eeprom_free());
  CHECK(runsUntilWrite(10) == 4);
  CHECK(eep_content.status == EEP_VALID);
  CHECK(eeprom_state() == vdm::kEepStateOk);
  CHECK(eeprom_free());
}

TEST_CASE("eepromloop: a write that fails 3 times gives up with eepState 2") {
  glue::begin();
  eeprom_read_layout(&eep_content);
  eepromloop();
  fake::eeprom.failWritesFrom = 1;
  eeprom_changed(EEP_CHANGED_ALL);
  for (int i = 0; i < 20; i++) eepromloop();
  CHECK(fake::eeprom.writes == 3);
  CHECK(eeprom_state() == vdm::kEepStateWriteFailed);
  CHECK(eeprom_free());
}

TEST_CASE("wave-0 functions: slot changes mark the sensors, no calibration records, no counters") {
  glue::begin();
  eeprom_changed_slot(3);
  CHECK(eep_content.status == EEP_CHANGED);
  vdm::CalibRecord rec = {};
  eeprom_store_calib(0, rec);
  CHECK(eeprom_cfg_flags() == 0);
  CHECK(eeprom_cfg_events() == 0);
  CHECK(eeprom_writes() == 0);
  CHECK(eeprom_lease_source() == vdm::kLeaseSourceDefault);
  CHECK(stub::calls.empty());
}
