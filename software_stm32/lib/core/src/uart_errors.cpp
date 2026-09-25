#include "vdm/uart_errors.h"

namespace vdm {

void countUartErrors(UartErrorCounters& c, uint32_t halErrorCode, bool ringFull) {
  if (halErrorCode & kUartErrorOverrun) ++c.overrun;
  if (halErrorCode & kUartErrorFraming) ++c.framing;
  if (halErrorCode & (kUartErrorNoise | kUartErrorParity)) ++c.noise;
  if (ringFull) ++c.dropped;
}

}  // namespace vdm
