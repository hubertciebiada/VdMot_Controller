#include "vdm/web_guard.h"

#include <stdio.h>
#include <string.h>

#include "vdm/common.h"

namespace vdm {

namespace {

constexpr size_t kGuardEchoMax = 48;

char lower(char c) { return (c >= 'A' && c <= 'Z') ? static_cast<char>(c + ('a' - 'A')) : c; }

bool equalsNoCase(const char* a, size_t alen, const char* b, size_t blen) {
  if (alen != blen) return false;
  for (size_t i = 0; i < alen; ++i) {
    if (lower(a[i]) != lower(b[i])) return false;
  }
  return true;
}

bool digitsAndDots(const char* s, size_t len) {
  for (size_t i = 0; i < len; ++i) {
    if (!((s[i] >= '0' && s[i] <= '9') || s[i] == '.')) return false;
  }
  return true;
}

// Entry s[0..len) of web.allowedHosts against the stripped host.
bool entryMatches(const char* s, size_t len, const char* host, size_t hostLen, bool hostIsIp,
                  uint32_t hostIp) {
  if (digitsAndDots(s, len)) {
    uint32_t ip = 0;
    return hostIsIp && parseIpv4(s, len, ip) && ip == hostIp;
  }
  return !hostIsIp && equalsNoCase(s, len, host, hostLen);
}

bool listMatches(const char* list, const char* host, size_t hostLen, bool hostIsIp,
                 uint32_t hostIp) {
  const size_t len = strlen(list);
  for (size_t pos = 0; pos < len;) {
    size_t end = pos;
    while (end < len && list[end] != ',') ++end;
    size_t b = pos, e = end;
    while (b < e && list[b] == ' ') ++b;
    while (e > b && list[e - 1] == ' ') --e;
    if (e > b && entryMatches(list + b, e - b, host, hostLen, hostIsIp, hostIp)) return true;
    pos = end + 1;
  }
  return false;
}

bool startsWithNoCase(const char* s, size_t len, const char* prefix, size_t n) {
  return len >= n && equalsNoCase(s, n, prefix, n);
}

}  // namespace

bool hostAllowed(const char* host, size_t len, const HostPolicy& p) {
  if (host == nullptr || len == 0) return true;
  // one ":<1..5 digits>" suffix
  const char* colon = static_cast<const char*>(memchr(host, ':', len));
  if (colon != nullptr) {
    const size_t at = static_cast<size_t>(colon - host);
    const size_t digits = len - at - 1;
    if (digits == 0 || digits > 5) return false;
    for (size_t k = at + 1; k < len; ++k) {
      if (host[k] < '0' || host[k] > '9') return false;
    }
    len = at;
  }
  if (len > 0 && host[len - 1] == '.') --len;
  if (len == 0) return false;
  const char* allowed = p.allowed != nullptr ? p.allowed : "";
  if (digitsAndDots(host, len)) {
    uint32_t ip = 0;
    if (!parseIpv4(host, len, ip) || ip == 0) return false;
    return ip == p.localIp || ip == p.ifaceIp || listMatches(allowed, host, len, true, ip);
  }
  const char* name = p.hostname != nullptr ? p.hostname : "";
  const size_t nameLen = strlen(name);
  if (nameLen > 0) {
    if (equalsNoCase(host, len, name, nameLen)) return true;
    if (len == nameLen + 6 && equalsNoCase(host, nameLen, name, nameLen) &&
        equalsNoCase(host + nameLen, 6, ".local", 6)) {
      return true;
    }
  }
  return listMatches(allowed, host, len, false, 0);
}

bool originAllowed(const char* origin, size_t len, const HostPolicy& p) {
  if (origin == nullptr) return true;
  size_t skip = 0;
  if (startsWithNoCase(origin, len, "http://", 7)) {
    skip = 7;
  } else if (startsWithNoCase(origin, len, "https://", 8)) {
    skip = 8;
  } else {
    return false;
  }
  const char* host = origin + skip;
  const size_t hostLen = len - skip;
  if (hostLen == 0 || memchr(host, '/', hostLen) != nullptr) return false;
  return hostAllowed(host, hostLen, p);
}

GuardVerdict checkRequest(const GuardRequest& r, const HostPolicy& p) {
  if (r.scope == GuardScope::Static) return GuardVerdict::Allow;
  if (!hostAllowed(r.host, r.hostLen, p)) return GuardVerdict::BadHost;
  if (!originAllowed(r.origin, r.originLen, p)) return GuardVerdict::BadOrigin;
  const bool write = r.method == HttpMethod::Post || r.method == HttpMethod::Delete;
  if (r.scope == GuardScope::Api && write &&
      !(r.marker != nullptr && r.markerLen == 1 && r.marker[0] == '1')) {
    return GuardVerdict::MissingHeader;
  }
  if (r.method == HttpMethod::Post && r.hasBody && !r.upload &&
      !(r.contentType != nullptr &&
        equalsNoCase(r.contentType, r.contentTypeLen, "application/json", 16))) {
    return GuardVerdict::BadContentType;
  }
  return GuardVerdict::Allow;
}

const char* guardErrorCode(GuardVerdict v) {
  switch (v) {
    case GuardVerdict::BadHost: return "host_not_allowed";
    case GuardVerdict::BadOrigin: return "origin_not_allowed";
    case GuardVerdict::MissingHeader: return "header_required";
    case GuardVerdict::BadContentType: return "unsupported_media_type";
    default: return "";
  }
}

uint16_t guardHttpStatus(GuardVerdict v) {
  switch (v) {
    case GuardVerdict::BadHost:
    case GuardVerdict::BadOrigin:
    case GuardVerdict::MissingHeader: return 403;
    case GuardVerdict::BadContentType: return 415;
    default: return 200;
  }
}

size_t guardDetail(GuardVerdict v, const GuardRequest& r, const HostPolicy& p, char* out,
                   size_t cap) {
  if (out == nullptr || cap == 0) return 0;
  out[0] = '\0';
  const char* echo = v == GuardVerdict::BadHost ? r.host : r.origin;
  size_t echoLen = v == GuardVerdict::BadHost ? r.hostLen : r.originLen;
  if (echo == nullptr) echoLen = 0;
  if (echoLen > kGuardEchoMax) echoLen = kGuardEchoMax;
  int n = 0;
  switch (v) {
    case GuardVerdict::BadHost: {
      char ip[16];
      formatIpv4(p.localIp != 0 ? p.localIp : p.ifaceIp, ip, sizeof ip);
      n = snprintf(out, cap, "%.*s: use %s or add the name to web.allowedHosts",
                   static_cast<int>(echoLen), echoLen ? echo : "", ip);
      break;
    }
    case GuardVerdict::BadOrigin:
      n = snprintf(out, cap, "%.*s", static_cast<int>(echoLen), echoLen ? echo : "");
      break;
    case GuardVerdict::MissingHeader: n = snprintf(out, cap, "X-VdMot: 1"); break;
    case GuardVerdict::BadContentType: n = snprintf(out, cap, "application/json required"); break;
    default: return 0;
  }
  return static_cast<size_t>(n) < cap ? static_cast<size_t>(n) : cap - 1;
}

bool RepeatLimiter::allow(uint8_t key, uint32_t nowMs) {
  if (key >= kKeys) return false;
  if (seen_[key] && elapsedMs(nowMs, lastMs_[key]) < intervalMs_) return false;
  seen_[key] = true;
  lastMs_[key] = nowMs;
  return true;
}

}  // namespace vdm
