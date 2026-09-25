#include "vdm/sys_health.h"

#include <string.h>

#include "vdm/common.h"

namespace vdm {

uint32_t stackLowThreshold(uint32_t stackBytes) {
  const uint32_t eighth = stackBytes / 8;
  return eighth > 512 ? eighth : 512;
}

size_t ResourceMonitor::onHeap(uint32_t freeHeap, uint32_t minFreeHeap, uint32_t largestBlock,
                               uint32_t nowMs, Event* out, size_t maxOut) {
  if (!sampled_ || largestBlock < minLargest_) minLargest_ = largestBlock;
  sampled_ = true;
  if (out == nullptr) return 0;
  size_t n = 0;
  if (n < maxOut && freeHeap < kLowHeapBytes &&
      (!heapReported_ || elapsedMs(nowMs, heapReportMs_) >= kRepeatMs)) {
    heapReported_ = true;
    heapReportMs_ = nowMs;
    out[n++] = makeEvent(EventCode::LowHeap, eventDefaultSeverity(EventCode::LowHeap), kNoValve,
                         static_cast<int32_t>(freeHeap), static_cast<int32_t>(minFreeHeap),
                         nullptr);
  }
  if (n < maxOut && largestBlock < kLowLargestBlockBytes &&
      (!fragReported_ || elapsedMs(nowMs, fragReportMs_) >= kRepeatMs)) {
    fragReported_ = true;
    fragReportMs_ = nowMs;
    out[n++] = makeEvent(EventCode::HeapFragmented, eventDefaultSeverity(EventCode::HeapFragmented),
                         kNoValve, static_cast<int32_t>(largestBlock),
                         static_cast<int32_t>(freeHeap), nullptr);
  }
  return n;
}

size_t ResourceMonitor::onStack(uint8_t task, const char* name, uint32_t stackBytes,
                                uint32_t minFreeBytes, Event* out, size_t maxOut) {
  if (task >= kMaxTasks || out == nullptr || maxOut == 0) return 0;
  const uint8_t bit = static_cast<uint8_t>(1u << task);
  if ((stackReported_ & bit) != 0 || minFreeBytes >= stackLowThreshold(stackBytes)) return 0;
  stackReported_ = static_cast<uint8_t>(stackReported_ | bit);
  out[0] = makeEvent(EventCode::StackLow, eventDefaultSeverity(EventCode::StackLow), kNoValve,
                     static_cast<int32_t>(minFreeBytes), static_cast<int32_t>(stackBytes), name);
  return 1;
}

bool httpStatusOk(const char* line, size_t len) {
  if (line == nullptr || len < 12) return false;
  if (memcmp(line, "HTTP/1.", 7) != 0 || (line[7] != '0' && line[7] != '1')) return false;
  if (memcmp(line + 8, " 200", 4) != 0) return false;
  return len == 12 || line[12] == ' ' || line[12] == '\r';
}

bool isAbnormalReset(int reason) {
  switch (reason) {
    case 4:  // ESP_RST_PANIC
    case 5:  // ESP_RST_INT_WDT
    case 6:  // ESP_RST_TASK_WDT
    case 7:  // ESP_RST_WDT
    case 9:  // ESP_RST_BROWNOUT
      return true;
    default:
      return false;
  }
}

}  // namespace vdm
