// The config codec of the C++ firmware 2.1.7 (software_esp32_revamped/lib/core, the code its
// storage runs) as a command-line tool for the QEMU harness (scenario nvs): the C++ side of the
// NVS `cfg`/`cfgx` blobs, since the C++ image cannot read or write the flash of Espressif's QEMU.
//
//   cpp_config encode <dir> <json>     defaults + POST /api/config patch <json> -> <dir>/cfg.bin,
//                                      <dir>/cfgx.bin (encodeConfig, encodeConfigExt)
//   cpp_config decode <cfg> <cfgx>     loadConfigBlobs -> the GET /api/config document
//
// Built by the harness: g++ -std=gnu++17 -I lib/core/include cpp_config.cpp lib/core/src/*.cpp
#include <stdio.h>
#include <string.h>

#include <string>
#include <vector>

#include "vdm/config.h"
#include "vdm/json_writer.h"

namespace {

bool writeFile(const char* path, const uint8_t* data, size_t len) {
  FILE* f = fopen(path, "wb");
  if (f == nullptr) return false;
  const bool ok = fwrite(data, 1, len, f) == len;
  return fclose(f) == 0 && ok;
}

std::vector<uint8_t> readFile(const char* path) {
  std::vector<uint8_t> out;
  FILE* f = fopen(path, "rb");
  if (f == nullptr) return out;
  uint8_t buf[512];
  size_t n;
  while ((n = fread(buf, 1, sizeof buf, f)) > 0) out.insert(out.end(), buf, buf + n);
  fclose(f);
  return out;
}

int encode(const char* dir, const char* json) {
  vdm::Config c;
  char path[72] = {0};
  const vdm::PatchResult r = vdm::applyConfigJson(c, json, strlen(json), path, sizeof path);
  if (r != vdm::PatchResult::Ok) {
    fprintf(stderr, "patch refused: result %d at %s\n", static_cast<int>(r), path);
    return 1;
  }
  static uint8_t base[vdm::kConfigBlobMax];
  static uint8_t ext[vdm::kConfigExtBlobMax];
  const size_t n = vdm::encodeConfig(c, base, sizeof base);
  const size_t x = vdm::encodeConfigExt(c, ext, sizeof ext);
  std::string b = std::string(dir) + "/cfg.bin", e = std::string(dir) + "/cfgx.bin";
  if (n == 0 || x == 0 || !writeFile(b.c_str(), base, n) || !writeFile(e.c_str(), ext, x)) {
    fprintf(stderr, "encode failed\n");
    return 1;
  }
  printf("cfg %zu B, cfgx %zu B\n", n, x);
  return 0;
}

int decode(const char* basePath, const char* extPath) {
  const std::vector<uint8_t> base = readFile(basePath), ext = readFile(extPath);
  vdm::StoredBlobs blobs;
  blobs.base = base.data();
  blobs.baseLen = base.size();
  blobs.ext = ext.empty() ? nullptr : ext.data();
  blobs.extLen = ext.size();
  vdm::Config c;
  vdm::LoadInfo info;
  if (!vdm::loadConfigBlobs(blobs, c, info)) {
    fprintf(stderr, "cfg not usable: decode result %d\n", static_cast<int>(info.base));
    return 1;
  }
  static char out[8192];
  vdm::JsonWriter jw(out, sizeof out);
  if (!vdm::writeConfigJson(jw, c)) {
    fprintf(stderr, "document does not fit\n");
    return 1;
  }
  printf("%s\n", jw.c_str());
  fprintf(stderr, "ext %d, applied %u, unknown %u, bad %u, repairs 0x%x\n",
          static_cast<int>(info.ext), info.extInfo.applied, info.extInfo.unknown, info.extInfo.bad,
          static_cast<unsigned>(info.repairs.mask));
  return 0;
}

}  // namespace

int main(int argc, char** argv) {
  if (argc == 4 && strcmp(argv[1], "encode") == 0) return encode(argv[2], argv[3]);
  if (argc == 4 && strcmp(argv[1], "decode") == 0) return decode(argv[2], argv[3]);
  fprintf(stderr, "usage: cpp_config encode <dir> <json> | decode <cfg> <cfgx>\n");
  return 2;
}
