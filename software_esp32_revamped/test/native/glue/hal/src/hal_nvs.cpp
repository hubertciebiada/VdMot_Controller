// Fake NVS (typed store fakes::nvs().ns) and Preferences on top of it, with the behaviour of
// ESP-IDF 4.4 and Arduino-ESP32 2.0.7 (return values, typed lookup, key length limit).
#include <string.h>

#include <map>
#include <string>
#include <vector>

#include "Preferences.h"
#include "fakes/fakes.h"
#include "hal_internal.h"
#include "nvs.h"
#include "nvs_flash.h"

namespace fakes {

namespace {

Nvs g_nvs;

struct Handle {
  std::string ns;
  bool readOnly;
};
std::map<nvs_handle_t, Handle> g_handles;
nvs_handle_t g_nextHandle = 1;

constexpr size_t kMaxKeyLen = 15;

std::vector<uint8_t> bytesOf(uint64_t v, size_t n) {
  std::vector<uint8_t> b(n);
  for (size_t i = 0; i < n; ++i) b[i] = static_cast<uint8_t>(v >> (8 * i));
  return b;
}

uint64_t valueOf(const std::vector<uint8_t>& b) {
  uint64_t v = 0;
  for (size_t i = b.size(); i > 0; --i) v = (v << 8) | b[i - 1];
  return v;
}

size_t widthOf(NvsType t) {
  switch (t) {
    case NvsType::U8:
    case NvsType::I8: return 1;
    case NvsType::U16:
    case NvsType::I16: return 2;
    case NvsType::U32:
    case NvsType::I32: return 4;
    case NvsType::U64:
    case NvsType::I64: return 8;
    default: return 0;
  }
}

bool isSigned(NvsType t) {
  return t == NvsType::I8 || t == NvsType::I16 || t == NvsType::I32 || t == NvsType::I64;
}

esp_err_t handleOf(nvs_handle_t h, Handle*& out) {
  auto it = g_handles.find(h);
  if (it == g_handles.end()) return ESP_ERR_NVS_INVALID_HANDLE;
  out = &it->second;
  return ESP_OK;
}

esp_err_t checkKey(const char* key) {
  if (key == nullptr || key[0] == '\0') return ESP_ERR_NVS_NOT_FOUND;
  return strlen(key) > kMaxKeyLen ? ESP_ERR_NVS_KEY_TOO_LONG : ESP_OK;
}

esp_err_t setEntry(nvs_handle_t h, const char* key, NvsType type, std::vector<uint8_t> bytes) {
  Handle* hd = nullptr;
  esp_err_t err = handleOf(h, hd);
  if (err != ESP_OK) return err;
  if (hd->readOnly) return ESP_ERR_NVS_READ_ONLY;
  err = checkKey(key);
  if (err != ESP_OK) return err;
  if (g_nvs.failSet.count(key) != 0) return ESP_ERR_NVS_NOT_ENOUGH_SPACE;
  g_nvs.ns[hd->ns][key] = NvsEntry{type, std::move(bytes)};
  ++g_nvs.sets[hd->ns + "/" + key];
  return ESP_OK;
}

// The entry of `key` when it has exactly `type` (ESP-IDF looks items up by type).
esp_err_t getEntry(nvs_handle_t h, const char* key, NvsType type, const NvsEntry*& out) {
  Handle* hd = nullptr;
  esp_err_t err = handleOf(h, hd);
  if (err != ESP_OK) return err;
  err = checkKey(key);
  if (err != ESP_OK) return err;
  const NvsEntry* e = g_nvs.get(hd->ns, key);
  if (e == nullptr || e->type != type) return ESP_ERR_NVS_NOT_FOUND;
  out = e;
  return ESP_OK;
}

template <typename T>
esp_err_t getInt(nvs_handle_t h, const char* key, NvsType type, T* out) {
  const NvsEntry* e = nullptr;
  const esp_err_t err = getEntry(h, key, type, e);
  if (err != ESP_OK) return err;
  if (out != nullptr) *out = static_cast<T>(valueOf(e->bytes));
  return ESP_OK;
}

esp_err_t getData(nvs_handle_t h, const char* key, NvsType type, void* out, size_t* length) {
  if (length == nullptr) return ESP_ERR_INVALID_ARG;
  const NvsEntry* e = nullptr;
  const esp_err_t err = getEntry(h, key, type, e);
  if (err != ESP_OK) return err;
  const size_t n = e->bytes.size();
  if (out == nullptr) {
    *length = n;
    return ESP_OK;
  }
  if (*length < n) {
    *length = n;
    return ESP_ERR_NVS_INVALID_LENGTH;
  }
  if (n > 0) memcpy(out, e->bytes.data(), n);
  *length = n;
  return ESP_OK;
}

}  // namespace

Nvs& nvs() { return g_nvs; }

void resetNvsVolatile() {
  Nvs next;
  next.ns.swap(g_nvs.ns);
  g_nvs = std::move(next);
  g_handles.clear();
  g_nextHandle = 1;
}

void Nvs::setU8(const std::string& n, const std::string& key, uint8_t v) {
  ns[n][key] = NvsEntry{NvsType::U8, bytesOf(v, 1)};
}
void Nvs::setI8(const std::string& n, const std::string& key, int8_t v) {
  ns[n][key] = NvsEntry{NvsType::I8, bytesOf(static_cast<uint8_t>(v), 1)};
}
void Nvs::setU16(const std::string& n, const std::string& key, uint16_t v) {
  ns[n][key] = NvsEntry{NvsType::U16, bytesOf(v, 2)};
}
void Nvs::setI16(const std::string& n, const std::string& key, int16_t v) {
  ns[n][key] = NvsEntry{NvsType::I16, bytesOf(static_cast<uint16_t>(v), 2)};
}
void Nvs::setU32(const std::string& n, const std::string& key, uint32_t v) {
  ns[n][key] = NvsEntry{NvsType::U32, bytesOf(v, 4)};
}
void Nvs::setI32(const std::string& n, const std::string& key, int32_t v) {
  ns[n][key] = NvsEntry{NvsType::I32, bytesOf(static_cast<uint32_t>(v), 4)};
}
void Nvs::setI64(const std::string& n, const std::string& key, int64_t v) {
  ns[n][key] = NvsEntry{NvsType::I64, bytesOf(static_cast<uint64_t>(v), 8)};
}
void Nvs::setStr(const std::string& n, const std::string& key, const std::string& v) {
  std::vector<uint8_t> b(v.begin(), v.end());
  b.push_back(0);
  ns[n][key] = NvsEntry{NvsType::Str, b};
}
void Nvs::setBlob(const std::string& n, const std::string& key, const std::vector<uint8_t>& v) {
  ns[n][key] = NvsEntry{NvsType::Blob, v};
}

bool Nvs::has(const std::string& n, const std::string& key) const { return get(n, key) != nullptr; }

const NvsEntry* Nvs::get(const std::string& n, const std::string& key) const {
  auto it = ns.find(n);
  if (it == ns.end()) return nullptr;
  auto k = it->second.find(key);
  return k == it->second.end() ? nullptr : &k->second;
}

uint64_t Nvs::getU(const std::string& n, const std::string& key) const {
  const NvsEntry* e = get(n, key);
  return e == nullptr || widthOf(e->type) == 0 ? 0 : valueOf(e->bytes);
}

int64_t Nvs::getI(const std::string& n, const std::string& key) const {
  const NvsEntry* e = get(n, key);
  if (e == nullptr || widthOf(e->type) == 0) return 0;
  const uint64_t v = valueOf(e->bytes);
  const size_t bits = 8 * widthOf(e->type);
  if (!isSigned(e->type) || bits == 64) return static_cast<int64_t>(v);
  const uint64_t sign = 1ull << (bits - 1);
  return static_cast<int64_t>((v ^ sign) - sign);
}

std::vector<uint8_t> Nvs::getBlob(const std::string& n, const std::string& key) const {
  const NvsEntry* e = get(n, key);
  return e == nullptr ? std::vector<uint8_t>{} : e->bytes;
}

}  // namespace fakes

// ---------------------------------------------------------------- ESP-IDF NVS API

using fakes::NvsType;

esp_err_t nvs_flash_init(void) { return fakes::nvs().initOk ? ESP_OK : ESP_ERR_NVS_NO_FREE_PAGES; }

esp_err_t nvs_flash_erase(void) {
  fakes::nvs().ns.clear();
  return ESP_OK;
}

esp_err_t nvs_open(const char* name, nvs_open_mode_t open_mode, nvs_handle_t* out_handle) {
  fakes::Nvs& n = fakes::nvs();
  ++n.opens;
  if (!n.initOk) return ESP_ERR_NVS_NOT_INITIALIZED;
  if (name == nullptr || out_handle == nullptr) return ESP_ERR_INVALID_ARG;
  if (strlen(name) > fakes::kMaxKeyLen) return ESP_ERR_NVS_INVALID_NAME;
  if (n.failOpen.count(name) != 0) return ESP_FAIL;
  if (n.ns.count(name) == 0) {
    if (open_mode == NVS_READONLY) return ESP_ERR_NVS_NOT_FOUND;
    n.ns[name];  // a read-write open creates the namespace
  }
  const nvs_handle_t h = fakes::g_nextHandle++;
  fakes::g_handles[h] = fakes::Handle{name, open_mode == NVS_READONLY};
  ++n.openHandles;
  *out_handle = h;
  return ESP_OK;
}

void nvs_close(nvs_handle_t handle) {
  if (fakes::g_handles.erase(handle) != 0) --fakes::nvs().openHandles;
}

esp_err_t nvs_commit(nvs_handle_t handle) {
  fakes::Handle* h = nullptr;
  const esp_err_t err = fakes::handleOf(handle, h);
  if (err != ESP_OK) return err;
  return fakes::nvs().failCommit ? ESP_FAIL : ESP_OK;
}

esp_err_t nvs_erase_key(nvs_handle_t handle, const char* key) {
  fakes::Handle* h = nullptr;
  esp_err_t err = fakes::handleOf(handle, h);
  if (err != ESP_OK) return err;
  if (h->readOnly) return ESP_ERR_NVS_READ_ONLY;
  err = fakes::checkKey(key);
  if (err != ESP_OK) return err;
  if (fakes::nvs().failErase) return ESP_FAIL;
  return fakes::nvs().ns[h->ns].erase(key) != 0 ? ESP_OK : ESP_ERR_NVS_NOT_FOUND;
}

esp_err_t nvs_erase_all(nvs_handle_t handle) {
  fakes::Handle* h = nullptr;
  const esp_err_t err = fakes::handleOf(handle, h);
  if (err != ESP_OK) return err;
  if (h->readOnly) return ESP_ERR_NVS_READ_ONLY;
  if (fakes::nvs().failErase) return ESP_FAIL;
  fakes::nvs().ns[h->ns].clear();
  return ESP_OK;
}

esp_err_t nvs_set_i8(nvs_handle_t h, const char* key, int8_t v) {
  return fakes::setEntry(h, key, NvsType::I8, fakes::bytesOf(static_cast<uint8_t>(v), 1));
}
esp_err_t nvs_set_u8(nvs_handle_t h, const char* key, uint8_t v) {
  return fakes::setEntry(h, key, NvsType::U8, fakes::bytesOf(v, 1));
}
esp_err_t nvs_set_i16(nvs_handle_t h, const char* key, int16_t v) {
  return fakes::setEntry(h, key, NvsType::I16, fakes::bytesOf(static_cast<uint16_t>(v), 2));
}
esp_err_t nvs_set_u16(nvs_handle_t h, const char* key, uint16_t v) {
  return fakes::setEntry(h, key, NvsType::U16, fakes::bytesOf(v, 2));
}
esp_err_t nvs_set_i32(nvs_handle_t h, const char* key, int32_t v) {
  return fakes::setEntry(h, key, NvsType::I32, fakes::bytesOf(static_cast<uint32_t>(v), 4));
}
esp_err_t nvs_set_u32(nvs_handle_t h, const char* key, uint32_t v) {
  return fakes::setEntry(h, key, NvsType::U32, fakes::bytesOf(v, 4));
}
esp_err_t nvs_set_i64(nvs_handle_t h, const char* key, int64_t v) {
  return fakes::setEntry(h, key, NvsType::I64, fakes::bytesOf(static_cast<uint64_t>(v), 8));
}
esp_err_t nvs_set_u64(nvs_handle_t h, const char* key, uint64_t v) {
  return fakes::setEntry(h, key, NvsType::U64, fakes::bytesOf(v, 8));
}
esp_err_t nvs_set_str(nvs_handle_t h, const char* key, const char* value) {
  if (value == nullptr) return ESP_ERR_INVALID_ARG;
  std::vector<uint8_t> b(value, value + strlen(value) + 1);
  return fakes::setEntry(h, key, NvsType::Str, b);
}
esp_err_t nvs_set_blob(nvs_handle_t h, const char* key, const void* value, size_t length) {
  if (value == nullptr && length > 0) return ESP_ERR_INVALID_ARG;
  const uint8_t* p = static_cast<const uint8_t*>(value);
  return fakes::setEntry(h, key, NvsType::Blob, std::vector<uint8_t>(p, p + length));
}

esp_err_t nvs_get_i8(nvs_handle_t h, const char* key, int8_t* out) {
  return fakes::getInt(h, key, NvsType::I8, out);
}
esp_err_t nvs_get_u8(nvs_handle_t h, const char* key, uint8_t* out) {
  return fakes::getInt(h, key, NvsType::U8, out);
}
esp_err_t nvs_get_i16(nvs_handle_t h, const char* key, int16_t* out) {
  return fakes::getInt(h, key, NvsType::I16, out);
}
esp_err_t nvs_get_u16(nvs_handle_t h, const char* key, uint16_t* out) {
  return fakes::getInt(h, key, NvsType::U16, out);
}
esp_err_t nvs_get_i32(nvs_handle_t h, const char* key, int32_t* out) {
  return fakes::getInt(h, key, NvsType::I32, out);
}
esp_err_t nvs_get_u32(nvs_handle_t h, const char* key, uint32_t* out) {
  return fakes::getInt(h, key, NvsType::U32, out);
}
esp_err_t nvs_get_i64(nvs_handle_t h, const char* key, int64_t* out) {
  return fakes::getInt(h, key, NvsType::I64, out);
}
esp_err_t nvs_get_u64(nvs_handle_t h, const char* key, uint64_t* out) {
  return fakes::getInt(h, key, NvsType::U64, out);
}
esp_err_t nvs_get_str(nvs_handle_t h, const char* key, char* out_value, size_t* length) {
  return fakes::getData(h, key, NvsType::Str, out_value, length);
}
esp_err_t nvs_get_blob(nvs_handle_t h, const char* key, void* out_value, size_t* length) {
  return fakes::getData(h, key, NvsType::Blob, out_value, length);
}

// ---------------------------------------------------------------- Preferences

Preferences::~Preferences() { end(); }

bool Preferences::begin(const char* name, bool readOnly, const char*) {
  if (started_) return false;
  readOnly_ = readOnly;
  if (nvs_open(name, readOnly ? NVS_READONLY : NVS_READWRITE, &handle_) != ESP_OK) return false;
  started_ = true;
  return true;
}

void Preferences::end() {
  if (!started_) return;
  nvs_close(handle_);
  started_ = false;
}

bool Preferences::clear() {
  if (!started_ || readOnly_) return false;
  return nvs_erase_all(handle_) == ESP_OK && nvs_commit(handle_) == ESP_OK;
}

bool Preferences::remove(const char* key) {
  if (!started_ || key == nullptr || readOnly_) return false;
  return nvs_erase_key(handle_, key) == ESP_OK && nvs_commit(handle_) == ESP_OK;
}

namespace {

size_t committed(nvs_handle_t h, esp_err_t err, size_t n) {
  if (err != ESP_OK || nvs_commit(h) != ESP_OK) return 0;
  return n;
}

}  // namespace

#define VDM_PREF_PUT(Name, Type, Setter, Size)                          \
  size_t Preferences::Name(const char* key, Type value) {               \
    if (!started_ || key == nullptr || readOnly_) return 0;             \
    return committed(handle_, Setter(handle_, key, value), Size);       \
  }

VDM_PREF_PUT(putChar, int8_t, nvs_set_i8, 1)
VDM_PREF_PUT(putUChar, uint8_t, nvs_set_u8, 1)
VDM_PREF_PUT(putShort, int16_t, nvs_set_i16, 2)
VDM_PREF_PUT(putUShort, uint16_t, nvs_set_u16, 2)
VDM_PREF_PUT(putInt, int32_t, nvs_set_i32, 4)
VDM_PREF_PUT(putUInt, uint32_t, nvs_set_u32, 4)
VDM_PREF_PUT(putLong, int32_t, nvs_set_i32, 4)
VDM_PREF_PUT(putULong, uint32_t, nvs_set_u32, 4)
VDM_PREF_PUT(putLong64, int64_t, nvs_set_i64, 8)
VDM_PREF_PUT(putULong64, uint64_t, nvs_set_u64, 8)
#undef VDM_PREF_PUT

size_t Preferences::putBool(const char* key, bool value) {
  return putUChar(key, static_cast<uint8_t>(value ? 1 : 0));
}

size_t Preferences::putString(const char* key, const char* value) {
  if (!started_ || key == nullptr || value == nullptr || readOnly_) return 0;
  return committed(handle_, nvs_set_str(handle_, key, value), strlen(value));
}

size_t Preferences::putBytes(const char* key, const void* value, size_t len) {
  if (!started_ || key == nullptr || value == nullptr || len == 0 || readOnly_) return 0;
  return committed(handle_, nvs_set_blob(handle_, key, value, len), len);
}

bool Preferences::isKey(const char* key) { return getType(key) != PT_INVALID; }

PreferenceType Preferences::getType(const char* key) {
  if (!started_ || key == nullptr) return PT_INVALID;
  auto it = fakes::g_handles.find(handle_);
  if (it == fakes::g_handles.end()) return PT_INVALID;
  const fakes::NvsEntry* e = fakes::nvs().get(it->second.ns, key);
  if (e == nullptr) return PT_INVALID;
  switch (e->type) {
    case NvsType::I8: return PT_I8;
    case NvsType::U8: return PT_U8;
    case NvsType::I16: return PT_I16;
    case NvsType::U16: return PT_U16;
    case NvsType::I32: return PT_I32;
    case NvsType::U32: return PT_U32;
    case NvsType::I64: return PT_I64;
    case NvsType::U64: return PT_U64;
    case NvsType::Str: return PT_STR;
    case NvsType::Blob: return PT_BLOB;
  }
  return PT_INVALID;
}

#define VDM_PREF_GET(Name, Type, Getter)                        \
  Type Preferences::Name(const char* key, Type defaultValue) {  \
    Type value = defaultValue;                                  \
    if (!started_ || key == nullptr) return value;              \
    if (Getter(handle_, key, &value) != ESP_OK) return defaultValue; \
    return value;                                               \
  }

VDM_PREF_GET(getChar, int8_t, nvs_get_i8)
VDM_PREF_GET(getUChar, uint8_t, nvs_get_u8)
VDM_PREF_GET(getShort, int16_t, nvs_get_i16)
VDM_PREF_GET(getUShort, uint16_t, nvs_get_u16)
VDM_PREF_GET(getInt, int32_t, nvs_get_i32)
VDM_PREF_GET(getUInt, uint32_t, nvs_get_u32)
VDM_PREF_GET(getLong, int32_t, nvs_get_i32)
VDM_PREF_GET(getULong, uint32_t, nvs_get_u32)
VDM_PREF_GET(getLong64, int64_t, nvs_get_i64)
VDM_PREF_GET(getULong64, uint64_t, nvs_get_u64)
#undef VDM_PREF_GET

bool Preferences::getBool(const char* key, bool defaultValue) {
  return getUChar(key, defaultValue ? 1 : 0) == 1;
}

size_t Preferences::getString(const char* key, char* value, size_t maxLen) {
  size_t len = 0;
  if (!started_ || key == nullptr || value == nullptr || maxLen == 0) return 0;
  if (nvs_get_str(handle_, key, nullptr, &len) != ESP_OK || len > maxLen) return 0;
  if (nvs_get_str(handle_, key, value, &len) != ESP_OK) return 0;
  return len;
}

String Preferences::getString(const char* key, String defaultValue) {
  size_t len = 0;
  if (!started_ || key == nullptr) return defaultValue;
  if (nvs_get_str(handle_, key, nullptr, &len) != ESP_OK || len == 0) return defaultValue;
  std::vector<char> buf(len);
  if (nvs_get_str(handle_, key, buf.data(), &len) != ESP_OK) return defaultValue;
  return String(buf.data());
}

size_t Preferences::getBytesLength(const char* key) {
  size_t len = 0;
  if (!started_ || key == nullptr) return 0;
  if (nvs_get_blob(handle_, key, nullptr, &len) != ESP_OK) return 0;
  return len;
}

size_t Preferences::getBytes(const char* key, void* buf, size_t maxLen) {
  size_t len = getBytesLength(key);
  if (len == 0 || buf == nullptr || maxLen == 0) return len;
  if (len > maxLen) return 0;
  if (nvs_get_blob(handle_, key, buf, &len) != ESP_OK) return 0;
  return len;
}
