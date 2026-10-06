// PROGMEM access as arduino-esp32 2.0.7 (the VdMot firmware's core) defines it: flash is
// memory-mapped on the ESP32, so every read is a plain load. ArduinoJson includes this header
// when ARDUINOJSON_ENABLE_PROGMEM is 1 without ARDUINO (json_body's reference program); the
// macros it does not find here (pgm_read_double) it defines itself with memcpy_P, which reads
// the same bytes.
#pragma once

#include <string.h>

#define PROGMEM
#define PGM_P const char*
#define PSTR(s) (s)

#define pgm_read_byte(addr) (*(const unsigned char*)(addr))
#define pgm_read_word(addr) ({ \
  typeof(addr) _addr = (addr); \
  *(const unsigned short*)(_addr); \
})
#define pgm_read_dword(addr) ({ \
  typeof(addr) _addr = (addr); \
  *(const unsigned long*)(_addr); \
})
#define pgm_read_float(addr) ({ \
  typeof(addr) _addr = (addr); \
  *(const float*)(_addr); \
})
#define pgm_read_ptr(addr) ({ \
  typeof(addr) _addr = (addr); \
  *(void* const*)(_addr); \
})

#define memcmp_P memcmp
#define memcpy_P memcpy
#define strcmp_P strcmp
#define strncmp_P strncmp
#define strlen_P strlen
