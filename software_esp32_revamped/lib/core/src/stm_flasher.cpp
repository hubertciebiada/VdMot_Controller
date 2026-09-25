#include "vdm/stm_flasher.h"

#include <string.h>

#include "vdm/stm_codec.h"

namespace vdm {

namespace {

constexpr uint8_t kAck = 0x79;
constexpr uint8_t kNack = 0x1F;
constexpr uint8_t kCmdGet = 0x00;
constexpr uint8_t kCmdGetId = 0x02;
constexpr uint8_t kCmdRead = 0x11;
constexpr uint8_t kCmdWrite = 0x31;
constexpr uint8_t kCmdExtErase = 0x44;
constexpr uint8_t kSync = 0x7F;

constexpr uint32_t kFlashBase = 0x08000000u;
constexpr uint32_t kRamBase = 0x20000000u;
constexpr uint32_t kRamTopMax = 0x20020000u;
constexpr uint32_t kMaxImage = 512u * 1024u;
constexpr uint32_t kBlockSize = 256;
constexpr uint32_t kAppBaud = 115200;
constexpr uint32_t kHandshakeBaud = 115200;  // the v1 STM boot window listens at 115200 8E1 only
constexpr uint32_t kMinBaud = 1200;
constexpr uint32_t kMaxBaud = 115200;
constexpr uint32_t kValidateChunksPerStep = 4;  // 1 KiB of image per step()
constexpr uint8_t kMaxTransitionsPerStep = 4;
constexpr size_t kAppReadPerStep = 256;

// 9 bytes: the period re-aligns the v1 STM's fixed 8-byte matcher.
const uint8_t kHandshake[] = {'D', 'E', 'A', 'D', 'B', 'E', 'E', 'F', '\n'};
const uint8_t kBeefit[] = {'B', 'E', 'E', 'F', 'I', 'T'};
constexpr uint64_t kDeadWord = 0x4445414442454546ull;  // "DEADBEEF"
constexpr uint64_t kBeefWord = 0x424545464954ull;      // "BEEFIT"
constexpr uint64_t kBeefMask = 0xFFFFFFFFFFFFull;

// F401/F411 sector sizes (sectors 0..7).
const uint32_t kSectorSize[] = {16u * 1024u, 16u * 1024u,  16u * 1024u,  16u * 1024u,
                                64u * 1024u, 128u * 1024u, 128u * 1024u, 128u * 1024u};

uint32_t chipFlashSize(uint16_t pid) {
  switch (pid) {
    case 0x423: return 256u * 1024u;
    case 0x431: return 512u * 1024u;
    case 0x433: return 512u * 1024u;
    default: return 0;
  }
}

uint32_t chipRamTop(uint16_t pid) {
  switch (pid) {
    case 0x423: return 0x20010000u;
    case 0x431: return 0x20020000u;
    case 0x433: return 0x20018000u;
    default: return 0;
  }
}

uint32_t readLe32(const uint8_t* p) {
  return static_cast<uint32_t>(p[0]) | (static_cast<uint32_t>(p[1]) << 8) |
         (static_cast<uint32_t>(p[2]) << 16) | (static_cast<uint32_t>(p[3]) << 24);
}

// CRC-32 (IEEE 802.3, reflected 0xEDB88320) on the pre-inverted state.
uint32_t crc32Update(uint32_t crc, const uint8_t* data, size_t len) {
  static const uint32_t kNibble[16] = {
      0x00000000u, 0x1DB71064u, 0x3B6E20C8u, 0x26D930ACu, 0x76DC4190u, 0x6B6B51F4u,
      0x4DB26158u, 0x5005713Cu, 0xEDB88320u, 0xF00F9344u, 0xD6D6A3E8u, 0xCB61B38Cu,
      0x9B64C2B0u, 0x86D3D2D4u, 0xA00AE278u, 0xBDBDF21Cu};
  for (size_t i = 0; i < len; ++i) {
    crc ^= data[i];
    crc = (crc >> 4) ^ kNibble[crc & 0x0Fu];
    crc = (crc >> 4) ^ kNibble[crc & 0x0Fu];
  }
  return crc;
}

bool isDigit(char c) { return c >= '0' && c <= '9'; }

// A run that ends with "VDM-HW:C" + 1..2 digits (contracts: STM image
// revision marker); writes "C<digits>" to tag.
bool hwMarker(const char* run, size_t len, char (&tag)[4]) {
  static const char kMarker[] = "VDM-HW:C";
  constexpr size_t kMarkerLen = sizeof kMarker - 1;
  if (len < kMarkerLen + 1 || !isDigit(run[len - 1])) return false;
  const size_t digits = len >= kMarkerLen + 2 && isDigit(run[len - 2]) ? 2 : 1;
  if (len < kMarkerLen + digits || memcmp(run + len - digits - kMarkerLen, kMarker, kMarkerLen) != 0) {
    return false;
  }
  tag[0] = 'C';
  memcpy(tag + 1, run + len - digits, digits);
  tag[1 + digits] = '\0';
  return true;
}

void scanBytes(detail::ImageScan& sc, const uint8_t* data, size_t len, ImageInfo& info) {
  sc.crc = crc32Update(sc.crc, data, len);
  for (size_t i = 0; i < len; ++i) {
    const uint8_t c = data[i];
    sc.window = (sc.window << 8) | c;
    if (sc.window == kDeadWord) sc.dead = true;
    if ((sc.window & kBeefMask) == kBeefWord) sc.beef = true;
    if (c == 0) {
      // runLen == sizeof run marks a run longer than any version (31 chars).
      if (!sc.versionFound && sc.runLen > 0 && sc.runLen < sizeof sc.run) {
        Version v;
        if (parseVersion(sc.run, sc.runLen, v)) {
          memcpy(info.version, sc.run, sc.runLen);
          info.version[sc.runLen] = '\0';
          sc.versionFound = true;
        }
      }
      char tag[4];
      if (sc.runLen < sizeof sc.run && hwMarker(sc.run, sc.runLen, tag)) {
        if (info.hwTag[0] == '\0') {
          memcpy(info.hwTag, tag, sizeof tag);
        } else if (strcmp(info.hwTag, tag) != 0) {
          info.hwConflict = true;
        }
      }
      sc.runLen = 0;
    } else if (c >= 0x20 && c <= 0x7E) {
      if (sc.runLen < sizeof sc.run - 1) sc.run[sc.runLen] = static_cast<char>(c);
      if (sc.runLen < sizeof sc.run) ++sc.runLen;
    } else {
      sc.runLen = 0;
    }
  }
  sc.offset += static_cast<uint32_t>(len);
}

void finishScan(const detail::ImageScan& sc, ImageInfo& info) {
  info.crc = ~sc.crc;
  info.hasHandshake = sc.dead && sc.beef;
}

// Family-independent checks; resets `out` and fills size and vectors.
FlashError checkHeader(FlashImage& img, ImageInfo& out) {
  out = ImageInfo{};
  const uint32_t size = img.size();
  out.size = size;
  if (size == 0) return FlashError::ImageEmpty;
  if (size > kMaxImage) return FlashError::ImageTooLarge;
  out.paddedSize = (size + 3u) & ~3u;
  if (size < 8) return FlashError::ImageBadVectors;
  uint8_t vec[8];
  if (!img.read(0, vec, sizeof vec)) return FlashError::ImageRead;
  out.initialSp = readLe32(vec);
  out.resetVector = readLe32(vec + 4);
  const uint32_t sp = out.initialSp;
  const uint32_t pc = out.resetVector;
  if (sp <= kRamBase || sp > kRamTopMax || (sp & 3u) != 0) return FlashError::ImageBadVectors;
  if ((pc & 1u) == 0 || pc < kFlashBase || pc >= kFlashBase + size) {
    return FlashError::ImageBadVectors;
  }
  return FlashError::None;
}

FlashError checkChip(const ImageInfo& info, uint16_t pid) {
  const uint32_t flash = chipFlashSize(pid);
  if (flash == 0) return FlashError::UnknownChip;
  if (info.size > flash) return FlashError::ImageTooLarge;
  if (info.initialSp > chipRamTop(pid)) return FlashError::ImageChipMismatch;
  return FlashError::None;
}

// Same numbers and suffix; the image's hw tag (usually a separate string in
// the binary, so empty) must match only when present. The board marker of
// the image is checked separately.
bool sameVersion(const Version& image, const Version& app) {
  return image.major == app.major && image.minor == app.minor && image.patch == app.patch &&
         strcmp(image.suffix, app.suffix) == 0 &&
         (image.hw[0] == '\0' || strcmp(image.hw, app.hw) == 0);
}

// Time `bytes` take on the wire at 8E1 (11 bits per byte), rounded up.
uint32_t wireMs(size_t bytes, uint32_t baud) {
  return static_cast<uint32_t>((bytes * 11u * 1000u + baud - 1u) / baud);
}

void addressFrame(uint32_t addr, uint8_t (&out)[5]) {
  out[0] = static_cast<uint8_t>(addr >> 24);
  out[1] = static_cast<uint8_t>(addr >> 16);
  out[2] = static_cast<uint8_t>(addr >> 8);
  out[3] = static_cast<uint8_t>(addr);
  out[4] = static_cast<uint8_t>(out[0] ^ out[1] ^ out[2] ^ out[3]);
}

}  // namespace

// ---------------------------------------------------------------- names

const char* flashPhaseName(FlashPhase p) {
  switch (p) {
    case FlashPhase::Idle: return "idle";
    case FlashPhase::Validating: return "validating";
    case FlashPhase::Resetting: return "resetting";
    case FlashPhase::Handshake: return "handshake";
    case FlashPhase::Sync: return "sync";
    case FlashPhase::GetId: return "getid";
    case FlashPhase::Erasing: return "erasing";
    case FlashPhase::Writing: return "writing";
    case FlashPhase::Verifying: return "verifying";
    case FlashPhase::Starting: return "starting";
    case FlashPhase::WaitingApp: return "waiting_app";
    case FlashPhase::Done: return "done";
    case FlashPhase::Failed: return "failed";
  }
  return "unknown";
}

uint8_t legacyFlashStatus(FlashPhase p) {
  switch (p) {
    case FlashPhase::Idle: return 0;
    case FlashPhase::Validating:
    case FlashPhase::Resetting:
    case FlashPhase::Handshake:
    case FlashPhase::Sync:
    case FlashPhase::GetId: return 1;
    case FlashPhase::Erasing: return 3;
    case FlashPhase::Writing: return 4;
    case FlashPhase::Verifying:
    case FlashPhase::Starting:
    case FlashPhase::WaitingApp: return 5;
    case FlashPhase::Done: return 6;
    case FlashPhase::Failed: return 8;
  }
  return 8;
}

const char* flashErrorName(FlashError e) {
  switch (e) {
    case FlashError::None: return "none";
    case FlashError::ImageEmpty: return "image_empty";
    case FlashError::ImageTooLarge: return "image_too_large";
    case FlashError::ImageBadVectors: return "image_bad_vectors";
    case FlashError::ImageNoHandshake: return "image_no_handshake";
    case FlashError::ImageChipMismatch: return "image_chip_mismatch";
    case FlashError::ImageRead: return "image_read";
    case FlashError::HandshakeTimeout: return "handshake_timeout";
    case FlashError::SyncFailed: return "sync_failed";
    case FlashError::UnknownChip: return "unknown_chip";
    case FlashError::Nack: return "nack";
    case FlashError::Timeout: return "timeout";
    case FlashError::VerifyMismatch: return "verify_mismatch";
    case FlashError::TransportWrite: return "transport_write";
    case FlashError::AppNotResponding: return "app_not_responding";
    case FlashError::AppVersionMismatch: return "app_version_mismatch";
    case FlashError::Aborted: return "aborted";
    case FlashError::BoardMismatch: return "board_mismatch";
    case FlashError::BoardRequired: return "board_required";
  }
  return "unknown";
}

// ---------------------------------------------------------------- board check

const char* boardCheckName(BoardCheck c) {
  switch (c) {
    case BoardCheck::Ok: return "ok";
    case BoardCheck::Untagged: return "untagged";
    case BoardCheck::Mismatch: return "mismatch";
    case BoardCheck::BoardRequired: return "board_required";
  }
  return "unknown";
}

bool boardTagValid(const char* s) {
  if (s == nullptr || s[0] != 'C') return false;
  size_t digits = 0;
  while (digits < 3 && s[1 + digits] >= '0' && s[1 + digits] <= '9') ++digits;
  return digits >= 1 && digits <= 2 && s[1 + digits] == '\0';
}

BoardCheck checkBoard(const char* imageHw, const char* boardHw) {
  constexpr size_t kTagCap = sizeof(ImageInfo::hwTag);
  const size_t imageLen = boundedLength(imageHw, kTagCap);
  const size_t boardLen = boundedLength(boardHw, kTagCap);
  if (imageLen == 0) return BoardCheck::Untagged;
  if (boardLen == 0) return BoardCheck::BoardRequired;
  return imageLen == boardLen && memcmp(imageHw, boardHw, imageLen) == 0 ? BoardCheck::Ok
                                                                         : BoardCheck::Mismatch;
}

// ---------------------------------------------------------------- image

FlashError validateImage(FlashImage& img, uint16_t chipPid, bool requireHandshake, ImageInfo& out) {
  FlashError e = checkHeader(img, out);
  if (e != FlashError::None) return e;
  if (chipPid != 0) {
    e = checkChip(out, chipPid);
    if (e != FlashError::None) return e;
  }
  detail::ImageScan sc;
  uint8_t buf[kBlockSize];
  while (sc.offset < out.size) {
    const uint32_t left = out.size - sc.offset;
    const size_t n = left < kBlockSize ? left : kBlockSize;
    if (!img.read(sc.offset, buf, n)) return FlashError::ImageRead;
    scanBytes(sc, buf, n, out);
  }
  finishScan(sc, out);
  if (requireHandshake && !out.hasHandshake) return FlashError::ImageNoHandshake;
  return FlashError::None;
}

uint8_t sectorsForImage(uint32_t size) {
  if (size == 0 || size > kMaxImage) return 0;
  uint32_t end = 0;
  for (uint8_t i = 0; i < sizeof kSectorSize / sizeof kSectorSize[0]; ++i) {
    end += kSectorSize[i];
    if (size <= end) return static_cast<uint8_t>(i + 1);
  }
  return 0;  // unreachable: the sectors add up to kMaxImage
}

// ---------------------------------------------------------------- flasher

StmFlasher::StmFlasher(FlashTransport& transport) : t_(transport), tx_(), rx_() {}

bool StmFlasher::begin(FlashImage& image, const FlashOptions& opt, uint32_t nowMs) {
  if (active() || opt.baud < kMinBaud || opt.baud > kMaxBaud) return false;
  img_ = &image;
  opt_ = opt;
  st_ = FlashStatus{};
  st_.startedMs = nowMs;
  sessionBaud_ = opt.baud;
  st_.baud = opt.baud;
  memcpy(st_.boardHw, opt.boardHw, sizeof st_.boardHw);
  st_.boardHw[sizeof st_.boardHw - 1] = '\0';
  scan_ = detail::ImageScan{};
  releaseMs_ = lastSendMs_ = waitStartMs_ = nowMs;
  waitLimitMs_ = 0;
  block_ = 0;
  verifyCrc_ = 0;
  syncTries_ = 0;
  touched_ = cleanup_ = lineDrop_ = abortRequested_ = false;
  rxLen_ = 0;
  enter(FlashPhase::Validating, nowMs);
  return true;
}

bool StmFlasher::active() const {
  return st_.phase != FlashPhase::Idle && st_.phase != FlashPhase::Done &&
         st_.phase != FlashPhase::Failed;
}

void StmFlasher::abort() {
  if (active()) abortRequested_ = true;
}

FlashPhase StmFlasher::step(uint32_t nowMs) {
  if (!active()) return st_.phase;
  if (abortRequested_) {
    abortRequested_ = false;
    if (!cleanup_) fail(FlashError::Aborted, 0, nowMs);
  }
  // Chain transitions that need no waiting (ACK already there -> next
  // frame). Four transitions contain at most one data frame, so a step never
  // queues more than one block.
  for (uint8_t i = 0; i < kMaxTransitionsPerStep && active(); ++i) {
    const FlashPhase phase = st_.phase;
    const uint8_t sub = sub_;
    const uint32_t block = block_;
    const bool cleanup = cleanup_;
    const uint8_t attempt = st_.attempt;
    const uint8_t retries = retries_;
    const uint8_t syncTries = syncTries_;
    runOnce(nowMs);
    if (phase == st_.phase && sub == sub_ && block == block_ && cleanup == cleanup_ &&
        attempt == st_.attempt && retries == retries_ && syncTries == syncTries_) {
      break;
    }
  }
  return st_.phase;
}

void StmFlasher::runOnce(uint32_t nowMs) {
  if (cleanup_) {
    stepPulse(nowMs);
    return;
  }
  switch (st_.phase) {
    case FlashPhase::Validating: stepValidating(nowMs); break;
    case FlashPhase::Resetting:
    case FlashPhase::Starting: stepPulse(nowMs); break;
    case FlashPhase::Handshake: stepHandshake(nowMs); break;
    case FlashPhase::Sync: stepSync(nowMs); break;
    case FlashPhase::GetId: stepGetId(nowMs); break;
    case FlashPhase::Erasing: stepErasing(nowMs); break;
    case FlashPhase::Writing: stepWriting(nowMs); break;
    case FlashPhase::Verifying: stepVerifying(nowMs); break;
    case FlashPhase::WaitingApp: stepWaitingApp(nowMs); break;
    case FlashPhase::Idle:
    case FlashPhase::Done:
    case FlashPhase::Failed: break;
  }
}

void StmFlasher::enter(FlashPhase p, uint32_t nowMs) {
  st_.phase = p;
  phaseStartMs_ = nowMs;
  sub_ = 0;
  retries_ = 0;
}

void StmFlasher::finish(FlashPhase p, uint32_t nowMs) {
  enter(p, nowMs);
  st_.finishedMs = nowMs;
  if (p == FlashPhase::Done) st_.percent = 100;
  touched_ = false;
  cleanup_ = false;
  img_ = nullptr;
}

void StmFlasher::fail(FlashError e, uint32_t address, uint32_t nowMs) {
  st_.error = e;
  st_.errorPhase = st_.phase;
  st_.errorAddress = address;
  if (touched_) {
    cleanup_ = true;
    sub_ = 0;
  } else {
    finish(FlashPhase::Failed, nowMs);
  }
}

// Write/verify failures retry the block; erase failures and exhausted block
// retries repeat erase+write+verify in the same ROM session.
void StmFlasher::opFailed(FlashError e, uint32_t address, uint32_t nowMs) {
  if (st_.phase != FlashPhase::Erasing && retries_ < opt_.blockRetries) {
    ++retries_;
    sub_ = 0;
    return;
  }
  if (st_.attempt < opt_.sessionRetries) {
    ++st_.attempt;
    enter(FlashPhase::Erasing, nowMs);
    return;
  }
  fail(e, address, nowMs);
}

void StmFlasher::setPercent(uint32_t p) {
  if (p > 100) p = 100;
  if (p > st_.percent) st_.percent = static_cast<uint8_t>(p);
}

bool StmFlasher::put(const uint8_t* data, size_t len, uint32_t nowMs) {
  if (t_.write(data, len) != len) {
    fail(FlashError::TransportWrite, 0, nowMs);
    return false;
  }
  return true;
}

bool StmFlasher::send(const uint8_t* data, size_t len, uint32_t nowMs, uint32_t timeoutMs) {
  t_.discardInput();
  rxLen_ = 0;
  if (!put(data, len, nowMs)) return false;
  // write() only queues: the reply cannot start before the frame has left.
  waitStartMs_ = nowMs;
  waitLimitMs_ = timeoutMs + wireMs(len, sessionBaud_);
  return true;
}

void StmFlasher::pump() {
  if (rxLen_ >= sizeof rx_) return;
  const size_t cap = sizeof rx_ - rxLen_;
  const size_t n = t_.read(rx_ + rxLen_, cap);
  rxLen_ += n < cap ? n : cap;
}

// Waits for an ACK followed by `need - 1` more bytes (need >= 1), or a NACK.
// Bytes before the first ACK/NACK are line noise and dropped. Nothing is
// consumed; the next send() clears the buffer.
StmFlasher::Resp StmFlasher::waitReply(uint32_t nowMs, size_t need) {
  pump();
  size_t skip = 0;
  while (skip < rxLen_ && rx_[skip] != kAck && rx_[skip] != kNack) ++skip;
  if (skip > 0) {
    memmove(rx_, rx_ + skip, rxLen_ - skip);
    rxLen_ -= skip;
  }
  if (rxLen_ > 0 && rx_[0] == kNack) return Resp::Nack;
  if (rxLen_ > 0 && rxLen_ >= need) return Resp::Ack;
  if (elapsedMs(nowMs, waitStartMs_) >= waitLimitMs_) return Resp::Timeout;
  return Resp::Pending;
}

uint32_t StmFlasher::blockCount() const {
  return (st_.image.paddedSize + kBlockSize - 1) / kBlockSize;
}

uint32_t StmFlasher::blockLen(uint32_t block) const {
  const uint32_t left = st_.image.paddedSize - block * kBlockSize;
  return left < kBlockSize ? left : kBlockSize;
}

// tx_ = N-1, data (image bytes, then 0xFF padding), checksum N-1^D0^..^DN.
bool StmFlasher::loadBlock(uint32_t block) {
  const uint32_t len = blockLen(block);
  const uint32_t off = block * kBlockSize;
  const uint32_t size = st_.image.size;
  const uint32_t avail = off >= size ? 0 : (size - off < len ? size - off : len);
  if (avail > 0 && (img_ == nullptr || !img_->read(off, tx_ + 1, avail))) return false;
  memset(tx_ + 1 + avail, 0xFF, len - avail);
  tx_[0] = static_cast<uint8_t>(len - 1);
  uint8_t cs = tx_[0];
  for (uint32_t i = 0; i < len; ++i) cs ^= tx_[1 + i];
  tx_[1 + len] = cs;
  return true;
}

void StmFlasher::stepValidating(uint32_t nowMs) {
  if (sub_ == 0) {
    const FlashError e = checkHeader(*img_, st_.image);
    if (e != FlashError::None) {
      fail(e, 0, nowMs);
      return;
    }
    st_.bytesTotal = st_.image.paddedSize;
    sub_ = 1;
    return;
  }
  const uint32_t size = st_.image.size;
  for (uint32_t i = 0; i < kValidateChunksPerStep && scan_.offset < size; ++i) {
    const uint32_t left = size - scan_.offset;
    const size_t n = left < kBlockSize ? left : kBlockSize;
    if (!img_->read(scan_.offset, tx_, n)) {
      fail(FlashError::ImageRead, scan_.offset, nowMs);
      return;
    }
    scanBytes(scan_, tx_, n, st_.image);
  }
  setPercent(2u * scan_.offset / size);
  if (scan_.offset < size) return;
  finishScan(scan_, st_.image);
  if (!opt_.force && !st_.image.hasHandshake) {
    fail(FlashError::ImageNoHandshake, 0, nowMs);
    return;
  }
  // The board check comes before anything touches the STM.
  st_.board = checkBoard(st_.image.hwTag, st_.boardHw);
  if (!opt_.force && (st_.image.hwConflict || st_.board == BoardCheck::Mismatch)) {
    fail(FlashError::BoardMismatch, 0, nowMs);
    return;
  }
  if (!opt_.force && st_.board == BoardCheck::BoardRequired) {
    fail(FlashError::BoardRequired, 0, nowMs);
    return;
  }
  enter(FlashPhase::Resetting, nowMs);
}

// NRST pulse for Resetting (UART 8E1: at the handshake baud in normal mode,
// at opt.baud in blank mode), Starting and the failure cleanup (both UART 8N1
// at the application baud).
void StmFlasher::stepPulse(uint32_t nowMs) {
  const bool boot = !cleanup_ && st_.phase == FlashPhase::Resetting;
  if (sub_ == 0) {
    t_.setReset(true);
    if (boot) {
      t_.configure(opt_.blank ? sessionBaud_ : kHandshakeBaud, true);
    } else {
      t_.configure(kAppBaud, false);
    }
    t_.discardInput();
    rxLen_ = 0;
    touched_ = true;
    waitStartMs_ = nowMs;
    sub_ = 1;
    if (boot) setPercent(2);
    return;
  }
  if (elapsedMs(nowMs, waitStartMs_) < opt_.resetPulseMs) return;
  t_.setReset(false);
  releaseMs_ = nowMs;
  if (cleanup_) {
    finish(FlashPhase::Failed, nowMs);
  } else if (boot) {
    enter(opt_.blank ? FlashPhase::Sync : FlashPhase::Handshake, nowMs);
    setPercent(opt_.blank ? 4 : 3);
  } else {
    // The application runs at 8N1 now: a later failure needs no new pulse.
    touched_ = false;
    lineDrop_ = false;
    enter(FlashPhase::WaitingApp, nowMs);
    setPercent(97);
  }
}

void StmFlasher::stepHandshake(uint32_t nowMs) {
  const uint32_t elapsed = elapsedMs(nowMs, releaseMs_);
  if (sub_ == 0) {
    // Reset glitches and stale bytes until the first DEADBEEF are no answer.
    t_.discardInput();
    rxLen_ = 0;
    if (elapsed < opt_.handshakeFirstMs) return;
    if (elapsed >= opt_.handshakeWindowMs) {
      fail(FlashError::HandshakeTimeout, 0, nowMs);
      return;
    }
    if (!put(kHandshake, sizeof kHandshake, nowMs)) return;
    lastSendMs_ = nowMs;
    sub_ = 1;
    return;
  }
  pump();
  for (size_t i = 0; i + sizeof kBeefit <= rxLen_; ++i) {
    if (memcmp(rx_ + i, kBeefit, sizeof kBeefit) == 0) {
      rxLen_ = 0;
      // The ROM bootloader autobauds on 0x7F, so it may run slower than the
      // boot window.
      if (sessionBaud_ != kHandshakeBaud) t_.configure(sessionBaud_, true);
      enter(FlashPhase::Sync, nowMs);
      setPercent(4);
      return;
    }
  }
  // Keep a possible "BEEFI" prefix for the next read.
  const size_t keep = sizeof kBeefit - 1;
  if (rxLen_ > keep) {
    memmove(rx_, rx_ + rxLen_ - keep, keep);
    rxLen_ = keep;
  }
  if (elapsed >= opt_.handshakeWindowMs) {
    fail(FlashError::HandshakeTimeout, 0, nowMs);
    return;
  }
  if (elapsedMs(nowMs, lastSendMs_) >= opt_.handshakeRepeatMs) {
    if (!put(kHandshake, sizeof kHandshake, nowMs)) return;
    lastSendMs_ = nowMs;
  }
}

void StmFlasher::stepSync(uint32_t nowMs) {
  static const uint8_t kFrame[] = {kSync};
  const uint8_t attempts = opt_.syncAttempts > 0 ? opt_.syncAttempts : 1;
  if (sub_ == 0) {
    if (elapsedMs(nowMs, phaseStartMs_) < opt_.afterBeefitMs) return;
    syncTries_ = 1;
    if (!send(kFrame, sizeof kFrame, nowMs, opt_.ackTimeoutMs)) return;
    sub_ = 1;
    return;
  }
  const Resp r = waitReply(nowMs, 1);
  if (r == Resp::Pending) return;
  if (r != Resp::Timeout) {  // ACK, or NACK = already synced
    enter(FlashPhase::GetId, nowMs);
    setPercent(5);
    return;
  }
  if (syncTries_ >= attempts) {
    if (!fallbackSession(nowMs)) fail(FlashError::SyncFailed, 0, nowMs);
    return;
  }
  ++syncTries_;
  send(kFrame, sizeof kFrame, nowMs, opt_.ackTimeoutMs);
}

// sub 0/1: GET (bootloader version, not fatal); sub 2/3: GET ID.
// Both replies are ACK, N, N+1 bytes, ACK.
void StmFlasher::stepGetId(uint32_t nowMs) {
  if (sub_ == 0 || sub_ == 2) {
    const uint8_t cmd = sub_ == 0 ? kCmdGet : kCmdGetId;
    const uint8_t frame[] = {cmd, static_cast<uint8_t>(cmd ^ 0xFF)};
    if (!send(frame, sizeof frame, nowMs, opt_.ackTimeoutMs)) return;
    ++sub_;
    return;
  }
  size_t need = 2;
  Resp r = waitReply(nowMs, need);
  if (r == Resp::Ack) {
    need = static_cast<size_t>(rx_[1]) + 4;
    r = waitReply(nowMs, need);
  }
  if (r == Resp::Pending) return;
  if (r == Resp::Ack && rx_[need - 1] != kAck) r = Resp::Nack;
  if (sub_ == 1) {
    if (r == Resp::Ack) st_.bootloaderVersion = rx_[2];
    sub_ = 2;
    return;
  }
  if (r != Resp::Ack) {
    if (retries_ < opt_.blockRetries) {
      ++retries_;
      sub_ = 2;
      return;
    }
    // A silent GetId (not a NACK) may be a bootloader at another baud.
    if (r == Resp::Nack || !fallbackSession(nowMs)) {
      fail(r == Resp::Nack ? FlashError::Nack : FlashError::Timeout, 0, nowMs);
    }
    return;
  }
  const uint16_t pid =
      rx_[1] >= 1 ? static_cast<uint16_t>((rx_[2] << 8) | rx_[3]) : static_cast<uint16_t>(rx_[2]);
  st_.chipPid = pid;
  const FlashError e = checkChip(st_.image, pid);
  if (e != FlashError::None) {
    fail(e, 0, nowMs);
    return;
  }
  enter(FlashPhase::Erasing, nowMs);
}

// 0x44 with the sector list: N-1 (2 bytes), sector numbers (2 bytes each),
// XOR of all of them.
void StmFlasher::stepErasing(uint32_t nowMs) {
  if (sub_ == 0) {
    static const uint8_t kFrame[] = {kCmdExtErase, kCmdExtErase ^ 0xFF};
    if (!send(kFrame, sizeof kFrame, nowMs, opt_.ackTimeoutMs)) return;
    sub_ = 1;
    return;
  }
  const Resp r = waitReply(nowMs, 1);
  if (r == Resp::Pending) return;
  if (r != Resp::Ack) {
    opFailed(r == Resp::Nack ? FlashError::Nack : FlashError::Timeout, kFlashBase, nowMs);
    return;
  }
  if (sub_ == 1) {
    const uint8_t n = sectorsForImage(st_.image.paddedSize);
    uint8_t frame[2 + 2 * 8 + 1];
    size_t len = 0;
    frame[len++] = 0;
    frame[len++] = static_cast<uint8_t>(n - 1);
    for (uint8_t s = 0; s < n; ++s) {
      frame[len++] = 0;
      frame[len++] = s;
    }
    uint8_t cs = 0;
    for (size_t i = 0; i < len; ++i) cs ^= frame[i];
    frame[len++] = cs;
    if (!send(frame, len, nowMs, opt_.eraseTimeoutMs)) return;
    sub_ = 2;
    return;
  }
  setPercent(15);
  st_.bytesDone = 0;
  block_ = 0;
  enter(FlashPhase::Writing, nowMs);
}

// Blocks 1..n-1, then block 0 (the vector table) last.
void StmFlasher::stepWriting(uint32_t nowMs) {
  const uint32_t count = blockCount();
  const uint32_t block = block_ + 1 < count ? block_ + 1 : 0;
  const uint32_t addr = kFlashBase + block * kBlockSize;
  if (sub_ == 0) {
    static const uint8_t kFrame[] = {kCmdWrite, kCmdWrite ^ 0xFF};
    if (!loadBlock(block)) {
      fail(FlashError::ImageRead, addr, nowMs);
      return;
    }
    if (!send(kFrame, sizeof kFrame, nowMs, opt_.ackTimeoutMs)) return;
    sub_ = 1;
    return;
  }
  const Resp r = waitReply(nowMs, 1);
  if (r == Resp::Pending) return;
  if (r != Resp::Ack) {
    opFailed(r == Resp::Nack ? FlashError::Nack : FlashError::Timeout, addr, nowMs);
    return;
  }
  if (sub_ == 1) {
    uint8_t a[5];
    addressFrame(addr, a);
    if (!send(a, sizeof a, nowMs, opt_.ackTimeoutMs)) return;
    sub_ = 2;
    return;
  }
  const uint32_t len = blockLen(block);
  if (sub_ == 2) {
    if (!send(tx_, len + 2, nowMs, opt_.ackTimeoutMs)) return;
    sub_ = 3;
    return;
  }
  st_.bytesDone += len;
  setPercent(15u + 60u * st_.bytesDone / st_.bytesTotal);
  ++block_;
  retries_ = 0;
  sub_ = 0;
  if (block_ >= count) {
    st_.bytesDone = 0;
    block_ = 0;
    verifyCrc_ = 0xFFFFFFFFu;
    enter(FlashPhase::Verifying, nowMs);
    setPercent(75);
  }
}

void StmFlasher::stepVerifying(uint32_t nowMs) {
  const uint32_t block = block_;
  const uint32_t addr = kFlashBase + block * kBlockSize;
  const uint32_t len = blockLen(block);
  if (sub_ == 0) {
    static const uint8_t kFrame[] = {kCmdRead, kCmdRead ^ 0xFF};
    if (!loadBlock(block)) {
      fail(FlashError::ImageRead, addr, nowMs);
      return;
    }
    if (!send(kFrame, sizeof kFrame, nowMs, opt_.ackTimeoutMs)) return;
    sub_ = 1;
    return;
  }
  const Resp r = waitReply(nowMs, sub_ == 3 ? len + 1 : 1);
  if (r == Resp::Pending) return;
  if (r != Resp::Ack) {
    opFailed(r == Resp::Nack ? FlashError::Nack : FlashError::Timeout, addr, nowMs);
    return;
  }
  if (sub_ == 1) {
    uint8_t a[5];
    addressFrame(addr, a);
    if (!send(a, sizeof a, nowMs, opt_.ackTimeoutMs)) return;
    sub_ = 2;
    return;
  }
  if (sub_ == 2) {
    const uint8_t n[2] = {static_cast<uint8_t>(len - 1), static_cast<uint8_t>((len - 1) ^ 0xFF)};
    // ACK + N+1 data bytes at 11 bits each, plus 200 ms (spec 02 R5).
    const uint32_t dataMs = wireMs(len + 1, sessionBaud_) + 200u;
    if (!send(n, sizeof n, nowMs, opt_.ackTimeoutMs + dataMs)) return;
    sub_ = 3;
    return;
  }
  for (uint32_t i = 0; i < len; ++i) {
    if (rx_[1 + i] != tx_[1 + i]) {
      opFailed(FlashError::VerifyMismatch, addr + i, nowMs);
      return;
    }
  }
  const uint32_t off = block * kBlockSize;
  const uint32_t size = st_.image.size;
  const uint32_t imageBytes = off >= size ? 0 : (size - off < len ? size - off : len);
  verifyCrc_ = crc32Update(verifyCrc_, tx_ + 1, imageBytes);
  st_.bytesDone += len;
  setPercent(75u + 20u * st_.bytesDone / st_.bytesTotal);
  ++block_;
  retries_ = 0;
  sub_ = 0;
  if (block_ < blockCount()) return;
  if (~verifyCrc_ != st_.image.crc) {
    fail(FlashError::ImageRead, 0, nowMs);  // the file changed during the run
    return;
  }
  if (opt_.blank) {
    // BOOT0 is still set: a reset would start the ROM bootloader again. The
    // user removes the jumper and resets the STM.
    t_.configure(kAppBaud, false);
    st_.manualReset = true;
    finish(FlashPhase::Done, nowMs);
    return;
  }
  enter(FlashPhase::Starting, nowMs);
  setPercent(95);
}

// One more session at fallbackBaud: new NRST pulse, then the handshake at
// 115200 (normal mode) or 0x7F at the fallback baud (blank mode).
bool StmFlasher::fallbackSession(uint32_t nowMs) {
  if (opt_.fallbackBaud == 0 || sessionBaud_ == opt_.fallbackBaud) return false;
  sessionBaud_ = opt_.fallbackBaud;
  st_.baud = sessionBaud_;
  syncTries_ = 0;
  enter(FlashPhase::Resetting, nowMs);
  return true;
}

void StmFlasher::stepWaitingApp(uint32_t nowMs) {
  uint8_t buf[64];
  size_t budget = kAppReadPerStep;
  while (budget > 0 && active()) {
    const size_t want = budget < sizeof buf ? budget : sizeof buf;
    size_t got = t_.read(buf, want);
    if (got == 0) break;
    if (got > want) got = want;
    budget -= got;
    for (size_t i = 0; i < got && active(); ++i) {
      const uint8_t c = buf[i];
      if (c == '\r' || c == '\n') {
        if (!lineDrop_ && rxLen_ > 0) onAppLine(nowMs);
        rxLen_ = 0;
        lineDrop_ = false;
      } else if (c >= 0x20 && c <= 0x7E && rxLen_ < sizeof rx_) {
        rx_[rxLen_++] = c;
      } else {
        lineDrop_ = true;  // noise or an over-long line: drop up to CR/LF
      }
    }
  }
  if (!active()) return;
  const uint32_t elapsed = elapsedMs(nowMs, releaseMs_);
  if (elapsed >= opt_.appTimeoutMs) {
    fail(FlashError::AppNotResponding, 0, nowMs);
    return;
  }
  if (elapsed < opt_.appBootMs) return;
  if (sub_ != 0 && elapsedMs(nowMs, lastSendMs_) < opt_.appPollMs) return;
  RequestLine req;
  buildGetVersion(req);
  if (sub_ == 0) {
    t_.discardInput();  // boot-window noise
    rxLen_ = 0;
    lineDrop_ = false;
  }
  if (!put(reinterpret_cast<const uint8_t*>(req.text), req.len, nowMs)) return;
  lastSendMs_ = nowMs;
  sub_ = 1;
  setPercent(98);
}

// "gvers <version> [<build>] ": the new application answers.
void StmFlasher::onAppLine(uint32_t nowMs) {
  const char* s = reinterpret_cast<const char*>(rx_);
  const size_t n = rxLen_;
  if (n < 6 || memcmp(s, "gvers ", 6) != 0) return;
  size_t p = 6;
  while (p < n && s[p] == ' ') ++p;
  size_t q = p;
  while (q < n && s[q] != ' ') ++q;
  Version app;
  if (!parseVersion(s + p, q - p, app)) return;
  st_.appVersion = app;
  if (!opt_.force && st_.image.version[0] != '\0') {
    Version image;
    parseVersion(st_.image.version, strlen(st_.image.version), image);
    if (!sameVersion(image, app)) {
      fail(FlashError::AppVersionMismatch, 0, nowMs);
      return;
    }
  }
  if (!opt_.force && st_.image.hwTag[0] != '\0' && app.hw[0] != '\0' &&
      strcmp(st_.image.hwTag, app.hw) != 0) {
    fail(FlashError::AppVersionMismatch, 0, nowMs);
    return;
  }
  finish(FlashPhase::Done, nowMs);
}

}  // namespace vdm
