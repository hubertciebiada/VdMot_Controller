// Fake Update library and esp_ota_ops on fakes::ota(): two app partitions with image states and a
// boot selection that the runner hooks hand to the next boot (Ota::bootloader() runs at a reset).
#include <string.h>

#include <string>

#include "Update.h"
#include "esp_ota_ops.h"
#include "fakes/fakes.h"
#include "hal_internal.h"

namespace fakes {

namespace {

Ota g_ota;

int indexOf(const esp_partition_t* p) {
  if (p == &g_ota.parts[0]) return 0;
  if (p == &g_ota.parts[1]) return 1;
  return -1;
}

void initPart(esp_partition_t& p, esp_partition_subtype_t subtype, uint32_t address,
              const char* label) {
  p = esp_partition_t{};
  p.type = ESP_PARTITION_TYPE_APP;
  p.subtype = subtype;
  p.address = address;
  p.size = 0x140000;
  strncpy(p.label, label, sizeof p.label - 1);
}

}  // namespace

Ota::Ota() {
  initPart(parts[0], ESP_PARTITION_SUBTYPE_APP_OTA_0, 0x10000, "app0");
  initPart(parts[1], ESP_PARTITION_SUBTYPE_APP_OTA_1, 0x150000, "app1");
}

void Ota::bootloader() {
  int sel = boot;
  if (state[sel] == ESP_OTA_IMG_PENDING_VERIFY) {
    state[sel] = ESP_OTA_IMG_ABORTED;  // not confirmed during its first boot: roll back
    sel = 1 - sel;
    boot = sel;
  } else if (state[sel] == ESP_OTA_IMG_NEW) {
    state[sel] = ESP_OTA_IMG_PENDING_VERIFY;
  }
  running = sel;
}

Ota& ota() { return g_ota; }

void resetOtaVolatile() {
  Ota next;
  next.state[0] = g_ota.state[0];
  next.state[1] = g_ota.state[1];
  next.running = g_ota.running;
  next.boot = g_ota.boot;
  g_ota = next;
}

}  // namespace fakes

// ---------------------------------------------------------------- Update

UpdateClass Update;

namespace {

size_t partitionSize() { return fakes::ota().parts[1 - fakes::ota().running].size; }

}  // namespace

bool UpdateClass::begin(size_t size, int command, int, uint8_t, const char*) {
  fakes::UpdateState& u = fakes::ota().update;
  ++u.begins;
  fakes::note("Update.begin " + std::to_string(size) + " " + std::to_string(command));
  if (u.running) return false;
  u.beginSize = size;
  u.beginCommand = command;
  if (!u.beginResult) {
    u.error = u.beginError;
    return false;
  }
  u.running = true;
  u.error = UPDATE_ERROR_OK;
  u.written.clear();
  u.md5.clear();
  return true;
}

size_t UpdateClass::write(uint8_t* data, size_t len) {
  fakes::UpdateState& u = fakes::ota().update;
  ++u.writes;
  if (u.error != UPDATE_ERROR_OK || !u.running) return 0;
  if (u.written.size() + len > partitionSize()) {
    u.error = UPDATE_ERROR_SPACE;
    u.running = false;
    return 0;
  }
  size_t n = len;
  if (u.written.size() + len > u.shortWriteAt) {
    n = u.shortWriteAt > u.written.size() ? u.shortWriteAt - u.written.size() : 0;
  }
  u.written.insert(u.written.end(), data, data + n);
  if (n < len) {
    u.error = UPDATE_ERROR_WRITE;  // the library aborts the update
    u.running = false;
  }
  return n;
}

bool UpdateClass::end(bool evenIfRemaining) {
  fakes::UpdateState& u = fakes::ota().update;
  ++u.ends;
  u.endEvenIfRemaining = evenIfRemaining;
  fakes::note(std::string("Update.end ") + (evenIfRemaining ? "1" : "0"));
  if (u.error != UPDATE_ERROR_OK || !u.running) return false;
  if (!isFinished() && !evenIfRemaining) {
    u.error = UPDATE_ERROR_ABORT;
    u.running = false;
    return false;
  }
  u.running = false;
  if (!u.endResult) {
    u.error = u.endError;
    return false;
  }
  fakes::Ota& o = fakes::ota();
  o.boot = 1 - o.running;
  o.state[o.boot] = ESP_OTA_IMG_NEW;
  return true;
}

void UpdateClass::abort() {
  fakes::UpdateState& u = fakes::ota().update;
  ++u.aborts;
  fakes::note("Update.abort");
  u.running = false;
  u.error = UPDATE_ERROR_ABORT;
}

const char* UpdateClass::errorString() {
  fakes::UpdateState& u = fakes::ota().update;
  if (!u.errorText.empty()) return u.errorText.c_str();
  switch (u.error) {
    case UPDATE_ERROR_OK: return "No Error";
    case UPDATE_ERROR_WRITE: return "Flash Write Failed";
    case UPDATE_ERROR_ERASE: return "Flash Erase Failed";
    case UPDATE_ERROR_READ: return "Flash Read Failed";
    case UPDATE_ERROR_SPACE: return "Not Enough Space";
    case UPDATE_ERROR_SIZE: return "Bad Size Given";
    case UPDATE_ERROR_STREAM: return "Stream Read Timeout";
    case UPDATE_ERROR_MD5: return "MD5 Check Failed";
    case UPDATE_ERROR_MAGIC_BYTE: return "Wrong Magic Byte";
    case UPDATE_ERROR_ACTIVATE: return "Could Not Activate The Firmware";
    case UPDATE_ERROR_NO_PARTITION: return "Partition Could Not be Found";
    case UPDATE_ERROR_BAD_ARGUMENT: return "Bad Argument";
    case UPDATE_ERROR_ABORT: return "Aborted";
    default: return "UNKNOWN";
  }
}

bool UpdateClass::setMD5(const char* expected_md5) {
  fakes::UpdateState& u = fakes::ota().update;
  fakes::note(std::string("Update.setMD5 ") + (expected_md5 != nullptr ? expected_md5 : ""));
  if (expected_md5 == nullptr || strlen(expected_md5) != 32) return false;
  u.md5 = expected_md5;
  return u.setMd5Result;
}

uint8_t UpdateClass::getError() { return fakes::ota().update.error; }
void UpdateClass::clearError() { fakes::ota().update.error = UPDATE_ERROR_OK; }
bool UpdateClass::hasError() { return fakes::ota().update.error != UPDATE_ERROR_OK; }
bool UpdateClass::isRunning() { return fakes::ota().update.running; }

bool UpdateClass::isFinished() {
  const fakes::UpdateState& u = fakes::ota().update;
  const size_t target = u.beginSize == UPDATE_SIZE_UNKNOWN ? partitionSize() : u.beginSize;
  return u.written.size() == target;
}

size_t UpdateClass::size() {
  const fakes::UpdateState& u = fakes::ota().update;
  return u.beginSize == UPDATE_SIZE_UNKNOWN ? partitionSize() : u.beginSize;
}

size_t UpdateClass::progress() { return fakes::ota().update.written.size(); }

size_t UpdateClass::remaining() { return size() - progress(); }

// ---------------------------------------------------------------- esp_ota_ops

const esp_partition_t* esp_ota_get_running_partition(void) {
  fakes::Ota& o = fakes::ota();
  return o.hasRunning ? &o.parts[o.running] : nullptr;
}

const esp_partition_t* esp_ota_get_boot_partition(void) {
  fakes::Ota& o = fakes::ota();
  return &o.parts[o.boot];
}

const esp_partition_t* esp_ota_get_next_update_partition(const esp_partition_t* start_from) {
  fakes::Ota& o = fakes::ota();
  if (!o.hasNext) return nullptr;
  const int from = start_from != nullptr ? fakes::indexOf(start_from) : o.running;
  return &o.parts[from == 1 ? 0 : 1];
}

esp_err_t esp_ota_get_state_partition(const esp_partition_t* partition,
                                      esp_ota_img_states_t* ota_state) {
  fakes::Ota& o = fakes::ota();
  if (o.stateResult != ESP_OK) return o.stateResult;
  const int i = fakes::indexOf(partition);
  if (i < 0 || ota_state == nullptr) return ESP_ERR_INVALID_ARG;
  if (o.state[i] == ESP_OTA_IMG_UNDEFINED) return ESP_ERR_NOT_FOUND;
  *ota_state = o.state[i];
  return ESP_OK;
}

esp_err_t esp_ota_mark_app_valid_cancel_rollback(void) {
  fakes::Ota& o = fakes::ota();
  ++o.markValid;
  fakes::note("esp_ota_mark_app_valid_cancel_rollback");
  if (o.markValidResult != ESP_OK) return o.markValidResult;
  o.state[o.running] = ESP_OTA_IMG_VALID;
  return ESP_OK;
}

esp_err_t esp_ota_mark_app_invalid_rollback_and_reboot(void) {
  fakes::Ota& o = fakes::ota();
  ++o.markInvalid;
  fakes::note("esp_ota_mark_app_invalid_rollback_and_reboot");
  const int other = 1 - o.running;
  if (o.state[other] != ESP_OTA_IMG_VALID) return ESP_FAIL;  // nothing to roll back to
  o.state[o.running] = ESP_OTA_IMG_INVALID;
  o.boot = other;
  esp_restart();
}

esp_err_t esp_ota_set_boot_partition(const esp_partition_t* partition) {
  const int i = fakes::indexOf(partition);
  if (i < 0) return ESP_ERR_INVALID_ARG;
  fakes::ota().boot = i;
  return ESP_OK;
}
