// Retry schedule for an operation that failed (e.g. an EEPROM read or write):
// the first retry `first` ticks after the failure, every further one after
// twice the previous interval, at most `max` ticks. Hardware-free.
#pragma once

#include <stdint.h>

namespace vdm {

class RetryBackoff {
 public:
  // first >= 1 and max >= first are enforced.
  RetryBackoff(uint32_t first, uint32_t max);

  // The operation failed (again): schedules the next retry.
  void failed();
  // The operation succeeded: no retry scheduled, the next failure starts
  // with the first interval again.
  void succeeded();

  // One tick passed. Returns true while a retry is due; it stays due until
  // failed() or succeeded() is called.
  bool tick();

  bool pending() const { return interval_ != 0; }
  uint32_t interval() const { return interval_; }

 private:
  uint32_t first_;
  uint32_t max_;
  uint32_t interval_;   // 0: no retry scheduled
  uint32_t remaining_;
};

}  // namespace vdm
