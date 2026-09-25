// State of the sibling fakes (linked into every glue executable, also where the real module
// replaces its fake: the state is then simply unused).
#include "siblings.h"

#include <vdm/event_log.h>

#include "sib_internal.h"

namespace sib {

namespace {

App g_app;
Logger g_logger;
Storage g_storage;
Net g_net;
Ota g_ota;
Mqtt g_mqtt;
StmLink g_stmLink;
StmService g_stmService;
Web g_web;

vdm::Event g_events[logger::kEventCapacity];
vdm::EventLog g_eventLog(g_events, logger::kEventCapacity);

}  // namespace

App& app() { return g_app; }
Logger& logger() { return g_logger; }
Storage& storage() { return g_storage; }
Net& net() { return g_net; }
Ota& ota() { return g_ota; }
Mqtt& mqtt() { return g_mqtt; }
StmLink& stmLink() { return g_stmLink; }
StmService& stmService() { return g_stmService; }
Web& web() { return g_web; }
vdm::EventLog& eventLog() { return g_eventLog; }

void reset() {
  g_app = App{};
  g_logger = Logger{};
  g_storage = Storage{};
  vdm::setDefaults(g_storage.loadedConfig);
  vdm::setDefaults(g_storage.active);
  g_net = Net{};
  g_ota = Ota{};
  g_mqtt = Mqtt{};
  g_stmLink = StmLink{};
  g_stmService = StmService{};
  g_web = Web{};
  g_eventLog.clear();
}

std::vector<vdm::Event> Logger::withCode(vdm::EventCode code) const {
  std::vector<vdm::Event> out;
  for (const vdm::Event& e : events) {
    if (e.code == code) out.push_back(e);
  }
  return out;
}

}  // namespace sib
