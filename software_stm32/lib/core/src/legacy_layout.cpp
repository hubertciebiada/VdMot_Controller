#include "vdm/legacy_layout.h"

#include <string.h>

#include "vdm/onewire_check.h"

namespace vdm {

namespace {

// 1.x stores the rom code of a slot in reverse order
void putSlot(uint8_t*& p, const SensorSlot& s) {
  *p++ = s.familycode;
  for (int k = 5; k >= 0; --k) *p++ = s.romcode[k];
  *p++ = s.crc;
}

void getSlot(const uint8_t*& p, SensorSlot& s) {
  s.familycode = *p++;
  for (int k = 5; k >= 0; --k) s.romcode[k] = *p++;
  s.crc = *p++;
}

void putU16(uint8_t*& p, uint16_t v) {
  *p++ = static_cast<uint8_t>(v);
  *p++ = static_cast<uint8_t>(v >> 8);
}

uint16_t getU16(const uint8_t*& p) {
  const uint16_t v = static_cast<uint16_t>(p[0] | (p[1] << 8));
  p += 2;
  return v;
}

}  // namespace

void encodeLegacyLayout(const LegacyLayout& in, uint8_t (&out)[kLegacyImageSize]) {
  uint8_t* p = out;
  *p++ = in.b_slave;
  memcpy(p, in.descr, sizeof(in.descr));
  p += sizeof(in.descr);
  memcpy(p, in.OneWireCfg, sizeof(in.OneWireCfg));
  p += sizeof(in.OneWireCfg);
  *p++ = in.currentbound_low_fac;
  *p++ = in.currentbound_high_fac;
  putU16(p, in.numberOfMovements);
  for (const SensorSlot& s : in.owsensors1) putSlot(p, s);
  for (const SensorSlot& s : in.owsensors2) putSlot(p, s);
  for (const SensorSlot& s : in.owsensors) putSlot(p, s);
  *p++ = in.startOnPower;
  putU16(p, in.noOfMinCounts);
  *p = in.maxCalibRetries;
}

void decodeLegacyLayout(const uint8_t (&in)[kLegacyImageSize], LegacyLayout& out) {
  const uint8_t* p = in;
  out.b_slave = *p++;
  memcpy(out.descr, p, sizeof(out.descr));
  p += sizeof(out.descr);
  memcpy(out.OneWireCfg, p, sizeof(out.OneWireCfg));
  p += sizeof(out.OneWireCfg);
  out.currentbound_low_fac = *p++;
  out.currentbound_high_fac = *p++;
  out.numberOfMovements = getU16(p);
  for (SensorSlot& s : out.owsensors1) getSlot(p, s);
  for (SensorSlot& s : out.owsensors2) getSlot(p, s);
  for (SensorSlot& s : out.owsensors) getSlot(p, s);
  out.startOnPower = *p++;
  out.noOfMinCounts = getU16(p);
  out.maxCalibRetries = *p;
}

uint16_t crc16Ccitt(const uint8_t* data, size_t length) {
  uint16_t crc = 0xFFFF;
  for (size_t i = 0; i < length; ++i) {
    crc = static_cast<uint16_t>(crc ^ (data[i] << 8));
    for (uint8_t bit = 0; bit < 8; ++bit) {
      crc = (crc & 0x8000) ? static_cast<uint16_t>((crc << 1) ^ 0x1021) : static_cast<uint16_t>(crc << 1);
    }
  }
  return crc;
}

static_assert(sizeof(SensorSlot) == 8, "a slot holds the 8 bytes of a device address in address order");

bool sensorSlotValid(const SensorSlot& s) {
  uint8_t address[sizeof(SensorSlot)];
  memcpy(address, &s, sizeof(address));

  bool erased = true;
  for (uint8_t b : address) {
    if (b != 0xFF) erased = false;
  }
  return erased || crc8(address, 7) == address[7];
}

}  // namespace vdm
