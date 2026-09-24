# Mutation testing: software_esp32_revamped/lib/core

Bar: overall score >= 85 %, every file >= 75 %.
Result (last complete run, round 2): **95.4 % overall (8656 / 9073 counted killed,
194 marked equivalent), lowest file 87.1 % (`ha_discovery.cpp`), all green.**

Tool: `tools/mutation/mutate.py`, config `tools/mutation/esp32.json`. Same rules as
the STM core (see `mutation-stm32.md`): sanitizer build, module test file first,
then the whole suite; build errors, crashes and timeouts count as killed.

## Per file (round 2, complete)

| file | killed/total | score |
|---|---|---|
| lib/core/src/auth.cpp | 281/297 | 94.6% |
| lib/core/src/calib_schedule.cpp | 419/423 | 99.1% |
| lib/core/src/common.cpp | 647/663 | 97.6% |
| lib/core/src/config.cpp | 1522/1628 | 93.5% |
| lib/core/src/event_log.cpp | 405/412 | 98.3% |
| lib/core/src/ha_discovery.cpp | 379/435 | 87.1% |
| lib/core/src/health_monitor.cpp | 296/305 | 97.0% |
| lib/core/src/json_api.cpp | 310/328 | 94.5% |
| lib/core/src/json_writer.cpp | 268/280 | 95.7% |
| lib/core/src/legacy_import.cpp | 485/519 | 93.4% |
| lib/core/src/line_assembler.cpp | 64/64 | 100.0% |
| lib/core/src/link_policy.cpp | 281/282 | 99.6% |
| lib/core/src/mqtt_topics.cpp | 504/525 | 96.0% |
| lib/core/src/poll_planner.cpp | 238/240 | 99.2% |
| lib/core/src/stm_codec.cpp | 948/948 | 100.0% |
| lib/core/src/stm_flasher.cpp | 936/1036 | 90.3% |
| lib/core/src/valve_model.cpp | 465/474 | 98.1% |
| lib/core/src/version.cpp | 208/214 | 97.2% |

## Status: paused

The full ESP run takes ~8 h on a 4-core machine (9267 mutants). After round 2 more
tests were added to kill survivors (`test_*.cpp`, new `test_common.cpp`). Round 3
verified the first 4891 mutants (auth … legacy_import) before it was stopped on
purpose: **206 survivors vs 248 in round 2 for the same range**, i.e. the new tests
kill ~42 more. The rest was not re-verified; its numbers above are from round 2.

CI still runs mutation on every PR (changed core files only), and a full run weekly
and on release tags, so the score is tracked continuously.

## Survivors left after round 2 (417)

| file | survivors | by operator |
|---|---|---|
| config.cpp | 106 | const 56, rel 29, bool 9, arith 9, log 3 |
| stm_flasher.cpp | 100 | const 63, rel 22, arith 6, bool 4, asgn 2, negcond 2, log 1 |
| ha_discovery.cpp | 56 | const 35, arith 12, rel 4, bool 3, log 2 |
| legacy_import.cpp | 34 | const 28, rel 4, bool 2 |
| mqtt_topics.cpp | 21 | const 12, rel 5, arith 2, bool 2 |
| json_api.cpp | 18 | const 15, rel 2, log 1 |
| auth.cpp | 16 | const 9, rel 3, bool 2, arith 1, log 1 |
| common.cpp | 16 | const 12, rel 3, arith 1 |
| json_writer.cpp | 12 | const 8, bool 4 |
| health_monitor.cpp | 9 | rel 6, const 2, bool 1 |
| valve_model.cpp | 9 | rel 6, const 3 |
| event_log.cpp | 7 | const 7 |
| version.cpp | 6 | rel 5, const 1 |
| calib_schedule.cpp | 4 | const 4 |
| poll_planner.cpp | 2 | const 1, rel 1 |
| link_policy.cpp | 1 | rel 1 |

Typical remaining survivors: string-literal lengths in JSON/discovery payloads,
`>=` vs `>` on clamps where both sides are equal only at unreachable limits,
retval/const mutants in error branches not observable through the public API.

## Next steps

1. Resume the run (kill cache keeps already-killed mutants):
   `python3 tools/mutation/mutate.py --config tools/mutation/esp32.json --jobs 2`
2. Triage survivors of `ha_discovery.cpp`, `config.cpp`, `stm_flasher.cpp`,
   `legacy_import.cpp` first (largest counts); add tests or `// NOMUTATE: reason`
   for proven equivalents.
