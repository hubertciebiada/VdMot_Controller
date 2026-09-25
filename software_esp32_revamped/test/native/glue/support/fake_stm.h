// The STM32 on the other end of Serial2 for the stm_link glue tests: answers the ESP-STM text
// protocol with the golden replies of test/native/support/stm_golden.h (the valve of a request
// substituted), watches NRST (GPIO 15: HIGH holds the STM in reset, the release boots it) and,
// for flash runs, hands every byte to the AN3155 simulator of test/native/support/sim_stm.h.
//
// Defaults: protocol 2, version "2.0.0-revamped_C2", no 1-Wire sensors, EEPROM idle (eepst 0),
// replies 3 ms after the request line. Commands above the protocol stay unanswered, like a v1 STM
// that ignores gproto.
#pragma once

#include <stddef.h>
#include <stdint.h>

#include <functional>
#include <map>
#include <memory>
#include <string>
#include <vector>

#include "fakes/fakes.h"
#include "sim_stm.h"

namespace glue {

class FakeStm : public fakes::SerialPeer {
 public:
  // Attaches to Serial2 and the NRST pin; detaches in the destructor.
  FakeStm();
  ~FakeStm() override;
  FakeStm(const FakeStm&) = delete;
  FakeStm& operator=(const FakeStm&) = delete;

  // ---- controls
  void protocol(uint8_t p);                // 1, 2 or 3; also picks the default version
  void version(const std::string& text);   // gvers reply without the command, e.g. "1.4.9_C2 17"
  void silent(bool on);                    // no replies at all
  // Firmware older than the minimum: answers only "gvers 1.3.5_C2", stgtp and gtgtp.
  void tooOld(bool on);
  void eepState(uint8_t n);                // eepst reply (0 idle)
  // Reply of one command: fn(request line without CR LF) -> reply line ("" = no reply).
  void answer(const std::string& cmd, std::function<std::string(const std::string&)> fn);
  // A line the STM sends on its own after `delayMs`.
  void unsolicited(const std::string& line, uint32_t delayMs = 0);
  uint32_t replyDelayMs = 3;
  uint32_t bootMs = 100;  // after an NRST release nothing is answered before this

  // Flash runs: every byte and the NRST line go to the AN3155 simulator (created on first use:
  // it fills a 512 KiB flash model).
  void useSimulator(bool on);
  vdm_test::SimStm& sim();

  // ---- observation
  struct Request {
    std::string line;  // without CR LF
    uint64_t atMs;
  };
  const std::vector<Request>& requests() const { return requests_; }
  // Request lines of one command, in order.
  std::vector<std::string> requestsOf(const std::string& cmd) const;
  int resets() const { return resets_; }  // NRST released after being held
  bool held() const { return held_; }

  // SerialPeer
  void onBegin(uint32_t baud, uint32_t config) override;
  void onTx(const uint8_t* data, size_t len) override;
  void poll() override;

 private:
  void onLine(const std::string& line);
  std::string reply(const std::string& cmd, const std::vector<std::string>& args,
                    const std::string& line) const;
  void onNrst(uint8_t level);

  uint8_t protocol_ = 2;
  std::string version_;
  bool silent_ = false;
  bool tooOld_ = false;
  uint8_t eep_ = 0;
  std::map<std::string, std::function<std::string(const std::string&)>> answers_;
  std::string rxLine_;
  std::vector<Request> requests_;
  bool held_ = false;
  int resets_ = 0;
  uint64_t bootUntilMs_ = 0;
  bool useSim_ = false;
  std::unique_ptr<vdm_test::SimStm> sim_;
  std::function<void(int, uint8_t)> previousOnWrite_;
};

}  // namespace glue
