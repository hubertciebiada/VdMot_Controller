// web_guard edge cases: case folding at 'Z', names starting with a digit, the port suffix digits,
// one-char and empty hostnames, near-miss scheme prefixes, the refusal text limits.
#include <string.h>

#include <string>

#include "doctest.h"
#include "vdm/web_guard.h"

using namespace vdm;

namespace {

HostPolicy named(const char* hostname) {
  HostPolicy p;
  p.localIp = 0x3301A8C0;
  p.hostname = hostname;
  return p;
}

bool hostOk(const char* h, const HostPolicy& p) { return hostAllowed(h, strlen(h), p); }
bool originOk(const char* o, const HostPolicy& p) { return originAllowed(o, strlen(o), p); }

}  // namespace

TEST_CASE("web_guard: 'Z' folds to 'z'") {
  CHECK(hostOk("VDMZ", named("vdmz")));
  CHECK(hostOk("vdmz", named("VDMZ")));
}

TEST_CASE("web_guard: a name that starts with a digit is a name, not an address") {
  CHECK(hostOk("1vdm", named("1vdm")));
  CHECK_FALSE(hostOk("1vdm", named("vdm")));
}

TEST_CASE("web_guard: every char of the port suffix must be a digit, 0 and 9 included") {
  const HostPolicy p = named("vdm");
  CHECK_FALSE(hostOk("vdm:x1", p));
  CHECK_FALSE(hostOk("vdm:1x", p));
  CHECK(hostOk("vdm:9", p));
  CHECK(hostOk("vdm:0", p));
  CHECK(hostOk("vdm:90", p));
}

TEST_CASE("web_guard: a one-char hostname matches, an empty one never matches \".local\"") {
  CHECK(hostOk("v", named("v")));
  CHECK(hostOk("v.local", named("v")));
  CHECK_FALSE(hostOk(".local", named("")));
  CHECK_FALSE(hostOk("x.local", named("")));
}

TEST_CASE("web_guard: a scheme missing one '/' is not a scheme") {
  const HostPolicy p = named("vdm");
  CHECK_FALSE(originOk("http:/Xvdm", p));
  CHECK_FALSE(originOk("https:/Xvdm", p));
  CHECK(originOk("http://vdm", p));
  CHECK(originOk("https://vdm", p));
}

TEST_CASE("web_guard: the BadHost text holds the longest address") {
  HostPolicy p = named("vdm");
  p.localIp = 0xFFFFFFFF;
  GuardRequest r;
  r.host = "x";
  r.hostLen = 1;
  char out[128];
  const size_t n = guardDetail(GuardVerdict::BadHost, r, p, out, sizeof out);
  CHECK(std::string(out, n) == "x: use 255.255.255.255 or add the name to web.allowedHosts");
}

TEST_CASE("web_guard: a text of exactly cap chars is cut to cap - 1") {
  GuardRequest r;
  const HostPolicy p = named("vdm");
  char out[16];
  // "X-VdMot: 1" is 10 chars
  CHECK(guardDetail(GuardVerdict::MissingHeader, r, p, out, 11) == 10);
  CHECK(guardDetail(GuardVerdict::MissingHeader, r, p, out, 10) == 9);
  CHECK(std::string(out) == "X-VdMot: ");
}
