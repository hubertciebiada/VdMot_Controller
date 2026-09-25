// Tests for vdm/web_guard.h: Host/Origin rules, the request guard matrix, the
// refusal texts and the repeat limiter.
#include <string.h>

#include <string>

#include "doctest.h"
#include "vdm/web_guard.h"

using namespace vdm;

namespace {

constexpr uint32_t kLocal = 0x3301A8C0;  // 192.168.1.51
constexpr uint32_t kIface = 0x0A00000A;  // 10.0.0.10

HostPolicy policy(const char* allowed = "") {
  HostPolicy p;
  p.localIp = kLocal;
  p.ifaceIp = kIface;
  p.hostname = "vdmot-east";
  p.allowed = allowed;
  return p;
}

bool host(const char* h, const HostPolicy& p = policy()) { return hostAllowed(h, strlen(h), p); }
bool origin(const char* o, const HostPolicy& p = policy()) {
  return originAllowed(o, strlen(o), p);
}

GuardRequest request(HttpMethod m, GuardScope s) {
  GuardRequest r;
  r.method = m;
  r.scope = s;
  r.host = "192.168.1.51";
  r.hostLen = strlen(r.host);
  return r;
}

void setMarker(GuardRequest& r, const char* v) {
  r.marker = v;
  r.markerLen = v ? strlen(v) : 0;
}

void setType(GuardRequest& r, const char* v) {
  r.contentType = v;
  r.contentTypeLen = v ? strlen(v) : 0;
  r.hasBody = true;
}

std::string refusalText(GuardVerdict v, const GuardRequest& r, const HostPolicy& p, size_t cap = 160) {
  char buf[160];
  const size_t n = guardDetail(v, r, p, buf, cap);
  CHECK(n == strlen(buf));
  return buf;
}

}  // namespace

TEST_CASE("hostAllowed: own addresses and names") {
  CHECK(host("192.168.1.51"));
  CHECK(host("10.0.0.10"));
  CHECK(host("192.168.1.51:80"));
  CHECK(host("192.168.1.51:65535"));
  CHECK(host("192.168.1.51:1"));
  CHECK(host("192.168.1.51."));
  CHECK(host("vdmot-east"));
  CHECK(host("VDMOT-EAST.local"));
  CHECK(host("vdmot-east.local."));
  CHECK(host("vdmot-east.LOCAL:8080"));
  CHECK(host("vdmot-east:80"));
}

TEST_CASE("hostAllowed: foreign hosts fail") {
  CHECK_FALSE(host("vdmot-east.evil.com"));
  CHECK_FALSE(host("vdmot-east.localx"));
  CHECK_FALSE(host("vdmot-eas"));
  CHECK_FALSE(host("xvdmot-east"));
  CHECK_FALSE(host("vdmot-east.loca"));
  CHECK_FALSE(host("192.168.1.52"));
  CHECK_FALSE(host("[::1]:80"));
  CHECK_FALSE(host("[::1]"));
  CHECK_FALSE(host("192.168.1.51:123456"));
  CHECK_FALSE(host("192.168.1.51:"));
  CHECK_FALSE(host("192.168.1.51:8a"));
  CHECK_FALSE(host("192.168.1.51:80:80"));
  CHECK_FALSE(host("192.168.1.300"));
  CHECK_FALSE(host("."));
  CHECK_FALSE(host(":80"));
  CHECK_FALSE(host("vdmot-east.."));
  HostPolicy vd = policy();
  vd.hostname = "VdMot";
  CHECK_FALSE(host("vdmot.evil.com", vd));
  CHECK(host("vdmot.local", vd));
}

TEST_CASE("hostAllowed: absent and empty hosts are fine") {
  CHECK(hostAllowed(nullptr, 0, policy()));
  CHECK(hostAllowed("x", 0, policy()));
}

TEST_CASE("hostAllowed: address 0 never matches") {
  HostPolicy p = policy();
  p.localIp = 0;
  p.ifaceIp = 0;
  CHECK_FALSE(host("0.0.0.0", p));
  CHECK_FALSE(host("0.0.0.0", policy("0.0.0.0")));
  CHECK_FALSE(host("192.168.1.51", p));
  CHECK(host("vdmot-east", p));
}

TEST_CASE("hostAllowed: the allow list takes names and addresses") {
  const HostPolicy p = policy(" vdmot.lan , 192.168.2.7,Heating.Example.org ");
  CHECK(host("vdmot.lan", p));
  CHECK(host("VDMOT.LAN:81", p));
  CHECK(host("192.168.2.7", p));
  CHECK(host("heating.example.org", p));
  CHECK_FALSE(host("192.168.2.8", p));
  CHECK_FALSE(host("vdmot", p));
  CHECK_FALSE(host("vdmot.lan.local", p));
  CHECK_FALSE(host("heating.example.or", p));
  // a name entry never matches an address and the other way round
  CHECK_FALSE(host("192.168.2.7", policy("vdmot.lan")));
  CHECK_FALSE(host("x", policy("1.2.3.4")));
  CHECK_FALSE(host("1.2.3.4", policy("1.2.3.4x")));
  CHECK(host("1.2.3.4", policy(",1.2.3.4")));
  CHECK(host("b", policy("a,,b")));
  CHECK_FALSE(host("b", policy("a,b.")));
  HostPolicy none = policy();
  none.allowed = nullptr;
  none.hostname = nullptr;
  CHECK_FALSE(host("vdmot-east", none));
  CHECK(host("192.168.1.51", none));
}

TEST_CASE("originAllowed") {
  const HostPolicy p = policy("vdmot.lan");
  CHECK(originAllowed(nullptr, 0, p));
  CHECK(origin("http://192.168.1.51", p));
  CHECK(origin("https://vdmot.lan:8443", p));
  CHECK(origin("HTTP://vdmot-east.local", p));
  CHECK(origin("HtTpS://10.0.0.10", p));
  CHECK_FALSE(origin("null", p));
  CHECK_FALSE(origin("http://evil", p));
  CHECK_FALSE(origin("ftp://192.168.1.51", p));
  CHECK_FALSE(origin("http://192.168.1.51/x", p));
  CHECK_FALSE(origin("http://192.168.1.51/", p));
  CHECK_FALSE(origin("http://", p));
  CHECK_FALSE(origin("https://", p));
  CHECK_FALSE(origin("http:/192.168.1.51", p));
  CHECK_FALSE(origin("", p));
  CHECK_FALSE(origin("192.168.1.51", p));
}

TEST_CASE("checkRequest: static scope is never checked") {
  GuardRequest r = request(HttpMethod::Post, GuardScope::Static);
  r.host = "evil.com";
  r.hostLen = 8;
  setType(r, "text/plain");
  CHECK(checkRequest(r, policy()) == GuardVerdict::Allow);
}

TEST_CASE("checkRequest: the X-VdMot marker for API writes") {
  const HostPolicy p = policy();
  CHECK(checkRequest(request(HttpMethod::Get, GuardScope::Api), p) == GuardVerdict::Allow);
  CHECK(checkRequest(request(HttpMethod::Other, GuardScope::Api), p) == GuardVerdict::Allow);
  GuardRequest r = request(HttpMethod::Post, GuardScope::Api);
  CHECK(checkRequest(r, p) == GuardVerdict::MissingHeader);
  setMarker(r, "0");
  CHECK(checkRequest(r, p) == GuardVerdict::MissingHeader);
  setMarker(r, "11");
  CHECK(checkRequest(r, p) == GuardVerdict::MissingHeader);
  setMarker(r, "");
  CHECK(checkRequest(r, p) == GuardVerdict::MissingHeader);
  setMarker(r, "1");
  CHECK(checkRequest(r, p) == GuardVerdict::Allow);
  r.marker = "12";
  r.markerLen = 1;  // only `markerLen` bytes count
  CHECK(checkRequest(r, p) == GuardVerdict::Allow);
  GuardRequest d = request(HttpMethod::Delete, GuardScope::Api);
  CHECK(checkRequest(d, p) == GuardVerdict::MissingHeader);
  setMarker(d, "1");
  CHECK(checkRequest(d, p) == GuardVerdict::Allow);
}

TEST_CASE("checkRequest: JSON Content-Type for bodies") {
  const HostPolicy p = policy();
  GuardRequest r = request(HttpMethod::Post, GuardScope::Api);
  setMarker(r, "1");
  CHECK(checkRequest(r, p) == GuardVerdict::Allow);  // no body, no type
  setType(r, "text/plain");
  CHECK(checkRequest(r, p) == GuardVerdict::BadContentType);
  setType(r, "APPLICATION/JSON");
  CHECK(checkRequest(r, p) == GuardVerdict::Allow);
  setType(r, "application/jsonx");
  CHECK(checkRequest(r, p) == GuardVerdict::BadContentType);
  setType(r, "application/jso");
  CHECK(checkRequest(r, p) == GuardVerdict::BadContentType);
  setType(r, nullptr);
  CHECK(checkRequest(r, p) == GuardVerdict::BadContentType);
  r.hasBody = false;
  CHECK(checkRequest(r, p) == GuardVerdict::Allow);
  // uploads keep their own multipart check
  GuardRequest u = request(HttpMethod::Post, GuardScope::Api);
  u.upload = true;
  setMarker(u, "1");
  setType(u, "multipart/form-data");
  CHECK(checkRequest(u, p) == GuardVerdict::Allow);
  setMarker(u, nullptr);
  CHECK(checkRequest(u, p) == GuardVerdict::MissingHeader);
  // a DELETE with a body is not checked for its type
  GuardRequest d = request(HttpMethod::Delete, GuardScope::Api);
  setMarker(d, "1");
  setType(d, "text/plain");
  CHECK(checkRequest(d, p) == GuardVerdict::Allow);
}

TEST_CASE("checkRequest: legacy scopes") {
  const HostPolicy p = policy();
  GuardRequest w = request(HttpMethod::Post, GuardScope::LegacyWrite);
  setType(w, "application/json");
  CHECK(checkRequest(w, p) == GuardVerdict::Allow);  // no marker needed
  setType(w, "text/plain");
  CHECK(checkRequest(w, p) == GuardVerdict::BadContentType);
  GuardRequest rd = request(HttpMethod::Get, GuardScope::LegacyRead);
  CHECK(checkRequest(rd, p) == GuardVerdict::Allow);
  rd.host = "evil.com";
  rd.hostLen = 8;
  CHECK(checkRequest(rd, p) == GuardVerdict::BadHost);
}

TEST_CASE("checkRequest: order host, origin, marker, content type") {
  const HostPolicy p = policy();
  GuardRequest r = request(HttpMethod::Post, GuardScope::Api);
  setType(r, "text/plain");
  r.origin = "http://evil";
  r.originLen = strlen(r.origin);
  r.host = "evil.com";
  r.hostLen = 8;
  CHECK(checkRequest(r, p) == GuardVerdict::BadHost);  // bad host wins over everything
  r.host = "192.168.1.51";
  r.hostLen = 12;
  CHECK(checkRequest(r, p) == GuardVerdict::BadOrigin);
  r.origin = "http://192.168.1.51";
  r.originLen = strlen(r.origin);
  CHECK(checkRequest(r, p) == GuardVerdict::MissingHeader);
  setMarker(r, "1");
  CHECK(checkRequest(r, p) == GuardVerdict::BadContentType);
  // bad origin wins over a bad content type
  r.origin = "null";
  r.originLen = 4;
  CHECK(checkRequest(r, p) == GuardVerdict::BadOrigin);
}

TEST_CASE("guard codes and statuses") {
  CHECK(std::string(guardErrorCode(GuardVerdict::Allow)) == "");
  CHECK(std::string(guardErrorCode(GuardVerdict::BadHost)) == "host_not_allowed");
  CHECK(std::string(guardErrorCode(GuardVerdict::BadOrigin)) == "origin_not_allowed");
  CHECK(std::string(guardErrorCode(GuardVerdict::MissingHeader)) == "header_required");
  CHECK(std::string(guardErrorCode(GuardVerdict::BadContentType)) == "unsupported_media_type");
  CHECK(std::string(guardErrorCode(static_cast<GuardVerdict>(9))) == "");
  CHECK(guardHttpStatus(GuardVerdict::Allow) == 200);
  CHECK(guardHttpStatus(GuardVerdict::BadHost) == 403);
  CHECK(guardHttpStatus(GuardVerdict::BadOrigin) == 403);
  CHECK(guardHttpStatus(GuardVerdict::MissingHeader) == 403);
  CHECK(guardHttpStatus(GuardVerdict::BadContentType) == 415);
  CHECK(guardHttpStatus(static_cast<GuardVerdict>(9)) == 200);
  CHECK(static_cast<int>(GuardVerdict::BadHost) == 1);
  CHECK(static_cast<int>(GuardVerdict::BadOrigin) == 2);
  CHECK(static_cast<int>(GuardVerdict::MissingHeader) == 3);
  CHECK(static_cast<int>(GuardVerdict::BadContentType) == 4);
}

TEST_CASE("guardDetail texts") {
  GuardRequest r = request(HttpMethod::Post, GuardScope::Api);
  r.host = "evil.com:80";
  r.hostLen = strlen(r.host);
  r.origin = "http://evil";
  r.originLen = strlen(r.origin);
  HostPolicy p = policy();
  CHECK(refusalText(GuardVerdict::BadHost, r, p) ==
        "evil.com:80: use 192.168.1.51 or add the name to web.allowedHosts");
  p.localIp = 0;
  CHECK(refusalText(GuardVerdict::BadHost, r, p) ==
        "evil.com:80: use 10.0.0.10 or add the name to web.allowedHosts");
  CHECK(refusalText(GuardVerdict::BadOrigin, r, p) == "http://evil");
  CHECK(refusalText(GuardVerdict::MissingHeader, r, p) == "X-VdMot: 1");
  CHECK(refusalText(GuardVerdict::BadContentType, r, p) == "application/json required");
  CHECK(refusalText(GuardVerdict::Allow, r, p) == "");
  // at most 48 characters of the host or origin are echoed
  const std::string longHost(60, 'h');
  r.host = longHost.c_str();
  r.hostLen = longHost.size();
  CHECK(refusalText(GuardVerdict::BadHost, r, p) ==
        std::string(48, 'h') + ": use 10.0.0.10 or add the name to web.allowedHosts");
  const std::string longOrigin = "http://" + std::string(50, 'o');
  r.origin = longOrigin.c_str();
  r.originLen = longOrigin.size();
  CHECK(refusalText(GuardVerdict::BadOrigin, r, p) == longOrigin.substr(0, 48));
  r.origin = nullptr;
  CHECK(refusalText(GuardVerdict::BadOrigin, r, p) == "");
  r.host = nullptr;
  CHECK(refusalText(GuardVerdict::BadHost, r, p) ==
        ": use 10.0.0.10 or add the name to web.allowedHosts");
  // truncation to the buffer
  CHECK(refusalText(GuardVerdict::MissingHeader, r, p, 5) == "X-Vd");
  char one[1] = {'x'};
  CHECK(guardDetail(GuardVerdict::MissingHeader, r, p, one, 1) == 0);
  CHECK(one[0] == '\0');
  CHECK(guardDetail(GuardVerdict::MissingHeader, r, p, nullptr, 10) == 0);
  CHECK(guardDetail(GuardVerdict::MissingHeader, r, p, one, 0) == 0);
}

TEST_CASE("RepeatLimiter: once per key and interval") {
  RepeatLimiter l;
  CHECK(l.allow(1, 1000));
  CHECK_FALSE(l.allow(1, 1000));
  CHECK_FALSE(l.allow(1, 60999));
  CHECK(l.allow(2, 1500));  // keys are independent
  CHECK(l.allow(1, 61000));
  CHECK_FALSE(l.allow(1, 61001));
  CHECK(l.allow(0, 0));
  CHECK(l.allow(7, 0));
  CHECK_FALSE(l.allow(8, 0));
  CHECK_FALSE(l.allow(255, 0));
}

TEST_CASE("RepeatLimiter: custom interval and millis wrap") {
  RepeatLimiter l(100);
  CHECK(l.allow(3, 0xFFFFFFC0u));
  CHECK_FALSE(l.allow(3, 0x00000023u));  // 99 ms later, across the wrap
  CHECK(l.allow(3, 0x00000024u));
  CHECK(RepeatLimiter::kKeys == 8);
}
