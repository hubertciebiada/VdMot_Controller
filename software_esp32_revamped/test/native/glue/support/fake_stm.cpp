#include "fake_stm.h"

#include <string.h>

#include <vdm/stm_codec.h>

#include "stm_golden.h"

namespace glue {

namespace {

constexpr int kNrstPin = 15;  // src/board.h kStmResetPin

std::vector<std::string> split(const std::string& line) {
  std::vector<std::string> out;
  size_t i = 0;
  while (i < line.size()) {
    while (i < line.size() && line[i] == ' ') ++i;
    const size_t start = i;
    while (i < line.size() && line[i] != ' ') ++i;
    if (i > start) out.push_back(line.substr(start, i - start));
  }
  return out;
}

// First golden line of `cmd` that is not an error form.
std::string golden(const std::string& cmd) {
  for (size_t i = 0; i < vdm_test::kStmGoldenCount; ++i) {
    const std::string g = vdm_test::kStmGolden[i];
    if (g.compare(0, cmd.size(), cmd) != 0 || (g.size() > cmd.size() && g[cmd.size()] != ' ')) {
      continue;
    }
    if (g.find(" err") != std::string::npos || g.find(" error") != std::string::npos) continue;
    return g;
  }
  return "";
}

// The golden line with its first field (the valve) replaced.
std::string withValve(const std::string& line, const std::string& valve) {
  const size_t a = line.find(' ');
  if (a == std::string::npos) return line;
  const size_t b = line.find(' ', a + 1);
  return line.substr(0, a + 1) + valve + (b == std::string::npos ? "" : line.substr(b));
}

}  // namespace

FakeStm::FakeStm() {
  protocol(2);
  bootAtMs_ = fakes::nowMs();
  fakes::serial(2).peer = this;
  previousOnWrite_ = fakes::gpio().onWrite;
  fakes::gpio().onWrite = [this](int pin, uint8_t level) {
    if (previousOnWrite_) previousOnWrite_(pin, level);
    if (pin == kNrstPin) onNrst(level);
  };
}

FakeStm::~FakeStm() {
  if (fakes::serial(2).peer == this) fakes::serial(2).peer = nullptr;
  fakes::gpio().onWrite = previousOnWrite_;
}

void FakeStm::protocol(uint8_t p) {
  protocol_ = p;
  if (p <= 1) {
    version_ = "1.4.9_Dev_C2 1712345678 ";
  } else if (p == 2) {
    version_ = "2.0.0-revamped_C2 1712345678 ";
  } else {
    version_ = "2.1.0-revamped_C2 1712345678 ";
  }
}

void FakeStm::version(const std::string& text) { version_ = text; }
void FakeStm::silent(bool on) { silent_ = on; }
void FakeStm::tooOld(bool on) { tooOld_ = on; }
void FakeStm::eepState(uint8_t n) { eep_ = n; }

void FakeStm::answer(const std::string& cmd, std::function<std::string(const std::string&)> fn) {
  answers_[cmd] = std::move(fn);
}

void FakeStm::unsolicited(const std::string& line, uint32_t delayMs) {
  fakes::serial(2).inject(line + "\r\n", fakes::nowMs() + delayMs);
}

void FakeStm::useSimulator(bool on) {
  if (on) sim();
  useSim_ = on;
}

vdm_test::SimStm& FakeStm::sim() {
  if (!sim_) sim_.reset(new vdm_test::SimStm(fakes::ms32()));
  return *sim_;
}

std::vector<std::string> FakeStm::requestsOf(const std::string& cmd) const {
  std::vector<std::string> out;
  for (const Request& r : requests_) {
    const std::vector<std::string> t = split(r.line);
    if (!t.empty() && t[0] == cmd) out.push_back(r.line);
  }
  return out;
}

void FakeStm::onBegin(uint32_t baud, uint32_t config) {
  if (sim_) sim_->configure(baud, config == SERIAL_8E1);
}

void FakeStm::onTx(const uint8_t* data, size_t len) {
  if (useSim_) {
    sim_->write(data, len);
    return;
  }
  for (size_t i = 0; i < len; ++i) {
    const char c = static_cast<char>(data[i]);
    if (c == '\n') {
      onLine(rxLine_);
      rxLine_.clear();
    } else if (c != '\r') {
      rxLine_.push_back(c);
    }
  }
}

void FakeStm::poll() {
  if (!useSim_) return;
  uint8_t buf[256];
  for (;;) {
    const size_t n = sim_->read(buf, sizeof buf);
    if (n == 0) break;
    fakes::serial(2).inject(std::string(reinterpret_cast<const char*>(buf), n));
  }
}

void FakeStm::onNrst(uint8_t level) {
  if (sim_) sim_->setReset(level == HIGH);
  if (level == HIGH) {
    held_ = true;
    rxLine_.clear();
    return;
  }
  if (held_) {
    held_ = false;
    ++resets_;
    bootUntilMs_ = fakes::nowMs() + bootMs;
    bootAtMs_ = fakes::nowMs();
    uptimeBaseS_ = 0;
  }
}

void FakeStm::onLine(const std::string& line) {
  requests_.push_back({line, fakes::nowMs()});
  if (held_ || silent_ || fakes::nowMs() < bootUntilMs_) return;
  const std::vector<std::string> t = split(line);
  if (t.empty()) return;
  const std::vector<std::string> args(t.begin() + 1, t.end());
  const std::string out = reply(t[0], args, line);
  if (out.empty()) return;
  fakes::serial(2).inject(out + "\r\n", fakes::nowMs() + replyDelayMs);
}

std::string FakeStm::reply(const std::string& cmd, const std::vector<std::string>& args,
                           const std::string& line) {
  auto it = answers_.find(cmd);
  if (it != answers_.end()) return it->second(line);
  const std::string valve = args.empty() ? "0" : args[0];
  if (tooOld_) {
    if (cmd == "gvers") return "gvers 1.3.5_C2";
    if (cmd == "stgtp") return "stgtp";
    if (cmd == "gtgtp") return "gtgtp " + valve + " 50 ";
    return "";
  }
  const vdm::Cmd c = vdm::cmdFromName(cmd.c_str(), cmd.size());
  if (c == vdm::Cmd::None || vdm::cmdMinProtocol(c) > protocol_) return "";
  if (cmd == "gvers") return "gvers " + version_;
  if (cmd == "gproto") return "gproto " + std::to_string(protocol_);
  if (cmd == "eepst") return "eepst " + std::to_string(eep_) + " ";
  if (cmd == "gstat" || cmd == "gstax") {
    const uint64_t up = uptimeBaseS_ + (fakes::nowMs() - bootAtMs_) / 1000;
    return withValve(golden(cmd), std::to_string(up));
  }
  if (cmd == "slcfg") {
    if (!args.empty()) leaseTimeout_ = static_cast<uint32_t>(std::stoul(args[0]));
    return "slcfg ok";
  }
  if (cmd == "sfspo" && args.size() == 2) {
    const uint32_t pct = static_cast<uint32_t>(std::stoul(args[1]));
    for (int v = 0; v < 12; ++v) {
      if (valve == "255" || valve == std::to_string(v)) failsafe_[v] = pct;
    }
  }
  if (cmd == "glcfg") {
    std::string out = "glcfg " + std::to_string(leaseTimeout_);
    for (uint32_t p : failsafe_) out += " " + std::to_string(p);
    return out;
  }
  if (cmd == "stlnt") {
    if (!args.empty()) learnTime_ = static_cast<uint32_t>(std::stoul(args[0]));
    return "stlnt";
  }
  if (cmd == "gtlnt") return "gtlnt " + std::to_string(learnTime_);
  if (cmd == "gonec" || cmd == "gowvc") return cmd + " 0 ";
  if (cmd == "gvlon") {
    const std::string zero = "00-00-00-00-00-00-00-00";
    if (valve == "255") {
      std::string ids;
      for (int i = 0; i < 2 * vdm::kValveCount; ++i) ids += (i == 0 ? "" : ",") + zero;
      return "gvlon 12 " + ids;
    }
    return "gvlon " + valve + " " + zero + " " + zero + " ";
  }
  if (cmd == "svmov" || cmd == "sfspo" || cmd == "sstop") return cmd + " " + valve + " ok";
  if (cmd == "stvls") return "stvls " + valve;
  if (cmd == "gvlvd" || cmd == "gvlvx" || cmd == "gvlvy" || cmd == "gtgtp" || cmd == "gprof") {
    return withValve(golden(cmd), valve);
  }
  const std::string g = golden(cmd);
  return g.empty() ? cmd : g;  // the bare acknowledgements (stons, staln, reset, ...)
}

}  // namespace glue
