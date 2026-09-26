# Mutation testing: stm32-glue

Scope: `software_stm32/src`, STM32 glue (Arduino code on host fakes).
Target: >= 95 % overall and >= 95 % for every file.
Result: **97.92 % overall (2639 / 2695 killed), lowest file `src/sysstat.cpp` 95.65 %, gate passed.**
Measured on commit `c79fecd` (2026-09-26), 6 workers, 10 min.

Tool: `tools/mutation/mutate.py`, config `tools/mutation/stm32-glue.json`. Every mutant is built
with the sanitizer build of the native tests (ASan + UBSan, `-Werror`) and runs the tests of its
module first, then the whole suite. A failed test, a crash or a timeout kills the mutant.
Not counted: stillborn mutants (they do not compile), mutants in code the native build does not
compile, and equivalent mutants. The 99 equivalent mutants are listed with a reason in
`tools/mutation/equivalents/stm32-glue/`; lines marked `// NOMUTATE` get no mutants.

## Reproduce

```sh
bash tools/native/docker.sh mutate stm32-glue            # this suite
bash tools/native/docker.sh mutate stm32-glue --files <file>
bash tools/native/docker.sh mutate-all                 # all four suites and the gate table
```

## Per file

| file | score | killed | survived | timeout | equivalent | stillborn | not compiled |
|---|---|---|---|---|---|---|---|
| `src/app.cpp` | 98.43 % | 565 | 9 | 0 | 7 | 16 | 0 |
| `src/communication.cpp` | 97.76 % | 652 | 15 | 3 | 26 | 30 | 0 |
| `src/eeprom.cpp` | 96.00 % | 96 | 4 | 0 | 1 | 1 | 0 |
| `src/i2c_bus.cpp` | 100.00 % | 9 | 0 | 0 | 0 | 0 | 0 |
| `src/main.cpp` | 100.00 % | 78 | 0 | 1 | 0 | 0 | 1 |
| `src/motor.cpp` | 96.25 % | 667 | 26 | 0 | 40 | 3 | 75 |
| `src/otasupport.cpp` | 100.00 % | 63 | 0 | 1 | 0 | 0 | 0 |
| `src/owDevices.cpp` | 99.50 % | 198 | 1 | 0 | 14 | 0 | 28 |
| `src/sysstat.cpp` | 95.65 % | 22 | 1 | 0 | 0 | 1 | 0 |
| `src/terminal.cpp` | 100.00 % | 282 | 0 | 2 | 11 | 18 | 0 |
| **total** | **97.92 %** | 2632 | 56 | 7 | 99 | 69 | 104 |

## Surviving mutants (56)

<details><summary>list</summary>

- `src/app.cpp:134:10` retval `counted` -> `0`
- `src/app.cpp:305:10` negcond `(` -> `(!`
- `src/app.cpp:501:8` const `0` -> `1`
- `src/app.cpp:652:50` rel `<` -> `<=`
- `src/app.cpp:718:53` log `||` -> `&&`
- `src/app.cpp:790:15` const `0` -> `1`
- `src/app.cpp:814:35` const `1` -> `2`
- `src/app.cpp:824:13` rel `>=` -> `>`
- `src/app.cpp:824:31` log `||` -> `&&`
- `src/communication.cpp:76:34` const `0` -> `1`
- `src/communication.cpp:81:54` arith `+` -> `-`
- `src/communication.cpp:81:56` const `1` -> `0`
- `src/communication.cpp:81:56` const `1` -> `2`
- `src/communication.cpp:185:18` rel `<` -> `<=`
- `src/communication.cpp:192:12` rel `>=` -> `>`
- `src/communication.cpp:201:18` rel `<` -> `<=`
- `src/communication.cpp:345:52` arith `+` -> `-`
- `src/communication.cpp:345:54` const `1` -> `0`
- `src/communication.cpp:345:54` const `1` -> `2`
- `src/communication.cpp:439:26` rel `<` -> `<=`
- `src/communication.cpp:629:32` const `3` -> `0`
- `src/communication.cpp:629:32` const `3` -> `2`
- `src/communication.cpp:629:34` log `&&` -> `||`
- `src/communication.cpp:629:51` const `5` -> `6`
- `src/eeprom.cpp:66:37` const `0` -> `1`
- `src/eeprom.cpp:69:30` bool `false` -> `true`
- `src/eeprom.cpp:71:36` const `0` -> `1`
- `src/eeprom.cpp:137:23` bool `true` -> `false`
- `src/motor.cpp:162:32` const `17` -> `16`
- `src/motor.cpp:163:33` const `17` -> `18`
- `src/motor.cpp:466:29` rel `<` -> `<=`
- `src/motor.cpp:913:72` log `||` -> `&&`
- `src/motor.cpp:995:45` bool `true` -> `false`
- `src/motor.cpp:1056:54` const `0` -> `1`
- `src/motor.cpp:1087:18` const `12` -> `11`
- `src/motor.cpp:1210:32` const `0` -> `1`
- `src/motor.cpp:1227:44` const `0` -> `1`
- `src/motor.cpp:1238:39` const `0` -> `1`
- `src/motor.cpp:1239:39` const `0` -> `1`
- `src/motor.cpp:1241:42` const `0` -> `1`
- `src/motor.cpp:1248:39` const `0` -> `1`
- `src/motor.cpp:1249:38` const `0` -> `1`
- `src/motor.cpp:1284:50` const `0` -> `1`
- `src/motor.cpp:1298:42` rel `<` -> `<=`
- `src/motor.cpp:1298:81` rel `>` -> `>=`
- `src/motor.cpp:1343:24` negcond `(` -> `(!`
- `src/motor.cpp:1343:37` rel `>` -> `>=`
- `src/motor.cpp:1343:39` const `50` -> `0`
- `src/motor.cpp:1343:39` const `50` -> `49`
- `src/motor.cpp:1392:44` const `0` -> `1`
- `src/motor.cpp:1397:42` const `0` -> `1`
- `src/motor.cpp:1398:38` const `0` -> `1`
- `src/motor.cpp:1612:91` const `200` -> `201`
- `src/motor.cpp:1632:21` const `0` -> `1`
- `src/owDevices.cpp:157:42` const `8` -> `0`
- `src/sysstat.cpp:13:25` bool `false` -> `true`

</details>
