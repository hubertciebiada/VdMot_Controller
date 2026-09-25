// Edge cases of src/terminal.cpp (glue_terminal): the read budget, the argument count and range of
// every command, the answers of stsnx/stsny, stvls, gvlon, staop and staln, failed calls.
#include "glue_test.h"
#include "hardware.h"
#include "stub_app.h"
#include "stub_communication.h"
#include "stub_motor.h"
#include "terminal.h"

namespace {

void begin() {
  glue::begin();
  Terminal_Init();
  fake::takeTx(Serial6);
}

int16_t command(const std::string& line, std::string& out) {
  fake::inject(Serial6, line);
  const int16_t result = Terminal_Serve();
  out = fake::takeTx(Serial6);
  return result;
}

// every argument error of a command gives the same answer and no call
void checkRefused(const std::vector<std::string>& lines, const std::string& answer) {
  for (const std::string& line : lines) {
    CAPTURE(line);
    std::string out;
    stub::calls.clear();
    command(line + "\n", out);
    CHECK(out == answer);
    CHECK(stub::calls.empty());
  }
}

}  // namespace

TEST_CASE("Terminal_Serve: at most 256 bytes are taken per call") {
  begin();
  fake::inject(Serial6, std::string(300, 'x'));
  CHECK(Terminal_Serve() == -1);
  CHECK(Serial6.available() == 44);
}

TEST_CASE("learn: the full 16-bit range, a refused command, bad arguments") {
  begin();
  std::string out;
  CHECK(command("learn 65535\n", out) == CMD_LEARN);
  CHECK(out.empty());
  CHECK(stub::calls == stub::Calls{"appsetaction(l, 65535, 0, 0)"});
  stub::motor.action = 1;
  CHECK(command("learn 3\n", out) == CMD_LEARN);
  CHECK(out == "valve machine command not accepted\r\n");
  stub::motor.action = 0;
  checkRefused({"learn", "learn x", "learn 1 2", "learn 65536"}, "to few arguments\r\n");
}

TEST_CASE("open and close: 100 % is the limit, a refused command, bad arguments") {
  begin();
  std::string out;
  CHECK(command("open 4 100\n", out) == CMD_OPEN);
  CHECK(out.empty());
  CHECK(stub::calls == stub::Calls{"appsetaction(o, 4, 100, 0)"});
  stub::motor.action = 1;
  CHECK(command("open 4 50\n", out) == CMD_OPEN);
  CHECK(out == "valve machine not idle\r\n");
  CHECK(command("close 4 50\n", out) == CMD_CLOSE);
  CHECK(out == "valve machine not idle\r\n");
  stub::motor.action = 0;
  checkRefused({"open 4 101", "open 4", "open 4 50 7", "open x 50", "open 4 x"}, "to few arguments\r\n");
  checkRefused({"close 4 101", "close 4", "close 4 50 7", "close x 50", "close 4 x"}, "to few arguments\r\n");
}

TEST_CASE("settar: the last valve and 100 %, nothing beyond, bad arguments") {
  begin();
  std::string out;
  CHECK(command("settar 11 100\n", out) == CMD_CLOSE);
  CHECK(out == "set valve 11 to 100\r\n");
  CHECK(+myvalvemots[11].target_position == 100);
  myvalvemots[1].target_position = 7;
  for (const char* quiet : {"settar 12 50\n", "settar 1 101\n"}) {
    CAPTURE(quiet);
    CHECK(command(quiet, out) == CMD_CLOSE);
    CHECK(out.empty());
  }
  CHECK(+myvalvemots[1].target_position == 7);
  for (const char* bad : {"settar 1 50 7\n", "settar x 50\n", "settar 1 x\n", "settar 1\n"}) {
    CAPTURE(bad);
    CHECK(command(bad, out) == CMD_CLOSE);
    CHECK(out == "to few arguments\r\n");
  }
  CHECK(+myvalvemots[0].target_position == 0);
  CHECK(+myvalvemots[1].target_position == 7);
}

TEST_CASE("smux, sdir and stdet take exactly one number") {
  begin();
  std::string out;
  fake::board.events.clear();
  for (const char* bad : {"smux 1 2\n", "sdir 1 2\n", "smux x\n", "sdir x\n"}) {
    CAPTURE(bad);
    CHECK(command(bad, out) == 0);
    CHECK(out.empty());
  }
  CHECK(fake::board.events.empty());
  CHECK(command("stdet 255 1\n", out) == 0);
  CHECK(out == "got detect valve status request - error\r\n");
  CHECK(command("stdet x\n", out) == 0);
  CHECK(out == "got detect valve status request - error\r\n");
  CHECK(stub::calls.empty());
}

TEST_CASE("sena: the full 16-bit value switches on") {
  begin();
  std::string out;
  CHECK(command("sena 1 65535\n", out) == 0);
  CHECK(terminal_manual_active());
}

TEST_CASE("stsnx and stsny: slot 1 and 2, a refused index, bad arguments") {
  begin();
  std::string out;
  CHECK(command("stsnx 3 65535\n", out) == 0);
  CHECK(out == "comm: set 1st sensor index\r\n");
  CHECK(command("stsny 4 5\n", out) == 0);
  CHECK(out == "comm: set 2nd sensor index\r\n");
  CHECK(stub::calls == stub::Calls{"comm_set_valve_sensor_index(3, 1, 65535)",
                                   "comm_set_valve_sensor_index(4, 2, 5)"});
  stub::communication.setSensorIndex = 1;
  CHECK(command("stsny 4 5\n", out) == 0);
  CHECK(out == "comm: set 2nd sensor index\r\ninvalid valve or sensor index\r\n");
  stub::communication.setSensorIndex = 0;
  checkRefused({"stsnx 3", "stsnx 3 5 7", "stsnx x 5", "stsnx 3 x", "stsnx", "stsny 4"},
               "to few arguments\r\n");
}

TEST_CASE("stvls: both addresses to the valve, then the sensors matched; errors") {
  begin();
  std::string out;
  CHECK(command("stvls 2 28-00-00-00-00-00-00-01 00-00-00-00-00-00-00-00\n", out) == 0);
  CHECK(out == "set valve sensors by address\r\n");
  CHECK(stub::calls == stub::Calls{"comm_set_valve_sensors(2, 28-00-00-00-00-00-00-01, 00-00-00-00-00-00-00-00)",
                                   "app_match_sensors()"});
  stub::calls.clear();
  stub::communication.setSensors = 1;
  CHECK(command("stvls 2 a b\n", out) == 0);
  CHECK(out == "set valve sensors by address\r\ninvalid valve index\r\n");
  CHECK(stub::calls == stub::Calls{"comm_set_valve_sensors(2, a, b)"});
  stub::communication.setSensors = 0;
  checkRefused({"stvls 2 a", "stvls x a b", "stvls"}, "set valve sensors by address\r\nto few arguments\r\n");
}

TEST_CASE("gvlon: the addresses of the last valve, errors") {
  begin();
  std::string out;
  stub::communication.firstSensor = "28-00-00-00-00-00-00-01";
  CHECK(command("gvlon 11\n", out) == 0);
  CHECK(out == "cmd: get 1st and 2nd onewire sensor addresses - gvlon 11 "
               "28-00-00-00-00-00-00-01 00-00-00-00-00-00-00-00 \r\n");
  CHECK(stub::calls == stub::Calls{"comm_print_valve_sensor_ids(11, ' ')"});
  checkRefused({"gvlon 12", "gvlon", "gvlon 1 2", "gvlon x"},
               "cmd: get 1st and 2nd onewire sensor addresses - error\r\n");
}

TEST_CASE("stlnt: 0 s is a valid learn time") {
  begin();
  std::string out;
  CHECK(command("stlnt 0\n", out) == 0);
  CHECK(out == "set valve learning time to 0\r\n");
  CHECK(stub::calls == stub::Calls{"comm_set_learntime(0)"});
}

TEST_CASE("staop and staln: the valve echoed, a refused valve, bad arguments") {
  begin();
  std::string out;
  CHECK(command("staop 3\n", out) == 0);
  CHECK(out == "got open valve request for 3\r\n");
  CHECK(command("staln 255\n", out) == 0);
  CHECK(out == "start learning for valve 255\r\n");
  CHECK(stub::calls == stub::Calls{"app_set_valveopen(3)", "app_set_valvelearning(255)"});
  stub::app.setValveOpen = 1;
  stub::app.setValveLearning = 1;
  CHECK(command("staop 3\n", out) == 0);
  CHECK(out == "got open valve request for - error\r\n");
  CHECK(command("staln 3\n", out) == 0);
  CHECK(out == "start learning for valve - error\r\n");
  stub::app.setValveOpen = 0;
  stub::app.setValveLearning = 0;
  checkRefused({"staop", "staop 1 2", "staop x"}, "got open valve request for - error\r\n");
  checkRefused({"staln", "staln 1 2", "staln x"}, "start learning for valve - error\r\n");
}

TEST_CASE("smotc: exactly two numbers") {
  begin();
  checkRefused({"smotc 17 17 1", "smotc x 17", "smotc 17 x", "smotc 17"},
               "got set motor characteristics request - error\r\n");
}
