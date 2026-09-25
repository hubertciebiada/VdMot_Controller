#include "runner_hooks.h"

#include <stdio.h>
#include <string.h>

#include <string>

#include "fakes/fakes.h"
#include "fakes/http.h"
#include "testkit.h"

namespace {

using testkit::Reset;

struct Section {
  uint8_t* start;
  size_t size;
};

Section rtc() {
  uint8_t* start = nullptr;
  size_t size = 0;
  if (!testkit::sectionBounds("vdm_rtc_noinit", start, size)) return {nullptr, 0};
  return {start, size};
}

// Variables in a named section carry no ASan redzones; the whole section is plain memory.
__attribute__((no_sanitize_address)) void rawCopy(volatile uint8_t* dst,
                                                  const volatile uint8_t* src, size_t n) {
  for (size_t i = 0; i < n; i++) dst[i] = src[i];
}

__attribute__((no_sanitize_address)) void rawFill(volatile uint8_t* dst, uint8_t value, size_t n) {
  for (size_t i = 0; i < n; i++) dst[i] = value;
}

esp_reset_reason_t reasonOf(Reset kind) {
  switch (kind) {
    case Reset::PowerOn: return ESP_RST_POWERON;
    case Reset::Pin: return ESP_RST_EXT;
    case Reset::Software: return ESP_RST_SW;
    case Reset::Watchdog: return ESP_RST_TASK_WDT;
  }
  return ESP_RST_UNKNOWN;
}

// ---- hand-off file: length-prefixed fields

void putU32(std::string& out, uint32_t v) {
  for (int i = 0; i < 4; ++i) out.push_back(static_cast<char>(v >> (8 * i)));
}

void putBytes(std::string& out, const void* data, size_t n) {
  putU32(out, static_cast<uint32_t>(n));
  out.append(static_cast<const char*>(data), n);
}

void putStr(std::string& out, const std::string& s) { putBytes(out, s.data(), s.size()); }

struct Reader {
  const std::string& in;
  size_t pos = 0;
  bool ok = true;
  uint32_t u32() {
    if (pos + 4 > in.size()) {
      ok = false;
      return 0;
    }
    uint32_t v = 0;
    for (int i = 3; i >= 0; --i) v = (v << 8) | static_cast<uint8_t>(in[pos + i]);
    pos += 4;
    return v;
  }
  std::string str() {
    const uint32_t n = u32();
    if (!ok || pos + n > in.size()) {
      ok = false;
      return "";
    }
    const std::string s = in.substr(pos, n);
    pos += n;
    return s;
  }
};

constexpr const char* kMagic = "VDMESP01";

void powerOn() {
  const Section s = rtc();
  if (s.size != 0) rawFill(s.start, 0xA5, s.size);
  fakes::esp().resetReason = ESP_RST_POWERON;
}

bool save(const char* path) {
  std::string out(kMagic);
  const Section s = rtc();
  std::string warm(s.size, '\0');
  if (s.size != 0) rawCopy(reinterpret_cast<uint8_t*>(&warm[0]), s.start, s.size);
  putStr(out, warm);
  const fakes::Nvs& nvs = fakes::nvs();
  putU32(out, static_cast<uint32_t>(nvs.ns.size()));
  for (const auto& n : nvs.ns) {
    putStr(out, n.first);
    putU32(out, static_cast<uint32_t>(n.second.size()));
    for (const auto& k : n.second) {
      putStr(out, k.first);
      putU32(out, static_cast<uint32_t>(k.second.type));
      putBytes(out, k.second.bytes.data(), k.second.bytes.size());
    }
  }
  const fakes::Fs& fs = fakes::fs();
  putU32(out, fs.formatted ? 1 : 0);
  putU32(out, static_cast<uint32_t>(fs.nodes.size()));
  for (const auto& n : fs.nodes) {
    putStr(out, n.first);
    putU32(out, n.second.dir ? 1 : 0);
    putBytes(out, n.second.data.data(), n.second.data.size());
  }
  const fakes::Ota& ota = fakes::ota();
  putU32(out, static_cast<uint32_t>(ota.state[0]));
  putU32(out, static_cast<uint32_t>(ota.state[1]));
  putU32(out, static_cast<uint32_t>(ota.running));
  putU32(out, static_cast<uint32_t>(ota.boot));
  FILE* f = fopen(path, "wb");
  if (f == nullptr) return false;
  const bool written = fwrite(out.data(), 1, out.size(), f) == out.size();
  return fclose(f) == 0 && written;
}

bool load(const char* path) {
  FILE* f = fopen(path, "rb");
  if (f == nullptr) return false;
  std::string in;
  char buf[4096];
  size_t n;
  while ((n = fread(buf, 1, sizeof buf, f)) > 0) in.append(buf, n);
  fclose(f);
  if (in.compare(0, strlen(kMagic), kMagic) != 0) return false;
  Reader r{in, strlen(kMagic)};
  const std::string warm = r.str();
  const Section s = rtc();
  if (!r.ok || warm.size() != s.size) return false;
  if (s.size != 0) rawCopy(s.start, reinterpret_cast<const uint8_t*>(warm.data()), s.size);
  fakes::Nvs& nvs = fakes::nvs();
  nvs.ns.clear();
  const uint32_t namespaces = r.u32();
  for (uint32_t i = 0; i < namespaces && r.ok; ++i) {
    const std::string name = r.str();
    const uint32_t keys = r.u32();
    auto& ns = nvs.ns[name];
    for (uint32_t k = 0; k < keys && r.ok; ++k) {
      const std::string key = r.str();
      const auto type = static_cast<fakes::NvsType>(r.u32());
      const std::string bytes = r.str();
      ns[key] = fakes::NvsEntry{type, std::vector<uint8_t>(bytes.begin(), bytes.end())};
    }
  }
  fakes::Fs& fs = fakes::fs();
  fs.formatted = r.u32() != 0;
  fs.nodes.clear();
  const uint32_t nodes = r.u32();
  for (uint32_t i = 0; i < nodes && r.ok; ++i) {
    const std::string p = r.str();
    const bool dir = r.u32() != 0;
    const std::string data = r.str();
    fakes::FsNode& node = fs.nodes[p];
    node.dir = dir;
    node.data.assign(data.begin(), data.end());
  }
  fakes::Ota& ota = fakes::ota();
  ota.state[0] = static_cast<esp_ota_img_states_t>(r.u32());
  ota.state[1] = static_cast<esp_ota_img_states_t>(r.u32());
  ota.running = static_cast<int>(r.u32());
  ota.boot = static_cast<int>(r.u32());
  if (!r.ok || r.pos != in.size()) return false;
  ota.bootloader();
  fakes::esp().resetReason = reasonOf(testkit::lastReset());
  return true;
}

const char* checkInvariants() {
  static std::string message;
  const fakes::Rtos& rtos = fakes::rtos();
  if (rtos.violations != 0) {
    message = std::to_string(rtos.violations) + " RTOS violation(s), the first: " +
              (rtos.violationLog.empty() ? std::string("?") : rtos.violationLog.front());
    return message.c_str();
  }
  if (rtos.criticalDepth != 0) {
    message = "a critical section is still entered (depth " + std::to_string(rtos.criticalDepth) +
              ")";
    return message.c_str();
  }
  const std::vector<std::string>& http = fakes::http::server().violations;
  if (!http.empty()) {
    message = "HTTP request not answered exactly once: " + http.front();
    return message.c_str();
  }
  return nullptr;
}

struct RegisterHooks {
  RegisterHooks() { testkit::setHooks({powerOn, save, load, checkInvariants}); }
} g_registerHooks;

}  // namespace

namespace glue {

std::vector<uint8_t> rtcSnapshot() {
  const Section s = rtc();
  std::vector<uint8_t> out(s.size);
  if (s.size != 0) rawCopy(out.data(), s.start, s.size);
  return out;
}

}  // namespace glue
