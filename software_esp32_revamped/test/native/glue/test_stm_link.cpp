// Tests of src/stm_link.cpp: UART and NRST wiring, bounded reads, the task loop against the fake
// STM, the port of the STM session (stm_service hand-over, config trust, flash image).
#include <Arduino.h>
#include <LittleFS.h>

#include <string.h>

#include <algorithm>

#include "fake_stm.h"
#include "glue_test.h"
#include "stm_link.h"

namespace {

// Runs the STM task for `ms` of fake time (2 ms per loop pass).
void runTask(uint32_t ms) {
  fakes::rtos().stopAfterYields(ms / 2);
  CHECK_THROWS_AS(stm_link::task(nullptr), fakes::YieldLimit);
}

void activeValve(uint8_t v) {
  sib::storage().active.valves[v].active = true;
  ++sib::storage().revision;
}

}  // namespace

TEST_CASE("stm_link begin: Serial2 8N1 at 115200 on pins 5/17 with 2048/512-byte buffers") {
  glue::begin();
  stm_link::begin();
  const fakes::SerialPort& p = fakes::serial(2);
  CHECK(p.open);
  CHECK(p.baud == 115200);
  CHECK(p.config == SERIAL_8N1);
  CHECK(p.rxPin == 5);
  CHECK(p.txPin == 17);
  CHECK(p.rxBufferSize == 2048);
  CHECK(p.txBufferSize == 512);
  CHECK(p.ends == 1);  // closed first, so the buffer sizes apply
  CHECK(fakes::gpio().level[15] == LOW);
  CHECK(fakes::gpio().mode[15] == OUTPUT);
}

TEST_CASE("stm_link releaseReset: BOOT0 low before NRST is released, both driven") {
  glue::begin();
  stm_link::releaseReset();
  CHECK(fakes::journal() == std::vector<std::string>{"gpio 14=0", "pinMode 14=OUTPUT",
                                                     "gpio 15=0", "pinMode 15=OUTPUT"});
}

TEST_CASE("stm_link pulseReset: NRST high for 100 ms") {
  glue::begin();
  stm_link::pulseReset();
  CHECK(fakes::journal() == std::vector<std::string>{"gpio 15=1", "gpio 15=0"});
  CHECK(fakes::rtos().delays == std::vector<uint32_t>{100});
  CHECK(fakes::nowMs() == 100);
}

TEST_CASE("stm_link task: the link comes up with a protocol 1 STM within 10 s") {
  glue::begin();
  glue::FakeStm stm;
  stm.protocol(1);
  stm_link::begin();
  runTask(10000);
  CHECK(sib::app().published.link == vdm::LinkState::Up);
  CHECK(sib::app().proto == 1);
  CHECK_FALSE(stm.requestsOf("gvers").empty());
  CHECK(fakes::esp().wdtAdds == 1);
}

TEST_CASE("stm_link task: the link comes up with a protocol 2 STM within 10 s") {
  glue::begin();
  glue::FakeStm stm;
  stm.protocol(2);
  stm_link::begin();
  runTask(10000);
  CHECK(sib::app().published.link == vdm::LinkState::Up);
  CHECK(sib::app().proto == 2);
  CHECK(sib::app().published.haveStatus);
}

TEST_CASE("stm_link task: the link comes up with a protocol 3 STM within 10 s") {
  glue::begin();
  glue::FakeStm stm;
  stm.protocol(3);
  stm_link::begin();
  runTask(10000);
  CHECK(sib::app().published.link == vdm::LinkState::Up);
  CHECK(stm.requestsOf("gproto").size() >= 1);
  CHECK(sib::app().proto == 3);
}

TEST_CASE("stm_link task: a silent STM is reset by the link policy") {
  glue::begin();
  glue::FakeStm stm;
  stm.silent(true);
  stm_link::begin();
  runTask(70000);
  CHECK(stm.resets() == 1);
  const int high = fakes::find("gpio 15=1");
  REQUIRE(high >= 0);
  CHECK(fakes::find("gpio 15=0", high) == high + 1);
  CHECK(std::count(fakes::rtos().delays.begin(), fakes::rtos().delays.end(), 100u) == 1);
  CHECK(sib::logger().has(vdm::EventCode::StmResetByPolicy));
  CHECK(sib::app().published.link != vdm::LinkState::Up);
}

TEST_CASE("stm_link task: a target command becomes an stgtp line") {
  glue::begin();
  activeValve(2);
  glue::FakeStm stm;
  stm_link::begin();
  app::Command c;
  c.type = app::CommandType::SetTarget;
  c.valve = 2;
  c.pos = 40;
  c.source = vdm::TargetSource::Web;
  // the command arrives while the task runs (a restarted task would hold the link again)
  fakes::rtos().onDelay = [&c](uint32_t) {
    if (fakes::nowMs() == 8000) sib::app().toReceive.push_back(c);
  };
  runTask(9000);
  CHECK(sib::app().published.link == vdm::LinkState::Up);
  const std::vector<std::string> sent = stm.requestsOf("stgtp");
  REQUIRE_FALSE(sent.empty());
  CHECK(sent.back() == "stgtp 2 40 ");
}

TEST_CASE("stm_link task: a user reset pulses NRST and logs it") {
  glue::begin();
  glue::FakeStm stm;
  stm_link::begin();
  app::Command c;
  c.type = app::CommandType::ResetStm;
  fakes::rtos().onDelay = [&c](uint32_t) {
    if (fakes::nowMs() == 6000) sib::app().toReceive.push_back(c);
  };
  runTask(6010);
  CHECK(stm.resets() == 1);
  CHECK(sib::logger().has(vdm::EventCode::StmResetByUser));
}

namespace {

// Runs the STM task with `cmd` arriving at fake time `atMs`.
void runWith(const app::Command& c, uint64_t atMs, uint32_t ms) {
  fakes::rtos().onDelay = [c, atMs](uint32_t) {
    if (fakes::nowMs() == atMs) sib::app().toReceive.push_back(c);
  };
  runTask(ms);
}

// A 4 KiB image with vectors, the handshake strings and the board marker `tag`.
std::string image(const std::string& tag) {
  std::string img(4096, '\x80');
  const uint32_t sp = 0x20020000u, pc = 0x080001C5u;
  memcpy(&img[0], &sp, 4);
  memcpy(&img[4], &pc, 4);
  const std::string strs = std::string("\x01" "DEADBEEF\0\x01" "BEEFIT\0\x01", 18) + "VDM-HW:" + tag +
                           std::string(1, '\0');
  img.replace(2000, strs.size(), strs);
  return img;
}

}  // namespace

TEST_CASE("stm_link task: the first request on the UART is gproto after the 5 s hold") {
  glue::begin();
  glue::FakeStm stm;
  stm_link::begin();
  std::string early = "-";
  fakes::rtos().onDelay = [&early](uint32_t) {
    if (fakes::nowMs() == 4990) early = fakes::serial(2).tx;
  };
  runTask(5100);
  CHECK(early.empty());
  const std::string tx = fakes::serial(2).tx;
  REQUIRE(tx.size() >= 9);
  CHECK(tx.substr(0, 9) == "gproto \r\n");
}

TEST_CASE("stm_link task: one pass reads at most 512 bytes from the UART") {
  glue::begin();
  stm_link::begin();
  fakes::serial(2).inject(std::string(600, 'x'));
  fakes::rtos().stopAfterYields(1);
  CHECK_THROWS_AS(stm_link::task(nullptr), fakes::YieldLimit);
  CHECK(fakes::serial(2).rx.size() == 88);
}

TEST_CASE("stm_link task: boot targets from stm_service are restored and logged") {
  glue::begin();
  activeValve(0);
  sib::stmService().bootTargets.valid[0] = true;
  sib::stmService().bootTargets.pos[0] = 33;
  sib::stmService().bootTargets.source[0] = vdm::TargetSource::Mqtt;
  sib::stmService().bootSource = vdm::RestoreSource::Nvs;
  glue::FakeStm stm;
  stm_link::begin();
  runTask(10000);
  const vdm::Event e = sib::logger().withCode(vdm::EventCode::TargetsRestored).at(0);
  CHECK(e.arg1 == 1);
  CHECK(e.arg2 == 2);
  CHECK(sib::app().published.valves[0].desired == 33);
  CHECK(sib::app().published.valves[0].source == vdm::TargetSource::Restored);
  REQUIRE_FALSE(stm.requestsOf("stgtp").empty());
  CHECK(stm.requestsOf("stgtp").front() == "stgtp 0 33 ");  // the fake STM does not keep it
  REQUIRE_FALSE(sib::stmService().storedTargets.empty());
  CHECK(sib::stmService().storedTargets.back().pos[0] == 33);
}

TEST_CASE("stm_link task: an active lease record from stm_service drives the failsafe at once") {
  glue::begin();
  activeValve(0);
  sib::storage().active.failsafe.timeoutMin = 5;
  sib::stmService().bootLeaseValid = true;
  sib::stmService().bootLease.lost = true;
  sib::stmService().bootLease.active = true;
  sib::stmService().bootLease.mask = 0x001;
  sib::stmService().bootLease.lostElapsedMs = 400000;
  sib::mqtt().regulator.mode = vdm::MqttMode::Mqtt;
  glue::FakeStm stm;
  stm_link::begin();
  runTask(10000);
  REQUIRE_FALSE(stm.requestsOf("stgtp").empty());
  CHECK(stm.requestsOf("stgtp").front() == "stgtp 0 50 ");
  REQUIRE_FALSE(sib::stmService().leaseRecords.empty());
  CHECK(sib::stmService().leaseRecords.back().active);
  CHECK(sib::stmService().leaseRecords.size() >= 9);  // once per second
}

TEST_CASE("stm_link task: stored config: the failsafe settings are pushed to a protocol 3 STM") {
  glue::begin();
  sib::storage().loadSource = storage::LoadSource::Stored;
  glue::FakeStm stm;
  stm.protocol(3);
  stm_link::begin();
  runTask(10000);
  CHECK(stm.requestsOf("sfspo") == std::vector<std::string>{"sfspo 255 255 "});
}

TEST_CASE("stm_link task: unsaved default config: nothing is pushed") {
  glue::begin();
  sib::storage().loadSource = storage::LoadSource::Defaults;
  sib::storage().configSaved = false;
  glue::FakeStm stm;
  stm.protocol(3);
  stm_link::begin();
  runTask(10000);
  CHECK_FALSE(stm.requestsOf("glcfg").empty());
  CHECK(stm.requestsOf("sfspo").empty());
  CHECK_FALSE(sib::app().published.lease.configTrusted);
}

TEST_CASE("stm_link task: default config saved since boot, or restored after an error, is trusted") {
  glue::begin();
  sib::storage().loadSource = storage::LoadSource::DefaultsAfterError;
  sib::storage().configSaved = true;
  glue::FakeStm stm;
  stm.protocol(3);
  stm_link::begin();
  runTask(10000);
  CHECK(stm.requestsOf("sfspo").size() == 1);
  CHECK(sib::app().published.lease.configTrusted);
}

TEST_CASE("stm_link task: after-error defaults without a save are not trusted") {
  glue::begin();
  sib::storage().loadSource = storage::LoadSource::DefaultsAfterError;
  sib::storage().configSaved = false;
  glue::FakeStm stm;
  stm.protocol(3);
  stm_link::begin();
  runTask(10000);
  CHECK(stm.requestsOf("sfspo").empty());
}

TEST_CASE("stm_link task: a config revision change is applied in the loop") {
  glue::begin();
  sib::storage().loadSource = storage::LoadSource::Stored;
  glue::FakeStm stm;
  stm.protocol(3);
  stm_link::begin();
  fakes::rtos().onDelay = [](uint32_t) {
    if (fakes::nowMs() == 9000) activeValve(5);
  };
  runTask(12000);
  CHECK(stm.requestsOf("sfspo") == std::vector<std::string>{"sfspo 255 255 ", "sfspo 5 50 "});
}

TEST_CASE("stm_link task: the STM save before an ESP restart reports Saved") {
  glue::begin();
  glue::FakeStm stm;
  stm_link::begin();
  fakes::rtos().onDelay = [](uint32_t) {
    if (fakes::nowMs() == 7000) sib::app().saveState = vdm::StmSaveState::Waiting;
  };
  runTask(9000);
  CHECK_FALSE(stm.requestsOf("eepst").empty());
  CHECK(sib::app().saveStates == std::vector<vdm::StmSaveState>{vdm::StmSaveState::Saved});
}

TEST_CASE("stm_link task: a scheduled calibration result goes to stm_service") {
  glue::begin();
  glue::FakeStm stm;
  stm_link::begin();
  app::Command c;
  c.type = app::CommandType::Calibrate;
  c.valve = vdm::kAllValves;
  c.scheduled = true;
  c.attempt = 3;
  runWith(c, 8000, 9000);
  CHECK(stm.requestsOf("staln") == std::vector<std::string>{"staln 255 "});
  REQUIRE(sib::stmService().calibResults.size() == 1);
  CHECK(sib::stmService().calibResults[0].attempt == 3);
  CHECK(sib::stmService().calibResults[0].ok);
}

TEST_CASE("stm_link task: a flash of an image for another board is refused before any reset") {
  glue::begin();
  fakes::fs().put("/stm/c1.bin", image("C1"));
  REQUIRE(LittleFS.begin());
  glue::FakeStm stm;
  stm_link::begin();
  app::Command c;
  c.type = app::CommandType::StartFlash;
  memcpy(c.image, "c1", 3);
  runWith(c, 8000, 20000);
  CHECK(sib::app().flashMarks == 1);
  CHECK(stm.resets() == 0);
  const vdm::Event e = sib::logger().withCode(vdm::EventCode::StmFlashFailed).at(0);
  CHECK(e.arg1 == static_cast<int32_t>(vdm::FlashError::BoardMismatch));
  CHECK(sib::logger().has(vdm::EventCode::StmFlashStarted));
  CHECK(fakes::fs().openHandles == 0);
  CHECK(fakes::serial(2).config == SERIAL_8N1);
  CHECK(sib::app().published.link == vdm::LinkState::Up);
}

TEST_CASE("stm_link task: a missing image and a pending restart refuse the flash") {
  glue::begin();
  glue::FakeStm stm;
  stm_link::begin();
  app::Command c;
  c.type = app::CommandType::StartFlash;
  c.blank = true;
  memcpy(c.image, "none", 5);
  runWith(c, 8000, 9000);
  const vdm::Event e = sib::logger().withCode(vdm::EventCode::StmFlashFailed).at(0);
  CHECK(e.arg1 == static_cast<int32_t>(vdm::FlashError::ImageRead));
  CHECK(std::string(e.text) == "none");
  sib::ota().restartPending = true;
  runWith(c, fakes::nowMs() + 100, 1000);
  CHECK(std::string(sib::logger().withCode(vdm::EventCode::StmFlashFailed).back().text) ==
        "restart pending");
}
