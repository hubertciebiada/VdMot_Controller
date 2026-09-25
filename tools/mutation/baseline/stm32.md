# Mutation baseline: stm32 (STM core)

Baseline of the new tool semantics on software_stm32/lib/core as of 58632d6 (the sources are unchanged in this branch), measured on 2026-09-25 with `tools/native/docker.sh mutate stm32 --no-cache` (VDM_MUTATION_JOBS=3) at 3bc8832. Equivalents: `tools/mutation/equivalents/stm32/`. Files changed by later packages are compared against these numbers (PLAN.md rule 5).

Result: **99.9 %** (975 of 976 counted mutants killed, 10 of them by a confirmed timeout); stillborn 53 (excluded), not compiled 0, equivalent 37, error 0. Mean 0.179 s per mutant, wall 2.0 min with 3 workers; VM: 6 CPUs shared with other agents' containers, not idle.

0 of 16 files are below 95 %; the gap column is the number of survivors each file still has to kill (or prove equivalent) to reach it.

| file | counted | killed | timeout | survived | stillborn | not compiled | equivalent | score | gap | s/mutant |
|---|---|---|---|---|---|---|---|---|---|---|
| lib/core/src/arg_parser.cpp | 164 | 164 | 0 | 0 | 6 | 0 | 0 | 100.0 % | 0 | 0.138 |
| lib/core/src/buf_writer.cpp | 129 | 129 | 0 | 0 | 5 | 0 | 2 | 100.0 % | 0 | 0.142 |
| lib/core/src/calibration.cpp | 93 | 92 | 0 | 1 | 3 | 0 | 5 | 98.9 % | 0 | 0.124 |
| lib/core/src/eeprom_layout.cpp | 56 | 56 | 0 | 0 | 4 | 0 | 0 | 100.0 % | 0 | 0.124 |
| lib/core/src/end_stop_detector.cpp | 66 | 66 | 0 | 0 | 1 | 0 | 8 | 100.0 % | 0 | 0.129 |
| lib/core/src/line_assembler.cpp | 69 | 60 | 9 | 0 | 0 | 0 | 1 | 100.0 % | 0 | 0.799 |
| lib/core/src/motor_params.cpp | 64 | 64 | 0 | 0 | 15 | 0 | 7 | 100.0 % | 0 | 0.135 |
| lib/core/src/move_classifier.cpp | 32 | 32 | 0 | 0 | 2 | 0 | 3 | 100.0 % | 0 | 0.127 |
| lib/core/src/onewire_check.cpp | 34 | 34 | 0 | 0 | 3 | 0 | 0 | 100.0 % | 0 | 0.129 |
| lib/core/src/profile_recorder.cpp | 67 | 66 | 1 | 0 | 2 | 0 | 4 | 100.0 % | 0 | 0.189 |
| lib/core/src/replies.cpp | 32 | 32 | 0 | 0 | 0 | 0 | 1 | 100.0 % | 0 | 0.108 |
| lib/core/src/replies_v2.cpp | 33 | 33 | 0 | 0 | 3 | 0 | 0 | 100.0 % | 0 | 0.136 |
| lib/core/src/retry_backoff.cpp | 32 | 32 | 0 | 0 | 0 | 0 | 3 | 100.0 % | 0 | 0.109 |
| lib/core/src/system_stats.cpp | 32 | 32 | 0 | 0 | 8 | 0 | 0 | 100.0 % | 0 | 0.105 |
| lib/core/src/target_rejection.cpp | 13 | 13 | 0 | 0 | 0 | 0 | 0 | 100.0 % | 0 | 0.116 |
| lib/core/src/tokenizer.cpp | 60 | 60 | 0 | 0 | 1 | 0 | 3 | 100.0 % | 0 | 0.139 |
| **total** | 976 | 965 | 10 | 1 | 53 | 0 | 37 | 99.9 % | 0 | 0.179 |

## Survivors (1)

The same list with the source lines is in `stm32.json`.

- lib/core/src/calibration.cpp:
  - 26:36 `||` -> `&&` (log)
