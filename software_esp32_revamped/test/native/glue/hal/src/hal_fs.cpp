// Fake LittleFS: an in-memory tree (fakes::fs().nodes) behind fs::FS / fs::File.
#include <string.h>

#include <algorithm>
#include <string>
#include <vector>

#include "FS.h"
#include "LittleFS.h"
#include "fakes/fakes.h"
#include "hal_internal.h"

namespace fakes {

struct FileHandle {
  std::string path;
  std::string name;  // base name
  bool dir = false;
  bool open = true;
  bool canRead = false;
  bool canWrite = false;
  bool append = false;
  size_t pos = 0;
  std::vector<std::string> children;  // directories: full paths of the direct children
  size_t next = 0;
  ~FileHandle() {
    if (open) --fs().openHandles;
  }
};

namespace {

Fs g_fs;

std::string parentOf(const std::string& path) {
  const size_t slash = path.rfind('/');
  if (slash == std::string::npos || slash == 0) return "/";
  return path.substr(0, slash);
}

std::string baseName(const std::string& path) {
  const size_t slash = path.rfind('/');
  return slash == std::string::npos ? path : path.substr(slash + 1);
}

bool isDir(const std::string& path) {
  if (path == "/") return true;
  auto it = g_fs.nodes.find(path);
  return it != g_fs.nodes.end() && it->second.dir;
}

bool nodeExists(const std::string& path) { return path == "/" || g_fs.nodes.count(path) != 0; }

size_t blocksOf(size_t bytes, size_t block) { return bytes == 0 ? 1 : (bytes + block - 1) / block; }

std::vector<std::string> childrenOf(const std::string& dir) {
  std::vector<std::string> out;
  const std::string prefix = dir == "/" ? "/" : dir + "/";
  for (const auto& n : g_fs.nodes) {
    const std::string& p = n.first;
    if (p.size() > prefix.size() && p.compare(0, prefix.size(), prefix) == 0 &&
        p.find('/', prefix.size()) == std::string::npos) {
      out.push_back(p);
    }
  }
  return out;
}

fs::File openHandle(const std::string& path, const char* mode) {
  const std::string m = mode != nullptr ? mode : "r";
  ++g_fs.opens;
  note("fs.open " + path + " " + m);
  if (!g_fs.mounted || path.empty() || path[0] != '/') return fs::File();
  if (g_fs.shouldFail("open", path)) return fs::File();
  auto h = std::make_shared<FileHandle>();
  h->path = path;
  h->name = path == "/" ? "/" : baseName(path);
  const bool plus = m.find('+') != std::string::npos;
  if (m[0] == 'r') {
    if (!nodeExists(path)) return fs::File();
    if (isDir(path)) {
      h->dir = true;
      h->children = childrenOf(path);
    }
    h->canRead = true;
    h->canWrite = plus && !h->dir;
  } else if (m[0] == 'w' || m[0] == 'a') {
    if (isDir(path) || !isDir(parentOf(path))) return fs::File();
    FsNode& node = g_fs.nodes[path];
    if (m[0] == 'w') node.data.clear();
    h->canWrite = true;
    h->canRead = plus;
    h->append = m[0] == 'a';
    h->pos = h->append ? node.data.size() : 0;
    ++g_fs.writeOpens;
  } else {
    return fs::File();
  }
  ++g_fs.openHandles;
  return fs::File(h);
}

}  // namespace

Fs& fs() { return g_fs; }

void resetFsVolatile() {
  Fs next;
  next.formatted = g_fs.formatted;
  next.nodes.swap(g_fs.nodes);
  g_fs = std::move(next);
}

void Fs::fail(const std::string& op, const std::string& path, int count) {
  failures.push_back({op, path, count});
}

bool Fs::shouldFail(const std::string& op, const std::string& path) {
  for (FsFailure& f : failures) {
    if (f.count > 0 && f.op == op && (f.path.empty() || f.path == path)) {
      --f.count;
      return true;
    }
  }
  return false;
}

void Fs::mkdirs(const std::string& path) {
  if (path.empty() || path == "/") return;
  mkdirs(parentOf(path));
  FsNode& n = nodes[path];
  n.dir = true;
  n.data.clear();
}

void Fs::put(const std::string& path, const std::string& content) {
  mkdirs(parentOf(path));
  FsNode& n = nodes[path];
  n.dir = false;
  n.data.assign(content.begin(), content.end());
}

bool Fs::exists(const std::string& path) const { return path == "/" || nodes.count(path) != 0; }

std::string Fs::read(const std::string& path) const {
  auto it = nodes.find(path);
  if (it == nodes.end() || it->second.dir) return "";
  return std::string(it->second.data.begin(), it->second.data.end());
}

size_t Fs::usedBytes() const {
  size_t used = baseUsedBytes;
  for (const auto& n : nodes) {
    used += n.second.dir ? blockSize : blocksOf(n.second.data.size(), blockSize) * blockSize;
  }
  return used;
}

}  // namespace fakes

// ---------------------------------------------------------------- fs::File

namespace fs {

size_t File::write(uint8_t c) { return write(&c, 1); }

size_t File::write(const uint8_t* buf, size_t size) {
  fakes::Fs& f = fakes::fs();
  if (!h_ || !h_->open || !h_->canWrite || size == 0) return 0;
  if (f.onWrite) f.onWrite(h_->path);
  if (f.shouldFail("write", h_->path)) return 0;
  std::vector<uint8_t>& data = f.nodes[h_->path].data;
  if (h_->append) h_->pos = data.size();
  // capacity: the blocks of this file may grow as far as the free space allows
  const size_t oldBlocks = fakes::blocksOf(data.size(), f.blockSize) * f.blockSize;
  const size_t usedOthers = f.usedBytes() - oldBlocks;
  const size_t room = f.totalBytes > usedOthers ? f.totalBytes - usedOthers : 0;
  const size_t maxSize = room / f.blockSize * f.blockSize;
  size_t n = size;
  if (h_->pos + n > maxSize) n = maxSize > h_->pos ? maxSize - h_->pos : 0;
  if (n == 0) return 0;
  if (h_->pos + n > data.size()) data.resize(h_->pos + n);
  memcpy(data.data() + h_->pos, buf, n);
  h_->pos += n;
  ++f.writes;
  f.bytesWritten += n;
  return n;
}

int File::available() {
  if (!h_ || !h_->open || !h_->canRead || h_->dir) return 0;
  auto it = fakes::fs().nodes.find(h_->path);
  if (it == fakes::fs().nodes.end()) return 0;
  const size_t size = it->second.data.size();
  return h_->pos < size ? static_cast<int>(size - h_->pos) : 0;
}

int File::read() {
  uint8_t c = 0;
  return read(&c, 1) == 1 ? c : -1;
}

int File::peek() {
  if (available() <= 0) return -1;
  return fakes::fs().nodes[h_->path].data[h_->pos];
}

void File::flush() {}

size_t File::read(uint8_t* buf, size_t size) {
  fakes::Fs& f = fakes::fs();
  if (!h_ || !h_->open || !h_->canRead || h_->dir) return 0;
  if (f.shouldFail("read", h_->path)) return 0;
  auto it = f.nodes.find(h_->path);
  if (it == f.nodes.end()) return 0;
  const std::vector<uint8_t>& data = it->second.data;
  if (h_->pos >= data.size()) return 0;
  const size_t n = std::min(size, data.size() - h_->pos);
  memcpy(buf, data.data() + h_->pos, n);
  h_->pos += n;
  return n;
}

bool File::seek(uint32_t pos, SeekMode mode) {
  if (!h_ || !h_->open || h_->dir) return false;
  if (fakes::fs().shouldFail("seek", h_->path)) return false;
  const size_t sz = size();
  size_t base = 0;
  if (mode == SeekCur) base = h_->pos;
  if (mode == SeekEnd) base = sz;
  h_->pos = base + pos;
  return true;
}

size_t File::position() const { return h_ ? h_->pos : 0; }

size_t File::size() const {
  if (!h_ || h_->dir) return 0;
  auto it = fakes::fs().nodes.find(h_->path);
  return it == fakes::fs().nodes.end() ? 0 : it->second.data.size();
}

bool File::setBufferSize(size_t) { return true; }

void File::close() {
  if (h_ && h_->open) {
    h_->open = false;
    --fakes::fs().openHandles;
  }
}

File::operator bool() const { return h_ && h_->open; }

time_t File::getLastWrite() { return 0; }

const char* File::path() const { return h_ ? h_->path.c_str() : nullptr; }

const char* File::name() const { return h_ ? h_->name.c_str() : nullptr; }

bool File::isDirectory(void) { return h_ && h_->open && h_->dir; }

File File::openNextFile(const char* mode) {
  if (!h_ || !h_->open || !h_->dir) return File();
  while (h_->next < h_->children.size()) {
    const std::string child = h_->children[h_->next++];
    if (fakes::fs().nodes.count(child) != 0) return fakes::openHandle(child, mode);
  }
  return File();
}

void File::rewindDirectory(void) {
  if (h_) h_->next = 0;
}

// ---------------------------------------------------------------- fs::FS

File FS::open(const char* path, const char* mode, const bool) {
  return fakes::openHandle(path != nullptr ? path : "", mode);
}

bool FS::exists(const char* path) {
  return fakes::fs().mounted && path != nullptr && fakes::nodeExists(path);
}

bool FS::remove(const char* path) {
  fakes::Fs& f = fakes::fs();
  const std::string p = path != nullptr ? path : "";
  ++f.removes;
  fakes::note("fs.remove " + p);
  if (!f.mounted || f.shouldFail("remove", p)) return false;
  auto it = f.nodes.find(p);
  if (it == f.nodes.end() || it->second.dir) return false;
  f.nodes.erase(it);
  return true;
}

bool FS::rename(const char* pathFrom, const char* pathTo) {
  fakes::Fs& f = fakes::fs();
  const std::string from = pathFrom != nullptr ? pathFrom : "";
  const std::string to = pathTo != nullptr ? pathTo : "";
  ++f.renames;
  fakes::note("fs.rename " + from + " " + to);
  if (!f.mounted || f.shouldFail("rename", from)) return false;
  if (f.nodes.count(from) == 0 || !fakes::isDir(fakes::parentOf(to)) || from == to) {
    return from == to && f.nodes.count(from) != 0;
  }
  if (fakes::isDir(to) && !fakes::childrenOf(to).empty()) return false;
  // the target is replaced (lfs_rename); a directory moves with its children
  f.nodes.erase(to);
  std::vector<std::pair<std::string, fakes::FsNode>> moved;
  for (auto it = f.nodes.begin(); it != f.nodes.end();) {
    const std::string& p = it->first;
    if (p == from || (p.size() > from.size() && p.compare(0, from.size() + 1, from + "/") == 0)) {
      moved.emplace_back(to + p.substr(from.size()), it->second);
      it = f.nodes.erase(it);
    } else {
      ++it;
    }
  }
  for (auto& m : moved) f.nodes[m.first] = m.second;
  return true;
}

bool FS::mkdir(const char* path) {
  fakes::Fs& f = fakes::fs();
  const std::string p = path != nullptr ? path : "";
  fakes::note("fs.mkdir " + p);
  if (!f.mounted || p.empty() || p[0] != '/' || f.shouldFail("mkdir", p)) return false;
  if (fakes::nodeExists(p) || !fakes::isDir(fakes::parentOf(p))) return false;
  f.nodes[p].dir = true;
  return true;
}

bool FS::rmdir(const char* path) {
  fakes::Fs& f = fakes::fs();
  const std::string p = path != nullptr ? path : "";
  fakes::note("fs.rmdir " + p);
  if (!f.mounted || f.shouldFail("rmdir", p)) return false;
  auto it = f.nodes.find(p);
  if (it == f.nodes.end() || !it->second.dir || !fakes::childrenOf(p).empty()) return false;
  f.nodes.erase(it);
  return true;
}

// ---------------------------------------------------------------- LittleFS

bool LittleFSFS::begin(bool formatOnFail, const char*, uint8_t, const char*) {
  fakes::Fs& f = fakes::fs();
  ++f.begins;
  fakes::note(std::string("fs.begin ") + (formatOnFail ? "1" : "0"));
  if (f.mounted) return true;
  if (!f.formatted || !f.mountOk) {
    if (!formatOnFail || !format()) return false;
  }
  f.mounted = true;
  return true;
}

void LittleFSFS::end() { fakes::fs().mounted = false; }

bool LittleFSFS::format() {
  fakes::Fs& f = fakes::fs();
  ++f.formats;
  fakes::note("fs.format");
  if (!f.formatOk) return false;
  f.nodes.clear();
  f.formatted = true;
  f.mountOk = true;
  return true;
}

size_t LittleFSFS::totalBytes() { return fakes::fs().mounted ? fakes::fs().totalBytes : 0; }

size_t LittleFSFS::usedBytes() { return fakes::fs().mounted ? fakes::fs().usedBytes() : 0; }

}  // namespace fs

fs::LittleFSFS LittleFS;
