// Edge cases of src/stm_link.cpp: a whole flash through the UART transport, the bounded input
// discard, the command and UART read limits of one pass, the one-second cadence, the interval
// between policy resets.
#include <Arduino.h>
#include <LittleFS.h>

#include <string.h>

#include <algorithm>
#include <string>
#include <vector>

#include "fake_stm.h"
#include "glue_test.h"
#include "stm_link.h"

namespace {

// Runs the STM task for `ms` of fake time (2 ms per loop pass).
void runTask(uint32_t ms) {
  fakes::rtos().stopAfterYields(ms / 2);
  CHECK_THROWS_AS(stm_link::task(nullptr), fakes::YieldLimit);
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

app::Command blankFlash(const char* board) {
  app::Command c;
  c.type = app::CommandType::StartFlash;
  c.blank = true;
  memcpy(c.image, "c2", 3);
  memcpy(c.board, board, strlen(board) + 1);
  return c;
}

// The other end of Serial2 once the flasher opened it at 8E1: `perPoll` bytes arrive on every
// look at the RX side, for `burst` looks (-1: endlessly).
struct Flood : fakes::SerialPeer {
  size_t perPoll = 0;
  long burst = -1;
  bool armed = false;
  int polls = 0;
  void onBegin(uint32_t, uint32_t config) override {
    if (config == SERIAL_8E1) armed = true;
  }
  void onTx(const uint8_t*, size_t) override {}
  void poll() override {
    if (!armed) return;
    ++polls;
    if (burst == 0) return;
    if (burst > 0) --burst;
    fakes::serial(2).inject(std::string(perPoll, 'z'));
  }
};

// Starts a blank flash (board C2, no STM answering) at 1000 ms with `flood` on the UART; returns
// the looks at the RX side and the bytes left in it after the loop pass that opened the UART
// for the bootloader (the flasher discards the input there).
std::pair<int, size_t> discardAgainst(Flood& flood) {
  fakes::fs().put("/stm/c2.bin", image("C2"));
  REQUIRE(LittleFS.begin());
  stm_link::begin();
  fakes::serial(2).peer = &flood;
  const app::Command c = blankFlash("C2");
  std::pair<int, size_t> seen{-1, 0};
  fakes::rtos().onDelay = [&](uint32_t) {
    if (fakes::nowMs() == 1000) sib::app().toReceive.push_back(c);
    if (flood.armed && seen.first < 0) seen = {flood.polls, fakes::serial(2).rx.size()};
  };
  runTask(1100);
  fakes::serial(2).peer = nullptr;
  REQUIRE(flood.armed);
  return seen;
}

}  // namespace

TEST_CASE("stm_link task: a blank flash runs through the UART to the new firmware") {
  glue::begin();
  const std::string img = image("C2");
  fakes::fs().put("/stm/c2.bin", img);
  REQUIRE(LittleFS.begin());
  glue::FakeStm stm;
  stm_link::begin();
  const app::Command c = blankFlash("C2");
  fakes::rtos().onDelay = [&](uint32_t) {
    if (fakes::nowMs() != 8000) return;
    // the link is up with the text protocol; the flash talks to the bootloader simulator
    REQUIRE(sib::app().published.link == vdm::LinkState::Up);
    stm.useSimulator(true);
    stm.sim().bootPinResets = 1;
    stm.sim().appReply = "gvers 2.1.0-revamped_C2 1712345678 ";
    sib::app().toReceive.push_back(c);
  };
  runTask(90000);
  CHECK_FALSE(sib::logger().has(vdm::EventCode::StmFlashFailed));
  REQUIRE(sib::logger().withCode(vdm::EventCode::StmFlashDone).size() == 1);
  CHECK(std::equal(img.begin(), img.end(), stm.sim().flash.begin(),
                   [](char a, uint8_t b) { return static_cast<uint8_t>(a) == b; }));
  CHECK(stm.sim().configs.back() == std::make_pair(uint32_t{115200}, false));
}

TEST_CASE("stm_link flash: the input discard reads at most 64 times 64 bytes") {
  glue::begin();
  Flood flood;
  flood.perPoll = 40;
  const std::pair<int, size_t> seen = discardAgainst(flood);
  CHECK(seen.first == 128);    // available() and read() per round
  CHECK(seen.second == 1024);  // 64 rounds of 80 bytes in, 64 out
}

TEST_CASE("stm_link flash: the input discard stops once nothing is left, even after one byte") {
  glue::begin();
  Flood flood;
  flood.perPoll = 1;
  flood.burst = 1;
  const std::pair<int, size_t> seen = discardAgainst(flood);
  CHECK(seen.first == 3);
  CHECK(seen.second == 0);
}

TEST_CASE("stm_link task: one pass takes at most 4 commands") {
  glue::begin();
  stm_link::begin();
  for (uint8_t i = 0; i < 6; ++i) {
    app::Command c;
    c.type = app::CommandType::SetTarget;
    c.valve = 0;
    c.pos = static_cast<uint8_t>(10 + i);
    sib::app().toReceive.push_back(c);
  }
  fakes::rtos().stopAfterYields(1);
  CHECK_THROWS_AS(stm_link::task(nullptr), fakes::YieldLimit);
  CHECK(sib::app().toReceive.size() == 2);
}

TEST_CASE("stm_link task: a single byte read on its own is part of the reply") {
  glue::begin();
  glue::FakeStm stm;
  stm.protocol(2);
  // passes run at even milliseconds: "2" arrives alone between "gproto " and CR LF
  stm.answer("gproto", [](const std::string&) {
    const uint64_t t = fakes::nowMs();
    fakes::serial(2).inject("gproto ", t + 3);
    fakes::serial(2).inject("2", t + 5);
    fakes::serial(2).inject("\r\n", t + 7);
    return std::string();
  });
  stm_link::begin();
  runTask(10000);
  CHECK(sib::app().proto == 2);
  CHECK(sib::app().published.link == vdm::LinkState::Up);
}

TEST_CASE("stm_link task: the second tick runs every 1000 ms of the loop clock") {
  glue::begin();
  glue::FakeStm stm;
  stm_link::begin();
  std::vector<uint64_t> ticks;
  size_t seen = 0;
  fakes::rtos().onDelay = [&](uint32_t) {
    const uint64_t pass = fakes::nowMs() - 2;  // the pass that just ran
    const size_t n = sib::stmService().leaseRecords.size();
    if (n != seen) ticks.push_back(pass);
    seen = n;
    // from here on the passes run at odd milliseconds
    if (fakes::nowMs() == 1502) fakes::advanceMs(1);
  };
  runTask(3600);
  CHECK(ticks == std::vector<uint64_t>{1000, 2001, 3001});
}

TEST_CASE("stm_link task: the next policy reset waits 10 min from the NRST release") {
  glue::begin();
  std::vector<uint64_t> released;
  fakes::gpio().onWrite = [&released](int pin, uint8_t level) {
    if (pin == 15 && level == LOW) released.push_back(fakes::nowMs());
  };
  glue::FakeStm stm;
  stm.silent(true);
  stm_link::begin();
  runTask(720000);
  REQUIRE(released.size() == 3);  // releaseReset() in begin(), then two policy resets
  CHECK(released[1] > 60000);
  CHECK(released[2] - released[1] >= 600000);
  CHECK(released[2] - released[1] < 610000);
}
