# Mutation testing: esp32-glue

Scope: `software_esp32_revamped/src`, ESP32 glue (Arduino code on host fakes).
Target: >= 95 % overall and >= 95 % for every file.
Result: **99.71 % overall (3744 / 3755 killed), lowest file `src/logger.cpp` 96.20 %, gate passed.**
Measured on commit `3ad136a` (2026-09-28), 6 workers, 28 min.

Tool: `tools/mutation/mutate.py`, config `tools/mutation/esp32-glue.json`. Every mutant is built
with the sanitizer build of the native tests (ASan + UBSan, `-Werror`) and runs the tests of its
module first, then the whole suite. A failed test, a crash or a timeout kills the mutant.
Not counted: stillborn mutants (they do not compile), mutants in code the native build does not
compile, and equivalent mutants. The 367 equivalent mutants are listed with a reason in
`tools/mutation/equivalents/esp32-glue/`; lines marked `// NOMUTATE` get no mutants.

## Reproduce

```sh
bash tools/native/docker.sh mutate esp32-glue            # this suite
bash tools/native/docker.sh mutate esp32-glue --files <file>
bash tools/native/docker.sh mutate-all                 # all four suites and the gate table
```

## Per file

| file | score | killed | survived | timeout | equivalent | stillborn | not compiled |
|---|---|---|---|---|---|---|---|
| `src/app.cpp` | 100.00 % | 136 | 0 | 0 | 7 | 11 | 0 |
| `src/logger.cpp` | 96.20 % | 146 | 6 | 6 | 21 | 2 | 3 |
| `src/main.cpp` | 100.00 % | 1 | 0 | 0 | 0 | 0 | 0 |
| `src/mqtt_client.cpp` | 99.70 % | 653 | 2 | 9 | 112 | 44 | 0 |
| `src/net.cpp` | 100.00 % | 294 | 0 | 0 | 31 | 6 | 0 |
| `src/ota.cpp` | 100.00 % | 238 | 0 | 4 | 8 | 3 | 0 |
| `src/stm_link.cpp` | 100.00 % | 72 | 0 | 2 | 1 | 6 | 0 |
| `src/stm_service.cpp` | 100.00 % | 55 | 0 | 0 | 6 | 1 | 0 |
| `src/storage.cpp` | 99.57 % | 691 | 3 | 7 | 77 | 85 | 0 |
| `src/web_server.cpp` | 100.00 % | 1429 | 0 | 1 | 104 | 331 | 0 |
| **total** | **99.71 %** | 3715 | 11 | 29 | 367 | 489 | 3 |

## Surviving mutants (11)

<details><summary>list</summary>

- `src/logger.cpp:113:81` const `3` -> `0`
- `src/logger.cpp:113:81` const `3` -> `2`
- `src/logger.cpp:113:81` const `3` -> `4`
- `src/logger.cpp:140:25` retval `kStepRotate` -> `0`
- `src/logger.cpp:186:6` negcond `(` -> `(!`
- `src/logger.cpp:238:77` const `0` -> `1`
- `src/mqtt_client.cpp:190:29` arith `+` -> `-`
- `src/mqtt_client.cpp:190:31` const `1` -> `0`
- `src/storage.cpp:195:56` const `0` -> `1`
- `src/storage.cpp:266:33` const `48` -> `49`
- `src/storage.cpp:504:44` bool `false` -> `true`

</details>
