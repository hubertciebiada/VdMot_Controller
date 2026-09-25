// Smoke tests of src/stm_link.cpp: UART and NRST wiring, the task loop against the fake STM.
#include <Arduino.h>

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
  CHECK(sib::app().proto == 2);  // the poll planner talks protocol 2 to a protocol 3 STM
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
