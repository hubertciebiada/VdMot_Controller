#include "logger.h"

#include <Arduino.h>
#include <LittleFS.h>
#include <WiFiUdp.h>
#include <freertos/FreeRTOS.h>
#include <freertos/semphr.h>
#include <new>
#include <stdarg.h>
#include <stdio.h>
#include <string.h>
#include <sys/time.h>
#include <time.h>

#include <vdm/log_sink.h>

#include "app.h"
#include "storage.h"

namespace logger {

namespace {

StaticSemaphore_t gMutexStorage;
SemaphoreHandle_t gMutex = nullptr;
// The 24 KB ring is allocated once in begin() (heap, never freed): static
// DRAM is too small for it next to the network stack. It is also the RAM
// buffer of the file sink: the file cursor lags behind it until a flush.
alignas(vdm::EventLog) uint8_t gLogMem[sizeof(vdm::EventLog)];
vdm::EventLog* gLogPtr = nullptr;

// App task only.
uint32_t gSyslogCursor = 0;  // syslog: every event is looked at once
uint32_t gFileCursor = 0;    // file: moved per line, only after it was written
vdm::LogFlushPolicy gPolicy;

// Guarded by the log mutex.
uint8_t gSyslogLevel = 0;
uint32_t gSyslogServer = 0;
uint16_t gSyslogPort = 514;
bool gPersist = true;
char gHostname[vdm::kStationNameMax + 1] = "VdMot";
uint32_t gUrgentSeq = 0;  // the last Warning+ event
bool gFlushRequested = false;
vdm::LogHealthInfo gStats;  // backlog is computed by stats()
uint32_t gStatsCursor = 0;
uint32_t gLastAttemptMs = 0;

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

void sendSyslog(const vdm::Event& e, uint32_t server, uint16_t port, const char* host) {
  if (server == 0 || port == 0) return;
  char msg[120];
  vdm::formatEventMessage(e, msg, sizeof msg);
  char pkt[240];
  const size_t len = vdm::formatSyslog(e, msg, host, pkt, sizeof pkt);
  if (gUdp.beginPacket(IPAddress(server), port)) {
    gUdp.write(reinterpret_cast<const uint8_t*>(pkt), len);
    gUdp.endPacket();
  }
}

// Syslog: at most kPendingLines events per pass, sent while the network is
// up; the cursor always advances (no backlog is kept for syslog).
void serviceSyslog(bool netUp) {
  uint8_t level;
  uint32_t server;
  uint16_t port;
  char host[sizeof gHostname];
  {
    Lock lock;
    level = gSyslogLevel;
    server = gSyslogServer;
    port = gSyslogPort;
    memcpy(host, gHostname, sizeof host);
  }
  const bool on = netUp && level > 0;
  vdm::Event batch[4];
  for (size_t round = 0; round < kPendingLines / 4; ++round) {
    uint32_t next = gSyslogCursor;
    const size_t n = readSince(gSyslogCursor, batch, 4, next);
    if (n == 0) break;
    for (size_t i = 0; i < n; ++i) {
      if (on && vdm::syslogWants(level, batch[i].severity)) sendSyslog(batch[i], server, port, host);
    }
    gSyslogCursor = next;
  }
}

enum Step : int32_t { kStepOk = 0, kStepOpen = 1, kStepWrite = 2, kStepRotate = 3, kStepSize = 4 };

// The log file during one flush attempt.
struct FileSink {
  File file;
  bool rotationBlocked = false;
};

// Appends one line (with '\n'), opening the file lazily and rotating it as
// needed. Returns kStepOk only when the whole line was written.
int32_t appendLine(FileSink& s, const char* line, size_t len) {
  if (!s.file) {
    s.file = LittleFS.open(kLogFile, FILE_APPEND);
    if (!s.file) return kStepOpen;
  }
  for (;;) {
    const vdm::LogFileStep step =
        vdm::logFileStep(s.file.size(), len, s.rotationBlocked, kLogFileMax, kLogFileSlack);
    if (step == vdm::LogFileStep::Append) break;
    if (step == vdm::LogFileStep::Defer) return kStepSize;
    // Rotate (the rename replaces the old file). esp_littlefs refuses to
    // rename an open file (a reader of /api/log): then the file may grow by
    // kLogFileSlack and the rotation is tried again at the next attempt.
    s.file.close();
    s.rotationBlocked = !LittleFS.rename(kLogFile, kLogFileOld);
    s.file = LittleFS.open(kLogFile, FILE_APPEND);
    if (!s.file) return kStepRotate;
  }
  return s.file.write(reinterpret_cast<const uint8_t*>(line), len) == len ? kStepOk : kStepWrite;
}

// Writes the file backlog (events after gFileCursor up to the last seq at
// the start). One open/close per attempt, nothing opened for Debug-only
// backlogs. Events the ring dropped before they were written become a gap
// line, checked per batch: the ring also drops events while the file is
// written.
void writeBacklog(uint32_t nowMs) {
  uint32_t upper;
  {
    Lock lock;
    upper = gLogPtr->lastSeq();
  }
  FileSink sink;
  int32_t failed = kStepOk;
  uint32_t lost = 0;
  vdm::Event batch[4];
  while (failed == kStepOk && gFileCursor < upper) {
    uint32_t next = gFileCursor;
    const size_t n = readSince(gFileCursor, batch, 4, next);
    if (n == 0) break;
    vdm::LogGap gap;
    if (vdm::detectLogGap(gFileCursor, batch[0].seq, gap)) {
      char line[64];
      const size_t len = vdm::formatLogGapLine(gap, line, sizeof line - 1);
      line[len] = '\n';
      failed = appendLine(sink, line, len + 1);
      if (failed != kStepOk) break;
      lost += gap.to - gap.from + 1;
      gFileCursor = gap.to;
    }
    for (size_t i = 0; i < n; ++i) {
      if (vdm::fileWantsSeverity(batch[i].severity)) {
        char line[161];
        const size_t len = vdm::formatEventLine(batch[i], line, sizeof line - 1);
        line[len] = '\n';
        failed = appendLine(sink, line, len + 1);
        if (failed != kStepOk) break;
      }
      gFileCursor = batch[i].seq;
    }
  }
  if (sink.file) sink.file.close();  // LittleFS commits on close
  const bool report = gPolicy.onAttempt(failed == kStepOk, nowMs);
  uint32_t lostTotal;
  {
    Lock lock;
    gStats.lost += lost;
    lostTotal = gStats.lost;
    gStats.flushes = gPolicy.attempts();
    gStats.failures = gPolicy.failures();
    gStats.flushed = true;
    gLastAttemptMs = nowMs;
    gStatsCursor = gFileCursor;
  }
  if (report) {
    log(vdm::EventCode::LogWriteFailed, vdm::kNoValve, failed, static_cast<int32_t>(lostTotal));
  }
}

// File sink of one pass: `force` writes the backlog now (restart path).
void serviceFile(bool force) {
  bool persist, requested;
  uint32_t last, urgentSeq;
  {
    Lock lock;
    persist = gPersist;
    requested = gFlushRequested || force;
    gFlushRequested = false;
    last = gLogPtr->lastSeq();
    urgentSeq = gUrgentSeq;
  }
  if (!persist || !storage::fsReady()) {
    // Nothing to write to: the RAM log is all there is.
    gFileCursor = last;
    Lock lock;
    gStatsCursor = last;
    return;
  }
  const uint32_t backlog = last - gFileCursor;
  if (force ? backlog == 0 : !gPolicy.due(backlog, urgentSeq > gFileCursor, requested, millis())) {
    return;
  }
  writeBacklog(millis());
}

}  // namespace

void begin() {
  if (gMutex != nullptr) return;
  gMutex = xSemaphoreCreateMutexStatic(&gMutexStorage);
  // On allocation failure the log still works with capacity 0 (every event
  // counted as dropped) and the serial mirror.
  vdm::Event* storage = new (std::nothrow) vdm::Event[kEventCapacity];
  gLogPtr = new (gLogMem) vdm::EventLog(storage, storage ? kEventCapacity : 0);
  gPolicy.begin(millis());
}

uint32_t log(const vdm::Event& in) {
  vdm::Event e = in;
  e.uptimeS = app::uptimeS();
  e.epoch = currentEpoch();
  uint32_t seq;
  {
    Lock lock;
    seq = gLogPtr->append(e);
    if (e.severity >= vdm::Severity::Warning) gUrgentSeq = seq;
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

uint32_t logSev(vdm::EventCode code, vdm::Severity sev, uint8_t valve, int32_t arg1,
                int32_t arg2, const char* text) {
  return log(vdm::makeEvent(code, sev, valve, arg1, arg2, text));
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
  vdm::buildHostname(hostname, gHostname, sizeof gHostname);  // syslog HOSTNAME: no spaces
}

void service(bool netUp) {
  serviceSyslog(netUp);
  serviceFile(false);
}

void flush() { serviceFile(true); }

void requestFlush() {
  Lock lock;
  gFlushRequested = true;
}

vdm::LogHealthInfo stats(uint32_t nowMs) {
  Lock lock;
  vdm::LogHealthInfo s = gStats;
  s.persist = gPersist;
  s.backlog = gLogPtr->lastSeq() - gStatsCursor;
  s.lastFlushAgeS = s.flushed ? vdm::elapsedMs(nowMs, gLastAttemptMs) / 1000 : 0;
  return s;
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
