# Mutation testing: software_stm32/lib/core

Bar: overall score >= 85 %, every file >= 75 %.
Result: **96.8 % overall (1062 / 1097 killed), lowest file 89.5 %, all green.**

Tool: `tools/mutation/mutate.py`, config `tools/mutation/stm32.json`, report
`tools/mutation/stm32.report.json`. Each mutant is built with the sanitizer build
of `test/native` (ASan + UBSan, `-Werror`), runs the module's own test file first
(`core_tests -sf=*test_<stem>.cpp`), then the full suite. Build errors, crashes
and timeouts count as killed.

## Reproduce

```sh
cd <repo>
python3 tools/mutation/mutate.py --config tools/mutation/stm32.json --jobs 2
# only some files:  ... --files lib/core/src/end_stop_detector.cpp
# ignore the kill cache (tools/mutation/stm32.cache.json):  ... --no-cache
```

Unit tests (sanitizers on):

```sh
cmake -S software_stm32/test/native -B build/native-stm32
cmake --build build/native-stm32 -j 2
ctest --test-dir build/native-stm32 --output-on-failure
```

## Per file (final run)

| file | score | killed/total | surviving (all equivalent) | excluded by NOMUTATE |
|---|---|---|---|---|
| arg_parser.cpp | 100.0 % | 175/175 | 0 | 8 mutants (4 lines) |
| buf_writer.cpp | 98.6 % | 137/139 | 2 | 6 mutants (2 lines) |
| calibration.cpp | 95.0 % | 96/101 | 5 | 0 |
| eeprom_layout.cpp | 100.0 % | 61/61 | 0 | 0 |
| end_stop_detector.cpp | 89.5 % | 68/76 | 8 | 0 |
| line_assembler.cpp | 98.6 % | 71/72 | 1 | 0 |
| motor_params.cpp | 94.2 % | 81/86 | 5 | 0 |
| move_classifier.cpp | 91.9 % | 34/37 | 3 | 0 |
| onewire_check.cpp | 100.0 % | 38/38 | 0 | 0 |
| profile_recorder.cpp | 94.5 % | 69/73 | 4 | 0 |
| replies.cpp | 97.2 % | 35/36 | 1 | 0 |
| replies_v2.cpp | 100.0 % | 43/43 | 0 | 0 |
| retry_backoff.cpp | 91.4 % | 32/35 | 3 | 0 |
| system_stats.cpp | 100.0 % | 40/40 | 0 | 0 |
| target_rejection.cpp | 100.0 % | 13/13 | 0 | 0 |
| tokenizer.cpp | 95.8 % | 69/72 | 3 | 0 |
| **total** | **96.8 %** | **1062/1097** | **35** | **14** |

Mutants on NOMUTATE lines are not generated, so they do not appear in the totals.
The excluded mutants are 4.4 % (arg_parser) and 4.1 % (buf_writer) of those files,
under the 5 % limit.

## NOMUTATE lines

| file:line | code | reason |
|---|---|---|
| arg_parser.cpp:28 | `return -1;` (hexValue) | callers only test `hexValue() < 0`, so any negative sentinel behaves the same |
| arg_parser.cpp:35 | `uint32_t value = 0;` (parseU32) | dead initial value: parseMagnitude() overwrites it on success, and it is not used on failure |
| arg_parser.cpp:50 | `uint32_t magnitude = 0;` (parseI32) | same as line 35 |
| arg_parser.cpp:65 | `uint8_t parsed[8];` | scratch array size: only indices 0..7 are used, so a larger size is invisible (smaller sizes were killed by ASan before the line was marked) |
| buf_writer.cpp:50 | `char digits[10];` | buffer size: all indexing is relative to `sizeof(digits)`, so a larger buffer is invisible |
| buf_writer.cpp:61 | `char text[11];` | same, relative to `sizeof(text)` |

## Surviving equivalent mutants (not marked)

These are not marked because the lines also hold killed mutants (a marker
excludes the whole line) or because marking them would exceed the 5 % limit.

| file:line | mutant | why no behaviour can differ |
|---|---|---|
| buf_writer.cpp:64 | `v < 0` -> `v < 1` | v == 0 gives magnitude `0u - 0 == 0`; the sign is decided by a separate `v < 0` |
| buf_writer.cpp:78 | `text[2]` -> `text[3]` | array size; only 2 bytes are appended |
| calibration.cpp:8 | `<` -> `<=` | mean == floor gives the same value |
| calibration.cpp:26 | `bound <= 0` -> `bound < 0` | bound 0 grows to `0 * x / 100 == 0` |
| calibration.cpp:29 | `bound >= cap` -> `>` | bound == cap: min(grown, cap) == cap == bound |
| calibration.cpp:38 | `mean > 0xFFFF` -> `>=` | mean == 0xFFFF gives 0xFFFF either way |
| calibration.cpp:46 | `sum < 0` -> `sum < 1` | `-0 == 0` |
| end_stop_detector.cpp:7 (2) | `v < 0` -> `<=` / `v < 1` | `-0 == 0` |
| end_stop_detector.cpp:23/24 | `raw > kRawLimit` -> `>=` (and `<` -> `<=`) | raw == limit is set to the limit |
| end_stop_detector.cpp:25 (3) | debounce cap 255 -> 254/256, `<` -> `<=` | debounce_ is private and only compared with `> 250`; any cap above 250 behaves the same |
| end_stop_detector.cpp:32 | `magnitude > peak_` -> `>=` | an equal value replaces an equal peak |
| line_assembler.cpp:19 | `len_(0)` -> `len_(1)` | the constructor body calls clearBuffer(), which sets len_ = 0 |
| motor_params.cpp:9, 12 (4) | static_assert conditions relaxed | compile-time checks only; relaxing a true assertion changes no code (tightening them fails the build and is killed) |
| motor_params.cpp:32 | `v > r.max` -> `>=` | v == max is set to max |
| move_classifier.cpp:10 (2) | `pct > 100` -> `>=` / `> 99` | pct 100 is capped to 100, the same value |
| move_classifier.cpp:55 | `peak <= 0` -> `< 0` | saturate16(0) == 0 |
| profile_recorder.cpp:10 | `v < 0` -> `v < 1` | `-0 == 0` |
| profile_recorder.cpp:39 | `spacing_ < kMaxSpacing` -> `<=` | unreachable: at spacing 0x8000 16-bit counts fall into at most 2 cells, so the loop already stopped on `size_ == kProfileSamples` |
| profile_recorder.cpp:41 | `kept = 0` -> `kept = 1` | samples_[0] is always kept in place, so starting at 1 gives the same array |
| profile_recorder.cpp:50 | `size_ ? ... : 0` -> `: 1` | unreachable: compact() only runs on a full buffer and always keeps >= 1 sample |
| replies.cpp:11 | `sizeof(fields[0])` -> `sizeof(fields[1])` | same element size |
| retry_backoff.cpp:7 | `max < first_` -> `<=` | equal values pick the same value |
| retry_backoff.cpp:9, 22 | `remaining_ = 0` -> `1` | remaining_ is only read when interval_ != 0, and failed() reloads it first |
| tokenizer.cpp:33 | `maxArgs > kMaxArgs` -> `>=` | maxArgs == kMaxArgs is set to itself |
| tokenizer.cpp:68, 75 | `uint32_t value = 0` -> `1` | dead initial value; argU32() sets it on success, and it is not used on failure |

## Tests added in this pass

- test_calibration: escalation of bound 1; a 1 mA stroke mean counts as valid.
- test_end_stop_detector: filter held at exactly 600 (safety limit) and 1000 (hard
  limit), current equal to the low bound, overCount saturates at 255, raw clamp is
  exactly +-100000 (checked against a reference filter).
- test_move_classifier: early check only for a run to the end stop; learned travel
  1; the 100 % cap (pct 99/100/101/200 at the 1/2 boundary); peak 1 is kept.
- test_profile_recorder: after compaction the next grid point follows the last kept sample.
- test_retry_backoff: doubling boundary at exactly half the maximum (3 -> 6 -> 7 and 40 -> 80 -> 160 -> 200).
- test_tokenizer: default state; arguments of a previous line are not visible (arg, argU32, argI32).

## Tool fixes made during this work (tools/mutation/mutate.py)

- `run()`: test output is decoded with `errors="replace"`. A mutant that printed
  non-UTF-8 bytes raised `UnicodeDecodeError` and killed the worker threads, which
  left most mutants unrun.
- `retval` mutants: the original text is now taken from the source rather than the
  copy with strings and comments blanked out, and `apply()` matches against the
  whole text, so it works across line breaks. Before this fix, every `return`
  whose expression had a char or string literal or spanned several lines failed
  to apply and was silently counted as "equivalent" (20 mutants in this library).
  All 20 are now run and killed.
