// Scripted results and recorded calls of the sibling fake of src/logger.cpp
// (siblings/fake_logger.cpp); both files go with src/logger.cpp. See siblings.h.
#pragma once

#include <stddef.h>
#include <stdint.h>

#include <string>
#include <vector>

#include <vdm/event_log.h>
#include <vdm/json_api.h>

#include "logger.h"

namespace sib {

struct Configure {
  uint8_t syslogLevel;
  uint32_t syslogServer;
  uint16_t syslogPort;
  bool persist;
  std::string hostname;
};
struct Logger {
  // recorded, and what read()/readSince()/lastSeq() answer (a real vdm::EventLog of 512)
  std::vector<vdm::Event> events;  // every log()/logSev(), seq assigned from 1
  std::vector<Configure> configures;
  int begins = 0;
  std::vector<bool> services;  // service(netUp)
  int flushes = 0;
  int flushRequests = 0;
  std::vector<std::string> debugLines;
  // scripted
  vdm::LogHealthInfo stats;
  std::vector<vdm::Event> withCode(vdm::EventCode code) const;
  bool has(vdm::EventCode code) const { return !withCode(code).empty(); }
};
Logger& logger();

}  // namespace sib
