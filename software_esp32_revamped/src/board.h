// WT32-ETH01 v1.2 board wiring of the VdMot controller.
#pragma once

#include <stdint.h>

namespace board {

// STM32 application UART (Serial2). ESP RX <- STM PA9, ESP TX -> STM PA10.
constexpr int kStmRxPin = 5;
constexpr int kStmTxPin = 17;
constexpr uint32_t kStmBaud = 115200;
constexpr int kStmRxBufferSize = 2048;  // >= one 1023-char line + a 256 B bootloader block
constexpr int kStmTxBufferSize = 512;   // one bootloader block + framing never blocks

// STM NRST through an inverting transistor (BC817), jumper X20 must be fitted:
// GPIO HIGH = STM held in reset, LOW = STM running. IO15 is a strapping pin
// with an internal pull-up during ESP reset (the STM may be held in reset
// while the ESP boots; it runs again as soon as setup drives the pin LOW).
constexpr int kStmResetPin = 15;
constexpr bool kStmResetAssertedLevel = true;  // HIGH
// IO14 is not connected to STM BOOT0 on the BlackPill boards; driven LOW and
// otherwise unused.
constexpr int kStmBoot0Pin = 14;

// Factory reset: GPIO2 (strapping pin), INPUT_PULLUP, held LOW for >= 5 s at
// boot -> factory defaults incl. network; once per fitting of the jumper
// (latch in NVS), sampled every kFactoryResetSampleMs.
constexpr int kFactoryResetPin = 2;
constexpr uint32_t kFactoryResetHoldMs = 5000;
constexpr uint32_t kFactoryResetSampleMs = 50;

// LAN8720 PHY (library defaults; spelled out for clarity).
constexpr int kEthPhyAddr = 1;
constexpr int kEthPhyPower = 16;
constexpr int kEthMdc = 23;
constexpr int kEthMdio = 18;

}  // namespace board
