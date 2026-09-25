// Fake Arduino-ESP32 2.0.7 FS.h: fs::File and fs::FS on the in-memory tree of fakes::fs().
//   - "w" truncates, "a" appends, "r" of a missing file fails, the parent directory must exist;
//   - rename() replaces the target (lfs_rename), remove() of a missing file fails;
//   - writes stop at the capacity (totalBytes, 4 KiB blocks per file, like LittleFS);
//   - File::name() is the base name (2.x), openNextFile() lists the direct children;
//   - fakes::fs().fail(op, path) makes the next operation of that kind on that path fail.
#pragma once

#include <stddef.h>
#include <stdint.h>
#include <time.h>

#include <memory>

#include "Print.h"
#include "WString.h"

#define FILE_READ "r"
#define FILE_WRITE "w"
#define FILE_APPEND "a"

namespace fakes {
struct FileHandle;
}  // namespace fakes

namespace fs {

enum SeekMode { SeekSet = 0, SeekCur = 1, SeekEnd = 2 };

class File : public Stream {
 public:
  File() = default;
  explicit File(std::shared_ptr<fakes::FileHandle> h) : h_(std::move(h)) {}

  size_t write(uint8_t c) override;
  size_t write(const uint8_t* buf, size_t size) override;
  using Print::write;
  int available() override;
  int read() override;
  int peek() override;
  void flush() override;
  size_t read(uint8_t* buf, size_t size);
  size_t readBytes(char* buffer, size_t length) {
    return read(reinterpret_cast<uint8_t*>(buffer), length);
  }
  bool seek(uint32_t pos, SeekMode mode);
  bool seek(uint32_t pos) { return seek(pos, SeekSet); }
  size_t position() const;
  size_t size() const;
  bool setBufferSize(size_t size);
  void close();
  operator bool() const;
  time_t getLastWrite();
  const char* path() const;
  const char* name() const;
  bool isDirectory(void);
  File openNextFile(const char* mode = FILE_READ);
  void rewindDirectory(void);

 private:
  std::shared_ptr<fakes::FileHandle> h_;
};

class FS {
 public:
  File open(const char* path, const char* mode = FILE_READ, const bool create = false);
  File open(const String& path, const char* mode = FILE_READ, const bool create = false) {
    return open(path.c_str(), mode, create);
  }
  bool exists(const char* path);
  bool exists(const String& path) { return exists(path.c_str()); }
  bool remove(const char* path);
  bool remove(const String& path) { return remove(path.c_str()); }
  bool rename(const char* pathFrom, const char* pathTo);
  bool rename(const String& pathFrom, const String& pathTo) {
    return rename(pathFrom.c_str(), pathTo.c_str());
  }
  bool mkdir(const char* path);
  bool mkdir(const String& path) { return mkdir(path.c_str()); }
  bool rmdir(const char* path);
  bool rmdir(const String& path) { return rmdir(path.c_str()); }
};

}  // namespace fs

using fs::File;
using fs::FS;
using fs::SeekCur;
using fs::SeekEnd;
using fs::SeekMode;
using fs::SeekSet;
