// Tests of src/communication.cpp (glue_communication), protocol 3: the commands of contracts.md 1.1
// against the stub values of app/eeprom/sysstat/owDevices (every argument boundary, the err forms,
// the gvlvy/gstax goldens), the EEPROM marks only on a change (S13), the lease poll of gvlvd/gvlvx,
// the USART1 error counters (S6), stdet (S15) and the board revision marker (W8).
#include <string.h>

#include "communication.h"
#include "glue_test.h"
#include "stub_app.h"
#include "stub_eeprom.h"
#include "stub_motor.h"
#include "stub_owDevices.h"
#include "stub_sysstat.h"
#include "vdm/lease.h"

extern const char kHardwareMarker[];

namespace {

void begin() {
  glue::begin();
  Serial1.begin(115200, SERIAL_8E1);
  communication_setup();
  fake::takeTx(Serial1);
  fake::takeTx(Serial6);
  stub::calls.clear();
}

std::string request(const std::string& line) {
  fake::inject(Serial1, line);
  communication_loop();
  return fake::takeTx(Serial1);
}

// the request and its reply, the calls it made
struct Exchange {
  std::string reply;
  stub::Calls calls;
};

Exchange exchange(const std::string& line) {
  stub::calls.clear();
  Exchange e;
  e.reply = request(line);
  e.calls = stub::calls;
  return e;
}

void goldenValve3() {
  valve_snapshot& s = stub::motor.snapshot[3];
  s.status = 9;
  s.actual_position = 50;
  s.target_position = 30;
  s.meancurrent = 17;
  s.opening_count = 3567;
  s.closing_count = 3610;
  s.deadzone_count = 43;
  s.calibRetries = 2;
  s.movements = 0;
  s.calibration = 0;
  s.calibActive = 0;
  s.diag.earlyWarn = false;
  s.diag.lastCalFailed = true;
  s.diag.earlyStops = 1;
  s.diag.last = {0, 1750, 1750, 1, 262, 6120};
  myvalves[3].cmdRejected = 4;
  stub::app.valveV3 = {66, 4, 50, 50, 3540, 0};
}

}  // namespace

TEST_CASE("gproto answers 3, gvers carries the board revision of the marker") {
  begin();
  CHECK(request("gproto\n") == "gproto 3\r\n");
  CHECK(request("gvers\n") == "gvers 2.1.0-revamped_C2 1 \r\n");
  CHECK(std::string(kHardwareMarker) == "VDM-HW:C2");
}

TEST_CASE("stdet: 255 tests every valve, another number is an error, no number no reply") {
  begin();
  Exchange e = exchange("stdet 255\n");
  CHECK(e.reply == "stdet \r\n");
  CHECK(e.calls == stub::Calls{"app_scan_valves()"});
  e = exchange("stdet 3\n");
  CHECK(e.reply == "stdet err\r\n");
  CHECK(e.calls.empty());
  CHECK(exchange("stdet 254\n").reply == "stdet err\r\n");
  CHECK(exchange("stdet 256\n").reply == "stdet err\r\n");
  e = exchange("stdet x\n");
  CHECK(e.reply.empty());
  CHECK(e.calls.empty());
  CHECK(exchange("stdet\n").reply.empty());
  CHECK(exchange("stdet 255 1\n").reply.empty());
}

TEST_CASE("gvlvd and gvlvx renew the lease through app_lease_poll, gvlvy and the rest do not") {
  begin();
  CHECK(exchange("gvlvd 0\n").calls.front() == "app_lease_poll()");
  CHECK(exchange("gvlvx 0\n").calls.front() == "app_lease_poll()");
  CHECK(exchange("gvlvd 12\n").calls == stub::Calls{"app_lease_poll()"});
  CHECK(stub::callsOf("app_lease_poll").size() == 1);
  Exchange e = exchange("gvlvy 0\n");
  CHECK(std::find(e.calls.begin(), e.calls.end(), "app_lease_poll()") == e.calls.end());
  CHECK(exchange("gtgtp 0\n").calls.empty());
}

TEST_CASE("slhbt: 0 and 1 reach the lease, the reply carries its state and remaining time") {
  begin();
  stub::app.leaseState = 1;
  stub::app.leaseRemainingS = 3540;
  Exchange e = exchange("slhbt 1\n");
  CHECK(e.reply == "slhbt 1 3540\r\n");
  CHECK(e.calls == stub::Calls{"app_lease_heartbeat(1)", "app_lease_state()", "app_lease_remaining_s()"});
  stub::app.leaseState = 2;
  stub::app.leaseRemainingS = 0;
  e = exchange("slhbt 0\n");
  CHECK(e.reply == "slhbt 2 0\r\n");
  CHECK(e.calls.front() == "app_lease_heartbeat(0)");
  for (const char* bad : {"slhbt 2\n", "slhbt\n", "slhbt 1 1\n", "slhbt x\n", "slhbt -1\n"}) {
    CAPTURE(bad);
    e = exchange(bad);
    CHECK(e.reply == "slhbt err\r\n");
    CHECK(e.calls.empty());
  }
}

TEST_CASE("slcfg: 0 and 5..1440 are stored and configure the lease, the rest is an error") {
  begin();
  eep_content.leaseTimeoutMin = 60;
  Exchange e = exchange("slcfg 60\n");
  CHECK(e.reply == "slcfg ok\r\n");
  CHECK(e.calls == stub::Calls{"app_lease_configure(60)", "app_lease_command()"});
  for (uint32_t m : {0u, 5u, 1440u}) {
    CAPTURE(m);
    e = exchange("slcfg " + std::to_string(m) + "\n");
    CHECK(e.reply == "slcfg ok\r\n");
    CHECK(eep_content.leaseTimeoutMin == m);
    CHECK(e.calls == stub::Calls{"eeprom_changed(0x0020)", "app_lease_configure(" + std::to_string(m) + ")",
                                 "app_lease_command()"});
  }
  for (const char* bad : {"slcfg 4\n", "slcfg 1441\n", "slcfg 65596\n", "slcfg\n", "slcfg 5 5\n", "slcfg x\n"}) {
    CAPTURE(bad);
    e = exchange(bad);
    CHECK(e.reply == "slcfg err\r\n");
    CHECK(e.calls.empty());
  }
  CHECK(eep_content.leaseTimeoutMin == 1440);
}

TEST_CASE("sfspo: one valve or all, 0..100 or 255, stored only when it changes") {
  begin();
  memset(eep_content.failsafePct, 50, sizeof eep_content.failsafePct);
  Exchange e = exchange("sfspo 3 40\n");
  CHECK(e.reply == "sfspo 3 ok\r\n");
  CHECK(e.calls == stub::Calls{"eeprom_changed(0x0040)", "app_set_failsafe(3, 40)", "app_lease_command()"});
  CHECK(eep_content.failsafePct[3] == 40);
  CHECK(eep_content.failsafePct[2] == 50);
  CHECK(eep_content.failsafePct[4] == 50);
  e = exchange("sfspo 3 40\n");
  CHECK(e.reply == "sfspo 3 ok\r\n");
  CHECK(e.calls == stub::Calls{"app_set_failsafe(3, 40)", "app_lease_command()"});
  CHECK(exchange("sfspo 0 0\n").reply == "sfspo 0 ok\r\n");
  CHECK(exchange("sfspo 11 100\n").reply == "sfspo 11 ok\r\n");
  CHECK(exchange("sfspo 5 255\n").reply == "sfspo 5 ok\r\n");
  CHECK(eep_content.failsafePct[0] == 0);
  CHECK(eep_content.failsafePct[11] == 100);
  CHECK(eep_content.failsafePct[5] == 255);
  e = exchange("sfspo 255 40\n");
  CHECK(e.reply == "sfspo 255 ok\r\n");
  CHECK(e.calls == stub::Calls{"eeprom_changed(0x0040)", "app_set_failsafe(255, 40)", "app_lease_command()"});
  for (unsigned v = 0; v < ACTUATOR_COUNT; v++) CHECK(eep_content.failsafePct[v] == 40);
  CHECK(exchange("sfspo 255 40\n").calls == stub::Calls{"app_set_failsafe(255, 40)", "app_lease_command()"});
  // only one valve differs: still one mark
  eep_content.failsafePct[7] = 41;
  CHECK(exchange("sfspo 255 40\n").calls.front() == "eeprom_changed(0x0040)");
  CHECK(eep_content.failsafePct[7] == 40);
  for (const char* bad : {"sfspo 3 101\n", "sfspo 3 254\n", "sfspo 3\n", "sfspo 3 40 1\n", "sfspo 3 x\n"}) {
    CAPTURE(bad);
    e = exchange(bad);
    CHECK(e.reply == "sfspo 3 err 1\r\n");
    CHECK(e.calls.empty());
  }
  for (const char* bad : {"sfspo 12 1\n", "sfspo 254 1\n", "sfspo\n", "sfspo x 1\n", "sfspo 256 1\n"}) {
    CAPTURE(bad);
    e = exchange(bad);
    CHECK(e.reply == "sfspo -1 err 1\r\n");
    CHECK(e.calls.empty());
  }
  CHECK(exchange("sfspo 255 101\n").reply == "sfspo 255 err 1\r\n");
  CHECK(eep_content.failsafePct[3] == 40);
}

TEST_CASE("glcfg: the lease timeout and the failsafe positions of the valves, extra arguments ignored") {
  begin();
  stub::app.leaseTimeout = 60;
  stub::app.failsafePct = 40;
  Exchange e = exchange("glcfg\n");
  CHECK(e.reply == "glcfg 60 40 40 40 40 40 40 40 40 40 40 40 40\r\n");
  REQUIRE(e.calls.size() == 14);
  CHECK(e.calls[0] == "app_lease_command()");
  CHECK(e.calls[1] == "app_failsafe_pct(0)");
  CHECK(e.calls[12] == "app_failsafe_pct(11)");
  CHECK(e.calls[13] == "app_lease_timeout()");
  CHECK(exchange("glcfg 1 2\n").reply == "glcfg 60 40 40 40 40 40 40 40 40 40 40 40 40\r\n");
}

TEST_CASE("gvlvy: the golden of contracts.md 1.2, gvlvx the same first values, none for a bad index") {
  begin();
  goldenValve3();
  Exchange e = exchange("gvlvy 3\n");
  CHECK(e.reply == "gvlvy 3 9 50 30 17 3567 3610 43 2 0 8 1 4 0 1750 1750 1 262 6120 66 4 50 50 3540 0\r\n");
  CHECK(e.calls == stub::Calls{"valve_get_snapshot(3)", "app_learn_pending(3, 9, 0)", "app_get_valve_v3(3)"});
  CHECK(request("gvlvx 3\n") == "gvlvx 3 9 50 30 17 3567 3610 43 2 0 8 1 4 0 1750 1750 1 262 6120\r\n");
  stub::app.valveV3 = {1023, 5, 255, 100, 86400, 255};
  CHECK(request("gvlvy 3\n") ==
        "gvlvy 3 9 50 30 17 3567 3610 43 2 0 8 1 4 0 1750 1750 1 262 6120 1023 5 255 100 86400 255\r\n");
  CHECK(request("gvlvy 11\n").rfind("gvlvy 11 ", 0) == 0);
  for (const char* bad : {"gvlvy 12\n", "gvlvy\n", "gvlvy 1 2\n", "gvlvy x\n"}) {
    CAPTURE(bad);
    e = exchange(bad);
    CHECK(e.reply.empty());
    CHECK(e.calls.empty());
  }
}

TEST_CASE("gstax: the golden of contracts.md 1.3, gstat the same first values") {
  begin();
  stub::sysstat.uptime = 86400;
  stub::sysstat.resets = 3;
  stub::sysstat.reason = vdm::BootReason::Pin;
  stub::app.leaseState = 1;
  stub::app.leaseRemainingS = 3540;
  stub::app.leaseClient = true;
  stub::app.leaseTimeout = 60;
  stub::eeprom.writes = 12;
  stub::app.tempAgeS = 2;
  stub::owDevices.scanAgeS = 3600;
  request("\x01\n");  // two malformed lines
  request("\x02\n");
  CHECK(request("gstax\n") == "gstax 86400 3 2 0 2 0 1 3540 1 60 0 0 0 0 0 0 0 0 0 12 2 3600 0\r\n");
  CHECK(request("gstax 1 2\n") == "gstax 86400 3 2 0 2 0 1 3540 1 60 0 0 0 0 0 0 0 0 0 12 2 3600 0\r\n");
  CHECK(request("gstat\n") == "gstat 86400 3 2 0 2 0\r\n");
}

TEST_CASE("gstax: every field from its source") {
  begin();
  stub::sysstat.uptime = 1;
  stub::sysstat.resets = 2;
  stub::sysstat.reason = vdm::BootReason::IndependentWatchdog;
  stub::eeprom.state = vdm::kEepStateWriteFailed;
  stub::app.leaseState = 2;
  stub::app.leaseRemainingS = 7;
  stub::app.leaseClient = false;
  stub::app.leaseTimeout = 1440;
  stub::app.failsafeMask = 0x0A5;
  stub::sysstat.safeMode = true;
  stub::sysstat.wdgResets = 3;
  stub::eeprom.cfgFlags = 0x81;
  stub::eeprom.cfgEvents = 5;
  stub::eeprom.writes = 4294967295u;
  stub::app.tempAgeS = 13;
  stub::owDevices.scanAgeS = 86401;
  stub::app.protectSuspended = true;
  // USART1 errors: overrun, framing, noise and parity (both count as noise), and a full ring
  fake::uartError(Serial1, HAL_UART_ERROR_ORE);
  fake::inject(Serial1, "a");
  fake::uartError(Serial1, HAL_UART_ERROR_FE | HAL_UART_ERROR_PE);
  fake::inject(Serial1, "b");
  fake::uartError(Serial1, HAL_UART_ERROR_NE);
  fake::inject(Serial1, "c");
  fake::inject(Serial1, "\n");
  communication_loop();
  fake::inject(Serial1, std::string(SERIAL_RX_BUFFER_SIZE + 1, '\n'));  // the ring holds 1023
  while (Serial1.available() > 0) communication_loop();
  const std::string reply = request("gstax\n");
  CHECK(reply == "gstax 1 2 4 0 0 2 2 7 0 1440 165 1 3 1 1 2 2 129 5 4294967295 13 86401 1\r\n");
  stub::app.protectSuspended = false;
  stub::sysstat.safeMode = false;
  stub::app.leaseClient = true;
  CHECK(request("gstax\n") == "gstax 1 2 4 0 0 2 2 7 1 1440 165 0 3 1 1 2 2 129 5 4294967295 13 86401 0\r\n");
}

TEST_CASE("the USART1 RX callback counts errors and still stores the bytes; installed with interrupts off") {
  glue::begin();
  Serial1.begin(115200, SERIAL_8E1);
  const uint32_t disables = fake::board.irqDisables;
  communication_setup();
  CHECK(fake::board.irqDisables == disables + 1);
  CHECK(fake::board.primask == 0);
  fake::takeTx(Serial6);
  fake::uartError(Serial1, HAL_UART_ERROR_ORE);
  CHECK(request("gproto\n") == "gproto 3\r\n");
  CHECK(request("gstax\n") == "gstax 0 0 0 0 0 0 0 0 0 0 0 0 0 1 0 0 0 0 0 0 0 0 0\r\n");
}

TEST_CASE("sstop: one valve or all through app_stop, an invalid index or a refusal is an error") {
  begin();
  Exchange e = exchange("sstop 3\n");
  CHECK(e.reply == "sstop 3 ok\r\n");
  CHECK(e.calls == stub::Calls{"app_stop(3)"});
  CHECK(exchange("sstop 0\n").reply == "sstop 0 ok\r\n");
  CHECK(exchange("sstop 11\n").reply == "sstop 11 ok\r\n");
  e = exchange("sstop 255\n");
  CHECK(e.reply == "sstop 255 ok\r\n");
  CHECK(e.calls == stub::Calls{"app_stop(255)"});
  for (const char* bad : {"sstop 12\n", "sstop 20\n", "sstop 254\n", "sstop\n", "sstop 1 2\n", "sstop x\n"}) {
    CAPTURE(bad);
    e = exchange(bad);
    CHECK(e.reply == "sstop -1 err 1\r\n");
    CHECK(e.calls.empty());
  }
  stub::app.stop = -1;
  CHECK(exchange("sstop 3\n").reply == "sstop -1 err 1\r\n");
}

TEST_CASE("gtlnt: the stored learn time, extra arguments ignored") {
  begin();
  stub::app.learnTime = 604800;
  Exchange e = exchange("gtlnt\n");
  CHECK(e.reply == "gtlnt 604800\r\n");
  CHECK(e.calls == stub::Calls{"app_get_learntime()"});
  stub::app.learnTime = 0;
  CHECK(request("gtlnt 5\n") == "gtlnt 0\r\n");
}

TEST_CASE("ssafe: only 0 leaves safe mode") {
  begin();
  Exchange e = exchange("ssafe 0\n");
  CHECK(e.reply == "ssafe ok\r\n");
  CHECK(e.calls == stub::Calls{"sysstat_leave_safe_mode()"});
  for (const char* bad : {"ssafe 1\n", "ssafe\n", "ssafe 0 0\n", "ssafe x\n"}) {
    CAPTURE(bad);
    e = exchange(bad);
    CHECK(e.reply == "ssafe err\r\n");
    CHECK(e.calls.empty());
  }
}

TEST_CASE("stlnt: the learn time goes to the app and is stored only when it changes") {
  begin();
  eep_content.learnTimeS = 604800;
  Exchange e = exchange("stlnt 3600\n");
  CHECK(e.reply == "stlnt\r\n");
  CHECK(e.calls == stub::Calls{"app_set_learntime(3600)", "eeprom_changed(0x0010)"});
  CHECK(eep_content.learnTimeS == 3600);
  e = exchange("stlnt 3600\n");
  CHECK(e.reply == "stlnt\r\n");
  CHECK(e.calls == stub::Calls{"app_set_learntime(3600)"});
  CHECK(comm_set_learntime(0) == 0);
  CHECK(eep_content.learnTimeS == 0);
  stub::app.setLearnTime = -1;
  e = exchange("stlnt 7\n");
  CHECK(e.reply.empty());
  CHECK(e.calls == stub::Calls{"app_set_learntime(7)"});
  CHECK(comm_set_learntime(8) == -1);
  CHECK(eep_content.learnTimeS == 0);
}

TEST_CASE("stlnm: every request resets the counters, the EEPROM is marked only for a new value") {
  begin();
  eep_content.numberOfMovements = 2000;
  Exchange e = exchange("stlnm 2000\n");
  CHECK(e.reply == "stlnm\r\n");
  CHECK(e.calls == stub::Calls{"app_set_learnmovements(2000)"});
  e = exchange("stlnm 50\n");
  CHECK(e.reply == "stlnm\r\n");
  CHECK(e.calls == stub::Calls{"app_set_learnmovements(50)", "eeprom_changed(0x0002)"});
  CHECK(eep_content.numberOfMovements == 50);
}

TEST_CASE("scalx: the escalation is stored only when it changes") {
  begin();
  eep_content.escalation = {1, 30, 40};
  Exchange e = exchange("scalx 1 30 40\n");
  CHECK(e.reply == "scalx ok\r\n");
  CHECK(e.calls == stub::Calls{"motor_set_escalation(1, 30, 40)"});
  eep_content.escalation = {1, 30, 41};
  CHECK(exchange("scalx 1 30 40\n").calls == stub::Calls{"motor_set_escalation(1, 30, 40)", "eeprom_changed(0x0008)"});
  eep_content.escalation = {1, 31, 40};
  CHECK(exchange("scalx 1 30 40\n").calls.back() == "eeprom_changed(0x0008)");
  eep_content.escalation = {0, 30, 40};
  CHECK(exchange("scalx 1 30 40\n").calls.back() == "eeprom_changed(0x0008)");
}

TEST_CASE("stsnx/stsny: the slot of the valve is marked, only when the address changes") {
  begin();
  const uint8_t rom[8] = {0x28, 1, 2, 3, 4, 5, 6, 0};
  memcpy(tempsensors[4].address, rom, 8);
  noOfDS18Devices = 5;
  Exchange e = exchange("stsnx 2 4\n");
  CHECK(e.reply == "stsnx\r\n");
  CHECK(e.calls == stub::Calls{"eeprom_changed_slot(2)"});
  CHECK(eep_content.owsensors1[2].familycode == 0x28);
  CHECK(eep_content.owsensors1[2].romcode[0] == 1);
  CHECK(eep_content.owsensors1[2].romcode[5] == 6);
  CHECK(myvalves[2].sensorindex1 == 4);
  CHECK(exchange("stsnx 2 4\n").calls.empty());
  e = exchange("stsny 2 4\n");
  CHECK(e.reply == "stsny\r\n");
  CHECK(e.calls == stub::Calls{"eeprom_changed_slot(14)"});
  CHECK(myvalves[2].sensorindex2 == 4);
  CHECK(exchange("stsny 11 4\n").calls == stub::Calls{"eeprom_changed_slot(23)"});
  CHECK(exchange("stsny 11 5\n").reply.empty());  // no sensor 5
  // one byte differs: stored again
  eep_content.owsensors1[2].crc = 1;
  CHECK(exchange("stsnx 2 4\n").calls == stub::Calls{"eeprom_changed_slot(2)"});
  CHECK(eep_content.owsensors1[2].crc == 0);
}

TEST_CASE("stvls: both slots of the valve, each marked only when it changes") {
  begin();
  // 28-01-02-03-04-05-06-crc with a valid CRC, found by the fake DallasTemperature
  fake::OneWireDevice& dev = fake::addOneWire(0x28, 7);
  std::string text;
  char part[4];
  for (unsigned i = 0; i < 8; i++) {
    snprintf(part, sizeof part, i ? "-%02X" : "%02X", dev.rom[i]);
    text += part;
  }
  memset(&eep_content.owsensors2[3], 0xFF, sizeof eep_content.owsensors2[3]);  // never assigned
  Exchange e = exchange("stvls 3 " + text + " 00-00-00-00-00-00-00-00\n");
  CHECK(e.reply == "stvls 3\r\n");
  CHECK(e.calls == stub::Calls{"eeprom_changed_slot(3)", "eeprom_changed_slot(15)"});
  CHECK(eep_content.owsensors1[3].familycode == 0x28);
  CHECK(eep_content.owsensors1[3].crc == dev.rom[7]);
  CHECK(eep_content.owsensors2[3].familycode == 0);
  e = exchange("stvls 3 " + text + " 00-00-00-00-00-00-00-00\n");
  CHECK(e.reply == "stvls 3\r\n");
  CHECK(e.calls.empty());
  // an invalid address changes nothing
  CHECK(exchange("stvls 3 28-00-00-00-00-00-00-01 zz\n").calls.empty());
  CHECK(exchange("stvls 12 " + text + " " + text + "\n").reply.empty());
}

TEST_CASE("comm_set_valve_sensor_index: invalid valve, slot or sensor changes nothing") {
  begin();
  noOfDS18Devices = 2;
  CHECK(comm_set_valve_sensor_index(12, 1, 0) == -1);
  CHECK(comm_set_valve_sensor_index(0, 1, 2) == -1);
  CHECK(comm_set_valve_sensor_index(0, 0, 1) == -1);
  CHECK(comm_set_valve_sensor_index(0, 3, 1) == -1);
  CHECK(stub::calls.empty());
  noOfDS18Devices = MAXONEWIRECNT + 1;
  CHECK(comm_set_valve_sensor_index(0, 1, MAXONEWIRECNT) == -1);
  CHECK(comm_set_valve_sensors(12, "00-00-00-00-00-00-00-00", "00-00-00-00-00-00-00-00") == -1);
  CHECK(stub::calls.empty());
}
