// Smoke tests of src/app.cpp: boot sequence, tasks, command queue, snapshot plumbing, app task.
#include <Arduino.h>

#include <vdm/version.h>

#include "app.h"
#include "glue_test.h"

namespace {

// The app task function created by app::setup().
TaskFunction_t appTaskFn() {
  for (const fakes::TaskRecord& t : fakes::rtos().tasks) {
    if (t.name == "app") return t.fn;
  }
  return nullptr;
}

// Runs the app task for `passes` loop passes (100 ms each).
void runAppTask(long passes) {
  fakes::rtos().stopAfterYields(passes);
  CHECK_THROWS_AS(appTaskFn()(nullptr), fakes::YieldLimit);
}

}  // namespace

TEST_CASE("app setup: the boot sequence") {
  glue::begin();
  app::setup();
  const std::vector<std::string> order = {
      "serial0.begin 115200 0x800001c -1 -1", "stm_link.begin", "logger.begin",
      "pinMode 2=INPUT_PULLUP", "delay 2", "storage.beginFs", "storage.loadConfig",
      "storage.setActiveConfig", "logger.configure", "stm_service.begin", "net.begin", "ota.begin",
      "mqtt.begin", "esp_task_wdt_init 30 1", "task stm", "task app", "task mqtt"};
  int at = -1;
  for (const std::string& step : order) {
    const int i = fakes::find(step, at + 1);
    INFO(step);
    CHECK(i > at);
    at = i;
  }
}

TEST_CASE("app setup: tasks of the binding table") {
  glue::begin();
  app::setup();
  const std::vector<fakes::TaskRecord>& t = fakes::rtos().tasks;
  REQUIRE(t.size() == 3);
  CHECK(t[0].name == "stm");
  CHECK(t[0].stackBytes == 6144);
  CHECK(t[0].priority == 5);
  CHECK(t[0].core == 1);
  CHECK(t[1].name == "app");
  CHECK(t[1].stackBytes == 6144);
  CHECK(t[1].priority == 3);
  CHECK(t[2].name == "mqtt");
  CHECK(t[2].stackBytes == 8192);
  CHECK(t[2].priority == 2);
  CHECK(t[2].core == 1);
}

TEST_CASE("app setup: the boot event carries the reset reason, the boot count and the version") {
  glue::begin();
  fakes::esp().resetReason = ESP_RST_SW;
  sib::storage().bootCount = 41;
  app::setup();
  const vdm::Event e = sib::logger().withCode(vdm::EventCode::Boot).at(0);
  CHECK(e.arg1 == ESP_RST_SW);
  CHECK(e.arg2 == 42);
  CHECK(e.severity == vdm::Severity::Info);
  CHECK(std::string(e.text) == vdm::firmwareVersion());
}

TEST_CASE("app setup: a panic reset makes the boot event a warning") {
  glue::begin();
  fakes::esp().resetReason = ESP_RST_PANIC;
  app::setup();
  CHECK(sib::logger().withCode(vdm::EventCode::Boot).at(0).severity == vdm::Severity::Warning);
}

TEST_CASE("app setup: GPIO2 held low for a second resets the configuration") {
  glue::begin();
  fakes::gpio().in[2] = LOW;
  app::setup();
  CHECK(sib::storage().factoryResets == 1);
  CHECK(fakes::find("delay 1000") > fakes::find("delay 2"));
  const vdm::Event e = sib::logger().withCode(vdm::EventCode::ConfigSaved).at(0);
  CHECK(std::string(e.text) == "factory");
  CHECK(e.arg2 == 0);
}

TEST_CASE("app setup: GPIO2 released within the second does not reset") {
  glue::begin();
  fakes::gpio().input = [](int pin, uint64_t now) { return pin == 2 && now < 500 ? LOW : -1; };
  app::setup();
  CHECK(sib::storage().factoryResets == 0);
  CHECK_FALSE(sib::logger().has(vdm::EventCode::ConfigSaved));
}

TEST_CASE("app setup: a formatted and a failed file system are logged") {
  glue::begin();
  sib::storage().formatted = true;
  sib::storage().beginFsResult = false;
  app::setup();
  const std::vector<vdm::Event> ev = sib::logger().withCode(vdm::EventCode::FsFormatted);
  REQUIRE(ev.size() == 2);
  CHECK(ev[0].arg1 == 0);
  CHECK(ev[1].arg1 == -1);
}

TEST_CASE("app submit: the queue takes 16 commands, receive hands them out in order") {
  glue::begin();
  app::Command c;
  CHECK_FALSE(app::submit(c));  // before setup: no queue
  app::setup();
  for (uint8_t i = 0; i < 16; ++i) {
    c.valve = i;
    CHECK(app::submit(c));
  }
  CHECK_FALSE(app::submit(c));
  app::Command out;
  for (uint8_t i = 0; i < 16; ++i) {
    REQUIRE(app::receive(out));
    CHECK(out.valve == i);
  }
  CHECK_FALSE(app::receive(out));
}

TEST_CASE("app snapshot: a published snapshot is what the accessors return") {
  glue::begin();
  app::setup();
  static app::StmSnapshot s;
  s.revision = 9;
  s.link = vdm::LinkState::Up;
  s.proto = 2;
  s.support = vdm::StmSupport::Supported;
  s.flash.phase = vdm::FlashPhase::Erasing;
  s.hwId = 0x431;
  app::publishStmSnapshot(s);
  CHECK(app::stmSnapshotRevision() == 9);
  CHECK(app::stmLinkState() == vdm::LinkState::Up);
  CHECK(app::stmProtocol() == 2);
  CHECK(app::stmSupport() == vdm::StmSupport::Supported);
  CHECK(app::stmFlashActive());
  static app::StmSnapshot out;
  app::readStmSnapshot(out);
  CHECK(out.hwId == 0x431);
  s.flash.phase = vdm::FlashPhase::Done;
  app::publishStmSnapshot(s);
  CHECK_FALSE(app::stmFlashActive());
}

TEST_CASE("app: save state, calibration info and flash mark") {
  glue::begin();
  CHECK(app::stmSaveState() == vdm::StmSaveState::Idle);
  app::requestStmSave();
  CHECK(app::stmSaveState() == vdm::StmSaveState::Waiting);
  app::setStmSaveState(vdm::StmSaveState::Saved);
  CHECK(app::stmSaveState() == vdm::StmSaveState::Saved);
  app::CalibInfo ci;
  ci.lastScheduledEpoch = 5;
  ci.nextSlot = 20261001;
  ci.nextEpoch = 7;
  app::setCalibInfo(ci);
  CHECK(app::calibInfo().nextSlot == 20261001);
  CHECK(app::calibInfo().nextEpoch == 7);
  CHECK_FALSE(app::stmFlashActive());
  app::markStmFlashActive();
  CHECK(app::stmFlashActive());
}

TEST_CASE("app readHealth: version, uptime and heap") {
  glue::begin();
  fakes::advanceMs(12345);
  fakes::esp().freeHeap = 1000;
  fakes::esp().minFreeHeap = 900;
  fakes::esp().maxAllocHeap = 800;
  vdm::HealthSnapshot h;
  app::readHealth(h);
  CHECK(std::string(h.version) == vdm::firmwareVersion());
  CHECK(h.uptimeS == 12);
  CHECK(h.freeHeap == 1000);
  CHECK(h.minFreeHeap == 900);
  CHECK(h.largestFreeBlock == 800);
  CHECK(app::nowMs() == 12345);
}

TEST_CASE("app task: once a second the network, the web server, OTA and the schedule") {
  glue::begin();
  app::setup();
  sib::net().up = true;
  sib::net().otaNetOk = true;
  sib::mqtt().status.state = vdm::MqttState::Connected;
  const uint32_t t0 = static_cast<uint32_t>(fakes::nowMs());  // setup waited 2 ms for GPIO2
  runAppTask(11);  // 1.1 s
  REQUIRE(sib::net().services.size() == 1);
  CHECK(sib::net().services[0].nowMs == t0 + 1000);
  CHECK(sib::net().services[0].mqttConnected);
  CHECK(sib::web().begins == 1);
  REQUIRE(sib::ota().services.size() == 1);
  CHECK(sib::ota().services[0].netOk);
  CHECK(sib::ota().services[0].webStarted);
  CHECK(sib::stmService().services == std::vector<uint32_t>{t0 + 1000});
  CHECK(sib::logger().services.size() == 11);
  CHECK(sib::storage().services == 11);
  CHECK(sib::ota().serviceRestarts.size() == 11);
  CHECK(fakes::rtos().delays.back() == 100);
  CHECK(fakes::esp().wdtAdds == 1);
}

TEST_CASE("app task: a new config revision reconfigures the logger and the network") {
  glue::begin();
  app::setup();
  vdm::Config c = sib::storage().active;
  vdm::copyString(c.station, sizeof c.station, "Boiler");
  char path[32];
  REQUIRE(storage::applyConfig(c, path, sizeof path));
  runAppTask(1);
  REQUIRE(sib::net().reconfigures.size() == 1);
  CHECK(std::string(sib::net().reconfigures[0].station) == "Boiler");
  CHECK(sib::logger().configures.back().hostname == "Boiler");
}

TEST_CASE("app task: low heap is reported below 30 KiB, again after an hour") {
  glue::begin();
  app::setup();
  fakes::esp().freeHeap = 30 * 1024;
  runAppTask(11);
  CHECK_FALSE(sib::logger().has(vdm::EventCode::LowHeap));
  fakes::esp().freeHeap = 30 * 1024 - 1;
  fakes::esp().minFreeHeap = 20000;
  runAppTask(10);
  const std::vector<vdm::Event> ev = sib::logger().withCode(vdm::EventCode::LowHeap);
  REQUIRE(ev.size() == 1);
  CHECK(ev[0].arg1 == 30 * 1024 - 1);
  CHECK(ev[0].arg2 == 20000);
  fakes::advanceMs(3600000 - 2000);
  runAppTask(10);
  CHECK(sib::logger().withCode(vdm::EventCode::LowHeap).size() == 1);
  runAppTask(10);
  CHECK(sib::logger().withCode(vdm::EventCode::LowHeap).size() == 2);
}
