// Tests of src/web_server.cpp through the request driver: the STM action handlers (service move,
// valve sensors, profile refresh, motor settings, reset), image removal, flash start and abort,
// factory reset and MQTT discovery, each with its boundaries and its error answers.
#include <string>

#include <vdm/stm_codec.h>

#include "glue_test.h"
#include "web_server.h"

namespace {

using fakes::http::Request;
using fakes::http::Response;

std::string errorBody(const char* code, const char* detail) {
  return std::string("{\"error\":\"") + code + "\",\"detail\":\"" + detail + "\"}";
}

void start() {
  sib::storage().active.valves[0].active = true;
  ++sib::storage().revision;
  web::begin();
}

Request apiPost(const std::string& url, const std::string& body) {
  Request r = fakes::http::post(url, body);
  r.header("X-VdMot", "1");
  return r;
}

Request apiDel(const std::string& url) {
  Request r = fakes::http::del(url);
  r.header("X-VdMot", "1");
  return r;
}

Response post(const std::string& url, const std::string& body) {
  return fakes::http::perform(apiPost(url, body));
}

int postCode(const std::string& url, const std::string& body) { return post(url, body).code; }

// A 1-Wire id with a valid CRC byte (Dallas/Maxim CRC-8 over the first 7 bytes).
vdm::OneWireId validId(uint8_t family, uint8_t serial) {
  vdm::OneWireId id;
  id.b[0] = family;
  for (int i = 1; i < 7; ++i) id.b[i] = static_cast<uint8_t>(serial + i);
  uint8_t crc = 0;
  for (int i = 0; i < 7; ++i) {
    uint8_t in = id.b[i];
    for (int bit = 0; bit < 8; ++bit) {
      const uint8_t mix = (crc ^ in) & 0x01;
      crc >>= 1;
      if (mix) crc ^= 0x8C;
      in >>= 1;
    }
  }
  id.b[7] = crc;
  return id;
}

const char* kMoveUrl = "/api/valves/2/service-move";
const char* kSensorsUrl = "/api/valves/3/sensors";
const char* kMotorUrl = "/api/stm/motor";

std::string move(const std::string& dir, const std::string& counts, const std::string& maxmA) {
  return "{\"dir\":" + dir + ",\"counts\":" + counts + ",\"maxmA\":" + maxmA + "}";
}

std::string motorBody(int low, int high, int sop, int minCnt, int reps) {
  return "{\"motor\":{\"lowC\":" + std::to_string(low) + ",\"highC\":" + std::to_string(high) +
         ",\"startOnPower\":" + std::to_string(sop) + ",\"noOfMinCount\":" +
         std::to_string(minCnt) + ",\"maxCalReps\":" + std::to_string(reps) + "}}";
}

std::string breakawayBody(const std::string& enable, int step, int maxmA) {
  return "{\"breakaway\":{\"enable\":" + enable + ",\"stepPct\":" + std::to_string(step) +
         ",\"maxmA\":" + std::to_string(maxmA) + "}}";
}

storage::ImageEntry image(const char* name, const char* hw) {
  storage::ImageEntry e;
  vdm::copyString(e.name, sizeof e.name, name);
  e.scanned = true;
  e.check = vdm::FlashError::None;
  vdm::copyString(e.hwTag, sizeof e.hwTag, hw);
  return e;
}

}  // namespace

// ---------------------------------------------------------------- service move

TEST_CASE("web service move: protocol 2 is required, the command carries dir, counts and maxmA") {
  glue::begin();
  sib::app().proto = 1;
  start();
  Response r = post(kMoveUrl, move("\"open\"", "10", "20"));
  CHECK(r.code == 409);
  CHECK(r.body == errorBody("unsupported", "STM protocol v2 required"));
  CHECK(sib::app().submitted.empty());
  sib::app().proto = 2;
  r = post(kMoveUrl, move("\"open\"", "1", "5"));
  CHECK(r.code == 202);
  CHECK(r.body == "{\"result\":\"queued\"}");
  r = post(kMoveUrl, move("\"close\"", "10000", "60"));
  CHECK(r.code == 202);
  REQUIRE(sib::app().submitted.size() == 2);
  const app::Command& a = sib::app().submitted[0];
  CHECK(a.type == app::CommandType::ServiceMove);
  CHECK(a.valve == 1);
  CHECK(a.dir == vdm::MoveDir::Open);
  CHECK(a.counts == 1);
  CHECK(a.maxmA == 5);
  const app::Command& b = sib::app().submitted[1];
  CHECK(b.dir == vdm::MoveDir::Close);
  CHECK(b.counts == 10000);
  CHECK(b.maxmA == 60);
  sib::app().proto = 3;
  CHECK(postCode(kMoveUrl, move("\"open\"", "10", "20")) == 202);
}

TEST_CASE("web service move: every member is required and checked") {
  glue::begin();
  sib::app().proto = 2;
  start();
  const std::string detail = "dir open|close, counts 1..10000, maxmA 5..60";
  for (const std::string& bad :
       {move("\"up\"", "10", "20"), move("5", "10", "20"), move("\"open\"", "0", "20"),
        move("\"open\"", "10001", "20"), move("\"open\"", "10", "4"),
        move("\"open\"", "10", "61"), move("\"open\"", "true", "20"),
        std::string("{\"counts\":10,\"maxmA\":20}"), std::string("{\"dir\":\"open\",\"maxmA\":20}"),
        std::string("{\"dir\":\"open\",\"counts\":10}"),
        std::string("{\"dir\":\"open\",\"counts\":10,\"maxmA\":20,\"x\":1}")}) {
    CAPTURE(bad);
    const Response r = post(kMoveUrl, bad);
    CHECK(r.code == 400);
    CHECK(r.body == errorBody("out_of_range", detail.c_str()));
  }
  CHECK(sib::app().submitted.empty());
  sib::app().submitResult = false;
  const Response r = post(kMoveUrl, move("\"open\"", "10", "20"));
  CHECK(r.code == 503);
  CHECK(r.body == errorBody("queue_full", "STM command queue full"));
  sib::app().submitResult = true;
  CHECK(post(kMoveUrl, "").body == errorBody("bad_request", "JSON body required"));
}

// ---------------------------------------------------------------- valve sensors

TEST_CASE("web valve sensors: the slots name the configured ids") {
  glue::begin();
  vdm::Config& c = sib::storage().active;
  c.temps[0].id = validId(0x28, 10);
  c.temps[1].id = validId(0x28, 20);
  c.temps[33].id = validId(0x28, 30);
  start();
  Response r = post(kSensorsUrl, "{\"slot1\":1,\"slot2\":2}");
  CHECK(r.code == 202);
  CHECK(r.body == "{\"result\":\"queued\"}");
  r = post(kSensorsUrl, "{\"slot1\":0,\"slot2\":34}");
  CHECK(r.code == 202);
  r = post(kSensorsUrl, "{\"slot1\":34,\"slot2\":0}");
  CHECK(r.code == 202);
  r = post(kSensorsUrl, "{\"slot1\":0,\"slot2\":0}");
  CHECK(r.code == 202);
  REQUIRE(sib::app().submitted.size() == 4);
  const vdm::OneWireId zero;
  const app::Command& a = sib::app().submitted[0];
  CHECK(a.type == app::CommandType::SetValveSensors);
  CHECK(a.valve == 2);
  CHECK(a.ids[0] == c.temps[0].id);
  CHECK(a.ids[1] == c.temps[1].id);
  CHECK(sib::app().submitted[1].ids[0] == zero);
  CHECK(sib::app().submitted[1].ids[1] == c.temps[33].id);
  CHECK(sib::app().submitted[2].ids[0] == c.temps[33].id);
  CHECK(sib::app().submitted[2].ids[1] == zero);
  CHECK(sib::app().submitted[3].ids[0] == zero);
  CHECK(sib::app().submitted[3].ids[1] == zero);
}

TEST_CASE("web valve sensors: ranges, distinct slots and valid ids") {
  glue::begin();
  vdm::Config& c = sib::storage().active;
  c.temps[0].id = validId(0x28, 10);
  c.temps[2].id = validId(0x28, 20);
  c.temps[3].id = validId(0x28, 40);
  c.temps[4].id = validId(0x28, 50);
  c.temps[4].id.b[7] ^= 0x01;  // CRC broken
  start();
  const char* detail = "slot1/slot2 0..34, distinct";
  for (const char* bad :
       {"{\"slot1\":35,\"slot2\":0}", "{\"slot1\":0,\"slot2\":35}", "{\"slot1\":-1,\"slot2\":0}",
        "{\"slot1\":0,\"slot2\":-1}", "{\"slot2\":0}", "{\"slot1\":0}", "{\"slot1\":1,\"slot2\":1}",
        "{\"slot1\":3,\"slot2\":3}", "{\"slot1\":0,\"slot2\":0,\"x\":1}",
        "{\"slot1\":true,\"slot2\":0}"}) {
    CAPTURE(bad);
    const Response r = post(kSensorsUrl, bad);
    CHECK(r.code == 400);
    CHECK(r.body == errorBody("out_of_range", detail));
  }
  Response r = post(kSensorsUrl, "{\"slot1\":2,\"slot2\":0}");  // slot 2 has no id
  CHECK(r.code == 400);
  CHECK(r.body == errorBody("invalid", "slot1 has no valid sensor id"));
  r = post(kSensorsUrl, "{\"slot1\":1,\"slot2\":5}");  // slot 5: bad CRC
  CHECK(r.code == 400);
  CHECK(r.body == errorBody("invalid", "slot2 has no valid sensor id"));
  r = post(kSensorsUrl, "{\"slot1\":5,\"slot2\":0}");
  CHECK(r.body == errorBody("invalid", "slot1 has no valid sensor id"));
  CHECK(sib::app().submitted.empty());
  r = post(kSensorsUrl, "{\"slot1\":3,\"slot2\":4}");
  CHECK(r.code == 202);
  REQUIRE(sib::app().submitted.size() == 1);
  CHECK(sib::app().submitted[0].ids[0] == c.temps[2].id);
  CHECK(sib::app().submitted[0].ids[1] == c.temps[3].id);
  sib::app().submitResult = false;
  CHECK(postCode(kSensorsUrl, "{\"slot1\":0,\"slot2\":0}") == 503);
}

// ---------------------------------------------------------------- profile refresh

TEST_CASE("web profile refresh: protocol 2 is required") {
  glue::begin();
  sib::app().proto = 1;
  start();
  Response r = post("/api/valves/4/profile", "");
  CHECK(r.code == 409);
  CHECK(r.body == errorBody("unsupported", "STM protocol v2 required"));
  CHECK(sib::app().submitted.empty());
  sib::app().proto = 2;
  r = post("/api/valves/4/profile", "");
  CHECK(r.code == 202);
  REQUIRE(sib::app().submitted.size() == 1);
  CHECK(sib::app().submitted[0].type == app::CommandType::RequestProfile);
  CHECK(sib::app().submitted[0].valve == 3);
}

// ---------------------------------------------------------------- motor settings

TEST_CASE("web motor: all five motor values are sent as one command") {
  glue::begin();
  start();
  Response r = post(kMotorUrl, motorBody(10, 40, 0, 0, 0));
  CHECK(r.code == 202);
  CHECK(r.body == "{\"result\":\"queued\"}");
  REQUIRE(sib::app().submitted.size() == 1);
  const app::Command& c = sib::app().submitted[0];
  CHECK(c.type == app::CommandType::SetMotorSettings);
  CHECK(c.hasMotor);
  CHECK_FALSE(c.hasLearnMovements);
  CHECK_FALSE(c.hasBreakaway);
  CHECK(c.motor.lowFactor == 10);
  CHECK(c.motor.highFactor == 40);
  CHECK(c.motor.startOnPower == 0);
  CHECK(c.motor.minCounts == 0);
  CHECK(c.motor.maxCalibRetries == 0);
  CHECK(c.motor.fieldCount == 5);
  CHECK(postCode(kMotorUrl, motorBody(40, 10, 100, 60000, 2)) == 202);
  REQUIRE(sib::app().submitted.size() == 2);
  const app::Command& d = sib::app().submitted[1];
  CHECK(d.motor.lowFactor == 40);
  CHECK(d.motor.highFactor == 10);
  CHECK(d.motor.startOnPower == 100);
  CHECK(d.motor.minCounts == 60000);
  CHECK(d.motor.maxCalibRetries == 2);
}

TEST_CASE("web motor: each motor value has its range") {
  glue::begin();
  start();
  for (const std::string& bad :
       {motorBody(9, 17, 30, 3000, 2), motorBody(41, 17, 30, 3000, 2),
        motorBody(17, 9, 30, 3000, 2), motorBody(17, 41, 30, 3000, 2),
        motorBody(17, 17, -1, 3000, 2), motorBody(17, 17, 101, 3000, 2),
        motorBody(17, 17, 30, -1, 2), motorBody(17, 17, 30, 60001, 2),
        motorBody(17, 17, 30, 3000, -1), motorBody(17, 17, 30, 3000, 3)}) {
    CAPTURE(bad);
    const Response r = post(kMotorUrl, bad);
    CHECK(r.code == 400);
    CHECK(r.body == errorBody("out_of_range", "motor"));
  }
  CHECK(sib::app().submitted.empty());
}

TEST_CASE("web motor: a partial motor update keeps the values read from the STM") {
  glue::begin();
  vdm::StmSnapshot& s = sib::app().snapshot;
  s.motor.lowFactor = 21;
  s.motor.highFactor = 22;
  s.motor.startOnPower = 23;
  s.motor.minCounts = 2400;
  s.motor.maxCalibRetries = 1;
  s.motor.fieldCount = 3;
  start();
  // not read yet: only a complete set is accepted
  Response r = post(kMotorUrl, "{\"motor\":{\"lowC\":12}}");
  CHECK(r.code == 409);
  CHECK(r.body == errorBody("unknown", "motor parameters not read yet: send all five"));
  r = post(kMotorUrl,
           "{\"motor\":{\"lowC\":12,\"highC\":13,\"startOnPower\":14,\"noOfMinCount\":15}}");
  CHECK(r.code == 409);
  CHECK(sib::app().submitted.empty());
  s.haveMotor = true;
  for (const char* one : {"{\"motor\":{\"lowC\":12}}", "{\"motor\":{\"highC\":13}}",
                          "{\"motor\":{\"startOnPower\":14}}", "{\"motor\":{\"noOfMinCount\":15}}",
                          "{\"motor\":{\"maxCalReps\":0}}", "{\"motor\":{}}"}) {
    CAPTURE(one);
    CHECK(postCode(kMotorUrl, one) == 202);
  }
  REQUIRE(sib::app().submitted.size() == 6);
  const std::vector<app::Command>& c = sib::app().submitted;
  CHECK(c[0].motor.lowFactor == 12);
  CHECK(c[0].motor.highFactor == 22);
  CHECK(c[0].motor.startOnPower == 23);
  CHECK(c[0].motor.minCounts == 2400);
  CHECK(c[0].motor.maxCalibRetries == 1);
  CHECK(c[0].motor.fieldCount == 5);
  CHECK(c[1].motor.lowFactor == 21);
  CHECK(c[1].motor.highFactor == 13);
  CHECK(c[2].motor.startOnPower == 14);
  CHECK(c[2].motor.highFactor == 22);
  CHECK(c[3].motor.minCounts == 15);
  CHECK(c[3].motor.startOnPower == 23);
  CHECK(c[4].motor.maxCalibRetries == 0);
  CHECK(c[4].motor.minCounts == 2400);
  CHECK(c[5].hasMotor);
  CHECK(c[5].motor.maxCalibRetries == 1);
  // values read from the STM that a v1 STM would not keep are refused
  s.motor.lowFactor = 5;
  r = post(kMotorUrl, "{\"motor\":{\"highC\":13}}");
  CHECK(r.code == 400);
  CHECK(r.body == errorBody("out_of_range", "motor"));
  CHECK(sib::app().submitted.size() == 6);
}

TEST_CASE("web motor: the members and their shapes") {
  glue::begin();
  sib::app().proto = 2;
  sib::app().snapshot.haveMotor = true;
  sib::app().snapshot.haveBreakaway = true;
  start();
  Response r = post(kMotorUrl, "{}");
  CHECK(r.code == 400);
  CHECK(r.body == errorBody("bad_request", "nothing to set"));
  r = post(kMotorUrl, "{\"motor\":null,\"breakaway\":null}");
  CHECK(r.code == 400);
  CHECK(r.body == errorBody("bad_request", "nothing to set"));
  r = post(kMotorUrl, "{\"learnMovements\":0,\"x\":1}");
  CHECK(r.code == 400);
  CHECK(r.body == errorBody("unknown_key", "motor/learnMovements/breakaway"));
  for (const char* bad : {"{\"motor\":5}", "{\"motor\":[1]}", "{\"motor\":{\"lowC\":12,\"x\":1}}"}) {
    CAPTURE(bad);
    r = post(kMotorUrl, bad);
    CHECK(r.code == 400);
    CHECK(r.body == errorBody("invalid", "motor"));
  }
  for (const char* bad : {"{\"breakaway\":5}", "{\"breakaway\":{\"enable\":true,\"x\":1}}"}) {
    CAPTURE(bad);
    r = post(kMotorUrl, bad);
    CHECK(r.code == 400);
    CHECK(r.body == errorBody("invalid", "breakaway"));
  }
  CHECK(sib::app().submitted.empty());
  // every member alone, and all three together
  CHECK(postCode(kMotorUrl, "{\"motor\":{\"lowC\":12,\"maxCalReps\":1}}") == 202);
  CHECK(postCode(kMotorUrl, "{\"learnMovements\":100}") == 202);
  CHECK(postCode(kMotorUrl, "{\"breakaway\":{\"enable\":true}}") == 202);
  CHECK(postCode(kMotorUrl,
                 "{\"motor\":{\"lowC\":12},\"learnMovements\":0,"
                 "\"breakaway\":{\"stepPct\":5,\"maxmA\":30}}") == 202);
  REQUIRE(sib::app().submitted.size() == 4);
  const std::vector<app::Command>& c = sib::app().submitted;
  CHECK(c[0].hasMotor);
  CHECK_FALSE(c[0].hasLearnMovements);
  CHECK_FALSE(c[0].hasBreakaway);
  CHECK_FALSE(c[1].hasMotor);
  CHECK(c[1].hasLearnMovements);
  CHECK(c[1].learnMovements == 100);
  CHECK_FALSE(c[1].hasBreakaway);
  CHECK_FALSE(c[2].hasMotor);
  CHECK_FALSE(c[2].hasLearnMovements);
  CHECK(c[2].hasBreakaway);
  CHECK(c[3].hasMotor);
  CHECK(c[3].hasLearnMovements);
  CHECK(c[3].learnMovements == 0);
  CHECK(c[3].hasBreakaway);
  CHECK(c[3].breakaway.stepPct == 5);
  CHECK(c[3].breakaway.maxmA == 30);
  sib::app().submitResult = false;
  r = post(kMotorUrl, "{\"learnMovements\":100}");
  CHECK(r.code == 503);
  CHECK(r.body == errorBody("queue_full", "STM command queue full"));
}

TEST_CASE("web motor: learnMovements is 0 or 50..65534") {
  glue::begin();
  start();
  for (const char* ok : {"{\"learnMovements\":0}", "{\"learnMovements\":50}",
                         "{\"learnMovements\":65534}"}) {
    CAPTURE(ok);
    CHECK(postCode(kMotorUrl, ok) == 202);
  }
  REQUIRE(sib::app().submitted.size() == 3);
  CHECK(sib::app().submitted[0].learnMovements == 0);
  CHECK(sib::app().submitted[1].learnMovements == 50);
  CHECK(sib::app().submitted[2].learnMovements == 65534);
  for (const char* bad : {"{\"learnMovements\":-1}", "{\"learnMovements\":1}",
                          "{\"learnMovements\":49}", "{\"learnMovements\":65535}",
                          "{\"learnMovements\":true}", "{\"learnMovements\":\"50\"}"}) {
    CAPTURE(bad);
    const Response r = post(kMotorUrl, bad);
    CHECK(r.code == 400);
    CHECK(r.body == errorBody("out_of_range", "learnMovements 0 or 50..65534"));
  }
  CHECK(sib::app().submitted.size() == 3);
}

TEST_CASE("web motor: breakaway needs protocol 2 and a complete set until it was read") {
  glue::begin();
  sib::app().proto = 1;
  vdm::StmSnapshot& s = sib::app().snapshot;
  s.breakaway.enable = true;
  s.breakaway.stepPct = 7;
  s.breakaway.maxmA = 44;
  start();
  Response r = post(kMotorUrl, breakawayBody("false", 0, 20));
  CHECK(r.code == 409);
  CHECK(r.body == errorBody("unsupported", "STM protocol v2 required"));
  sib::app().proto = 2;
  r = post(kMotorUrl, "{\"breakaway\":{\"enable\":false,\"stepPct\":3}}");
  CHECK(r.code == 409);
  CHECK(r.body == errorBody("unknown", "breakaway not read yet: send all three"));
  CHECK(sib::app().submitted.empty());
  r = post(kMotorUrl, breakawayBody("false", 0, 20));
  CHECK(r.code == 202);
  CHECK(postCode(kMotorUrl, breakawayBody("true", 100, 60)) == 202);
  REQUIRE(sib::app().submitted.size() == 2);
  CHECK_FALSE(sib::app().submitted[0].breakaway.enable);
  CHECK(sib::app().submitted[0].breakaway.stepPct == 0);
  CHECK(sib::app().submitted[0].breakaway.maxmA == 20);
  CHECK(sib::app().submitted[1].breakaway.enable);
  CHECK(sib::app().submitted[1].breakaway.stepPct == 100);
  CHECK(sib::app().submitted[1].breakaway.maxmA == 60);
  for (const std::string& bad :
       {breakawayBody("1", 5, 30), breakawayBody("true", -1, 30), breakawayBody("true", 101, 30),
        breakawayBody("true", 5, 19), breakawayBody("true", 5, 61)}) {
    CAPTURE(bad);
    r = post(kMotorUrl, bad);
    CHECK(r.code == 400);
    CHECK(r.body == errorBody("out_of_range", "breakaway"));
  }
  CHECK(sib::app().submitted.size() == 2);
  // read once: single members keep the other values
  s.haveBreakaway = true;
  CHECK(postCode(kMotorUrl, "{\"breakaway\":{\"stepPct\":9}}") == 202);
  CHECK(postCode(kMotorUrl, "{\"breakaway\":{\"maxmA\":33}}") == 202);
  CHECK(postCode(kMotorUrl, "{\"breakaway\":{\"enable\":false}}") == 202);
  REQUIRE(sib::app().submitted.size() == 5);
  const std::vector<app::Command>& c = sib::app().submitted;
  CHECK(c[2].breakaway.enable);
  CHECK(c[2].breakaway.stepPct == 9);
  CHECK(c[2].breakaway.maxmA == 44);
  CHECK(c[3].breakaway.stepPct == 7);
  CHECK(c[3].breakaway.maxmA == 33);
  CHECK_FALSE(c[4].breakaway.enable);
  CHECK(c[4].breakaway.maxmA == 44);
  // a value read from the STM out of range is refused
  s.breakaway.maxmA = 10;
  r = post(kMotorUrl, "{\"breakaway\":{\"enable\":true}}");
  CHECK(r.code == 400);
  CHECK(r.body == errorBody("out_of_range", "breakaway"));
  CHECK(sib::app().submitted.size() == 5);
}

// ---------------------------------------------------------------- STM reset

TEST_CASE("web STM reset: only {\"confirm\":true} resets") {
  glue::begin();
  start();
  for (const char* bad : {"{}", "{\"confirm\":false}", "{\"confirm\":1}"}) {
    CAPTURE(bad);
    const Response r = post("/api/stm/reset", bad);
    CHECK(r.code == 400);
    CHECK(r.body == errorBody("confirm_required", "{\\\"confirm\\\":true}"));
  }
  CHECK(sib::app().submitted.empty());
  CHECK(postCode("/api/stm/reset", "{\"confirm\":true}") == 202);
  REQUIRE(sib::app().submitted.size() == 1);
  CHECK(sib::app().submitted[0].type == app::CommandType::ResetStm);
  CHECK(sib::app().submitted[0].valve == vdm::kNoValve);
}

// ---------------------------------------------------------------- images

TEST_CASE("web images: the list answers every image once") {
  glue::begin();
  storage::ImageEntry a = image("fw", "C2");
  a.size = 5;
  a.crc = 0xabc;
  storage::ImageEntry b;
  vdm::copyString(b.name, sizeof b.name, "old");
  sib::storage().images = {a, b};
  start();
  const Response r = fakes::http::perform(fakes::http::get("/api/stm/images"));
  CHECK(r.code == 200);
  CHECK(r.body ==
        "[{\"name\":\"fw\",\"size\":5,\"crc32\":\"0x00000abc\",\"version\":null,\"check\":\"none\","
        "\"hw\":\"C2\"},{\"name\":\"old\",\"size\":0,\"crc32\":null,\"version\":null,"
        "\"check\":null,\"hw\":null}]");
  sib::storage().images.clear();
  CHECK(fakes::http::perform(fakes::http::get("/api/stm/images")).body == "[]");
}

TEST_CASE("web image delete: each storage result has its answer") {
  glue::begin();
  start();
  Response r = fakes::http::perform(apiDel("/api/stm/images/fw.bin"));
  CHECK(r.code == 204);
  CHECK(sib::storage().deletedImages == std::vector<std::string>{"fw"});
  const std::string longest(31, 'a');
  CHECK(fakes::http::perform(apiDel("/api/stm/images/" + longest)).code == 204);
  CHECK(sib::storage().deletedImages.back() == longest);
  sib::storage().deleteImageResult = storage::ImageResult::NotFound;
  r = fakes::http::perform(apiDel("/api/stm/images/fw"));
  CHECK(r.code == 404);
  CHECK(r.body == errorBody("not_found", "fw"));
  sib::storage().deleteImageResult = storage::ImageResult::Busy;
  r = fakes::http::perform(apiDel("/api/stm/images/fw"));
  CHECK(r.code == 409);
  CHECK(r.body == errorBody("busy", "fw"));
  sib::storage().deleteImageResult = storage::ImageResult::Io;
  r = fakes::http::perform(apiDel("/api/stm/images/fw"));
  CHECK(r.code == 500);
  CHECK(r.body == errorBody("io_error", "fw"));
  sib::storage().deleteImageResult = storage::ImageResult::Ok;
  sib::app().flashActive = true;
  const size_t before = sib::storage().deletedImages.size();
  r = fakes::http::perform(apiDel("/api/stm/images/fw"));
  CHECK(r.code == 409);
  CHECK(r.body == errorBody("flashing", "STM flash in progress"));
  CHECK(sib::storage().deletedImages.size() == before);
}

// ---------------------------------------------------------------- flash

TEST_CASE("web flash: the request members") {
  glue::begin();
  const std::string longest(31, 'b');
  sib::storage().images = {image("fw", ""), image(longest.c_str(), "")};
  start();
  const char* detail = "image, mode normal|blank, force, board C1|C2";
  for (const char* bad :
       {"{}", "{\"image\":5}", "{\"image\":\"fw\",\"board\":5}", "{\"image\":\"fw\",\"board\":\"C3\"}",
        "{\"image\":\"a b\"}", "{\"image\":\"fw\",\"mode\":\"fast\"}", "{\"image\":\"fw\",\"mode\":1}",
        "{\"image\":\"fw\",\"force\":1}", "{\"image\":\"fw\",\"x\":1}"}) {
    CAPTURE(bad);
    const Response r = post("/api/stm/flash", bad);
    CHECK(r.code == 400);
    CHECK(r.body == errorBody("bad_request", detail));
  }
  CHECK(sib::app().submitted.empty());
  CHECK(postCode("/api/stm/flash", "{\"image\":\"fw\",\"board\":\"C2\",\"mode\":\"normal\"}") == 202);
  CHECK(postCode("/api/stm/flash", "{\"image\":\"" + longest + ".bin\",\"force\":false}") == 202);
  REQUIRE(sib::app().submitted.size() == 2);
  const app::Command& c = sib::app().submitted[0];
  CHECK(c.type == app::CommandType::StartFlash);
  CHECK(std::string(c.image) == "fw");
  CHECK(std::string(c.board) == "C2");
  CHECK_FALSE(c.blank);
  CHECK_FALSE(c.force);
  CHECK(std::string(sib::app().submitted[1].image) == longest);
  sib::app().submitResult = false;
  CHECK(postCode("/api/stm/flash", "{\"image\":\"fw\"}") == 503);
}

TEST_CASE("web flash: busy, restarting, unknown and unchecked images are refused") {
  glue::begin();
  storage::ImageEntry pending = image("pending", "");
  pending.scanned = false;
  storage::ImageEntry nohs = image("nohs", "");
  nohs.check = vdm::FlashError::ImageNoHandshake;
  storage::ImageEntry bad = image("bad", "");
  bad.check = vdm::FlashError::ImageBadVectors;
  sib::storage().images = {image("fw", ""), pending, nohs, bad};
  start();
  sib::ota().uploadActive = true;
  Response r = post("/api/stm/flash", "{\"image\":\"fw\"}");
  CHECK(r.code == 409);
  CHECK(r.body == errorBody("busy", "upload or flash running"));
  sib::ota().uploadActive = false;
  sib::ota().restartPending = true;
  r = post("/api/stm/flash", "{\"image\":\"fw\"}");
  CHECK(r.code == 409);
  CHECK(r.body == errorBody("restarting", "ESP restart pending"));
  sib::ota().restartPending = false;
  r = post("/api/stm/flash", "{\"image\":\"none\"}");
  CHECK(r.code == 404);
  CHECK(r.body == errorBody("not_found", "none"));
  r = post("/api/stm/flash", "{\"image\":\"pending\"}");
  CHECK(r.code == 409);
  CHECK(r.body == errorBody("validating", "image check pending, retry"));
  r = post("/api/stm/flash", "{\"image\":\"nohs\"}");
  CHECK(r.code == 400);
  CHECK(r.body == errorBody("invalid_image", "image_no_handshake"));
  r = post("/api/stm/flash", "{\"image\":\"bad\",\"force\":true}");
  CHECK(r.code == 400);
  CHECK(r.body == errorBody("invalid_image", "image_bad_vectors"));
  CHECK(sib::app().submitted.empty());
  // force flashes an image without the handshake marker
  r = post("/api/stm/flash", "{\"image\":\"nohs\",\"force\":true}");
  CHECK(r.code == 202);
  REQUIRE(sib::app().submitted.size() == 1);
  CHECK(sib::app().submitted[0].force);
  CHECK(std::string(sib::app().submitted[0].image) == "nohs");
}

TEST_CASE("web flash: the running STM's board tag decides, even a one-letter one") {
  glue::begin();
  sib::storage().images = {image("fw", "C1")};
  vdm::copyString(sib::app().snapshot.version.hw, sizeof sib::app().snapshot.version.hw, "C");
  start();
  const Response r = post("/api/stm/flash", "{\"image\":\"fw\",\"board\":\"C1\"}");
  CHECK(r.code == 409);
  CHECK(r.body == errorBody("board_mismatch", "image C1, board C"));
  CHECK(sib::app().submitted.empty());
}

TEST_CASE("web flash abort: only a running flash is aborted") {
  glue::begin();
  start();
  Response r = post("/api/stm/flash/abort", "");
  CHECK(r.code == 409);
  CHECK(r.body == errorBody("idle", "no flash running"));
  CHECK(sib::app().submitted.empty());
  sib::app().flashActive = true;
  r = post("/api/stm/flash/abort", "");
  CHECK(r.code == 202);
  CHECK(r.body == "{\"result\":\"queued\"}");
  REQUIRE(sib::app().submitted.size() == 1);
  CHECK(sib::app().submitted[0].type == app::CommandType::AbortFlash);
  sib::app().submitResult = false;
  CHECK(postCode("/api/stm/flash/abort", "") == 503);
}

// ---------------------------------------------------------------- factory reset

TEST_CASE("web factory reset: confirmed, erased, restart requested") {
  glue::begin();
  start();
  for (const char* bad : {"{}", "{\"confirm\":\"yes\"}", "{\"confirm\":true}"}) {
    CAPTURE(bad);
    const Response r = post("/api/system/factory-reset", bad);
    CHECK(r.code == 400);
    CHECK(r.body == errorBody("confirm_required", "{\\\"confirm\\\":\\\"factory-reset\\\"}"));
  }
  CHECK(sib::storage().factoryResets == 0);
  sib::app().flashActive = true;
  Response r = post("/api/system/factory-reset", "{\"confirm\":\"factory-reset\"}");
  CHECK(r.code == 409);
  CHECK(r.body == errorBody("flashing", "STM flash in progress"));
  CHECK(sib::storage().factoryResets == 0);
  sib::app().flashActive = false;
  sib::storage().factoryResetResult = false;
  r = post("/api/system/factory-reset", "{\"confirm\":\"factory-reset\"}");
  CHECK(r.code == 500);
  CHECK(r.body == errorBody("nvs", "erase failed"));
  CHECK(sib::ota().restartRequests.empty());
  sib::storage().factoryResetResult = true;
  r = post("/api/system/factory-reset", "{\"confirm\":\"factory-reset\"}");
  CHECK(r.code == 202);
  CHECK(r.body == "{\"result\":\"restarting\"}");
  CHECK(sib::storage().factoryResets == 2);
  REQUIRE(sib::ota().restartRequests.size() == 1);
  CHECK(sib::ota().restartRequests[0].reason == 3);
  CHECK(sib::ota().restartRequests[0].delayMs == 1000);
}

// ---------------------------------------------------------------- discovery

TEST_CASE("web MQTT discovery: an unknown action is refused") {
  glue::begin();
  sib::storage().active.mqtt.mode = vdm::MqttMode::Mqtt;
  start();
  for (const char* bad : {"{}", "{\"action\":\"clear\"}", "{\"action\":1}"}) {
    CAPTURE(bad);
    const Response r = post("/api/mqtt/discovery", bad);
    CHECK(r.code == 400);
    CHECK(r.body == errorBody("bad_request", "action publish|delete|republish"));
  }
  CHECK(sib::mqtt().discoveryRequests.empty());
}
