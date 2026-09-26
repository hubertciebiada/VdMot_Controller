# Mutation testing: esp32-glue

Scope: `software_esp32_revamped/src`, ESP32 glue (Arduino code on host fakes).
Target: >= 95 % overall and >= 95 % for every file.
Result: **99.71 % overall (3753 / 3764 killed), lowest file `src/logger.cpp` 96.05 %, gate passed.**
Measured on commit `bd3d91c` (2026-09-26), 6 workers, 29 min.

Tool: `tools/mutation/mutate.py`, config `tools/mutation/esp32-glue.json`. Every mutant is built
with the sanitizer build of the native tests (ASan + UBSan, `-Werror`) and runs the tests of its
module first, then the whole suite. A failed test, a crash or a timeout kills the mutant.
Not counted: stillborn mutants (they do not compile), mutants in code the native build does not
compile, and equivalent mutants. The 372 equivalent mutants are listed with a reason in
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
| `src/app.cpp` | 100.00 % | 129 | 0 | 0 | 7 | 15 | 0 |
| `src/logger.cpp` | 96.05 % | 140 | 6 | 6 | 21 | 1 | 3 |
| `src/main.cpp` | 100.00 % | 1 | 0 | 0 | 0 | 0 | 0 |
| `src/mqtt_client.cpp` | 99.70 % | 652 | 2 | 9 | 112 | 48 | 0 |
| `src/net.cpp` | 100.00 % | 336 | 0 | 0 | 31 | 10 | 0 |
| `src/ota.cpp` | 100.00 % | 214 | 0 | 4 | 10 | 3 | 0 |
| `src/stm_link.cpp` | 100.00 % | 64 | 0 | 2 | 1 | 10 | 0 |
| `src/stm_service.cpp` | 100.00 % | 55 | 0 | 0 | 6 | 5 | 0 |
| `src/storage.cpp` | 99.57 % | 681 | 3 | 7 | 77 | 83 | 0 |
| `src/web_server.cpp` | 100.00 % | 1452 | 0 | 1 | 107 | 378 | 0 |
| **total** | **99.71 %** | 3724 | 11 | 29 | 372 | 553 | 3 |

## Surviving mutants (11)

<details><summary>list</summary>

- `src/logger.cpp:108:81` const `3` -> `0`
- `src/logger.cpp:108:81` const `3` -> `2`
- `src/logger.cpp:108:81` const `3` -> `4`
- `src/logger.cpp:134:25` retval `kStepRotate` -> `0`
- `src/logger.cpp:179:6` negcond `(` -> `(!`
- `src/logger.cpp:231:77` const `0` -> `1`
- `src/mqtt_client.cpp:189:29` arith `+` -> `-`
- `src/mqtt_client.cpp:189:31` const `1` -> `0`
- `src/storage.cpp:193:56` const `0` -> `1`
- `src/storage.cpp:264:33` const `48` -> `49`
- `src/storage.cpp:472:44` bool `false` -> `true`

</details>
