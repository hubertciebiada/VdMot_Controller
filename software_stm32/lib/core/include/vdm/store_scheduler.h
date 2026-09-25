// When the configuration EEPROM is written and read again. The glue (eepromloop(), once per
// second) runs the steps tick() returns and reports their results. Hardware-free.
//
// - A change is written kDebounceS ticks after the last change, at most kMaxDelayS ticks after
//   the first unsaved one, so a client that repeats its configuration does not wear the EEPROM.
// - A failed write is repeated at the next ticks, kWriteAttempts per change; then the change is
//   given up for now (writeFailed) and retried on a RetryBackoff schedule. A change while
//   writeFailed gets one attempt (the bus is known to be bad).
// - After a failed read nothing is written (RAM holds fallbacks for what could not be read): the
//   read is repeated on the backoff schedule (Reread) and the changes wait.
// - retrying(): the step follows a failure, the glue restarts the I2C bus before it.
#pragma once

#include <stdint.h>

#include "vdm/retry_backoff.h"

namespace vdm {

class StoreScheduler {
 public:
  static constexpr uint16_t kDebounceS = 3;     // write 3 s after the last change
  static constexpr uint16_t kMaxDelayS = 30;    // at most 30 s after the first unsaved change
  static constexpr uint8_t kWriteAttempts = 3;  // per change, one per tick

  explicit StoreScheduler(uint32_t retryFirstS = 30, uint32_t retryMaxS = 3600);

  // result of the start-up read or of a Reread
  void readResult(bool ok);
  // fields (kChanged*, vdm/config_store.h) changed in RAM
  void changed(uint16_t fields);

  enum class Step : uint8_t { None, Write, Reread };
  Step tick();
  // the step tick() returned follows a failed read or write
  bool retrying() const;
  // the fields the Write step has to store
  uint16_t dirty() const { return dirty_; }
  void writeResult(bool ok);

  bool readFailed() const { return readFailed_; }
  bool writeFailed() const { return writeFailed_; }
  // gstat eepState: kEepStateReadFailed, kEepStatePending, kEepStateWriteFailed, kEepStateOk
  uint8_t eepState() const;
  // a reset may happen now: no write is waiting (or none can happen until a read succeeds)
  bool free() const;

 private:
  RetryBackoff backoff_;
  uint16_t dirty_ = 0;
  uint16_t sinceChange_ = 0;
  uint16_t sinceFirst_ = 0;
  uint8_t attempts_ = 0;      // failed attempts of the pending change
  bool pending_ = false;      // a change waits for its write (debounce or attempts left)
  bool readFailed_ = false;
  bool writeFailed_ = false;  // the attempts failed, the backoff retries the write
};

}  // namespace vdm
