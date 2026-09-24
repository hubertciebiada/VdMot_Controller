# VdMot STM32 firmware

## Build

    pio run -j 2                           # default envs (STM32_release_C1, STM32_release_C2)
    pio run -j 2 -e STM32F411_release_C2   # one env; see platformio.ini for all of them

## Native unit tests

Hardware-free logic lives in `lib/core` (no Arduino headers) and is tested on the
host with doctest, `-Wall -Wextra -Werror` and AddressSanitizer/UBSan:

    cmake -S test/native -B build/native && cmake --build build/native -j 2 && ctest --test-dir build/native --output-on-failure

## Protocol

The UART protocol to the ESP32 is v1 plus the v2 commands described in
[PROTOCOL_V2.md](PROTOCOL_V2.md) (diagnostics, service move, breakaway escalation).
