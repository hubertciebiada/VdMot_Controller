// Tests of src/terminal.cpp (glue_terminal): banner, line handling, the debug terminal's commands
// with their output and calls, and the limits of the terminal's hardware access (S10): a motor
// output switched on by sena goes off after 2 s or above 60 mA, smux/sdir/sena only while the valve
// machine is idle, seteep only while the EEPROM is readable, stdet answers on the debug port only.
#include "glue_test.h"
#include "hardware.h"
#include "stub_app.h"
#include "stub_eeprom.h"
#include "stub_motor.h"
#include "stub_communication.h"
#include "stub_owDevices.h"
#include "stub_sysstat.h"
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

// the Write events of a pin
std::vector<fake::Event> writes(uint32_t pin) { return fake::eventsOf(fake::Ev::Write, pin); }

const uint32_t kEna[6] = {CTRL_ENA0, CTRL_ENA1, CTRL_ENA2, CTRL_ENA3, CTRL_ENA4, CTRL_ENA5};

}  // namespace

extern volatile int analog_current;

TEST_CASE("Terminal_Init: USART6 on PA12/PA11 at 115200, banner with version and revision") {
  glue::begin();
  CHECK(Terminal_Init() == 0);
  CHECK(Serial6.rxPin == PA12);
  CHECK(Serial6.txPin == PA11);
  CHECK(Serial6.baud == 115200);
  CHECK(Serial6.config == SERIAL_8N1);
  CHECK(fake::takeTx(Serial6) == "VdMot Controller 2.1.0-revamped_C2\r\n");
  CHECK(Serial6.flushes == 1);
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
  CHECK(out == "Version: 2.1.0-revamped\r\n");
  CHECK(command("stdet 255\n", out) == 0);
  CHECK(out == "got detect valve status request - reset all valves\r\nstdet \r\n");
  CHECK(fake::takeTx(Serial1).empty());
  CHECK(stub::calls == stub::Calls{"app_scan_valves()"});
  CHECK(command("stdet 3\n", out) == 0);
  CHECK(out == "got detect valve status request - error\r\nstdet \r\n");
  CHECK(command("stdet\n", out) == 0);
  CHECK(out == "got detect valve status request - error\r\n");
  CHECK(fake::takeTx(Serial1).empty());
  CHECK(stub::calls == stub::Calls{"app_scan_valves()"});
}

TEST_CASE("Terminal_Serve: seteep sets the base fields and marks everything, saveep marks everything") {
  begin();
  std::string out;
  strncpy(eep_content.descr, "old", sizeof eep_content.descr);
  eep_content.b_slave = 3;
  eep_content.OneWireCfg[0] = 1;
  eep_content.OneWireCfg[1] = 2;
  eep_content.OneWireCfg[2] = 3;
  CHECK(command("seteep\n", out) == 0);
  CHECK(out == "write EEPROM layout...\r\nset eeprom layout\r\n");
  CHECK(std::string(eep_content.descr) == "VdMot Controller");
  CHECK(eep_content.b_slave == 0);
  CHECK(eep_content.OneWireCfg[0] == 0);
  CHECK(eep_content.OneWireCfg[1] == 0);
  CHECK(eep_content.OneWireCfg[2] == 0);
  CHECK(command("saveep\n", out) == 0);
  CHECK(out == "saved eeprom layout\r\n");
  CHECK(stub::calls == stub::Calls{"eeprom_state()", "eeprom_changed(0x00ff)", "eeprom_changed(0x00ff)"});
}

TEST_CASE("Terminal_Serve: seteep is refused while the EEPROM could not be read") {
  begin();
  std::string out;
  stub::eeprom.state = vdm::kEepStateReadFailed;
  strncpy(eep_content.descr, "old", sizeof eep_content.descr);
  CHECK(command("seteep\n", out) == 0);
  CHECK(out == "eeprom not readable, write blocked\r\n");
  CHECK(std::string(eep_content.descr) == "old");
  CHECK(stub::calls == stub::Calls{"eeprom_state()"});
  stub::eeprom.state = vdm::kEepStateWriteFailed;
  CHECK(command("seteep\n", out) == 0);
  CHECK(out == "write EEPROM layout...\r\nset eeprom layout\r\n");
}

TEST_CASE("Terminal_Serve: getone prints the sensor data, stm is gone") {
  begin();
  std::string out;
  stub::owDevices.sensorData = "{\"cnt\":0}\r\n";
  CHECK(command("getone\n", out) == 0);
  CHECK(out == "{\"cnt\":0}\r\n");
  CHECK(command("stm 1\n", out) == CMD_NONE);
  CHECK(out == "unknown command\r\n");
}

TEST_CASE("sena: the output and the valve PSU on, off after 2 s; the valve machine gets no command meanwhile") {
  for (unsigned ch = 0; ch < 6; ch++) {
    CAPTURE(ch);
    begin();
    std::string out;
    fake::advanceMs(12345);
    stub::calls.clear();
    CHECK(command("sena " + std::to_string(ch) + " 1\n", out) == 0);
    CHECK(out.empty());
    CHECK(stub::calls == stub::Calls{"valve_idle()", "sysstat_safe_mode()"});
    CHECK(fake::board.out[kEna[ch]] == HIGH);
    CHECK(fake::board.out[POWER_ENA] == LOW);
    for (unsigned other = 0; other < 6; other++) {
      if (other != ch) CHECK(writes(kEna[other]).empty());
    }
    CHECK(terminal_manual_active());
    fake::advanceMs(1999);
    terminal_supervise();
    CHECK(fake::board.out[kEna[ch]] == HIGH);
    CHECK(terminal_manual_active());
    CHECK(fake::takeTx(Serial6).empty());
    fake::advanceMs(1);
    terminal_supervise();
    CHECK(fake::board.out[kEna[ch]] == LOW);
    CHECK(fake::board.out[POWER_ENA] == HIGH);
    CHECK_FALSE(terminal_manual_active());
    CHECK(fake::takeTx(Serial6) == "sena: output off\r\n");
    CHECK(writes(kEna[ch]).size() == 2);
    terminal_supervise();
    CHECK(fake::takeTx(Serial6).empty());
  }
}

TEST_CASE("sena: off at once above 60 mA filtered current, either sign") {
  for (int current : {601, -601}) {
    CAPTURE(current);
    begin();
    std::string out;
    command("sena 2 1\n", out);
    analog_current = current < 0 ? -600 : 600;
    terminal_supervise();
    CHECK(terminal_manual_active());
    analog_current = current;
    terminal_supervise();
    CHECK_FALSE(terminal_manual_active());
    CHECK(fake::board.out[CTRL_ENA2] == LOW);
  }
}

TEST_CASE("sena: refused while the valve machine works or in safe mode; 0 switches off; bad arguments") {
  begin();
  std::string out;
  stub::motor.idle = false;
  CHECK(command("sena 1 1\n", out) == 0);
  CHECK(out == "valve machine busy\r\n");
  CHECK(writes(CTRL_ENA1).empty());
  CHECK_FALSE(terminal_manual_active());
  stub::motor.idle = true;
  stub::sysstat.safeMode = true;
  CHECK(command("sena 1 1\n", out) == 0);
  CHECK(out == "valve machine busy\r\n");
  CHECK(writes(CTRL_ENA1).empty());
  CHECK(writes(POWER_ENA).empty());
  stub::sysstat.safeMode = false;
  // off: only that output, the manual enable of another output stays
  command("sena 1 1\n", out);
  CHECK(command("sena 4 0\n", out) == 0);
  CHECK(fake::board.out[CTRL_ENA4] == LOW);
  CHECK(writes(CTRL_ENA4).size() == 1);
  CHECK(terminal_manual_active());
  CHECK(fake::board.out[POWER_ENA] == LOW);
  CHECK(command("sena 1 0\n", out) == 0);
  CHECK_FALSE(terminal_manual_active());
  CHECK(fake::board.out[CTRL_ENA1] == LOW);
  CHECK(fake::board.out[POWER_ENA] == HIGH);
  // a second sena ends the first one
  command("sena 1 1\n", out);
  fake::advanceMs(1500);
  command("sena 2 1\n", out);
  CHECK(fake::board.out[CTRL_ENA1] == LOW);
  CHECK(fake::board.out[CTRL_ENA2] == HIGH);
  CHECK(fake::board.out[POWER_ENA] == LOW);
  fake::advanceMs(1999);
  terminal_supervise();
  CHECK(terminal_manual_active());
  fake::board.events.clear();
  for (const char* bad : {"sena 6 1\n", "sena 1\n", "sena 1 x\n", "sena x 1\n"}) {
    CAPTURE(bad);
    CHECK(command(bad, out) == 0);
    CHECK(out.empty());
  }
  CHECK(fake::board.events.empty());
}

TEST_CASE("smux and sdir only while the valve machine is idle") {
  begin();
  std::string out;
  CHECK(command("smux 1\n", out) == 0);
  CHECK(fake::board.out[CTRL_MUX] == LOW);  // C2: MUX_ON() drives the pin low
  CHECK(command("sdir 1\n", out) == 0);
  CHECK(fake::board.out[CTRL_DIRECTION] == HIGH);
  CHECK(out.empty());
  CHECK(command("smux 0\n", out) == 0);
  CHECK(fake::board.out[CTRL_MUX] == HIGH);
  CHECK(command("sdir 0\n", out) == 0);
  CHECK(fake::board.out[CTRL_DIRECTION] == LOW);
  stub::motor.idle = false;
  fake::board.events.clear();
  CHECK(command("smux 1\n", out) == 0);
  CHECK(out == "valve machine busy\r\n");
  CHECK(command("sdir 1\n", out) == 0);
  CHECK(out == "valve machine busy\r\n");
  CHECK(fake::board.events.empty());
  CHECK(command("smux\n", out) == 0);
  CHECK(command("sdir\n", out) == 0);
  CHECK(out.empty());
}

TEST_CASE("terminal stlnt stores the learn time through comm_set_learntime, smotc marks only a change") {
  begin();
  std::string out;
  CHECK(command("stlnt 3600\n", out) == 0);
  CHECK(out == "set valve learning time to 3600\r\n");
  stub::communication.setLearnTime = -1;
  CHECK(command("stlnt 5\n", out) == 0);
  CHECK(out == "set valve learning time to - error\r\n");
  CHECK(command("stlnt\n", out) == 0);
  CHECK(out == "set valve learning time to - error\r\n");
  CHECK(stub::calls == stub::Calls{"comm_set_learntime(3600)", "comm_set_learntime(5)"});
  stub::calls.clear();
  CHECK(command("smotc 17 17\n", out) == 0);
  CHECK(out == "got set motor characteristics request - valid\r\n");
  CHECK(stub::calls == stub::Calls{"motor_get_params()"});
  stub::calls.clear();
  CHECK(command("smotc 18 17\n", out) == 0);
  CHECK(out == "got set motor characteristics request - valid\r\n");
  CHECK(stub::calls ==
        stub::Calls{"motor_get_params()", "motor_set_params(18, 17, 50, 3000, 0)", "eeprom_changed(0x0004)"});
  stub::calls.clear();
  CHECK(command("smotc 17 99\n", out) == 0);
  CHECK(out == "got set motor characteristics request - values out of bounds\r\n");
  CHECK(stub::calls == stub::Calls{"motor_get_params()"});
}
