#include "stub_ds2438.h"

#include <string.h>

namespace stub {

Ds2438 ds2438;

uint64_t ds2438Key(const uint8_t* address) {
  uint64_t key = 0;
  memcpy(&key, address, sizeof key);
  return key;
}

namespace {

void resetDs2438() { ds2438 = Ds2438(); }

Registrar g_registrar(resetDs2438);

}  // namespace

}  // namespace stub

using stub::log;

DS2438::DS2438(OneWire* ow) : _oneWire(ow) {
  memset(_address, 0, sizeof _address);
  _addressFound = false;
}

bool DS2438::begin(uint8_t retries) {
  log("DS2438::begin(%u)", retries);
  return stub::ds2438.begin;
}

void DS2438::setAddress(uint8_t* address) {
  log("DS2438::setAddress(%02x-%02x-%02x-%02x-%02x-%02x-%02x-%02x)", address[0], address[1], address[2],
      address[3], address[4], address[5], address[6], address[7]);
  memcpy(_address, address, sizeof _address);
  _addressFound = true;
}

float DS2438::readVAD() {
  log("DS2438::readVAD()");
  const auto it = stub::ds2438.vad.find(stub::ds2438Key(_address));
  _vad = it == stub::ds2438.vad.end() ? -10 : it->second;
  return _vad;
}
