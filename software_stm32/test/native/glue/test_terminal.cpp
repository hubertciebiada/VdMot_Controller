// Smoke tests of src/terminal.cpp (glue_terminal): banner, line handling and a sample of the debug
// terminal's commands with their output and calls.
#include "glue_test.h"
#include "hardware.h"
#include "stub_app.h"
#include "stub_eeprom.h"
#include "stub_motor.h"
#include "stub_owDevices.h"
#include "terminal.h"

namespace {

void begin() {
  glue::begin();
  Terminal_Init();
  fake::takeTx(Serial6);
}

// one line through Terminal_Serve(); its result, the terminal output in out
int16_t command(const std::string& line, std::string& out) {
  fake::inject(Serial6, line);
  const int16_t result = Terminal_Serve();
  out = fake::takeTx(Serial6);
  return result;
}

}  // namespace

TEST_CASE("Terminal_Init: USART6 on PA12/PA11 at 115200, banner with version and revision") {
  glue::begin();
  CHECK(Terminal_Init() == 0);
  CHECK(Serial6.rxPin == PA12);
  CHECK(Serial6.txPin == PA11);
  CHECK(Serial6.baud == 115200);
  CHECK(Serial6.config == SERIAL_8N1);
  CHECK(fake::takeTx(Serial6) == "VdMot Controller 2.0.0-revamped_C2\r\n");
  CHECK(Serial6.flushes == 1);
  CHECK(testmode == 0);
}

TEST_CASE("Terminal_Serve: no complete line, too many arguments, unknown command") {
  begin();
  std::string out;
  CHECK(command("help", out) == -1);
  CHECK(out.empty());
  CHECK(command("\n", out) == CMD_HELP);
  CHECK(out == "Help:\r\n*********************\r\n");
  CHECK(command("open 1 2 3 4\n", out) == -1);
  CHECK(out == "too many arguments\r\n");
  CHECK(command("xyzzy\n", out) == CMD_NONE);
  CHECK(out == "unknown command\r\n");
  CHECK(stub::calls.empty());
}

TEST_CASE("Terminal_Serve: learn, open and close hand the command to the valve machine") {
  begin();
  std::string out;
  CHECK(command("learn 3\n", out) == CMD_LEARN);
  CHECK(command("open 4 50\n", out) == CMD_OPEN);
  CHECK(command("close 5 100\n", out) == CMD_CLOSE);
  CHECK(out.empty());
  CHECK(command("close 5 101\n", out) == CMD_CLOSE);
  CHECK(out == "to few arguments\r\n");
  stub::motor.action = -1;
  CHECK(command("open 4 50\n", out) == CMD_OPEN);
  CHECK(out == "valve machine not idle\r\n");
  CHECK(stub::calls == stub::Calls{"appsetaction(l, 3, 0, 0)", "appsetaction(o, 4, 50, 0)",
                                   "appsetaction(c, 5, 100, 0)", "appsetaction(o, 4, 50, 0)"});
}

TEST_CASE("Terminal_Serve: settar, gvers and stdet 255") {
  begin();
  std::string out;
  CHECK(command("settar 2 40\n", out) == CMD_CLOSE);
  CHECK(out == "set valve 2 to 40\r\n");
  CHECK(+myvalvemots[2].target_position == 40);
  CHECK(command("gvers\n", out) == 0);
  CHECK(out == "Version: 2.0.0-revamped\r\n");
  CHECK(command("stdet 255\n", out) == 0);
  CHECK(out == "got detect valve status request - reset all valves\r\n");
  CHECK(fake::takeTx(Serial1) == "stdet \r\n");
  CHECK(stub::calls == stub::Calls{"app_scan_valves()"});
}

TEST_CASE("Terminal_Serve: seteep writes the base layout, saveep marks everything changed") {
  begin();
  std::string out;
  CHECK(command("seteep\n", out) == 0);
  CHECK(out == "write EEPROM layout...\r\nset eeprom layout\r\n");
  CHECK(std::string(eep_content.descr) == "VdMot Controller");
  CHECK(command("saveep\n", out) == 0);
  CHECK(out == "saved eeprom layout\r\n");
  CHECK(stub::calls ==
        stub::Calls{"eeprom_fill()", "eeprom_write_layout(&eep_content)", "eeprom_changed(0x00ff)"});
}

TEST_CASE("Terminal_Serve: getone prints the sensor data, stm switches the test mode") {
  begin();
  std::string out;
  stub::owDevices.sensorData = "{\"cnt\":0}\r\n";
  CHECK(command("getone\n", out) == 0);
  CHECK(out == "{\"cnt\":0}\r\n");
  CHECK(command("stm 1\n", out) == 0);
  CHECK(out == "testmode on\r\n");
  CHECK(testmode == 1);
  CHECK(command("stm 0\n", out) == 0);
  CHECK(testmode == 0);
}

TEST_CASE("terminal_supervise and terminal_manual_active: nothing supervised yet") {
  begin();
  terminal_supervise();
  CHECK_FALSE(terminal_manual_active());
  CHECK(fake::board.events.empty());
}
