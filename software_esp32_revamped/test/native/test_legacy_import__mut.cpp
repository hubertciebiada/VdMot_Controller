// legacy_import: a station name starting with a control character, the report of the first item
// and an empty root topic.
#include <stdint.h>
#include <string.h>

#include <map>
#include <string>

#include "doctest.h"
#include "vdm/config.h"
#include "vdm/json_writer.h"
#include "vdm/legacy_import.h"

using namespace vdm;

namespace {

// In-memory NVS with string keys only.
class StrNvs : public LegacyNvsReader {
 public:
  std::map<std::string, std::string> strs;

  bool readInt(const char*, const char*, int64_t&) override { return false; }
  bool readString(const char* ns, const char* key, char* out, size_t cap, bool& truncated) override {
    auto it = strs.find(std::string(ns) + "/" + key);
    if (it == strs.end() || cap == 0) return false;
    const size_t n = it->second.size() < cap - 1 ? it->second.size() : cap - 1;
    memcpy(out, it->second.data(), n);
    out[n] = '\0';
    truncated = it->second.size() > cap - 1;
    return true;
  }
  bool readBlob(const char*, const char*, uint8_t*, size_t, size_t&) override { return false; }
};

std::string reportJson(const ImportReport& r, const Config& c) {
  static char buf[1024];
  JsonWriter jw(buf, sizeof buf);
  REQUIRE(writeImportReportJson(jw, r, c));
  return std::string(buf, jw.length());
}

}  // namespace

TEST_CASE("legacy: a station name starting with a control character is rejected, not taken as empty") {
  StrNvs n;
  n.strs["sysCfg/stName"] = "\x01VdMot";
  Config c;
  const ImportReport r = importLegacyConfig(n, c);
  CHECK(r.rejected == 1);
  CHECK(r.imported == 0);
  CHECK(std::string(r.firstRejected) == "sysCfg/stName");
  Config d;
  setDefaults(d);
  CHECK(std::string(c.mqtt.rootTopic) == d.mqtt.rootTopic);
  CHECK(std::string(c.station) == d.station);
}

TEST_CASE("writeImportReportJson: the first valve in renamed, an empty root topic is null") {
  ImportReport r;
  r.renamedValves = 1u << 0;
  Config c;
  setDefaults(c);
  c.mqtt.rootTopic[0] = '\0';
  strcpy(c.valves[0].name, "a_b");
  strcpy(c.valves[0].topic, "a/b");
  const std::string j = reportJson(r, c);
  CHECK(j.find("\"rootTopic\":null") != std::string::npos);
  CHECK(j.find("\"renamed\":[{\"kind\":\"valve\",\"n\":1,\"name\":\"a_b\",\"topic\":\"a/b\"}]") !=
        std::string::npos);
}
