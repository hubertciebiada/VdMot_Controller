// System health: heap and stack alarms, the OTA self-check answer, reset
// reasons. Hardware-free.
#pragma once

#include <stddef.h>
#include <stdint.h>

#include "vdm/event_log.h"

namespace vdm {

// A task's stack is low below max(512, stackBytes / 8) free bytes.
uint32_t stackLowThreshold(uint32_t stackBytes);

class ResourceMonitor {
 public:
  static constexpr uint32_t kLowHeapBytes = 30 * 1024;
  static constexpr uint32_t kLowLargestBlockBytes = 8 * 1024;  // 2x Update's 4 KB buffer
  static constexpr uint32_t kRepeatMs = 3600000;
  static constexpr uint8_t kMaxTasks = 8;
  // Every 10 s. LowHeap (free < kLowHeapBytes; arg1 free, arg2 min free) and
  // HeapFragmented (largest < kLowLargestBlockBytes; arg1 largest, arg2
  // free), each at most once per kRepeatMs, LowHeap first. Tracks the
  // minimum largest block. Returns the events written to `out`.
  size_t onHeap(uint32_t freeHeap, uint32_t minFreeHeap, uint32_t largestBlock, uint32_t nowMs,
                Event* out, size_t maxOut);
  // StackLow once per task index per boot when minFree < stackLowThreshold(stackBytes);
  // text = name (truncated to kEventTextMax). task >= kMaxTasks is ignored.
  size_t onStack(uint8_t task, const char* name, uint32_t stackBytes, uint32_t minFreeBytes,
                 Event* out, size_t maxOut);
  uint32_t minLargestBlock() const { return minLargest_; }  // 0 before the first onHeap()

 private:
  bool heapReported_ = false;
  uint32_t heapReportMs_ = 0;
  bool fragReported_ = false;
  uint32_t fragReportMs_ = 0;
  uint32_t minLargest_ = 0;
  bool sampled_ = false;
  uint8_t stackReported_ = 0;  // bit per task
};

// OTA self-check: the status line starts "HTTP/1.0 200" or "HTTP/1.1 200"
// followed by the end, ' ' or '\r'.
bool httpStatusOk(const char* line, size_t len);

// ESP-IDF esp_reset_reason_t values that point at a crash or a power
// problem: PANIC 4, INT_WDT 5, TASK_WDT 6, WDT 7, BROWNOUT 9.
bool isAbnormalReset(int reason);

}  // namespace vdm
