#include <stdint.h>

#include "doctest.h"
#include "vdm/uart_errors.h"

using vdm::UartErrorCounters;

namespace {

bool equal(const UartErrorCounters& c, uint32_t overrun, uint32_t framing, uint32_t noise, uint32_t dropped) {
  return c.overrun == overrun && c.framing == framing && c.noise == noise && c.dropped == dropped;
}

}  // namespace

TEST_CASE("countUartErrors: a byte without error counts nothing") {
  UartErrorCounters c;
  vdm::countUartErrors(c, 0, false);
  CHECK(equal(c, 0, 0, 0, 0));
}

TEST_CASE("countUartErrors: each HAL error bit alone") {
  UartErrorCounters c;
  vdm::countUartErrors(c, vdm::kUartErrorOverrun, false);
  CHECK(equal(c, 1, 0, 0, 0));
  vdm::countUartErrors(c, vdm::kUartErrorFraming, false);
  CHECK(equal(c, 1, 1, 0, 0));
  vdm::countUartErrors(c, vdm::kUartErrorNoise, false);
  CHECK(equal(c, 1, 1, 1, 0));
  vdm::countUartErrors(c, vdm::kUartErrorParity, false);  // parity counts as noise
  CHECK(equal(c, 1, 1, 2, 0));
}

TEST_CASE("countUartErrors: combined bits count once each, parity and noise together once") {
  UartErrorCounters c;
  vdm::countUartErrors(c, vdm::kUartErrorOverrun | vdm::kUartErrorFraming | vdm::kUartErrorNoise |
                             vdm::kUartErrorParity, false);
  CHECK(equal(c, 1, 1, 1, 0));
  vdm::countUartErrors(c, 0xFFFFFFF0u, false);  // bits the HAL has no counter for
  CHECK(equal(c, 1, 1, 1, 0));
}

TEST_CASE("countUartErrors: a full ring counts a dropped byte, also with an error") {
  UartErrorCounters c;
  vdm::countUartErrors(c, 0, true);
  CHECK(equal(c, 0, 0, 0, 1));
  vdm::countUartErrors(c, vdm::kUartErrorFraming, true);
  CHECK(equal(c, 0, 1, 0, 2));
}

TEST_CASE("countUartErrors: the counters wrap") {
  UartErrorCounters c;
  c.overrun = UINT32_MAX;
  c.framing = UINT32_MAX;
  c.noise = UINT32_MAX;
  c.dropped = UINT32_MAX;
  vdm::countUartErrors(c, vdm::kUartErrorOverrun | vdm::kUartErrorFraming | vdm::kUartErrorNoise, true);
  CHECK(equal(c, 0, 0, 0, 0));
}

TEST_CASE("uart_errors: the bits of HAL_UART_ERROR_PE, NE, FE and ORE") {
  CHECK(vdm::kUartErrorParity == 0x01);
  CHECK(vdm::kUartErrorNoise == 0x02);
  CHECK(vdm::kUartErrorFraming == 0x04);
  CHECK(vdm::kUartErrorOverrun == 0x08);
}
