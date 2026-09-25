// AN3155 STM simulator for the flasher: v1 boot window (DEADBEEF/BEEFIT),
// ROM bootloader with a flash model and an application answering gvers.
// Shared by the core flasher tests and the glue harness (the fake STM in
// bootloader mode). Header-only: every test executable includes it.
#pragma once

#include <stdint.h>
#include <string.h>

#include <deque>
#include <set>
#include <string>
#include <utility>
#include <vector>

#include "vdm/stm_flasher.h"

namespace vdm_test {

constexpr uint8_t ACK = 0x79;
constexpr uint8_t NACK = 0x1F;
constexpr uint32_t kBase = 0x08000000u;
constexpr uint32_t kKiB = 1024u;

struct Lcg {
  uint32_t s;
  explicit Lcg(uint32_t seed) : s(seed) {}
  uint32_t next() {
    s = s * 1664525u + 1013904223u;
    return s >> 8;
  }
  uint32_t below(uint32_t n) { return n == 0 ? 0 : next() % n; }
};

struct WriteRec {
  uint32_t addr;
  uint32_t len;
};

class SimStm : public vdm::FlashTransport {
 public:
  explicit SimStm(const uint32_t& now) : flash(512 * kKiB), now_(now) {
    Lcg r(99);
    for (auto& b : flash) b = static_cast<uint8_t>(r.next());
    original = flash;
  }

  // ---- behaviour knobs
  bool hasBootLoop = true;          // running app answers DEADBEEF (v1 fixed chunks)
  uint32_t windowMs = 3000;
  uint32_t windowReadyMs = 12;      // bytes before this after release are lost
  int stray = 0;                    // garbage bytes in the window chunk at boot
  std::string beefitPrefix;         // noise before "BEEFIT"
  std::string bootNoise;            // sent 1 ms after every reset release
  uint32_t replySpacingMs = 0;      // per-byte delay of replies (split reads)
  int bootPinResets = 0;            // next N releases boot into the ROM bootloader
  std::string appReply = "gvers 1.4.9_Dev_C1 1 ";
  bool appAnswers = true;
  std::string appNoise;             // sent before each gvers reply
  uint16_t pid = 0x431;
  uint8_t pidN = 1;
  uint8_t blVersion = 0x31;
  bool getSilent = false;
  int getIdSilent = 0;
  int getIdBadEnd = 0;
  int syncSilent = 0;
  bool syncNack = false;            // bootloader already synced
  bool rdp = false;
  int nackErase = 0;
  int dropEraseAck = 0;
  uint32_t eraseDelayMs = 300;
  int nackWriteData = 0;
  int dropWriteAck = 0;
  std::set<uint32_t> nackWriteAddr;  // NACK every write to this block address
  int corruptReads = 0;
  uint32_t corruptOffset = 5;
  std::set<uint32_t> stuck;          // flash addresses that program wrong
  int noiseReplies = 0;              // next N replies get a 0x55 byte in front
  bool instantReplies = false;       // bootloader answers within the same millisecond
  uint32_t bootMaxBaud = 115200;     // the ROM bootloader ignores bytes sent faster than this
  std::multiset<uint32_t> corruptReadAt;  // one corrupted read-back per entry (block address)
  std::deque<std::string> echoes;    // sent once each, on every 8E1 '\n' received
  std::string echoRepeat;            // sent on every 8E1 '\n' once `echoes` is empty
  size_t writeLimit = SIZE_MAX;

  // ---- observation
  std::vector<uint8_t> flash, original;
  std::vector<std::pair<uint32_t, bool>> configs;
  std::vector<std::pair<uint32_t, bool>> resets;  // (time, asserted)
  std::vector<std::pair<uint32_t, std::vector<uint8_t>>> writes;
  std::vector<WriteRec> programmed;
  std::vector<uint32_t> readAddrs;
  std::vector<std::vector<uint8_t>> eraseFrames;
  std::vector<uint8_t> commands;
  std::vector<uint32_t> handshakeTimes;
  std::vector<uint32_t> gversTimes;
  std::vector<uint32_t> syncTimes;
  uint32_t beefitAt = 0;
  bool inReset = false;
  bool even = false;
  uint32_t baud = 0;

  void configure(uint32_t b, bool e) override {
    baud = b;
    even = e;
    configs.emplace_back(b, e);
  }
  size_t write(const uint8_t* data, size_t len) override {
    advance();
    const size_t n = len < writeLimit ? len : writeLimit;
    writes.emplace_back(now_, std::vector<uint8_t>(data, data + n));
    for (size_t i = 0; i < n; ++i) receive(data[i]);
    return n;
  }
  size_t read(uint8_t* out, size_t cap) override {
    advance();
    size_t n = 0;
    while (n < cap && !out_.empty() && out_.front().first <= now_) {
      out[n++] = out_.front().second;
      out_.pop_front();
    }
    return n;
  }
  void discardInput() override {
    advance();
    while (!out_.empty() && out_.front().first <= now_) out_.pop_front();
  }
  void setReset(bool asserted) override {
    resets.emplace_back(now_, asserted);
    if (asserted) {
      inReset = true;
      mode_ = Mode::Reset;
      out_.clear();
      return;
    }
    if (!inReset) return;
    inReset = false;
    bootAt_ = now_;
    chunk_.clear();
    line_.clear();
    if (!bootNoise.empty()) replyText(bootNoise, 1);
    if (bootPinResets > 0) {
      --bootPinResets;
      enterBootloader();
    } else if (hasBootLoop) {
      mode_ = Mode::Window;
      for (int i = 0; i < stray; ++i) chunk_.push_back('x');
    } else {
      mode_ = Mode::App;
      appReadyAt_ = now_ + 600;
    }
  }

  bool inBootloader() const { return mode_ == Mode::Boot; }
  bool inApp() { advance(); return mode_ == Mode::App; }
  size_t writesEqual(const std::vector<uint8_t>& frame) const {
    size_t n = 0;
    for (auto& w : writes) n += w.second == frame ? 1 : 0;
    return n;
  }

 private:
  enum class Mode { App, Reset, Window, Jumping, Boot };
  enum class Bs { Unsynced, Idle, Cmd2, Addr, WrN, WrData, EraseHdr, EraseList, ReadN };

  const uint32_t& now_;
  Mode mode_ = Mode::App;
  uint32_t bootAt_ = 0;
  uint32_t jumpAt_ = 0;
  uint32_t appReadyAt_ = 0;
  std::string chunk_;
  std::string line_;
  std::deque<std::pair<uint32_t, uint8_t>> out_;
  Bs bs_ = Bs::Unsynced;
  uint8_t cmd_ = 0;
  uint8_t next_ = 0;
  std::vector<uint8_t> buf_;
  size_t need_ = 0;
  uint32_t addr_ = 0;
  uint8_t n_ = 0;

  void enterBootloader() {
    mode_ = Mode::Boot;
    bs_ = syncNack ? Bs::Idle : Bs::Unsynced;
  }

  void advance() {
    if (mode_ == Mode::Window && now_ - bootAt_ >= windowMs) {
      mode_ = Mode::App;
      appReadyAt_ = now_ + 600;
    }
    if (mode_ == Mode::Jumping && now_ - jumpAt_ < 0x80000000u) enterBootloader();
  }

  void reply(const std::vector<uint8_t>& bytes, uint32_t delay = 1) {
    if (instantReplies && mode_ == Mode::Boot) delay = 0;
    uint32_t t = now_ + delay;
    if (noiseReplies > 0 && mode_ == Mode::Boot) {
      --noiseReplies;
      out_.emplace_back(t, 0x55);
    }
    for (uint8_t b : bytes) {
      out_.emplace_back(t, b);
      t += replySpacingMs;
    }
  }
  void replyText(const std::string& s, uint32_t delay) {
    reply(std::vector<uint8_t>(s.begin(), s.end()), delay);
  }

  void receive(uint8_t b) {
    if (b == '\n' && even) {
      if (!echoes.empty()) {
        replyText(echoes.front(), 1);
        echoes.pop_front();
      } else if (!echoRepeat.empty()) {
        replyText(echoRepeat, 1);
      }
    }
    switch (mode_) {
      case Mode::Reset:
      case Mode::Jumping: return;
      case Mode::Window: return windowByte(b);
      case Mode::App: return appByte(b);
      case Mode::Boot: return bootByte(b);
    }
  }

  void windowByte(uint8_t b) {
    if (!even || baud != 115200 || now_ - bootAt_ < windowReadyMs) return;
    handshakeTimes.push_back(now_);
    chunk_.push_back(static_cast<char>(b));
    if (chunk_.size() < 8) return;
    const bool match = chunk_ == "DEADBEEF";
    chunk_.clear();
    if (!match) return;
    beefitAt = now_ + 10;
    replyText(beefitPrefix + "BEEFIT\r\n", 10);
    mode_ = Mode::Jumping;
    jumpAt_ = now_ + 210;
  }

  void appByte(uint8_t b) {
    if (even) return;  // 8E1 bytes are garbage for the 8N1 application
    if (b != '\r' && b != '\n') {
      line_.push_back(static_cast<char>(b));
      return;
    }
    if (line_ == "gvers ") {
      gversTimes.push_back(now_);
      if (appAnswers && now_ >= appReadyAt_) replyText(appNoise + appReply + "\r\n", 3);
    }
    line_.clear();
  }

  void bootByte(uint8_t b) {
    if (!even || baud > bootMaxBaud) return;
    switch (bs_) {
      case Bs::Unsynced:
        if (b != 0x7F) return;
        syncTimes.push_back(now_);
        if (syncSilent > 0) {
          --syncSilent;
          return;
        }
        reply({ACK});
        bs_ = Bs::Idle;
        return;
      case Bs::Idle:
        if (b == 0x7F) syncTimes.push_back(now_);
        cmd_ = b;
        bs_ = Bs::Cmd2;
        return;
      case Bs::Cmd2: return command(b);
      case Bs::Addr: return addrByte(b);
      case Bs::WrN:
        n_ = b;
        buf_.clear();
        need_ = static_cast<size_t>(n_) + 2;
        bs_ = Bs::WrData;
        return;
      case Bs::WrData:
        buf_.push_back(b);
        if (buf_.size() == need_) writeDone();
        return;
      case Bs::EraseHdr:
        buf_.push_back(b);
        if (buf_.size() == 2) {
          const uint16_t nm1 = static_cast<uint16_t>((buf_[0] << 8) | buf_[1]);
          need_ = nm1 >= 0xFFF0 ? 3 : 2 + 2 * (static_cast<size_t>(nm1) + 1) + 1;
          bs_ = Bs::EraseList;
        }
        return;
      case Bs::EraseList:
        buf_.push_back(b);
        if (buf_.size() == need_) eraseDone();
        return;
      case Bs::ReadN:
        buf_.push_back(b);
        if (buf_.size() == 2) readDone();
        return;
    }
  }

  void command(uint8_t b) {
    bs_ = Bs::Idle;
    if (b != static_cast<uint8_t>(cmd_ ^ 0xFF)) {
      reply({NACK});
      return;
    }
    commands.push_back(cmd_);
    if (rdp && (cmd_ == 0x31 || cmd_ == 0x11 || cmd_ == 0x44)) {
      reply({NACK});
      return;
    }
    switch (cmd_) {
      case 0x00:
        if (!getSilent) {
          reply({ACK, 11, blVersion, 0x00, 0x01, 0x02, 0x11, 0x21, 0x31, 0x44, 0x63, 0x73, 0x82,
                 0x92, ACK});
        }
        return;
      case 0x02: {
        if (getIdSilent > 0) {
          --getIdSilent;
          return;
        }
        std::vector<uint8_t> r = {ACK, pidN};
        if (pidN == 0) {
          r.push_back(static_cast<uint8_t>(pid));
        } else {
          r.push_back(static_cast<uint8_t>(pid >> 8));
          r.push_back(static_cast<uint8_t>(pid));
          for (int i = 1; i < pidN; ++i) r.push_back(0xAA);
        }
        if (getIdBadEnd > 0) {
          --getIdBadEnd;
          r.push_back(0x00);
        } else {
          r.push_back(ACK);
        }
        reply(r);
        return;
      }
      case 0x44:
        if (nackErase > 0) {
          --nackErase;
          reply({NACK});
          return;
        }
        reply({ACK});
        buf_.clear();
        bs_ = Bs::EraseHdr;
        return;
      case 0x31:
      case 0x11:
        reply({ACK});
        next_ = cmd_;
        buf_.clear();
        bs_ = Bs::Addr;
        return;
      default: reply({NACK});
    }
  }

  void addrByte(uint8_t b) {
    buf_.push_back(b);
    if (buf_.size() < 5) return;
    bs_ = Bs::Idle;
    if ((buf_[0] ^ buf_[1] ^ buf_[2] ^ buf_[3]) != buf_[4]) {
      reply({NACK});
      return;
    }
    addr_ = (static_cast<uint32_t>(buf_[0]) << 24) | (static_cast<uint32_t>(buf_[1]) << 16) |
            (static_cast<uint32_t>(buf_[2]) << 8) | buf_[3];
    if (addr_ < kBase || addr_ >= kBase + flash.size()) {
      reply({NACK});
      return;
    }
    reply({ACK});
    buf_.clear();
    bs_ = next_ == 0x31 ? Bs::WrN : Bs::ReadN;
  }

  void writeDone() {
    bs_ = Bs::Idle;
    const size_t len = static_cast<size_t>(n_) + 1;
    uint8_t cs = n_;
    for (size_t i = 0; i < len; ++i) cs ^= buf_[i];
    const uint32_t off = addr_ - kBase;
    if (cs != buf_[len] || len % 4 != 0 || addr_ % 4 != 0 || off + len > flash.size() ||
        nackWriteAddr.count(addr_) != 0) {
      reply({NACK});
      return;
    }
    if (nackWriteData > 0) {
      --nackWriteData;
      reply({NACK});
      return;
    }
    for (size_t i = 0; i < len; ++i) {
      uint8_t v = buf_[i];
      if (stuck.count(addr_ + static_cast<uint32_t>(i))) v ^= 0x01;
      flash[off + i] &= v;  // programming only clears bits
    }
    programmed.push_back({addr_, static_cast<uint32_t>(len)});
    if (dropWriteAck > 0) {
      --dropWriteAck;
      return;
    }
    reply({ACK}, 2);
  }

  void eraseDone() {
    bs_ = Bs::Idle;
    uint8_t cs = 0;
    for (size_t i = 0; i + 1 < buf_.size(); ++i) cs ^= buf_[i];
    eraseFrames.push_back(buf_);
    if (cs != buf_.back() || buf_.size() == 3) {
      reply({NACK});
      return;
    }
    static const uint32_t kSec[] = {16, 16, 16, 16, 64, 128, 128, 128};
    for (size_t i = 2; i + 1 < buf_.size(); i += 2) {
      const uint16_t s = static_cast<uint16_t>((buf_[i] << 8) | buf_[i + 1]);
      if (s >= 8) {
        reply({NACK});
        return;
      }
      uint32_t start = 0;
      for (uint16_t k = 0; k < s; ++k) start += kSec[k] * kKiB;
      memset(flash.data() + start, 0xFF, kSec[s] * kKiB);
    }
    if (dropEraseAck > 0) {
      --dropEraseAck;
      return;
    }
    reply({ACK}, eraseDelayMs);
  }

  void readDone() {
    bs_ = Bs::Idle;
    if (buf_[1] != static_cast<uint8_t>(buf_[0] ^ 0xFF)) {
      reply({NACK});
      return;
    }
    readAddrs.push_back(addr_);
    const size_t len = static_cast<size_t>(buf_[0]) + 1;
    const uint32_t off = addr_ - kBase;
    std::vector<uint8_t> r = {ACK};
    for (size_t i = 0; i < len; ++i) r.push_back(off + i < flash.size() ? flash[off + i] : 0);
    if (corruptReads > 0) {
      --corruptReads;
      r[1 + corruptOffset] ^= 0x40;
    }
    const auto once = corruptReadAt.find(addr_);
    if (once != corruptReadAt.end()) {
      corruptReadAt.erase(once);
      r[1 + corruptOffset] ^= 0x40;
    }
    reply(r, 2);
  }
};

}  // namespace vdm_test
