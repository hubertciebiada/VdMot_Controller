// Fake of khoih-prog/STM32_TimerInterrupt 1.3.0: attachInterruptInterval() records the interval and
// the callback; the fake time runs the callback of every attached timer when its interval has
// passed (fake::advanceUs, TIM1 before TIM2 within one millisecond). Glue-facing: no STL.
#pragma once

#include <stdint.h>

#include "fake_cmsis.h"

typedef void (*timerCallback)();

class STM32TimerInterrupt {
 public:
  explicit STM32TimerInterrupt(TIM_TypeDef* timer);
  ~STM32TimerInterrupt();
  bool attachInterruptInterval(unsigned long interval, timerCallback cb);
  void detachInterrupt() { callback = nullptr; }

  // ---- fake state
  TIM_TypeDef* timer;
  unsigned long intervalUs = 0;
  timerCallback callback = nullptr;
  bool failAttach = false;  // the next attachInterruptInterval() fails
  unsigned attaches = 0;
  uint64_t lastFireUs = 0;
  unsigned fires = 0;
};

typedef STM32TimerInterrupt STM32Timer;
