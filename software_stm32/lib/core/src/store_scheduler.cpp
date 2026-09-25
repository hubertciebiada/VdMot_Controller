#include "vdm/store_scheduler.h"

#include "vdm/replies_v2.h"

namespace vdm {

StoreScheduler::StoreScheduler(uint32_t retryFirstS, uint32_t retryMaxS) : backoff_(retryFirstS, retryMaxS) {}

void StoreScheduler::readResult(bool ok) {
  readFailed_ = !ok;
  if (ok) {
    backoff_.succeeded();
  } else {
    backoff_.failed();
  }
}

void StoreScheduler::changed(uint16_t fields) {
  dirty_ |= fields;
  if (!pending_) sinceFirst_ = 0;
  pending_ = true;
  sinceChange_ = 0;
}

StoreScheduler::Step StoreScheduler::tick() {
  if (readFailed_) return backoff_.tick() ? Step::Reread : Step::None;
  if (pending_) {
    // after a failed attempt the debounce has passed already: the write is repeated at once
    ++sinceChange_;
    ++sinceFirst_;
    return sinceChange_ >= kDebounceS || sinceFirst_ >= kMaxDelayS ? Step::Write : Step::None;
  }
  return writeFailed_ && backoff_.tick() ? Step::Write : Step::None;
}

bool StoreScheduler::retrying() const { return readFailed_ || writeFailed_ || attempts_ > 0; }

void StoreScheduler::writeResult(bool ok) {
  if (ok) {
    dirty_ = 0;
    pending_ = false;
    attempts_ = 0;
    writeFailed_ = false;
    backoff_.succeeded();
    return;
  }
  // attempts_ is not counted while writeFailed_, the next successful write clears it
  if (writeFailed_ || ++attempts_ >= kWriteAttempts) {
    pending_ = false;
    writeFailed_ = true;
    backoff_.failed();
  }
}

uint8_t StoreScheduler::eepState() const {
  if (readFailed_) return kEepStateReadFailed;
  if (pending_) return kEepStatePending;
  return writeFailed_ ? kEepStateWriteFailed : kEepStateOk;
}

bool StoreScheduler::free() const { return readFailed_ || !pending_; }

}  // namespace vdm
