// Golden recorder of the C++ glue_system scenarios (docs/rust/GLUE-DESIGN-STM.md §7.4): one text
// file per test case in $VDM_GOLDEN_DIR, every boot of the case appended by its own process (the
// fork runner of tools/native/testkit). Linked into glue_system_golden (CMakeLists.txt here);
// the tests and the firmware sources are compiled unchanged, the recorder sees them through
// linker wraps of loop_system(), setup_system(), fake::inject() and testkit::reboot(), and
// through a doctest listener (start and end of a case).
//
// File format (software_stm32_rust/glue/src/system/golden.rs reads it):
//   case <test case name>
//   boot <n> <power-on|pin|software|watchdog>        reset that started boot n
//   t <ms> <rx|tx|dbg> "<bytes>"                      USART1 in (fake::inject), USART1 out,
//                                                     USART6 out; ms of the boot's fake time when
//                                                     the bytes went in, or when the
//                                                     loop_system()/setup_system() call that wrote
//                                                     them started
//   end <case|reboot power-on|reboot pin|...>         how the boot ended
//   e <addr> <64 hex digits>                          a 32-byte row of the 24LC64 that is not
//                                                     erased (0xFF), at the end of the boot
//   noinit <424 hex digits>                           the no-init cells in the STM32 layout
//                                                     (warm_state, guard_cell, reset_cell)
// Bytes: printable ASCII as is, \r \n \t \\ \" and \xHH for the others.
#include <elf.h>
#include <link.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include <string>

#include "doctest.h"
#include "fake_board.h"
#include "testkit.h"

extern HardwareSerial Serial6;

extern "C" {
void __real__Z11loop_systemv();
void __real__Z12setup_systemv();
void __real__ZN4fake6injectER14HardwareSerialRKNSt7__cxx1112basic_stringIcSt11char_traitsIcESaIcEEE(
  HardwareSerial& port, const std::string& bytes);
[[noreturn]] void __real__ZN7testkit6rebootENS_5ResetE(testkit::Reset kind);
}

namespace {

FILE* g_file = nullptr;
// bytes of Serial1's TX log already written (the tests clear the log with fake::takeTx)
size_t g_tx1Seen = 0;

const char* resetName(testkit::Reset r) {
  switch (r) {
    case testkit::Reset::PowerOn: return "power-on";
    case testkit::Reset::Pin: return "pin";
    case testkit::Reset::Software: return "software";
    case testkit::Reset::Watchdog: return "watchdog";
  }
  return "unknown";
}

uint32_t nowMs() { return static_cast<uint32_t>(fake::board.nowUs / 1000); }

void event(uint32_t ms, const char* dir, const char* data, size_t len) {
  if (g_file == nullptr || len == 0) return;
  fprintf(g_file, "t %u %s \"", ms, dir);
  for (size_t i = 0; i < len; i++) {
    const unsigned char c = static_cast<unsigned char>(data[i]);
    if (c == '\r') {
      fputs("\\r", g_file);
    } else if (c == '\n') {
      fputs("\\n", g_file);
    } else if (c == '\t') {
      fputs("\\t", g_file);
    } else if (c == '\\' || c == '"') {
      fprintf(g_file, "\\%c", c);
    } else if (c >= 0x20 && c < 0x7F) {
      fputc(c, g_file);
    } else {
      fprintf(g_file, "\\x%02x", c);
    }
  }
  fputs("\"\n", g_file);
}

// what the firmware wrote since the last call, at the time ms
void flushPorts(uint32_t ms) {
  if (Serial1.txLength < g_tx1Seen) g_tx1Seen = 0;
  if (Serial1.txLength > g_tx1Seen) {
    event(ms, "tx", Serial1.txLog + g_tx1Seen, Serial1.txLength - g_tx1Seen);
    g_tx1Seen = Serial1.txLength;
  }
  // the tests never read USART6: drained here, so its 16 KiB log never fills
  event(ms, "dbg", Serial6.txLog, Serial6.txLength);
  Serial6.txLength = 0;
}

// local symbols of this executable (the static no-init cells), from its ELF symbol table
const uint8_t* symbol(const char* name) {
  FILE* f = fopen("/proc/self/exe", "rb");
  if (f == nullptr) return nullptr;
  const uint8_t* found = nullptr;
  ElfW(Ehdr) eh;
  if (fread(&eh, sizeof eh, 1, f) == 1 && eh.e_shentsize == sizeof(ElfW(Shdr))) {
    std::string sections(eh.e_shnum * sizeof(ElfW(Shdr)), '\0');
    if (fseek(f, static_cast<long>(eh.e_shoff), SEEK_SET) == 0 &&
        fread(&sections[0], 1, sections.size(), f) == sections.size()) {
      const ElfW(Shdr)* sh = reinterpret_cast<const ElfW(Shdr)*>(sections.data());
      for (unsigned i = 0; i < eh.e_shnum && found == nullptr; i++) {
        if (sh[i].sh_type != SHT_SYMTAB || sh[i].sh_link >= eh.e_shnum) continue;
        const ElfW(Shdr)& strs = sh[sh[i].sh_link];
        std::string syms(sh[i].sh_size, '\0');
        std::string names(strs.sh_size + 1, '\0');
        if (fseek(f, static_cast<long>(sh[i].sh_offset), SEEK_SET) != 0 ||
            fread(&syms[0], 1, syms.size(), f) != syms.size() ||
            fseek(f, static_cast<long>(strs.sh_offset), SEEK_SET) != 0 ||
            fread(&names[0], 1, strs.sh_size, f) != strs.sh_size) {
          continue;
        }
        const ElfW(Sym)* s = reinterpret_cast<const ElfW(Sym)*>(syms.data());
        for (size_t k = 0; k < syms.size() / sizeof(ElfW(Sym)); k++) {
          if (s[k].st_name < strs.sh_size && strcmp(&names[s[k].st_name], name) == 0) {
            ElfW(Addr) bias = 0;
            dl_iterate_phdr(
              [](struct dl_phdr_info* info, size_t, void* data) {
                *static_cast<ElfW(Addr)*>(data) = info->dlpi_addr;
                return 1;
              },
              &bias);
            found = reinterpret_cast<const uint8_t*>(bias + s[k].st_value);
            break;
          }
        }
      }
    }
  }
  fclose(f);
  return found;
}

void hex(const uint8_t* p, size_t n) {
  for (size_t i = 0; i < n; i++) fprintf(g_file, "%02x", p[i]);
}

void endBoot(const std::string& how) {
  if (g_file == nullptr) return;
  flushPorts(nowMs());
  fprintf(g_file, "end %s\n", how.c_str());
  for (unsigned row = 0; row < fake::kEepromSize; row += 32) {
    const uint8_t* b = fake::eeprom.bytes + row;
    bool erased = true;
    for (unsigned i = 0; i < 32; i++) erased = erased && b[i] == 0xFF;
    if (erased) continue;
    fprintf(g_file, "e %04x ", row);
    hex(b, 32);
    fputc('\n', g_file);
  }
  // the STM32 layout of the cells (docs/rust/GLUE-DESIGN-STM.md §3.2): warm_state 180 bytes,
  // guard_cell 20, reset_cell 12
  const uint8_t* warm = symbol("_ZL10warm_state");
  const uint8_t* guard = symbol("_ZL10guard_cell");
  const uint8_t* counter = symbol("_ZL10reset_cell");
  if (warm == nullptr || guard == nullptr || counter == nullptr) {
    fprintf(stderr, "golden recorder: no-init cells not found in the symbol table\n");
    abort();
  }
  fputs("noinit ", g_file);
  hex(warm, 180);
  hex(guard, 20);
  hex(counter, 12);
  fputc('\n', g_file);
  fclose(g_file);
  g_file = nullptr;
}

// "test_system_calib.cpp" + "system W2-5: after a calibration ..." -> "calib__w2_5_after_a_calibration..."
std::string slug(const char* file, const char* name) {
  std::string stem = file;
  const size_t slash = stem.find_last_of('/');
  if (slash != std::string::npos) stem = stem.substr(slash + 1);
  const size_t dot = stem.find('.');
  if (dot != std::string::npos) stem = stem.substr(0, dot);
  const std::string prefix = "test_system_";
  if (stem.compare(0, prefix.size(), prefix) == 0) stem = stem.substr(prefix.size());
  std::string s;
  for (const char* p = name; *p != '\0'; p++) {
    const char c = *p;
    if ((c >= 'a' && c <= 'z') || (c >= '0' && c <= '9')) {
      s += c;
    } else if (c >= 'A' && c <= 'Z') {
      s += static_cast<char>(c - 'A' + 'a');
    } else if (!s.empty() && s.back() != '_') {
      s += '_';
    }
  }
  const std::string word = "system_";
  if (s.compare(0, word.size(), word) == 0) s = s.substr(word.size());
  if (s.size() > 64) s.resize(64);
  while (!s.empty() && s.back() == '_') s.pop_back();
  return stem + "__" + s;
}

struct GoldenListener : public doctest::IReporter {
  explicit GoldenListener(const doctest::ContextOptions&) {}
  void test_case_start(const doctest::TestCaseData& tc) override {
    const char* dir = getenv("VDM_GOLDEN_DIR");
    if (dir == nullptr || dir[0] == '\0') return;
    const std::string path = std::string(dir) + "/" + slug(tc.m_file.c_str(), tc.m_name) + ".txt";
    g_file = fopen(path.c_str(), testkit::boot() == 0 ? "w" : "a");
    if (g_file == nullptr) {
      fprintf(stderr, "golden recorder: cannot write %s\n", path.c_str());
      abort();
    }
    if (testkit::boot() == 0) fprintf(g_file, "case %s\n", tc.m_name);
    fprintf(g_file, "boot %u %s\n", testkit::boot(), resetName(testkit::lastReset()));
    g_tx1Seen = 0;
  }
  void test_case_end(const doctest::CurrentTestCaseStats&) override { endBoot("case"); }
  void report_query(const doctest::QueryData&) override {}
  void test_run_start() override {}
  void test_run_end(const doctest::TestRunStats&) override {}
  void test_case_reenter(const doctest::TestCaseData&) override {}
  void test_case_exception(const doctest::TestCaseException&) override {}
  void subcase_start(const doctest::SubcaseSignature&) override {}
  void subcase_end() override {}
  void log_assert(const doctest::AssertData&) override {}
  void log_message(const doctest::MessageData&) override {}
  void test_case_skipped(const doctest::TestCaseData&) override {}
};

REGISTER_LISTENER("golden", 1, GoldenListener);

// one call of the firmware's main loop or set-up: what it wrote, at the time it started
template <class F>
void observe(F&& real) {
  const uint32_t ms = nowMs();
  flushPorts(ms);
  try {
    real();
  } catch (...) {
    flushPorts(ms);
    throw;
  }
  flushPorts(ms);
}

}  // namespace

extern "C" {

void __wrap__Z11loop_systemv() { observe(__real__Z11loop_systemv); }

void __wrap__Z12setup_systemv() { observe(__real__Z12setup_systemv); }

void __wrap__ZN4fake6injectER14HardwareSerialRKNSt7__cxx1112basic_stringIcSt11char_traitsIcESaIcEEE(
  HardwareSerial& port, const std::string& bytes) {
  flushPorts(nowMs());
  if (&port == &Serial1) event(nowMs(), "rx", bytes.data(), bytes.size());
  __real__ZN4fake6injectER14HardwareSerialRKNSt7__cxx1112basic_stringIcSt11char_traitsIcESaIcEEE(port, bytes);
}

[[noreturn]] void __wrap__ZN7testkit6rebootENS_5ResetE(testkit::Reset kind) {
  endBoot(std::string("reboot ") + resetName(kind));
  __real__ZN7testkit6rebootENS_5ResetE(kind);
}

}  // extern "C"
