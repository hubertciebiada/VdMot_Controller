#include "stub_boot_jump.h"

void JumpToBootloader(void) {
  stub::log("JumpToBootloader()");
  throw fake::BootloaderJump{};
}
