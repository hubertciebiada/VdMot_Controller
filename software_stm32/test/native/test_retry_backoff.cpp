#include <stdint.h>

#include "doctest.h"
#include "vdm/retry_backoff.h"

using vdm::RetryBackoff;

namespace {

// ticks until the retry is due (tick() returns true), at most limit
uint32_t ticksUntilDue(RetryBackoff& b, uint32_t limit) {
  for (uint32_t n = 1; n <= limit; n++) {
    if (b.tick()) return n;
  }
  return 0;
}

}  // namespace

TEST_CASE("RetryBackoff: nothing is due without a failure") {
  RetryBackoff b(30, 3600);
  CHECK_FALSE(b.pending());
  CHECK(b.interval() == 0);
  CHECK(ticksUntilDue(b, 10000) == 0);
}

TEST_CASE("RetryBackoff: intervals double up to the maximum") {
  RetryBackoff b(30, 200);
  b.failed();
  CHECK(b.pending());
  CHECK(ticksUntilDue(b, 1000) == 30);
  b.failed();
  CHECK(ticksUntilDue(b, 1000) == 60);
  b.failed();
  CHECK(ticksUntilDue(b, 1000) == 120);
  b.failed();
  CHECK(ticksUntilDue(b, 1000) == 200);
  b.failed();
  CHECK(ticksUntilDue(b, 1000) == 200);
}

TEST_CASE("RetryBackoff: a due retry stays due until the caller reports the outcome") {
  RetryBackoff b(2, 10);
  b.failed();
  CHECK_FALSE(b.tick());
  CHECK(b.tick());
  CHECK(b.tick());
  CHECK(b.tick());
  b.succeeded();
  CHECK_FALSE(b.pending());
  CHECK_FALSE(b.tick());
}

TEST_CASE("RetryBackoff: success resets the schedule to the first interval") {
  RetryBackoff b(5, 100);
  b.failed();
  b.failed();
  b.failed();
  CHECK(b.interval() == 20);
  b.succeeded();
  b.failed();
  CHECK(b.interval() == 5);
  CHECK(ticksUntilDue(b, 100) == 5);
}

TEST_CASE("RetryBackoff: degenerate configurations stay usable") {
  RetryBackoff zero(0, 0);
  zero.failed();
  CHECK(zero.interval() == 1);
  CHECK(zero.tick());

  RetryBackoff inverted(50, 10);  // max below first: first wins
  inverted.failed();
  CHECK(inverted.interval() == 50);
  inverted.failed();
  CHECK(inverted.interval() == 50);

  RetryBackoff huge(0x80000001u, 0xFFFFFFFFu);  // doubling must not overflow
  huge.failed();
  huge.failed();
  CHECK(huge.interval() == 0xFFFFFFFFu);
  huge.failed();
  CHECK(huge.interval() == 0xFFFFFFFFu);
}

TEST_CASE("RetryBackoff: doubling stops exactly at half the maximum") {
  // 3 == 7 / 2 is not above half: it doubles to 6, then 6 > 3 jumps to the maximum
  RetryBackoff b(3, 7);
  b.failed();
  CHECK(b.interval() == 3);
  b.failed();
  CHECK(b.interval() == 6);
  b.failed();
  CHECK(b.interval() == 7);

  // an interval between a third and half of the maximum still doubles
  RetryBackoff c(40, 200);
  c.failed();
  c.failed();
  CHECK(c.interval() == 80);
  c.failed();
  CHECK(c.interval() == 160);
  c.failed();
  CHECK(c.interval() == 200);
}
