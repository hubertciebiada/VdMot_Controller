#include "vdm/retry_backoff.h"

namespace vdm {

RetryBackoff::RetryBackoff(uint32_t first, uint32_t max)
    : first_(first == 0 ? 1 : first),
      max_(max < first_ ? first_ : max),
      interval_(0),
      remaining_(0) {}

void RetryBackoff::failed() {
  if (interval_ == 0) {
    interval_ = first_;
  } else {
    interval_ = interval_ > max_ / 2 ? max_ : interval_ * 2;
  }
  remaining_ = interval_;
}

void RetryBackoff::succeeded() {
  interval_ = 0;
  remaining_ = 0;
}

bool RetryBackoff::tick() {
  if (interval_ == 0) return false;
  if (remaining_ > 0) --remaining_;
  return remaining_ == 0;
}

}  // namespace vdm
