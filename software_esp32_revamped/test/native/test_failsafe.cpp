// failsafe: constants, ranges, the regulator rule and every name.
#include <string>

#include "doctest.h"
#include "vdm/config.h"
#include "vdm/failsafe.h"

using namespace vdm;

TEST_CASE("failsafe: constants") {
  CHECK(kFailsafeHold == 255);
  CHECK(kFailsafePctDefault == 50);
  CHECK(kFailsafeTimeoutDefaultMin == 60);
  CHECK(kFailsafeTimeoutMinMin == 5);
  CHECK(kFailsafeTimeoutMaxMin == 1440);
}

TEST_CASE("failsafe: lease timeout range is 0 or 5..1440") {
  CHECK(leaseTimeoutValid(0));
  CHECK_FALSE(leaseTimeoutValid(1));
  CHECK_FALSE(leaseTimeoutValid(4));
  CHECK(leaseTimeoutValid(5));
  CHECK(leaseTimeoutValid(6));
  CHECK(leaseTimeoutValid(60));
  CHECK(leaseTimeoutValid(1439));
  CHECK(leaseTimeoutValid(1440));
  CHECK_FALSE(leaseTimeoutValid(1441));
  CHECK_FALSE(leaseTimeoutValid(65536));
  CHECK_FALSE(leaseTimeoutValid(UINT32_MAX));
}

TEST_CASE("failsafe: position range is 0..100 or 255") {
  CHECK(failsafePctValid(0));
  CHECK(failsafePctValid(1));
  CHECK(failsafePctValid(50));
  CHECK(failsafePctValid(99));
  CHECK(failsafePctValid(100));
  CHECK_FALSE(failsafePctValid(101));
  CHECK_FALSE(failsafePctValid(254));
  CHECK(failsafePctValid(255));
  CHECK_FALSE(failsafePctValid(256));
  CHECK_FALSE(failsafePctValid(UINT32_MAX));
}

TEST_CASE("failsafe: regulatorCause for every mode, session and HA status") {
  struct Row {
    MqttMode mode;
    bool connected;
    HaStatus ha;
    RegulatorCause cause;
  };
  const Row rows[] = {
      {MqttMode::Off, false, HaStatus::Unknown, RegulatorCause::Alive},
      {MqttMode::Off, false, HaStatus::Online, RegulatorCause::Alive},
      {MqttMode::Off, false, HaStatus::Offline, RegulatorCause::Alive},
      {MqttMode::Off, true, HaStatus::Unknown, RegulatorCause::Alive},
      {MqttMode::Off, true, HaStatus::Online, RegulatorCause::Alive},
      {MqttMode::Off, true, HaStatus::Offline, RegulatorCause::Alive},
      {MqttMode::Mqtt, false, HaStatus::Unknown, RegulatorCause::BrokerDown},
      {MqttMode::Mqtt, false, HaStatus::Online, RegulatorCause::BrokerDown},
      {MqttMode::Mqtt, false, HaStatus::Offline, RegulatorCause::BrokerDown},
      {MqttMode::Mqtt, true, HaStatus::Unknown, RegulatorCause::Alive},
      {MqttMode::Mqtt, true, HaStatus::Online, RegulatorCause::Alive},
      {MqttMode::Mqtt, true, HaStatus::Offline, RegulatorCause::Alive},  // HA ignored
      {MqttMode::MqttHa, false, HaStatus::Unknown, RegulatorCause::BrokerDown},
      {MqttMode::MqttHa, false, HaStatus::Online, RegulatorCause::BrokerDown},
      {MqttMode::MqttHa, false, HaStatus::Offline, RegulatorCause::BrokerDown},
      {MqttMode::MqttHa, true, HaStatus::Unknown, RegulatorCause::Alive},
      {MqttMode::MqttHa, true, HaStatus::Online, RegulatorCause::Alive},
      {MqttMode::MqttHa, true, HaStatus::Offline, RegulatorCause::HaOffline},
  };
  for (const Row& r : rows) {
    CAPTURE(static_cast<int>(r.mode));
    CAPTURE(r.connected);
    CAPTURE(static_cast<int>(r.ha));
    RegulatorInput in;
    in.mode = r.mode;
    in.brokerConnected = r.connected;
    in.ha = r.ha;
    in.commandSeq = 7;  // never part of the rule
    CHECK(regulatorCause(in) == r.cause);
  }
}

TEST_CASE("failsafe: defaults of the shared structs") {
  const RegulatorInput in;
  CHECK(in.mode == MqttMode::Off);
  CHECK_FALSE(in.brokerConnected);
  CHECK(in.ha == HaStatus::Unknown);
  CHECK(in.commandSeq == 0);
  CHECK(regulatorCause(in) == RegulatorCause::Alive);

  const LeaseStatus s;
  CHECK(s.mode == LeaseMode::None);
  CHECK(s.state == LeaseState::Off);
  CHECK(s.remainS == 0);
  CHECK(s.timeoutMin == 0);
  CHECK(s.failsafeMask == 0);
  CHECK(s.regulator == RegulatorCause::BrokerDown);
  CHECK(s.regulatorLostS == 0);
  CHECK_FALSE(s.configSynced);
  CHECK_FALSE(s.configFailed);
  CHECK(s.configTrusted);
}

TEST_CASE("failsafe: names of every value and out of range") {
  CHECK(std::string(leaseStateName(LeaseState::Off)) == "off");
  CHECK(std::string(leaseStateName(LeaseState::Running)) == "running");
  CHECK(std::string(leaseStateName(LeaseState::Expired)) == "expired");
  CHECK(std::string(leaseStateName(static_cast<LeaseState>(3))) == "unknown");

  CHECK(std::string(leaseModeName(LeaseMode::None)) == "none");
  CHECK(std::string(leaseModeName(LeaseMode::Stm)) == "stm");
  CHECK(std::string(leaseModeName(LeaseMode::Emulated)) == "esp");
  CHECK(std::string(leaseModeName(static_cast<LeaseMode>(3))) == "unknown");

  CHECK(std::string(haStatusName(HaStatus::Unknown)) == "unknown");
  CHECK(std::string(haStatusName(HaStatus::Online)) == "online");
  CHECK(std::string(haStatusName(HaStatus::Offline)) == "offline");
  CHECK(std::string(haStatusName(static_cast<HaStatus>(3))) == "unknown");

  CHECK(std::string(regulatorCauseName(RegulatorCause::Alive)) == "alive");
  CHECK(std::string(regulatorCauseName(RegulatorCause::BrokerDown)) == "broker_down");
  CHECK(std::string(regulatorCauseName(RegulatorCause::HaOffline)) == "ha_offline");
  CHECK(std::string(regulatorCauseName(static_cast<RegulatorCause>(3))) == "unknown");

  CHECK(std::string(failsafeKindName(FailsafeKind::None)) == "off");
  CHECK(std::string(failsafeKindName(FailsafeKind::Lease)) == "lease");
  CHECK(std::string(failsafeKindName(FailsafeKind::Blocked)) == "blocked");
  CHECK(std::string(failsafeKindName(static_cast<FailsafeKind>(3))) == "unknown");
}
