#include "runner_hooks.h"

#include "fake_board.h"
#include "testkit.h"

namespace {

using testkit::Region;
using testkit::Reset;

Region noinit() {
  uint8_t* start = nullptr;
  size_t size = 0;
  if (!testkit::sectionBounds(".noinit", start, size)) return {nullptr, 0};
  return {start, size};
}

// The hooks copy the whole section. Objects with __attribute__((noinit)) (src/sysstat.cpp) get ASan
// redzones like any global, objects with __attribute__((section(".noinit"))) do not: these copies
// bypass the ASan checks (the redzone bytes themselves do not matter).
__attribute__((no_sanitize_address)) void rawCopy(volatile uint8_t* dst, const volatile uint8_t* src, size_t n) {
  for (size_t i = 0; i < n; i++) dst[i] = src[i];
}

__attribute__((no_sanitize_address)) void rawFill(volatile uint8_t* dst, uint8_t value, size_t n) {
  for (size_t i = 0; i < n; i++) dst[i] = value;
}

// RCC_CSR flags a reset sets (RM0368: a software or watchdog reset also asserts NRST)
uint32_t resetFlags(Reset kind) {
  switch (kind) {
    case Reset::PowerOn:
      return RCC_CSR_PORRSTF | RCC_CSR_PINRSTF | RCC_CSR_BORRSTF;
    case Reset::Pin:
      return RCC_CSR_PINRSTF;
    case Reset::Software:
      return RCC_CSR_SFTRSTF | RCC_CSR_PINRSTF;
    case Reset::Watchdog:
      return RCC_CSR_IWDGRSTF | RCC_CSR_PINRSTF;
  }
  return 0;
}

void powerOn() {
  const Region section = noinit();
  if (section.size != 0) rawFill(static_cast<uint8_t*>(section.data), 0xA5, section.size);
  fake::powerOn();
}

bool save(const char* path) {
  const Region section = noinit();
  std::vector<uint8_t> warm(section.size);
  if (section.size != 0) rawCopy(warm.data(), static_cast<uint8_t*>(section.data), section.size);
  uint32_t csr = fake_RCC.CSR.value;
  const Region regions[] = {
      {warm.data(), warm.size()}, {fake::eeprom.bytes, sizeof fake::eeprom.bytes}, {&csr, sizeof csr}};
  return testkit::saveRegions(path, regions, 3);
}

bool load(const char* path) {
  const Region section = noinit();
  std::vector<uint8_t> warm(section.size);
  uint32_t csr = 0;
  const Region regions[] = {
      {warm.data(), warm.size()}, {fake::eeprom.bytes, sizeof fake::eeprom.bytes}, {&csr, sizeof csr}};
  if (!testkit::loadRegions(path, regions, 3)) return false;
  if (section.size != 0) rawCopy(static_cast<uint8_t*>(section.data), warm.data(), section.size);
  fake_RCC.CSR.value = (csr & RCC_CSR_RESET_FLAGS) | resetFlags(testkit::lastReset());
  return true;
}

const char* checkInvariants() {
  if (fake::board.primask != 0) return "interrupts left disabled (PRIMASK 1)";
  return nullptr;
}

struct RegisterHooks {
  RegisterHooks() { testkit::setHooks({powerOn, save, load, checkInvariants}); }
} g_registerHooks;

}  // namespace

namespace glue {

std::vector<uint8_t> noinitSnapshot() {
  const Region section = noinit();
  std::vector<uint8_t> out(section.size);
  if (section.size != 0) rawCopy(out.data(), static_cast<uint8_t*>(section.data), section.size);
  return out;
}

}  // namespace glue
