// STM32 flashing over the ST ROM bootloader (AN3155, USART, 8E1) as a
// non-blocking state machine over an abstract byte transport, plus image
// validation. Hardware-free; the stm_link glue implements the transport on
// Serial2/NRST and calls step() every 1-2 ms from the STM task while the
// LinkPolicy is suspended. References: specs/02-stm-flashing.md §7, §11.
#pragma once

#include <stddef.h>
#include <stdint.h>

#include "vdm/common.h"
#include "vdm/version.h"

namespace vdm {

// UART + reset line owned by the flasher while it runs.
class FlashTransport {
 public:
  virtual ~FlashTransport() = default;
  // Re-open the UART: 8E1 (bootloader) or 8N1 (application), RX buffer >= 1 KiB.
  virtual void configure(uint32_t baud, bool evenParity) = 0;
  // Non-blocking write; returns bytes accepted (the glue's TX buffer is
  // large enough for one 256-byte block + framing, so a short write is an
  // error for the flasher).
  virtual size_t write(const uint8_t* data, size_t len) = 0;
  // Non-blocking read of up to `cap` bytes; returns bytes read.
  virtual size_t read(uint8_t* out, size_t cap) = 0;
  virtual void discardInput() = 0;
  // true = hold the STM in reset (NRST asserted; IO15 HIGH on the board).
  virtual void setReset(bool asserted) = 0;
};

// Random-access image (LittleFS file in glue, array in tests).
class FlashImage {
 public:
  virtual ~FlashImage() = default;
  virtual uint32_t size() const = 0;
  // Reads len bytes at offset; false on I/O error or out of range.
  virtual bool read(uint32_t offset, uint8_t* out, size_t len) = 0;
};

enum class FlashPhase : uint8_t {
  Idle,
  Validating,   // image checks before touching the STM
  Resetting,    // NRST pulse, UART 8E1
  Handshake,    // DEADBEEF every 100 ms, wait BEEFIT (normal mode)
  Sync,         // 0x7F -> ACK/NACK
  GetId,        // 0x02
  Erasing,      // 0x44 sector list (or mass erase)
  Writing,      // 0x31 blocks, block 0 last
  Verifying,    // 0x11 read-back compare
  Starting,     // NRST pulse, UART 8N1
  WaitingApp,   // gvers until the new application answers
  Done,
  Failed,
};
const char* flashPhaseName(FlashPhase p);  // "idle","validating",... (spec 02 R8)
// Legacy /stmupdstatus status code 0..8 for the old UI (spec 02 §8).
uint8_t legacyFlashStatus(FlashPhase p);

enum class FlashError : uint8_t {
  None = 0,
  ImageEmpty,
  ImageTooLarge,       // > flash size of the detected chip (or 512 KiB before GetId)
  ImageBadVectors,     // initial SP / reset vector outside RAM / image
  ImageNoHandshake,    // "DEADBEEF"/"BEEFIT" missing (next update would need BOOT0)
  ImageChipMismatch,   // SP above the RAM top of the detected chip
  ImageRead,           // FlashImage::read failed
  HandshakeTimeout,    // no BEEFIT within handshakeWindowMs
  SyncFailed,          // no ACK/NACK to 0x7F after retries
  UnknownChip,         // PID not 0x423/0x431/0x433
  Nack,                // bootloader NACK (see errorPhase/errorAddress)
  Timeout,             // no ACK in time
  VerifyMismatch,      // read-back differs (errorAddress = first bad byte)
  TransportWrite,      // short write
  AppNotResponding,    // no gvers after flashing
  AppVersionMismatch,  // gvers differs from the version found in the image
  Aborted,
};
const char* flashErrorName(FlashError e);

// Image facts found by validateImage().
struct ImageInfo {
  uint32_t size = 0;
  uint32_t paddedSize = 0;       // size rounded up to 4 (padding 0xFF)
  uint32_t initialSp = 0;
  uint32_t resetVector = 0;
  bool hasHandshake = false;     // contains "DEADBEEF" and "BEEFIT"
  uint32_t crc = 0;              // CRC32 of the unpadded image
  char version[32] = {0};        // first NUL-terminated string that parses as a
                                 // Version and contains no spaces, "" if none
};

// Pure image checks (spec 02 R3). chipPid 0 = not known yet (then only the
// family-independent checks run: size 1..512 KiB, SP in 0x20000000..
// 0x20020000 and 4-aligned, reset vector odd and inside [0x08000000,
// 0x08000000+size)). With a PID, size <= flash size (0x423 256 KiB, 0x431
// 512 KiB, 0x433 512 KiB) and SP <= RAM top (0x423 0x20010000, 0x431
// 0x20020000, 0x433 0x20018000). requireHandshake rejects images without the
// DEADBEEF/BEEFIT strings.
FlashError validateImage(FlashImage& img, uint16_t chipPid, bool requireHandshake, ImageInfo& out);

// Sectors of an STM32F401/F411 covering [0, size): 16,16,16,16,64,128,...
// KiB. Returns the number of sectors (1..8), 0 when size is 0 or > 512 KiB.
uint8_t sectorsForImage(uint32_t size);

struct FlashOptions {
  bool blank = false;            // STM already in the ROM bootloader (BOOT0 held): no DEADBEEF
  bool force = false;            // skip handshake-string and version checks
  uint32_t baud = 115200;
  // Binding timing (DESIGN.md "STM flasher"):
  uint16_t resetPulseMs = 100;         // NRST asserted
  uint16_t handshakeFirstMs = 20;      // after reset release
  uint16_t handshakeRepeatMs = 100;    // DEADBEEF resend period
  uint16_t handshakeWindowMs = 2500;   // give up
  uint16_t afterBeefitMs = 250;        // before 0x7F
  uint8_t syncAttempts = 3;
  uint16_t ackTimeoutMs = 1000;        // command/address/data ACK
  uint32_t eraseTimeoutMs = 60000;
  uint8_t blockRetries = 3;
  uint8_t sessionRetries = 2;          // whole erase+write+verify again, same ROM session
  uint16_t appBootMs = 4000;           // after the final reset, before the first gvers
  uint16_t appPollMs = 1000;           // gvers period
  uint16_t appTimeoutMs = 15000;       // total wait for gvers
};

struct FlashStatus {
  FlashPhase phase = FlashPhase::Idle;
  FlashError error = FlashError::None;
  FlashPhase errorPhase = FlashPhase::Idle;
  uint32_t errorAddress = 0;
  uint8_t percent = 0;            // monotonic 0..100
  uint32_t bytesDone = 0;         // written (Writing) or verified (Verifying)
  uint32_t bytesTotal = 0;        // paddedSize
  uint16_t chipPid = 0;           // from GetId
  uint8_t bootloaderVersion = 0;  // from GET (0x00) when read
  uint8_t attempt = 0;            // session retry number (0 = first)
  uint32_t startedMs = 0;
  uint32_t finishedMs = 0;
  ImageInfo image;
  Version appVersion;             // gvers after flashing
};

class StmFlasher {
 public:
  explicit StmFlasher(FlashTransport& transport);

  // Starts a run. False (and nothing touched) when a run is active. The image
  // object must outlive the run.
  bool begin(FlashImage& image, const FlashOptions& opt, uint32_t nowMs);
  // Advances the state machine; never blocks longer than the time to queue
  // one block (~300 bytes) on the transport. Returns the current phase.
  FlashPhase step(uint32_t nowMs);
  // Requests an abort: the STM is reset into the application (which may be
  // gone if erase already started; then the status says Failed/Aborted).
  void abort();

  bool active() const;  // phase not Idle/Done/Failed
  const FlashStatus& status() const { return st_; }

 private:
  FlashTransport& t_;
  FlashImage* img_ = nullptr;
  FlashOptions opt_;
  FlashStatus st_;
  uint32_t phaseStartMs_ = 0;
  uint32_t lastSendMs_ = 0;
  uint32_t block_ = 0;
  uint8_t retries_ = 0;
  uint8_t sub_ = 0;           // sub-step within a phase
  uint8_t rx_[272];           // one read-back block + framing
  size_t rxLen_ = 0;
  bool abortRequested_ = false;
};

}  // namespace vdm
