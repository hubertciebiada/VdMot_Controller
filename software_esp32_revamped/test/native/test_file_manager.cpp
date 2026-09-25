// file_manager: path syntax, kinds and deletability (every row of the
// table), kind names and protect reasons, legacy image names, the
// /api/files document.
#include <string.h>

#include <string>

#include "doctest.h"
#include "vdm/file_manager.h"
#include "vdm/json_writer.h"

using namespace vdm;

namespace {

bool valid(const std::string& p) { return fsPathValid(p.data(), p.size()); }
FileKind kind(const std::string& p) { return classifyFsPath(p.data(), p.size()); }
bool legacy(const std::string& p) { return isLegacyImageFile(p.data(), p.size()); }

}  // namespace

TEST_CASE("file_manager: path syntax") {
  CHECK(valid("/a"));
  CHECK(valid("/stm/x.bin"));
  CHECK(valid("/Bad Name.bin"));
  CHECK(valid("/\xC5\x81" "azienka.bin"));  // UTF-8
  CHECK(valid("/.x"));
  CHECK(valid("/..x"));
  CHECK(valid("/a/.b/c"));
  CHECK(valid("/" + std::string(94, 'a')));  // 95 bytes
  CHECK_FALSE(valid("/" + std::string(95, 'a')));  // 96 bytes
  CHECK_FALSE(valid(""));
  CHECK_FALSE(valid("/"));
  CHECK_FALSE(valid("a"));
  CHECK_FALSE(valid("ab"));
  CHECK_FALSE(valid("//a"));
  CHECK_FALSE(valid("/a//b"));
  CHECK_FALSE(valid("/a/"));
  CHECK_FALSE(valid("/."));
  CHECK_FALSE(valid("/.."));
  CHECK_FALSE(valid("/a/../b"));
  CHECK_FALSE(valid("/a/./b"));
  CHECK_FALSE(valid("/a/.."));
  CHECK_FALSE(valid("/a\x01"));
  CHECK_FALSE(valid(std::string("/a\0b", 4)));
  CHECK_FALSE(fsPathValid(nullptr, 3));
}

TEST_CASE("file_manager: kinds, deletability and reasons") {
  struct Row {
    const char* path;
    FileKind kind;
    const char* name;
    bool deletable;
    const char* reason;
  };
  const Row rows[] = {
      {"/stm/x.bin", FileKind::StmImage, "stm_image", false, "use DELETE /api/stm/images/<name>"},
      {"/stm/x.bin.part", FileKind::UploadPart, "upload_part", true, ""},
      {"/stm/y.part", FileKind::UploadPart, "upload_part", true, ""},
      {"/stm/notes.txt", FileKind::Other, "other", true, ""},
      {"/stm/X.BIN", FileKind::Other, "other", true, ""},
      {"/stm/a/b.bin", FileKind::Other, "other", true, ""},
      {"/log/events.log", FileKind::Log, "log", false, "event log"},
      {"/log/a/b", FileKind::Log, "log", false, "event log"},
      {"/sys/cfg.bak", FileKind::Internal, "internal", false, "internal file"},
      {"/sys/import.json", FileKind::Internal, "internal", false, "internal file"},
      {"/HADiscovery.cfg", FileKind::LegacyHaList, "legacy_ha_list", false,
       "kept for a rollback to the legacy firmware"},
      {"/HADiscovery.cfg.done", FileKind::Other, "other", true, ""},
      {"/HADiscovery.cf", FileKind::Other, "other", true, ""},
      {"/STM.BIN", FileKind::LegacyImage, "legacy_image", true, ""},
      {"/fw.bin", FileKind::LegacyImage, "legacy_image", true, ""},
      {"/log", FileKind::Other, "other", true, ""},
      {"/sysx/a", FileKind::Other, "other", true, ""},
      {"/index.html", FileKind::Other, "other", true, ""},
  };
  for (const Row& r : rows) {
    CAPTURE(r.path);
    const FileKind k = kind(r.path);
    CHECK(k == r.kind);
    CHECK(std::string(fileKindName(k)) == r.name);
    CHECK(fileDeletable(k) == r.deletable);
    CHECK(std::string(fileProtectReason(k)) == r.reason);
  }
  CHECK(std::string(fileKindName(static_cast<FileKind>(99))) == "other");
  CHECK(std::string(fileProtectReason(static_cast<FileKind>(99))).empty());
}

TEST_CASE("file_manager: legacy image names") {
  CHECK(legacy("/a.bin"));
  CHECK(legacy("/A.BIN"));
  CHECK(legacy("/a.Bin"));
  CHECK(legacy("/a.biN"));
  CHECK(legacy("/a.bIn"));
  CHECK_FALSE(legacy("/.bin"));
  CHECK_FALSE(legacy("/stm/a.bin"));
  CHECK_FALSE(legacy("/a.bin.part"));
  CHECK_FALSE(legacy("/a.bi"));
  CHECK_FALSE(legacy("/a.bix"));
  CHECK_FALSE(legacy("/a.xin"));
  CHECK_FALSE(legacy("/a_bin"));
  CHECK_FALSE(legacy("/aabin"));
  CHECK_FALSE(legacy("a.bin"));
  CHECK_FALSE(legacy("xa.bin"));
  CHECK_FALSE(isLegacyImageFile(nullptr, 6));
}

TEST_CASE("file_manager: the /api/files document") {
  FileEntry files[3] = {};
  strcpy(files[0].path, "/stm/x.bin");
  files[0].size = 98304;
  strcpy(files[1].path, "/Bad \"1\".bin");
  files[1].size = 7;
  strcpy(files[2].path, "/sys/cfg.bak");
  files[2].size = 0;
  char buf[512];
  JsonWriter jw(buf, sizeof buf);
  CHECK(writeFilesJson(jw, files, 3, 1507328, 204800, true));
  CHECK(std::string(buf, jw.length()) ==
        "{\"total\":1507328,\"used\":204800,\"truncated\":true,\"files\":["
        "{\"path\":\"/stm/x.bin\",\"size\":98304,\"kind\":\"stm_image\",\"deletable\":false},"
        "{\"path\":\"/Bad \\\"1\\\".bin\",\"size\":7,\"kind\":\"legacy_image\",\"deletable\":true},"
        "{\"path\":\"/sys/cfg.bak\",\"size\":0,\"kind\":\"internal\",\"deletable\":false}]}");
  JsonWriter empty(buf, sizeof buf);
  CHECK(writeFilesJson(empty, files, 0, 0, 0, false));
  CHECK(std::string(buf, empty.length()) ==
        "{\"total\":0,\"used\":0,\"truncated\":false,\"files\":[]}");
  JsonWriter small(buf, 30);
  CHECK_FALSE(writeFilesJson(small, files, 3, 1, 1, false));
}
