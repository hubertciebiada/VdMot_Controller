// Fork-per-case doctest runner of the native glue suites; the options are in testkit.h.
//
// The parent process never runs test code: it lists the cases that pass the doctest filters (in
// file order) and forks one process per case and boot, so each case starts from the parent's
// pristine static state. Verdicts come from the exit status of the case process.
#define DOCTEST_CONFIG_IMPLEMENT
#include "doctest.h"

#include "testkit.h"

#include <elf.h>
#include <errno.h>
#include <link.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>

#include <iostream>
#include <string>
#include <vector>

namespace testkit {
namespace {

// Exit codes of a case process.
constexpr int kExitPassed = 0;
constexpr int kExitFailed = 1;
constexpr int kExitNoFork = 2;      // reboot() without the fork runner
constexpr int kExitInvariant = 3;   // Hooks::checkInvariants reported a violation
constexpr int kExitNoReboot = 4;    // reboot() after a failed assertion
constexpr int kExitHandOff = 5;     // the stores (Hooks::save, Hooks::load) or the reset kind
                                    // could not be handed over
constexpr int kExitReboot = 75;     // reboot() requested the next boot

// Exit codes of the runner (testkit.h).
constexpr int kRunnerFailed = 1;
constexpr int kRunnerTimeout = 124;
constexpr int kRunnerError = 125;

Hooks g_hooks;  // zero-initialized before any static initializer calls setHooks()
unsigned g_boot = 0;
Reset g_lastReset = Reset::PowerOn;
bool g_caseProcess = false;  // this process runs one case for the fork runner
std::string g_storesPath;
std::string g_resetPath;
const char* g_caseName = "";
const char* g_invariantMessage = nullptr;
unsigned g_invariantFailures = 0;

// Verdict of one case: Timeout and Error are no evidence against the code under test.
enum class Outcome { Passed, Failed, Timeout, Error };
struct Verdict {
  Outcome outcome;
  std::string reason;
};

struct CaseInfo {
  std::string name;
  std::string file;
  unsigned line;
};
std::vector<CaseInfo> g_cases;

void flushAll() {
  std::cout.flush();
  std::cerr.flush();
  fflush(nullptr);
}

bool envFlag(const char* name) {
  const char* v = getenv(name);
  return v != nullptr && v[0] != '\0' && strcmp(v, "0") != 0;
}

int envInt(const char* name, int fallback) {
  const char* v = getenv(name);
  if (v == nullptr || v[0] == '\0') return fallback;
  char* end = nullptr;
  const long n = strtol(v, &end, 10);
  if (*end != '\0' || n < 0 || n > 100000) return fallback;
  return static_cast<int>(n);
}

const char* resetName(Reset r) {
  switch (r) {
    case Reset::PowerOn: return "power-on";
    case Reset::Pin: return "pin";
    case Reset::Software: return "software";
    case Reset::Watchdog: return "watchdog";
  }
  return "unknown";
}

// Collects the cases of a --dt-list-test-cases query (the parent's case list).
struct ListReporter : public doctest::IReporter {
  explicit ListReporter(const doctest::ContextOptions&) {}
  void report_query(const doctest::QueryData& in) override {
    for (unsigned i = 0; i < in.num_data; ++i) {
      const doctest::TestCaseData* tc = in.data[i];
      g_cases.push_back({tc->m_name, tc->m_file.c_str(), tc->m_line});
    }
  }
  void test_run_start() override {}
  void test_run_end(const doctest::TestRunStats&) override {}
  void test_case_start(const doctest::TestCaseData&) override {}
  void test_case_reenter(const doctest::TestCaseData&) override {}
  void test_case_end(const doctest::CurrentTestCaseStats&) override {}
  void test_case_exception(const doctest::TestCaseException&) override {}
  void subcase_start(const doctest::SubcaseSignature&) override {}
  void subcase_end() override {}
  void log_assert(const doctest::AssertData&) override {}
  void log_message(const doctest::MessageData&) override {}
  void test_case_skipped(const doctest::TestCaseData&) override {}
};

// Console output of a case process without doctest's run summary: its counts would describe
// the filtered run of one case, the runner prints the verdict instead.
struct CaseReporter : public doctest::ConsoleReporter {
  using doctest::ConsoleReporter::ConsoleReporter;
  void test_run_end(const doctest::TestRunStats&) override {}
};

// Hooks::checkInvariants at the end of a boot (the case ends or reboots); false after a violation.
bool invariantsHold() {
  if (g_hooks.checkInvariants == nullptr) return true;
  const char* message = g_hooks.checkInvariants();
  if (message == nullptr) return true;
  g_invariantMessage = message;
  ++g_invariantFailures;
  std::cout << "[testkit] invariant violated in \"" << g_caseName << "\" at boot " << g_boot << ": "
            << message << std::endl;
  return false;
}

// Checks Hooks::checkInvariants after every executed case (fork and no-fork mode).
struct InvariantListener : public doctest::IReporter {
  explicit InvariantListener(const doctest::ContextOptions&) {}
  void test_case_start(const doctest::TestCaseData& tc) override { g_caseName = tc.m_name; }
  void test_case_end(const doctest::CurrentTestCaseStats&) override { invariantsHold(); }
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

// Arguments that only query doctest (help, version, listings): no case is run.
bool isQuery(const char* arg) {
  static const char* const kQueries[] = {"h", "help", "?", "v", "version", "c", "count", "ltc",
                                         "list-test-cases", "lts", "list-test-suites", "lr",
                                         "list-reporters"};
  while (*arg == '-') ++arg;
  if (strncmp(arg, "dt-", 3) == 0) arg += 3;
  for (const char* q : kQueries) {
    if (strcmp(arg, q) == 0) return true;
  }
  return false;
}

int runDoctest(std::vector<const char*> args) {
  doctest::Context context(static_cast<int>(args.size()), args.data());
  return context.run();
}

[[noreturn]] void exitCase(int code) {
  flushAll();
  _exit(code);
}

// Child process: one boot of case `index` (1-based, among the cases passing the filters).
[[noreturn]] void runCaseProcess(std::vector<const char*> args, unsigned index, bool failFast,
                                 int timeoutS) {
  if (timeoutS > 0) alarm(static_cast<unsigned>(timeoutS));
  if (g_boot == 0) {
    if (g_hooks.powerOn != nullptr) g_hooks.powerOn();
  } else {
    if (g_hooks.load != nullptr && !g_hooks.load(g_storesPath.c_str())) {
      std::cout << "[testkit] loading the stores for boot " << g_boot << " failed" << std::endl;
      exitCase(kExitHandOff);
    }
    if (g_lastReset == Reset::PowerOn && g_hooks.powerOn != nullptr) g_hooks.powerOn();
  }
  const std::string first = "--dt-first=" + std::to_string(index);
  const std::string last = "--dt-last=" + std::to_string(index);
  args.push_back("--dt-order-by=file");
  args.push_back(first.c_str());
  args.push_back(last.c_str());
  args.push_back("--dt-no-intro");
  args.push_back("--dt-no-version");
  args.push_back("--dt-minimal");
  args.push_back("--dt-reporters=testkit_case");
  if (failFast) args.push_back("--dt-abort-after=1");
  const int result = runDoctest(args);
  if (g_invariantMessage != nullptr) exitCase(kExitInvariant);
  exitCase(result == 0 ? kExitPassed : kExitFailed);
}

bool readResetKind(Reset& kind) {
  FILE* f = fopen(g_resetPath.c_str(), "rb");
  if (f == nullptr) return false;
  const int c = fgetc(f);
  fclose(f);
  if (c < static_cast<int>(Reset::PowerOn) || c > static_cast<int>(Reset::Watchdog)) return false;
  kind = static_cast<Reset>(c);
  return true;
}

// Runs every boot of one case.
Verdict runCase(const std::vector<const char*>& args, unsigned index, bool failFast, int timeoutS,
                int maxBoots) {
  unlink(g_storesPath.c_str());
  unlink(g_resetPath.c_str());
  Reset kind = Reset::PowerOn;
  for (unsigned boot = 0;; ++boot) {
    flushAll();
    const pid_t pid = fork();
    if (pid < 0) return {Outcome::Error, std::string("fork failed: ") + strerror(errno)};
    if (pid == 0) {
      g_boot = boot;
      g_lastReset = kind;
      g_caseProcess = true;
      runCaseProcess(args, index, failFast, timeoutS);
    }
    int status = 0;
    while (waitpid(pid, &status, 0) < 0) {
      if (errno != EINTR) {
        return {Outcome::Error, std::string("waitpid failed: ") + strerror(errno)};
      }
    }
    const std::string at = " at boot " + std::to_string(boot);
    if (WIFSIGNALED(status)) {
      const int sig = WTERMSIG(status);
      if (sig == SIGALRM) {
        return {Outcome::Timeout,
                "timeout (signal 14) after " + std::to_string(timeoutS) + " s" + at};
      }
      return {Outcome::Failed,
              "killed by signal " + std::to_string(sig) + " (" + strsignal(sig) + ")" + at};
    }
    const int code = WEXITSTATUS(status);
    if (code == kExitReboot) {
      if (boot + 1 >= static_cast<unsigned>(maxBoots)) {
        return {Outcome::Failed, "too many boots (VDM_MAX_BOOTS " + std::to_string(maxBoots) + ")"};
      }
      if (!readResetKind(kind)) return {Outcome::Error, "reboot without a reset kind" + at};
      continue;
    }
    switch (code) {
      case kExitPassed: return {Outcome::Passed, ""};
      case kExitInvariant: return {Outcome::Failed, "invariant violated" + at};
      case kExitNoReboot:
        return {Outcome::Failed, "reboot requested after a failed assertion" + at};
      case kExitHandOff: return {Outcome::Error, "stores or reset kind not handed over" + at};
      default: return {Outcome::Failed, "failed (exit " + std::to_string(code) + ")" + at};
    }
  }
}

int runForked(const std::vector<const char*>& args, bool failFast, int timeoutS, int maxBoots) {
  std::vector<const char*> listArgs = args;
  listArgs.push_back("--dt-order-by=file");
  listArgs.push_back("--dt-list-test-cases");
  listArgs.push_back("--dt-reporters=testkit_list");
  runDoctest(listArgs);

  const char* tmp = getenv("TMPDIR");
  std::string dirTemplate = std::string(tmp != nullptr && tmp[0] != '\0' ? tmp : "/tmp") +
                            "/testkit-XXXXXX";
  std::vector<char> dir(dirTemplate.begin(), dirTemplate.end());
  dir.push_back('\0');
  if (mkdtemp(dir.data()) == nullptr) {
    std::cout << "[testkit] cannot create a hand-off directory: " << strerror(errno) << std::endl;
    return kRunnerError;
  }
  g_storesPath = std::string(dir.data()) + "/stores";
  g_resetPath = std::string(dir.data()) + "/reset";

  unsigned ran = 0;
  unsigned failed = 0;
  bool anyFailed = false;
  bool anyTimeout = false;
  bool anyError = false;
  for (size_t i = 0; i < g_cases.size(); ++i) {
    ++ran;
    const Verdict verdict = runCase(args, static_cast<unsigned>(i + 1), failFast, timeoutS,
                                    maxBoots);
    if (verdict.outcome == Outcome::Passed) continue;
    ++failed;
    anyFailed = anyFailed || verdict.outcome == Outcome::Failed;
    anyTimeout = anyTimeout || verdict.outcome == Outcome::Timeout;
    anyError = anyError || verdict.outcome == Outcome::Error;
    const CaseInfo& c = g_cases[i];
    std::cout << "[testkit] FAILED \"" << c.name << "\" (" << c.file << ":" << c.line
              << "): " << verdict.reason << std::endl;
    if (failFast) {
      std::cout << "[testkit] fail-fast: stopped after the first failing case" << std::endl;
      break;
    }
  }
  unlink(g_storesPath.c_str());
  unlink(g_resetPath.c_str());
  rmdir(dir.data());
  std::cout << "[testkit] " << g_cases.size() << " cases, " << ran << " run: " << (ran - failed)
            << " passed, " << failed << " failed" << std::endl;
  if (anyFailed) return kRunnerFailed;
  if (anyError) return kRunnerError;
  return anyTimeout ? kRunnerTimeout : 0;
}

// dl_iterate_phdr() reports the executable first: its load address is the bias of a PIE.
int executableBias(struct dl_phdr_info* info, size_t, void* data) {
  *static_cast<ElfW(Addr)*>(data) = info->dlpi_addr;
  return 1;
}

int runInProcess(std::vector<const char*> args, bool failFast) {
  if (failFast) args.push_back("--dt-abort-after=1");
  if (g_hooks.powerOn != nullptr) g_hooks.powerOn();
  const int result = runDoctest(args);
  flushAll();
  return (result != 0 || g_invariantFailures != 0) ? 1 : 0;
}

}  // namespace

unsigned boot() { return g_boot; }

Reset lastReset() { return g_lastReset; }

void reboot(Reset kind) {
  if (!g_caseProcess) {
    std::cout << "[testkit] reboot(" << resetName(kind)
              << ") needs the fork runner (not available with --no-fork)" << std::endl;
    exitCase(kExitNoFork);
  }
  const doctest::detail::ContextState* cs = doctest::detail::g_cs;
  if (cs != nullptr && cs->numAssertsFailedCurrentTest_atomic > 0) {
    std::cout << "[testkit] failed assertion at boot " << g_boot << ": no " << resetName(kind)
              << " reboot" << std::endl;
    exitCase(kExitNoReboot);
  }
  // the case does not end in this process, so the listener would never see this boot
  if (!invariantsHold()) exitCase(kExitInvariant);
  if (g_hooks.save != nullptr && !g_hooks.save(g_storesPath.c_str())) {
    std::cout << "[testkit] saving the stores at boot " << g_boot << " failed" << std::endl;
    exitCase(kExitHandOff);
  }
  FILE* f = fopen(g_resetPath.c_str(), "wb");
  if (f == nullptr || fputc(static_cast<int>(kind), f) == EOF || fclose(f) != 0) {
    std::cout << "[testkit] cannot write the reset kind at boot " << g_boot << std::endl;
    exitCase(kExitHandOff);
  }
  exitCase(kExitReboot);
}

void setHooks(const Hooks& hooks) { g_hooks = hooks; }

bool saveRegions(const char* path, const Region* regions, size_t count) {
  FILE* f = fopen(path, "wb");
  if (f == nullptr) return false;
  bool ok = true;
  for (size_t i = 0; i < count && ok; ++i) {
    const uint64_t size = regions[i].size;
    ok = fwrite(&size, sizeof size, 1, f) == 1 &&
         (size == 0 || fwrite(regions[i].data, 1, regions[i].size, f) == regions[i].size);
  }
  return fclose(f) == 0 && ok;
}

bool loadRegions(const char* path, const Region* regions, size_t count) {
  FILE* f = fopen(path, "rb");
  if (f == nullptr) return false;
  bool ok = true;
  for (size_t i = 0; i < count && ok; ++i) {
    uint64_t size = 0;
    ok = fread(&size, sizeof size, 1, f) == 1 && size == regions[i].size &&
         (size == 0 || fread(regions[i].data, 1, regions[i].size, f) == regions[i].size);
  }
  ok = ok && fgetc(f) == EOF;
  fclose(f);
  return ok;
}

bool sectionBounds(const char* name, uint8_t*& start, size_t& size) {
  FILE* f = fopen("/proc/self/exe", "rb");
  if (f == nullptr) return false;
  bool found = false;
  ElfW(Ehdr) eh;
  if (fread(&eh, sizeof eh, 1, f) == 1 && memcmp(eh.e_ident, ELFMAG, SELFMAG) == 0 &&
      eh.e_shentsize == sizeof(ElfW(Shdr)) && eh.e_shstrndx < eh.e_shnum) {
    std::vector<ElfW(Shdr)> sections(eh.e_shnum);
    if (fseek(f, static_cast<long>(eh.e_shoff), SEEK_SET) == 0 &&
        fread(sections.data(), sizeof(ElfW(Shdr)), sections.size(), f) == sections.size()) {
      const ElfW(Shdr)& strtab = sections[eh.e_shstrndx];
      std::vector<char> names(strtab.sh_size + 1, '\0');
      if (fseek(f, static_cast<long>(strtab.sh_offset), SEEK_SET) == 0 &&
          fread(names.data(), 1, strtab.sh_size, f) == strtab.sh_size) {
        for (const ElfW(Shdr)& s : sections) {
          if (s.sh_name < strtab.sh_size && s.sh_addr != 0 && strcmp(&names[s.sh_name], name) == 0) {
            ElfW(Addr) bias = 0;
            dl_iterate_phdr(executableBias, &bias);
            start = reinterpret_cast<uint8_t*>(bias + s.sh_addr);
            size = s.sh_size;
            found = true;
            break;
          }
        }
      }
    }
  }
  fclose(f);
  return found;
}

}  // namespace testkit

REGISTER_REPORTER("testkit_list", 0, testkit::ListReporter);
REGISTER_REPORTER("testkit_case", 0, testkit::CaseReporter);
REGISTER_LISTENER("testkit_invariants", 0, testkit::InvariantListener);

int main(int argc, char** argv) {
  bool failFast = testkit::envFlag("VDM_FAIL_FAST");
  bool noFork = testkit::envFlag("VDM_GLUE_NOFORK");
  bool query = false;
  std::vector<const char*> args;
  args.push_back(argv[0]);
  for (int i = 1; i < argc; ++i) {
    if (strcmp(argv[i], "--fail-fast") == 0) {
      failFast = true;
    } else if (strcmp(argv[i], "--no-fork") == 0) {
      noFork = true;
    } else {
      query = query || testkit::isQuery(argv[i]);
      args.push_back(argv[i]);
    }
  }
  if (query) return testkit::runDoctest(args);
  if (noFork) return testkit::runInProcess(args, failFast);
  const int timeoutS = testkit::envInt("VDM_CASE_TIMEOUT_S", 5);
  const int maxBoots = testkit::envInt("VDM_MAX_BOOTS", 8);
  return testkit::runForked(args, failFast, timeoutS, maxBoots < 1 ? 1 : maxBoots);
}
