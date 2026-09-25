// Request guard of the HTTP API: Host, Origin, the X-VdMot marker and the
// JSON Content-Type, checked in this order for every /api/* request and the
// legacy aliases before anything is buffered. Hardware-free.
#pragma once

#include <stddef.h>
#include <stdint.h>

#include "vdm/json_api.h"

namespace vdm {

enum class GuardScope : uint8_t { Static, Api, LegacyRead, LegacyWrite };
enum class GuardVerdict : uint8_t {
  Allow = 0,
  BadHost = 1,
  BadOrigin = 2,
  MissingHeader = 3,
  BadContentType = 4
};

struct GuardRequest {
  HttpMethod method = HttpMethod::Get;
  GuardScope scope = GuardScope::Static;
  bool upload = false;   // StmImageUpload / EspOta route
  bool hasBody = false;  // Content-Length > 0
  const char* host = nullptr;
  size_t hostLen = 0;  // Host header value
  const char* origin = nullptr;
  size_t originLen = 0;  // nullptr = no Origin header
  const char* marker = nullptr;
  size_t markerLen = 0;  // X-VdMot value, nullptr = absent
  const char* contentType = nullptr;
  size_t contentTypeLen = 0;  // media type without parameters
};

struct HostPolicy {
  uint32_t localIp = 0;       // connection's local address (legacy uint32 layout)
  uint32_t ifaceIp = 0;       // current interface address
  const char* hostname = "";  // buildHostname(station)
  const char* allowed = "";   // web.allowedHosts
};

// Host rule: absent/empty ok. One ":<1..5 digits>" suffix and one trailing
// '.' are stripped; a dotted IPv4 must equal localIp, ifaceIp or an IPv4
// entry of `allowed` (address 0 never matches); a name must equal
// (case-insensitive) hostname, hostname + ".local" or a name entry of
// `allowed`. Anything else (IPv6 literals included) fails.
bool hostAllowed(const char* host, size_t len, const HostPolicy& p);
// Origin rule: "http://" or "https://" (case-insensitive) followed by a
// non-empty host part without '/' that hostAllowed() accepts. "null" fails.
bool originAllowed(const char* origin, size_t len, const HostPolicy& p);
// Static: always Allow. Else Host, Origin (when present), X-VdMot == "1"
// for POST/DELETE in scope Api, Content-Type application/json
// (case-insensitive) for a POST with a body that is not an upload.
GuardVerdict checkRequest(const GuardRequest& r, const HostPolicy& p);
// "host_not_allowed", "origin_not_allowed", "header_required",
// "unsupported_media_type"; "" for Allow.
const char* guardErrorCode(GuardVerdict v);
// 403 / 415, 200 for Allow.
uint16_t guardHttpStatus(GuardVerdict v);
// Detail text of a refusal: BadHost "<host, max 48>: use <local IP> or add
// the name to web.allowedHosts" (local IP = localIp, else ifaceIp);
// BadOrigin "<origin, max 48>"; MissingHeader "X-VdMot: 1"; BadContentType
// "application/json required"; Allow "". Returns the length.
size_t guardDetail(GuardVerdict v, const GuardRequest& r, const HostPolicy& p, char* out,
                   size_t cap);

// At most one "yes" per key and interval (event throttling).
class RepeatLimiter {
 public:
  static constexpr uint8_t kKeys = 8;
  explicit RepeatLimiter(uint32_t intervalMs = 60000) : intervalMs_(intervalMs) {}
  // First call per key true, then true again once intervalMs passed since
  // the last true; key >= kKeys false.
  bool allow(uint8_t key, uint32_t nowMs);

 private:
  uint32_t intervalMs_;
  bool seen_[kKeys] = {};
  uint32_t lastMs_[kKeys] = {};
};

}  // namespace vdm
