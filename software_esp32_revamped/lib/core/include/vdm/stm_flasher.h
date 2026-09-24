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
// family-independent checks run: size 1..512 KiB, SP in (0x20000000,
// 0x20020000] and 4-aligned (an SP equal to the RAM base leaves no stack),
// reset vector odd and inside [0x08000000, 0x08000000+size)). With a PID,
// size <= flash size (0x423 256 KiB, 0x431 512 KiB, 0x433 512 KiB) and SP <=
// RAM top (0x423 0x20010000, 0x431 0x20020000, 0x433 0x20018000); any other
// non-zero PID gives UnknownChip. requireHandshake rejects images without the
// DEADBEEF/BEEFIT strings. Checks run cheapest first; the first failing one is
// returned and `out` keeps what was learnt up to that point.
FlashError validateImage(FlashImage& img, uint16_t chipPid, bool requireHandshake, ImageInfo& out);

// Sectors of an STM32F401/F411 covering [0, size): 16,16,16,16,64,128,...
// KiB. Returns the number of sectors (1..8), 0 when size is 0 or > 512 KiB.
uint8_t sectorsForImage(uint32_t size);

namespace detail {
// Incremental state of the image scan (CRC32, handshake strings, version
// string). Shared by validateImage() and the Validating phase, which scans a
// bounded number of bytes per step().
struct ImageScan {
  uint32_t offset = 0;
  uint32_t crc = 0xFFFFFFFFu;
  uint64_t window = 0;     // last 8 bytes, newest in the low byte
  bool dead = false;       // "DEADBEEF" seen
  bool beef = false;       // "BEEFIT" seen
  bool versionFound = false;
  uint8_t runLen = 0;      // printable run so far; 32 = too long
  char run[32] = {0};
};
}  // namespace detail

struct FlashOptions {
  bool blank = false;            // STM already in the ROM bootloader (BOOT0 held): no DEADBEEF
  bool force = false;            // skip handshake-string and version checks
  uint32_t baud = 115200;        // ROM bootloader 8E1; the v1 boot window is always 115200
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
  uint16_t appTimeoutMs = 15000;       // total wait for gvers, from NRST release
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

  // Starts a run. False (and nothing touched) when a run is active or
  // opt.baud is outside 1200..115200 (the AN3155 USART range). The image
  // object must outlive the run.
  //
  // Wire details beyond DESIGN.md §15:
  //  - The handshake sends "DEADBEEF\n" (9 bytes). The v1 STM compares fixed
  //    8-byte chunks without resync, so a stray byte at reset would misalign
  //    a pure 8-byte stream forever; the 9-byte period re-aligns within 8
  //    sends. The hardened STM ignores CR/LF.
  //  - Normal mode opens the UART at 115200 8E1 for the handshake (the v1
  //    boot window listens only there) and switches to opt.baud when BEEFIT
  //    arrives; blank mode opens it at opt.baud directly.
  //  - Every ACK timeout starts when the frame is queued and includes the
  //    frame's wire time at opt.baud (11 bits per byte), so slow bauds do not
  //    time out a 258-byte data frame that is still being sent.
  //  - GetId first sends GET (0x00) for the bootloader version; a failed GET
  //    is not fatal (the byte stays 0).
  //  - The verify pass recomputes the image CRC32; a difference from the
  //    validated one (file changed during the run) fails with ImageRead.
  //  - Any failure after the STM was touched pulses NRST and restores 8N1
  //    before the phase becomes Failed (phase keeps the failing phase during
  //    that pulse). Failures while waiting for the app need no new pulse.
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
  enum class Resp : uint8_t { Pending, Ack, Nack, Timeout };

  void enter(FlashPhase p, uint32_t nowMs);
  void finish(FlashPhase p, uint32_t nowMs);
  void fail(FlashError e, uint32_t address, uint32_t nowMs);
  void opFailed(FlashError e, uint32_t address, uint32_t nowMs);
  void setPercent(uint32_t p);
  bool put(const uint8_t* data, size_t len, uint32_t nowMs);
  bool send(const uint8_t* data, size_t len, uint32_t nowMs, uint32_t timeoutMs);
  void pump();
  Resp waitReply(uint32_t nowMs, size_t need);
  uint32_t blockCount() const;
  uint32_t blockLen(uint32_t block) const;
  bool loadBlock(uint32_t block);
  void runOnce(uint32_t nowMs);
  void stepValidating(uint32_t nowMs);
  void stepPulse(uint32_t nowMs);
  void stepHandshake(uint32_t nowMs);
  void stepSync(uint32_t nowMs);
  void stepGetId(uint32_t nowMs);
  void stepErasing(uint32_t nowMs);
  void stepWriting(uint32_t nowMs);
  void stepVerifying(uint32_t nowMs);
  void stepWaitingApp(uint32_t nowMs);
  void onAppLine(uint32_t nowMs);

  FlashTransport& t_;
  FlashImage* img_ = nullptr;
  FlashOptions opt_;
  FlashStatus st_;
  detail::ImageScan scan_;
  uint32_t phaseStartMs_ = 0;
  uint32_t releaseMs_ = 0;     // NRST released (Resetting/Starting)
  uint32_t lastSendMs_ = 0;
  uint32_t waitStartMs_ = 0;
  uint32_t waitLimitMs_ = 0;
  uint32_t block_ = 0;         // index in write/verify order
  uint32_t verifyCrc_ = 0;
  uint8_t retries_ = 0;
  uint8_t syncTries_ = 0;
  uint8_t sub_ = 0;            // sub-step within a phase
  bool touched_ = false;       // STM reset/UART re-opened: failure needs a pulse
  bool cleanup_ = false;       // failure pulse in progress
  bool lineDrop_ = false;      // WaitingApp: discard up to the next CR/LF
  bool abortRequested_ = false;
  uint8_t tx_[260];            // data frame N-1, 256 data, checksum / expected block
  uint8_t rx_[272];            // one read-back block + framing, or one app line
  size_t rxLen_ = 0;
};

}  // namespace vdm
