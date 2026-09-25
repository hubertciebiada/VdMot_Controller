// A scripted STM at the text-line level for the StmSession tests: answers
// request lines (without CR LF) like the firmware of a protocol, keeps the
// targets, the assembly holds, the lease settings and the learn time it was
// given, and reports an uptime counted from its last reset. Header-only.
#pragma once

#include <stdint.h>

#include <functional>
#include <map>
#include <string>
#include <vector>

namespace vdm_test {

class LineStm {
 public:
  // ---- behaviour
  uint8_t protocol = 3;            // 1 (legacy: no gproto), 2 or 3
  std::string version;             // gvers reply without the command; "" = per protocol
  bool silent = false;             // no replies at all
  bool tooOld = false;             // answers only gvers 1.3.5_C2, stgtp and gtgtp
  uint8_t eep = 1;                 // eepst reply (1 idle)
  uint32_t uptimeBaseS = 100;      // uptime at the last reset
  uint32_t resets = 3;             // gstat/gstax reset counter
  uint8_t lease = 1;               // gstax/slhbt lease state
  uint32_t leaseTimeout = 60;
  uint32_t failsafe[12] = {50, 50, 50, 50, 50, 50, 50, 50, 50, 50, 50, 50};
  uint32_t learnTime = 604800;
  uint8_t target[12] = {50, 50, 50, 50, 50, 50, 50, 50, 50, 50, 50, 50};
  bool assembly[12] = {};
  // Reply of one command: fn(line) -> reply ("" = none).
  std::map<std::string, std::function<std::string(const std::string&)>> answers;

  // ---- observation
  std::vector<std::string> lines;

  void reset(uint64_t nowMs) {
    bootAtMs_ = nowMs;
    uptimeBaseS = 0;
    for (uint8_t& t : target) t = 50;
    for (bool& a : assembly) a = false;
  }

  std::vector<std::string> linesOf(const std::string& cmd) const {
    std::vector<std::string> out;
    for (const std::string& l : lines) {
      if (l.compare(0, cmd.size(), cmd) == 0 && (l.size() == cmd.size() || l[cmd.size()] == ' ')) {
        out.push_back(l);
      }
    }
    return out;
  }

  std::string answer(const std::string& line, uint64_t nowMs) {
    lines.push_back(line);
    if (silent) return "";
    std::vector<std::string> t;
    size_t i = 0;
    while (i < line.size()) {
      while (i < line.size() && line[i] == ' ') ++i;
      const size_t s = i;
      while (i < line.size() && line[i] != ' ') ++i;
      if (i > s) t.push_back(line.substr(s, i - s));
    }
    if (t.empty()) return "";
    const std::string& cmd = t[0];
    auto it = answers.find(cmd);
    if (it != answers.end()) return it->second(line);
    const int v = t.size() > 1 ? std::stoi(t[1]) : 0;
    const bool one = v >= 0 && v < 12;
    if (tooOld) {
      if (cmd == "gvers") return "gvers 1.3.5_C2";
      if (cmd == "stgtp" && one && t.size() > 2) target[v] = static_cast<uint8_t>(std::stoi(t[2]));
      if (cmd == "stgtp") return "stgtp";
      if (cmd == "gtgtp" && one) return "gtgtp " + t[1] + " " + std::to_string(target[v]) + " ";
      return "";
    }
    if (cmd == "gproto") return protocol >= 2 ? "gproto " + std::to_string(protocol) : "";
    if (cmd == "gvers") {
      if (!version.empty()) return "gvers " + version;
      if (protocol <= 1) return "gvers 1.4.9_Dev_C2 1712345678 ";
      return protocol == 2 ? "gvers 2.0.0-revamped_C2 1712345678 "
                           : "gvers 2.1.0-revamped_C2 1712345678 ";
    }
    const std::string up = std::to_string(uptimeBaseS + (nowMs - bootAtMs_) / 1000);
    if (cmd == "gstat" && protocol >= 2) return "gstat " + up + " " + std::to_string(resets) + " 2 0 0 1";
    if (cmd == "gstax" && protocol >= 3) {
      return "gstax " + up + " " + std::to_string(resets) + " 2 0 0 1 " + std::to_string(lease) +
             " 3540 1 " + std::to_string(leaseTimeout) + " 0 0 0 0 0 0 0 0 0 0 0 0 0";
    }
    if (cmd == "eepst") return "eepst " + std::to_string(eep) + " ";
    if (cmd == "ghwin") return "ghwin 1073 ";
    if (cmd == "gmotc") return "gmotc 17 17 50 3000 0 ";
    if (cmd == "gtlnm") return "gtlnm 2000 ";
    if (cmd == "gcalx" && protocol >= 2) return "gcalx 1 10 40";
    if (cmd == "gonec" || cmd == "gowvc") return cmd + " 0 ";
    if (cmd == "gvlst" ) return "gvlst 12 1,1,1,1,1,1,1,1,1,1,1,1, ";
    if (cmd == "gvlon") {
      const std::string z = "00-00-00-00-00-00-00-00";
      if (t.size() > 1 && t[1] == "255") {
        std::string ids;
        for (int k = 0; k < 24; ++k) ids += (k == 0 ? "" : ",") + z;
        return "gvlon 12 " + ids;
      }
      return "gvlon " + t[1] + " " + z + " " + z + " ";
    }
    if (cmd == "gtgtp" && one) return "gtgtp " + t[1] + " " + std::to_string(target[v]) + " ";
    if (cmd == "gvlvd" && one) return "gvlvd " + t[1] + " 42 18 1 215 -500 57 3120 3350 230 0 ";
    if (cmd == "gvlvx" && protocol >= 2 && one) return valveEx(v, false);
    if (cmd == "gvlvy" && protocol >= 3 && one) return valveEx(v, true);
    if (cmd == "stgtp" && one && t.size() > 2) {
      target[v] = static_cast<uint8_t>(std::stoi(t[2]));
      assembly[v] = false;
      return "stgtp";
    }
    if (cmd == "staop") {
      for (int k = 0; k < 12; ++k) {
        if (v == 255 || v == k) {
          target[k] = 100;
          assembly[k] = true;
        }
      }
      return "staop";
    }
    if (protocol >= 3) {
      if (cmd == "slhbt") return "slhbt " + std::to_string(lease) + " 3540";
      if (cmd == "slcfg") {
        leaseTimeout = static_cast<uint32_t>(std::stoul(t[1]));
        return "slcfg ok";
      }
      if (cmd == "sfspo") {
        for (int k = 0; k < 12; ++k) {
          if (v == 255 || v == k) failsafe[k] = static_cast<uint32_t>(std::stoul(t[2]));
        }
        return "sfspo " + t[1] + " ok";
      }
      if (cmd == "glcfg") {
        std::string out = "glcfg " + std::to_string(leaseTimeout);
        for (uint32_t p : failsafe) out += " " + std::to_string(p);
        return out;
      }
      if (cmd == "gtlnt") return "gtlnt " + std::to_string(learnTime);
      if (cmd == "sstop") return "sstop " + t[1] + " ok";
      if (cmd == "ssafe") return "ssafe ok";
    }
    if (cmd == "stlnt") {
      learnTime = static_cast<uint32_t>(std::stoul(t[1]));
      return "stlnt";
    }
    if (cmd == "svmov" && protocol >= 2) return "svmov " + t[1] + " ok";
    if (cmd == "stons" || cmd == "masns" || cmd == "staln" || cmd == "stdet" || cmd == "smotc" ||
        cmd == "stlnm") {
      return cmd;
    }
    return "";
  }

 private:
  std::string valveEx(int v, bool v3) const {
    std::string s = std::string(v3 ? "gvlvy " : "gvlvx ") + std::to_string(v) + " 1 40 " +
                    std::to_string(target[v]) +
                    " 21 3120 3350 -230 0 57 0 0 0 1 3000 1450 3 412 8123";
    if (v3) s += " " + std::to_string(assembly[v] ? 256 : 0) + " 0 50 " + std::to_string(target[v]) + " 0 0";
    return s;
  }

  uint64_t bootAtMs_ = 0;
};

}  // namespace vdm_test
