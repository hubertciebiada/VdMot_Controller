// Cases for the self-test of the fork-per-case runner. selftest.sh runs this executable with
// suite filters and checks the runner's verdicts: the suites other than "pass" fail on purpose.
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#include "doctest.h"
#include "testkit.h"

using testkit::Reset;

namespace {

int g_counter = 0;  // file-static state: every case starts at 0
// Stands for .noinit / RTC RAM (kept by pin, software and watchdog resets); the hooks find it
// by its section name like a glue harness finds .noinit.
__attribute__((section(".testkit_warm"))) uint8_t g_warm[32];
uint32_t g_store = 0;               // stands for EEPROM / NVS: kept by every reset
const char* g_violation = nullptr;  // reported by the invariant hook

testkit::Region warmSection() {
  uint8_t* start = nullptr;
  size_t size = 0;
  if (!testkit::sectionBounds(".testkit_warm", start, size)) return {nullptr, 0};
  return {start, size};
}

void powerOn() {
  const testkit::Region warm = warmSection();
  if (warm.data != nullptr) memset(warm.data, 0xA5, warm.size);
}

bool save(const char* path) {
  const testkit::Region regions[] = {warmSection(), {&g_store, sizeof g_store}};
  return regions[0].data != nullptr && testkit::saveRegions(path, regions, 2);
}

bool load(const char* path) {
  const testkit::Region regions[] = {warmSection(), {&g_store, sizeof g_store}};
  return regions[0].data != nullptr && testkit::loadRegions(path, regions, 2);
}

const char* checkInvariants() { return g_violation; }

struct RegisterHooks {
  RegisterHooks() { testkit::setHooks({powerOn, save, load, checkInvariants}); }
} g_registerHooks;

// Appends this process id to $TESTKIT_MARKERS/<name>, so the driver sees which cases ran where.
void marker(const char* name) {
  const char* dir = getenv("TESTKIT_MARKERS");
  if (dir == nullptr) return;
  char path[512];
  snprintf(path, sizeof path, "%s/%s", dir, name);
  FILE* f = fopen(path, "a");
  if (f == nullptr) return;
  fprintf(f, "%d\n", static_cast<int>(getpid()));
  fclose(f);
}

bool warmIs(uint8_t value) {
  for (uint8_t b : g_warm) {
    if (b != value) return false;
  }
  return true;
}

volatile int g_one = 1;  // keeps the failing checks from being constant expressions

}  // namespace

TEST_SUITE("pass") {
  TEST_CASE("static state starts fresh in every case (1)") { CHECK(++g_counter == 1); }

  TEST_CASE("static state starts fresh in every case (2)") { CHECK(++g_counter == 1); }

  TEST_CASE("sectionBounds finds a section of the executable") {
    uint8_t* start = nullptr;
    size_t size = 0;
    REQUIRE(testkit::sectionBounds(".testkit_warm", start, size));
    CHECK(start == g_warm);
    CHECK(size == sizeof g_warm);
    CHECK_FALSE(testkit::sectionBounds(".testkit_none", start, size));
  }

  TEST_CASE("boot 0 is a power-on with the power-on pattern") {
    CHECK(testkit::boot() == 0);
    CHECK(testkit::lastReset() == Reset::PowerOn);
    CHECK(warmIs(0xA5));
    CHECK(g_store == 0);
  }

  TEST_CASE("a pin reset hands the stores and the warm RAM to boot 1") {
    if (testkit::boot() == 0) {
      g_counter = 5;
      g_store = 42;
      memset(g_warm, 7, sizeof g_warm);
      testkit::reboot(Reset::Pin);
    }
    CHECK(testkit::boot() == 1);
    CHECK(testkit::lastReset() == Reset::Pin);
    CHECK(g_store == 42);
    CHECK(warmIs(7));
    CHECK(g_counter == 0);  // ordinary RAM is not handed over
  }

  TEST_CASE("software and watchdog resets keep the warm RAM, a power-on refills it") {
    switch (testkit::boot()) {
      case 0:
        g_store = 1;
        memset(g_warm, 1, sizeof g_warm);
        testkit::reboot(Reset::Software);
      case 1:
        CHECK(testkit::lastReset() == Reset::Software);
        CHECK(warmIs(1));
        g_store = 2;
        memset(g_warm, 2, sizeof g_warm);
        testkit::reboot(Reset::Watchdog);
      case 2:
        CHECK(testkit::lastReset() == Reset::Watchdog);
        CHECK(warmIs(2));
        g_store = 3;
        memset(g_warm, 3, sizeof g_warm);
        testkit::reboot(Reset::PowerOn);
      default:
        CHECK(testkit::boot() == 3);
        CHECK(testkit::lastReset() == Reset::PowerOn);
        CHECK(warmIs(0xA5));
        CHECK(g_store == 3);
    }
  }
}

TEST_SUITE("pid") {
  TEST_CASE("process of case 1") { marker("pid"); }

  TEST_CASE("process of case 2") { marker("pid"); }
}

TEST_SUITE("fail") {
  TEST_CASE("first failing case") {
    marker("fail1");
    CHECK(g_one == 2);
  }

  TEST_CASE("second failing case") {
    marker("fail2");
    CHECK(g_one == 2);
  }
}

TEST_SUITE("hang") {
  TEST_CASE("a case that never ends") {
    marker("hang");
    while (g_one == 1) {
    }
  }
}

TEST_SUITE("boots") {
  TEST_CASE("a case that reboots forever") {
    marker("boots");
    testkit::reboot(Reset::Software);
  }
}

TEST_SUITE("invariant") {
  TEST_CASE("a broken invariant fails a case without a failed assertion") {
    g_violation = "1 RTOS violation";
    CHECK(g_one == 1);
  }
}

TEST_SUITE("failreboot") {
  TEST_CASE("no reboot after a failed assertion") {
    marker("failreboot");
    CHECK(g_one == 2);
    testkit::reboot(Reset::Pin);
  }
}
