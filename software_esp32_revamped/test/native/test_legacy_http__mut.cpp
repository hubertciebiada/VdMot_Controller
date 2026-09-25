// legacy_http: edge cases of the /valves document (a name that fills its
// whole config buffer).
#include <string.h>

#include <string>

#include "doctest.h"
#include "vdm/legacy_http.h"

using namespace vdm;

TEST_CASE("legacy /valves: a name without terminator is cut at kItemNameMax characters") {
  ValveState st;
  ValveConfig cfg;
  ValveView v;
  v.state = &st;
  v.config = &cfg;
  st.status = 1;
  memset(cfg.name, 'N', sizeof cfg.name);  // no NUL inside the buffer
  char buf[1024];
  JsonWriter jw(buf, sizeof buf);
  REQUIRE(writeLegacyValvesJson(jw, &v, 1));
  const std::string j(buf, jw.length());
  const std::string name = "\"name\":\"" + std::string(kItemNameMax, 'N') + "\",";
  CHECK(j.find(name) != std::string::npos);
}
