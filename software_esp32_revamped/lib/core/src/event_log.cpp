// Stub: contract in vdm/event_log.h; implemented by the core implementer.
#include "vdm/event_log.h"

#include "vdm/json_writer.h"

namespace vdm {

const char* severityName(Severity) { return ""; }
bool parseSeverity(const char*, size_t, Severity&) { return false; }
uint8_t syslogSeverity(Severity) { return 7; }
const char* eventCodeName(EventCode) { return "unknown"; }
Severity eventDefaultSeverity(EventCode) { return Severity::Info; }
bool eventIsCalibrationOutcome(EventCode) { return false; }

Event makeEvent(EventCode code, Severity sev, uint8_t valve, int32_t arg1, int32_t arg2,
                const char* text) {
  Event e;
  e.code = code;
  e.severity = sev;
  e.valve = valve;
  e.arg1 = arg1;
  e.arg2 = arg2;
  copyString(e.text, sizeof e.text, text ? text : "");
  return e;
}

EventLog::EventLog(Event* storage, size_t capacity) : buf_(storage), cap_(storage ? capacity : 0) {}

uint32_t EventLog::append(const Event&) { return 0; }
uint32_t EventLog::firstSeq() const { return 0; }
uint32_t EventLog::lastSeq() const { return 0; }
size_t EventLog::read(const EventFilter& f, Event*, size_t, uint32_t& nextSince) const {
  nextSince = f.sinceSeq;
  return 0;
}
bool EventLog::get(uint32_t, Event&) const { return false; }
void EventLog::clear() {}

size_t formatEventMessage(const Event&, char* out, size_t cap) {
  if (cap) out[0] = '\0';
  return 0;
}
size_t formatEventLine(const Event&, char* out, size_t cap) {
  if (cap) out[0] = '\0';
  return 0;
}
bool writeEventJson(JsonWriter&, const Event&) { return false; }

}  // namespace vdm
