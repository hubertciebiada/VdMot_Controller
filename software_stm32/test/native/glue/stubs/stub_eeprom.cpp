#include "stub_eeprom.h"

struct eeprom_layout eep_content;

namespace stub {

Eeprom eeprom;

namespace {

void resetEeprom() {
  eeprom = Eeprom();
  eep_content = eeprom_layout();
}

Registrar g_registrar(resetEeprom);

}  // namespace

}  // namespace stub

using stub::eeprom;
using stub::log;

int16_t eepromsetup() {
  log("eepromsetup()");
  return eeprom.setup;
}

int16_t eepromloop() {
  log("eepromloop()");
  return eeprom.loop;
}

int16_t eeprom_write_layout(struct eeprom_layout* lay) {
  log("eeprom_write_layout(%s)", lay == &eep_content ? "&eep_content" : "other");
  return eeprom.writeLayout;
}

int16_t eeprom_read_layout(struct eeprom_layout* lay) {
  log("eeprom_read_layout(%s)", lay == &eep_content ? "&eep_content" : "other");
  return eeprom.readLayout;
}

void eeprom_changed(uint16_t fields) { log("eeprom_changed(0x%04x)", fields); }

void eeprom_changed_slot(uint8_t slot) { log("eeprom_changed_slot(%u)", slot); }

void eeprom_store_calib(uint8_t valve, const vdm::CalibRecord& rec) {
  (void)rec;
  log("eeprom_store_calib(%u)", valve);
}

uint8_t eeprom_cfg_flags(void) {
  log("eeprom_cfg_flags()");
  return eeprom.cfgFlags;
}

uint32_t eeprom_cfg_events(void) {
  log("eeprom_cfg_events()");
  return eeprom.cfgEvents;
}

uint32_t eeprom_writes(void) {
  log("eeprom_writes()");
  return eeprom.writes;
}

uint8_t eeprom_lease_source(void) {
  log("eeprom_lease_source()");
  return eeprom.leaseSource;
}

bool eeprom_free() {
  log("eeprom_free()");
  return eeprom.free;
}

uint8_t eeprom_state() {
  log("eeprom_state()");
  return eeprom.state;
}
