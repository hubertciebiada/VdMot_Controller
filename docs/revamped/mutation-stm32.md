# Mutation testing: stm32

Scope: `software_stm32/lib/core`, STM32 core (pure logic).
Target: >= 95 % overall and >= 95 % for every file.
Result: **98.96 % overall (2094 / 2116 killed), lowest file `lib/core/src/calibration.cpp` 95.05 %, gate passed.**
Measured on commit `74f4d05` (2026-09-26), 6 workers, 3 min.

Tool: `tools/mutation/mutate.py`, config `tools/mutation/stm32.json`. Every mutant is built
with the sanitizer build of the native tests (ASan + UBSan, `-Werror`) and runs the tests of its
module first, then the whole suite. A failed test, a crash or a timeout kills the mutant.
Not counted: stillborn mutants (they do not compile), mutants in code the native build does not
compile, and equivalent mutants. The 46 equivalent mutants are listed with a reason in
`tools/mutation/equivalents/stm32/`; lines marked `// NOMUTATE` get no mutants.

## Reproduce

```sh
bash tools/native/docker.sh mutate stm32            # this suite
bash tools/native/docker.sh mutate stm32 --files <file>
bash tools/native/docker.sh mutate-all                 # all four suites and the gate table
```

## Per file

| file | score | killed | survived | timeout | equivalent | stillborn | not compiled |
|---|---|---|---|---|---|---|---|
| `lib/core/src/arg_parser.cpp` | 100.00 % | 164 | 0 | 0 | 0 | 6 | 0 |
| `lib/core/src/buf_writer.cpp` | 99.24 % | 130 | 1 | 0 | 2 | 5 | 0 |
| `lib/core/src/calibration.cpp` | 95.05 % | 96 | 5 | 0 | 5 | 3 | 0 |
| `lib/core/src/config_blocks.cpp` | 100.00 % | 194 | 0 | 0 | 0 | 10 | 0 |
| `lib/core/src/config_store.cpp` | 100.00 % | 60 | 0 | 0 | 0 | 0 | 0 |
| `lib/core/src/eeprom_layout.cpp` | 100.00 % | 116 | 0 | 0 | 0 | 4 | 0 |
| `lib/core/src/end_stop_detector.cpp` | 95.92 % | 94 | 4 | 0 | 8 | 2 | 0 |
| `lib/core/src/failsafe.cpp` | 100.00 % | 19 | 0 | 0 | 0 | 5 | 0 |
| `lib/core/src/fault_retry.cpp` | 100.00 % | 34 | 0 | 0 | 0 | 1 | 0 |
| `lib/core/src/lease.cpp` | 97.83 % | 45 | 1 | 0 | 0 | 3 | 0 |
| `lib/core/src/legacy_layout.cpp` | 100.00 % | 80 | 0 | 0 | 0 | 5 | 0 |
| `lib/core/src/line_assembler.cpp` | 100.00 % | 60 | 0 | 9 | 1 | 0 | 0 |
| `lib/core/src/manual_enable.cpp` | 100.00 % | 11 | 0 | 0 | 0 | 0 | 0 |
| `lib/core/src/motor_params.cpp` | 100.00 % | 64 | 0 | 0 | 7 | 15 | 0 |
| `lib/core/src/move_classifier.cpp` | 100.00 % | 89 | 0 | 0 | 9 | 2 | 0 |
| `lib/core/src/onewire_check.cpp` | 100.00 % | 34 | 0 | 0 | 0 | 3 | 0 |
| `lib/core/src/presence_test.cpp` | 95.56 % | 43 | 2 | 0 | 0 | 10 | 0 |
| `lib/core/src/profile_recorder.cpp` | 97.18 % | 68 | 2 | 1 | 4 | 2 | 0 |
| `lib/core/src/protection_guard.cpp` | 100.00 % | 21 | 0 | 0 | 1 | 0 | 0 |
| `lib/core/src/replies.cpp` | 100.00 % | 32 | 0 | 0 | 1 | 0 | 0 |
| `lib/core/src/replies_v2.cpp` | 100.00 % | 38 | 0 | 0 | 0 | 5 | 0 |
| `lib/core/src/replies_v3.cpp` | 100.00 % | 5 | 0 | 0 | 0 | 0 | 0 |
| `lib/core/src/reset_guard.cpp` | 100.00 % | 58 | 0 | 0 | 0 | 0 | 0 |
| `lib/core/src/retry_backoff.cpp` | 100.00 % | 32 | 0 | 0 | 3 | 0 | 0 |
| `lib/core/src/settings.cpp` | 100.00 % | 23 | 0 | 0 | 0 | 0 | 0 |
| `lib/core/src/stall_detector.cpp` | 100.00 % | 10 | 0 | 0 | 0 | 0 | 0 |
| `lib/core/src/store_scheduler.cpp` | 100.00 % | 40 | 0 | 0 | 0 | 3 | 0 |
| `lib/core/src/system_stats.cpp` | 100.00 % | 32 | 0 | 0 | 0 | 8 | 0 |
| `lib/core/src/target_rejection.cpp` | 100.00 % | 13 | 0 | 0 | 0 | 0 | 0 |
| `lib/core/src/temp_filter.cpp` | 100.00 % | 44 | 0 | 0 | 2 | 0 | 0 |
| `lib/core/src/temp_refresh.cpp` | 100.00 % | 16 | 0 | 0 | 0 | 0 | 0 |
| `lib/core/src/tokenizer.cpp` | 100.00 % | 60 | 0 | 0 | 3 | 1 | 0 |
| `lib/core/src/uart_errors.cpp` | 100.00 % | 8 | 0 | 0 | 0 | 0 | 0 |
| `lib/core/src/valve_scheduler.cpp` | 96.52 % | 194 | 7 | 0 | 0 | 5 | 0 |
| `lib/core/src/warm_state.cpp` | 100.00 % | 57 | 0 | 0 | 0 | 2 | 0 |
| **total** | **98.96 %** | 2084 | 22 | 10 | 46 | 100 | 0 |

## NOMUTATE lines

| file:line | reason |
|---|---|
| `lib/core/src/arg_parser.cpp:28` | callers only test hexValue() < 0, so any negative sentinel (-1/-2) is indistinguishable |
| `lib/core/src/arg_parser.cpp:35` | initial value is dead; parseMagnitude() overwrites it on success and it is unused on failure |
| `lib/core/src/arg_parser.cpp:50` | initial value is dead; parseMagnitude() overwrites it on success and it is unused on failure |
| `lib/core/src/arg_parser.cpp:65` | scratch array size; only indices 0..7 are written/read, a larger size is unobservable |
| `lib/core/src/buf_writer.cpp:50` | buffer size; all indexing uses sizeof(digits), so a larger buffer is unobservable |
| `lib/core/src/buf_writer.cpp:61` | buffer size; all indexing uses sizeof(text), so a larger buffer is unobservable |
| `lib/core/src/settings.cpp:7` | `<=` is equivalent, 50 raised to 50 stays 50 |
| `lib/core/src/settings.cpp:8` | `>=` is equivalent, 65534 capped at 65534 stays 65534 |
| `lib/core/src/stall_detector.cpp:10` | `<=` saturates at limit_ + 1, which stalled() (>=) cannot tell apart (the wrap at UINT32_MAX takes 2^32 ticks) |

## Surviving mutants (22)

<details><summary>list</summary>

- `lib/core/src/buf_writer.cpp:64:26` rel `<` -> `<=`
- `lib/core/src/calibration.cpp:26:36` log `||` -> `&&`
- `lib/core/src/calibration.cpp:28:52` rel `>` -> `>=`
- `lib/core/src/calibration.cpp:32:16` rel `>` -> `>=`
- `lib/core/src/calibration.cpp:46:33` rel `<` -> `<=`
- `lib/core/src/calibration.cpp:48:15` rel `>` -> `>=`
- `lib/core/src/end_stop_detector.cpp:48:23` rel `<` -> `<=`
- `lib/core/src/end_stop_detector.cpp:48:25` const `255` -> `254`
- `lib/core/src/end_stop_detector.cpp:48:25` const `255` -> `256`
- `lib/core/src/end_stop_detector.cpp:59:23` rel `>` -> `>=`
- `lib/core/src/lease.cpp:15:59` rel `>` -> `>=`
- `lib/core/src/presence_test.cpp:21:45` rel `<` -> `<=`
- `lib/core/src/presence_test.cpp:21:47` const `0` -> `1`
- `lib/core/src/profile_recorder.cpp:7:44` rel `>` -> `>=`
- `lib/core/src/profile_recorder.cpp:10:24` rel `<` -> `<=`
- `lib/core/src/valve_scheduler.cpp:23:54` rel `>` -> `>=`
- `lib/core/src/valve_scheduler.cpp:36:25` rel `>` -> `>=`
- `lib/core/src/valve_scheduler.cpp:51:19` rel `<` -> `<=`
- `lib/core/src/valve_scheduler.cpp:90:25` rel `<` -> `<=`
- `lib/core/src/valve_scheduler.cpp:112:29` bool `true` -> `false`
- `lib/core/src/valve_scheduler.cpp:112:49` bool `false` -> `true`
- `lib/core/src/valve_scheduler.cpp:130:25` rel `<` -> `<=`

</details>
