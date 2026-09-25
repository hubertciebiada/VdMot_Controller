// Smoke tests of src/communication.cpp (glue_communication): today's v1 replies byte-exact against
// stub values, and the line handling of communication_loop().
#include "communication.h"
#include "glue_test.h"
#include "stub_app.h"
#include "stub_eeprom.h"
#include "stub_motor.h"
#include "stub_owDevices.h"
#include "stub_sysstat.h"

namespace {

void begin() {
  glue::begin();
  Serial1.begin(115200, SERIAL_8E1);  // the boot window (BootSetup) opened the port
  communication_setup();
  fake::takeTx(Serial1);
  fake::takeTx(Serial6);
  stub::calls.clear();
}

// one loop run; what went to the ESP
std::string request(const std::string& line) {
  fake::inject(Serial1, line);
  communication_loop();
  return fake::takeTx(Serial1);
}

}  // namespace

TEST_CASE("communication_setup: USART1 on PA10/PA9 at 115200 8N1, bytes of the boot window dropped") {
  glue::begin();
  Serial1.begin(115200, SERIAL_8E1);
  fake::inject(Serial1, "\xFF\x12garbage");
  communication_setup();
  CHECK(Serial1.rxPin == PA10);
  CHECK(Serial1.txPin == PA9);
  CHECK(Serial1.baud == 115200);
  CHECK(Serial1.config == SERIAL_8N1);
  CHECK(Serial1.available() == 0);
  CHECK(fake::takeTx(Serial6) == "SERIAL_BUFFER_SIZE TX=1024 RX=1024\r\n");
  CHECK(fake::takeTx(Serial1).empty());
}

TEST_CASE("v1 replies: gvers, gproto, gtgtp, stgtp") {
  begin();
  CHECK(request("gvers\n") == "gvers 2.1.0-revamped_C2 1 \r\n");
  CHECK(request("gproto\n") == "gproto 3\r\n");
  myvalvemots[0].target_position = 30;
  CHECK(request("gtgtp 0\n") == "gtgtp 0 30 \r\n");
  CHECK(request("stgtp 0 50\n") == "stgtp\r\n");
  CHECK(+myvalvemots[0].target_position == 50);
  CHECK(request("gtgtp 0\n") == "gtgtp 0 50 \r\n");
  CHECK(stub::calls == stub::Calls{"app_target_changed(0)"});
}

TEST_CASE("v1 replies: gvlvd with the valve and its sensors") {
  begin();
  myvalvemots[0].actual_position = 30;
  myvalvemots[0].meancurrent = 21;
  myvalvemots[0].status = VLV_STATE_IDLE;
  myvalvemots[0].calibration = 1;
  myvalvemots[0].opening_count = 3600;
  myvalvemots[0].closing_count = 3610;
  myvalvemots[0].deadzone_count = 10;
  myvalvemots[0].calibRetries = 1;
  myvalves[0].sensorindex1 = 2;
  myvalves[0].sensorindex2 = VALVE_SENSOR_UNKNOWN;
  myvalves[0].movements = 7;
  tempsensors[2].temperature = 215;
  CHECK(request("gvlvd 0\n") == "gvlvd 0 30 21 129 215 -500 7 3600 3610 10 1 \r\n");
  CHECK(request("gvlvd 12\n").empty());
}

TEST_CASE("v1 replies: gstat counts the dropped and malformed lines") {
  begin();
  stub::sysstat.uptime = 7;
  stub::sysstat.resets = 2;
  stub::sysstat.reason = vdm::BootReason::Pin;
  stub::eeprom.state = vdm::kEepStatePending;
  CHECK(request("gstat\n") == "gstat 7 2 2 0 0 1\r\n");
  CHECK(request("stgtp 1 2 3 4 5 6\n").empty());  // too many arguments
  CHECK(request("gstat\n") == "gstat 7 2 2 0 1 1\r\n");
}

TEST_CASE("communication_loop: an unknown command and bad arguments get no reply") {
  begin();
  CHECK(request("xyzzy 1\n").empty());
  CHECK(request("stgtp 12 50\n").empty());
  CHECK(request("stgtp 0 101\n").empty());
  CHECK(request("gtgtp\n").empty());
  CHECK(stub::calls.empty());
  CHECK(fake::takeTx(Serial6) ==
        "set target pos\r\ninvalid arguments\r\nset target pos\r\ninvalid arguments\r\n"
        "get target pos\r\ninvalid arguments\r\n");
}

TEST_CASE("communication_loop: at most 4 requests per call, the rest stays queued") {
  begin();
  fake::inject(Serial1, "gtgtp 0\ngtgtp 1\ngtgtp 2\ngtgtp 3\ngtgtp 4\n");
  CHECK(communication_loop() == 0);
  CHECK(fake::takeTx(Serial1) == "gtgtp 0 0 \r\ngtgtp 1 0 \r\ngtgtp 2 0 \r\ngtgtp 3 0 \r\n");
  CHECK(Serial1.available() == 8);
  CHECK(communication_loop() == 0);
  CHECK(fake::takeTx(Serial1) == "gtgtp 4 0 \r\n");
  CHECK(communication_loop() == -1);
}

TEST_CASE("communication_loop: reads at most 512 bytes per call") {
  begin();
  const unsigned before = Serial1.readCalls;
  fake::inject(Serial1, std::string(513, 'a'));
  CHECK(communication_loop() == -1);
  CHECK(Serial1.readCalls - before == 512);
  CHECK(Serial1.available() == 1);
}

TEST_CASE("communication_loop: an idle partial line is dropped after more than 100 ms") {
  begin();
  fake::inject(Serial1, "gtgtp");
  communication_loop();
  fake::advanceMs(100);
  communication_loop();  // exactly 100 ms: kept
  CHECK(request(" 2\n") == "gtgtp 2 0 \r\n");
  fake::inject(Serial1, "gtgtp");
  communication_loop();
  fake::advanceMs(101);
  fake::takeTx(Serial6);
  communication_loop();  // 101 ms: dropped
  CHECK(fake::takeTx(Serial6) == "comm: incomplete line dropped\r\n");
  CHECK(request(" 2\n").empty());
  CHECK(request("gstat\n") == "gstat 0 0 0 0 1 0\r\n");
}
