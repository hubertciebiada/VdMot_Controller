// The 1.x configuration layout: 309 bytes at EEPROM address 0x0007, written by
// firmware 1.x and 2.x in exactly this byte order (a downgrade to 1.4.x keeps
// reading it), and the checks of its content. Hardware-free.
//
// Byte order: b_slave (1), descr (25), OneWireCfg (3), currentbound_low_fac (1),
// currentbound_high_fac (1), numberOfMovements (2), owsensors1[12],
// owsensors2[12], owsensors[10] (8 each: family, rom[5], rom[4], rom[3],
// rom[2], rom[1], rom[0], crc), startOnPower (1), noOfMinCounts (2),
// maxCalibRetries (1). Multi-byte fields little endian.
#pragma once

#include <stddef.h>
#include <stdint.h>

namespace vdm {

constexpr uint8_t kValveCount = 12;        // == ACTUATOR_COUNT (static_assert in include/hardware.h)
constexpr uint8_t kExtraSensorSlots = 10;  // == ADDITIONAL_SENSOR_COUNT
constexpr size_t kLegacyImageSize = 309;

// One 1-Wire sensor slot: the device address family, rom[0..5], crc. The class
// keeps its 1.x name because the glue also names it `struct
// ds1820_eeprom_layout` (include/hardware.h brings it into the global
// namespace); lib/core uses SensorSlot.
struct ds1820_eeprom_layout {
  uint8_t familycode;  // address byte 0
  uint8_t romcode[6];  // address bytes 1..6
  uint8_t crc;         // address byte 7
};
using SensorSlot = ds1820_eeprom_layout;

// Field names as in the 1.x RAM struct (include/hardware.h), so the glue keeps
// its accesses like eep_content.owsensors1[x].
struct LegacyLayout {
  uint8_t b_slave;     // 0 master, > 0 slave (not used)
  char descr[25];      // system description (not used)
  uint8_t OneWireCfg[3];
  uint8_t currentbound_low_fac;
  uint8_t currentbound_high_fac;
  uint16_t numberOfMovements;
  SensorSlot owsensors1[kValveCount];       // first sensor of each valve
  SensorSlot owsensors2[kValveCount];       // second sensor of each valve
  SensorSlot owsensors[kExtraSensorSlots];  // additional sensors (not used)
  uint8_t startOnPower;
  uint16_t noOfMinCounts;
  uint8_t maxCalibRetries;
};

void encodeLegacyLayout(const LegacyLayout& in, uint8_t (&out)[kLegacyImageSize]);
void decodeLegacyLayout(const uint8_t (&in)[kLegacyImageSize], LegacyLayout& out);

// CRC-16/CCITT-FALSE: polynomial 0x1021, initial value 0xFFFF, not reflected,
// no final xor ("123456789" -> 0x29B1). Protects the 1.x layout (block A).
uint16_t crc16Ccitt(const uint8_t* data, size_t length);

// A slot that was never assigned (all bytes 0xFF), or one whose CRC-8 over
// family and rom[0..5] matches its crc (the all-zero slot of a cleared
// assignment included). Anything else is a torn write or a flipped bit.
bool sensorSlotValid(const SensorSlot& s);

}  // namespace vdm
