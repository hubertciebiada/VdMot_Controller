#include "logger.h"

#include <Arduino.h>
#include <LittleFS.h>
#include <WiFiUdp.h>
#include <freertos/FreeRTOS.h>
#include <freertos/semphr.h>
#include <new>
#include <stdarg.h>
#include <stdio.h>
#include <sys/time.h>
#include <time.h>

#include "app.h"
#include "storage.h"

namespace logger {

namespace {

StaticSemaphore_t gMutexStorage;
SemaphoreHandle_t gMutex = nullptr;
// The 24 KB ring is allocated once in begin() (heap, never freed): static
// DRAM is too small for it next to the network stack.
alignas(vdm::EventLog) uint8_t gLogMem[sizeof(vdm::EventLog)];
vdm::EventLog* gLogPtr = nullptr;

// Sequence numbers of events still to be written to file/syslog.
uint32_t gSinkCursor = 0;

uint8_t gSyslogLevel = 0;
uint32_t gSyslogServer = 0;
uint16_t gSyslogPort = 514;
bool gPersist = true;
char gHostname[vdm::kStationNameMax + 1] = "VdMot";
WiFiUDP gUdp;

class Lock {
 public:
  Lock() {
    begin();  // log() may run before app::setup() called begin()
    xSemaphoreTake(gMutex, portMAX_DELAY);
  }
  ~Lock() { xSemaphoreGive(gMutex); }
  Lock(const Lock&) = delete;
  Lock& operator=(const Lock&) = delete;
};

uint32_t currentEpoch() {
  struct timeval tv;
  gettimeofday(&tv, nullptr);
  // Before SNTP the clock starts at 1970: report "unknown".
  return tv.tv_sec >= 1577836800 ? static_cast<uint32_t>(tv.tv_sec) : 0;
}

bool syslogWants(vdm::Severity s) {
  switch (gSyslogLevel) {
    case 1: return s >= vdm::Severity::Warning;
    case 2: return s >= vdm::Severity::Info;
    case 3: return true;
    default: return false;
  }
}

void writeFileLine(const char* line) {
  if (!gPersist || !storage::fsReady()) return;
  File f = LittleFS.open(kLogFile, FILE_APPEND);
  if (!f) return;
  if (f.size() >= kLogFileMax) {
    f.close();
    LittleFS.remove(kLogFileOld);
    LittleFS.rename(kLogFile, kLogFileOld);
    f = LittleFS.open(kLogFile, FILE_APPEND);
    if (!f) return;
  }
  f.println(line);
  f.close();
}

void sendSyslog(const vdm::Event& e, const char* msg) {
  if (gSyslogServer == 0 || gSyslogPort == 0) return;
  // RFC 5424: <PRI>1 TIMESTAMP HOST APP PROCID MSGID SD MSG ; facility local0 (16).
  char pkt[240];
  const unsigned pri = 16u * 8u + vdm::syslogSeverity(e.severity);
  const int n = snprintf(pkt, sizeof pkt, "<%u>1 - %s vdmot - %s - %s", pri, gHostname,
                         vdm::eventCodeName(e.code), msg);
  if (n <= 0) return;
  const IPAddress server(gSyslogServer);
  if (gUdp.beginPacket(server, gSyslogPort)) {
    gUdp.write(reinterpret_cast<const uint8_t*>(pkt),
               static_cast<size_t>(n) < sizeof pkt ? static_cast<size_t>(n) : sizeof pkt - 1);
    gUdp.endPacket();
  }
}

}  // namespace

void begin() {
  if (gMutex != nullptr) return;
  gMutex = xSemaphoreCreateMutexStatic(&gMutexStorage);
  // On allocation failure the log still works with capacity 0 (every event
  // counted as dropped) and the serial mirror.
  vdm::Event* storage = new (std::nothrow) vdm::Event[kEventCapacity];
  gLogPtr = new (gLogMem) vdm::EventLog(storage, storage ? kEventCapacity : 0);
}

uint32_t log(const vdm::Event& in) {
  vdm::Event e = in;
  e.uptimeS = app::uptimeS();
  e.epoch = currentEpoch();
  uint32_t seq;
  {
    Lock lock;
    seq = gLogPtr->append(e);
  }
  e.seq = seq;
  char line[160];
  vdm::formatEventLine(e, line, sizeof line);
  Serial.println(line);
  return seq;
}

uint32_t log(vdm::EventCode code, uint8_t valve, int32_t arg1, int32_t arg2, const char* text) {
  return log(vdm::makeEvent(code, vdm::eventDefaultSeverity(code), valve, arg1, arg2, text));
}

size_t read(const vdm::EventFilter& f, vdm::Event* out, size_t maxOut, uint32_t& nextSince,
            uint32_t& firstSeq, uint32_t& lastSeq, uint32_t& dropped) {
  Lock lock;
  firstSeq = gLogPtr->firstSeq();
  lastSeq = gLogPtr->lastSeq();
  dropped = gLogPtr->dropped();
  return gLogPtr->read(f, out, maxOut, nextSince);
}

uint32_t lastSeq() {
  Lock lock;
  return gLogPtr->lastSeq();
}

size_t readSince(uint32_t sinceSeq, vdm::Event* out, size_t maxOut, uint32_t& nextSince) {
  vdm::EventFilter f;
  f.sinceSeq = sinceSeq;
  Lock lock;
  return gLogPtr->read(f, out, maxOut, nextSince);
}

void configure(uint8_t syslogLevel, uint32_t syslogServer, uint16_t syslogPort, bool persist,
               const char* hostname) {
  Lock lock;
  gSyslogLevel = syslogLevel;
  gSyslogServer = syslogServer;
  gSyslogPort = syslogPort;
  gPersist = persist;
  vdm::copyString(gHostname, sizeof gHostname, hostname);
}

void service(bool netUp) {
  // Bounded: at most kPendingLines events per call. Events that fell out of
  // the ring meanwhile are skipped (the gap is visible in the file by seq).
  vdm::Event batch[4];
  for (size_t round = 0; round < kPendingLines / 4; ++round) {
    uint32_t next = gSinkCursor;
    const size_t n = readSince(gSinkCursor, batch, 4, next);
    if (n == 0) break;
    for (size_t i = 0; i < n; ++i) {
      char line[160];
      vdm::formatEventLine(batch[i], line, sizeof line);
      writeFileLine(line);
      if (netUp && syslogWants(batch[i].severity)) {
        char msg[120];
        vdm::formatEventMessage(batch[i], msg, sizeof msg);
        sendSyslog(batch[i], msg);
      }
    }
    gSinkCursor = next;
  }
}

void debug(const char* fmt, ...) {
#ifdef VDM_DEV_BUILD
  char buf[160];
  va_list ap;
  va_start(ap, fmt);
  vsnprintf(buf, sizeof buf, fmt, ap);
  va_end(ap);
  Serial.println(buf);
#else
  (void)fmt;
#endif
}

}  // namespace logger
