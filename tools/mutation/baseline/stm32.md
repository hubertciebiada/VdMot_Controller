# Mutation baseline: stm32 (STM core)

Baseline of the tool semantics on software_stm32/lib/core as of 58632d6 (the sources are unchanged at bc90f81), measured on 2026-09-25 with `tools/mutation/stm32.json` of a163b02 (--jobs 6 --no-cache) on a container copy of the bc90f81 tree. Refreshed after two generator fixes: the '&&' after an enum value is mutated (end_stop_detector.cpp:50, killed) and so are the comparisons on lines with a cast (18 mutants in buf_writer.cpp, calibration.cpp, move_classifier.cpp and profile_recorder.cpp; 8 survive, all at a saturation or sign boundary). The mean s/mutant now counts both runs of a re-run timeout and leaves out not_compiled mutants. Equivalents: `tools/mutation/equivalents/stm32/`. Files changed by later packages are compared against these numbers.

Result: **99.1 %** (986 of 995 counted mutants killed, 10 of them by a confirmed timeout); stillborn 53 (excluded), not compiled 0, equivalent 37, error 0. Mean 0.327 s per mutant (1048 built), wall 2.0 min with 6 workers; VM: 6 CPUs, no other test or mutation run, load average 0.4 at the start.

0 of 16 files are below 95 %; the gap column is the number of survivors each file still has to kill (or prove equivalent) to reach it.

| file | counted | killed | timeout | survived | stillborn | not compiled | equivalent | score | gap | s/mutant |
|---|---|---|---|---|---|---|---|---|---|---|
| lib/core/src/arg_parser.cpp | 164 | 164 | 0 | 0 | 6 | 0 | 0 | 100.0 % | 0 | 0.233 |
| lib/core/src/buf_writer.cpp | 131 | 130 | 0 | 1 | 5 | 0 | 2 | 99.2 % | 0 | 0.252 |
| lib/core/src/calibration.cpp | 101 | 96 | 0 | 5 | 3 | 0 | 5 | 95.0 % | 0 | 0.198 |
| lib/core/src/eeprom_layout.cpp | 56 | 56 | 0 | 0 | 4 | 0 | 0 | 100.0 % | 0 | 0.188 |
| lib/core/src/end_stop_detector.cpp | 67 | 67 | 0 | 0 | 1 | 0 | 8 | 100.0 % | 0 | 0.224 |
| lib/core/src/line_assembler.cpp | 69 | 60 | 9 | 0 | 0 | 0 | 1 | 100.0 % | 0 | 1.571 |
| lib/core/src/motor_params.cpp | 64 | 64 | 0 | 0 | 15 | 0 | 7 | 100.0 % | 0 | 0.245 |
| lib/core/src/move_classifier.cpp | 36 | 35 | 0 | 1 | 2 | 0 | 3 | 97.2 % | 0 | 0.228 |
| lib/core/src/onewire_check.cpp | 34 | 34 | 0 | 0 | 3 | 0 | 0 | 100.0 % | 0 | 0.255 |
| lib/core/src/profile_recorder.cpp | 71 | 68 | 1 | 2 | 2 | 0 | 4 | 97.2 % | 0 | 0.374 |
| lib/core/src/replies.cpp | 32 | 32 | 0 | 0 | 0 | 0 | 1 | 100.0 % | 0 | 0.331 |
| lib/core/src/replies_v2.cpp | 33 | 33 | 0 | 0 | 3 | 0 | 0 | 100.0 % | 0 | 0.249 |
| lib/core/src/retry_backoff.cpp | 32 | 32 | 0 | 0 | 0 | 0 | 3 | 100.0 % | 0 | 0.221 |
| lib/core/src/system_stats.cpp | 32 | 32 | 0 | 0 | 8 | 0 | 0 | 100.0 % | 0 | 0.167 |
| lib/core/src/target_rejection.cpp | 13 | 13 | 0 | 0 | 0 | 0 | 0 | 100.0 % | 0 | 0.218 |
| lib/core/src/tokenizer.cpp | 60 | 60 | 0 | 0 | 1 | 0 | 3 | 100.0 % | 0 | 0.208 |
| **total** | 995 | 976 | 10 | 9 | 53 | 0 | 37 | 99.1 % | 0 | 0.327 |

## Survivors (9)

The same list with the source lines is in `stm32.json`.

- lib/core/src/buf_writer.cpp:
  - 64:26 `<` -> `<=` (rel)
- lib/core/src/calibration.cpp:
  - 26:36 `||` -> `&&` (log)
  - 28:52 `>` -> `>=` (rel)
  - 32:16 `>` -> `>=` (rel)
  - 46:33 `<` -> `<=` (rel)
  - 48:15 `>` -> `>=` (rel)
- lib/core/src/move_classifier.cpp:
  - 15:44 `>` -> `>=` (rel)
- lib/core/src/profile_recorder.cpp:
  - 7:44 `>` -> `>=` (rel)
  - 10:24 `<` -> `<=` (rel)
