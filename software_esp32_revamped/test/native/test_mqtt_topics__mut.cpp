// mqtt_topics: HA prefixes at the topic length limit, reused subscription arrays, segments with
// '/' of the full length and command topics that only look like valve calibrations.
#include <string.h>

#include <string>

#include "doctest.h"
#include "vdm/mqtt_topics.h"

using namespace vdm;

namespace {

using Segments = char[kValveCount][kSegmentMax + 1];

InboundTopic parse(const TopicContext& c, const char* prefix, const std::string& t,
                   const Segments* seg = nullptr) {
  return parseInboundTopic(c, prefix, t.c_str(), t.size(), seg ? *seg : nullptr);
}

}  // namespace

TEST_CASE("buildSubscriptions: a HA prefix whose status topic is exactly kTopicMax long") {
  const TopicContext c;
  const std::string prefix(kTopicMax - 7, 'h');  // + "/status" = kTopicMax
  Subscription out[8];
  for (Subscription& s : out) memset(s.filter, 'x', sizeof s.filter);  // reused entries: every filter gets its own NUL
  const size_t n = buildSubscriptions(c, MqttMode::MqttHa, prefix.c_str(), nullptr, out, 8);
  REQUIRE(n == 5);
  CHECK(std::string(out[0].filter) == "VdMotFBH/valves/+/target/set");
  CHECK(std::string(out[1].filter) == "VdMotFBH/valves/+/target/set/set");
  CHECK(std::string(out[2].filter) == "VdMotFBH/cmd/#");
  CHECK(std::string(out[3].filter) == "homeassistant/status");
  CHECK(std::string(out[4].filter) == prefix + "/status");
  CHECK(out[4].qos == 1);
}

TEST_CASE("buildSubscriptions: a HA prefix one char too long adds no entry") {
  const TopicContext c;
  const std::string prefix(kTopicMax - 6, 'h');
  Subscription out[8];
  CHECK(buildSubscriptions(c, MqttMode::MqttHa, prefix.c_str(), nullptr, out, 8) == 4);
  CHECK(std::string(out[3].filter) == "homeassistant/status");
  CHECK(out[4].filter[0] == '\0');
}

TEST_CASE("buildSubscriptions: a configured segment with '/' of the full segment length") {
  TopicContext c;
  c.separate = false;
  Segments seg = {};
  memcpy(seg[2], "abcd/fghij", kSegmentMax);
  Subscription out[8];
  REQUIRE(buildSubscriptions(c, MqttMode::Mqtt, nullptr, seg, out, 8) == 5);
  CHECK(std::string(out[3].filter) == "VdMotFBH/valves/abcd/fghij/target");
  CHECK(std::string(out[4].filter) == "VdMotFBH/valves/abcd/fghij/target/set");
}

TEST_CASE("parseInboundTopic: the HA status topic of a prefix at the length limit") {
  const TopicContext c;
  const std::string prefix(kTopicMax - 7, 'h');
  CHECK(parse(c, prefix.c_str(), prefix + "/status").kind == InboundKind::HaStatus);
  CHECK(parse(c, prefix.c_str(), prefix + "/statu").kind == InboundKind::None);
}

TEST_CASE("parseInboundTopic: a topic longer than kTopicMax is none of ours") {
  const TopicContext c;
  const std::string t = "VdMotFBH/valves/" + std::string(kTopicMax, '1') + "/target/set";
  CHECK(parse(c, "ha", t).kind == InboundKind::None);
  CHECK(parse(c, "ha", "VdMotFBH/valves/1/target/set").kind == InboundKind::Target);
}

TEST_CASE("parseInboundTopic: commands that only look like a valve calibration") {
  const TopicContext c;
  CHECK(parse(c, "ha", "VdMotFBH/cmd/abcdefg1/calibrate").kind == InboundKind::UnknownCommand);
  CHECK(parse(c, "ha", "VdMotFBH/cmd/valvesX1/calibrate").kind == InboundKind::UnknownCommand);
  CHECK(parse(c, "ha", "VdMotFBH/cmd/valves/1234567890x").kind == InboundKind::UnknownCommand);
  const InboundTopic r = parse(c, "ha", "VdMotFBH/cmd/valves/3/calibrate");
  CHECK(r.kind == InboundKind::CalibrateValve);
  CHECK(r.valve == 2);
}
