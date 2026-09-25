// Logging: structured events (vdm::EventLog in RAM, 512 entries), mirrored
// to the debug serial port, a rotating file log on LittleFS and optional UDP
// syslog (RFC 5424). Thread-safe entry points; slow I/O (file, UDP) happens
// only in service(), called from the app task.
#pragma once

#include <stddef.h>
#include <stdint.h>

#include <vdm/event_log.h>
#include <vdm/json_api.h>

namespace logger {

constexpr size_t kEventCapacity = 512;
// File rotation (binding): /log/events.log up to 64 KB, then renamed to
// /log/events.1.log (replacing it) -> at most 2 x 64 KB on flash.
constexpr const char* kLogFile = "/log/events.log";
constexpr const char* kLogFileOld = "/log/events.1.log";
constexpr size_t kLogFileMax = 64 * 1024;
// A batch may carry the file this far past kLogFileMax before it rotates.
constexpr size_t kLogFileSlack = 8192;
// File/syslog sinks follow the RAM log with a cursor; service() writes at
// most kPendingLines events per call. Events overwritten in the RAM ring
// before service() reached them are skipped (visible as a seq gap).
constexpr size_t kPendingLines = 32;

void begin();

// Records an event (any task, also before the network is up). Fills
// seq/uptime/epoch. Returns the seq.
uint32_t log(const vdm::Event& e);
// Shorthand with the default severity of the code.
uint32_t log(vdm::EventCode code, uint8_t valve = vdm::kNoValve, int32_t arg1 = 0,
             int32_t arg2 = 0, const char* text = nullptr);
// Same with an explicit severity (codes whose severity depends on the args).
uint32_t logSev(vdm::EventCode code, vdm::Severity sev, uint8_t valve = vdm::kNoValve,
                int32_t arg1 = 0, int32_t arg2 = 0, const char* text = nullptr);

// Filtered copy for the API (see vdm::EventLog::read).
size_t read(const vdm::EventFilter& f, vdm::Event* out, size_t maxOut, uint32_t& nextSince,
            uint32_t& firstSeq, uint32_t& lastSeq, uint32_t& dropped);
uint32_t lastSeq();

// Events after `sinceSeq`, oldest first; the MQTT task calls this with its
// own cursor.
size_t readSince(uint32_t sinceSeq, vdm::Event* out, size_t maxOut, uint32_t& nextSince);

// Sink configuration (from Config): syslog level/server/port, file on/off,
// syslog HOSTNAME field (the station name, made a host name with
// vdm::buildHostname).
void configure(uint8_t syslogLevel, uint32_t syslogServer, uint16_t syslogPort, bool persist,
               const char* hostname);

// App task: drains pending lines to the file (rotation) and syslog (when the
// network is up). Bounded work per call.
void service(bool netUp);

// App task, restart path: writes the pending lines to the file now.
void flush();
// Any task: the next service() writes the whole backlog (GET /api/log).
void requestFlush();
// Sink statistics for /api/health.
vdm::LogHealthInfo stats(uint32_t nowMs);

// Debug text (serial only, dev builds): printf-style, truncated at 160 chars.
void debug(const char* fmt, ...) __attribute__((format(printf, 1, 2)));

}  // namespace logger
