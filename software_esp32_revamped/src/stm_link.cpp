#include "stm_link.h"

#include <Arduino.h>
#include <esp_task_wdt.h>
#include <string.h>

#include <new>

#include <vdm/config.h>
#include <vdm/stm_flasher.h>
#include <vdm/stm_session.h>

#include "app.h"
#include "boot_alloc.h"
#include "board.h"
#include "logger.h"
#include "mqtt_client.h"
#include "ota.h"
#include "stm_service.h"
#include "storage.h"

namespace stm_link {

namespace {

HardwareSerial& gUart = Serial2;

void openUart(uint32_t baud, bool evenParity) {
  gUart.end();
  gUart.setRxBufferSize(board::kStmRxBufferSize);
  gUart.setTxBufferSize(board::kStmTxBufferSize);
  gUart.begin(baud, evenParity ? SERIAL_8E1 : SERIAL_8N1, board::kStmRxPin, board::kStmTxPin);
}

void setReset(bool asserted) {
  digitalWrite(board::kStmResetPin, asserted == board::kStmResetAssertedLevel ? HIGH : LOW);
}

// ---------------------------------------------------------------- flasher I/O

class UartTransport : public vdm::FlashTransport {
 public:
  void configure(uint32_t baud, bool evenParity) override { openUart(baud, evenParity); }
  // The flasher writes at most one frame (<= 260 B) per step after the
  // previous one was ACKed, so the 512 B TX ring buffer never blocks here.
  size_t write(const uint8_t* data, size_t len) override { return gUart.write(data, len); }
  size_t read(uint8_t* out, size_t cap) override {
    const int avail = gUart.available();
    if (avail <= 0 || cap == 0) return 0;
    return gUart.read(out, static_cast<size_t>(avail) < cap ? static_cast<size_t>(avail) : cap);
  }
  void discardInput() override {
    uint8_t buf[64];
    for (int i = 0; i < 64 && gUart.available() > 0; ++i) gUart.read(buf, sizeof buf);
  }
  void setReset(bool asserted) override { stm_link::setReset(asserted); }
};

// ---------------------------------------------------------------- port

storage::FileImage gImage;

class Port : public vdm::StmSessionPort {
 public:
  void logEvent(const vdm::Event& e) override { logger::log(e); }
  uint32_t pulseReset(uint32_t) override {
    stm_link::pulseReset();
    return app::nowMs();
  }
  void publish(const vdm::StmSnapshot& s) override { app::publishStmSnapshot(s); }
  void requestLastGoodCopy(const char* image) override { storage::requestLastGoodCopy(image); }
  bool restartPending() override { return ota::restartPending(); }
  void markFlashActive() override { app::markStmFlashActive(); }
  void storeDesiredTargets(const vdm::PersistedTargets& t) override {
    stm_service::storeDesiredTargets(t);
  }
  void postScheduledCalibResult(uint16_t attempt, bool ok, vdm::CalibFailure reason) override {
    stm_service::postScheduledCalibResult(attempt, ok, reason);
  }
  void setStmSaveState(vdm::StmSaveState s) override { app::setStmSaveState(s); }
  void storeLeaseRecord(const vdm::LeaseClient::Snapshot& s) override {
    stm_service::storeLeaseRecord(s);
  }
  vdm::FlashImage* openImage(const char* name) override {
    return gImage.open(name) ? &gImage : nullptr;
  }
  void closeImage() override { gImage.close(); }
};

UartTransport gTransport;
Port gPort;

vdm::StmSession& newSession() {
  vdm::StmSession* s =
      new (std::nothrow) vdm::StmSession(gPort, gTransport, bootAlloc<vdm::StmSnapshot>());
  if (s == nullptr) abort();  // out of memory at boot: nothing sensible to do
  return *s;
}

// ~12 KB (config, snapshot, reply): allocated once at boot, like bootAlloc().
vdm::StmSession& gSession = newSession();
vdm::Config& gCfg = bootAlloc<vdm::Config>();
uint32_t gCfgRevision = 0;

void reloadConfig() {
  gCfgRevision = storage::configRevision();
  storage::getConfig(gCfg);
  // Defaults nobody saved must not overwrite the failsafe settings the STM holds.
  const storage::LoadSource src = storage::bootLoadSource();
  const bool defaults =
      src == storage::LoadSource::Defaults || src == storage::LoadSource::DefaultsAfterError;
  gSession.applyConfig(gCfg, !defaults || storage::configSavedSinceBoot());
}

void readUart(uint32_t now) {
  char buf[128];
  // Bounded per iteration: at most 4 chunks (512 B) before yielding.
  for (int chunk = 0; chunk < 4; ++chunk) {
    const int avail = gUart.available();
    if (avail <= 0) return;
    const size_t want =
        static_cast<size_t>(avail) < sizeof buf ? static_cast<size_t>(avail) : sizeof buf;
    const size_t n = gUart.read(reinterpret_cast<uint8_t*>(buf), want);
    gSession.onRx(buf, n, now);
  }
}

}  // namespace

void pulseReset() {
  setReset(true);
  vTaskDelay(pdMS_TO_TICKS(100));
  setReset(false);
}

void releaseReset() {
  // BOOT0 LOW before NRST is released, so a wired BOOT0 never starts the
  // ROM bootloader.
  digitalWrite(board::kStmBoot0Pin, LOW);
  pinMode(board::kStmBoot0Pin, OUTPUT);
  setReset(false);
  pinMode(board::kStmResetPin, OUTPUT);
}

void begin() {
  // R6: never reset the STM on ESP boot. NRST was released in initVariant
  // already; doing it again is harmless and covers a missing hook.
  releaseReset();
  openUart(board::kStmBaud, false);
}

void task(void*) {
  esp_task_wdt_add(nullptr);
  const uint32_t start = app::nowMs();
  reloadConfig();
  vdm::RestoreSource src = vdm::RestoreSource::None;
  const vdm::PersistedTargets& targets = stm_service::bootTargets(src);
  vdm::LeaseClient::Snapshot lease;
  const bool haveLease = stm_service::bootLease(lease);
  gSession.begin(start, targets, src, haveLease ? &lease : nullptr);
  uint32_t lastSecondMs = start;
  for (;;) {
    esp_task_wdt_reset();
    const uint32_t now = app::nowMs();
    if (storage::configRevision() != gCfgRevision) reloadConfig();

    app::Command cmd;
    for (int i = 0; i < 4 && app::receive(cmd); ++i) gSession.handleCommand(cmd, now);

    if (gSession.flashing()) {
      gSession.flashStep(now);
    } else {
      readUart(now);
      gSession.poll(now);
      if (const vdm::RequestLine* line = gSession.nextToSend(now)) {
        gUart.write(reinterpret_cast<const uint8_t*>(line->text), line->len);
        gSession.onSent(app::nowMs());
      }
    }
    if (vdm::elapsedMs(now, lastSecondMs) >= 1000) {
      lastSecondMs = now;
      gSession.everySecond(now, mqtt::regulatorState(), app::stmSaveState());
    }
    gSession.publishIfDue(now);
    vTaskDelay(pdMS_TO_TICKS(2));
  }
}

}  // namespace stm_link
