// Stub: contract in vdm/mqtt_topics.h; implemented by the core implementer.
#include "vdm/mqtt_topics.h"

namespace vdm {

namespace {
size_t emptyOut(char* out, size_t cap) {
  if (cap) out[0] = '\0';
  return 0;
}
}  // namespace

size_t buildMainTopic(const TopicContext&, char* out, size_t cap) { return emptyOut(out, cap); }
size_t buildSegment(const char*, uint8_t, char* out, size_t cap) { return emptyOut(out, cap); }
bool topicIsCompat(Topic) { return false; }
bool topicRetained(Topic, bool) { return false; }
size_t buildTopic(const TopicContext&, Topic, const char*, char* out, size_t cap) {
  return emptyOut(out, cap);
}
size_t buildTargetCommandTopic(const TopicContext&, const char*, char* out, size_t cap) {
  return emptyOut(out, cap);
}
int parseTargetCommandTopic(const TopicContext&, const char*, size_t,
                            const char[kValveCount][kSegmentMax + 1]) {
  return -1;
}
TargetPayload parseTargetPayload(const char*, size_t, uint8_t&) { return TargetPayload::Empty; }

size_t formatTemp(int32_t, bool, bool, char* out, size_t cap) { return emptyOut(out, cap); }
size_t formatVolt(double, bool, bool, char* out, size_t cap) { return emptyOut(out, cap); }
size_t formatUptime(uint32_t, char* out, size_t cap) { return emptyOut(out, cap); }
size_t formatValveState(uint8_t, bool, char* out, size_t cap) { return emptyOut(out, cap); }
size_t formatSystemState(uint8_t, bool, char* out, size_t cap) { return emptyOut(out, cap); }
size_t formatCalibDate(const LocalTime&, char* out, size_t cap) { return emptyOut(out, cap); }
size_t formatLegacyCounter(uint32_t, char* out, size_t cap) { return emptyOut(out, cap); }

void PublishScheduler::configure(const Params& p) { p_ = p; }
void PublishScheduler::onConnected(uint32_t) {}
bool PublishScheduler::takeFullPublish(uint32_t) { return false; }
bool PublishScheduler::takeItem(uint8_t, bool, uint32_t) { return false; }
void PublishScheduler::markAllPublished(uint32_t) {}

}  // namespace vdm
