// Fake ESP-IDF 4.4 esp_attr.h. RTC_NOINIT_ATTR variables go into the section vdm_rtc_noinit: the
// runner hooks fill it with 0xA5 at power-on and hand it to the next boot of a case after a
// software, pin or watchdog reset (RTC slow memory keeps its content then).
#pragma once

#define RTC_NOINIT_ATTR __attribute__((section("vdm_rtc_noinit")))
#define RTC_DATA_ATTR
#define RTC_RODATA_ATTR
#define IRAM_ATTR
#define DRAM_ATTR
#define WORD_ALIGNED_ATTR __attribute__((aligned(4)))
#define NOINLINE_ATTR __attribute__((noinline))
