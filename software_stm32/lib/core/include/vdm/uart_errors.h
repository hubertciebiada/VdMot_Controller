// Counters of the receive errors of the ESP UART (gstax uartOre, uartFe, uartNe, rxDropped).
// Hardware-free: the glue passes the HAL error code the UART interrupt set for a received byte
// and whether the receive ring had no room for it.
#pragma once

#include <stdint.h>

namespace vdm {

// the HAL_UART_ERROR_* bits (the glue static_asserts them)
constexpr uint32_t kUartErrorParity = 0x01;
constexpr uint32_t kUartErrorNoise = 0x02;
constexpr uint32_t kUartErrorFraming = 0x04;
constexpr uint32_t kUartErrorOverrun = 0x08;

// since start-up; they wrap, consumers use differences
struct UartErrorCounters {
  uint32_t overrun = 0;
  uint32_t framing = 0;
  uint32_t noise = 0;    // noise and parity errors
  uint32_t dropped = 0;  // bytes that found the receive ring full
};

// one received byte: every error bit of halErrorCode counts once (parity counts as noise)
void countUartErrors(UartErrorCounters& c, uint32_t halErrorCode, bool ringFull);

}  // namespace vdm
